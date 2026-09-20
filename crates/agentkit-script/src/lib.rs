//! Sandboxed scripting for AgentKit components.
//!
//! [`ScriptEngine`] compiles and runs Rhai scripts with no ambient host
//! capabilities: no filesystem access, no network access and no module
//! resolution. Scripts exchange data with the host as JSON values only. A host
//! that needs to expose capabilities registers them explicitly as JSON-typed
//! functions with [`ScriptEngine::register_host_fn`]; an engine with no
//! registrations physically exposes none.

pub mod engine;
pub mod error;
pub mod limits;
pub mod payload;
pub mod script;

pub use engine::ScriptEngine;
pub use error::ScriptError;
pub use limits::Limits;
pub use payload::{PAYLOAD_CAP, cap_payload};
pub use script::CompiledScript;

/// Origin recorded for scripts that were not loaded from a file.
pub const INLINE_ORIGIN: &str = "<inline>";
