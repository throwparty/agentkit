//! The Seatbelt backend.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;

use super::BACKEND;
use super::config::{NetworkSetting, Settings};
use super::defaults;
use super::holder::Holder;
use super::host::SharedHost;
use super::mounts;
use super::payload::Payload;
use super::policy;
use super::protocol::{ConfigPayload, WirePosture};
use super::workspace;
use crate::backends::shared::{self, DOWNLOAD_TIMEOUT, UPLOAD_TIMEOUT};
use crate::sandbox::contract::{
    Backend, ContributionSort, ContributionSupport, CreateMode, CreateRequest, Error, Execution,
    Inspection, SandboxRecord, Scope, Teardown, WorkspaceBinding,
};
use crate::sandbox::facets::Lifecycle;
use crate::sandbox::report::{Capability, Check, Readiness, State};

/// The Seatbelt backend, over a host seam.
pub(crate) struct SeatbeltBackend {
    settings: Settings,
    host: SharedHost,
    helper: PathBuf,
    state_root: PathBuf,
    metadata: PathBuf,
}

impl SeatbeltBackend {
    pub(crate) fn new(settings: Settings, host: SharedHost) -> Result<Self, Error> {
        let helper = helper_path()?;
        let base = agentkit_common::data_dir("litterbox");
        Ok(Self {
            settings,
            host,
            helper,
            state_root: base.join("seatbelt"),
            metadata: base.join("metadata"),
        })
    }

    pub(crate) fn with_helper(settings: Settings, host: SharedHost, helper: PathBuf) -> Self {
        let base = agentkit_common::data_dir("litterbox");
        Self {
            settings,
            host,
            helper,
            state_root: base.join("seatbelt"),
            metadata: base.join("metadata"),
        }
    }

    fn state_dir(&self, project: &str, name: &str) -> PathBuf {
        self.state_root.join(project).join(name)
    }

    fn posture(&self) -> WirePosture {
        match self.settings.network {
            NetworkSetting::Denied => WirePosture::Denied,
            NetworkSetting::Host => WirePosture::Host,
        }
    }

    fn posture_name(&self) -> &'static str {
        match self.settings.network {
            NetworkSetting::Denied => "denied",
            NetworkSetting::Host => "host",
        }
    }

    fn create_sandbox(&self, request: &CreateRequest) -> Result<SandboxRecord, Error> {
        if request.mode == CreateMode::ReuseExisting {
            return Err(Error::Unsupported {
                operation: format!("repair the sandbox '{}'", request.name),
                backend: BACKEND.to_string(),
                primitive: "a backend recovery path".to_string(),
                reason: "the Seatbelt backend cannot recreate a holder against an existing record"
                    .to_string(),
                remedy: format!(
                    "run `litterbox delete --force {}` and create it again",
                    request.name
                ),
            });
        }
        if !request.ports.is_empty() {
            return Err(Error::Unsupported {
                operation: "create".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a network-bind rule".to_string(),
                reason: "the Seatbelt profile emits neither network-bind nor network-inbound"
                    .to_string(),
                remedy: "remove the declared ports".to_string(),
            });
        }
        let readiness = self.readiness_report();
        if let Some(check) = readiness
            .checks
            .iter()
            .find(|check| check.state != State::Available)
        {
            return Err(Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: check
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("{} is not available", check.primitive)),
                remedy: check
                    .remedy
                    .clone()
                    .unwrap_or_else(|| "resolve the readiness report".to_string()),
            });
        }

        policy::validate_tag(&request.name).map_err(|reason| Error::Misconfigured {
            message: format!(
                "sandbox name '{}' is not a valid tag: {reason}",
                request.name
            ),
        })?;

        let state = self.state_dir(&request.project_slug, &request.name);
        if state.exists() {
            return Err(Error::Conflict {
                name: request.name.clone(),
            });
        }
        let real_home = self
            .host
            .real_home()
            .map_err(|error| Error::failed(format!("cannot read the real home: {error}")))?;
        let socket = socket_path()?;
        let mut baseline_reads = Vec::new();
        for path in &self.settings.baseline_reads {
            match self.host.realpath(path) {
                Ok(resolved) => baseline_reads.push(resolved),
                Err(error)
                    if defaults::BASELINE_READS
                        .iter()
                        .any(|default| Path::new(default) == path) =>
                {
                    tracing::debug!(
                        path = %path.display(),
                        %error,
                        "skipping an absent default baseline path"
                    );
                }
                Err(error) => {
                    return Err(Error::Misconfigured {
                        message: format!(
                            "backends.seatbelt.baseline-reads entry '{}' cannot be resolved: {error}",
                            path.display()
                        ),
                    });
                }
            }
        }
        fs::create_dir_all(&state).map_err(|error| {
            Error::failed(format!(
                "cannot create the state directory '{}': {error}",
                state.display()
            ))
        })?;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).map_err(|error| {
            Error::failed(format!("cannot protect the state directory: {error}"))
        })?;

        // A copied workspace is materialised into the state directory; a bound
        // workspace is the host directory the VCS owns, used directly because
        // Seatbelt has no mount namespace.
        let workspace = match request.workspace.binding {
            WorkspaceBinding::Copied => {
                let workspace = state.join("workspace");
                if let Err(error) = workspace::copy_from(&request.workspace.source, &workspace) {
                    let _ = fs::remove_dir_all(&state);
                    return Err(error);
                }
                workspace
            }
            WorkspaceBinding::Bound => request.workspace.source.clone(),
        };

        let context = mounts::Context {
            workspace: &workspace,
            state_dir: &state,
            metadata_dir: &self.metadata,
            home: &real_home,
        };
        let mounts = match mounts::from_steps(&request.provision, &context, self.host.as_ref()) {
            Ok(mounts) => mounts,
            Err(error) => {
                let _ = fs::remove_dir_all(&state);
                return Err(error);
            }
        };
        let mut granted = mounts.reads.clone();
        granted.extend(baseline_reads.iter().cloned());
        let environment = match super::environment::merge(&request.provision, &workspace, &granted)
        {
            Ok(environment) => environment,
            Err(error) => {
                let _ = fs::remove_dir_all(&state);
                return Err(error);
            }
        };
        let config = ConfigPayload {
            workspace: path_bytes(&workspace),
            reads: mounts.reads.iter().map(|path| path_bytes(path)).collect(),
            literals: mounts.named.iter().map(|path| path_bytes(path)).collect(),
            writes: vec![path_bytes(&workspace)],
            posture: self.posture(),
            endpoints: Vec::new(),
            tag: request.name.as_bytes().to_vec(),
            environment: environment
                .variables
                .iter()
                .map(|(name, value)| (name.as_bytes().to_vec(), value.as_bytes().to_vec()))
                .collect(),
            baseline_reads: baseline_reads.iter().map(|path| path_bytes(path)).collect(),
            mach_services: self
                .settings
                .mach_services
                .iter()
                .map(|service| service.as_bytes().to_vec())
                .collect(),
            masked_paths: request
                .workspace
                .masked_paths
                .iter()
                .map(|path| path_bytes(path))
                .collect(),
        };

        let log = state.join("holder.log");
        let holder = match Holder::start(self.host.clone(), &self.helper, &socket, &log, config) {
            Ok(holder) => holder,
            Err(error) => {
                let _ = fs::remove_dir_all(&state);
                let _ = fs::remove_file(&socket);
                return Err(error);
            }
        };

        for step in &request.provision {
            let Some(action) = &step.sandbox else {
                continue;
            };
            let execution =
                match holder.exec(&["/bin/sh".to_string(), "-c".to_string(), action.run.clone()]) {
                    Ok(execution) => execution,
                    Err(error) => {
                        let _ = holder.signal(super::host::Signal::Kill);
                        let _ = fs::remove_dir_all(&state);
                        let _ = fs::remove_file(&socket);
                        return Err(error);
                    }
                };
            if execution.exit_code != 0 {
                let output = shared::combined_output(&execution.stdout, &execution.stderr);
                let _ = holder.signal(super::host::Signal::Kill);
                let _ = fs::remove_dir_all(&state);
                let _ = fs::remove_file(&socket);
                return Err(shared::command_failure(
                    BACKEND,
                    step,
                    execution.exit_code,
                    &output,
                ));
            }
        }

        let payload = Payload {
            version: super::payload::VERSION,
            helper: self.helper.clone(),
            holder_pid: holder.pid,
            holder_start_secs: holder.start.secs,
            holder_start_micros: holder.start.micros,
            socket,
            workspace_host: workspace.clone(),
            workspace_sandbox: workspace,
            reads: mounts.reads,
            collapsed: mounts.collapsed,
            provenance: environment.provenance,
            posture: self.posture_name().to_string(),
            tag: request.name.clone(),
        };

        if let Err(error) = write_payload(&state, &payload) {
            let _ = holder.signal(super::host::Signal::Kill);
            let _ = fs::remove_dir_all(&state);
            let _ = fs::remove_file(&payload.socket);
            return Err(error);
        }

        Ok(SandboxRecord {
            name: request.name.clone(),
            backend: BACKEND.to_string(),
            project_slug: request.project_slug.clone(),
            data: payload.to_value()?,
        })
    }

    fn pause_sandbox(&self, record: &SandboxRecord) -> Result<(), Error> {
        let payload = Payload::from_record(record)?;
        self.require_live(&payload, "pause")?;
        self.host
            .signal_group(payload.holder_pid, super::host::Signal::Stop)
            .map_err(|error| Error::failed(format!("cannot pause the holder: {error}")))?;
        self.wait_for(&payload, super::host::PidState::Stopped, "pause")
    }

    fn resume_sandbox(&self, record: &SandboxRecord) -> Result<(), Error> {
        let payload = Payload::from_record(record)?;
        match payload.state(self.host.as_ref()) {
            super::payload::PayloadState::Paused => {}
            super::payload::PayloadState::Live => return Ok(()),
            super::payload::PayloadState::Missing => {
                return Err(Error::NotFound {
                    name: record.name.clone(),
                });
            }
            super::payload::PayloadState::Foreign => {
                return Err(Error::Denied {
                    operation: "resume".to_string(),
                    reason: "the recorded holder is not the process at that pid".to_string(),
                });
            }
            super::payload::PayloadState::Invalid(reason) => {
                return Err(Error::failed(reason));
            }
        }
        self.host
            .signal_group(payload.holder_pid, super::host::Signal::Continue)
            .map_err(|error| Error::failed(format!("cannot resume the holder: {error}")))?;
        self.wait_for(&payload, super::host::PidState::Running, "resume")
    }

    fn delete_sandbox(&self, record: &SandboxRecord) -> Result<Teardown, Error> {
        let state = self.state_dir(&record.project_slug, &record.name);
        let payload = Payload::from_record(record).ok();
        let mut compute_stopped = false;
        let mut detail = None;
        if let Some(payload) = &payload {
            match payload.state(self.host.as_ref()) {
                super::payload::PayloadState::Live | super::payload::PayloadState::Paused => {
                    let _ = self
                        .host
                        .signal_group(payload.holder_pid, super::host::Signal::Kill);
                }
                super::payload::PayloadState::Missing => {
                    // The pid is absent; a group may still hold confined
                    // children. Signal it and log the orphan.
                    let _ = self
                        .host
                        .signal_group(payload.holder_pid, super::host::Signal::Kill);
                    tracing::warn!(
                        sandbox = %record.name,
                        "the holder was gone; the recorded group was signalled as an orphan"
                    );
                }
                super::payload::PayloadState::Foreign => {
                    detail = Some("the recorded pid belongs to another process".to_string());
                    tracing::warn!(
                        sandbox = %record.name,
                        "the recorded pid now belongs to another process; it was not signalled"
                    );
                }
                super::payload::PayloadState::Invalid(reason) => {
                    detail = Some(reason.clone());
                    tracing::warn!(sandbox = %record.name, %reason, "the record is not readable");
                }
            }
            let _ = fs::remove_file(&payload.socket);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while self.host.pid_exists(payload.holder_pid) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if self.host.pid_exists(payload.holder_pid) {
                tracing::warn!(
                    sandbox = %record.name,
                    "the holder group did not stop within 2s; its state is being removed"
                );
                detail = Some("the holder group did not stop within 2s".to_string());
            } else {
                compute_stopped = true;
            }
        } else {
            detail = Some("the record is unreadable".to_string());
            tracing::warn!(
                sandbox = %record.name,
                "the record is unreadable; any socket it named in the temp directory is not removed"
            );
        }
        let _ = fs::remove_dir_all(&state);
        Ok(Teardown {
            compute_stopped,
            state_paths: vec![state],
            detail,
        })
    }

    fn require_live(&self, payload: &Payload, operation: &str) -> Result<(), Error> {
        match payload.state(self.host.as_ref()) {
            super::payload::PayloadState::Live => Ok(()),
            super::payload::PayloadState::Paused => Ok(()),
            super::payload::PayloadState::Missing => Err(Error::NotFound {
                name: payload.tag.clone(),
            }),
            super::payload::PayloadState::Foreign => Err(Error::Denied {
                operation: operation.to_string(),
                reason: "the recorded holder is not the process at that pid".to_string(),
            }),
            super::payload::PayloadState::Invalid(reason) => Err(Error::failed(reason)),
        }
    }

    fn wait_for(
        &self,
        payload: &Payload,
        wanted: super::host::PidState,
        operation: &str,
    ) -> Result<(), Error> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            match self.host.pid_state(payload.holder_pid) {
                Ok(state) if state == wanted => return Ok(()),
                Ok(super::host::PidState::Gone) => {
                    return Err(Error::NotFound {
                        name: payload.tag.clone(),
                    });
                }
                Ok(_) if std::time::Instant::now() >= deadline => {
                    return Err(Error::failed(format!(
                        "the holder did not reach the expected state for {operation}"
                    )));
                }
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(error) => {
                    return Err(Error::failed(format!(
                        "cannot read the holder state during {operation}: {error}"
                    )));
                }
            }
        }
    }

    fn readiness_report(&self) -> Readiness {
        let mut checks = vec![Check {
            primitive: "macOS".to_string(),
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        }];

        let symbol = std::ffi::CString::new("sandbox_init_with_parameters").expect("literal");
        // SAFETY: the symbol name is NUL terminated and dlsym only looks it up.
        let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) };
        checks.push(Check {
            primitive: "sandbox_init_with_parameters".to_string(),
            state: if found.is_null() {
                State::Unavailable
            } else {
                State::Available
            },
            reason: found
                .is_null()
                .then(|| "the symbol is not exported by this macOS".to_string()),
            remedy: found
                .is_null()
                .then(|| "use a macOS that still ships the sandbox entry point".to_string()),
            fallback: None,
        });

        checks.push(self.helper_check());
        checks.push(self.probe_check());
        checks.push(self.state_root_check());

        Readiness {
            backend: BACKEND.to_string(),
            checks,
        }
    }

    fn helper_check(&self) -> Check {
        let metadata = fs::metadata(&self.helper);
        let mut state = State::Available;
        let mut reason = None;
        let mut remedy = None;
        match metadata {
            Ok(metadata) => {
                use std::os::unix::fs::MetadataExt;

                let mode = metadata.permissions().mode() & 0o777;
                let owner = metadata.uid();
                // SAFETY: geteuid has no arguments and cannot fail.
                let effective = unsafe { libc::geteuid() };
                if !metadata.is_file() {
                    state = State::Unavailable;
                    reason = Some("the helper is not a regular file".to_string());
                    remedy = Some("reinstall litterbox".to_string());
                } else if mode & 0o022 != 0 {
                    state = State::Unavailable;
                    reason = Some("the helper is group or world writable".to_string());
                    remedy = Some("reinstall the helper with mode 0755".to_string());
                } else if owner != 0 && owner != effective {
                    state = State::Unavailable;
                    reason = Some("the helper is owned by another user".to_string());
                    remedy = Some("reinstall the helper as this user or root".to_string());
                }
            }
            Err(error) => {
                state = State::Unavailable;
                reason = Some(format!(
                    "the helper '{}' is not readable: {error}",
                    self.helper.display()
                ));
                remedy = Some("install the helper beside litterbox".to_string());
            }
        }
        Check {
            primitive: "the Seatbelt helper".to_string(),
            state,
            reason,
            remedy,
            fallback: None,
        }
    }

    fn probe_check(&self) -> Check {
        let mut child = match std::process::Command::new(&self.helper)
            .arg("--probe")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return Check {
                    primitive: "the Seatbelt probe".to_string(),
                    state: State::Unavailable,
                    reason: Some(format!("the probe could not start: {error}")),
                    remedy: Some("check the helper is executable".to_string()),
                    fallback: None,
                };
            }
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    return Check {
                        primitive: "the Seatbelt probe".to_string(),
                        state: State::Available,
                        reason: None,
                        remedy: None,
                        fallback: None,
                    };
                }
                Ok(Some(status)) => {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        use std::io::Read;
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    return Check {
                        primitive: "the Seatbelt probe".to_string(),
                        state: State::Unavailable,
                        reason: Some(format!("the probe exited {status}: {}", stderr.trim())),
                        remedy: Some("the macOS sandbox facility is not usable".to_string()),
                        fallback: None,
                    };
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Check {
                        primitive: "the Seatbelt probe".to_string(),
                        state: State::Unavailable,
                        reason: Some("the probe did not finish within two seconds".to_string()),
                        remedy: Some("the macOS sandbox facility is not usable".to_string()),
                        fallback: None,
                    };
                }
                Err(error) => {
                    let _ = child.kill();
                    return Check {
                        primitive: "the Seatbelt probe".to_string(),
                        state: State::Unavailable,
                        reason: Some(format!("the probe could not be observed: {error}")),
                        remedy: Some("the macOS sandbox facility is not usable".to_string()),
                        fallback: None,
                    };
                }
            }
        }
    }

    fn state_root_check(&self) -> Check {
        if let Some(parent) = self.state_root.parent() {
            match fs::create_dir_all(parent) {
                Ok(()) => {
                    return Check {
                        primitive: "the state root".to_string(),
                        state: State::Available,
                        reason: None,
                        remedy: None,
                        fallback: None,
                    };
                }
                Err(error) => {
                    return Check {
                        primitive: "the state root".to_string(),
                        state: State::Unavailable,
                        reason: Some(format!(
                            "the state root '{}' cannot be created: {error}",
                            parent.display()
                        )),
                        remedy: Some("choose a writable data directory".to_string()),
                        fallback: None,
                    };
                }
            }
        }
        Check {
            primitive: "the state root".to_string(),
            state: State::Unavailable,
            reason: Some("the state root has no parent".to_string()),
            remedy: Some("choose a writable data directory".to_string()),
            fallback: None,
        }
    }

    fn inspect_sandbox(&self, record: &SandboxRecord) -> Result<Inspection, Error> {
        let payload = Payload::from_record(record)?;
        let status = match payload.state(self.host.as_ref()) {
            super::payload::PayloadState::Live => {
                let holder = self.holder_for(record)?;
                match holder.status() {
                    Ok(()) => crate::sandbox::contract::SandboxStatus::Active,
                    Err(error) => crate::sandbox::contract::SandboxStatus::Error {
                        reason: error.to_string(),
                    },
                }
            }
            super::payload::PayloadState::Paused => crate::sandbox::contract::SandboxStatus::Paused,
            super::payload::PayloadState::Missing => {
                crate::sandbox::contract::SandboxStatus::Missing
            }
            super::payload::PayloadState::Foreign => {
                crate::sandbox::contract::SandboxStatus::Error {
                    reason: "the recorded holder is not the process at that pid".to_string(),
                }
            }
            super::payload::PayloadState::Invalid(reason) => {
                crate::sandbox::contract::SandboxStatus::Error { reason }
            }
        };
        let egress = match payload.posture.as_str() {
            "host" => crate::sandbox::report::EgressPosture::Direct,
            _ => crate::sandbox::report::EgressPosture::Denied,
        };
        let status = if matches!(
            status,
            crate::sandbox::contract::SandboxStatus::Active
                | crate::sandbox::contract::SandboxStatus::Paused
        ) && !payload.workspace_host.exists()
        {
            crate::sandbox::contract::SandboxStatus::Missing
        } else {
            status
        };
        Ok(Inspection {
            name: record.name.clone(),
            backend: BACKEND.to_string(),
            status,
            egress,
            workspace: payload.workspace_sandbox.to_string_lossy().into_owned(),
            environment: payload.provenance.clone(),
            forwards: Vec::new(),
        })
    }

    fn list_sandboxes(&self, scope: &Scope) -> Vec<SandboxRecord> {
        let projects = match scope {
            Scope::Repository { project_slug } => {
                vec![(project_slug.clone(), self.state_root.join(project_slug))]
            }
            Scope::Machine => match fs::read_dir(&self.state_root) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.path(),
                        )
                    })
                    .collect(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => {
                    tracing::warn!(%error, "cannot list the seatbelt state root");
                    Vec::new()
                }
            },
        };

        let mut records = Vec::new();
        for (project, project_dir) in projects {
            let Ok(entries) = fs::read_dir(&project_dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let payload_path = entry.path().join("payload.toml");
                let Ok(text) = fs::read_to_string(&payload_path) else {
                    tracing::warn!(sandbox = %name, "the payload file is missing");
                    continue;
                };
                let Ok(value) = toml::from_str::<toml::Value>(&text) else {
                    tracing::warn!(sandbox = %name, "the payload file is not TOML");
                    continue;
                };
                let record = SandboxRecord {
                    name: name.clone(),
                    backend: BACKEND.to_string(),
                    project_slug: project.clone(),
                    data: value,
                };
                match Payload::from_record(&record) {
                    Ok(_) => records.push(record),
                    Err(error) => {
                        tracing::warn!(sandbox = %name, %error, "the payload is unreadable")
                    }
                }
            }
        }
        records
    }

    fn capability_report(&self) -> Vec<Capability> {
        vec![
            Capability {
                operation: "pause and resume".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
                fallback: None,
            },
            Capability {
                operation: "resource limits".to_string(),
                state: State::Unavailable,
                reason: Some("Seatbelt has no CPU, memory or task quota".to_string()),
                remedy: Some("bound the workload from outside the sandbox".to_string()),
                fallback: None,
            },
            Capability {
                operation: "port forwarding".to_string(),
                state: State::Unavailable,
                reason: Some(
                    "the profile emits neither network-bind nor network-inbound".to_string(),
                ),
                remedy: Some("use an egress proxy instead".to_string()),
                fallback: None,
            },
        ]
    }

    fn holder_for(&self, record: &SandboxRecord) -> Result<Holder, Error> {
        let payload = Payload::from_record(record)?;
        match payload.state(self.host.as_ref()) {
            super::payload::PayloadState::Live => {}
            super::payload::PayloadState::Paused => {
                return Err(Error::Denied {
                    operation: "exec".to_string(),
                    reason: "the sandbox is paused".to_string(),
                });
            }
            super::payload::PayloadState::Missing => {
                return Err(Error::NotFound {
                    name: record.name.clone(),
                });
            }
            super::payload::PayloadState::Foreign => {
                return Err(Error::Denied {
                    operation: "exec".to_string(),
                    reason: "the recorded holder is not the process at that pid".to_string(),
                });
            }
            super::payload::PayloadState::Invalid(reason) => {
                return Err(Error::failed(reason));
            }
        }
        Ok(Holder::open(
            self.host.clone(),
            payload.holder_pid,
            super::host::ProcessStart {
                secs: payload.holder_start_secs,
                micros: payload.holder_start_micros,
            },
            payload.socket.clone(),
            payload.workspace_host.parent().map_or_else(
                || PathBuf::from("holder.log"),
                |state| state.join("holder.log"),
            ),
        ))
    }
}

impl Lifecycle for SeatbeltBackend {
    fn pause<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { self.pause_sandbox(sandbox) })
    }

    fn resume<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { self.resume_sandbox(sandbox) })
    }
}

impl Backend for SeatbeltBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move { self.create_sandbox(request) })
    }

    fn shell(&self, _sandbox: &SandboxRecord) -> Result<Vec<String>, Error> {
        // The baseline grants /bin and the tool layer needs the shell even when
        // a devShell's PATH does not contain one.
        Ok(vec!["/bin/sh".to_string(), "-c".to_string()])
    }

    fn list<'a>(&'a self, scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async move { Ok(self.list_sandboxes(scope)) })
    }

    fn exec<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        let holder = match self.holder_for(sandbox) {
            Ok(holder) => holder,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let command = command.to_vec();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || holder.exec(&command))
                .await
                .map_err(|error| Error::failed(format!("the command task failed: {error}")))?
        })
    }

    fn upload<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a Path,
        destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        let payload = match Payload::from_record(sandbox) {
            Ok(payload) => payload,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let source = source.to_path_buf();
        let destination = destination.to_string();
        Box::pin(async move {
            let root = payload.workspace_host;
            shared::transfer(UPLOAD_TIMEOUT, move || {
                workspace::transfer_in(&source, &root, &destination)
            })
            .await
        })
    }

    fn download<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a str,
        destination: &'a Path,
        selector: Option<std::sync::Arc<dyn crate::sandbox::contract::SnapshotSelector>>,
    ) -> BoxFuture<'a, Result<(), Error>> {
        let payload = match Payload::from_record(sandbox) {
            Ok(payload) => payload,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let source = source.to_string();
        let destination = destination.to_path_buf();
        Box::pin(async move {
            let root = payload.workspace_host;
            shared::transfer(DOWNLOAD_TIMEOUT, move || {
                workspace::transfer_out(&root, &source, &destination, selector.as_deref())
            })
            .await
        })
    }

    fn inspect<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move { self.inspect_sandbox(sandbox) })
    }

    fn delete<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<Teardown, Error>> {
        Box::pin(async move { self.delete_sandbox(sandbox) })
    }

    fn stop<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<Teardown, Error>> {
        Box::pin(async move {
            let payload = Payload::from_record(sandbox).ok();
            if let Some(payload) = &payload {
                let _ = self
                    .host
                    .signal_group(payload.holder_pid, super::host::Signal::Kill);
            }
            Ok(Teardown {
                compute_stopped: payload.is_some(),
                state_paths: Vec::new(),
                detail: None,
            })
        })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async { Ok(self.readiness_report()) })
    }

    fn capabilities(&self) -> BoxFuture<'_, Result<Vec<Capability>, Error>> {
        Box::pin(async { Ok(self.capability_report()) })
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

fn write_payload(state: &Path, payload: &Payload) -> Result<(), Error> {
    let value = payload.to_value()?;
    let text = toml::to_string(&value)
        .map_err(|error| Error::failed(format!("cannot serialise the payload: {error}")))?;
    let path = state.join("payload.toml");
    fs::write(&path, text)
        .map_err(|error| Error::failed(format!("cannot write '{}': {error}", path.display())))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| Error::failed(format!("cannot protect '{}': {error}", path.display())))?;
    Ok(())
}

fn helper_path() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe()
        .map_err(|error| Error::failed(format!("cannot find the current executable: {error}")))?;
    let parent = exe
        .parent()
        .ok_or_else(|| Error::failed("the current executable has no parent"))?;
    Ok(parent.join("litterbox-seatbelt-helper"))
}

fn socket_path() -> Result<PathBuf, Error> {
    let mut bytes = [0u8; 16];
    // SAFETY: the buffer is valid for its length, which getentropy limits to 256.
    let result = unsafe { libc::getentropy(bytes.as_mut_ptr() as *mut libc::c_void, bytes.len()) };
    if result != 0 {
        return Err(Error::failed(format!(
            "cannot generate a socket name: {}",
            std::io::Error::last_os_error()
        )));
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(std::env::temp_dir().join(format!("lbx-{hex}.sock")))
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::SeatbeltBackend;
    use crate::backends::seatbelt::config::{BaselineSetting, NetworkSetting, Settings};
    use crate::backends::seatbelt::host::{Host, SystemHost};
    use crate::sandbox::contract::{Backend, CreateMode, CreateRequest, Error, Workspace};
    use crate::sandbox::facets::Lifecycle;

    fn helper() -> PathBuf {
        std::env::current_exe()
            .expect("test executable")
            .parent()
            .expect("deps")
            .parent()
            .expect("target")
            .join("litterbox-seatbelt-helper")
    }

    fn settings() -> Settings {
        Settings {
            network: NetworkSetting::Denied,
            baseline: BaselineSetting::Default,
            baseline_reads: crate::backends::seatbelt::defaults::BASELINE_READS
                .iter()
                .map(PathBuf::from)
                .collect(),
            mach_services: crate::backends::seatbelt::defaults::MACH_SERVICES
                .iter()
                .map(|service| service.to_string())
                .collect(),
        }
    }

    struct Cleanup {
        pid: u32,
        socket: PathBuf,
        state: PathBuf,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            // SAFETY: the group belongs to a sandbox this test created; a dead
            // group returns ESRCH and is ignored.
            unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
            let mut status = 0;
            // SAFETY: the holder is a child of this test process; waitpid
            // reaps it. A reaper thread in the holder may have won the race,
            // in which case this returns ECHILD.
            unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, 0) };
            let _ = std::fs::remove_file(&self.socket);
            let _ = std::fs::remove_dir_all(&self.state);
            if let Some(project) = self.state.parent() {
                let _ = std::fs::remove_dir(project);
            }
        }
    }

    async fn sandbox(
        label: &str,
    ) -> (
        SeatbeltBackend,
        crate::sandbox::contract::SandboxRecord,
        PathBuf,
        Cleanup,
    ) {
        sandbox_with(label, Vec::new()).await
    }

    async fn sandbox_with(
        label: &str,
        provision: Vec<crate::sandbox::contract::ResolvedStep>,
    ) -> (
        SeatbeltBackend,
        crate::sandbox::contract::SandboxRecord,
        PathBuf,
        Cleanup,
    ) {
        let helper = helper();
        assert!(helper.exists(), "build the helper before this test");
        let source = tempfile::tempdir().expect("source");
        std::fs::write(source.path().join("README"), b"hello").expect("file");
        let project = format!("test-{}-{}-{}", std::process::id(), label, line!());
        let name = "one";
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project)
            .join(name);
        let _ = std::fs::remove_dir_all(&state);
        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), helper);
        let request = CreateRequest {
            name: name.to_string(),
            project_slug: project,
            workspace: Workspace::copied(source.path().to_path_buf()),
            provision,
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        };
        let record = backend.create(&request).await.expect("create");
        let payload = payload(&record);
        let cleanup = Cleanup {
            pid: payload.holder_pid,
            socket: payload.socket.clone(),
            state: state.clone(),
        };
        (backend, record, state, cleanup)
    }

    fn payload(
        record: &crate::sandbox::contract::SandboxRecord,
    ) -> crate::backends::seatbelt::payload::Payload {
        crate::backends::seatbelt::payload::Payload::from_record(record).expect("payload")
    }

    fn request(source: &Path, project: &str) -> CreateRequest {
        CreateRequest {
            name: "one".to_string(),
            project_slug: project.to_string(),
            workspace: Workspace::copied(source.to_path_buf()),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        }
    }

    #[tokio::test]
    async fn create_refuses_when_readiness_fails() {
        let helper = tempfile::tempdir().expect("helper");
        let backend = SeatbeltBackend::with_helper(
            settings(),
            Arc::new(SystemHost::new()),
            helper.path().to_path_buf(),
        );
        let source = tempfile::tempdir().expect("source");
        let project = format!("test-{}-readiness", std::process::id());
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project);
        let _ = std::fs::remove_dir_all(&state);

        let error = backend
            .create(&request(source.path(), &project))
            .await
            .expect_err("untrusted helper");
        assert!(matches!(error, Error::Unavailable { .. }), "{error}");
        assert!(!state.exists(), "create touched the state root");
    }

    #[tokio::test]
    async fn create_refuses_an_unresolvable_configured_baseline() {
        let helper = helper();
        let mut settings = settings();
        settings
            .baseline_reads
            .push(PathBuf::from("/lbx-no-such-baseline-read"));
        let backend = SeatbeltBackend::with_helper(settings, Arc::new(SystemHost::new()), helper);
        let source = tempfile::tempdir().expect("source");
        let project = format!("test-{}-baseline", std::process::id());
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project);
        let _ = std::fs::remove_dir_all(&state);

        let error = backend
            .create(&request(source.path(), &project))
            .await
            .expect_err("unresolvable baseline");
        assert!(matches!(error, Error::Misconfigured { .. }), "{error}");
        assert!(!state.exists(), "create touched the state root");
    }

    #[tokio::test]
    async fn inspect_reports_a_missing_workspace() {
        let (backend, record, _state, _cleanup) = sandbox("missing-workspace").await;
        let workspace = payload(&record).workspace_host;
        std::fs::remove_dir_all(&workspace).expect("remove workspace");
        let inspection = backend.inspect(&record).await.expect("inspect");
        assert_eq!(
            inspection.status,
            crate::sandbox::contract::SandboxStatus::Missing
        );
        backend.delete(&record).await.expect("delete");
    }

    #[tokio::test]
    async fn exec_returns_a_workload_status_and_both_streams() {
        let (backend, record, state, _cleanup) = sandbox("streams").await;
        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo out; echo err >&2; exit 7".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.exit_code, 7);
        assert_eq!(execution.stdout, "out\n");
        assert!(execution.stderr.contains("err"));
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn exec_refuses_a_paused_or_foreign_holder_and_reports_missing() {
        let (backend, mut record, state, _cleanup) = sandbox("state").await;

        backend.pause(&record).await.expect("pause");
        assert!(matches!(
            backend.exec(&record, &["/bin/true".to_string()]).await,
            Err(Error::Denied { .. })
        ));
        backend.resume(&record).await.expect("resume");

        let mut foreign = payload(&record);
        foreign.holder_start_secs += 1;
        record.data = foreign.to_value().expect("value");
        assert!(matches!(
            backend.exec(&record, &["/bin/true".to_string()]).await,
            Err(Error::Denied { .. })
        ));

        let mut missing = payload(&record);
        missing.holder_pid = 999_999;
        record.data = missing.to_value().expect("value");
        assert!(matches!(
            backend.exec(&record, &["/bin/true".to_string()]).await,
            Err(Error::NotFound { .. })
        ));

        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn a_sandbox_with_no_steps_has_only_the_three_owned_variables() {
        let (backend, record, state, _cleanup) = sandbox("no-steps").await;
        let execution = backend
            .exec(&record, &["/usr/bin/env".to_string()])
            .await
            .expect("exec");
        let mut names: Vec<&str> = execution
            .stdout
            .lines()
            .filter_map(|line| line.split('=').next())
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["HOME", "PWD", "TMPDIR"]);
        assert!(!execution.stdout.contains("PATH="));
        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn writes_and_reads_outside_the_workspace_are_denied() {
        let (backend, record, state, _cleanup) = sandbox("isolation").await;
        let outside = "/private/tmp/lbx-t021-outside";
        let _ = std::fs::remove_file(outside);
        let write = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!("echo escape > {outside}"),
                ],
            )
            .await
            .expect("exec");
        assert_ne!(write.exit_code, 0);
        assert!(!Path::new(outside).exists());

        let secret = tempfile::tempdir().expect("secret");
        let secret_file = secret.path().join("credential");
        std::fs::write(&secret_file, b"material").expect("secret");
        let read = backend
            .exec(
                &record,
                &[
                    "/bin/cat".to_string(),
                    secret_file.to_string_lossy().into_owned(),
                ],
            )
            .await
            .expect("exec");
        assert_ne!(read.exit_code, 0);
        assert!(read.stdout.is_empty());

        let descendant = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!("/bin/sh -c 'echo child > {outside}'"),
                ],
            )
            .await
            .expect("exec");
        assert_ne!(descendant.exit_code, 0);
        assert!(!Path::new(outside).exists());

        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn a_bound_workspace_masks_the_metadata_path() {
        let helper = helper();
        assert!(helper.exists(), "build the helper before this test");
        let source = tempfile::tempdir().expect("source");
        std::fs::write(source.path().join("visible.txt"), b"hello").expect("file");
        std::fs::create_dir_all(source.path().join(".jj")).expect("jj dir");
        std::fs::write(source.path().join(".jj").join("secret.txt"), b"top secret")
            .expect("secret");

        let project = format!("test-{}-bound-mask-{}", std::process::id(), line!());
        let name = "one";
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project)
            .join(name);
        let _ = std::fs::remove_dir_all(&state);
        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), helper);
        let request = CreateRequest {
            name: name.to_string(),
            project_slug: project,
            workspace: Workspace::bound(source.path().to_path_buf(), vec![PathBuf::from(".jj")]),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        };
        let record = backend.create(&request).await.expect("create");
        let payload = payload(&record);
        let _cleanup = Cleanup {
            pid: payload.holder_pid,
            socket: payload.socket.clone(),
            state: state.clone(),
        };

        let read_visible = backend
            .exec(
                &record,
                &["/bin/cat".to_string(), "visible.txt".to_string()],
            )
            .await
            .expect("exec");
        assert_eq!(read_visible.exit_code, 0, "{}", read_visible.stderr);
        assert_eq!(read_visible.stdout, "hello");

        let read_secret = backend
            .exec(
                &record,
                &["/bin/cat".to_string(), ".jj/secret.txt".to_string()],
            )
            .await
            .expect("exec");
        assert_ne!(read_secret.exit_code, 0, "the masked path is unreadable");

        let write_secret = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo evil > .jj/evil.txt".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_ne!(write_secret.exit_code, 0, "the masked path is unwritable");
        assert!(
            !source.path().join(".jj").join("evil.txt").exists(),
            "the write did not reach the host"
        );

        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn a_large_contribution_is_collapsed_through_create() {
        let source = tempfile::tempdir().expect("source");
        let closure = source.path().join("closure");
        for index in 0..=64 {
            std::fs::create_dir_all(closure.join(format!("p{index}"))).expect("package");
        }
        let mounts: Vec<PathBuf> = (0..=64)
            .map(|index| closure.join(format!("p{index}")))
            .collect();
        let step = crate::sandbox::contract::ResolvedStep {
            identity: "dev".to_string(),
            kind: "command".to_string(),
            placement: crate::sandbox::contract::Placement::Host,
            sandbox: None,
            contributions: crate::sandbox::contract::Contributions {
                mounts,
                ..crate::sandbox::contract::Contributions::default()
            },
        };
        let (backend, record, state, _cleanup) = sandbox_with("collapse", vec![step]).await;
        let payload = payload(&record);
        assert_eq!(payload.reads.len(), 1);
        assert_eq!(payload.collapsed.len(), 1);
        assert_eq!(payload.collapsed[0].contributed, 65);
        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo ok".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.stdout, "ok\n");
        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn a_baseline_directory_is_granted_to_the_environment() {
        let step = crate::sandbox::contract::ResolvedStep {
            identity: "path".to_string(),
            kind: "devshell".to_string(),
            placement: crate::sandbox::contract::Placement::Host,
            sandbox: None,
            contributions: crate::sandbox::contract::Contributions {
                environment: std::collections::BTreeMap::from([(
                    "PATH".to_string(),
                    "/usr/bin:/bin".to_string(),
                )]),
                ..crate::sandbox::contract::Contributions::default()
            },
        };
        let (backend, record, state, _cleanup) = sandbox_with("baseline-path", vec![step]).await;
        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo $PATH".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.stdout.trim(), "/usr/bin:/bin");
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn a_truncated_stream_is_marked() {
        let (backend, record, state, _cleanup) = sandbox("truncated").await;
        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "/usr/bin/yes 0123456789 | /usr/bin/head -c 2000000".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.exit_code, 0);
        assert!(
            execution.stdout.ends_with("[truncated]"),
            "stdout was {} bytes",
            execution.stdout.len()
        );
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn exec_through_a_contributed_symlink() {
        let dir = tempfile::tempdir().expect("dir");
        let link = dir.path().join("sh");
        std::os::unix::fs::symlink("/bin/sh", &link).expect("symlink");
        let step = crate::sandbox::contract::ResolvedStep {
            identity: "link".to_string(),
            kind: "devshell".to_string(),
            placement: crate::sandbox::contract::Placement::Host,
            sandbox: None,
            contributions: crate::sandbox::contract::Contributions {
                mounts: vec![link.clone()],
                ..crate::sandbox::contract::Contributions::default()
            },
        };
        let (backend, record, state, _cleanup) = sandbox_with("symlink-exec", vec![step]).await;
        let execution = backend
            .exec(
                &record,
                &[
                    link.to_str().expect("utf8").to_string(),
                    "-c".to_string(),
                    "echo ok".to_string(),
                ],
            )
            .await
            .expect("exec through a contributed symlink");
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[ignore = "requires Xcode"]
    #[tokio::test]
    async fn a_contributed_toolchain_compiles_and_an_uncontributed_one_does_not() {
        let xcode = std::path::PathBuf::from("/Applications/Xcode.app");
        let licence = std::path::PathBuf::from("/Library/Preferences/com.apple.dt.Xcode.plist");
        assert!(xcode.exists(), "this host has no Xcode installation");
        assert!(licence.exists(), "this host has no Xcode licence state");

        let step = crate::sandbox::contract::ResolvedStep {
            identity: "toolchain".to_string(),
            kind: "devshell".to_string(),
            placement: crate::sandbox::contract::Placement::Host,
            sandbox: None,
            contributions: crate::sandbox::contract::Contributions {
                environment: std::collections::BTreeMap::from([
                    ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                    (
                        "DEVELOPER_DIR".to_string(),
                        "/Applications/Xcode.app/Contents/Developer".to_string(),
                    ),
                ]),
                mounts: vec![xcode, licence],
                ..crate::sandbox::contract::Contributions::default()
            },
        };
        let (backend, record, state, _cleanup) = sandbox_with("toolchain", vec![step]).await;
        let workspace = state.join("workspace");
        std::fs::write(
            workspace.join("hello.c"),
            b"#include <stdio.h>\nint main(void){printf(\"built\\n\");return 0;}\n",
        )
        .expect("source");
        let compile = backend
            .exec(
                &record,
                &[
                    "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang".to_string(),
                    "-isysroot".to_string(),
                    "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk".to_string(),
                    "-o".to_string(),
                    "hello".to_string(),
                    "hello.c".to_string(),
                ],
            )
            .await
            .expect("compile");
        assert_eq!(compile.exit_code, 0, "{}", compile.stderr);
        let run = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "./hello".to_string(),
                ],
            )
            .await
            .expect("run");
        assert_eq!(run.stdout, "built\n");
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);

        let (backend, record, state, _cleanup) = sandbox("no-toolchain").await;
        let workspace = state.join("workspace");
        std::fs::write(workspace.join("hello.c"), b"int main(void){return 0;}\n").expect("source");
        let compile = backend
            .exec(
                &record,
                &[
                    "/usr/bin/clang".to_string(),
                    "-o".to_string(),
                    "hello".to_string(),
                    "hello.c".to_string(),
                ],
            )
            .await
            .expect("compile");
        assert_ne!(compile.exit_code, 0);
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn a_bare_command_name_is_resolved_through_path() {
        let (backend, record, state, _cleanup) = sandbox("bare-name").await;
        let execution = backend
            .exec(&record, &["ls".to_string(), "/".to_string()])
            .await
            .expect("exec");
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        assert!(execution.stdout.contains("usr"));
        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn the_tool_shell_is_absolute() {
        let (backend, record, state, _cleanup) = sandbox("tool-shell").await;
        assert_eq!(
            backend.shell(&record).expect("shell"),
            vec!["/bin/sh".to_string(), "-c".to_string()]
        );
        backend.delete(&record).await.expect("delete");
        let _ = state;
    }

    #[tokio::test]
    async fn pause_resume_and_delete() {
        let (backend, record, state, _cleanup) = sandbox("lifecycle").await;
        let host = Arc::new(SystemHost::new());
        backend.pause(&record).await.expect("pause");
        let payload = payload(&record);
        assert_eq!(
            payload.state(host.as_ref()),
            crate::backends::seatbelt::payload::PayloadState::Paused
        );
        assert!(matches!(
            backend.exec(&record, &["/bin/true".to_string()]).await,
            Err(Error::Denied { .. })
        ));
        backend.resume(&record).await.expect("resume");
        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo up".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.stdout, "up\n");
        backend.delete(&record).await.expect("delete");
        assert!(!state.exists(), "the state directory is removed");
    }

    #[tokio::test]
    async fn delete_succeeds_when_the_holder_is_gone_and_when_the_record_is_unreadable() {
        let (backend, record, state, _cleanup) = sandbox("delete-gone").await;
        let payload = payload(&record);
        let host = SystemHost::new();
        host.signal_group(
            payload.holder_pid,
            crate::backends::seatbelt::host::Signal::Kill,
        )
        .expect("kill");
        backend.delete(&record).await.expect("delete after kill");
        assert!(!state.exists());

        let (backend, mut record, state, _cleanup) = sandbox("delete-unreadable").await;
        record.data = toml::Value::Table(Default::default());
        backend.delete(&record).await.expect("delete unreadable");
        assert!(!state.exists());
    }

    #[tokio::test]
    async fn inspect_reports_active_paused_and_missing() {
        let (backend, record, state, _cleanup) = sandbox("inspect").await;
        let live = backend.inspect(&record).await.expect("inspect");
        assert_eq!(live.status, crate::sandbox::contract::SandboxStatus::Active);
        assert_eq!(live.backend, "seatbelt");
        assert!(live.forwards.is_empty());

        backend.pause(&record).await.expect("pause");
        let paused = backend.inspect(&record).await.expect("inspect paused");
        assert_eq!(
            paused.status,
            crate::sandbox::contract::SandboxStatus::Paused
        );
        backend.resume(&record).await.expect("resume");

        let payload = payload(&record);
        let host = SystemHost::new();
        host.signal_group(
            payload.holder_pid,
            crate::backends::seatbelt::host::Signal::Kill,
        )
        .expect("kill");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let missing = backend.inspect(&record).await.expect("inspect missing");
        assert_eq!(
            missing.status,
            crate::sandbox::contract::SandboxStatus::Missing
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn list_returns_records_and_reports_a_corrupt_payload() {
        let (backend, record, state, _cleanup) = sandbox("list").await;
        let repository = backend
            .list(&crate::sandbox::contract::Scope::Repository {
                project_slug: record.project_slug.clone(),
            })
            .await
            .expect("list");
        assert!(repository.iter().any(|entry| entry.name == record.name));

        let machine = backend
            .list(&crate::sandbox::contract::Scope::Machine)
            .await
            .expect("list machine");
        assert!(machine.iter().any(|entry| entry.name == record.name));

        std::fs::write(state.join("payload.toml"), "not toml").expect("corrupt");
        let after = backend
            .list(&crate::sandbox::contract::Scope::Machine)
            .await
            .expect("list corrupt");
        assert!(!after
            .iter()
            .any(|entry| entry.name == record.name && entry.project_slug == record.project_slug));
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn upload_and_download_round_trip() {
        let (backend, record, state, _cleanup) = sandbox("transfer").await;
        let host = tempfile::tempdir().expect("host");
        let source = host.path().join("source");
        std::fs::write(&source, b"payload").expect("source");
        backend
            .upload(&record, &source, "in")
            .await
            .expect("upload");
        let destination = host.path().join("destination");
        backend
            .download(&record, "in", &destination, None)
            .await
            .expect("download");
        assert_eq!(
            std::fs::read(destination.join("in/source")).expect("read"),
            b"payload"
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn a_snapshot_downloads_the_workspace_from_its_sandbox_path() {
        let (backend, record, state, _cleanup) = sandbox("snapshot-path").await;
        let workspace = backend.inspect(&record).await.expect("inspect").workspace;
        let host = payload(&record).workspace_host;
        std::fs::write(host.join("RESULT.md"), b"report").expect("write");

        let staging = tempfile::tempdir().expect("staging");
        backend
            .download(&record, &workspace, staging.path(), None)
            .await
            .expect("download");
        assert_eq!(
            std::fs::read(staging.path().join("RESULT.md")).expect("read"),
            b"report"
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn capabilities_name_the_unsupported_operations() {
        let (backend, _record, state, _cleanup) = sandbox("capabilities").await;
        let capabilities = backend.capabilities().await.expect("capabilities");
        assert!(capabilities.iter().any(|entry| {
            entry.operation == "pause and resume"
                && entry.state == crate::sandbox::report::State::Available
        }));
        assert!(capabilities.iter().any(|entry| {
            entry.operation == "resource limits"
                && entry.state == crate::sandbox::report::State::Unavailable
        }));
        assert!(capabilities.iter().any(|entry| {
            entry.operation == "port forwarding"
                && entry.state == crate::sandbox::report::State::Unavailable
        }));
        let _ = std::fs::remove_dir_all(&state);
    }

    #[tokio::test]
    async fn readiness_reports_the_five_checks() {
        let helper = helper();
        assert!(helper.exists(), "build the helper before this test");
        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), helper);
        let readiness = backend.readiness().await.expect("readiness");
        assert_eq!(readiness.checks.len(), 5);
        assert!(readiness.is_available(), "{:?}", readiness.checks);
    }

    #[tokio::test]
    async fn a_missing_or_untrusted_helper_is_unavailable() {
        let missing = SeatbeltBackend::with_helper(
            settings(),
            Arc::new(SystemHost::new()),
            PathBuf::from("/does/not/exist/litterbox-seatbelt-helper"),
        );
        let readiness = missing.readiness().await.expect("readiness");
        assert!(!readiness.is_available());
        assert!(readiness.checks.iter().any(|check| {
            check.primitive == "the Seatbelt helper"
                && check.state == crate::sandbox::report::State::Unavailable
        }));

        let directory = tempfile::tempdir().expect("tempdir");
        let untrusted = directory.path().join("helper");
        std::fs::copy(helper(), &untrusted).expect("copy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&untrusted, std::fs::Permissions::from_mode(0o777))
                .expect("mode");
        }
        let backend =
            SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), untrusted);
        let readiness = backend.readiness().await.expect("readiness");
        assert!(readiness.checks.iter().any(|check| {
            check.primitive == "the Seatbelt helper"
                && check.state == crate::sandbox::report::State::Unavailable
        }));
    }

    #[tokio::test]
    async fn a_failing_probe_is_unavailable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let probe = directory.path().join("helper");
        std::fs::write(&probe, "#!/bin/sh\nexit 1\n").expect("script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), probe);
        let readiness = backend.readiness().await.expect("readiness");
        assert!(readiness.checks.iter().any(|check| {
            check.primitive == "the Seatbelt probe"
                && check.state == crate::sandbox::report::State::Unavailable
        }));
    }

    #[tokio::test]
    async fn create_refuses_ports_duplicates_and_bad_tags() {
        let helper = helper();
        assert!(helper.exists(), "build the helper before this test");
        let source = tempfile::tempdir().expect("source");
        let project = format!("test-refusals-{}", std::process::id());
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project)
            .join("one");
        let _ = std::fs::remove_dir_all(
            agentkit_common::data_dir("litterbox")
                .join("seatbelt")
                .join(&project),
        );
        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), helper);

        let ports = CreateRequest {
            name: "one".to_string(),
            project_slug: project.clone(),
            workspace: Workspace::copied(source.path().to_path_buf()),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: vec![crate::sandbox::contract::RequestedPort {
                name: "http".to_string(),
                target: 8080,
            }],
            mode: CreateMode::Fresh,
        };
        assert!(matches!(
            backend.create(&ports).await,
            Err(Error::Unsupported { .. })
        ));
        assert!(!state.exists());

        let bad_tag = CreateRequest {
            name: "with space".to_string(),
            project_slug: project.clone(),
            workspace: Workspace::copied(source.path().to_path_buf()),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        };
        assert!(matches!(
            backend.create(&bad_tag).await,
            Err(Error::Misconfigured { .. })
        ));

        let request = CreateRequest {
            name: "one".to_string(),
            project_slug: project.clone(),
            workspace: Workspace::copied(source.path().to_path_buf()),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        };
        let record = backend.create(&request).await.expect("create");
        assert!(matches!(
            backend.create(&request).await,
            Err(Error::Conflict { .. })
        ));
        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(
            agentkit_common::data_dir("litterbox")
                .join("seatbelt")
                .join(&project),
        );
    }

    #[tokio::test]
    async fn create_exec_and_cleanup() {
        let helper = helper();
        assert!(helper.exists(), "build the helper before this test");
        let source = tempfile::tempdir().expect("source");
        std::fs::write(source.path().join("README"), b"hello").expect("file");
        let project = format!("test-project-{}", std::process::id());
        let name = "one";
        let state = agentkit_common::data_dir("litterbox")
            .join("seatbelt")
            .join(&project)
            .join(name);
        let _ = std::fs::remove_dir_all(&state);

        let backend = SeatbeltBackend::with_helper(settings(), Arc::new(SystemHost::new()), helper);
        let request = CreateRequest {
            name: name.to_string(),
            project_slug: project.clone(),
            workspace: Workspace::copied(source.path().to_path_buf()),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
            mode: CreateMode::Fresh,
        };
        let record = backend.create(&request).await.expect("create");
        assert_eq!(record.backend, "seatbelt");

        let execution = backend
            .exec(
                &record,
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo hi".to_string(),
                ],
            )
            .await
            .expect("exec");
        assert_eq!(execution.stdout, "hi\n");
        assert_eq!(execution.exit_code, 0);

        backend.delete(&record).await.expect("delete");
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(
            agentkit_common::data_dir("litterbox")
                .join("seatbelt")
                .join(&project),
        );
    }
}
