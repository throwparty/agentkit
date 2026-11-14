//! The Seatbelt backend.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;

use super::BACKEND;
use super::config::{NetworkSetting, Settings};
use super::holder::Holder;
use super::host::SharedHost;
use super::mounts;
use super::payload::PayloadV1;
use super::policy;
use super::protocol::{ConfigPayload, WirePosture};
use super::workspace;
use crate::backends::shared;
use crate::sandbox::contract::{
    Backend, ContributionSort, ContributionSupport, CreateRequest, Error, Execution, Inspection,
    SandboxRecord, Scope,
};
use crate::sandbox::report::{Capability, Readiness, State};

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
        fs::create_dir_all(&state).map_err(|error| {
            Error::failed(format!(
                "cannot create the state directory '{}': {error}",
                state.display()
            ))
        })?;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).map_err(|error| {
            Error::failed(format!("cannot protect the state directory: {error}"))
        })?;

        let workspace = state.join("workspace");
        if let Err(error) = workspace::copy_from(&request.workspace.source, &workspace) {
            let _ = fs::remove_dir_all(&state);
            return Err(error);
        }

        let context = mounts::Context {
            workspace: &workspace,
            state_dir: &state,
            metadata_dir: &self.metadata,
            credential_denies: &self.settings.credential_denies,
        };
        let mounts = match mounts::from_steps(&request.provision, &context, self.host.as_ref()) {
            Ok(mounts) => mounts,
            Err(error) => {
                let _ = fs::remove_dir_all(&state);
                return Err(error);
            }
        };
        let environment =
            match super::environment::merge(&request.provision, &workspace, &mounts.reads) {
                Ok(environment) => environment,
                Err(error) => {
                    let _ = fs::remove_dir_all(&state);
                    return Err(error);
                }
            };
        let real_home = self
            .host
            .real_home()
            .map_err(|error| Error::failed(format!("cannot read the real home: {error}")))?;

        let baseline_reads: Vec<PathBuf> = self
            .settings
            .baseline_reads
            .iter()
            .filter(|path| self.host.realpath(path).is_ok())
            .cloned()
            .collect();

        let config = ConfigPayload {
            workspace: path_bytes(&workspace),
            reads: mounts.reads.iter().map(|path| path_bytes(path)).collect(),
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
            credential_denies: self
                .settings
                .credential_denies
                .iter()
                .map(|path| path_bytes(path))
                .collect(),
            real_home: path_bytes(&real_home),
        };

        let socket = socket_path()?;
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

        let payload = PayloadV1 {
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
            paused: false,
        };

        Ok(SandboxRecord {
            name: request.name.clone(),
            backend: BACKEND.to_string(),
            project_slug: request.project_slug.clone(),
            data: payload.to_value()?,
        })
    }

    fn holder_for(&self, record: &SandboxRecord) -> Result<Holder, Error> {
        let payload = PayloadV1::from_record(record)?;
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

impl Backend for SeatbeltBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move { self.create_sandbox(request) })
    }

    fn list<'a>(&'a self, _scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn exec<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(async move { self.holder_for(sandbox)?.exec(command) })
    }

    fn upload<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a Path,
        _destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async {
            Err(Error::Unsupported {
                operation: "upload".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a materialised record".to_string(),
                reason: "upload is not wired yet".to_string(),
                remedy: "wait for the transfer task".to_string(),
            })
        })
    }

    fn download<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
        _source: &'a str,
        _destination: &'a Path,
        _selector: Option<std::sync::Arc<dyn crate::sandbox::contract::SnapshotSelector>>,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async {
            Err(Error::Unsupported {
                operation: "download".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a materialised record".to_string(),
                reason: "download is not wired yet".to_string(),
                remedy: "wait for the transfer task".to_string(),
            })
        })
    }

    fn inspect<'a>(
        &'a self,
        _sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async {
            Err(Error::Unsupported {
                operation: "inspect".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a materialised record".to_string(),
                reason: "inspect is not wired yet".to_string(),
                remedy: "wait for the inspection task".to_string(),
            })
        })
    }

    fn delete<'a>(&'a self, _sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async {
            Err(Error::Unsupported {
                operation: "delete".to_string(),
                backend: BACKEND.to_string(),
                primitive: "a materialised record".to_string(),
                reason: "delete is not wired yet".to_string(),
                remedy: "wait for the lifecycle task".to_string(),
            })
        })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async {
            Ok(Readiness {
                backend: BACKEND.to_string(),
                checks: Vec::new(),
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
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::SeatbeltBackend;
    use crate::backends::seatbelt::config::{BaselineSetting, NetworkSetting, Settings};
    use crate::backends::seatbelt::host::SystemHost;
    use crate::sandbox::contract::{Backend, CreateRequest, Workspace};

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
            baseline_reads: vec![
                PathBuf::from("/System/Library"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/usr/lib"),
                PathBuf::from("/bin"),
                PathBuf::from("/private/etc"),
                PathBuf::from("/dev"),
            ],
            mach_services: Vec::new(),
            credential_denies: Vec::new(),
        }
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
            workspace: Workspace {
                source: source.path().to_path_buf(),
            },
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
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

        let _ = std::fs::remove_dir_all(&state);
    }
}
