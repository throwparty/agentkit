//! Host-scoped operations, where no project owns the sandbox set.

use crate::backends;
use crate::config::Config;
use crate::sandbox::contract::{Error, Scope};

use super::bulk::{PauseSummary, pause_all};

/// Pause every sandbox on the machine through the selected mechanism.
pub async fn pause_machine(config: &Config) -> Result<PauseSummary, Error> {
    let selected = backends::select_as(config, "pause").await?;
    let lifecycle = selected.lifecycle_or_unsupported("pause")?;
    let records = selected.core().list(&Scope::Machine).await?;
    Ok(pause_all(&records, lifecycle.as_ref()).await)
}
