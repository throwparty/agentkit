//! The hardened Rhai engine builders: two structurally separated kinds.
//!
//! **Policy engines** run `pre_tool_use` scripts and receive decision
//! inputs only — they physically lack the acting host functions. **Behaviour
//! engines** run every other event with the full host API (registered by
//! the session host via the registrar passed to
//! [`behaviour_engine`]).
//!
//! The Rhai machinery itself lives in `agentkit-script`; this module only
//! fixes the tackle-specific policy: fixed internal safety limits
//! (operations, call levels, expression depths, collection and string sizes,
//! a 256 KiB script size cap, a per-engine time budget), no acting host
//! functions on policy engines, and fail-closed handling of every
//! [`ScriptError`] by the caller (the permission pipeline treats error or
//! timeout as ask). Argument payloads above the cap are replaced by a digest
//! map so scripts can detect but cannot read them, and `print`/`debug` go to
//! stderr, never stdout.

mod engine;

pub mod host;

pub use agentkit_script::{PAYLOAD_CAP, cap_payload};
pub use engine::{
    BEHAVIOUR_TIME, Compiled, POLICY_TIME, PolicyScripts, SCRIPT_MAX_SIZE, behaviour_engine,
    policy_engine,
};
