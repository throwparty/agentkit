//! The sandbox view: what paths it holds, how they are validated, and the argv
//! that builds it.
//!
//! Validation and rendering take the same value, so the object that was
//! checked is the object that bwrap mounts. Each accepted path is opened once
//! and mounted through that descriptor, which closes the gap between the check
//! and the mount.

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf};

use crate::sandbox::contract::Error;

/// Where the sandbox finds the workspace.
pub const WORKSPACE: &str = "/src";

/// Where the sandbox finds its home.
pub const HOME: &str = "/home/litterbox";

/// How a path appears in the sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Read-only, for the granted host trees and the baseline.
    ReadOnly,
    /// Read-write, for the workspace.
    ReadWrite,
}

/// One host path the sandbox can see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// The host path, which is also the sandbox path.
    pub path: PathBuf,
    /// How the path is mounted.
    pub access: Access,
}

/// A validated mount and the descriptor it will be mounted through.
#[derive(Debug)]
pub struct Pinned {
    /// The mount.
    pub mount: Mount,
    /// The descriptor, opened once and never re-resolved.
    pub file: File,
}

impl Pinned {
    /// The descriptor's number.
    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }
}

/// The mounts a sandbox was given, ready to render.
#[derive(Debug, Default)]
pub struct View {
    /// Every host path, in the order it will be mounted.
    pub pinned: Vec<Pinned>,
}

/// The paths a contributed path must stay away from.
#[derive(Clone, Debug, Default)]
pub struct Boundaries {
    /// The host home directory.
    pub home: PathBuf,
    /// The metadata directory.
    pub metadata: PathBuf,
    /// The backend's state directory.
    pub state: PathBuf,
}

impl Boundaries {
    /// Every path a contributed mount must not resolve to or through.
    fn protected(&self) -> Vec<&Path> {
        vec![
            Path::new("/"),
            Path::new("/etc"),
            Path::new("/run"),
            Path::new("/var"),
            Path::new("/tmp"),
            Path::new("/proc"),
            Path::new("/sys"),
            Path::new("/dev"),
            self.home.as_path(),
            self.metadata.as_path(),
            self.state.as_path(),
        ]
    }
}

/// One path the view builder is asked to mount.
#[derive(Clone, Debug)]
pub struct Requested {
    /// The host path.
    pub path: PathBuf,
    /// Whether the path is contributed rather than trusted.
    pub contributed: bool,
}

impl View {
    /// Validate and pin the requested paths.
    ///
    /// Contributed paths get the boundary rule set: absolute, no protected
    /// root, no socket, device or FIFO, and no overlap with the workspace.
    /// Their content is the contributing layer's concern; this backend only
    /// refuses a path that would widen the boundary the view draws. Trusted
    /// paths, which are the baseline and the mounts this backend resolved
    /// itself, only get the pinning rule.
    pub fn build(
        requested: &[Requested],
        workspace: &Path,
        boundaries: &Boundaries,
    ) -> Result<Self, Error> {
        let mut pinned = Vec::with_capacity(requested.len());
        let mut seen = std::collections::BTreeSet::new();
        for request in requested {
            // A trusted path may be a symlink the host owns, such as a library
            // alias or a store path. Resolve it to the file to open, but mount
            // it under the name the sandbox looks it up by; replacing the name
            // with the target would leave the name missing from the view.
            let target = if request.contributed {
                request.path.clone()
            } else {
                std::fs::canonicalize(&request.path).unwrap_or_else(|_| request.path.clone())
            };
            if !seen.insert(request.path.clone()) {
                continue;
            }
            if request.contributed {
                check_contributed(&request.path, workspace, boundaries)?;
            } else {
                check_trusted(&target, boundaries)?;
            }
            let file = pin(&target).map_err(|error| Error::Misconfigured {
                message: format!("{} cannot be mounted: {error}", request.path.display()),
            })?;
            let metadata = file.metadata().map_err(|error| Error::Misconfigured {
                message: format!("{} cannot be inspected: {error}", request.path.display()),
            })?;
            if metadata.file_type().is_socket()
                || metadata.file_type().is_fifo()
                || metadata.file_type().is_char_device()
                || metadata.file_type().is_block_device()
            {
                return Err(Error::Misconfigured {
                    message: format!(
                        "{} is not a file or directory; a sandbox mount binds neither",
                        request.path.display()
                    ),
                });
            }
            pinned.push(Pinned {
                mount: Mount {
                    path: request.path.clone(),
                    access: Access::ReadOnly,
                },
                file,
            });
        }
        Ok(Self { pinned })
    }
}

/// The rule for a path a project contributed.
fn check_contributed(path: &Path, workspace: &Path, boundaries: &Boundaries) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(Error::Misconfigured {
            message: format!("{} is not an absolute path", path.display()),
        });
    }
    // A lexical check is only sound on a normalised path: `..` would let a
    // contribution name a protected root without spelling it.
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::Misconfigured {
            message: format!("{} is not a normalised path", path.display()),
        });
    }
    for protected in boundaries.protected() {
        // A contribution may be neither a protected root, nor anything inside
        // one, nor an ancestor of one. Two exceptions: every absolute path
        // starts with the root, and /tmp is a shared scratch root where a
        // specific subtree is a legitimate grant.
        let scratch = protected == Path::new("/tmp");
        let inside = protected != Path::new("/") && !scratch && path.starts_with(protected);
        let ancestor = protected.starts_with(path);
        if path == protected || inside || ancestor {
            return Err(Error::Misconfigured {
                message: format!("{} is a protected path", path.display()),
            });
        }
    }
    if overlaps(path, workspace) {
        return Err(Error::Misconfigured {
            message: format!(
                "{} overlaps the workspace {}, which is the only writable tree",
                path.display(),
                workspace.display()
            ),
        });
    }
    Ok(())
}

/// The rule for a path the backend resolved itself.
fn check_trusted(path: &Path, boundaries: &Boundaries) -> Result<(), Error> {
    if path == Path::new("/") {
        return Err(Error::Misconfigured {
            message: "the sandbox root cannot be a bind".to_string(),
        });
    }
    if path.starts_with(&boundaries.home) {
        return Err(Error::Misconfigured {
            message: format!("{} is inside the host home", path.display()),
        });
    }
    if path.starts_with(&boundaries.state) || path.starts_with(&boundaries.metadata) {
        return Err(Error::Misconfigured {
            message: format!("{} is inside litterbox's own state", path.display()),
        });
    }
    Ok(())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

/// Open a path once, refusing symlinks and magic links.
fn pin(path: &Path) -> io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: open_how is a plain struct of integers, for which all-zero is a
    // valid value.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_PATH | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: the pointers are valid for the duration of the call and the
    // struct matches the kernel's layout for open_how.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is owned by this value from here on.
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

/// What the renderer needs that is not part of the view.
#[derive(Clone, Debug)]
pub struct RenderOptions<'a> {
    /// The bwrap binary.
    pub bwrap: &'a Path,
    /// The holder command, bound into the view.
    pub holder: &'a Path,
    /// The argument the holder runs with.
    pub holder_arguments: &'a [String],
    /// The host directory bound at the workspace path.
    pub workspace_host: &'a Path,
    /// The descriptor bwrap reports its status on.
    pub status_fd: i32,
    /// Whether the sandbox gets its own network namespace.
    pub network: crate::backends::bubblewrap::backend::Network,
    /// The links to create in the private root, as `(target, link)`.
    pub symlinks: &'a [(PathBuf, PathBuf)],
}

impl View {
    /// The mounts this view holds, in render order.
    pub fn mounts(&self) -> Vec<Mount> {
        self.pinned
            .iter()
            .map(|pinned| pinned.mount.clone())
            .collect()
    }

    /// The descriptors the mechanism is handed, in render order.
    ///
    /// Each is close-on-exec, so only the launch that clears the flag inherits
    /// it; nothing else the backend spawns can recover the mount.
    pub fn descriptors(&self) -> Vec<i32> {
        self.pinned.iter().map(Pinned::fd).collect()
    }

    /// Render bwrap's argv for this view.
    pub fn render(&self, options: &RenderOptions<'_>) -> Vec<String> {
        let mut argv: Vec<String> = Vec::new();
        let mut push = |value: String| argv.push(value);
        push(options.bwrap.to_string_lossy().to_string());
        for flag in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
        ] {
            push(flag.to_string());
        }
        if options.network == crate::backends::bubblewrap::backend::Network::Denied {
            push("--unshare-net".to_string());
        }
        for flag in ["--disable-userns", "--hostname", "litterbox"] {
            push(flag.to_string());
        }
        for flag in ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"] {
            push(flag.to_string());
        }
        for flag in ["--tmpfs", "/home", "--dir", HOME] {
            push(flag.to_string());
        }
        // A shebang names `/bin/sh` or `/usr/bin/env` by absolute path, so the
        // links have to exist before any script runs, whatever the PATH says.
        for flag in ["--dir", "/bin", "--dir", "/usr", "--dir", "/usr/bin"] {
            push(flag.to_string());
        }
        for (target, link) in options.symlinks {
            push("--symlink".to_string());
            push(target.to_string_lossy().to_string());
            push(link.to_string_lossy().to_string());
        }
        for pinned in &self.pinned {
            push("--ro-bind".to_string());
            push(format!("/proc/self/fd/{}", pinned.fd()));
            push(pinned.mount.path.to_string_lossy().to_string());
        }
        push("--bind".to_string());
        push(options.workspace_host.to_string_lossy().to_string());
        push(WORKSPACE.to_string());
        push("--chdir".to_string());
        push(WORKSPACE.to_string());
        push("--json-status-fd".to_string());
        push(options.status_fd.to_string());
        push("--".to_string());
        push(options.holder.to_string_lossy().to_string());
        for argument in options.holder_arguments {
            push(argument.clone());
        }
        argv
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Access, Boundaries, Mount, RenderOptions, Requested, View, WORKSPACE};
    use crate::sandbox::contract::Error;

    fn boundaries(root: &Path) -> Boundaries {
        Boundaries {
            home: root.join("home"),
            metadata: root.join("metadata"),
            state: root.join("state"),
        }
    }

    fn sandbox_layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let grant = temporary.path().join("grant");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(grant.join("abc")).expect("granted path");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(grant.join("abc").join("data"), b"x").expect("file");
        (temporary, grant, workspace)
    }

    #[test]
    fn a_contributed_path_is_pinned_and_read_only() {
        let (temporary, grant, workspace) = sandbox_layout();
        let requested = vec![Requested {
            path: grant.join("abc"),
            contributed: true,
        }];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        assert_eq!(view.pinned.len(), 1);
        assert_eq!(view.pinned[0].mount.access, Access::ReadOnly);
        assert_eq!(view.pinned[0].mount.path, grant.join("abc"));
    }

    #[test]
    fn the_argv_is_the_one_the_policy_expects() {
        let (temporary, grant, workspace) = sandbox_layout();
        let requested = vec![
            Requested {
                path: grant.join("abc"),
                contributed: true,
            },
            Requested {
                path: grant.clone(),
                contributed: true,
            },
        ];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        let options = RenderOptions {
            bwrap: Path::new("/bin/bwrap"),
            holder: Path::new("/bin/sleep"),
            holder_arguments: &[String::from("2147483647")],
            workspace_host: &workspace,
            status_fd: 3,
            network: crate::backends::bubblewrap::backend::Network::Denied,
            symlinks: &[],
        };
        let argv: Vec<String> = view
            .render(&options)
            .into_iter()
            .map(|argument| normalise_descriptors(&argument))
            .collect();

        let expected: Vec<String> = vec![
            "/bin/bwrap".to_string(),
            "--unshare-user".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-ipc".to_string(),
            "--unshare-uts".to_string(),
            "--unshare-cgroup".to_string(),
            "--unshare-net".to_string(),
            "--disable-userns".to_string(),
            "--hostname".to_string(),
            "litterbox".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--tmpfs".to_string(),
            "/home".to_string(),
            "--dir".to_string(),
            "/home/litterbox".to_string(),
            "--dir".to_string(),
            "/bin".to_string(),
            "--dir".to_string(),
            "/usr".to_string(),
            "--dir".to_string(),
            "/usr/bin".to_string(),
            "--ro-bind".to_string(),
            "/proc/self/fd/<n>".to_string(),
            grant.join("abc").to_string_lossy().to_string(),
            "--ro-bind".to_string(),
            "/proc/self/fd/<n>".to_string(),
            grant.to_string_lossy().to_string(),
            "--bind".to_string(),
            workspace.to_string_lossy().to_string(),
            WORKSPACE.to_string(),
            "--chdir".to_string(),
            WORKSPACE.to_string(),
            "--json-status-fd".to_string(),
            "3".to_string(),
            "--".to_string(),
            "/bin/sleep".to_string(),
            "2147483647".to_string(),
        ];
        assert_eq!(argv, expected);
    }

    #[test]
    fn a_duplicate_path_is_mounted_once() {
        let (temporary, grant, workspace) = sandbox_layout();
        let path = grant.join("abc");
        let requested = vec![
            Requested {
                path: path.clone(),
                contributed: true,
            },
            Requested {
                path,
                contributed: true,
            },
        ];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        assert_eq!(view.pinned.len(), 1);
    }

    #[test]
    fn a_relative_path_is_refused() {
        let (temporary, _grant, workspace) = sandbox_layout();
        let requested = vec![Requested {
            path: PathBuf::from("relative"),
            contributed: true,
        }];
        let error = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect_err("refused");
        match error {
            Error::Misconfigured { message } => {
                assert!(message.contains("absolute"), "{message}");
            }
            other => panic!("expected a configuration error, got {other:?}"),
        }
    }

    #[test]
    fn a_protected_root_is_refused() {
        let (temporary, _grant, workspace) = sandbox_layout();
        let requested = vec![Requested {
            path: PathBuf::from("/etc"),
            contributed: true,
        }];
        let error = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect_err("refused");
        assert!(error.to_string().contains("protected"), "{error}");
    }

    #[test]
    fn a_path_overlapping_the_workspace_is_refused() {
        let (temporary, _grant, workspace) = sandbox_layout();
        let inner = workspace.join("inside");
        std::fs::create_dir_all(&inner).expect("dir");
        let requested = vec![Requested {
            path: inner,
            contributed: true,
        }];
        let error = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect_err("refused");
        assert!(error.to_string().contains("overlaps"), "{error}");
    }

    #[test]
    fn a_symlink_is_refused_rather_than_followed() {
        let (temporary, grant, workspace) = sandbox_layout();
        let link = grant.join("link");
        std::os::unix::fs::symlink(grant.join("abc"), &link).expect("symlink");
        let requested = vec![Requested {
            path: link,
            contributed: true,
        }];
        let error = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect_err("refused");
        assert!(error.to_string().contains("cannot be mounted"), "{error}");
    }

    #[test]
    fn a_socket_is_refused() {
        let (temporary, grant, workspace) = sandbox_layout();
        let socket = grant.join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("listener");
        let requested = vec![Requested {
            path: socket,
            contributed: true,
        }];
        let error = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect_err("refused");
        assert!(
            error.to_string().contains("not a file or directory"),
            "{error}"
        );
    }

    #[test]
    fn a_trusted_symlink_is_mounted_at_the_name_it_was_given() {
        let (temporary, grant, workspace) = sandbox_layout();
        let alias = grant.join("libfoo.so.1");
        std::os::unix::fs::symlink(grant.join("abc"), &alias).expect("symlink");
        let requested = vec![Requested {
            path: alias.clone(),
            contributed: false,
        }];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        assert_eq!(
            view.pinned[0].mount.path, alias,
            "the sandbox must find the name the loader asks for"
        );
    }

    #[test]
    fn a_trusted_path_outside_the_grants_is_allowed() {
        let (temporary, _grant, workspace) = sandbox_layout();
        let shell = temporary.path().join("shell");
        std::fs::write(&shell, b"#!").expect("file");
        let requested = vec![Requested {
            path: shell.clone(),
            contributed: false,
        }];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        assert_eq!(view.pinned.len(), 1);
    }

    #[test]
    fn the_mount_type_is_carried() {
        let mount = Mount {
            path: PathBuf::from("/opt/toolchain/abc"),
            access: Access::ReadWrite,
        };
        assert_eq!(mount.access, Access::ReadWrite);
    }

    #[test]
    fn the_argv_carries_every_invariant() {
        let (temporary, grant, workspace) = sandbox_layout();
        let requested = vec![Requested {
            path: grant.join("abc"),
            contributed: true,
        }];
        let view = View::build(&requested, &workspace, &boundaries(temporary.path()))
            .expect("view builds");
        let options = RenderOptions {
            bwrap: Path::new("/bin/bwrap"),
            holder: Path::new("/bin/sleep"),
            holder_arguments: &[String::from("2147483647")],
            workspace_host: &workspace,
            status_fd: 3,
            network: crate::backends::bubblewrap::backend::Network::Denied,
            symlinks: &[],
        };
        let argv = view.render(&options);
        let contains = |flag: &str| argv.iter().any(|argument| argument == flag);

        for namespace in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--unshare-net",
        ] {
            assert!(contains(namespace), "missing {namespace}");
        }
        assert!(contains("--unshare-net"), "the default denies the network");

        let mut shared = options.clone();
        shared.network = crate::backends::bubblewrap::backend::Network::Host;
        let shared_argv = view.render(&shared);
        assert!(
            !shared_argv
                .iter()
                .any(|argument| argument == "--unshare-net"),
            "host networking keeps the host namespace"
        );
        assert!(
            shared_argv
                .iter()
                .any(|argument| argument == "--unshare-user"),
            "the other namespaces are still unshared"
        );
        assert!(
            contains("--disable-userns"),
            "nested user namespaces are not blocked"
        );
        assert!(
            !contains("--die-with-parent"),
            "the sandbox is tied to the caller"
        );
        assert!(!contains("--as-pid-1"), "the pid-1 process must reap");
        assert!(
            !contains("--setenv"),
            "an environment value reached the argv"
        );
        assert!(
            !contains("--clearenv"),
            "the spawn carries the environment, not a flag"
        );
        assert!(contains("--proc") && contains("--dev") && contains("--tmpfs"));
        assert_eq!(
            argv.iter().filter(|argument| *argument == "--bind").count(),
            1,
            "the workspace must be the only writable host tree"
        );
        assert_eq!(
            argv.iter()
                .filter(|argument| *argument == "--ro-bind")
                .count(),
            1,
            "every other host path is read-only"
        );
    }

    fn normalise_descriptors(argument: &str) -> String {
        if let Some(rest) = argument.strip_prefix("/proc/self/fd/")
            && rest.chars().all(|character| character.is_ascii_digit())
        {
            return "/proc/self/fd/<n>".to_string();
        }
        argument.to_string()
    }
}
