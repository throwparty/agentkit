//! The fixed-order permission pipeline — the only path to tool
//! execution aside from user direct invocation.
//!
//! Order: grant-store deny records short-circuit; then the
//! `pre_tool_use` policy script (allow and deny are final, ask falls
//! through, the script sees a `previously_granted` flag); then grant
//! allow records are honoured; then the fixed ask fallback runs
//! regardless of the interactive indicator — the indicator informs
//! scripts only.
//!
//! The grant store is in-memory only, keyed by actor and invokable:
//! grants live for the session's active life in this process and are
//! gone on close, on process exit, and on resume after restart.
//! Nothing about a grant is ever persisted.
//!
//! There is no harness-side timeout and no headless auto-reject: a
//! cancelled or client-timed-out request resolves as reject-once — no
//! grant is written and the next ask asks again.

mod grants;
mod pipeline;
mod policy;
mod request;

#[cfg(test)]
mod tests;

pub use grants::GrantStore;
pub use pipeline::{Assessment, ConsentViaPermission, PermissionPipeline, Verdict};
pub use policy::{Decision, PolicyRequest, PolicyScript, PolicyVerdict};
pub use request::{
    PermissionChoice, PermissionRequest, PermissionRequester, ask_options,
};
