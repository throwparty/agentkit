//! Bubblewrap's implementation of the contract.

use std::path::{Path, PathBuf};

use std::sync::Mutex;

use futures_util::future::BoxFuture;

use super::BACKEND;
use super::host::SharedHost;
use super::payload::{BubblewrapData, RecordedMount, RecordedRoot};
use super::view::{Boundaries, RenderOptions, Requested, View, WORKSPACE};
use super::{environment, holder, join, lifecycle, readiness, validate, workspace};
use crate::backends::shared::{EXEC_OUTPUT_CAP, PROVISION_OUTPUT_CAP, run_steps};
use crate::sandbox::contract::{
    Backend, ContributionSort, ContributionSupport, CreateRequest, Error, Execution, Forward,
    Inspection, ProvisioningClass, ProvisioningFailure, SandboxRecord, Scope,
    forwarded_port_env_var,
};
use crate::sandbox::facets::{Lifecycle, Limits, ResourceLimits};
use crate::sandbox::report::{Capability, EgressPosture, Readiness, State};

/// The descriptor bwrap reports its status on.
const STATUS_FD: i32 = 3;

/// Whether the sandbox gets its own network namespace.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Network {
    /// The sandbox has its own network namespace, so it reaches nothing beyond
    /// its own loopback.
    #[default]
    Denied,
    /// The sandbox shares the host's network namespace, so it reaches whatever
    /// the host reaches and a listener it starts is reachable from the host.
    Host,
}

impl Network {}

/// The egress posture an inspection reports for a network setting.
fn egress(network: Network) -> EgressPosture {
    match network {
        Network::Host => EgressPosture::Direct,
        Network::Denied => EgressPosture::Denied,
    }
}

/// Bubblewrap's settings, typed here and nowhere else.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BubblewrapSettings {
    /// Memory ceiling each sandbox receives, in MiB.
    pub memory_mb: Option<u64>,
    /// Maximum tasks or processes each sandbox may create.
    pub tasks: Option<u32>,
    /// Whether the sandbox gets its own network namespace.
    pub network: Network,
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
        // The same probe readiness reports, so a host without a user manager
        // is launched unscoped instead of failing at create.
        self.host.user_scope()
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
        let requested = Self::requested_paths(request, holder_command, self.settings.network);
        View::build(
            &requested,
            workspace,
            &self.boundaries(&request.project_slug, &request.name)?,
        )
    }

    /// The paths a sandbox is given, before they are opened and pinned.
    fn requested_paths(
        request: &CreateRequest,
        holder_command: &holder::Command,
        network: Network,
    ) -> Vec<Requested> {
        let mut requested: Vec<Requested> = Vec::new();
        // The mechanism's own command: the pid-1 shell and the utility that
        // blocks in it. Everything else the sandbox can run is contributed.
        for path in &holder_command.binds {
            requested.push(Requested {
                path: path.clone(),
                contributed: false,
            });
        }
        // A sandbox that shares the host's network still needs the host's name
        // resolution and certificate roots to use it.
        if network == Network::Host {
            for path in [
                "/etc/resolv.conf",
                "/etc/hosts",
                "/etc/nsswitch.conf",
                "/etc/ssl",
            ] {
                let path = PathBuf::from(path);
                if path.exists() {
                    requested.push(Requested {
                        path,
                        contributed: false,
                    });
                }
            }
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

    /// Apply the configured bounds to the scope the sandbox runs in.
    fn apply_limits(&self, holder: &holder::Holder) -> Result<(), Error> {
        let memory = self.settings.memory_mb;
        let tasks = self.settings.tasks;
        if memory.is_none() && tasks.is_none() {
            return Ok(());
        }
        let Some(unit) = &holder.unit else {
            return Err(Error::Unsupported {
                operation: "bound the sandbox".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a cgroup v2 user scope".to_string(),
                reason: "the sandbox was created without a scope, so the configured bounds \
                         cannot be applied"
                    .to_string(),
                remedy: "remove the bounds, or run on a host with a delegating user manager"
                    .to_string(),
            });
        };
        if let Some(memory_mb) = memory {
            self.host
                .write_scope(unit, "memory.max", &(memory_mb * 1024 * 1024).to_string())
                .map_err(|error| {
                    Error::failed(format!("the memory bound was not applied: {error}"))
                })?;
        }
        if let Some(tasks) = tasks {
            self.host
                .write_scope(unit, "pids.max", &tasks.to_string())
                .map_err(|error| {
                    Error::failed(format!("the task bound was not applied: {error}"))
                })?;
        }
        Ok(())
    }

    /// The payload for a sandbox that has started.
    #[allow(clippy::too_many_arguments)]
    fn payload(
        &self,
        holder: &holder::Holder,
        view: &View,
        workspace: &Path,
        environment: &crate::sandbox::environment::Environment,
        forwards: &[crate::sandbox::contract::Forward],
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
            network: self.settings.network,
            forwards: forwards.to_vec(),
        })
    }

    /// The payload of a live sandbox, or the failure its operation reports.
    fn live(&self, sandbox: &SandboxRecord) -> Result<BubblewrapData, Error> {
        match validate::validate(sandbox, &*self.host) {
            validate::Verdict::Live(data) => Ok(*data),
            validate::Verdict::Absent => Err(Error::NotFound {
                name: sandbox.name.clone(),
            }),
            validate::Verdict::Forbidden { reason } => Err(Error::Denied {
                operation: format!("use '{}'", sandbox.name),
                reason,
            }),
            validate::Verdict::Editions(error) => Err(error),
        }
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

    fn list<'a>(&'a self, scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async move {
            let base = agentkit_common::data_dir("litterbox").join("bubblewrap");
            let projects: Vec<PathBuf> = match scope {
                Scope::Repository { project_slug } => vec![base.join(project_slug)],
                Scope::Machine => std::fs::read_dir(&base)
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.path())
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            let mut records = Vec::new();
            for project in projects {
                let Ok(entries) = std::fs::read_dir(&project) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let payload_path = entry.path().join("payload.toml");
                    let Ok(text) = std::fs::read_to_string(&payload_path) else {
                        tracing::warn!(
                            path = %payload_path.display(),
                            "a sandbox state directory has no readable payload"
                        );
                        continue;
                    };
                    let data: BubblewrapData = match toml::from_str(&text) {
                        Ok(data) => data,
                        Err(error) => {
                            tracing::warn!(
                                path = %payload_path.display(),
                                %error,
                                "a sandbox payload is corrupt"
                            );
                            continue;
                        }
                    };
                    let name = entry.file_name().to_string_lossy().to_string();
                    let project_slug = project
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_default();
                    records.push(SandboxRecord {
                        name,
                        backend: BACKEND.to_string(),
                        project_slug,
                        data: toml::Value::try_from(data)
                            .unwrap_or(toml::Value::Table(Default::default())),
                    });
                }
            }
            Ok(records)
        })
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
            tokio::task::spawn_blocking(move || {
                join::run(&*host, &data, &command, EXEC_OUTPUT_CAP)
            })
                .await
                .map_err(|error| Error::failed(format!("the join task failed: {error}")))?
        })
    }

    fn upload<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a Path,
        destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = self.live(sandbox)?;
            workspace::transfer_in(Path::new(&data.workspace_host), source, destination)
        })
    }

    fn download<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a str,
        destination: &'a Path,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = self.live(sandbox)?;
            workspace::transfer_out(Path::new(&data.workspace_host), source, destination)
        })
    }

    fn inspect<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move {
            match validate::validate(sandbox, &*self.host) {
                validate::Verdict::Live(data) => Ok(Inspection {
                    name: sandbox.name.clone(),
                    backend: BACKEND.to_string(),
                    status: if lifecycle::is_paused(&*self.host, &data) {
                        crate::sandbox::contract::SandboxStatus::Paused
                    } else {
                        crate::sandbox::contract::SandboxStatus::Active
                    },
                    egress: egress(data.network),
                    workspace: data.workspace.clone(),
                    environment: data.environment.clone(),
                    forwards: data.forwards.clone(),
                }),
                validate::Verdict::Absent => {
                    // The process is gone but the payload is still the record
                    // of what the sandbox was, including its network posture.
                    let data = BubblewrapData::from_record(sandbox)?;
                    Ok(Inspection {
                        name: sandbox.name.clone(),
                        backend: BACKEND.to_string(),
                        status: crate::sandbox::contract::SandboxStatus::Missing,
                        egress: egress(data.network),
                        workspace: data.workspace,
                        environment: data.environment,
                        forwards: data.forwards,
                    })
                }
                validate::Verdict::Forbidden { reason } => Err(Error::Denied {
                    operation: format!("inspect '{}'", sandbox.name),
                    reason,
                }),
                validate::Verdict::Editions(error) => Err(error),
            }
        })
    }

    fn delete<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            // A record that cannot be read is still removable, so an edition
            // failure only means the process is not signalled. A record whose
            // live process does not match it is cleaned up without a signal:
            // the pid now belongs to something else.
            if let validate::Verdict::Live(data) = validate::validate(sandbox, &*self.host) {
                let holder = holder::Holder {
                    launcher_pid: data.launcher_pid,
                    sandbox_pid: data.sandbox_pid,
                    sandbox_start: Some(data.sandbox_start),
                    namespaces: super::host::NamespaceInodes {
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
                holder::teardown(&*self.host, &holder, std::time::Duration::from_secs(30))?;
            }
            let directory = workspace::sandbox_directory(&sandbox.project_slug, &sandbox.name);
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Error::failed(format!(
                    "{} could not be removed: {error}",
                    directory.display()
                ))),
            }
        })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async {
            Ok(Readiness {
                backend: BACKEND.to_string(),
                checks: readiness::checks(&*self.host),
            })
        })
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<Vec<Capability>, Error>> {
        Box::pin(async {
            let mut capabilities = Vec::new();
            let readiness = Readiness {
                backend: BACKEND.to_string(),
                checks: readiness::checks(&*self.host),
            };
            if let Some(cgroup) = readiness
                .checks
                .iter()
                .find(|check| check.primitive == "a cgroup v2 user scope")
                && cgroup.state != State::Available
            {
                for operation in ["pause and resume", "resource limits"] {
                    capabilities.push(Capability {
                        operation: operation.to_string(),
                        state: cgroup.state,
                        reason: cgroup.reason.clone(),
                        remedy: cgroup.remedy.clone(),
                        fallback: cgroup.fallback.clone(),
                    });
                }
            }
            capabilities.push(Capability {
                operation: "shared host network".to_string(),
                state: match self.settings.network {
                    Network::Host => State::Available,
                    Network::Denied => State::Unavailable,
                },
                reason: Some(match self.settings.network {
                    Network::Host => {
                        "the sandbox shares the host's network namespace, so it reaches whatever \
                         the host reaches"
                            .to_string()
                    }
                    Network::Denied => {
                        "the sandbox has its own network namespace and reaches only its own \
                         loopback"
                            .to_string()
                    }
                }),
                remedy: Some(match self.settings.network {
                    Network::Host => {
                        "set network = \"denied\" in the user configuration to isolate egress"
                            .to_string()
                    }
                    Network::Denied => {
                        "set network = \"host\" in the user configuration to allow egress"
                            .to_string()
                    }
                }),
                fallback: None,
            });
            Ok(capabilities)
        })
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

impl Lifecycle for BubblewrapBackend {
    fn pause<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = self.live(sandbox)?;
            lifecycle::pause(&*self.host, &data).await
        })
    }

    fn resume<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = self.live(sandbox)?;
            lifecycle::resume(&*self.host, &data)
        })
    }
}

impl Limits for BubblewrapBackend {
    fn set_limits(
        &self,
        sandbox: &SandboxRecord,
        limits: &ResourceLimits,
    ) -> BoxFuture<'_, Result<(), Error>> {
        let data = match self.live(sandbox) {
            Ok(data) => data,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let limits = *limits;
        Box::pin(async move { lifecycle::set_limits(&*self.host, &data, &limits) })
    }
}

impl BubblewrapBackend {
    /// The forwards a request needs under a network setting.
    ///
    /// A shared network means the sandbox listens on the host port directly,
    /// so the host port and the target are the same. A port whose name cannot
    /// become an environment variable is an error, never a dropped entry.
    fn forwards(request: &CreateRequest, network: Network) -> Result<Vec<Forward>, Error> {
        if request.ports.is_empty() {
            return Ok(Vec::new());
        }
        if network != Network::Host {
            return Err(Error::Unsupported {
                operation: format!("create the sandbox '{}'", request.name),
                backend: BACKEND.to_string(),
                primitive: "a shared host network".to_string(),
                reason: "the sandbox has its own network namespace, so nothing can reach a \
                         listener inside it; set backends.bubblewrap.network = \"host\" to share \
                         the host's"
                    .to_string(),
                remedy: "set network = \"host\" in the user configuration or remove the ports"
                    .to_string(),
            });
        }

        request
            .ports
            .iter()
            .map(|port| {
                let env_var = forwarded_port_env_var(&port.name)
                    .map_err(|message| Error::Misconfigured { message })?;
                Ok(Forward {
                    name: port.name.clone(),
                    target: port.target,
                    host_port: port.target,
                    bind: request.forward_bind.clone(),
                    env_var,
                })
            })
            .collect()
    }

    async fn create_sandbox(&self, request: &CreateRequest) -> Result<SandboxRecord, Error> {
        let forwards = Self::forwards(request, self.settings.network)?;

        let directory = workspace::sandbox_directory(&request.project_slug, &request.name);
        if directory.exists() {
            return Err(Error::Conflict {
                name: request.name.clone(),
            });
        }

        // Every check has to pass except the cgroup one, which the registry
        // gates behind the user's acceptance of the degraded mode.
        for check in readiness::checks(&*self.host) {
            if check.state == State::Available || check.primitive == "a cgroup v2 user scope" {
                continue;
            }
            return Err(Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: format!(
                    "{} is {:?}: {}",
                    check.primitive,
                    check.state,
                    check.reason.unwrap_or_else(|| "not available".to_string())
                ),
                remedy: check
                    .remedy
                    .unwrap_or_else(|| "run `litterbox backends`".to_string()),
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
        let mut environment = environment::merge(&request.provision)?;
        // A shared network means the sandbox listens on the host, so the
        // project's ports are reachable without a forwarder. Report them the
        // way every other backend does, and tell the workload where they are.
        for forward in &forwards {
            environment
                .variables
                .insert(forward.env_var.clone(), forward.target.to_string());
            environment
                .provenance
                .variables
                .push(crate::sandbox::environment::Variable {
                    name: forward.env_var.clone(),
                    change: crate::sandbox::environment::Change::Set,
                });
        }
        // A shared network needs a trust store, and OpenSSL's compiled default
        // does not point into the closure. Point it at the host's bundle,
        // which the view binds.
        if self.settings.network == Network::Host {
            for candidate in [
                "/etc/ssl/certs/ca-bundle.crt",
                "/etc/ssl/certs/ca-certificates.crt",
            ] {
                if let Ok(path) = std::fs::canonicalize(candidate) {
                    environment.variables.insert(
                        "SSL_CERT_FILE".to_string(),
                        path.to_string_lossy().to_string(),
                    );
                    environment
                        .provenance
                        .variables
                        .push(crate::sandbox::environment::Variable {
                            name: "SSL_CERT_FILE".to_string(),
                            change: crate::sandbox::environment::Change::Set,
                        });
                    break;
                }
            }
        }
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
                &forwards,
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

    #[allow(clippy::too_many_arguments)]
    async fn create_holder(
        &self,
        request: &CreateRequest,
        bwrap: &Path,
        holder_command: &holder::Command,
        environment: &crate::sandbox::environment::Environment,
        workspace: &Path,
        forwards: &[crate::sandbox::contract::Forward],
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
            network: self.settings.network,
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
        let spawn_inherit = view.descriptors();
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
                inherit: &spawn_inherit,
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

        self.apply_limits(&holder)?;

        let data = self.payload(&holder, &view, workspace, environment, forwards)?;
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
                        join::run(&*host, &data, &command, PROVISION_OUTPUT_CAP)
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
        // The orchestrator keeps the record; the backend keeps a copy beside
        // the workspace so enumeration can rebuild it without one.
        let payload_path = workspace
            .parent()
            .map(|parent| parent.join("payload.toml"))
            .ok_or_else(|| Error::failed("the state directory has no parent"))?;
        let text = toml::to_string_pretty(&data)
            .map_err(|error| Error::failed(format!("the payload could not be written: {error}")))?;
        std::fs::write(&payload_path, text).map_err(|error| {
            Error::failed(format!(
                "{} could not be written: {error}",
                payload_path.display()
            ))
        })?;
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{BubblewrapBackend, BubblewrapSettings, Network};
    use crate::backends::bubblewrap::host::{
        FakeHost, MountEntry, NamespaceInodes, ProcessInfo, ProcessState, RootIdentity,
    };
    use crate::backends::bubblewrap::payload::{
        BubblewrapData, RecordedMount, RecordedRoot, VERSION,
    };
    use crate::sandbox::contract::{
        Backend, ContributionSort, CreateRequest, Error, RequestedPort, SandboxAction,
        SandboxRecord, SandboxStatus, Workspace,
    };
    use crate::sandbox::contract::{Placement, ResolvedStep};
    use crate::sandbox::environment::{Origin, Provenance};
    use crate::sandbox::report::EgressPosture;

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

    fn payload(network: Network) -> BubblewrapData {
        BubblewrapData {
            version: VERSION,
            launcher_pid: 10,
            launcher_start: 100,
            sandbox_pid: 11,
            sandbox_start: 101,
            namespaces: [1, 2, 3, 4, 5, 6, 7],
            uid: 1000,
            root: RecordedRoot {
                device: 8,
                inode: 9,
            },
            mounts: vec![RecordedMount {
                path: "/nix/store/abc".to_string(),
                read_only: true,
            }],
            workspace_host: "/state/workspace".to_string(),
            workspace: "/src".to_string(),
            scope: None,
            paused: false,
            environment: Provenance {
                origin: Origin::Default,
                variables: Vec::new(),
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
            network,
            forwards: Vec::new(),
        }
    }

    fn record(data: BubblewrapData) -> SandboxRecord {
        SandboxRecord {
            name: "demo".to_string(),
            backend: "bubblewrap".to_string(),
            project_slug: "project".to_string(),
            data: data.into_value().expect("payload"),
        }
    }

    fn live_host(data: &BubblewrapData) -> FakeHost {
        let mut host = FakeHost::default();
        host.processes.insert(
            data.sandbox_pid,
            ProcessInfo {
                state: ProcessState::Waiting,
                start_time: data.sandbox_start,
                uid: data.uid,
            },
        );
        host.namespaces.insert(
            data.sandbox_pid,
            NamespaceInodes {
                user: data.namespaces[0],
                ipc: data.namespaces[1],
                net: data.namespaces[2],
                pid: data.namespaces[3],
                mnt: data.namespaces[4],
                uts: data.namespaces[5],
                cgroup: data.namespaces[6],
            },
        );
        host.roots.insert(
            data.sandbox_pid,
            RootIdentity {
                device: data.root.device,
                inode: data.root.inode,
            },
        );
        let mut mounts: Vec<MountEntry> = data
            .mounts
            .iter()
            .map(|mount| MountEntry {
                path: PathBuf::from(&mount.path),
                read_only: mount.read_only,
                fstype: "ext4".to_string(),
            })
            .collect();
        mounts.push(MountEntry {
            path: PathBuf::from("/src"),
            read_only: false,
            fstype: "ext4".to_string(),
        });
        host.mounts.insert(data.sandbox_pid, mounts);
        host
    }

    #[tokio::test]
    async fn inspection_reports_the_recorded_egress_when_live() {
        for (network, expected) in [
            (Network::Host, EgressPosture::Direct),
            (Network::Denied, EgressPosture::Denied),
        ] {
            let data = payload(network);
            let backend =
                BubblewrapBackend::new(BubblewrapSettings::default(), Arc::new(live_host(&data)));
            let inspection = backend
                .inspect(&record(data))
                .await
                .expect("inspection succeeds");
            assert_eq!(inspection.status, SandboxStatus::Active);
            assert_eq!(inspection.egress, expected);
        }
    }

    #[tokio::test]
    async fn inspection_reports_the_recorded_egress_when_absent() {
        for (network, expected) in [
            (Network::Host, EgressPosture::Direct),
            (Network::Denied, EgressPosture::Denied),
        ] {
            let backend = backend();
            let inspection = backend
                .inspect(&record(payload(network)))
                .await
                .expect("inspection succeeds");
            assert_eq!(inspection.status, SandboxStatus::Missing);
            assert_eq!(inspection.egress, expected);
        }
    }

    #[tokio::test]
    async fn inspection_refuses_an_unreadable_payload() {
        let mut data = payload(Network::Denied);
        data.version = VERSION - 1;
        let error = backend()
            .inspect(&record(data))
            .await
            .expect_err("inspection refuses");
        assert!(matches!(error, Error::Unavailable { .. }), "{error:?}");
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
            BubblewrapBackend::requested_paths(&request, &command, Network::Denied)
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
                mounts: vec![PathBuf::from("/opt/toolchain/abc")],
                ..Contributions::default()
            },
        });
        let command = crate::backends::bubblewrap::holder::Command {
            shell: std::path::PathBuf::from("/usr/bin/sh"),
            binds: Vec::new(),
            path: String::new(),
        };
        let requested = BubblewrapBackend::requested_paths(&request, &command, Network::Denied);
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].path, PathBuf::from("/opt/toolchain/abc"));
        assert!(requested[0].contributed);
    }

    #[test]
    fn a_host_sandbox_requests_the_resolver_and_certificate_paths() {
        let request = request(Vec::new());
        let command = crate::backends::bubblewrap::holder::Command {
            shell: PathBuf::from("/usr/bin/sh"),
            binds: Vec::new(),
            path: String::new(),
        };
        let host_paths: Vec<PathBuf> =
            BubblewrapBackend::requested_paths(&request, &command, Network::Host)
                .into_iter()
                .map(|requested| requested.path)
                .collect();
        for path in [
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/nsswitch.conf",
            "/etc/ssl",
        ] {
            if Path::new(path).exists() {
                assert!(
                    host_paths.contains(&PathBuf::from(path)),
                    "the host view is missing {path}"
                );
            }
        }

        let denied_paths: Vec<PathBuf> =
            BubblewrapBackend::requested_paths(&request, &command, Network::Denied)
                .into_iter()
                .map(|requested| requested.path)
                .collect();
        for path in [
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/nsswitch.conf",
            "/etc/ssl",
        ] {
            assert!(
                !denied_paths.contains(&PathBuf::from(path)),
                "the denied view requested {path}"
            );
        }
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
                assert_eq!(primitive, "a shared host network");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn a_host_port_becomes_a_forward_with_the_same_host_port() {
        let request = request(vec![RequestedPort {
            name: "www".to_string(),
            target: 8080,
        }]);
        let forwards = BubblewrapBackend::forwards(&request, Network::Host).expect("forwards");
        assert_eq!(forwards.len(), 1);
        assert_eq!(forwards[0].name, "www");
        assert_eq!(forwards[0].target, 8080);
        assert_eq!(forwards[0].host_port, 8080);
        assert_eq!(forwards[0].bind, "127.0.0.1");
        assert_eq!(forwards[0].env_var, "LITTERBOX_FWD_PORT_WWW");
    }

    #[test]
    fn a_denied_port_names_the_setting_that_would_allow_it() {
        let request = request(vec![RequestedPort {
            name: "www".to_string(),
            target: 8080,
        }]);
        match BubblewrapBackend::forwards(&request, Network::Denied) {
            Err(Error::Unsupported { reason, remedy, .. }) => {
                assert!(reason.contains("network"), "{reason}");
                assert!(remedy.contains("host"), "{remedy}");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }

    #[test]
    fn a_port_name_that_cannot_be_an_environment_variable_is_refused() {
        let request = request(vec![RequestedPort {
            name: "----".to_string(),
            target: 8080,
        }]);
        match BubblewrapBackend::forwards(&request, Network::Host) {
            Err(Error::Misconfigured { message }) => {
                assert!(message.contains("not a valid port name"), "{message}");
            }
            other => panic!("expected misconfigured, got {other:?}"),
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
    async fn deleting_an_absent_sandbox_succeeds() {
        use crate::sandbox::contract::{SandboxRecord, Scope};

        let backend = backend();
        let record = SandboxRecord {
            name: "never-created-carrier-probe".to_string(),
            backend: "bubblewrap".to_string(),
            project_slug: "probe".to_string(),
            data: toml::Value::Table(Default::default()),
        };
        backend.delete(&record).await.expect("idempotent delete");
        let listed = backend
            .list(&Scope::Repository {
                project_slug: "probe".to_string(),
            })
            .await
            .expect("listed");
        assert!(
            !listed
                .iter()
                .any(|sandbox| sandbox.name == "never-created-carrier-probe"),
            "an absent sandbox is not listed"
        );
    }

    /// Create a sandbox, or skip the test when this host cannot run one.
    ///
    /// The host suite is ignored by default but CI runs ignored tests too,
    /// and a runner that blocks unprivileged user namespaces cannot create a
    /// sandbox at all. The ADR says to skip with the requirement named rather
    /// than fail.
    async fn create_or_skip(
        backend: &BubblewrapBackend,
        request: &CreateRequest,
    ) -> Option<SandboxRecord> {
        match backend.create(request).await {
            Ok(record) => Some(record),
            Err(Error::Unavailable { reason, remedy, .. }) => {
                eprintln!("skipping: {reason}; {remedy}");
                None
            }
            Err(error) => panic!("create failed: {error}"),
        }
    }

    #[tokio::test]
    #[ignore = "starts a real sandbox; run where bubblewrap is on PATH"]
    async fn a_real_sandbox_confines_writes() {
        use crate::backends::bubblewrap::host::SystemHost;
        use crate::backends::bubblewrap::{holder, payload};

        let staged = tempfile::tempdir().expect("staged");
        let backend =
            BubblewrapBackend::new(BubblewrapSettings::default(), Arc::new(SystemHost::new()));
        let request = test_request("confine", staged.path());
        let Some(record) = create_or_skip(&backend, &request).await else {
            return;
        };
        let data = payload::BubblewrapData::from_record(&record).expect("payload");
        let holder = holder::Holder {
            launcher_pid: data.launcher_pid,
            sandbox_pid: data.sandbox_pid,
            sandbox_start: Some(data.sandbox_start),
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
        let command = |program: &str| vec!["sh".to_string(), "-c".to_string(), program.to_string()];
        let write = backend
            .exec(&record, &command("echo ok > /src/probe.txt; echo written"))
            .await
            .expect("write");
        let state = crate::backends::bubblewrap::workspace::sandbox_directory("probe", "confine");
        let outside = backend
            .exec(&record, &command(&format!("echo x > {}", state.display())))
            .await
            .expect("write outside");
        let home = std::env::var("HOME").expect("HOME is set");
        let host_home = backend
            .exec(&record, &command(&format!("ls {home}")))
            .await
            .expect("home read");
        let read = backend
            .exec(&record, &command("cat /src/probe.txt"))
            .await
            .expect("read back");
        let _ = holder::teardown(
            &SystemHost::new(),
            &holder,
            std::time::Duration::from_secs(10),
        );
        let _ = std::fs::remove_dir_all(&state);
        assert_eq!(write.exit_code, 0, "{}", write.stderr);
        assert_eq!(read.exit_code, 0, "the workspace is writable");
        assert_eq!(read.stdout.trim(), "ok");
        assert_ne!(
            outside.exit_code, 0,
            "an ungranted host tree is in the view: {}",
            outside.stdout
        );
        assert_ne!(
            host_home.exit_code, 0,
            "the host home is not in the view: {}",
            host_home.stdout
        );
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
        let Some(record) = create_or_skip(&backend, &request).await else {
            return;
        };
        let data = payload::BubblewrapData::from_record(&record).expect("payload");
        let holder = holder::Holder {
            launcher_pid: data.launcher_pid,
            sandbox_pid: data.sandbox_pid,
            sandbox_start: Some(data.sandbox_start),
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
