//! The workspace a sandbox binds, and the transfers into and out of it.
//!
//! The orchestrator hands create a staged temporary tree, so the backend keeps
//! its own copy. Every transfer resolves the workspace root once and then walks
//! it descriptor by descriptor, refusing a link at every step, so a link the
//! sandbox planted cannot make a transfer read or write outside the tree.

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::sandbox::contract::{Error, SnapshotSelector};

/// The workspace's name, as a path component.
const WORKSPACE_COMPONENT: &str = "src";

/// The root of this backend's state, keyed by project and sandbox.
pub fn sandbox_directory(project: &str, name: &str) -> PathBuf {
    agentkit_common::data_dir("litterbox")
        .join("bubblewrap")
        .join(project)
        .join(name)
}

/// The workspace inside a sandbox's state directory.
pub fn workspace_directory(project: &str, name: &str) -> PathBuf {
    sandbox_directory(project, name).join("workspace")
}

/// Copy the staged tree into the sandbox's workspace.
///
/// The destination is not shared with a workload yet, so this copy is the one
/// transfer that may work from paths rather than descriptors. It still refuses
/// a link, because the staged tree decides what the sandbox comes to hold.
pub fn copy_from(staged: &Path, destination: &Path) -> Result<(), Error> {
    copy_tree(staged, destination)
        .map_err(|error| Error::failed(format!("the workspace could not be materialised: {error}")))
}

/// Copy a host path into the sandbox workspace.
pub fn transfer_in(root: &Path, source: &Path, destination: &str) -> Result<(), Error> {
    let root = Root::open(root)
        .map_err(|error| Error::failed(format!("the workspace cannot be opened: {error}")))?;
    let mut relative = components(destination, "upload into the sandbox")?;
    if let Ok(metadata) = std::fs::symlink_metadata(source)
        && metadata.is_file()
    {
        // The contract names a directory, so a file lands inside it rather than
        // replacing it.
        let name = source.file_name().ok_or_else(|| {
            Error::failed(format!("{} has no name to upload under", source.display()))
        })?;
        relative.push(name.to_os_string());
    }
    copy_in(&root, &relative, source).map_err(|error| {
        Error::failed(format!(
            "{} could not be uploaded: {error}",
            source.display()
        ))
    })
}

/// Copy a sandbox path out to the host.
///
/// A selector narrows the copy to the paths a snapshot carries; the walk asks
/// it before reading anything, so a rejected path never reaches the host.
pub fn transfer_out(
    root: &Path,
    source: &str,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    let root = Root::open(root)
        .map_err(|error| Error::failed(format!("the workspace cannot be opened: {error}")))?;
    let relative = components(source, "download from the sandbox")?;
    let Some((name, parents)) = relative.split_last() else {
        // The workspace itself: its contents merge into the destination.
        return copy_contents(&root.directory, Path::new(""), destination, selector);
    };
    let relative = parents
        .iter()
        .map(PathBuf::from)
        .chain([PathBuf::from(name)])
        .fold(PathBuf::new(), |mut path, part| {
            path.push(part);
            path
        });
    let parent = descend(&root, parents, false)
        .map_err(|error| Error::failed(format!("{source} cannot be reached: {error}")))?;
    let opened = match open_in(&parent, name, libc::O_PATH) {
        Ok(opened) => opened,
        // A link at the source is copied as a link, not followed.
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            if skipped(selector, &relative, false)? {
                return Ok(());
            }
            return copy_leaf(&parent, name, &relative, destination, selector).map_err(|error| {
                Error::failed(format!("{source} cannot be copied: {error}"))
            });
        }
        Err(error) => {
            return Err(Error::failed(format!(
                "{source} cannot be read: {error}"
            )));
        }
    };
    let metadata = opened.metadata().map_err(|error| {
        Error::failed(format!("{} cannot be read: {error}", root.path.display()))
    })?;
    if skipped(selector, &relative, metadata.is_dir())? {
        return Ok(());
    }
    if metadata.is_dir() {
        let directory = open_in(&parent, name, libc::O_RDONLY | libc::O_DIRECTORY)
            .map_err(|error| Error::failed(format!("{source} cannot be read: {error}")))?;
        copy_contents(&directory, &relative, destination, selector)
    } else {
        copy_leaf(&parent, name, &relative, destination, selector)
            .map_err(|error| Error::failed(format!("{source} cannot be copied: {error}")))
    }
}

/// Whether the selector keeps a path out of the copy.
fn skipped(
    selector: Option<&dyn SnapshotSelector>,
    relative: &Path,
    is_dir: bool,
) -> Result<bool, Error> {
    match selector {
        // The workspace root is never filtered: it is the thing being copied.
        Some(selector) if !relative.as_os_str().is_empty() => {
            Ok(!selector.selects(relative, is_dir)?)
        }
        _ => Ok(false),
    }
}

/// The workspace root, opened once and never resolved through again.
struct Root {
    /// The directory every path is resolved beneath.
    directory: std::fs::File,
    /// The path it was opened from, for diagnostics.
    path: PathBuf,
}

impl Root {
    fn open(path: &Path) -> io::Result<Self> {
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(path)?;
        Ok(Self {
            directory,
            path: path.to_path_buf(),
        })
    }
}

/// Resolve a sandbox path into the components below the workspace root.
///
/// An absolute path has to name the workspace itself, component by component;
/// anything else is refused rather than remapped into the workspace.
fn components(path: &str, operation: &str) -> Result<Vec<OsString>, Error> {
    let narrowed = Path::new(path);
    let mut pieces = narrowed.components();
    let mut names = Vec::new();
    if narrowed.is_absolute() {
        let _ = pieces.next();
        match pieces.next() {
            Some(Component::Normal(first)) if first.as_bytes() == WORKSPACE_COMPONENT.as_bytes() => {}
            _ => return Err(confinement(path, operation)),
        }
    }
    for piece in pieces {
        match piece {
            Component::Normal(name) => names.push(name.to_os_string()),
            Component::CurDir => {}
            _ => return Err(confinement(path, operation)),
        }
    }
    Ok(names)
}

fn confinement(path: &str, operation: &str) -> Error {
    Error::Denied {
        operation: operation.to_string(),
        reason: format!("'{path}' leaves the sandbox workspace"),
    }
}

/// Walk to a directory below the root, optionally creating the missing ones.
fn descend(root: &Root, relative: &[OsString], create: bool) -> io::Result<std::fs::File> {
    let mut current = root.directory.try_clone()?;
    for name in relative {
        current = match open_in(&current, name, libc::O_PATH | libc::O_DIRECTORY) {
            Ok(opened) => opened,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                mkdir_in(&current, name)?;
                open_in(&current, name, libc::O_PATH | libc::O_DIRECTORY)?
            }
            Err(error) => return Err(error),
        };
    }
    Ok(current)
}

/// Open one name inside a pinned directory without following a link.
fn open_in(directory: &std::fs::File, name: &OsStr, flags: i32) -> io::Result<std::fs::File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("a path component holds a NUL byte"))?;
    // SAFETY: open_how is a plain struct of integers, for which all-zero is a
    // valid value.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (flags | libc::O_CLOEXEC) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: the descriptor is a directory, the pointer outlives the call and
    // the struct matches the kernel's layout for open_how.
    let opened = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            name.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if opened < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is owned by this value from here on.
    Ok(unsafe { std::fs::File::from_raw_fd(opened as RawFd) })
}

fn mkdir_in(directory: &std::fs::File, name: &OsStr) -> io::Result<()> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("a path component holds a NUL byte"))?;
    // SAFETY: the descriptor is a directory and the name is a single component.
    if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn readlink_in(directory: &std::fs::File, name: &OsStr) -> io::Result<PathBuf> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("a path component holds a NUL byte"))?;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: the buffer is writable for its whole length and the name is one
    // component of the directory.
    let length = unsafe {
        libc::readlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_vec(buffer)))
}

/// Copy a host tree into the workspace at the given components.
fn copy_in(root: &Root, relative: &[OsString], source: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "{} is a symlink, which an upload does not follow",
            source.display()
        )));
    }
    if metadata.is_dir() {
        descend(root, relative, true)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let mut child = relative.to_vec();
            child.push(entry.file_name());
            copy_in(root, &child, &entry.path())?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a file or directory",
            source.display()
        )));
    }
    let Some((name, parents)) = relative.split_last() else {
        return Err(io::Error::other("the upload names no destination"));
    };
    let directory = descend(root, parents, true)?;
    let mut target = open_in(
        &directory,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
    )?;
    let mut reader = std::fs::File::open(source)?;
    io::copy(&mut reader, &mut target)?;
    target.set_permissions(metadata.permissions())?;
    Ok(())
}

/// Copy a directory's entries into an existing host directory.
fn copy_contents(
    directory: &std::fs::File,
    relative: &Path,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    std::fs::create_dir_all(destination).map_err(|error| {
        Error::failed(format!(
            "{} cannot be created: {error}",
            destination.display()
        ))
    })?;
    for name in entries(directory).map_err(|error| {
        Error::failed(format!("the workspace could not be read: {error}"))
    })? {
        copy_leaf(
            directory,
            &name,
            &relative.join(&name),
            destination,
            selector,
        )?;
    }
    Ok(())
}

/// Copy one entry of a directory into a host directory, keeping a link.
fn copy_leaf(
    directory: &std::fs::File,
    name: &OsStr,
    relative: &Path,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    let target = destination.join(name);
    let opened = match open_in(directory, name, libc::O_PATH) {
        Ok(opened) => opened,
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            if skipped(selector, relative, false)? {
                return Ok(());
            }
            let link = readlink_in(directory, name).map_err(|error| {
                Error::failed(format!("{} cannot be read: {error}", target.display()))
            })?;
            return std::os::unix::fs::symlink(link, &target).map_err(|error| {
                Error::failed(format!("{} cannot be created: {error}", target.display()))
            });
        }
        Err(error) => {
            return Err(Error::failed(format!(
                "{} cannot be read: {error}",
                target.display()
            )));
        }
    };
    let metadata = opened.metadata().map_err(|error| {
        Error::failed(format!("{} cannot be read: {error}", target.display()))
    })?;
    if skipped(selector, relative, metadata.is_dir())? {
        return Ok(());
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(&target).map_err(|error| {
            Error::failed(format!("{} cannot be created: {error}", target.display()))
        })?;
        let directory = open_in(directory, name, libc::O_RDONLY | libc::O_DIRECTORY)
            .map_err(|error| {
                Error::failed(format!("{} cannot be read: {error}", target.display()))
            })?;
        for entry in entries(&directory).map_err(|error| {
            Error::failed(format!("{} cannot be read: {error}", target.display()))
        })? {
            copy_leaf(
                &directory,
                &entry,
                &relative.join(&entry),
                &target,
                selector,
            )?;
        }
        return Ok(());
    }
    let mut source = open_in(directory, name, libc::O_RDONLY).map_err(|error| {
        Error::failed(format!("{} cannot be read: {error}", target.display()))
    })?;
    let mut copy = std::fs::File::create(&target).map_err(|error| {
        Error::failed(format!("{} cannot be created: {error}", target.display()))
    })?;
    io::copy(&mut source, &mut copy)
        .and_then(|_| copy.set_permissions(metadata.permissions()))
        .map_err(|error| Error::failed(format!("{} cannot be copied: {error}", target.display())))
}

/// The names in a directory, read through the descriptor rather than a path.
struct Entries {
    directory: *mut libc::DIR,
}

impl Entries {
    fn open(directory: &std::fs::File) -> io::Result<Self> {
        // `fdopendir` takes ownership of the descriptor, so hand it a duplicate.
        let duplicate = directory.try_clone()?;
        // SAFETY: the duplicate is a directory descriptor and is given up here.
        let opened = unsafe { libc::fdopendir(duplicate.into_raw_fd()) };
        if opened.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { directory: opened })
    }
}

impl Iterator for Entries {
    type Item = OsString;

    fn next(&mut self) -> Option<OsString> {
        loop {
            // SAFETY: the directory stream is open and owned by this value.
            let entry = unsafe { libc::readdir(self.directory) };
            if entry.is_null() {
                return None;
            }
            // SAFETY: the entry names a NUL-terminated string the stream owns.
            let name = unsafe { (*entry).d_name.as_ptr() };
            // SAFETY: the pointer is NUL-terminated by the kernel.
            let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            return Some(OsStr::from_bytes(name).to_os_string());
        }
    }
}

impl Drop for Entries {
    fn drop(&mut self) {
        // SAFETY: the stream was opened by `open` and is closed exactly once.
        unsafe { libc::closedir(self.directory) };
    }
}

fn entries(directory: &std::fs::File) -> io::Result<Entries> {
    Entries::open(directory)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other(format!(
            "{} is a symlink, which a transfer does not follow",
            from.display()
        )));
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            // Uploads and the staged copy refuse a link; only a download keeps
            // one, and it goes through `copy_leaf`.
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
        std::fs::set_permissions(to, metadata.permissions())?;
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{} is not a file or directory",
        from.display()
    )))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        WORKSPACE_COMPONENT, components, copy_from, sandbox_directory, transfer_in, transfer_out,
        workspace_directory,
    };
    use crate::backends::bubblewrap::view::WORKSPACE;
    use crate::sandbox::contract::SnapshotSelector;

    fn staged() -> (tempfile::TempDir, std::path::PathBuf) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let source = temporary.path().join("staged");
        std::fs::create_dir_all(source.join("src")).expect("dirs");
        std::fs::write(source.join("README.md"), b"hello").expect("file");
        std::fs::write(source.join("src").join("main.rs"), b"fn main() {}").expect("file");
        (temporary, source)
    }

    #[test]
    fn the_state_paths_are_keyed_by_project_and_name() {
        let directory = sandbox_directory("project", "demo");
        assert!(directory.ends_with("bubblewrap/project/demo"));
        assert_eq!(
            workspace_directory("project", "demo"),
            directory.join("workspace")
        );
    }

    #[test]
    fn the_workspace_name_matches_the_bound_path() {
        assert_eq!(WORKSPACE, format!("/{WORKSPACE_COMPONENT}"));
    }

    #[test]
    fn a_staged_tree_is_copied_with_its_contents() {
        let (temporary, source) = staged();
        let destination = temporary.path().join("workspace");
        copy_from(&source, &destination).expect("copied");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
        );
        assert!(destination.join("src").join("main.rs").is_file());
    }

    #[test]
    fn a_symlink_in_the_staged_tree_is_refused() {
        let (temporary, source) = staged();
        std::os::unix::fs::symlink("/etc", source.join("link")).expect("symlink");
        let destination = temporary.path().join("workspace");
        let error = copy_from(&source, &destination).expect_err("refused");
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn an_upload_writes_inside_the_workspace() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        let host = temporary.path().join("upload.txt");
        std::fs::write(&host, b"payload").expect("file");
        transfer_in(&root, &host, "docs").expect("uploaded");
        assert_eq!(
            std::fs::read(root.join("docs").join("upload.txt")).expect("read"),
            b"payload"
        );
    }

    #[test]
    fn a_file_upload_lands_inside_the_destination_directory() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("README.md"), b"hello").expect("existing");
        let host = temporary.path().join("notes.txt");
        std::fs::write(&host, b"notes").expect("file");
        transfer_in(&root, &host, "/src").expect("uploaded");
        assert_eq!(
            std::fs::read(root.join("notes.txt")).expect("read"),
            b"notes"
        );
        assert_eq!(
            std::fs::read(root.join("README.md")).expect("read"),
            b"hello",
            "the upload merges into a populated directory"
        );
    }

    #[test]
    fn an_absolute_upload_outside_the_workspace_is_refused() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        let host = temporary.path().join("upload.txt");
        std::fs::write(&host, b"payload").expect("file");
        for destination in ["/etc", "/src-other/file", "/etc/passwd"] {
            let error = transfer_in(&root, &host, destination).expect_err("refused");
            assert!(
                error.to_string().contains("leaves the sandbox workspace"),
                "{destination}: {error}"
            );
            assert!(
                !temporary.path().join("etc").exists(),
                "{destination} was remapped into the workspace"
            );
        }
    }

    #[test]
    fn an_upload_that_leaves_the_workspace_is_refused() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        let host = temporary.path().join("upload.txt");
        std::fs::write(&host, b"payload").expect("file");
        let error = transfer_in(&root, &host, "../outside.txt").expect_err("refused");
        assert!(
            error.to_string().contains("leaves the sandbox workspace"),
            "{error}"
        );
    }

    #[test]
    fn an_upload_through_a_link_is_refused() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        let elsewhere = temporary.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("dir");
        std::os::unix::fs::symlink(&elsewhere, root.join("link")).expect("symlink");
        let host = temporary.path().join("upload.txt");
        std::fs::write(&host, b"payload").expect("file");
        let error = transfer_in(&root, &host, "/src/link").expect_err("refused");
        assert!(!elsewhere.join("upload.txt").exists(), "{error}");
    }

    #[test]
    fn a_download_through_a_link_is_refused() {
        let (temporary, _source) = staged();
        let root = temporary.path().join("workspace");
        std::fs::create_dir_all(&root).expect("root");
        std::os::unix::fs::symlink("/etc", root.join("link")).expect("symlink");
        let destination = temporary.path().join("snapshot");
        let error =
            transfer_out(&root, "/src/link/passwd", &destination, None).expect_err("refused");
        assert!(!destination.join("passwd").exists(), "{error}");
    }

    #[test]
    fn a_directory_download_copies_the_tree_and_keeps_nested_links() {
        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        std::os::unix::fs::symlink("README.md", root.join("readme-link")).expect("symlink");
        std::os::unix::fs::symlink("../README.md", root.join("src").join("up-link"))
            .expect("symlink");
        let destination = temporary.path().join("snapshot");
        transfer_out(&root, "/src", &destination, None).expect("downloaded");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
        );
        assert_eq!(
            std::fs::read(destination.join("src").join("main.rs")).expect("read"),
            b"fn main() {}"
        );
        let link = destination.join("readme-link");
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(&link).expect("target"),
            Path::new("README.md")
        );
        let nested = destination.join("src").join("up-link");
        assert!(
            std::fs::symlink_metadata(&nested)
                .expect("link")
                .file_type()
                .is_symlink(),
            "a nested link is preserved"
        );
        assert_eq!(
            std::fs::read_link(&nested).expect("target"),
            Path::new("../README.md")
        );
    }

    #[test]
    fn a_download_drops_paths_the_selector_rejects() {
        struct RejectTarget;

        impl SnapshotSelector for RejectTarget {
            fn selects(
                &self,
                relative: &Path,
                _is_dir: bool,
            ) -> Result<bool, crate::sandbox::contract::Error> {
                Ok(!relative.starts_with("target"))
            }
        }

        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        std::fs::create_dir_all(root.join("target")).expect("target");
        std::fs::write(root.join("target").join("artifact"), b"built").expect("file");

        let destination = temporary.path().join("snapshot");
        transfer_out(&root, "/src", &destination, Some(&RejectTarget)).expect("downloaded");

        assert!(destination.join("README.md").is_file());
        assert!(destination.join("src").join("main.rs").is_file());
        assert!(
            !destination.join("target").exists(),
            "the transfer must not copy a rejected directory"
        );
    }

    #[test]
    fn a_file_download_lands_in_the_destination() {
        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        let destination = temporary.path().join("out");
        std::fs::create_dir_all(&destination).expect("dir");
        transfer_out(&root, "README.md", &destination, None).expect("downloaded");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
        );
    }

    #[test]
    fn a_download_outside_the_workspace_is_refused() {
        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        let error = transfer_out(&root, "../../etc/passwd", temporary.path(), None)
            .expect_err("refused");
        assert!(
            error.to_string().contains("leaves the sandbox workspace"),
            "{error}"
        );
        let error =
            transfer_out(&root, "/etc/passwd", temporary.path(), None).expect_err("refused");
        assert!(
            error.to_string().contains("leaves the sandbox workspace"),
            "{error}"
        );
    }

    #[test]
    fn a_workspace_path_resolves_to_components_beneath_the_root() {
        assert_eq!(
            components("/src/a/b", "upload").expect("resolved"),
            vec![
                std::ffi::OsString::from("a"),
                std::ffi::OsString::from("b")
            ]
        );
        assert_eq!(
            components("/src", "upload").expect("resolved"),
            Vec::<std::ffi::OsString>::new()
        );
        assert_eq!(
            components("a", "upload").expect("resolved"),
            vec![std::ffi::OsString::from("a")]
        );
        assert!(components("/src/etc", "upload").is_ok());
        assert!(components("/etc", "upload").is_err());
        assert!(components("../a", "upload").is_err());
    }
}
