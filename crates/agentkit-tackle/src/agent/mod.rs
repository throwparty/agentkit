//! The agent loop and the core orchestration: running a prompt turn, direct
//! invocation, tool-call execution, nested agents, fork, compaction, seed,
//! and titling. The model seam itself lives in [`crate::providers`]; the RPC
//! surface that drives this lives in [`crate::acp_v1`].
//!
//! This module is declarations and re-exports only.

pub mod compaction;
pub mod config_options;
pub mod direct;
pub mod fork;
pub mod nested;
pub mod prompt;
pub mod retry;
pub mod runtime;
pub mod script_loop;
pub mod seed;
pub mod sink;
pub mod titling;
pub mod tool_call;
pub mod turn;

mod outcome;

pub use outcome::{TurnOutcome, TurnStop};
