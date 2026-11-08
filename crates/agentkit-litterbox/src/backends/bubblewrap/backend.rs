//! Bubblewrap's implementation of the contract.
//!
//! The mechanism is built in stages: this module holds the shape the registry
//! constructs, and the later tasks fill in the view, the holder and the join.

use std::path::Path;

use futures_util::future::BoxFuture;

use super::BACKEND;
use super::host::SharedHost;
use crate::sandbox::contract::{
    Backend, ContributionSupport, CreateRequest, Error, Execution, Inspection, SandboxRecord, Scope,
};
use crate::sandbox::report::{Capability, Check, Readiness, State};

/// Bubblewrap's settings, typed here and nowhere else.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BubblewrapSettings {
    /// Memory ceiling each sandbox receives, in MiB.
    pub memory_mb: Option<u64>,
    /// Maximum tasks or processes each sandbox may create.
    pub tasks: Option<u32>,
}

/// Bubblewrap's implementation of the contract.
pub struct BubblewrapBackend {
    settings: BubblewrapSettings,
    host: SharedHost,
}

impl BubblewrapBackend {
    /// Build a backend from its validated settings and the host it reads.
    pub fn new(settings: BubblewrapSettings, host: SharedHost) -> Self {
        Self { settings, host }
    }

    /// The settings the backend was built with.
    pub fn settings(&self) -> &BubblewrapSettings {
        &self.settings
    }

    /// The host reads and writes.
    pub fn host(&self) -> &SharedHost {
        &self.host
    }

    fn not_wired(&self, operation: &str) -> Error {
        Error::Unavailable {
            backend: BACKEND.to_string(),
            reason: format!("{operation} is not implemented yet"),
            remedy: "use a backend that implements it until the bubblewrap backend lands"
                .to_string(),
        }
    }
}

impl Backend for BubblewrapBackend {
    fn create<'a>(
        &'a self,
        _request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move { Err(self.not_wired("create")) })
    }

    fn list<'a>(&'a self, _scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn exec<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(async move { Err(self.not_wired("a command")) })
    }

    fn upload<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a Path,
        _destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { Err(self.not_wired("upload")) })
    }

    fn download<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a str,
        _destination: &'a Path,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { Err(self.not_wired("download")) })
    }

    fn inspect<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move { Err(self.not_wired("inspection")) })
    }

    fn delete<'a>(&'a self, _sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async {
            Ok(Readiness {
                backend: BACKEND.to_string(),
                checks: vec![Check {
                    primitive: "the bubblewrap backend".to_string(),
                    state: State::Unavailable,
                    reason: Some("the backend is not wired yet".to_string()),
                    remedy: Some(
                        "use a backend that is implemented until the bubblewrap backend lands"
                            .to_string(),
                    ),
                    fallback: None,
                }],
            })
        })
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<Vec<Capability>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn contribution_support(&self) -> Vec<ContributionSupport> {
        Vec::new()
    }
}
