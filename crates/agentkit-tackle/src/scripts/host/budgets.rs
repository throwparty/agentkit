//! Fixed internal budgets, deliberately not user configuration.

use std::time::Duration;

pub const MAX_LIVE_EPHEMERAL_FORKS: usize = 4;
pub const MAX_PROMPTS_PER_SCRIPT: usize = 2;
pub const MAX_PROMPTS_PER_TURN: usize = 8;
pub const MAX_PROMPTS_PER_SESSION: usize = 64;
/// The parent-session cost cap for script-driven model calls.
pub const PARENT_COST_CAP_USD: f64 = 5.0;
/// history_search row and byte caps.
pub const HISTORY_MAX_ROWS: usize = 20;
pub const HISTORY_MAX_BYTES: usize = 4 * 1024;
/// await_completion's poll interval.
pub(super) const COMPLETION_POLL: Duration = Duration::from_millis(25);
