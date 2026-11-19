//! Selects the ignore files a snapshot's rules are read from.

use std::path::Path;

/// Selects the ignore files a snapshot's rules are read from.
///
/// A snapshot has to know the sandbox's own rules before it decides what to
/// transfer, so the transfer runs twice: once for these files, and once for
/// everything else under the rules they describe.
pub struct IgnoreFiles;

impl crate::sandbox::contract::SnapshotSelector for IgnoreFiles {
    fn selects(
        &self,
        relative: &Path,
        is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        // A directory is kept so the walk reaches the rules nested inside it.
        if is_dir {
            return Ok(true);
        }
        Ok(relative
            .file_name()
            .is_some_and(|name| name == ".gitignore")
            || relative == Path::new(".git/info/exclude"))
    }
}
