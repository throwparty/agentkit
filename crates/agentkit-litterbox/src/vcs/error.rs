/// A VCS failure, classified without naming a backend type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The branch or sandbox already exists.
    #[error("sandbox '{name}' already exists")]
    Exists {
        /// The name that was addressed.
        name: String,
    },
    /// The branch or sandbox is gone.
    #[error("sandbox '{name}' not found")]
    NotFound {
        /// The name that was addressed.
        name: String,
    },
    /// The backend cannot perform the operation.
    #[error("{operation} is not supported by {backend}: {reason}. {remedy}")]
    Unsupported {
        /// The operation requested.
        operation: String,
        /// The backend that cannot perform it.
        backend: String,
        /// The primitive the backend lacks.
        primitive: String,
        /// Why it cannot perform it.
        reason: String,
        /// What the user can do instead.
        remedy: String,
    },
    /// The backend failed. The message is the backend's own diagnosis.
    #[error("{message}")]
    Backend {
        /// The operation that failed.
        operation: String,
        /// The backend's message, preserved from the previous error type.
        message: String,
    },
    /// Local I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
