//! Sandboxed scripting for AgentKit components.
//!
//! [`ScriptEngine`] compiles and runs Rhai scripts with no host capabilities:
//! no filesystem access, no network access, no module resolution and no host
//! functions. Scripts exchange data with the host as JSON values only.

pub mod engine;
pub mod error;
pub mod limits;
pub mod script;

pub use engine::ScriptEngine;
pub use error::ScriptError;
pub use limits::Limits;
pub use script::CompiledScript;

/// Origin recorded for scripts that were not loaded from a file.
pub const INLINE_ORIGIN: &str = "<inline>";
