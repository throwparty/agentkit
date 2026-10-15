use std::path::PathBuf;

use thiserror::Error;

/// Typed failures from every stage of script compilation and execution.
///
/// Every variant carries the script's origin (a file path, or an inline marker
/// such as `<inline>`) so failures can be attributed in logs and error
/// responses.
#[derive(Debug, Error)]
pub enum ScriptError {
    /// The script file could not be read.
    #[error("failed to read script '{path}': {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The script source could not be compiled.
    #[error("failed to compile script '{origin}': {message}")]
    Compile { origin: String, message: String },
    /// The script does not define the requested function.
    #[error("script '{origin}' does not define fn {function}/{arity}")]
    MissingFunction {
        origin: String,
        function: String,
        arity: usize,
    },
    /// The script raised a runtime error.
    #[error("script '{origin}' failed: {message}")]
    Runtime { origin: String, message: String },
    /// The script exceeded one of the configured [`crate::Limits`].
    #[error("script '{origin}' exceeded the {limit} limit")]
    LimitExceeded { origin: String, limit: &'static str },
    /// The script exceeded its wall-clock budget.
    #[error("script '{origin}' exceeded the {timeout:?} timeout")]
    Timeout {
        origin: String,
        timeout: std::time::Duration,
    },
    /// The script returned a value that could not be represented as JSON.
    #[error("script '{origin}' returned an unusable value: {message}")]
    InvalidReturn { origin: String, message: String },
}
