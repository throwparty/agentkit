//! The Docker backend's own error type.
//!
//! The contract knows only the neutral failure classes; classifying a daemon
//! error is Docker's business, so these types live beside the client.

use thiserror::Error;

/// How a backend failure should be interpreted by callers, independent of the
/// wire format the backend happens to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    NotFound,
    Conflict,
    Other,
}

impl FailureKind {
    pub fn is_not_found(self) -> bool {
        self == Self::NotFound
    }

    pub fn is_conflict(self) -> bool {
        self == Self::Conflict
    }
}

/// An opaque backend error, plus the little that callers are allowed to branch on.
#[derive(Debug)]
pub struct BackendError {
    kind: FailureKind,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl BackendError {
    pub fn new(
        kind: FailureKind,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        Self {
            kind,
            source: source.into(),
        }
    }

    pub fn other(source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        Self::new(FailureKind::Other, source)
    }

    pub fn kind(&self) -> FailureKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::ComputeError;
    use crate::sandbox::contract::Error;

    #[test]
    fn a_configuration_refusal_keeps_its_class() {
        let error: Error = ComputeError::Config("set DOCKER_HOST".to_string()).into();
        assert!(matches!(error, Error::Misconfigured { .. }), "{error:?}");

        let error: Error = ComputeError::Timeout { seconds: 5 }.into();
        assert!(matches!(error, Error::Failed { .. }), "{error:?}");
    }

    #[test]
    fn a_daemon_that_disappears_is_unavailable_not_failed() {
        let error: Error = ComputeError::Connection {
            source: super::BackendError::other("connection refused"),
        }
        .into();
        match error {
            Error::Unavailable {
                backend,
                reason,
                remedy,
            } => {
                assert_eq!(backend, "docker");
                assert!(reason.contains("connection refused"), "{reason}");
                assert!(remedy.contains("daemon"), "{remedy}");
            }
            other => panic!("expected an unavailable report, got {other:?}"),
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Error, Debug)]
pub enum ComputeError {
    /// The client is misconfigured.
    #[error("{0}")]
    Config(String),
    /// A local I/O failure while staging or extracting.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("backend connection failed: {source}")]
    Connection {
        #[source]
        source: BackendError,
    },
    #[error("image inspection failed: {source}")]
    ImageInspect {
        #[source]
        source: BackendError,
    },
    #[error("image pull failed: {source}")]
    ImagePull {
        #[source]
        source: BackendError,
    },
    #[error("sandbox provisioning failed: {source}")]
    ContainerProvision {
        #[source]
        source: BackendError,
    },
    #[error("sandbox limits update failed: {source}")]
    ContainerLimits {
        #[source]
        source: BackendError,
    },
    #[error("sandbox inspection failed: {source}")]
    ContainerInspect {
        #[source]
        source: BackendError,
    },
    #[error("sandbox pause failed: {source}")]
    ContainerPause {
        #[source]
        source: BackendError,
    },
    #[error("sandbox resume failed: {source}")]
    ContainerResume {
        #[source]
        source: BackendError,
    },
    #[error("sandbox delete failed: {source}")]
    ContainerDelete {
        #[source]
        source: BackendError,
    },
    #[error("the command did not finish within {seconds} seconds and may still be running")]
    Timeout {
        /// The deadline that expired.
        seconds: u64,
    },
    #[error("command execution failed: {source}")]
    ContainerExec {
        #[source]
        source: BackendError,
    },
    #[error("upload failed: {source}")]
    ContainerUpload {
        #[source]
        source: BackendError,
    },
    #[error("the transfer exceeds the {limit_bytes}-byte limit")]
    TransferTooLarge {
        /// The ceiling that was crossed.
        limit_bytes: u64,
    },
    #[error("download failed: {source}")]
    ContainerDownload {
        #[source]
        source: BackendError,
    },
    #[error("sandbox listing failed: {source}")]
    ContainerList {
        #[source]
        source: BackendError,
    },
}

impl From<ComputeError> for crate::sandbox::contract::Error {
    fn from(error: ComputeError) -> Self {
        match error {
            // A setting this client cannot honour is a configuration failure
            // wherever it is detected.
            ComputeError::Config(message) => Self::Misconfigured { message },
            // A daemon that disappears later is a host problem, not a failed
            // command, so the caller gets the reason and a remedy.
            ComputeError::Connection { source } => Self::Unavailable {
                backend: super::backend::BACKEND.to_string(),
                reason: source.to_string(),
                remedy: "start the Docker daemon and point DOCKER_HOST at it".to_string(),
            },
            other => Self::Failed {
                message: other.to_string(),
                source: Some(Box::new(other)),
            },
        }
    }
}
