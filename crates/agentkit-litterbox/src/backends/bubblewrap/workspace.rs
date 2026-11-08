//! The workspace a sandbox binds, and the transfers into and out of it.
//!
//! The orchestrator hands create a staged temporary tree, so the backend keeps
//! its own copy. Every path is opened without following a link, so a link the
//! sandbox planted cannot make a transfer read or write outside the tree.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::BACKEND;
use super::view::WORKSPACE;
use crate::sandbox::contract::Error;

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
pub fn copy_from(staged: &Path, destination: &Path) -> Result<(), Error> {
    copy_tree(staged, destination)
        .map_err(|error| Error::failed(format!("the workspace could not be materialised: {error}")))
}

/// Copy a host path into the sandbox workspace.
pub fn transfer_in(root: &Path, source: &Path, destination: &str) -> Result<(), Error> {
    let target = resolve_inside(root, destination)?;
    copy_tree(source, &target).map_err(|error| {
        Error::failed(format!(
            "{} could not be uploaded: {error}",
            source.display()
        ))
    })
}

/// Copy a sandbox path out to the host.
pub fn transfer_out(root: &Path, source: &str, destination: &Path) -> Result<(), Error> {
    let source = resolve_inside(root, source)?;
    let metadata = std::fs::symlink_metadata(&source)
        .map_err(|error| Error::failed(format!("{} cannot be read: {error}", source.display())))?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination).map_err(|error| {
            Error::failed(format!(
                "{} cannot be created: {error}",
                destination.display()
            ))
        })?;
        for entry in std::fs::read_dir(&source).map_err(|error| {
            Error::failed(format!("{} cannot be listed: {error}", source.display()))
        })? {
            let entry = entry.map_err(|error| {
                Error::failed(format!("the workspace could not be read: {error}"))
            })?;
            copy_entry(&entry.path(), &destination.join(entry.file_name())).map_err(|error| {
                Error::failed(format!(
                    "{} cannot be copied: {error}",
                    entry.path().display()
                ))
            })?;
        }
        return Ok(());
    }
    let name = source
        .file_name()
        .ok_or_else(|| Error::failed(format!("{} has no name", source.display())))?;
    copy_entry(&source, &destination.join(name))
        .map_err(|error| Error::failed(format!("{} cannot be copied: {error}", source.display())))
}

/// Resolve a sandbox path inside the workspace, refusing an escape.
fn resolve_inside(root: &Path, path: &str) -> Result<PathBuf, Error> {
    let relative = path.strip_prefix(WORKSPACE).unwrap_or(path);
    let relative = relative.trim_start_matches('/');
    let mut resolved = root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            _ => {
                return Err(Error::Misconfigured {
                    message: format!("'{path}' leaves the sandbox workspace"),
                });
            }
        }
    }
    Ok(resolved)
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
            // one, and it goes through `copy_entry`.
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

fn copy_entry(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(from)?;
        return std::os::unix::fs::symlink(target, to);
    }
    copy_tree(from, to)
}

/// The backend's own error for a workspace that does not exist yet.
pub fn missing_workspace(path: &Path) -> Error {
    Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("{} does not exist", path.display()),
        remedy: "recreate the sandbox".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        copy_from, missing_workspace, resolve_inside, sandbox_directory, transfer_in, transfer_out,
        workspace_directory,
    };

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
        transfer_in(&root, &host, "docs/upload.txt").expect("uploaded");
        assert_eq!(
            std::fs::read(root.join("docs").join("upload.txt")).expect("read"),
            b"payload"
        );
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
    fn a_directory_download_copies_the_tree_and_keeps_a_symlink() {
        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        std::os::unix::fs::symlink("README.md", root.join("readme-link")).expect("symlink");
        let destination = temporary.path().join("snapshot");
        transfer_out(&root, "/src", &destination).expect("downloaded");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
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
    }

    #[test]
    fn a_file_download_lands_in_the_destination() {
        let (temporary, source) = staged();
        let root = temporary.path().join("workspace");
        copy_from(&source, &root).expect("copied");
        let destination = temporary.path().join("out");
        std::fs::create_dir_all(&destination).expect("dir");
        transfer_out(&root, "README.md", &destination).expect("downloaded");
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
        let error = transfer_out(&root, "../../etc/passwd", temporary.path()).expect_err("refused");
        assert!(
            error.to_string().contains("leaves the sandbox workspace"),
            "{error}"
        );
    }

    #[test]
    fn a_workspace_path_resolves_beneath_the_root() {
        let root = Path::new("/state/workspace");
        assert_eq!(
            resolve_inside(root, "/src/a/b").expect("resolved"),
            Path::new("/state/workspace/a/b")
        );
        assert_eq!(
            resolve_inside(root, "a").expect("resolved"),
            Path::new("/state/workspace/a")
        );
    }

    #[test]
    fn a_missing_workspace_is_unavailable() {
        let error = missing_workspace(Path::new("/state/missing"));
        assert!(error.to_string().contains("does not exist"), "{error}");
    }
}
