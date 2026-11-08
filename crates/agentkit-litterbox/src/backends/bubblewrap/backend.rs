//! Bubblewrap's implementation of the contract.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use futures_util::future::BoxFuture;

use super::BACKEND;
use super::host::SharedHost;
use super::payload::{BubblewrapData, RecordedMount, RecordedRoot};
use super::view::{Boundaries, RenderOptions, Requested, View, WORKSPACE};
use super::{environment, holder, join, validate, workspace};
use crate::backends::shared::run_steps;
use crate::sandbox::contract::{
    Backend, ContributionSort, ContributionSupport, CreateRequest, Error, Execution, Inspection,
    ProvisioningClass, ProvisioningFailure, SandboxRecord, Scope,
};
use crate::sandbox::report::{Capability, Check, Readiness, State};

/// The descriptor bwrap reports its status on.
const STATUS_FD: i32 = 3;

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

    fn boundaries(&self, project: &str, name: &str) -> Result<Boundaries, Error> {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Err(Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: "HOME is not set, so the backend cannot tell which host tree holds the \
                         user's files"
                    .to_string(),
                remedy: "set HOME before running litterbox".to_string(),
            });
        };
        Ok(Boundaries {
            home,
            metadata: agentkit_common::data_dir("litterbox").join("metadata"),
            state: workspace::sandbox_directory(project, name),
        })
    }

    fn scoped(&self) -> bool {
        self.host.resolve("systemd-run").is_some()
    }

    /// Gather the mounts the steps contributed.
    fn contributed_mounts(request: &CreateRequest) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for step in &request.provision {
            for path in &step.contributions.mounts {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    /// Build the view a sandbox receives.
    fn build_view(
        &self,
        request: &CreateRequest,
        holder_command: &holder::Command,
        workspace: &Path,
    ) -> Result<View, Error> {
        View::build(
            &Self::requested_paths(request, holder_command),
            workspace,
            &self.boundaries(&request.project_slug, &request.name)?,
        )
    }

    /// The paths a sandbox is given, before they are opened and pinned.
    fn requested_paths(request: &CreateRequest, holder_command: &holder::Command) -> Vec<Requested> {
        let mut requested: Vec<Requested> = Vec::new();
        // The mechanism's own command: the pid-1 shell and the utility that
        // blocks in it. Everything else the sandbox can run is contributed.
        for path in &holder_command.binds {
            requested.push(Requested {
                path: path.clone(),
                contributed: false,
            });
        }
        requested.extend(
            Self::contributed_mounts(request)
                .into_iter()
                .map(|path| Requested {
                    path,
                    contributed: true,
                }),
        );
        requested
    }

    /// The payload for a sandbox that has started.
    #[allow(clippy::too_many_arguments)]
    fn payload(
        &self,
        holder: &holder::Holder,
        view: &View,
        workspace: &Path,
        environment: &crate::sandbox::environment::Environment,
    ) -> Result<BubblewrapData, Error> {
        let launcher = self
            .host
            .process(holder.launcher_pid)
            .map(|process| (holder.launcher_pid, process.start_time))
            .unwrap_or((holder.launcher_pid, 0));
        let sandbox = self
            .host
            .process(holder.sandbox_pid)
            .map(|process| (holder.sandbox_pid, process.start_time))
            .unwrap_or((holder.sandbox_pid, 0));
        let root = self
            .host
            .root_identity(holder.sandbox_pid)
            .ok_or_else(|| Error::failed("the sandbox root could not be read"))?;
        // The namespaces are read once the view is built: bwrap reports its
        // status before setup finishes, and the inodes it holds then are not
        // necessarily the ones it ends up with.
        let namespaces = self
            .host
            .namespaces(holder.sandbox_pid)
            .ok_or_else(|| Error::failed("the sandbox namespaces could not be read"))?;
        Ok(BubblewrapData {
            version: super::payload::VERSION,
            launcher_pid: launcher.0,
            launcher_start: launcher.1,
            sandbox_pid: sandbox.0,
            sandbox_start: sandbox.1,
            namespaces: namespaces.entries().map(|(_, inode)| inode),
            uid: self
                .host
                .process(holder.sandbox_pid)
                .map(|process| process.uid)
                .unwrap_or_default(),
            root: RecordedRoot {
                device: root.device,
                inode: root.inode,
            },
            mounts: view
                .pinned
                .iter()
                .map(|pinned| RecordedMount::from(&pinned.mount))
                .collect(),
            workspace_host: workspace.to_string_lossy().to_string(),
            workspace: WORKSPACE.to_string(),
            scope: holder.unit.clone(),
            paused: false,
            environment: environment.provenance.clone(),
        })
    }

    fn cleanup(&self, project: &str, name: &str, holder: Option<&holder::Holder>) {
        if let Some(holder) = holder
            && let Err(error) =
                holder::teardown(&*self.host, holder, std::time::Duration::from_secs(10))
        {
            tracing::warn!(%error, "the sandbox did not stop after a failed create");
        }
        let directory = workspace::sandbox_directory(project, name);
        if let Err(error) = std::fs::remove_dir_all(&directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "the state directory could not be removed");
        }
    }
}

impl Backend for BubblewrapBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move { self.create_sandbox(request).await })
    }

    fn list<'a>(&'a self, _scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn exec<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(async move {
            let data = match validate::validate(sandbox, &*self.host) {
                validate::Verdict::Live(data) => *data,
                validate::Verdict::Absent => {
                    return Err(Error::NotFound {
                        name: sandbox.name.clone(),
                    });
                }
                validate::Verdict::Forbidden { reason } => {
                    return Err(Error::Denied {
                        operation: format!("run a command in '{}'", sandbox.name),
                        reason,
                    });
                }
                validate::Verdict::Editions(error) => return Err(error),
            };
            let host = self.host.clone();
            let command = command.to_vec();
            tokio::task::spawn_blocking(move || join::run_serialised(&*host, &data, &command))
                .await
                .map_err(|error| Error::failed(format!("the join task failed: {error}")))?
        })
    }

    fn upload<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a Path,
        _destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { Err(Error::failed("upload is not implemented yet")) })
    }

    fn download<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a str,
        _destination: &'a Path,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { Err(Error::failed("download is not implemented yet")) })
    }

    fn inspect<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move { Err(Error::failed("inspection is not implemented yet")) })
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
        vec![
            ContributionSupport {
                sort: ContributionSort::Environment,
                state: State::Available,
                reason: None,
                remedy: None,
                fallback: None,
            },
            ContributionSupport {
                sort: ContributionSort::Mount,
                state: State::Available,
                reason: None,
                remedy: None,
                fallback: None,
            },
        ]
    }
}

impl BubblewrapBackend {
    async fn create_sandbox(&self, request: &CreateRequest) -> Result<SandboxRecord, Error> {
        if !request.ports.is_empty() {
            return Err(Error::Unsupported {
                operation: format!("create the sandbox '{}'", request.name),
                backend: BACKEND.to_string(),
                primitive: "a host port namespace".to_string(),
                reason: "the bubblewrap sandbox has its own network namespace, so nothing can \
                         reach a listener inside it"
                    .to_string(),
                remedy: "remove the project's ports or use a backend that forwards them"
                    .to_string(),
            });
        }

        let directory = workspace::sandbox_directory(&request.project_slug, &request.name);
        if directory.exists() {
            return Err(Error::Conflict {
                name: request.name.clone(),
            });
        }

        let bwrap = self
            .host
            .resolve("bwrap")
            .ok_or_else(|| Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: "bubblewrap does not resolve on the host".to_string(),
                remedy: "install bubblewrap or add it to PATH".to_string(),
            })?;
        let command = holder::command(&*self.host)?;
        let environment = environment::merge(&request.provision)?;
        let workspace = workspace::workspace_directory(&request.project_slug, &request.name);
        std::fs::create_dir_all(&workspace).map_err(|error| {
            Error::failed(format!("the state directory could not be created: {error}"))
        })?;

        let mut started: Option<holder::Holder> = None;
        let outcome = self
            .create_holder(
                request,
                &bwrap,
                &command,
                &environment,
                &workspace,
                &mut started,
            )
            .await;
        match outcome {
            Ok(record) => Ok(record),
            Err(error) => {
                self.cleanup(&request.project_slug, &request.name, started.as_ref());
                Err(error)
            }
        }
    }

    async fn create_holder(
        &self,
        request: &CreateRequest,
        bwrap: &Path,
        holder_command: &holder::Command,
        environment: &crate::sandbox::environment::Environment,
        workspace: &Path,
        started: &mut Option<holder::Holder>,
    ) -> Result<SandboxRecord, Error> {
        workspace::copy_from(&request.workspace.source, workspace)?;
        let view = self.build_view(request, holder_command, workspace)?;
        let holder_arguments = holder_command.arguments();
        let scope = if self.scoped() {
            let launcher = self
                .host
                .resolve("systemd-run")
                .ok_or_else(|| Error::Unavailable {
                    backend: BACKEND.to_string(),
                    reason: "the scope launcher does not resolve on the host".to_string(),
                    remedy: "install systemd or accept the degraded mode".to_string(),
                })?;
            Some((
                launcher,
                format!(
                    "litterbox-{}-{}-{}",
                    request.project_slug,
                    request.name,
                    std::process::id()
                ),
            ))
        } else {
            None
        };
        let argv = view.render(&RenderOptions {
            bwrap,
            holder: &holder_command.shell,
            holder_arguments: &holder_arguments,
            workspace_host: workspace,
            status_fd: STATUS_FD,
        });
        let status_path = workspace
            .parent()
            .map(|parent| parent.join("status"))
            .unwrap_or_else(|| PathBuf::from("status"));
        let stderr_path = workspace
            .parent()
            .map(|parent| parent.join("bwrap-stderr"))
            .unwrap_or_else(|| PathBuf::from("bwrap-stderr"));

        let spawn_argv = argv.clone();
        let spawn_environment = environment.clone();
        let spawn_scope = scope.clone();
        let spawn_host = self.host.clone();
        let spawn_status = status_path.clone();
        let spawn_stderr = stderr_path.clone();
        let holder = tokio::task::spawn_blocking(move || {
            holder::start(&holder::Start {
                argv: &spawn_argv,
                host: &*spawn_host,
                environment: &spawn_environment,
                scope: spawn_scope
                    .as_ref()
                    .map(|(launcher, unit)| (launcher.as_path(), unit.as_str())),
                status_path: &spawn_status,
                stderr_path: &spawn_stderr,
            })
        })
        .await
        .map_err(|error| Error::failed(format!("the holder task failed: {error}")))??;
        *started = Some(holder.clone());

        holder::await_ready(
            &*self.host,
            holder.sandbox_pid,
            &view.mounts(),
            holder::READY_TIMEOUT,
        )
        .await?;

        let data = self.payload(&holder, &view, workspace, environment)?;
        let last_step = Mutex::new(None);
        let host = self.host.clone();
        let sandbox_name = request.name.clone();
        let payload_for_steps = data.clone();
        let provision = run_steps(
            BACKEND,
            &sandbox_name,
            &request.provision,
            &last_step,
            move |_identity, run| {
                let host = host.clone();
                let data = payload_for_steps.clone();
                let command = vec!["sh".to_string(), "-c".to_string(), run.to_string()];
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || {
                        join::run_serialised(&*host, &data, &command)
                    })
                    .await
                    .map_err(|error| Error::failed(format!("the step task failed: {error}")))?
                })
            },
            |_step, error| error,
        );
        let steps = tokio::time::timeout(crate::backends::shared::CREATE_BUDGET, provision)
            .await
            .map_err(|_| Error::Provisioning {
                failure: Box::new(ProvisioningFailure::new(
                    ProvisioningClass::Failed,
                    Some(BACKEND.to_string()),
                    None,
                    None,
                    format!(
                        "create exceeded the {}s provisioning budget",
                        crate::backends::shared::CREATE_BUDGET.as_secs()
                    ),
                    "reduce the work the provisioning steps perform",
                )),
            })??;

        let mut data = data;
        data.environment.steps = steps;
        Ok(SandboxRecord {
            name: request.name.clone(),
            backend: BACKEND.to_string(),
            project_slug: request.project_slug.clone(),
            data: data.into_value()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{BubblewrapBackend, BubblewrapSettings};
    use crate::backends::bubblewrap::host::FakeHost;
    use crate::sandbox::contract::{
        Backend, ContributionSort, CreateRequest, Error, RequestedPort, SandboxAction, Workspace,
    };
    use crate::sandbox::contract::{Placement, ResolvedStep};

    fn request(ports: Vec<RequestedPort>) -> CreateRequest {
        CreateRequest {
            name: "demo".to_string(),
            project_slug: "project".to_string(),
            workspace: Workspace {
                source: std::path::PathBuf::from("/staged"),
            },
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports,
        }
    }

    fn backend() -> BubblewrapBackend {
        BubblewrapBackend::new(BubblewrapSettings::default(), Arc::new(FakeHost::default()))
    }

    #[test]
    fn both_contribution_sorts_are_available() {
        let support = backend().contribution_support();
        assert_eq!(support.len(), 2);
        assert!(
            support
                .iter()
                .any(|entry| entry.sort == ContributionSort::Environment)
        );
        assert!(
            support
                .iter()
                .any(|entry| entry.sort == ContributionSort::Mount)
        );
    }

    fn holder_command() -> crate::backends::bubblewrap::holder::Command {
        crate::backends::bubblewrap::holder::Command {
            shell: std::path::PathBuf::from("/usr/bin/sh"),
            binds: vec![std::path::PathBuf::from("/usr/bin/sh")],
            path: "/usr/bin".to_string(),
        }
    }

    #[test]
    fn a_sandbox_with_no_steps_requests_only_the_holder_command() {
        let request = request(Vec::new());
        let command = holder_command();
        let requested: Vec<std::path::PathBuf> =
            BubblewrapBackend::requested_paths(&request, &command)
                .into_iter()
                .map(|entry| entry.path)
                .collect();
        assert_eq!(
            requested,
            vec![std::path::PathBuf::from("/usr/bin/sh")],
            "the view holds the holder command and nothing else"
        );
    }

    #[test]
    fn a_contributed_mount_is_requested_as_contributed() {
        use crate::sandbox::contract::Contributions;

        let mut request = request(Vec::new());
        request.provision.push(ResolvedStep {
            identity: "dev".to_string(),
            kind: "devshell".to_string(),
            placement: Placement::Host,
            sandbox: None,
            contributions: Contributions {
                mounts: vec![std::path::PathBuf::from("/opt/toolchain/abc")],
                ..Contributions::default()
            },
        });
        let command = crate::backends::bubblewrap::holder::Command {
            shell: std::path::PathBuf::from("/usr/bin/sh"),
            binds: Vec::new(),
            path: String::new(),
        };
        let requested = BubblewrapBackend::requested_paths(&request, &command);
        assert_eq!(requested.len(), 1);
        assert_eq!(
            requested[0].path,
            std::path::PathBuf::from("/opt/toolchain/abc")
        );
        assert!(requested[0].contributed);
    }

    #[tokio::test]
    async fn a_declared_port_is_refused() {
        let backend = backend();
        let request = request(vec![RequestedPort {
            name: "www".to_string(),
            target: 8080,
        }]);
        match backend.create(&request).await {
            Err(Error::Unsupported { primitive, .. }) => {
                assert_eq!(primitive, "a host port namespace");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_missing_mechanism_is_unavailable() {
        let backend = backend();
        match backend.create(&request(Vec::new())).await {
            Err(Error::Unavailable { reason, .. }) => {
                assert!(reason.contains("bubblewrap"), "{reason}");
            }
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn a_command_step_carries_no_host_contribution() {
        let step = ResolvedStep {
            identity: "build".to_string(),
            kind: "command".to_string(),
            placement: Placement::Sandbox,
            sandbox: Some(SandboxAction {
                run: "make".to_string(),
            }),
            contributions: Default::default(),
        };
        assert!(step.contributions.mounts.is_empty());
    }

    /// The userland the host test runs in.
    ///
    /// A sandbox has no tools its project did not contribute, so the test
    /// declares what it needs the same way a devShell would: the paths, their
    /// loader dependencies and the `PATH` that finds them.
    fn userland() -> ResolvedStep {
        use crate::backends::bubblewrap::host::{Host, SystemHost};
        use crate::sandbox::contract::Contributions;
        use crate::sandbox::environment::{Change, Variable};

        let host = SystemHost::new();
        let mut mounts = Vec::new();
        let mut path = Vec::new();
        for name in ["sh", "cat", "ls", "readlink"] {
            let binary = host.resolve(name).expect("the host supplies the utility");
            let mode = host.ownership(&binary).map_or(0, |(_, mode)| mode);
            for path in crate::backends::bubblewrap::holder::runtime(&binary, mode) {
                if !mounts.contains(&path) {
                    mounts.push(path);
                }
            }
            if let Some(directory) = binary.parent() {
                let directory = directory.to_path_buf();
                if !mounts.contains(&directory) {
                    mounts.push(directory.clone());
                }
                let entry = directory.to_string_lossy().to_string();
                if !path.contains(&entry) {
                    path.push(entry);
                }
            }
            if !mounts.contains(&binary) {
                mounts.push(binary);
            }
        }
        let mut contributions = Contributions {
            mounts,
            ..Contributions::default()
        };
        contributions
            .environment
            .insert("PATH".to_string(), path.join(":"));
        contributions.provenance.variables.push(Variable {
            name: "PATH".to_string(),
            change: Change::Set,
        });
        ResolvedStep {
            identity: "userland".to_string(),
            kind: "command".to_string(),
            placement: Placement::Host,
            sandbox: None,
            contributions,
        }
    }

    fn test_request(name: &str, staged: &std::path::Path) -> CreateRequest {
        CreateRequest {
            name: name.to_string(),
            project_slug: "probe".to_string(),
            workspace: Workspace {
                source: staged.to_path_buf(),
            },
            provision: vec![userland()],
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
        }
    }

    #[tokio::test]
    #[ignore = "starts a real sandbox; run where bubblewrap is on PATH"]
    async fn a_real_sandbox_runs_a_command() {
        use crate::backends::bubblewrap::host::SystemHost;
        use crate::backends::bubblewrap::{holder, payload};

        let staged = tempfile::tempdir().expect("staged");
        std::fs::write(staged.path().join("README.md"), b"hello from the sandbox").expect("file");
        let backend =
            BubblewrapBackend::new(BubblewrapSettings::default(), Arc::new(SystemHost::new()));
        let request = test_request("demo", staged.path());
        let record = backend.create(&request).await.expect("created");
        let data = payload::BubblewrapData::from_record(&record).expect("payload");
        let holder = holder::Holder {
            launcher_pid: data.launcher_pid,
            sandbox_pid: data.sandbox_pid,
            namespaces: crate::backends::bubblewrap::host::NamespaceInodes {
                user: data.namespaces[0],
                ipc: data.namespaces[1],
                net: data.namespaces[2],
                pid: data.namespaces[3],
                mnt: data.namespaces[4],
                uts: data.namespaces[5],
                cgroup: data.namespaces[6],
            },
            unit: data.scope.clone(),
        };
        let outcome = backend
            .exec(
                &record,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat README.md; readlink /proc/self/ns/pid".to_string(),
                ],
            )
            .await;
        let _ = holder::teardown(
            &SystemHost::new(),
            &holder,
            std::time::Duration::from_secs(10),
        );
        let execution = outcome.expect("executed");
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        assert!(execution.stdout.contains("hello from the sandbox"));
        assert!(
            execution
                .stdout
                .contains(&format!("pid:[{}]", data.namespaces[3]))
        );
    }
}
