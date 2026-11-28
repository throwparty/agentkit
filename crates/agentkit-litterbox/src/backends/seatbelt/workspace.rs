//! The Seatbelt workspace and its transfers.
//!
//! The sandbox sees the workspace at its real host path, so that path is both
//! the root transfers resolve under and the path a sandbox names. The walk
//! lives in `backends::transfer`, shared with the other mechanisms.

use std::path::{Path, PathBuf};

use crate::backends::transfer;
use crate::sandbox::contract::{Error, SnapshotSelector};

/// Copy a staged tree into the sandbox workspace.
pub(crate) fn copy_from(source: &Path, destination: &Path) -> Result<(), Error> {
    transfer::copy_from(source, destination)
}

/// Copy a host path into the workspace at a sandbox destination.
pub(crate) fn transfer_in(
    source: &Path,
    workspace: &Path,
    destination: &str,
    masks: &[PathBuf],
) -> Result<(), Error> {
    transfer::transfer_in(workspace, workspace, source, destination, masks)
}

/// Copy a sandbox path out to the host, applying a caller's snapshot rules.
pub(crate) fn transfer_out(
    workspace: &Path,
    source: &str,
    destination: &Path,
    selector: Option<&dyn SnapshotSelector>,
    masks: &[PathBuf],
) -> Result<(), Error> {
    transfer::transfer_out(workspace, workspace, source, destination, selector, masks)
}
