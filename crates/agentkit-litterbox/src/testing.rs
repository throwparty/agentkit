//! The conformance suite a backend must pass.
//!
//! One generic exercise drives the contract's whole surface. It is run against
//! [`TestBackend`] always, and against the Docker backend when a daemon is
//! available, with no backend-specific branch in the suite itself.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use crate::sandbox::contract::{
    Backend, ContributionSupport, CreateRequest, Error, Execution, Inspection, ProvisionStep,
    ProvisioningClass, ProvisioningFailure, SandboxRecord, SandboxStatus, Scope, Workspace,
};
use crate::sandbox::environment::{Origin, Provenance, StepProvenance};
use crate::sandbox::facets::Lifecycle;
use crate::sandbox::report::{EgressPosture, Fallback, Readiness, State};

/// An in-memory backend that honours the contract without confining anything.
pub struct TestBackend {
    records: Mutex<Vec<SandboxRecord>>,
    files: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    paused: Mutex<std::collections::HashSet<String>>,
    requests: Mutex<Vec<CreateRequest>>,
    environments: Mutex<std::collections::HashMap<String, Provenance>>,
    fail_create: bool,
    provision_failure: Option<String>,
    provision_state: State,
    workspace: String,
}

impl Default for TestBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TestBackend {
    /// An empty backend.
    pub fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            files: Mutex::new(std::collections::HashMap::new()),
            paused: Mutex::new(std::collections::HashSet::new()),
            requests: Mutex::new(Vec::new()),
            environments: Mutex::new(std::collections::HashMap::new()),
            fail_create: false,
            provision_failure: None,
            provision_state: State::Available,
            workspace: "/sandbox".to_string(),
        }
    }

    /// A backend whose create always fails, for rollback tests.
    pub fn failing() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            files: Mutex::new(std::collections::HashMap::new()),
            paused: Mutex::new(std::collections::HashSet::new()),
            requests: Mutex::new(Vec::new()),
            environments: Mutex::new(std::collections::HashMap::new()),
            fail_create: true,
            provision_failure: None,
            provision_state: State::Available,
            workspace: "/sandbox".to_string(),
        }
    }

    /// A backend whose create fails when it reaches the named step.
    pub fn failing_step(identity: &str) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            files: Mutex::new(std::collections::HashMap::new()),
            paused: Mutex::new(std::collections::HashSet::new()),
            requests: Mutex::new(Vec::new()),
            environments: Mutex::new(std::collections::HashMap::new()),
            fail_create: false,
            provision_failure: Some(identity.to_string()),
            provision_state: State::Available,
            workspace: "/sandbox".to_string(),
        }
    }

    /// A backend that reports the given provisioning support state.
    pub fn with_provision_state(mut self, state: State) -> Self {
        self.provision_state = state;
        self
    }

    /// The create requests this backend has seen, in order.
    pub fn requests(&self) -> Vec<CreateRequest> {
        self.requests.lock().expect("test backend lock").clone()
    }
}

impl Backend for TestBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move {
            self.requests
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .push(request.clone());
            if self.fail_create {
                return Err(Error::failed("the mechanism refused to create the sandbox"));
            }
            if let Some(identity) = &self.provision_failure
                && request
                    .provision
                    .iter()
                    .any(|step| step.identity() == identity)
            {
                return Err(Error::Provisioning {
                    failure: Box::new(ProvisioningFailure {
                        class: ProvisioningClass::Failed,
                        backend: Some("test".to_string()),
                        step: Some(identity.clone()),
                        kind: Some("command".to_string()),
                        reason: format!("provisioning step '{identity}' failed with exit code 7"),
                        remedy: "fix the step and create again".to_string(),
                        exit_code: Some(7),
                        output: Some("boom".to_string()),
                    }),
                });
            }
            {
                let records = self
                    .records
                    .lock()
                    .map_err(|_| Error::failed("test backend lock poisoned"))?;
                if records.iter().any(|existing| existing.name == request.name) {
                    return Err(Error::Conflict {
                        name: request.name.clone(),
                    });
                }
            }
            if let Ok(bytes) = std::fs::read(request.workspace.source.join("README.md")) {
                self.files
                    .lock()
                    .map_err(|_| Error::failed("test backend lock poisoned"))?
                    .insert(format!("{}/README.md", self.workspace), bytes);
            }
            let record = SandboxRecord {
                name: request.name.clone(),
                backend: "test".to_string(),
                project_slug: request.project_slug.clone(),
                data: toml::Value::Table(Default::default()),
            };
            self.records
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .push(record.clone());
            let steps = request
                .provision
                .iter()
                .map(|step| StepProvenance {
                    identity: step.identity().to_string(),
                    kind: step.kind(),
                    variables: Vec::new(),
                })
                .collect();
            self.environments
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .insert(
                    request.name.clone(),
                    Provenance {
                        origin: Origin::Default,
                        variables: Vec::new(),
                        host_paths: Vec::new(),
                        steps,
                    },
                );
            Ok(record)
        })
    }

    fn list<'a>(&'a self, scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async move {
            let records = self
                .records
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .clone();
            Ok(match scope {
                Scope::Repository { project_slug } => records
                    .into_iter()
                    .filter(|record| &record.project_slug == project_slug)
                    .collect(),
                Scope::Machine => records,
            })
        })
    }

    fn exec<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(async move {
            let script = command.last().cloned().unwrap_or_default();
            if let Some(path) = script.split_whitespace().find(|part| part.starts_with('/')) {
                let files = self
                    .files
                    .lock()
                    .map_err(|_| Error::failed("test backend lock poisoned"))?;
                if let Some(bytes) = files.get(path) {
                    return Ok(Execution {
                        exit_code: 0,
                        stdout: String::from_utf8_lossy(bytes).into_owned(),
                        stderr: String::new(),
                    });
                }
                return Err(Error::NotFound {
                    name: path.to_string(),
                });
            }
            Ok(Execution {
                exit_code: 0,
                stdout: "conformance\n".to_string(),
                stderr: String::new(),
            })
        })
    }

    fn upload<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !destination.starts_with(&self.workspace) {
                return Err(Error::Denied {
                    operation: format!("upload to {destination}"),
                    reason: "outside the sandbox workspace".to_string(),
                });
            }
            let name = source
                .file_name()
                .ok_or_else(|| Error::failed("the upload path has no file name"))?;
            let bytes = std::fs::read(source)
                .map_err(|error| Error::failed(format!("could not read the fixture: {error}")))?;
            self.files
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .insert(format!("{destination}/{}", name.to_string_lossy()), bytes);
            Ok(())
        })
    }

    fn download<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !source.starts_with(&self.workspace) {
                return Err(Error::Denied {
                    operation: format!("download from {source}"),
                    reason: "outside the sandbox workspace".to_string(),
                });
            }
            let bytes = self
                .files
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .get(source)
                .cloned()
                .ok_or_else(|| Error::NotFound {
                    name: source.to_string(),
                })?;
            let name = std::path::Path::new(source)
                .file_name()
                .ok_or_else(|| Error::failed("the transfer path has no file name"))?;
            let target = destination.join(name);
            std::fs::write(&target, bytes)
                .map_err(|error| Error::failed(format!("could not write the download: {error}")))?;
            Ok(())
        })
    }

    fn inspect<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move {
            let exists = self
                .records
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .iter()
                .any(|record| record.name == sandbox.name);
            let paused = self
                .paused
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .contains(&sandbox.name);
            let status = if !exists {
                SandboxStatus::Missing
            } else if paused {
                SandboxStatus::Paused
            } else {
                SandboxStatus::Active
            };
            Ok(Inspection {
                name: sandbox.name.clone(),
                backend: "test".to_string(),
                status,
                egress: EgressPosture::Denied,
                workspace: self.workspace.clone(),
                environment: self
                    .environments
                    .lock()
                    .map_err(|_| Error::failed("test backend lock poisoned"))?
                    .get(&sandbox.name)
                    .cloned()
                    .unwrap_or(Provenance {
                        origin: Origin::Default,
                        variables: Vec::new(),
                        host_paths: Vec::new(),
                        steps: Vec::new(),
                    }),
                forwards: Vec::new(),
            })
        })
    }

    fn delete<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            self.records
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .retain(|record| record.name != sandbox.name);
            self.paused
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .remove(&sandbox.name);
            Ok(())
        })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async {
            Ok(Readiness {
                backend: "test".to_string(),
                checks: Vec::new(),
            })
        })
    }

    fn capabilities<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Vec<crate::sandbox::report::Capability>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn contribution_support(&self) -> Vec<ContributionSupport> {
        Vec::new()
    }
}

impl Lifecycle for TestBackend {
    fn pause<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            self.paused
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .insert(sandbox.name.clone());
            Ok(())
        })
    }

    fn resume<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            self.paused
                .lock()
                .map_err(|_| Error::failed("test backend lock poisoned"))?
                .remove(&sandbox.name);
            Ok(())
        })
    }
}

/// A name that does not collide with a concurrent or previous run.
pub fn run_name(stem: &str) -> String {
    format!("{stem}-{}", std::process::id())
}

/// Drive a backend through the whole contract.
pub async fn exercise<S: Backend + ?Sized>(backend: &S) -> Result<(), Error> {
    let workspace = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage a workspace: {error}")))?;
    std::fs::write(workspace.path().join("README.md"), "hello\n")
        .map_err(|error| Error::failed(format!("could not write the fixture: {error}")))?;

    let name = run_name("conformance");
    let request = CreateRequest {
        name: name.clone(),
        project_slug: "conformance".to_string(),
        workspace: Workspace {
            source: workspace.path().to_path_buf(),
        },
        provision: Vec::new(),
        forward_bind: "127.0.0.1".to_string(),
        ports: Vec::new(),
    };

    let record = backend.create(&request).await?;
    assert_eq!(record.name, name, "create returns the name");

    match backend.create(&request).await {
        Err(Error::Conflict { .. }) => {}
        other => panic!("a duplicate name must conflict, got {other:?}"),
    }

    let scope = Scope::Repository {
        project_slug: "conformance".to_string(),
    };
    let inspection = backend.inspect(&record).await?;
    let workspace_path = inspection.workspace.clone();
    assert!(
        workspace_path.starts_with('/'),
        "the backend reports an absolute workspace: {workspace_path}"
    );

    let prepared = backend
        .exec(
            &record,
            &[
                "sh".to_string(),
                "-c".to_string(),
                format!("cat {workspace_path}/README.md"),
            ],
        )
        .await?;
    assert_eq!(
        prepared.stdout, "hello\n",
        "create materialised the prepared workspace"
    );

    let listed = backend.list(&scope).await?;
    assert!(
        listed.iter().any(|entry| entry.name == record.name),
        "a created sandbox is enumerable"
    );
    let machine = backend.list(&Scope::Machine).await?;
    assert!(
        machine.iter().any(|entry| entry.name == record.name),
        "a created sandbox appears in machine scope"
    );
    let other = backend
        .list(&Scope::Repository {
            project_slug: "another-project".to_string(),
        })
        .await?;
    assert!(
        other.is_empty(),
        "repository scope does not leak another project: {other:?}"
    );

    let execution = backend
        .exec(
            &record,
            &[
                "sh".to_string(),
                "-c".to_string(),
                "echo conformance".to_string(),
            ],
        )
        .await?;
    assert_eq!(
        execution.exit_code, 0,
        "exec succeeds: {}",
        execution.stderr
    );
    assert!(
        execution.stdout.contains("conformance"),
        "exec returns stdout: {}",
        execution.stdout
    );

    // Uploading into a populated directory merges rather than replaces.
    let extra = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage an upload: {error}")))?;
    std::fs::write(extra.path().join("notes.txt"), "notes\n")
        .map_err(|error| Error::failed(format!("could not write the fixture: {error}")))?;
    backend
        .upload(&record, &extra.path().join("notes.txt"), &workspace_path)
        .await?;

    let uploaded = backend
        .exec(
            &record,
            &[
                "sh".to_string(),
                "-c".to_string(),
                format!("cat {workspace_path}/notes.txt"),
            ],
        )
        .await?;
    assert_eq!(uploaded.stdout, "notes\n", "the upload landed");

    let existing = backend
        .exec(
            &record,
            &[
                "sh".to_string(),
                "-c".to_string(),
                format!("cat {workspace_path}/README.md"),
            ],
        )
        .await?;
    assert_eq!(
        existing.stdout, "hello\n",
        "an upload merges into a populated directory"
    );

    // A destination outside the workspace is refused, not silently confined.
    match backend
        .upload(&record, &extra.path().join("notes.txt"), "/etc")
        .await
    {
        Err(Error::Denied { .. }) | Err(Error::Unsupported { .. }) => {}
        other => panic!("an escaping upload must be refused, got {other:?}"),
    }

    let downloaded = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage a download: {error}")))?;
    backend
        .download(
            &record,
            &format!("{workspace_path}/README.md"),
            downloaded.path(),
        )
        .await?;
    let round_trip = std::fs::read_to_string(downloaded.path().join("README.md"))
        .map_err(|error| Error::failed(format!("the download wrote nothing: {error}")))?;
    assert_eq!(round_trip, "hello\n", "transfer round-trips the bytes");

    let inspection = backend.inspect(&record).await?;
    assert_eq!(
        inspection.status,
        SandboxStatus::Active,
        "inspect reports active"
    );
    assert_eq!(
        inspection.workspace, workspace_path,
        "inspect reports the workspace"
    );

    backend.delete(&record).await?;
    let listed = backend.list(&scope).await?;
    assert!(
        !listed.iter().any(|entry| entry.name == record.name),
        "delete removes the record"
    );
    assert_eq!(
        backend.inspect(&record).await?.status,
        SandboxStatus::Missing,
        "a deleted sandbox is reported missing"
    );
    backend.delete(&record).await?;

    exercise_provisioning(backend).await?;

    Ok(())
}

/// Drive provisioning: an ordered sequence must be applied and reported.
pub async fn exercise_provisioning<S: Backend + ?Sized>(backend: &S) -> Result<(), Error> {
    let workspace = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage a workspace: {error}")))?;
    std::fs::write(workspace.path().join("README.md"), "hello\n")
        .map_err(|error| Error::failed(format!("could not write the fixture: {error}")))?;

    let request = CreateRequest {
        name: run_name("provisioning"),
        project_slug: "conformance".to_string(),
        workspace: Workspace {
            source: workspace.path().to_path_buf(),
        },
        provision: vec![
            ProvisionStep::Command {
                identity: "z-first".to_string(),
                run: "true".to_string(),
            },
            ProvisionStep::Command {
                identity: "a-second".to_string(),
                run: "true".to_string(),
            },
        ],
        forward_bind: "127.0.0.1".to_string(),
        ports: Vec::new(),
    };

    let record = backend.create(&request).await?;
    let inspection = backend.inspect(&record).await?;
    let identities: Vec<&str> = inspection
        .environment
        .steps
        .iter()
        .map(|step| step.identity.as_str())
        .collect();
    assert_eq!(
        identities,
        vec!["z-first", "a-second"],
        "the backend reports the resolved steps in list order"
    );
    assert!(
        inspection
            .environment
            .steps
            .iter()
            .all(|step| step.kind == ProvisionKind::Command),
        "the backend reports each step's kind"
    );

    backend.delete(&record).await?;
    Ok(())
}

/// Drive the lifecycle facet: pause, observe, resume, observe.
pub async fn exercise_lifecycle<S: Lifecycle + ?Sized>(backend: &S) -> Result<(), Error> {
    let workspace = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage a workspace: {error}")))?;
    std::fs::write(workspace.path().join("README.md"), "hello\n")
        .map_err(|error| Error::failed(format!("could not write the fixture: {error}")))?;

    let request = CreateRequest {
        name: run_name("lifecycle"),
        project_slug: "conformance".to_string(),
        workspace: Workspace {
            source: workspace.path().to_path_buf(),
        },
        provision: Vec::new(),
        forward_bind: "127.0.0.1".to_string(),
        ports: Vec::new(),
    };

    let record = backend.create(&request).await?;
    backend.pause(&record).await?;
    assert_eq!(
        backend.inspect(&record).await?.status,
        SandboxStatus::Paused,
        "pause is observable"
    );
    backend.resume(&record).await?;
    assert_eq!(
        backend.inspect(&record).await?.status,
        SandboxStatus::Active,
        "resume is observable"
    );
    backend.delete(&record).await?;
    Ok(())
}

/// An operation the mechanism does not provide is refused structurally.
pub fn exercise_absent_facet() -> Result<(), Error> {
    let core: Arc<dyn Backend> = Arc::new(TestBackend::new());
    let selected = crate::sandbox::selected::Selected::new("test", core);

    match selected.lifecycle_or_unsupported("pause") {
        Err(Error::Unsupported {
            operation,
            backend,
            primitive,
            reason,
            remedy,
        }) => {
            assert_eq!(operation, "pause");
            assert_eq!(backend, "test");
            assert!(!primitive.is_empty(), "the missing primitive is named");
            assert!(!reason.is_empty(), "the reason is given");
            assert!(!remedy.is_empty(), "a remedy is offered");
            Ok(())
        }
        Err(other) => Err(Error::failed(format!(
            "an absent facet must be unsupported, got {other}"
        ))),
        Ok(_) => Err(Error::failed(
            "an absent facet was accepted instead of refused",
        )),
    }
}

/// Drive a backend whose create is expected to fail, and prove nothing
/// survived it.
pub async fn exercise_failed_create<S: Backend + ?Sized>(backend: &S) -> Result<(), Error> {
    exercise_failed_create_with(backend, &run_name("conformance-failure"), Vec::new()).await
}

/// Drive a failing create, optionally failing through a provisioning step.
pub async fn exercise_failed_create_with<S: Backend + ?Sized>(
    backend: &S,
    name: &str,
    provision: Vec<ProvisionStep>,
) -> Result<(), Error> {
    let workspace = tempfile::tempdir()
        .map_err(|error| Error::failed(format!("could not stage a workspace: {error}")))?;
    std::fs::write(workspace.path().join("README.md"), "hello\n")
        .map_err(|error| Error::failed(format!("could not write the fixture: {error}")))?;

    let request = CreateRequest {
        name: name.to_string(),
        project_slug: "conformance".to_string(),
        workspace: Workspace {
            source: workspace.path().to_path_buf(),
        },
        provision,
        forward_bind: "127.0.0.1".to_string(),
        ports: Vec::new(),
    };

    backend
        .create(&request)
        .await
        .expect_err("the backend must refuse to create");

    let listed = backend
        .list(&Scope::Repository {
            project_slug: "conformance".to_string(),
        })
        .await?;
    assert!(
        listed.iter().all(|record| record.name != name),
        "a failed create left a record behind"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        TestBackend, exercise, exercise_absent_facet, exercise_failed_create, exercise_lifecycle,
    };
    use crate::sandbox::contract::{
        Backend, CreateRequest, ProvisionKind, ProvisionStep, Workspace,
    };
    use crate::sandbox::report::State;

    #[tokio::test]
    async fn the_test_backend_conforms() {
        let backend = TestBackend::new();
        exercise(&backend).await.expect("the contract is honoured");
    }

    #[tokio::test]
    async fn the_test_backend_honours_the_lifecycle_facet() {
        let backend = TestBackend::new();
        exercise_lifecycle(&backend)
            .await
            .expect("pause and resume are observable");
    }

    #[tokio::test]
    async fn a_failed_create_leaves_no_record() {
        let backend = TestBackend::failing();
        exercise_failed_create(&backend)
            .await
            .expect("a failure leaves nothing behind");
    }

    #[test]
    fn an_absent_facet_is_reported_structurally() {
        exercise_absent_facet().expect("the refusal names its primitive, reason and remedy");
    }

    fn request(provision: Vec<ProvisionStep>) -> CreateRequest {
        CreateRequest {
            name: "recorded".to_string(),
            project_slug: "conformance".to_string(),
            workspace: Workspace {
                source: std::path::PathBuf::from("/tmp/workspace"),
            },
            provision,
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
        }
    }

    #[tokio::test]
    async fn the_test_backend_records_the_request_verbatim() {
        let backend = TestBackend::new();
        let provision = vec![
            ProvisionStep::Command {
                identity: "first".to_string(),
                run: "true".to_string(),
            },
            ProvisionStep::Command {
                identity: "second".to_string(),
                run: "true".to_string(),
            },
        ];
        backend
            .create(&request(provision.clone()))
            .await
            .expect("create");
        let recorded = backend.requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].provision, provision);
    }

    #[tokio::test]
    async fn the_test_backend_records_an_empty_sequence() {
        let backend = TestBackend::new();
        backend.create(&request(Vec::new())).await.expect("create");
        assert!(backend.requests()[0].provision.is_empty());
    }

    #[test]
    fn the_test_backend_reports_no_contributions() {
        assert!(TestBackend::new().contribution_support().is_empty());
    }

    #[tokio::test]
    async fn a_recorded_request_carries_only_its_own_steps() {
        let backend = TestBackend::new();
        let provision = vec![ProvisionStep::Command {
            identity: "fetch".to_string(),
            run: "true".to_string(),
        }];
        backend
            .create(&request(provision.clone()))
            .await
            .expect("create");
        let recorded = &backend.requests()[0];
        assert_eq!(
            recorded.provision, provision,
            "the request carries exactly the steps it was given"
        );
    }
}
