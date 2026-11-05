//! How the contract's failure classes map onto the surfaces that expose them.
//!
//! The classification is written once, here, so the CLI exit code and the MCP
//! error class are not re-derived in every backend.

use agentkit_common::exit::ExitClass;

use super::contract::Error;

/// The failure's meaning, in terms a caller can branch on.
///
/// A wire protocol may only have a handful of codes; this classification keeps
/// the distinctions the contract makes, and surfaces publish it alongside
/// whatever code their protocol requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// The sandbox does not exist.
    NotFound,
    /// The sandbox already exists.
    Conflict,
    /// The mechanism does not provide the operation.
    Unsupported,
    /// Permission was refused.
    Denied,
    /// The mechanism cannot run on this host.
    Unavailable,
    /// The configuration is unusable.
    Misconfigured,
    /// The operation ran and failed.
    Failed,
}

impl FailureClass {
    /// The stable name a machine-readable surface carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Conflict => "conflict",
            Self::Unsupported => "unsupported",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
            Self::Misconfigured => "misconfigured",
            Self::Failed => "failed",
        }
    }
}

/// The class an agent-facing surface reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceClass {
    /// The caller addressed something that is not there, or asked for a name
    /// that is already taken.
    InvalidParams,
    /// The request is well formed but cannot be served: the mechanism lacks the
    /// operation, permission was refused, or the host cannot run it.
    InvalidRequest,
    /// The operation ran and failed; nothing the caller changes will help.
    Internal,
}

impl Error {
    /// The process exit class for this failure.
    pub fn exit_class(&self) -> ExitClass {
        match self.class() {
            FailureClass::NotFound => ExitClass::NotFound,
            FailureClass::Conflict => ExitClass::Conflict,
            FailureClass::Unsupported => ExitClass::Unsupported,
            FailureClass::Denied => ExitClass::Denied,
            FailureClass::Unavailable => ExitClass::Unavailable,
            FailureClass::Misconfigured => ExitClass::Usage,
            FailureClass::Failed => ExitClass::Failed,
        }
    }

    /// The meaning of this failure, finer than any wire protocol's codes.
    pub fn class(&self) -> FailureClass {
        match self {
            Self::NotFound { .. } => FailureClass::NotFound,
            Self::Conflict { .. } => FailureClass::Conflict,
            Self::Unsupported { .. } => FailureClass::Unsupported,
            Self::Denied { .. } => FailureClass::Denied,
            Self::Unavailable { .. } => FailureClass::Unavailable,
            Self::Misconfigured { .. } => FailureClass::Misconfigured,
            Self::Failed { .. } => FailureClass::Failed,
            Self::Provisioning { failure } => match failure.class {
                crate::sandbox::contract::ProvisioningClass::Misconfigured => {
                    FailureClass::Misconfigured
                }
                crate::sandbox::contract::ProvisioningClass::Unsupported => {
                    FailureClass::Unsupported
                }
                crate::sandbox::contract::ProvisioningClass::Denied => FailureClass::Denied,
                crate::sandbox::contract::ProvisioningClass::Unavailable => {
                    FailureClass::Unavailable
                }
                crate::sandbox::contract::ProvisioningClass::Failed => FailureClass::Failed,
            },
        }
    }

    /// The agent-facing class for this failure.
    pub fn surface_class(&self) -> SurfaceClass {
        match self.class() {
            FailureClass::NotFound | FailureClass::Conflict => SurfaceClass::InvalidParams,
            FailureClass::Unsupported | FailureClass::Unavailable | FailureClass::Denied => {
                SurfaceClass::InvalidRequest
            }
            FailureClass::Misconfigured | FailureClass::Failed => SurfaceClass::Internal,
        }
    }

    /// Whether the addressed sandbox does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, FailureClass, SurfaceClass};
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
        assert_ne!(error.class(), FailureClass::NotFound);
        assert!(!error.is_not_found());
    }

    #[test]
    fn every_failure_has_its_own_class_name() {
        let errors = [
            Error::NotFound {
                name: "demo".to_string(),
            },
            Error::Conflict {
                name: "demo".to_string(),
            },
            Error::Unsupported {
                operation: "pause".to_string(),
                backend: "bubblewrap".to_string(),
                primitive: "a sandbox-level state".to_string(),
                reason: "it has none".to_string(),
                remedy: "stop the process".to_string(),
            },
            Error::Denied {
                operation: "read".to_string(),
                reason: "outside the workspace".to_string(),
            },
            Error::Unavailable {
                backend: "bubblewrap".to_string(),
                reason: "user namespaces are disabled".to_string(),
                remedy: "enable them".to_string(),
            },
            Error::Misconfigured {
                message: "backends.docker.image is not set".to_string(),
            },
            Error::failed("backend exploded"),
        ];

        let names: Vec<&str> = errors.iter().map(|error| error.class().as_str()).collect();
        let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        assert!(names.contains(&"not-found"));
        assert!(names.contains(&"unsupported"));
        assert!(names.contains(&"denied"));
    }

    #[test]
    fn unsupported_names_operation_reason_and_remedy() {
        let error = Error::Unsupported {
            operation: "forward port".to_string(),
            backend: "bubblewrap".to_string(),
            primitive: "a host port namespace".to_string(),
            reason: "the sandbox shares no host port namespace".to_string(),
            remedy: "run the service on a forwarded Docker sandbox".to_string(),
        };
        let message = error.to_string();
        assert!(message.contains("forward port"));
        assert!(message.contains("bubblewrap"));
        assert!(message.contains("remedy") || message.contains("run the service"));
        assert_eq!(error.exit_class(), ExitClass::Unsupported);
        assert_eq!(error.surface_class(), SurfaceClass::InvalidRequest);
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
    fn an_unavailable_mechanism_and_bad_configuration_have_their_own_classes() {
        let unavailable = Error::Unavailable {
            backend: "bubblewrap".to_string(),
            reason: "user namespaces are disabled".to_string(),
            remedy: "enable them".to_string(),
        };
        assert_eq!(unavailable.exit_class(), ExitClass::Unavailable);
        assert_eq!(unavailable.surface_class(), SurfaceClass::InvalidRequest);

        let misconfigured = Error::Misconfigured {
            message: "backends.docker.image is not set".to_string(),
        };
        assert_eq!(misconfigured.exit_class(), ExitClass::Usage);
        assert_eq!(misconfigured.surface_class(), SurfaceClass::Internal);
    }
}
