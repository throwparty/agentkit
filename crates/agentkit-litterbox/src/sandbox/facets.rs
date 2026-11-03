//! Optional operations a mechanism may or may not provide.
//!
//! A facet is a supertrait of [`Sandbox`]: implementing it means the backend
//! can do these things *as well as* everything in the contract. Code that
//! requires a facet is compiled against a backend that has it, so a mechanism
//! without one cannot be asked for the operation by accident.

//! A core-only value cannot be asked for a facet operation:
//!
//! ```compile_fail
//! use agentkit_litterbox::sandbox::contract::Sandbox;
//! use agentkit_litterbox::sandbox::facets::Lifecycle;
//!
//! fn pause_all(lifecycle: &dyn Lifecycle) {
//!     let _ = lifecycle;
//! }
//!
//! fn from_core(core: &dyn Sandbox) {
//!     pause_all(core);
//! }
//! ```

use futures_util::future::BoxFuture;

use super::contract::{Error, Forward, Sandbox, SandboxRecord};

/// Bounds on the resources a sandbox may consume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Memory ceiling in bytes.
    pub memory_bytes: Option<u64>,
    /// Maximum number of tasks or processes.
    pub tasks: Option<u32>,
}

/// Pausing and resuming a sandbox.
pub trait Lifecycle: Sandbox {
    /// Stop the sandbox's work without destroying it.
    fn pause(&self, sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>>;

    /// Continue a paused sandbox.
    fn resume(&self, sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>>;
}

/// Bounding what a sandbox may consume.
pub trait Limits: Sandbox {
    /// Apply limits to a running sandbox.
    fn set_limits(
        &self,
        sandbox: &SandboxRecord,
        limits: &ResourceLimits,
    ) -> BoxFuture<'_, Result<(), Error>>;
}

/// Ports forwarded from the sandbox to the host.
pub trait Forwarding: Sandbox {
    /// Report the ports this sandbox forwards.
    fn forwards(&self, sandbox: &SandboxRecord) -> BoxFuture<'_, Result<Vec<Forward>, Error>>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;

    use super::{Lifecycle, ResourceLimits};
    use crate::sandbox::contract::{
        CreateRequest, Error, Execution, Inspection, Sandbox, SandboxRecord, Scope,
    };

    struct Pausable;

    impl Sandbox for Pausable {
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
                    backend: "pausable".to_string(),
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

    impl Lifecycle for Pausable {
        fn pause(&self, _sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn resume(&self, _sandbox: &SandboxRecord) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn record() -> SandboxRecord {
        SandboxRecord {
            name: "demo".to_string(),
            backend: "pausable".to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::Table(Default::default()),
        }
    }

    #[tokio::test]
    async fn one_object_is_both_the_contract_and_its_facet() {
        let backend = Arc::new(Pausable);
        let core: Arc<dyn Sandbox> = backend.clone();
        let lifecycle: Arc<dyn Lifecycle> = backend;

        lifecycle.pause(&record()).await.expect("pause works");
        core.list(&Scope::Machine).await.expect("list works");
    }

    #[test]
    fn limits_default_to_unbounded() {
        let limits = ResourceLimits::default();
        assert!(limits.memory_bytes.is_none());
        assert!(limits.tasks.is_none());
    }
}
