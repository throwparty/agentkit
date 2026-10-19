//! The host-API traits and payload types.

use crate::store::{SessionId, SessionKind, TurnId};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("store error: {0}")]
    Store(#[from] crate::store::StoreError),
    #[error("unknown actor `{0}`")]
    UnknownActor(String),
    #[error("budget exhausted: {0}")]
    Budget(&'static str),
    #[error("nesting beyond depth one is forbidden")]
    DepthExceeded,
    #[error("await timed out after {0}s")]
    Timeout(i64),
    #[error("model error: {0}")]
    Model(String),
    #[error("invalid argument: {0}")]
    Invalid(&'static str),
}

/// A history_search row.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    pub role: String,
    pub content: String,
    pub turn_id: TurnId,
    pub created_at: i64,
}

/// What await_completion returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub stop_reason: String,
    pub final_message: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Current context utilisation for a session: `used` and `size` are both
/// in tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextUsage {
    pub used: u64,
    pub size: u64,
}

/// A rough token estimate from raw context bytes: about four bytes per
/// token. The single conversion point for every bytes-vs-tokens seam
/// (usage snapshots, the model-switch auto-compaction trigger, script
/// context usage) so they never disagree.
pub fn estimate_tokens(bytes: u64) -> u64 {
    bytes / 4
}

/// Drives a user prompt through the session's normal agent loop —
/// model-mediated and pipeline-gated. The turn loop implements this
/// scripts never invoke tools directly.
pub trait PromptDispatcher: Send + Sync {
    fn dispatch(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
}

/// Publishes turn completions; await_completion polls it. The turn
/// loop publishes.
pub trait CompletionSink: Send + Sync {
    fn publish(&self, session: &SessionId, completion: Completion);
    fn take(&self, session: &SessionId) -> Option<Completion>;
}

/// Surfaces a script-driven model call to the client. A narrow
/// seam: the script loop gets one-way notification authority only, never
/// the connection (which also carries client requests).
pub trait NoticeSink: Send + Sync {
    /// A script issued a model call for `session`.
    fn script_model_call(&self, session: &SessionId);
}

/// A test-only dispatcher for scripts that prompt without a real model
/// loop wired: the user message is already stored, so there is nothing
/// left to do. Production wires [`crate::acp_v1::script_loop`] instead.
#[cfg(test)]
pub struct LoopPendingDispatcher;

#[cfg(test)]
impl PromptDispatcher for LoopPendingDispatcher {
    fn dispatch(&self, _session: &SessionId, _text: &str) -> Result<(), HostError> {
        Ok(())
    }
}

/// A test-only completion sink for scripts that await a completion
/// without a real loop: a non-end_turn answer, so the script degrades
/// gracefully instead of hanging on its timeout.
#[cfg(test)]
pub struct LoopPendingCompletions;

#[cfg(test)]
impl CompletionSink for LoopPendingCompletions {
    fn publish(&self, _session: &SessionId, _completion: Completion) {}
    fn take(&self, _session: &SessionId) -> Option<Completion> {
        Some(Completion {
            stop_reason: "model-loop-pending".to_owned(),
            final_message: String::new(),
            input_tokens: 0,
            output_tokens: 0,
        })
    }
}

/// The seam between the script host and the session engine: history
/// search, fork, prompt, compaction recording, titling.
pub trait SessionAccess: Send + Sync {
    /// The session's kind — the depth-one rule reads it.
    fn session_kind(&self, session: &SessionId) -> Result<SessionKind, HostError>;
    /// Forks the session; usage of an ephemeral fork attributes to its
    /// parent (the DAG is shared).
    fn fork(
        &self,
        source: &SessionId,
        ephemeral: bool,
        actor: &str,
    ) -> Result<SessionId, HostError>;
    fn send_prompt(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
    fn await_completion(
        &self,
        session: &SessionId,
        timeout_secs: i64,
    ) -> Result<Completion, HostError>;
    fn insert_seed(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
    /// Appends a compaction turn; `first_retained` clamped to be newer
    /// than any older compaction turn on the chain.
    fn record_compaction(
        &self,
        session: &SessionId,
        summary: &str,
        first_retained: Option<&TurnId>,
    ) -> Result<(), HostError>;
    fn set_session_title(&self, session: &SessionId, title: &str) -> Result<(), HostError>;
    fn history_search(
        &self,
        session: &SessionId,
        query: &str,
    ) -> Result<Vec<HistoryRow>, HostError>;
    fn context_usage(&self, session: &SessionId) -> Result<ContextUsage, HostError>;
    /// The session's cost plus its live ephemeral forks': the parent's
    /// attributable cost, which the cap is enforced against.
    fn attributed_cost(&self, session: &SessionId) -> Result<f64, HostError>;
}

/// Runs an async store operation to completion. Host functions are
/// synchronous; on a runtime this must be the multi-thread flavor
/// (block_in_place), which is the production shape.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("host runtime");
            runtime.block_on(future)
        }
    }
}
