//! Bulk operations that must not be derailed by a sandbox disappearing.

use crate::sandbox::contract::SandboxRecord;
use crate::sandbox::facets::Lifecycle;

/// What a bulk pause managed to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PauseSummary {
    /// Sandboxes that paused.
    pub paused: usize,
    /// Sandboxes that had already gone.
    pub skipped: usize,
    /// Failures, each naming the sandbox and the reason.
    pub failures: Vec<String>,
}

impl PauseSummary {
    /// Whether every sandbox that still exists paused.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Pause every record, treating one that has already gone as success and
/// finishing the batch even when one record fails.
pub async fn pause_all<L: Lifecycle + ?Sized>(
    records: &[SandboxRecord],
    lifecycle: &L,
) -> PauseSummary {
    let mut summary = PauseSummary::default();
    for record in records {
        match lifecycle.pause(record).await {
            Ok(()) => summary.paused += 1,
            Err(error) if error.is_not_found() => summary.skipped += 1,
            Err(error) => {
                tracing::warn!(sandbox = %record.name, %error, "the sandbox could not be paused");
                summary.failures.push(format!("{}: {error}", record.name));
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use futures_util::future::BoxFuture;

    use super::pause_all;
    use crate::sandbox::contract::{
        Backend, CreateRequest, Error, Execution, Inspection, SandboxRecord, Scope,
    };
    use crate::sandbox::facets::Lifecycle;

    /// A backend that pauses everything except the sandboxes it is told to
    /// treat as missing or broken.
    struct Fake {
        missing: Option<&'static str>,
        broken: Option<&'static str>,
    }

    impl Backend for Fake {
        fn create<'a>(
            &'a self,
            _request: &'a CreateRequest,
        ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
            Box::pin(async { Err(Error::failed("not used")) })
        }

        fn list<'a>(
            &'a self,
            _scope: &'a Scope,
        ) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn exec<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
            _command: &'a [String],
        ) -> BoxFuture<'a, Result<Execution, Error>> {
            Box::pin(async { Err(Error::failed("not used")) })
        }

        fn upload<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
            _source: &'a std::path::Path,
            _destination: &'a str,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("not used")) })
        }

        fn download<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
            _source: &'a str,
            _destination: &'a std::path::Path,
            _selector: Option<&'a dyn crate::sandbox::contract::SnapshotSelector>,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("not used")) })
        }

        fn inspect<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
        ) -> BoxFuture<'a, Result<Inspection, Error>> {
            Box::pin(async { Err(Error::failed("not used")) })
        }

        fn delete<'a>(&'a self, _sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn readiness<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<crate::sandbox::report::Readiness, Error>> {
            Box::pin(async {
                Ok(crate::sandbox::report::Readiness {
                    backend: "fake".to_string(),
                    checks: Vec::new(),
                })
            })
        }

        fn capabilities<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<Vec<crate::sandbox::report::Capability>, Error>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn contribution_support(&self) -> Vec<crate::sandbox::contract::ContributionSupport> {
            Vec::new()
        }
    }

    impl Lifecycle for Fake {
        fn pause<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async move {
                if self.missing == Some(sandbox.name.as_str()) {
                    return Err(Error::NotFound {
                        name: sandbox.name.clone(),
                    });
                }
                if self.broken == Some(sandbox.name.as_str()) {
                    return Err(Error::failed("the mechanism broke"));
                }
                Ok(())
            })
        }

        fn resume<'a>(&'a self, _sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn record(name: &str) -> SandboxRecord {
        SandboxRecord {
            name: name.to_string(),
            backend: "fake".to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::Table(Default::default()),
        }
    }

    #[tokio::test]
    async fn every_sandbox_is_paused() {
        let records = vec![record("one"), record("two"), record("three")];
        let fake = Fake {
            missing: None,
            broken: None,
        };
        let summary = pause_all(&records, &fake).await;
        assert_eq!(summary.paused, 3);
        assert_eq!(summary.skipped, 0);
        assert!(summary.is_complete());
    }

    #[tokio::test]
    async fn a_vanished_sandbox_is_skipped_not_fatal() {
        let records = vec![record("one"), record("gone"), record("three")];
        let fake = Fake {
            missing: Some("gone"),
            broken: None,
        };
        let summary = pause_all(&records, &fake).await;
        assert_eq!(summary.paused, 2);
        assert_eq!(summary.skipped, 1);
        assert!(summary.is_complete());
    }

    #[tokio::test]
    async fn a_real_failure_is_reported_and_the_batch_continues() {
        let records = vec![record("one"), record("bad"), record("three")];
        let fake = Fake {
            missing: None,
            broken: Some("bad"),
        };
        let summary = pause_all(&records, &fake).await;
        assert_eq!(
            summary.paused, 2,
            "the sandboxes after the failure still pause"
        );
        assert_eq!(summary.failures.len(), 1);
        assert!(
            summary.failures[0].contains("bad"),
            "{:?}",
            summary.failures
        );
        assert!(summary.failures[0].contains("the mechanism broke"));
        assert!(!summary.is_complete());
    }
}
