//! Sandboxed scripting for AgentKit components.
//!
//! [`ScriptEngine`] compiles and runs Rhai scripts with no host capabilities:
//! no filesystem access, no network access, no module resolution and no host
//! functions. Scripts exchange data with the host as JSON values only.

pub mod limits;

pub use limits::Limits;
