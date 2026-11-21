//! The workspace a sandbox binds, and the transfers into and out of it.
//!
//! The walk itself lives in `backends::transfer`, shared with the other
//! mechanisms; this module supplies the paths and the component the sandbox
//! sees.

use std::path::{Path, PathBuf};

use crate::backends::bubblewrap::view::WORKSPACE;
use crate::backends::transfer;
use crate::sandbox::contract::{Error, SnapshotSelector};

/// The workspace's name, as a path component.
#[cfg(test)]
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
pub fn copy_from(staged: &Path, destination: &Path) -> Result<(), Error> {
    transfer::copy_from(staged, destination)
}

/// Copy a host path into the sandbox workspace.
pub fn transfer_in(
    root: &Path,
    source: &Path,
    destination: &str,
    masks: &[PathBuf],
) -> Result<(), Error> {
    transfer::transfer_in(root, Path::new(WORKSPACE), source, destination, masks)
}

/// Copy a sandbox path out to the host.
pub fn transfer_out(
    root: &Path,
    source: &str,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
    masks: &[PathBuf],
) -> Result<(), Error> {
    transfer::transfer_out(
        root,
        Path::new(WORKSPACE),
        source,
        destination,
        selector,
        masks,
    )
}

#[cfg(test)]
mod tests {
    use super::{WORKSPACE_COMPONENT, copy_from, sandbox_directory, workspace_directory};
    use crate::backends::bubblewrap::view::WORKSPACE;

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
        let temporary = tempfile::tempdir().expect("tempdir");
        let source = temporary.path().join("staged");
        std::fs::create_dir_all(source.join("src")).expect("dirs");
        std::fs::write(source.join("README.md"), b"hello").expect("file");
        let destination = temporary.path().join("workspace");
        copy_from(&source, &destination).expect("copied");
        assert_eq!(
            std::fs::read(destination.join("README.md")).expect("read"),
            b"hello"
        );
    }
}
