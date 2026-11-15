//! The workspace walk every mechanism shares.
//!
//! A workspace is a host directory a sandbox may write. Transfers resolve the
//! workspace root once and then walk it descriptor by descriptor, refusing a
//! link at every step, so a link the sandbox planted cannot make a transfer
//! read or write outside the tree. The staged copy at create is the one walk
//! over host-trusted input: a link is kept when it stays inside the tree and
//! refused when it leaves.

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::sandbox::contract::{Error, SnapshotSelector};

/// Copy the staged tree into a sandbox workspace.
///
/// The destination is not shared with a workload yet, so this copy may work
/// from paths. A link is kept when its target stays under the staged root, so
/// a repository that tracks one creates a normal workspace, and refused when
/// it points outside the tree.
pub(crate) fn copy_from(staged: &Path, destination: &Path) -> Result<(), Error> {
    copy_staged(staged, destination, staged)
        .map_err(|error| Error::failed(format!("the workspace could not be materialised: {error}")))
}

/// Copy a host path into the sandbox workspace.
pub(crate) fn transfer_in(
    root: &Path,
    sandbox_root: &Path,
    source: &Path,
    destination: &str,
) -> Result<(), Error> {
    let root = Root::open(root)
        .map_err(|error| Error::failed(format!("the workspace cannot be opened: {error}")))?;
    let mut relative = components(destination, sandbox_root, "upload into the sandbox")?;
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
/// A selector narrows the copy to the paths a snapshot includes; the walk asks
/// it before reading anything, so a rejected path never reaches the host.
pub(crate) fn transfer_out(
    root: &Path,
    sandbox_root: &Path,
    source: &str,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    let root = Root::open(root)
        .map_err(|error| Error::failed(format!("the workspace cannot be opened: {error}")))?;
    let destination = Destination::open(destination).map_err(|error| {
        Error::failed(format!(
            "{} cannot be created: {error}",
            destination.display()
        ))
    })?;
    let relative = components(source, sandbox_root, "download from the sandbox")?;
    let Some((name, parents)) = relative.split_last() else {
        // The workspace itself: its contents merge into the destination.
        return copy_contents(
            &root.directory,
            Path::new(""),
            &destination.directory,
            selector,
        );
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
    let opened = match open_in(
        &parent,
        name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW,
    ) {
        Ok(opened) => opened,
        // A link at the source is copied as a link, not followed.
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            if skipped(selector, &relative, false)? {
                return Ok(());
            }
            let target = readlink_in(&parent, name)
                .map_err(|error| Error::failed(format!("{source} cannot be read: {error}")))?;
            return symlink_in(&destination.directory, name, &target)
                .map_err(|error| Error::failed(format!("{source} cannot be copied: {error}")));
        }
        Err(error) => {
            return Err(Error::failed(format!("{source} cannot be read: {error}")));
        }
    };
    let metadata = opened.metadata().map_err(|error| {
        Error::failed(format!("{} cannot be read: {error}", root.path.display()))
    })?;
    if skipped(selector, &relative, metadata.is_dir())? {
        return Ok(());
    }
    if metadata.is_dir() {
        // The descriptor is the one validated above: the name is not resolved
        // a second time.
        let created = directory_in(&destination.directory, name)
            .map_err(|error| Error::failed(format!("{source} cannot be copied: {error}")))?;
        copy_contents(&opened, &relative, &created, selector)
    } else {
        copy_leaf(&parent, name, &relative, &destination.directory, selector)
            .map_err(|error| Error::failed(format!("{source} cannot be copied: {error}")))
    }
}

/// The destination, opened once and never resolved through again.
///
/// Everything below it is created and opened relative to a descriptor, so a
/// link in the caller's destination tree is refused rather than followed.
struct Destination {
    directory: std::fs::File,
}

impl Destination {
    fn open(path: &Path) -> io::Result<Self> {
        // The caller names the root, so its parents are the caller's business;
        // everything below it is resolved from this descriptor instead.
        std::fs::create_dir_all(path)?;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self { directory })
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
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self {
            directory,
            path: path.to_path_buf(),
        })
    }
}

/// Resolve a sandbox path into the components below the workspace root.
///
/// An absolute path has to name the workspace the sandbox sees, component by
/// component; anything else is refused rather than remapped into the
/// workspace.
fn components(path: &str, sandbox_root: &Path, operation: &str) -> Result<Vec<OsString>, Error> {
    let narrowed = Path::new(path);
    let mut names = Vec::new();
    let path = if narrowed.is_absolute() {
        if narrowed == sandbox_root {
            return Ok(names);
        }
        match narrowed.strip_prefix(sandbox_root) {
            Ok(relative) => relative,
            Err(_) => return Err(confinement(narrowed, sandbox_root, operation)),
        }
    } else {
        narrowed
    };
    for piece in path.components() {
        match piece {
            Component::Normal(name) => names.push(name.to_os_string()),
            Component::CurDir => {}
            _ => return Err(confinement(narrowed, sandbox_root, operation)),
        }
    }
    Ok(names)
}

fn confinement(path: &Path, sandbox_root: &Path, operation: &str) -> Error {
    Error::Denied {
        operation: operation.to_string(),
        reason: format!(
            "'{}' leaves the sandbox workspace at {}",
            path.display(),
            sandbox_root.display()
        ),
    }
}

/// Walk to a directory below the root, optionally creating the missing ones.
fn descend(root: &Root, relative: &[OsString], create: bool) -> io::Result<std::fs::File> {
    let mut current = root.directory.try_clone()?;
    for name in relative {
        current = match open_in(
            &current,
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        ) {
            Ok(opened) => opened,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                mkdir_in(&current, name)?;
                open_in(
                    &current,
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )?
            }
            Err(error) => return Err(error),
        };
    }
    Ok(current)
}

/// Open one name inside a pinned directory, without following a link.
fn open_in(directory: &std::fs::File, name: &OsStr, flags: i32) -> io::Result<std::fs::File> {
    open_at(directory, name, flags, 0)
}

/// Open one name inside a pinned directory, with a mode for the created case.
fn open_at(
    directory: &std::fs::File,
    name: &OsStr,
    flags: i32,
    mode: u32,
) -> io::Result<std::fs::File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("a path component holds a NUL byte"))?;
    // SAFETY: the descriptor is a directory and the name is one component of it.
    let opened = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode as libc::c_uint,
        )
    };
    if opened < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is owned by this value from here on.
    Ok(unsafe { std::fs::File::from_raw_fd(opened) })
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

/// A directory inside a pinned parent, created when it does not exist yet.
fn directory_in(directory: &std::fs::File, name: &OsStr) -> io::Result<std::fs::File> {
    match open_in(
        directory,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    ) {
        Ok(opened) => Ok(opened),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            mkdir_in(directory, name)?;
            open_in(
                directory,
                name,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        }
        Err(error) => Err(error),
    }
}

/// Create a link inside a pinned directory.
fn symlink_in(directory: &std::fs::File, name: &OsStr, target: &Path) -> io::Result<()> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::other("a path component holds a NUL byte"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("a link target holds a NUL byte"))?;
    // SAFETY: the descriptor is a directory, both names are single components
    // and the strings outlive the call.
    if unsafe { libc::symlinkat(target.as_ptr(), directory.as_raw_fd(), name.as_ptr()) } < 0 {
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
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
    )?;
    let mut reader = std::fs::File::open(source)?;
    io::copy(&mut reader, &mut target)?;
    target.set_permissions(metadata.permissions())?;
    Ok(())
}

/// Copy a directory's entries into an open host directory.
fn copy_contents(
    directory: &std::fs::File,
    relative: &Path,
    destination: &std::fs::File,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    for name in entries(directory)
        .map_err(|error| Error::failed(format!("the workspace could not be read: {error}")))?
    {
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

/// Copy one entry of a directory into an open host directory, keeping a link.
fn copy_leaf(
    directory: &std::fs::File,
    name: &OsStr,
    relative: &Path,
    destination: &std::fs::File,
    selector: Option<&dyn SnapshotSelector>,
) -> Result<(), Error> {
    let opened = match open_in(
        directory,
        name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW,
    ) {
        Ok(opened) => opened,
        // A link is copied as a link, never resolved: the snapshot records the
        // target, and following it would read outside the workspace.
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            if skipped(selector, relative, false)? {
                return Ok(());
            }
            let target = readlink_in(directory, name).map_err(|error| {
                Error::failed(format!("{} cannot be read: {error}", relative.display()))
            })?;
            return symlink_in(destination, name, &target).map_err(|error| {
                Error::failed(format!("{} cannot be copied: {error}", relative.display()))
            });
        }
        Err(error) => {
            return Err(Error::failed(format!(
                "{} cannot be read: {error}",
                relative.display()
            )));
        }
    };
    let metadata = opened.metadata().map_err(|error| {
        Error::failed(format!("{} cannot be read: {error}", relative.display()))
    })?;
    if skipped(selector, relative, metadata.is_dir())? {
        return Ok(());
    }
    if metadata.is_dir() {
        let target = directory_in(destination, name).map_err(|error| {
            Error::failed(format!("{} cannot be created: {error}", relative.display()))
        })?;
        return copy_contents(&opened, relative, &target, selector);
    }
    if !metadata.is_file() {
        // A FIFO would block the transfer and a device would stream without
        // bound, so neither is copied.
        return Err(Error::failed(format!(
            "{} is not a file or directory; a download copies neither",
            relative.display()
        )));
    }
    let mut copy = open_at(
        destination,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
        0o600,
    )
    .map_err(|error| Error::failed(format!("{} cannot be created: {error}", relative.display())))?;
    let mut source = opened;
    io::copy(&mut source, &mut copy)
        .and_then(|_| copy.set_permissions(metadata.permissions()))
        .map_err(|error| Error::failed(format!("{} cannot be copied: {error}", relative.display())))
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

/// Copy a staged tree, keeping a link only when its target stays inside it.
fn copy_staged(from: &Path, to: &Path, root: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(from)?;
        let resolved = resolve_link(from, &target);
        if !resolved.starts_with(root) {
            return Err(io::Error::other(format!(
                "{} is a symlink that leaves the workspace",
                from.display()
            )));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return std::os::unix::fs::symlink(&target, to);
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_staged(&entry.path(), &to.join(entry.file_name()), root)?;
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
        "{} is not a file, directory or symlink",
        from.display()
    )))
}

/// Resolve a link target against its own directory, lexically.
///
/// Lexical because a repository may track a link whose target has not been
/// materialised yet; following it with `realpath` would refuse a valid tree.
fn resolve_link(link: &Path, target: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    let base = if target.is_absolute() {
        Path::new("/")
    } else {
        link.parent().unwrap_or(Path::new("/"))
    };
    for component in base.components().chain(target.components()) {
        match component {
            Component::RootDir => {
                resolved.clear();
                resolved.push("/");
            }
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => resolved.push(part),
            Component::Prefix(_) => {}
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::{components, copy_from, transfer_in, transfer_out};
    use crate::sandbox::contract::SnapshotSelector;

    struct Skip(&'static str);

    impl SnapshotSelector for Skip {
        fn selects(
            &self,
            relative: &std::path::Path,
            _is_dir: bool,
        ) -> Result<bool, crate::sandbox::contract::Error> {
            Ok(relative != std::path::Path::new(self.0))
        }
    }

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn a_staged_tree_is_copied_with_its_contents() {
        let root = workspace();
        let staged = root.path().join("staged");
        std::fs::create_dir_all(staged.join("src")).expect("dirs");
        std::fs::write(staged.join("README.md"), b"hello").expect("file");
        std::fs::write(staged.join("src/main.rs"), b"fn main() {}").expect("file");
        let destination = root.path().join("workspace");
        copy_from(&staged, &destination).expect("copied");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
        );
        assert!(destination.join("src/main.rs").is_file());
    }

    #[test]
    fn a_staged_link_inside_the_tree_is_kept() {
        let root = workspace();
        let staged = root.path().join("staged");
        std::fs::create_dir_all(staged.join("v2")).expect("dirs");
        std::os::unix::fs::symlink("v2", staged.join("latest")).expect("symlink");
        let destination = root.path().join("workspace");
        copy_from(&staged, &destination).expect("copied");
        let link = destination.join("latest");
        assert!(std::fs::symlink_metadata(&link).expect("link").is_symlink());
        assert_eq!(
            std::fs::read_link(&link).expect("target"),
            std::path::PathBuf::from("v2")
        );
    }

    #[test]
    fn a_staged_link_out_of_the_tree_is_refused() {
        let root = workspace();
        let staged = root.path().join("staged");
        std::fs::create_dir_all(&staged).expect("dirs");
        std::os::unix::fs::symlink("/etc/passwd", staged.join("escape")).expect("symlink");
        let error = copy_from(&staged, &root.path().join("workspace")).expect_err("escape");
        assert!(
            error.to_string().contains("leaves the workspace"),
            "{error}"
        );
    }

    #[test]
    fn an_absolute_sandbox_path_names_the_root() {
        let root = std::path::Path::new("/state/workspace");
        assert!(
            components("/state/workspace", root, "download")
                .expect("root")
                .is_empty()
        );
        assert_eq!(
            components("/state/workspace/src/main.rs", root, "download").expect("path"),
            vec![
                std::ffi::OsString::from("src"),
                std::ffi::OsString::from("main.rs")
            ]
        );
        assert!(components("/state/other", root, "download").is_err());
        assert!(components("../escape", root, "download").is_err());
    }

    #[test]
    fn uploads_land_inside_the_workspace_only() {
        let root = workspace();
        let workspace = root.path().join("ws");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let source = root.path().join("source");
        std::fs::write(&source, b"in").expect("file");
        transfer_in(
            &workspace,
            &workspace,
            &source,
            workspace.join("nested/in").to_str().expect("utf8"),
        )
        .expect("upload");
        assert_eq!(
            std::fs::read(workspace.join("nested/in/source")).expect("read"),
            b"in"
        );
    }

    #[test]
    fn downloads_keep_the_selector_and_copy_links_as_links() {
        let root = workspace();
        let workspace = root.path().join("ws");
        std::fs::create_dir_all(workspace.join("keep")).expect("dirs");
        std::fs::write(workspace.join("keep/inside"), b"kept").expect("file");
        std::fs::create_dir_all(workspace.join("target")).expect("dirs");
        std::fs::write(workspace.join("target/junk"), b"junk").expect("file");
        std::os::unix::fs::symlink("/etc/passwd", workspace.join("link")).expect("symlink");
        let destination = root.path().join("out");
        transfer_out(
            &workspace,
            &workspace,
            workspace.to_str().expect("utf8"),
            &destination,
            Some(&Skip("target") as &dyn SnapshotSelector),
        )
        .expect("download");
        assert!(destination.join("keep/inside").exists());
        assert!(!destination.join("target").exists());
        assert!(
            std::fs::symlink_metadata(destination.join("link"))
                .expect("link")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_planted_link_component_is_refused() {
        let root = workspace();
        let workspace = root.path().join("ws");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, workspace.join("planted")).expect("symlink");
        let source = root.path().join("source");
        std::fs::write(&source, b"in").expect("file");
        let error = transfer_in(
            &workspace,
            &workspace,
            &source,
            workspace.join("planted/file").to_str().expect("utf8"),
        )
        .expect_err("planted link");
        assert!(!outside.join("file").exists(), "the write escaped");
        assert!(!outside.join("source").exists(), "the write escaped");
        assert!(error.to_string().contains("uploaded"), "{error}");
    }
}
