//! Materialise the workspace and move files in and out of it.
//!
//! macOS has no mount namespace, so the workspace is a host directory. Every
//! component is opened without following a link, and a symlink, device or FIFO
//! is refused rather than copied.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf};

use crate::sandbox::contract::Error;

/// Copy a staged tree into the sandbox workspace.
pub(crate) fn copy_from(source: &Path, destination: &Path) -> Result<(), Error> {
    fs::create_dir_all(destination).map_err(|error| failed(destination, error))?;
    copy_tree(source, destination)
}

/// Copy a host path into the workspace at a relative destination.
pub(crate) fn transfer_in(source: &Path, workspace: &Path, destination: &str) -> Result<(), Error> {
    let relative = relative(destination)?;
    let target = workspace.join(&relative);
    ensure_parents(workspace, &relative)?;
    copy_tree(source, &target)
}

/// Copy a workspace path out to the host.
pub(crate) fn transfer_out(
    workspace: &Path,
    source: &str,
    destination: &Path,
) -> Result<(), Error> {
    let relative = relative(source)?;
    let target = checked_source(workspace, &relative)?;
    if destination.exists() {
        return Err(Error::Denied {
            operation: "download".to_string(),
            reason: format!("the destination '{}' already exists", destination.display()),
        });
    }
    copy_tree(&target, destination)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), Error> {
    if let Ok(metadata) = fs::symlink_metadata(destination)
        && metadata.file_type().is_symlink()
    {
        return Err(refused(destination, "is a symlink"));
    }
    let metadata = fs::symlink_metadata(source).map_err(|error| failed(source, error))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(refused(source, "is a symlink"));
    }
    if kind.is_socket() {
        return Err(refused(source, "is a socket"));
    }
    if kind.is_fifo() {
        return Err(refused(source, "is a FIFO"));
    }
    if kind.is_char_device() || kind.is_block_device() {
        return Err(refused(source, "is a device"));
    }

    if kind.is_dir() {
        fs::create_dir_all(destination).map_err(|error| failed(destination, error))?;
        fs::set_permissions(destination, metadata.permissions())
            .map_err(|error| failed(destination, error))?;
        let mut entries: Vec<_> = fs::read_dir(source)
            .map_err(|error| failed(source, error))?
            .collect::<Result<_, _>>()
            .map_err(|error| failed(source, error))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if kind.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| failed(parent, error))?;
        }
        fs::copy(source, destination).map_err(|error| failed(source, error))?;
        fs::set_permissions(destination, metadata.permissions())
            .map_err(|error| failed(destination, error))?;
        return Ok(());
    }

    Err(refused(source, "is not a regular file or directory"))
}

fn relative(value: &str) -> Result<PathBuf, Error> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() {
        return Err(Error::Misconfigured {
            message: "a transfer path is empty".to_string(),
        });
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => {
                return Err(Error::Misconfigured {
                    message: format!("the transfer path '{value}' must be relative"),
                });
            }
        }
    }
    Ok(relative)
}

fn ensure_parents(workspace: &Path, relative: &Path) -> Result<(), Error> {
    let mut current = workspace.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for component in &components[..components.len().saturating_sub(1)] {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(refused(&current, "is a symlink"));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(refused(&current, "is not a directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| failed(&current, error))?;
            }
            Err(error) => return Err(failed(&current, error)),
        }
    }
    Ok(())
}

fn checked_source(workspace: &Path, relative: &Path) -> Result<PathBuf, Error> {
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| failed(&current, error))?;
        if metadata.file_type().is_symlink() {
            return Err(refused(&current, "is a symlink"));
        }
    }
    Ok(current)
}

fn failed(path: &Path, error: std::io::Error) -> Error {
    Error::failed(format!("cannot transfer '{}': {error}", path.display()))
}

fn refused(path: &Path, reason: &str) -> Error {
    Error::Denied {
        operation: "transfer".to_string(),
        reason: format!("the sandbox path '{}' {reason}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use super::{copy_from, transfer_in, transfer_out};
    use crate::sandbox::contract::Error;

    fn workspace() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn a_staged_tree_is_copied_with_modes() {
        let source = workspace();
        let destination = workspace();
        std::fs::create_dir_all(source.path().join("nested")).expect("dir");
        std::fs::write(source.path().join("nested/file"), b"payload").expect("file");
        std::fs::set_permissions(
            source.path().join("nested/file"),
            std::fs::Permissions::from_mode(0o640),
        )
        .expect("mode");

        copy_from(source.path(), destination.path()).expect("copy");
        let copied = destination.path().join("nested/file");
        assert_eq!(std::fs::read(&copied).expect("read"), b"payload");
        assert_eq!(
            std::fs::metadata(&copied)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[test]
    fn a_symlink_device_or_fifo_is_refused() {
        let source = workspace();
        let destination = workspace();
        symlink("/etc/hosts", source.path().join("link")).expect("symlink");
        let error = copy_from(source.path(), destination.path()).expect_err("symlink");
        assert!(matches!(error, Error::Denied { .. }), "{error}");
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn upload_writes_inside_the_workspace_only() {
        let ws = workspace();
        let file = ws.path().join("source");
        std::fs::write(&file, b"in").expect("file");
        transfer_in(&file, ws.path(), "nested/inside").expect("upload");
        assert_eq!(
            std::fs::read(ws.path().join("nested/inside")).expect("read"),
            b"in"
        );

        let error = transfer_in(&file, ws.path(), "../escape").expect_err("traversal");
        assert!(matches!(error, Error::Misconfigured { .. }), "{error}");
        assert!(!ws.path().parent().expect("parent").join("escape").exists());
    }

    #[test]
    fn upload_through_a_planted_symlink_is_refused() {
        let ws = workspace();
        let outside = workspace();
        let file = ws.path().join("source");
        std::fs::write(&file, b"in").expect("file");
        symlink(outside.path(), ws.path().join("nested")).expect("symlink");
        let error = transfer_in(&file, ws.path(), "nested/inside").expect_err("planted link");
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn download_refuses_a_planted_symlink_and_preserves_modes() {
        let ws = workspace();
        let destination = workspace();
        std::fs::write(ws.path().join("file"), b"out").expect("file");
        std::fs::set_permissions(
            ws.path().join("file"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("mode");
        transfer_out(ws.path(), "file", &destination.path().join("file")).expect("download");
        assert_eq!(
            std::fs::read(destination.path().join("file")).expect("read"),
            b"out"
        );
        assert_eq!(
            std::fs::metadata(destination.path().join("file"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        symlink("/etc/hosts", ws.path().join("link")).expect("symlink");
        let error = transfer_out(ws.path(), "link", &destination.path().join("link"))
            .expect_err("planted link");
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn an_existing_destination_is_refused() {
        let ws = workspace();
        let destination = workspace();
        std::fs::write(ws.path().join("file"), b"out").expect("file");
        std::fs::write(destination.path().join("file"), b"old").expect("old");
        let error =
            transfer_out(ws.path(), "file", &destination.path().join("file")).expect_err("exists");
        assert!(error.to_string().contains("already exists"), "{error}");
    }
}
