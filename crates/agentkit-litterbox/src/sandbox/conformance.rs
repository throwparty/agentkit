//! The conformance suite a backend must pass.
//!
//! One generic exercise drives the contract's whole surface. It is run against
//! [`TestBackend`] always, and against the Docker backend when a daemon is
//! available, with no backend-specific branch in the suite itself.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use super::contract::{
    Backend, CreateRequest, Error, Execution, Inspection, SandboxRecord, SandboxStatus, Scope,
    Workspace,
};
use super::environment::{Origin, Provenance};
use super::facets::Lifecycle;
use super::report::{EgressPosture, Readiness};

/// An in-memory backend that honours the contract without confining anything.
pub struct TestBackend {
    records: Mutex<Vec<SandboxRecord>>,
    files: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    paused: Mutex<std::collections::HashSet<String>>,
    fail_create: bool,
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
            fail_create: false,
            workspace: "/sandbox".to_string(),
        }
    }

    /// A backend whose create always fails, for rollback tests.
    pub fn failing() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            files: Mutex::new(std::collections::HashMap::new()),
            paused: Mutex::new(std::collections::HashSet::new()),
            fail_create: true,
            workspace: "/sandbox".to_string(),
        }
    }
}

impl Backend for TestBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move {
            if self.fail_create {
                return Err(Error::failed("the mechanism refused to create the sandbox"));
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
                environment: Provenance {
                    origin: Origin::Default,
                    variables: Vec::new(),
                    host_paths: Vec::new(),
                },
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

    fn capabilities<'a>(&'a self) -> BoxFuture<'a, Result<Vec<super::report::Capability>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
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
        environment: super::contract::EnvironmentSource::Mechanism,
        forward_bind: "127.0.0.1".to_string(),
        setup_command: None,
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
        environment: super::contract::EnvironmentSource::Mechanism,
        forward_bind: "127.0.0.1".to_string(),
        setup_command: None,
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
    let selected = super::selected::Selected::new("test", core);

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
    exercise_failed_create_with(backend, &run_name("conformance-failure"), None).await
}

/// Drive a failing create, optionally failing through the setup command.
pub async fn exercise_failed_create_with<S: Backend + ?Sized>(
    backend: &S,
    name: &str,
    setup_command: Option<&str>,
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
        environment: super::contract::EnvironmentSource::Mechanism,
        forward_bind: "127.0.0.1".to_string(),
        setup_command: setup_command.map(str::to_string),
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
}
