//! How the contract's failures present to the surfaces that expose them.
//!
//! The classification is written once, here, so the CLI exit code and the MCP
//! error class are not re-derived in every backend.

use agentkit_common::exit::ExitClass;

use super::contract::Error;
use crate::domain::{ComputeError, SandboxError};

/// The class an agent-facing surface reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceClass {
    /// The caller can fix the request: address something else, drop the
    /// operation, or ask for permission.
    InvalidParams,
    /// The operation ran and failed; nothing the caller changes will help.
    Internal,
}

impl Error {
    /// The process exit class for this failure.
    pub fn exit_class(&self) -> ExitClass {
        match self {
            Self::NotFound { .. } => ExitClass::NotFound,
            Self::Conflict { .. } => ExitClass::Conflict,
            Self::Unsupported { .. } => ExitClass::Unsupported,
            Self::Denied { .. } => ExitClass::Denied,
            Self::Failed { .. } => ExitClass::Failed,
        }
    }

    /// The agent-facing class for this failure.
    pub fn surface_class(&self) -> SurfaceClass {
        match self {
            Self::NotFound { .. }
            | Self::Conflict { .. }
            | Self::Unsupported { .. }
            | Self::Denied { .. } => SurfaceClass::InvalidParams,
            Self::Failed { .. } => SurfaceClass::Internal,
        }
    }

    /// Whether the addressed sandbox does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

impl From<SandboxError> for Error {
    fn from(error: SandboxError) -> Self {
        match &error {
            SandboxError::SandboxExists { name } => {
                return Self::Conflict { name: name.clone() };
            }
            SandboxError::SandboxNotFound { name } => {
                return Self::NotFound { name: name.clone() };
            }
            _ => {}
        }

        Self::Failed {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

impl From<ComputeError> for Error {
    fn from(error: ComputeError) -> Self {
        Self::Failed {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, SurfaceClass};
    use crate::domain::SandboxError;
    use agentkit_common::exit::ExitClass;

    #[test]
    fn not_found_is_a_usage_error_for_the_caller() {
        let error = Error::NotFound {
            name: "demo".to_string(),
        };
        assert_eq!(error.exit_class(), ExitClass::NotFound);
        assert_eq!(error.surface_class(), SurfaceClass::InvalidParams);
        assert!(error.is_not_found());
    }

    #[test]
    fn conflict_is_distinct_from_not_found() {
        let error = Error::Conflict {
            name: "demo".to_string(),
        };
        assert_eq!(error.exit_class(), ExitClass::Conflict);
        assert_eq!(error.surface_class(), SurfaceClass::InvalidParams);
        assert!(!error.is_not_found());
    }

    #[test]
    fn unsupported_names_operation_reason_and_remedy() {
        let error = Error::Unsupported {
            operation: "forward port".to_string(),
            backend: "bubblewrap".to_string(),
            reason: "the sandbox shares no host port namespace".to_string(),
            remedy: "run the service on a forwarded Docker sandbox".to_string(),
        };
        let message = error.to_string();
        assert!(message.contains("forward port"));
        assert!(message.contains("bubblewrap"));
        assert!(message.contains("remedy") || message.contains("run the service"));
        assert_eq!(error.exit_class(), ExitClass::Unsupported);
        assert_eq!(error.surface_class(), SurfaceClass::InvalidParams);
    }

    #[test]
    fn denied_and_failed_are_separate_classes() {
        let denied = Error::Denied {
            operation: "read".to_string(),
            reason: "outside the workspace".to_string(),
        };
        let failed = Error::failed("backend exploded");
        assert_eq!(denied.exit_class(), ExitClass::Denied);
        assert_eq!(failed.exit_class(), ExitClass::Failed);
        assert_eq!(failed.surface_class(), SurfaceClass::Internal);
    }

    #[test]
    fn domain_errors_keep_their_identity() {
        let exists: Error = SandboxError::SandboxExists {
            name: "demo".to_string(),
        }
        .into();
        assert!(matches!(exists, Error::Conflict { .. }));

        let missing: Error = SandboxError::SandboxNotFound {
            name: "demo".to_string(),
        }
        .into();
        assert!(matches!(missing, Error::NotFound { .. }));

        let other: Error = SandboxError::Config("bad".to_string()).into();
        assert!(matches!(other, Error::Failed { .. }));
    }
}
