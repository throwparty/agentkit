//! Turning a devShell output path into a store view.

use std::path::PathBuf;

use crate::sandbox::contract::StoreView;

use super::PrepareError;
use super::runner::CommandRunner;

/// The absolute read-only paths a devShell needs.
pub fn closure(
    output_path: &str,
    view: StoreView,
    runner: &dyn CommandRunner,
) -> Result<Vec<PathBuf>, PrepareError> {
    match view {
        StoreView::Whole => Ok(vec![PathBuf::from("/nix/store")]),
        StoreView::Closure => {
            let output = runner
                .run(
                    "nix",
                    &[
                        "path-info".to_string(),
                        "-r".to_string(),
                        output_path.to_string(),
                    ],
                    None,
                    None,
                )
                .map_err(|error| PrepareError::new(error, "run nix path-info on the host"))?;
            if !output.success() {
                return Err(PrepareError::new(
                    output.first_line(),
                    "run nix develop on the host first",
                ));
            }
            let mut paths: Vec<PathBuf> = output
                .stdout_text()
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect();
            paths.sort();
            paths.dedup();
            Ok(paths)
        }
    }
}
