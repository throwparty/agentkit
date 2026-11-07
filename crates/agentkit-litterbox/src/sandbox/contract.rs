//! The platform- and mechanism-neutral sandbox contract.
//!
//! Nothing here names a container, an image, a namespace or a backend. A
//! backend is a mechanism that implements this trait; the CLI and the MCP
//! server hold a boxed trait object and do not know which one they hold.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use futures_util::future::BoxFuture;

use crate::sandbox::environment::{Environment, Origin, Provenance};

/// A sandbox as it is remembered between processes.
///
/// The record carries only the identity every surface needs and a payload the
/// backend that created it understands. A backend's own identifier lives in
/// `data`, never in the record's identity.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Workspace {
    /// Host directory holding the tree to materialise.
    pub source: PathBuf,
}

/// A port a project expects to have forwarded.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct RequestedPort {
    /// Stable name for the port, used to derive the environment variable.
    pub name: String,
    /// Port inside the sandbox.
    pub target: u16,
}

/// The environment variable that carries a forwarded port's host port.
///
/// This is the one place the name rule lives: configuration validation and
/// every backend use it, so a name that loads is a name that can be forwarded.
pub fn forwarded_port_env_var(name: &str) -> Result<String, String> {
    let slug = name.trim().to_ascii_lowercase().replace(' ', "-");
    let valid = !slug.is_empty()
        && slug.len() <= 63
        && slug
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
        && slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !valid {
        let name: String = name.chars().flat_map(char::escape_default).collect();
        return Err(format!(
            "'{name}' is not a valid port name; use lowercase letters, digits and hyphens"
        ));
    }
    Ok(format!(
        "LITTERBOX_FWD_PORT_{}",
        slug.replace('-', "_").to_ascii_uppercase()
    ))
}

/// Where a provisioning step runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    /// On the host, before the sandbox exists.
    Host,
    /// Inside the sandbox, once it exists.
    Sandbox,
}

/// A sort of contribution a host step produces.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ContributionSort {
    /// Environment variables a command receives.
    Environment,
    /// Read-only paths mounted into the sandbox.
    Mount,
}

impl ContributionSort {
    /// The stable name a surface reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Mount => "mount",
        }
    }
}

/// What a backend says about one contribution sort.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ContributionSupport {
    /// The contribution sort.
    pub sort: ContributionSort,
    /// Whether the backend can apply it.
    pub state: crate::sandbox::report::State,
    /// Why it is in this state.
    pub reason: Option<String>,
    /// What the user can do about it.
    pub remedy: Option<String>,
    /// The weaker mechanism the backend substitutes when degraded.
    pub fallback: Option<crate::sandbox::report::Fallback>,
}

/// A provisioning step: the project's declaration and the resolved value.
///
/// The kind is a name, not a closed enum. The registry in `provision::kind`
/// owns the settings each kind accepts; the contract carries them opaquely so
/// a backend never needs to know a kind.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProvisionStep {
    /// The step's stable identity.
    pub identity: String,
    /// The kind name.
    pub kind: String,
    /// The settings that kind owns, keyed by name.
    #[serde(flatten)]
    pub settings: BTreeMap<String, toml::Value>,
}

impl ProvisionStep {
    /// The step's identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The step's kind name.
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// A command a sandbox step runs inside the sandbox.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxAction {
    /// The shell command to run.
    pub run: String,
}

/// What a host step contributes to the sandbox.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Contributions {
    /// Variable names and values the sandbox should receive.
    pub environment: BTreeMap<String, String>,
    /// Where the contributions came from and what changed.
    pub provenance: Provenance,
    /// Absolute read-only paths to mount.
    pub mounts: Vec<PathBuf>,
    /// The host path the recorded workspace variables name.
    pub workspace_root: Option<PathBuf>,
    /// The variables whose values name the workspace root.
    pub workspace_variables: Vec<String>,
}

impl Default for Contributions {
    fn default() -> Self {
        Self {
            environment: BTreeMap::new(),
            provenance: Provenance {
                origin: Origin::Default,
                variables: Vec::new(),
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
            mounts: Vec::new(),
            workspace_root: None,
            workspace_variables: Vec::new(),
        }
    }
}

impl Contributions {
    /// Build the runtime environment for a workspace path.
    ///
    /// Only the recorded workspace variables are rewritten; store paths are
    /// left alone, because the store is mounted at its own path.
    pub fn environment_for(&self, workspace: &str) -> Environment {
        let mut variables = self.environment.clone();
        if let Some(root) = &self.workspace_root {
            let root = root.to_string_lossy();
            for name in &self.workspace_variables {
                if let Some(value) = variables.get_mut(name) {
                    *value = rebase_workspace(value, &root, workspace);
                }
            }
        }
        Environment {
            variables,
            provenance: self.provenance.clone(),
            workspace: workspace.to_string(),
        }
    }
}

/// Rewrite the workspace root where it names a whole path, leaving sibling paths
/// such as `/repo-cache` and store paths that merely contain the text alone.
fn rebase_workspace(value: &str, root: &str, workspace: &str) -> String {
    if root.is_empty() {
        return value.to_string();
    }
    let mut rebased = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(offset) = rest.find(root) {
        let after = offset + root.len();
        let starts_a_path = rest[..offset]
            .chars()
            .next_back()
            .is_none_or(|character| !continues_a_path(character));
        let ends_a_path = rest[after..]
            .chars()
            .next()
            .is_none_or(|character| character == '/' || !continues_a_path(character));
        rebased.push_str(&rest[..offset]);
        if starts_a_path && ends_a_path {
            rebased.push_str(workspace);
        } else {
            rebased.push_str(root);
        }
        rest = &rest[after..];
    }
    rebased.push_str(rest);
    rebased
}

/// Whether a character continues a path or a path component.
fn continues_a_path(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '/')
}

/// A step resolved against a backend: what to run and what to apply.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedStep {
    /// The step's stable identity.
    pub identity: String,
    /// The kind name, for attribution.
    pub kind: String,
    /// Where the step runs.
    pub placement: Placement,
    /// The command to run inside the sandbox, when the step runs there.
    pub sandbox: Option<SandboxAction>,
    /// What the step contributes, when it runs on the host.
    pub contributions: Contributions,
}

impl ResolvedStep {
    /// The step's identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The step's kind name.
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// Everything a backend needs to create a sandbox.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CreateRequest {
    /// The sandbox name.
    pub name: String,
    /// The project the sandbox belongs to.
    pub project_slug: String,
    /// The prepared workspace.
    pub workspace: Workspace,
    /// The ordered provisioning steps for this sandbox.
    pub provision: Vec<ResolvedStep>,
    /// The host address forwarded ports bind to, from user configuration.
    pub forward_bind: String,
    /// Ports the project expects to be forwarded.
    pub ports: Vec<RequestedPort>,
}

/// Which sandboxes an enumeration covers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
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

/// The class of a provisioning failure, independent of the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvisioningClass {
    /// The project's configuration is unusable.
    Misconfigured,
    /// The backend cannot apply this kind.
    Unsupported,
    /// Consent was refused.
    Denied,
    /// The host cannot apply this kind right now.
    Unavailable,
    /// The step ran and failed.
    Failed,
}

impl ProvisioningClass {
    /// The stable name a surface reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Misconfigured => "misconfigured",
            Self::Unsupported => "unsupported",
            Self::Denied => "denied",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

/// A failure that names the provisioning step responsible.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProvisioningFailure {
    /// The failure's class.
    pub class: ProvisioningClass,
    /// The backend that was asked, when one was.
    pub backend: Option<String>,
    /// The step identity, when one is known.
    pub step: Option<String>,
    /// The step kind, when one is known.
    pub kind: Option<String>,
    /// Why the step could not run.
    pub reason: String,
    /// What the user can do about it.
    pub remedy: String,
    /// The exit status, when a command ran.
    pub exit_code: Option<i32>,
    /// A bounded excerpt of the command's output, when it ran.
    pub output: Option<String>,
}

impl ProvisioningFailure {
    /// A failure with no exit status or output.
    pub fn new(
        class: ProvisioningClass,
        backend: Option<String>,
        step: Option<String>,
        kind: Option<String>,
        reason: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            class,
            backend,
            step,
            kind,
            reason: reason.into(),
            remedy: remedy.into(),
            exit_code: None,
            output: None,
        }
    }
}

/// Render a provisioning failure so a step, remedy and exit status are named
/// even in a bare message.
fn provisioning_message(failure: &ProvisioningFailure) -> String {
    let reason: String = failure
        .reason
        .chars()
        .flat_map(char::escape_default)
        .collect();
    let remedy: String = failure
        .remedy
        .chars()
        .flat_map(char::escape_default)
        .collect();
    let mut message = String::new();
    if let Some(backend) = &failure.backend {
        message.push_str(&format!("{backend}: "));
    }
    match (&failure.step, &failure.kind) {
        (Some(step), Some(kind)) => {
            message.push_str(&format!("provisioning step '{step}' ({kind}): {reason}"));
        }
        (Some(step), None) => {
            message.push_str(&format!("provisioning step '{step}': {reason}"));
        }
        _ => message.push_str(&reason),
    }
    if !remedy.is_empty() {
        message.push_str(&format!(" -- {remedy}"));
    }
    if let Some(exit_code) = failure.exit_code {
        message.push_str(&format!(" (exit code {exit_code})"));
    }
    if let Some(output) = &failure.output {
        message.push('\n');
        message.extend(output.chars().flat_map(char::escape_default));
    }
    message
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
    #[error("{backend} does not support {operation} (needs {primitive}): {reason}. {remedy}")]
    Unsupported {
        /// The operation that was requested.
        operation: String,
        /// The backend that cannot perform it.
        backend: String,
        /// The primitive the mechanism lacks.
        primitive: String,
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
    /// The mechanism this host needs is not available.
    #[error("{backend} is not available here: {reason}. {remedy}")]
    Unavailable {
        /// The backend that cannot run.
        backend: String,
        /// What is missing.
        reason: String,
        /// What the user can do.
        remedy: String,
    },
    /// The configuration is unusable.
    #[error("configuration: {message}")]
    Misconfigured {
        /// What is wrong, naming the key or file.
        message: String,
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
    /// A provisioning step failed, with the step named.
    #[error("{}", provisioning_message(failure))]
    Provisioning {
        /// The structured failure.
        failure: Box<ProvisioningFailure>,
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
pub trait Backend: Send + Sync {
    /// Create a sandbox and return its record.
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>>;
    /// Enumerate sandboxes in the given scope.
    fn list<'a>(&'a self, scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>>;
    /// Run one command.
    fn exec<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>>;

    /// Copy a host path into a directory inside the sandbox.
    fn upload<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>>;

    /// Copy a sandbox path out to the host.
    fn download<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<(), Error>>;

    /// Describe a sandbox without changing it.
    fn inspect<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>>;
    /// Remove a sandbox and everything the backend created for it.
    fn delete<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>>;
    /// Report whether this backend can run here, and why not when it cannot.
    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<crate::sandbox::report::Readiness, Error>>;
    /// Report which optional operations this backend provides.
    fn capabilities(&self)
    -> BoxFuture<'_, Result<Vec<crate::sandbox::report::Capability>, Error>>;
    /// Report which contribution sorts this backend can apply, and why not.
    ///
    /// Support is a static statement about the mechanism; host-dependent
    /// checks belong in [`Backend::readiness`], which the orchestrator calls
    /// before create.
    fn contribution_support(&self) -> Vec<ContributionSupport>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::future::BoxFuture;

    use super::{
        Backend, CreateRequest, Error, Execution, Inspection, ProvisionStep, SandboxRecord, Scope,
    };

    struct Stub;

    impl Backend for Stub {
        fn create<'a>(
            &'a self,
            _request: &'a CreateRequest,
        ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
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
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn upload<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
            _source: &'a std::path::Path,
            _destination: &'a str,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn download<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
            _source: &'a str,
            _destination: &'a std::path::Path,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn inspect<'a>(
            &'a self,
            _sandbox: &'a SandboxRecord,
        ) -> BoxFuture<'a, Result<Inspection, Error>> {
            Box::pin(async { Err(Error::failed("stub")) })
        }

        fn delete<'a>(&'a self, _sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn readiness<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<crate::sandbox::report::Readiness, Error>> {
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

        fn contribution_support(&self) -> Vec<super::ContributionSupport> {
            Vec::new()
        }
    }

    #[test]
    fn an_inspection_carries_no_container_or_environment_value() {
        let inspection = Inspection {
            name: "demo".to_string(),
            backend: "docker".to_string(),
            status: super::SandboxStatus::Active,
            egress: crate::sandbox::report::EgressPosture::Direct,
            workspace: "/src".to_string(),
            environment: crate::sandbox::environment::Provenance {
                origin: crate::sandbox::environment::Origin::Backend {
                    from: "the Docker image".to_string(),
                },
                variables: vec![crate::sandbox::environment::Variable {
                    name: "TOKEN".to_string(),
                    change: crate::sandbox::environment::Change::Unchanged,
                }],
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
            forwards: Vec::new(),
        };

        let json = serde_json::to_string(&inspection).expect("serialises");
        assert!(!json.contains("container"), "{json}");
        assert!(!json.contains("SECRET-VALUE"), "{json}");
        assert!(json.contains("TOKEN"), "{json}");
    }

    #[test]
    fn a_provisioning_display_names_the_backend_step_remedy_and_escaped_output() {
        let error = Error::Provisioning {
            failure: Box::new(super::ProvisioningFailure {
                class: super::ProvisioningClass::Failed,
                backend: Some("docker".to_string()),
                step: Some("fetch".to_string()),
                kind: Some("command".to_string()),
                reason: "it failed".to_string(),
                remedy: "fix the step".to_string(),
                exit_code: Some(7),
                output: Some("raw\u{1b}[2J".to_string()),
            }),
        };
        let rendered = error.to_string();
        assert!(rendered.contains("docker"), "{rendered}");
        assert!(rendered.contains("fetch"), "{rendered}");
        assert!(rendered.contains("command"), "{rendered}");
        assert!(rendered.contains("fix the step"), "{rendered}");
        assert!(rendered.contains("exit code 7"), "{rendered}");
        assert!(!rendered.contains('\u{1b}'), "{rendered}");
    }

    #[test]
    fn a_provision_step_keeps_its_settings() {
        let step: ProvisionStep =
            toml::from_str("identity = 'dev'\nkind = 'devshell'\ninstallable = './nix'\n")
                .expect("a provision step parses");
        assert_eq!(step.identity(), "dev");
        assert_eq!(step.kind(), "devshell");
        assert_eq!(
            step.settings
                .get("installable")
                .and_then(toml::Value::as_str),
            Some("./nix")
        );
        let rendered = toml::to_string(&step).expect("a provision step serialises");
        assert!(rendered.contains("installable"), "{rendered}");
    }

    #[tokio::test]
    async fn the_contract_is_object_safe() {
        let sandbox: Arc<dyn Backend> = Arc::new(Stub);
        assert!(sandbox.list(&Scope::Machine).await.is_ok());
        let record = SandboxRecord {
            name: "demo".to_string(),
            backend: "stub".to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::Table(Default::default()),
        };
        assert_eq!(record.name, "demo");
    }
}
