//! The behaviour host API: `SessionAccess`, the host-function surface, and the fixed internal budgets.

mod access;
mod api;
mod behaviour;
mod budgets;

#[cfg(test)]
mod tests;

pub use access::StoreAccess;
pub use api::{
    Completion, CompletionSink, ContextUsage, HistoryRow, HostError, NoticeSink, PromptDispatcher,
    SessionAccess, block_on, estimate_tokens,
};
pub use budgets::{
    HISTORY_MAX_BYTES, HISTORY_MAX_ROWS, MAX_LIVE_EPHEMERAL_FORKS, MAX_PROMPTS_PER_SCRIPT,
    MAX_PROMPTS_PER_SESSION, MAX_PROMPTS_PER_TURN, PARENT_COST_CAP_USD,
};
pub use behaviour::{BehaviourHost, register_host_api};
#[cfg(test)]
pub use api::{LoopPendingCompletions, LoopPendingDispatcher};
