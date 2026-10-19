//! The agent loop: the turn loop and its outcomes, nested agent invocation, and retry. The model seam itself lives in [`crate::providers`].

pub mod nested;
pub mod retry;
pub mod runtime;
pub mod sink;
pub mod turn;

use crate::store::TurnUsage;

/// The terminal state of one executed turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStop {
    /// The model finished without requesting tools.
    EndTurn,
    /// The model-request cap was hit mid-work; the user may continue.
    MaxTurnRequests,
    /// The conversation no longer fits the model's window: the turn ended
    /// with the actionable overflow note instead of a model answer. The
    /// caller surfaces the context size and may auto-compact.
    ContextLength,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    pub stop: TurnStop,
    pub usage: TurnUsage,
}
