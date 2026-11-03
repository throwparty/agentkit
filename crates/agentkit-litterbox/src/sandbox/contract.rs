//! The platform- and mechanism-neutral sandbox contract.
//!
//! Nothing here names a container, an image, a namespace or a backend. A
//! backend is a mechanism that implements this trait; the CLI and the MCP
//! server hold a boxed trait object and do not know which one they hold.

use std::fmt;
use std::path::PathBuf;

use futures_util::future::BoxFuture;

/// A sandbox as it is remembered between processes.
///
/// The record carries only the identity every surface needs and a payload the
/// backend that created it understands. A backend's own identifier lives in
/// `data`, never in the record's identity.
#[derive(Clone, Debug, PartialEq)]
pub struct SandboxRecord {
    /// The sandbox name, unique within a project.
    pub name: String,
    /// The backend that hosts this sandbox.
    pub backend: String,
    /// The project the sandbox belongs to.
    pub project_slug: String,
    /// Backend-owned addressing data.
    pub data: toml::Value,
}

/// A workspace prepared for materialisation inside a sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// Host directory holding the tree to materialise.
    pub source: PathBuf,
    /// Path the sandbox sees the workspace at.
    pub mount: String,
}

/// A port a project expects to have forwarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestedPort {
    /// Stable name for the port, used to derive the environment variable.
    pub name: String,
    /// Port inside the sandbox.
    pub target: u16,
}

/// Everything a backend needs to create a sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateRequest {
    /// The sandbox name.
    pub name: String,
    /// The project the sandbox belongs to.
    pub project_slug: String,
    /// The prepared workspace.
    pub workspace: Workspace,
    /// A command to run once, confined, before the sandbox is returned.
    pub setup_command: Option<String>,
    /// Ports the project expects to be forwarded.
    pub ports: Vec<RequestedPort>,
}

/// Which sandboxes an enumeration covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every sandbox belonging to one project.
    Repository {
        /// The project slug to enumerate.
        project_slug: String,
    },
    /// Every sandbox belonging to any project.
    Machine,
}

/// The state of a sandbox, in terms every backend can report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxStatus {
    /// Running and able to accept commands.
    Active,
    /// Paused, and resumable.
    Paused,
    /// The record exists but the compute does not.
    Missing,
    /// The backend could not be asked.
    Unknown,
    /// The backend reported a failure.
    Error {
        /// Why the sandbox is in an error state.
        reason: String,
    },
}

/// The result of running one command in a sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Execution {
    /// The command's exit status.
    pub exit_code: i32,
    /// Everything the command wrote to standard output.
    pub stdout: String,
    /// Everything the command wrote to standard error.
    pub stderr: String,
}

impl fmt::Display for Execution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "exit_code={}, stdout=\"{}\", stderr=\"{}\"",
            self.exit_code, self.stdout, self.stderr
        )
    }
}

/// A port this sandbox forwards to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    /// The port name the project declared.
    pub name: String,
    /// Port inside the sandbox.
    pub target: u16,
    /// Port on the host.
    pub host_port: u16,
    /// Address on the host the port is bound to.
    pub bind: String,
    /// Environment variable carrying the host port.
    pub env_var: String,
}

/// What a neutral inspection reports about a sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    /// The sandbox name.
    pub name: String,
    /// The backend hosting it.
    pub backend: String,
    /// Its state.
    pub status: SandboxStatus,
    /// How it reaches the network.
    pub egress: crate::sandbox::report::EgressPosture,
    /// Where the sandbox sees the workspace.
    pub workspace: String,
    /// Where its environment came from and what was changed.
    pub environment: crate::sandbox::environment::Provenance,
    /// Ports forwarded to the host.
    pub forwards: Vec<Forward>,
}

/// A failure classified by meaning rather than by wire format.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The sandbox does not exist.
    #[error("sandbox '{name}' not found")]
    NotFound {
        /// The sandbox that was addressed.
        name: String,
    },
    /// The sandbox already exists.
    #[error("sandbox '{name}' already exists")]
    Conflict {
        /// The sandbox that was addressed.
        name: String,
    },
    /// The backend does not provide this operation.
    #[error("{backend} does not support {operation}: {reason}. {remedy}")]
    Unsupported {
        /// The operation that was requested.
        operation: String,
        /// The backend that cannot perform it.
        backend: String,
        /// Why the mechanism cannot perform it.
        reason: String,
        /// What the user can do instead.
        remedy: String,
    },
    /// Permission was refused.
    #[error("{operation} denied: {reason}")]
    Denied {
        /// The operation that was refused.
        operation: String,
        /// Why it was refused.
        reason: String,
    },
    /// The operation ran and failed.
    #[error("{message}")]
    Failed {
        /// What went wrong, in the backend's own vocabulary.
        message: String,
        /// The underlying cause, when there is one.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

impl Error {
    /// A failure with a message and no further cause.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            source: None,
        }
    }
}

/// The operations every backend can honour.
///
/// Object-safe: methods take `&self` and return boxed futures, so a caller can
/// hold one value whatever backend the user selected.
pub trait Sandbox {
    /// Create a sandbox and return its record.
    fn create(&self, request: &CreateRequest) -> BoxFuture<'_, Result<SandboxRecord, Error>>;

    /// Enumerate sandboxes in the given scope.
    fn list(&self, scope: &Scope) -> BoxFuture<'_, Result<Vec<SandboxRecord>, Error>>;

    /// Run one command.
    fn exec(
        &self,
        sandbox: &SandboxRecord,
        command: &[String],
    ) -> BoxFuture<'_, Result<Execution, Error>>;

    /// Copy a host path into the sandbox.
    fn upload(
        &self,
        sandbox: &SandboxRecord,
        source: &std::path::Path,
        destination: &str,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Copy a sandbox path out to the host.
    fn download(
        &self,
        sandbox: &SandboxRecord,
        source: &str,
        destination: &std::path::Path,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Describe a sandbox without changing it.
    fn inspect(&self, sandbox: &SandboxRecord) -> BoxFuture<'_, Result<Inspection, Error>>;

    /// Remove a sandbox and everything the backend created for it.
    fn delete(&self, sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>>;

    /// Report whether this backend can run here, and why not when it cannot.
    fn readiness(&self) -> BoxFuture<'_, Result<crate::sandbox::report::Readiness, Error>>;

    /// Report which optional operations this backend provides.
    fn capabilities(&self)
    -> BoxFuture<'_, Result<Vec<crate::sandbox::report::Capability>, Error>>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;

    use super::{CreateRequest, Error, Execution, Inspection, Sandbox, SandboxRecord, Scope};

    struct Stub;

    impl Sandbox for Stub {
        fn create(&self, _request: &CreateRequest) -> BoxFuture<'_, Result<SandboxRecord, Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn list(&self, _scope: &Scope) -> BoxFuture<'_, Result<Vec<SandboxRecord>, Error>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn exec(
            &self,
            _sandbox: &SandboxRecord,
            _command: &[String],
        ) -> BoxFuture<'_, Result<Execution, Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn upload(
            &self,
            _sandbox: &SandboxRecord,
            _source: &std::path::Path,
            _destination: &str,
        ) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn download(
            &self,
            _sandbox: &SandboxRecord,
            _source: &str,
            _destination: &std::path::Path,
        ) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn inspect(&self, _sandbox: &SandboxRecord) -> BoxFuture<'_, Result<Inspection, Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn delete(&self, _sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn readiness(&self) -> BoxFuture<'_, Result<crate::sandbox::report::Readiness, Error>> {
            Box::pin(async {
                Ok(crate::sandbox::report::Readiness {
                    backend: "stub".to_string(),
                    checks: Vec::new(),
                })
            })
        }

        fn capabilities(
            &self,
        ) -> BoxFuture<'_, Result<Vec<crate::sandbox::report::Capability>, Error>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[test]
    fn the_contract_is_object_safe() {
        let sandbox: Arc<dyn Sandbox> = Arc::new(Stub);
        let record = SandboxRecord {
            name: "demo".to_string(),
            backend: "stub".to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::Table(Default::default()),
        };
        assert_eq!(record.name, "demo");
        assert!(Arc::strong_count(&sandbox) == 1);
    }
}
