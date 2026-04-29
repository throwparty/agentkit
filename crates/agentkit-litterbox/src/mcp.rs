use glob::{MatchOptions, Pattern};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tempfile;

#[cfg(test)]
use glob::glob as glob_paths;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::io;
#[cfg(test)]
use std::path::PathBuf;

use crate::compute::{ContainerInspection, DockerCompute};
use crate::config_loader;
use crate::domain::{
    ComputeError, ExecutionResult, ForwardedPort, ForwardedPortMapping, SandboxConfig,
    SandboxError, SandboxMetadata, SandboxStatus, slugify_name,
};
use crate::sandbox::{
    DockerSandboxProvider, SandboxProvider, branch_name_for_slug, container_name_for_slug,
};
use crate::scm::{Scm, ThreadSafeScm};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SandboxCreateArgs {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub sandbox: String,
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteArgs {
    pub sandbox: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PatchArgs {
    pub sandbox: String,
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BashArgs {
    pub sandbox: String,
    pub command: String,
    pub workdir: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LsArgs {
    pub sandbox: String,
    pub path: String,
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GlobArgs {
    pub sandbox: String,
    pub pattern: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GrepArgs {
    pub sandbox: String,
    pub pattern: String,
    pub path: String,
    pub include: Option<String>,
}

#[derive(Clone)]
pub struct SandboxServer {
    tool_router: ToolRouter<Self>,
}

impl Default for SandboxServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl SandboxServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "sandbox-create",
        description = "Create a new sandbox based on the current repository HEAD"
    )]
    async fn sandbox_create(
        &self,
        Parameters(args): Parameters<SandboxCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let config = config_loader::load_final()
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let image =
            config.docker.image.clone().ok_or_else(|| {
                McpError::internal_error("missing docker.image".to_string(), None)
            })?;
        let forwarded_ports = config
            .ports
            .ports
            .iter()
            .map(|port| ForwardedPort {
                name: port.name.clone(),
                target: port.target,
            })
            .collect();
        let provider = build_provider_with_config(&config).map_err(map_error)?;
        let sandbox_config = SandboxConfig {
            image,
            setup_command: config.docker.setup_command.clone(),
            forwarded_ports,
        };
        let metadata = provider
            .create(&args.name, &sandbox_config)
            .await
            .map_err(map_error)?;
        let content = Content::json(metadata)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        name = "sandbox-ports",
        description = "Get forwarded ports for a sandbox"
    )]
    async fn sandbox_ports(
        &self,
        Parameters(args): Parameters<SandboxPortsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let inspection = provider
            .inspect_container(&metadata.container_id)
            .await
            .map_err(|error| map_sandbox_error(&args.sandbox, error))?;
        let forwarded_ports = forwarded_ports_from_inspection(&inspection);
        let response = SandboxPortsResponse {
            name: args.sandbox,
            forwarded_ports,
        };
        let content = Content::json(response)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "read", description = "Read a file from the sandbox")]
    async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let content = read_in_sandbox(&provider, &metadata, &args.path, args.offset, args.limit)
            .await
            .map_err(|error| map_read_error(&args.sandbox, error))?;
        let content = Content::text(content);
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "write", description = "Write a file into the sandbox")]
    async fn write(
        &self,
        Parameters(args): Parameters<WriteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        write_in_sandbox(&provider, &metadata, &args.path, &args.content)
            .await
            .map_err(|error| map_write_error(&args.sandbox, error))?;
        snapshot_after(
            &provider,
            &metadata,
            &args.sandbox,
            SnapshotTrigger::Write { path: args.path },
        )
        .await
        .map_err(map_error)?;
        Ok(CallToolResult::success(Vec::new()))
    }

    #[tool(
        name = "patch",
        description = "Apply a unified diff inside the sandbox"
    )]
    async fn patch(
        &self,
        Parameters(args): Parameters<PatchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        patch_in_sandbox(&provider, &metadata, &args.path, &args.diff)
            .await
            .map_err(|error| map_patch_error(&args.sandbox, error))?;
        snapshot_after(
            &provider,
            &metadata,
            &args.sandbox,
            SnapshotTrigger::Patch { path: args.path },
        )
        .await
        .map_err(map_error)?;
        Ok(CallToolResult::success(Vec::new()))
    }

    #[tool(
        name = "bash",
        description = "Execute a shell command inside the sandbox"
    )]
    async fn bash(
        &self,
        Parameters(args): Parameters<BashArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let result = bash_in_sandbox(
            &provider,
            &metadata,
            &args.command,
            args.workdir.as_deref(),
            args.timeout,
        )
        .await
        .map_err(|error| map_bash_error(&args.sandbox, error))?;
        snapshot_after(
            &provider,
            &metadata,
            &args.sandbox,
            SnapshotTrigger::Bash {
                command: args.command.clone(),
            },
        )
        .await
        .map_err(map_error)?;
        let content = Content::json(result)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "ls", description = "List directory entries")]
    async fn ls(&self, Parameters(args): Parameters<LsArgs>) -> Result<CallToolResult, McpError> {
        let recursive = args.recursive.unwrap_or(false);
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let entries = ls_in_sandbox(&provider, &metadata, &args.path, recursive)
            .await
            .map_err(|error| map_ls_error(&args.sandbox, error))?;
        let content = Content::json(entries)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "glob", description = "Find files matching a glob pattern")]
    async fn glob(
        &self,
        Parameters(args): Parameters<GlobArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let matches = glob_in_sandbox(&provider, &metadata, &args.pattern, args.path.as_deref())
            .await
            .map_err(|error| map_glob_tool_error(&args.sandbox, error))?;
        let content = Content::json(matches)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "grep", description = "Search file contents for a pattern")]
    async fn grep(
        &self,
        Parameters(args): Parameters<GrepArgs>,
    ) -> Result<CallToolResult, McpError> {
        let provider = build_provider().map_err(map_error)?;
        let metadata = resolve_sandbox_metadata(&args.sandbox).map_err(map_error)?;
        let matches = grep_in_sandbox(
            &provider,
            &metadata,
            &args.pattern,
            &args.path,
            args.include.as_deref(),
        )
        .await
        .map_err(|error| map_grep_error(&args.sandbox, error))?;
        let content = Content::json(matches)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SandboxServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Litterbox sandbox management".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy)]
struct ToolDoc {
    name: &'static str,
    description: &'static str,
    params: &'static [ParamDoc],
}

#[derive(Clone, Copy)]
struct ParamDoc {
    name: &'static str,
    type_name: &'static str,
    required: bool,
    description: &'static str,
}

const SANDBOX_NAME_PARAM: ParamDoc = ParamDoc {
    name: "sandbox",
    type_name: "string",
    required: true,
    description: "Sandbox name.",
};

const PATH_PARAM: ParamDoc = ParamDoc {
    name: "path",
    type_name: "string",
    required: true,
    description: "Path inside the sandbox.",
};

const TOOL_DOCS: &[ToolDoc] = &[
    ToolDoc {
        name: "sandbox-create",
        description: "Create a new sandbox based on the current repository HEAD.",
        params: &[ParamDoc {
            name: "name",
            type_name: "string",
            required: true,
            description: "Sandbox name.",
        }],
    },
    ToolDoc {
        name: "sandbox-ports",
        description: "Get forwarded ports for a sandbox.",
        params: &[SANDBOX_NAME_PARAM],
    },
    ToolDoc {
        name: "read",
        description: "Read a file from the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "offset",
                type_name: "integer",
                required: false,
                description: "Line offset (0-based).",
            },
            ParamDoc {
                name: "limit",
                type_name: "integer",
                required: false,
                description: "Maximum number of lines to read.",
            },
        ],
    },
    ToolDoc {
        name: "write",
        description: "Write a file into the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "content",
                type_name: "string",
                required: true,
                description: "Full file contents to write.",
            },
        ],
    },
    ToolDoc {
        name: "patch",
        description: "Apply a unified diff inside the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "diff",
                type_name: "string",
                required: true,
                description: "Unified diff to apply.",
            },
        ],
    },
    ToolDoc {
        name: "bash",
        description: "Execute a shell command inside the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "command",
                type_name: "string",
                required: true,
                description: "Shell command to execute.",
            },
            ParamDoc {
                name: "workdir",
                type_name: "string",
                required: false,
                description: "Working directory inside the sandbox.",
            },
            ParamDoc {
                name: "timeout",
                type_name: "integer",
                required: false,
                description: "Timeout in seconds.",
            },
        ],
    },
    ToolDoc {
        name: "ls",
        description: "List directory entries.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "recursive",
                type_name: "boolean",
                required: false,
                description: "Recursively list all descendants.",
            },
        ],
    },
    ToolDoc {
        name: "glob",
        description: "Find files matching a glob pattern.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "pattern",
                type_name: "string",
                required: true,
                description: "Glob pattern to match.",
            },
            ParamDoc {
                name: "path",
                type_name: "string",
                required: false,
                description: "Base path for matching.",
            },
        ],
    },
    ToolDoc {
        name: "grep",
        description: "Search file contents for a pattern.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "pattern",
                type_name: "string",
                required: true,
                description: "Pattern to search for.",
            },
            PATH_PARAM,
            ParamDoc {
                name: "include",
                type_name: "string",
                required: false,
                description: "Glob include filter (e.g., *.rs).",
            },
        ],
    },
];

pub fn generate_mcp_docs() -> String {
    let mut output = String::from("# 🤖 MCP tools\n\n");

    let mut tools = TOOL_DOCS.to_vec();
    tools.sort_by(|a, b| a.name.cmp(b.name));

    for tool in tools {
        output.push_str(&format!("## `{}`\n\n", tool.name));
        output.push_str(tool.description);
        output.push_str("\n\n");

        if tool.params.is_empty() {
            output.push_str("Parameters: none\n\n");
            continue;
        }

        output.push_str("Parameters:\n\n");
        for param in tool.params {
            let requirement = if param.required { "required" } else { "optional" };
            output.push_str(&format!(
                "- `{}` ({}, {}) {}\n",
                param.name, param.type_name, requirement, param.description
            ));
        }
        output.push('\n');
    }

    output
}

pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let service = SandboxServer::new().serve(stdio()).await.inspect_err(|e| {
        eprintln!("Error starting MCP server: {e}");
    })?;
    service.waiting().await?;
    Ok(())
}

fn build_provider() -> Result<DockerSandboxProvider<ThreadSafeScm, DockerCompute>, SandboxError> {
    let config = config_loader::load_final().map_err(|e| SandboxError::Config(e.to_string()))?;
    build_provider_with_config(&config)
}

fn build_provider_with_config(
    config: &crate::config::Config,
) -> Result<DockerSandboxProvider<ThreadSafeScm, DockerCompute>, SandboxError> {
    let scm =
        ThreadSafeScm::open_with_prefix(std::path::Path::new("."), config.project.slug.clone())?;
    let compute = DockerCompute::connect()?;
    Ok(DockerSandboxProvider::new(scm, compute))
}

fn map_error(error: SandboxError) -> McpError {
    match error {
        SandboxError::InvalidName { .. } => McpError::invalid_params(error.to_string(), None),
        SandboxError::SandboxExists { .. } => McpError::invalid_params(error.to_string(), None),
        SandboxError::SandboxNotFound { .. } => McpError::invalid_params(error.to_string(), None),
        _ => McpError::internal_error(error.to_string(), None),
    }
}

fn map_sandbox_error(name: &str, error: SandboxError) -> McpError {
    if is_container_missing(&error) {
        return McpError::invalid_params(format!("Sandbox '{}' not found.", name), None);
    }
    map_error(error)
}

fn resolve_sandbox_metadata(name: &str) -> Result<SandboxMetadata, SandboxError> {
    let slug = slugify_name(name)?;
    let config = config_loader::load_final().map_err(|e| SandboxError::Config(e.to_string()))?;
    let scm = ThreadSafeScm::open_with_prefix(Path::new("."), config.project.slug)?;
    let repo_prefix = scm.repo_prefix()?;
    Ok(SandboxMetadata {
        name: name.to_string(),
        branch_name: branch_name_for_slug(&slug),
        container_id: container_name_for_slug(&repo_prefix, &slug),
        status: SandboxStatus::Active,
        forwarded_ports: Vec::new(),
    })
}

fn is_container_missing(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::Compute(ComputeError::ContainerExec {
            source: bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }
        })
            | SandboxError::Compute(ComputeError::ContainerInspect {
                source: bollard::errors::Error::DockerResponseServerError {
                    status_code: 404,
                    ..
                }
            })
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SandboxPortsArgs {
    pub sandbox: String,
}

#[derive(Debug, Serialize)]
struct SandboxPortsResponse {
    pub name: String,
    pub forwarded_ports: Vec<ForwardedPortMapping>,
}

fn forwarded_ports_from_inspection(inspection: &ContainerInspection) -> Vec<ForwardedPortMapping> {
    let mut env_map: HashMap<u16, String> = HashMap::new();
    for entry in &inspection.env {
        if let Some((key, value)) = entry.split_once('=')
            && key.starts_with("LITTERBOX_FWD_PORT_")
            && let Ok(port) = value.parse::<u16>()
        {
            env_map.insert(port, key.to_string());
        }
    }

    let mut mappings = Vec::new();
    for (container_port, bindings) in &inspection.port_bindings {
        let target = container_port
            .split('/')
            .next()
            .and_then(|value| value.parse::<u16>().ok());
        let target = match target {
            Some(target) => target,
            None => continue,
        };

        for binding in bindings {
            let host_port = binding
                .host_port
                .as_ref()
                .and_then(|value| value.parse::<u16>().ok());
            let host_port = match host_port {
                Some(host_port) => host_port,
                None => continue,
            };

            let env_var = match env_map.get(&host_port) {
                Some(env) => env.clone(),
                None => continue,
            };
            let name = env_var
                .strip_prefix("LITTERBOX_FWD_PORT_")
                .unwrap_or("")
                .to_ascii_lowercase()
                .replace('_', "-");

            mappings.push(ForwardedPortMapping {
                name,
                target,
                host_port,
                env_var,
            });
        }
    }

    mappings
}

#[derive(Debug)]
enum LsError {
    Sandbox(SandboxError),
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
}

fn map_ls_error(sandbox: &str, error: LsError) -> McpError {
    match error {
        LsError::Sandbox(error) => map_sandbox_error(sandbox, error),
        LsError::NotFound { path } => {
            McpError::invalid_params(format!("path not found: {}", path), None)
        }
        LsError::PermissionDenied { path } => {
            McpError::invalid_params(format!("permission denied: {}", path), None)
        }
        LsError::Failed { path, message } => {
            McpError::internal_error(format!("failed to list {}: {}", path, message), None)
        }
    }
}

async fn ls_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    path: &str,
    recursive: bool,
) -> Result<Vec<String>, LsError> {
    let container_path = resolve_container_path(path);
    let command = if recursive {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("find {} -mindepth 1 -print", shell_escape(&container_path)),
        ]
    } else {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("ls -1A {}", shell_escape(&container_path)),
        ]
    };
    let result = exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(LsError::Sandbox)?;
    if result.exit_code != 0 {
        return Err(classify_ls_failure(&container_path, &result));
    }
    Ok(parse_ls_output(&result.stdout, &container_path, recursive))
}

fn classify_ls_failure(path: &str, result: &ExecutionResult) -> LsError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("No such file or directory") {
        LsError::NotFound {
            path: path.to_string(),
        }
    } else if message.contains("Permission denied") {
        LsError::PermissionDenied {
            path: path.to_string(),
        }
    } else if message.is_empty() {
        LsError::Failed {
            path: path.to_string(),
            message: format!("exit code {}", result.exit_code),
        }
    } else {
        LsError::Failed {
            path: path.to_string(),
            message: message.to_string(),
        }
    }
}

fn parse_ls_output(output: &str, base: &str, recursive: bool) -> Vec<String> {
    let mut entries: Vec<String> = output
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .map(|line| {
            if recursive {
                if let Some(stripped) = line.strip_prefix(base) {
                    stripped.strip_prefix('/').unwrap_or(stripped).to_string()
                } else {
                    line.to_string()
                }
            } else {
                line.to_string()
            }
        })
        .collect();
    entries.sort();
    entries
}

#[derive(Debug)]
enum ReadError {
    Sandbox(SandboxError),
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
}

#[derive(Debug)]
enum WriteError {
    Sandbox(SandboxError),
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
}

#[derive(Debug)]
enum PatchError {
    InvalidPatch {
        source: String,
    },
    ReadFile {
        path: String,
        source: Box<ReadError>,
    },
    WriteFile {
        path: String,
        source: Box<WriteError>,
    },
    ApplyFailed {
        path: String,
        source: String,
    },
}

#[derive(Debug)]
enum BashError {
    Sandbox(SandboxError),
}

#[derive(Debug, Clone)]
enum SnapshotTrigger {
    Write { path: String },
    Patch { path: String },
    Bash { command: String },
}

fn map_read_error(sandbox: &str, error: ReadError) -> McpError {
    match error {
        ReadError::Sandbox(error) => map_sandbox_error(sandbox, error),
        ReadError::NotFound { path } => {
            McpError::invalid_params(format!("file not found: {}", path), None)
        }
        ReadError::PermissionDenied { path } => {
            McpError::invalid_params(format!("permission denied: {}", path), None)
        }
        ReadError::Failed { path, message } => {
            McpError::internal_error(format!("failed to read {}: {}", path, message), None)
        }
    }
}

fn map_write_error(sandbox: &str, error: WriteError) -> McpError {
    match error {
        WriteError::Sandbox(error) => map_sandbox_error(sandbox, error),
        WriteError::NotFound { path } => {
            McpError::invalid_params(format!("path not found: {}", path), None)
        }
        WriteError::PermissionDenied { path } => {
            McpError::invalid_params(format!("permission denied: {}", path), None)
        }
        WriteError::Failed { path, message } => {
            McpError::internal_error(format!("failed to write {}: {}", path, message), None)
        }
    }
}

fn map_patch_error(_sandbox: &str, error: PatchError) -> McpError {
    match error {
        PatchError::InvalidPatch { source } => {
            McpError::invalid_params(format!("invalid patch: {}", source), None)
        }
        PatchError::ReadFile { path, source } => McpError::internal_error(
            format!("failed to read file {} for patching: {:?}", path, source),
            None,
        ),
        PatchError::WriteFile { path, source } => McpError::internal_error(
            format!("failed to write patched file {}: {:?}", path, source),
            None,
        ),
        PatchError::ApplyFailed { path, source } => McpError::internal_error(
            format!("failed to apply patch to {}: {}", path, source),
            None,
        ),
    }
}

fn map_bash_error(sandbox: &str, error: BashError) -> McpError {
    match error {
        BashError::Sandbox(error) => map_sandbox_error(sandbox, error),
    }
}

async fn snapshot_after<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    sandbox: &str,
    trigger: SnapshotTrigger,
) -> Result<(), SandboxError> {
    let config = config_loader::load_final().map_err(|e| SandboxError::Config(e.to_string()))?;
    let scm = ThreadSafeScm::for_sandbox(Path::new("."), config.project.slug.clone(), sandbox)?;

    // Download container /src to temp staging directory
    let staging_dir = tempfile::tempdir()
        .map_err(|e| SandboxError::Config(format!("Failed to create temp dir: {}", e)))?;
    provider
        .download_path(metadata, "/src", staging_dir.path())
        .await?;

    // Commit from staging directory to snapshot branch
    let _ = scm.commit_snapshot_from_staging(staging_dir.path(), &snapshot_message(&trigger))?;

    Ok(())
}

fn snapshot_message(trigger: &SnapshotTrigger) -> String {
    match trigger {
        SnapshotTrigger::Write { path } => format!("write: {}", path),
        SnapshotTrigger::Patch { path } => format!("patch: {}", path),
        SnapshotTrigger::Bash { command } => format!("bash: {}", command),
    }
}

#[allow(unused)]
fn snapshot_after_with_scm<S: Scm>(scm: &S, trigger: SnapshotTrigger) -> Result<(), SandboxError> {
    if !scm.has_changes()? {
        return Ok(());
    }
    scm.stage_all()?;
    scm.commit_snapshot(&snapshot_message(&trigger))?;
    Ok(())
}

async fn read_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ReadError> {
    let container_path = resolve_container_path(path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("cat -- {}", shell_escape(&container_path)),
    ];
    let result = exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(ReadError::Sandbox)?;
    if result.exit_code != 0 {
        return Err(classify_read_failure(&container_path, &result));
    }
    Ok(slice_content(&result.stdout, offset, limit))
}

async fn write_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    path: &str,
    content: &str,
) -> Result<(), WriteError> {
    let container_path = resolve_container_path(path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "printf %s {} > {}",
            shell_escape(content),
            shell_escape(&container_path)
        ),
    ];
    let result = exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(WriteError::Sandbox)?;
    if result.exit_code != 0 {
        return Err(classify_write_failure(&container_path, &result));
    }
    Ok(())
}

async fn patch_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    path: &str,
    diff: &str,
) -> Result<(), PatchError> {
    // Read current file content
    let original_content = read_in_sandbox(provider, metadata, path, None, None)
        .await
        .map_err(|e| PatchError::ReadFile {
            path: path.to_string(),
            source: Box::new(e),
        })?;

    // Parse and apply patch using diffy
    let patch = diffy::Patch::from_str(diff).map_err(|e| PatchError::InvalidPatch {
        source: e.to_string(),
    })?;

    let patched_content =
        diffy::apply(&original_content, &patch).map_err(|e| PatchError::ApplyFailed {
            path: path.to_string(),
            source: e.to_string(),
        })?;

    // Write patched content back
    write_in_sandbox(provider, metadata, path, &patched_content)
        .await
        .map_err(|e| PatchError::WriteFile {
            path: path.to_string(),
            source: Box::new(e),
        })?;

    Ok(())
}

async fn bash_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    command: &str,
    workdir: Option<&str>,
    timeout: Option<u64>,
) -> Result<ExecutionResult, BashError> {
    let command = build_bash_command(command, workdir, timeout);
    let command = vec!["sh".to_string(), "-c".to_string(), command];
    exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(BashError::Sandbox)
}

fn classify_read_failure(path: &str, result: &ExecutionResult) -> ReadError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("No such file or directory") {
        ReadError::NotFound {
            path: path.to_string(),
        }
    } else if message.contains("Permission denied") {
        ReadError::PermissionDenied {
            path: path.to_string(),
        }
    } else if message.is_empty() {
        ReadError::Failed {
            path: path.to_string(),
            message: format!("exit code {}", result.exit_code),
        }
    } else {
        ReadError::Failed {
            path: path.to_string(),
            message: message.to_string(),
        }
    }
}

fn classify_write_failure(path: &str, result: &ExecutionResult) -> WriteError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("No such file or directory") {
        WriteError::NotFound {
            path: path.to_string(),
        }
    } else if message.contains("Permission denied") {
        WriteError::PermissionDenied {
            path: path.to_string(),
        }
    } else if message.is_empty() {
        WriteError::Failed {
            path: path.to_string(),
            message: format!("exit code {}", result.exit_code),
        }
    } else {
        WriteError::Failed {
            path: path.to_string(),
            message: message.to_string(),
        }
    }
}

fn build_bash_command(command: &str, workdir: Option<&str>, timeout: Option<u64>) -> String {
    let command = if let Some(workdir) = workdir {
        let container_path = resolve_container_path(workdir);
        format!("cd {} && {}", shell_escape(&container_path), command)
    } else {
        command.to_string()
    };

    if let Some(timeout) = timeout {
        format!("timeout {}s sh -c {}", timeout, shell_escape(&command))
    } else {
        command
    }
}

async fn exec_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    command: Vec<String>,
) -> Result<ExecutionResult, SandboxError> {
    provider.shell(metadata, &command).await
}

fn resolve_container_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/src/{}", path)
    }
}

fn shell_escape(value: &str) -> String {
    let mut escaped = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            escaped.push_str("'\"'\"'");
        } else {
            escaped.push(ch);
        }
    }
    escaped.push('\'');
    escaped
}

#[cfg(test)]
fn read_file_content(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> io::Result<String> {
    let content = fs::read_to_string(path)?;
    Ok(slice_content(&content, offset, limit))
}

#[cfg(test)]
fn list_dir_entries(path: &Path, recursive: bool) -> io::Result<Vec<String>> {
    let mut entries = Vec::new();
    if recursive {
        visit_dir(path, path, &mut entries)?;
    } else {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            entries.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
fn glob_entries(pattern: &str, base: &Path) -> io::Result<Vec<String>> {
    let absolute_pattern = build_glob_pattern(pattern, base);
    let mut entries = Vec::new();
    let walker = glob_paths(&absolute_pattern)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    for entry in walker {
        let path = entry.map_err(io::Error::other)?;
        let display = if let Ok(relative) = path.strip_prefix(base) {
            relative.to_string_lossy().into_owned()
        } else {
            path.to_string_lossy().into_owned()
        };
        entries.push(display);
    }

    entries.sort();
    Ok(entries)
}

#[cfg(test)]
fn build_glob_pattern(pattern: &str, base: &Path) -> String {
    let candidate = Path::new(pattern);
    if candidate.is_absolute() {
        pattern.to_string()
    } else {
        base.join(candidate).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
fn visit_dir(base: &Path, current: &Path, entries: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);
        entries.push(relative.to_string_lossy().into_owned());
        if entry.file_type()?.is_dir() {
            visit_dir(base, &path, entries)?;
        }
    }
    Ok(())
}

fn slice_content(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let start = offset.unwrap_or(0);
    let max = limit.unwrap_or(usize::MAX);
    if max == 0 {
        return String::new();
    }

    let mut result = String::new();
    for (index, segment) in content.split_inclusive('\n').enumerate() {
        if index >= start {
            if index - start >= max {
                break;
            }
            result.push_str(segment);
        }
    }
    result
}

#[derive(Debug)]
enum GlobError {
    Sandbox(SandboxError),
    InvalidPattern { pattern: String, message: String },
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
}

#[derive(Debug)]
enum GrepError {
    Sandbox(SandboxError),
    InvalidPattern { pattern: String, message: String },
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
}

fn map_glob_tool_error(sandbox: &str, error: GlobError) -> McpError {
    match error {
        GlobError::Sandbox(error) => map_sandbox_error(sandbox, error),
        GlobError::InvalidPattern { pattern, message } => McpError::invalid_params(
            format!("invalid glob pattern '{}': {}", pattern, message),
            None,
        ),
        GlobError::NotFound { path } => {
            McpError::invalid_params(format!("path not found: {}", path), None)
        }
        GlobError::PermissionDenied { path } => {
            McpError::invalid_params(format!("permission denied: {}", path), None)
        }
        GlobError::Failed { path, message } => {
            McpError::internal_error(format!("glob failed for {}: {}", path, message), None)
        }
    }
}

async fn glob_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    pattern: &str,
    base_path: Option<&str>,
) -> Result<Vec<String>, GlobError> {
    let base = base_path
        .map(resolve_container_path)
        .unwrap_or_else(|| "/src".to_string());
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("find {} -mindepth 1 -print", shell_escape(&base)),
    ];
    let result = exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(GlobError::Sandbox)?;
    if result.exit_code != 0 {
        return Err(classify_glob_failure(&base, &result));
    }

    let pattern_is_absolute = Path::new(pattern).is_absolute();
    let pattern = Pattern::new(pattern).map_err(|error| GlobError::InvalidPattern {
        pattern: pattern.to_string(),
        message: error.to_string(),
    })?;
    let options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };

    let mut entries = Vec::new();
    for entry in parse_find_output(&result.stdout) {
        let relative = strip_base_prefix(&entry, &base);
        let candidate = if pattern_is_absolute {
            entry.as_str()
        } else {
            relative.as_str()
        };
        if pattern.matches_with(candidate, options) {
            let display = if pattern_is_absolute { entry } else { relative };
            entries.push(display);
        }
    }

    entries.sort();
    Ok(entries)
}

fn classify_glob_failure(base: &str, result: &ExecutionResult) -> GlobError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("No such file or directory") {
        GlobError::NotFound {
            path: base.to_string(),
        }
    } else if message.contains("Permission denied") {
        GlobError::PermissionDenied {
            path: base.to_string(),
        }
    } else if message.is_empty() {
        GlobError::Failed {
            path: base.to_string(),
            message: format!("exit code {}", result.exit_code),
        }
    } else {
        GlobError::Failed {
            path: base.to_string(),
            message: message.to_string(),
        }
    }
}

fn parse_find_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn strip_base_prefix(path: &str, base: &str) -> String {
    if let Some(stripped) = path.strip_prefix(base) {
        stripped.strip_prefix('/').unwrap_or(stripped).to_string()
    } else {
        path.to_string()
    }
}

fn map_grep_error(sandbox: &str, error: GrepError) -> McpError {
    match error {
        GrepError::Sandbox(error) => map_sandbox_error(sandbox, error),
        GrepError::InvalidPattern { pattern, message } => McpError::invalid_params(
            format!("invalid grep pattern '{}': {}", pattern, message),
            None,
        ),
        GrepError::NotFound { path } => {
            McpError::invalid_params(format!("path not found: {}", path), None)
        }
        GrepError::PermissionDenied { path } => {
            McpError::invalid_params(format!("permission denied: {}", path), None)
        }
        GrepError::Failed { path, message } => {
            McpError::internal_error(format!("grep failed for {}: {}", path, message), None)
        }
    }
}

async fn grep_in_sandbox<P: SandboxProvider>(
    provider: &P,
    metadata: &SandboxMetadata,
    pattern: &str,
    path: &str,
    include: Option<&str>,
) -> Result<Vec<String>, GrepError> {
    let container_path = resolve_container_path(path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        build_grep_command(pattern, &container_path, include),
    ];
    let result = exec_in_sandbox(provider, metadata, command)
        .await
        .map_err(GrepError::Sandbox)?;
    if result.exit_code == 0 {
        return Ok(parse_grep_output(&result.stdout));
    }
    if result.exit_code == 1 && result.stderr.trim().is_empty() {
        return Ok(Vec::new());
    }
    Err(classify_grep_failure(&container_path, pattern, &result))
}

fn build_grep_command(pattern: &str, path: &str, include: Option<&str>) -> String {
    let mut parts = vec!["grep".to_string(), "-R".to_string(), "-n".to_string()];
    if let Some(include) = include {
        parts.push(format!("--include={}", shell_escape(include)));
    }
    parts.push("--".to_string());
    parts.push(shell_escape(pattern));
    parts.push(shell_escape(path));
    parts.join(" ")
}

fn classify_grep_failure(path: &str, pattern: &str, result: &ExecutionResult) -> GrepError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("No such file or directory") {
        GrepError::NotFound {
            path: path.to_string(),
        }
    } else if message.contains("Permission denied") {
        GrepError::PermissionDenied {
            path: path.to_string(),
        }
    } else if message.contains("Unmatched") || message.contains("Invalid") {
        GrepError::InvalidPattern {
            pattern: pattern.to_string(),
            message: message.to_string(),
        }
    } else if message.is_empty() {
        GrepError::Failed {
            path: path.to_string(),
            message: format!("exit code {}", result.exit_code),
        }
    } else {
        GrepError::Failed {
            path: path.to_string(),
            message: message.to_string(),
        }
    }
}

fn parse_grep_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::{ContainerInspection, PortBindingSpec};
    use futures_util::future::BoxFuture;
    use git2::{ErrorCode, Oid, Repository, Signature};
    use std::fs;
    use std::io::Write;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    struct TestScm {
        has_changes: bool,
        committed_messages: Mutex<Vec<String>>,
    }

    impl TestScm {
        fn new(has_changes: bool) -> Self {
            Self {
                has_changes,
                committed_messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[test]
    fn forwarded_ports_from_inspection_builds_mapping() {
        let inspection = ContainerInspection {
            env: vec!["LITTERBOX_FWD_PORT_WEB=3001".to_string()],
            port_bindings: HashMap::from([(
                "8080/tcp".to_string(),
                vec![PortBindingSpec {
                    host_ip: Some("0.0.0.0".to_string()),
                    host_port: Some("3001".to_string()),
                }],
            )]),
        };

        let mappings = forwarded_ports_from_inspection(&inspection);

        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].name, "web");
        assert_eq!(mappings[0].target, 8080);
        assert_eq!(mappings[0].host_port, 3001);
        assert_eq!(mappings[0].env_var, "LITTERBOX_FWD_PORT_WEB");
    }

    impl Scm for TestScm {
        fn create_branch(&self, _slug: &str) -> Result<String, SandboxError> {
            Ok("branch".to_string())
        }

        fn delete_branch(&self, _slug: &str) -> Result<(), SandboxError> {
            Ok(())
        }

        fn make_archive(&self, _reference: &str) -> Result<Vec<u8>, SandboxError> {
            Ok(Vec::new())
        }

        fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError> {
            Ok(Vec::new())
        }

        fn repo_prefix(&self) -> Result<String, SandboxError> {
            Ok("repo".to_string())
        }

        fn has_changes(&self) -> Result<bool, SandboxError> {
            Ok(self.has_changes)
        }

        fn stage_all(&self) -> Result<(), SandboxError> {
            Ok(())
        }

        fn commit_snapshot(&self, message: &str) -> Result<Option<Oid>, SandboxError> {
            self.committed_messages
                .lock()
                .expect("commit lock")
                .push(message.to_string());
            Ok(Some(Oid::zero()))
        }

        fn apply_patch(&self, _diff: &str) -> Result<(), SandboxError> {
            Ok(())
        }
    }

    fn init_repo() -> (TempDir, Repository) {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = Repository::init(tempdir.path()).expect("init repo");
        fs::write(tempdir.path().join("README.md"), "initial").expect("write");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("README.md")).expect("add path");
        let tree_id = index.write_tree().expect("write tree");
        {
            let tree = repo.find_tree(tree_id).expect("tree");
            let signature = Signature::now("Test", "test@example.com").expect("signature");
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
        }
        (tempdir, repo)
    }

    struct TestProvider {
        shell_result: Mutex<Option<Result<ExecutionResult, SandboxError>>>,
        last_command: Arc<Mutex<Option<Vec<String>>>>,
    }

    impl TestProvider {
        fn new(
            result: Result<ExecutionResult, SandboxError>,
            last_command: Arc<Mutex<Option<Vec<String>>>>,
        ) -> Self {
            Self {
                shell_result: Mutex::new(Some(result)),
                last_command,
            }
        }
    }

    struct MultiResultProvider {
        results: Arc<Mutex<Vec<Result<ExecutionResult, SandboxError>>>>,
    }

    impl MultiResultProvider {
        fn new(results: Arc<Mutex<Vec<Result<ExecutionResult, SandboxError>>>>) -> Self {
            Self { results }
        }
    }

impl SandboxProvider for MultiResultProvider {
        fn create<'a>(
            &'a self,
            _name: &'a str,
            _config: &'a SandboxConfig,
        ) -> BoxFuture<'a, Result<SandboxMetadata, SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

    fn pause<'a>(&'a self, _container_id: &'a str) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            Err(SandboxError::SandboxNotFound {
                name: "unused".to_string(),
            })
        })
    }

    fn inspect_container<'a>(
        &'a self,
        _container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>> {
        Box::pin(async move {
            Err(SandboxError::SandboxNotFound {
                name: "unused".to_string(),
            })
        })
    }

        fn resume<'a>(&'a self, _container_id: &'a str) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn delete<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn shell<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            _command: &'a [String],
        ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>> {
            let results = Arc::clone(&self.results);
            Box::pin(async move {
                let mut results_lock = results.lock().expect("results lock");
                if results_lock.is_empty() {
                    return Err(SandboxError::SandboxNotFound {
                        name: "no more results".to_string(),
                    });
                }
                results_lock.remove(0)
            })
        }

        fn upload_path<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            _src_path: &'a Path,
            _dest_path: &'a str,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn download_path<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            _src_path: &'a str,
            _dest_path: &'a Path,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }
    }

impl SandboxProvider for TestProvider {
        fn create<'a>(
            &'a self,
            _name: &'a str,
            _config: &'a SandboxConfig,
        ) -> BoxFuture<'a, Result<SandboxMetadata, SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

    fn pause<'a>(&'a self, _container_id: &'a str) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            Err(SandboxError::SandboxNotFound {
                name: "unused".to_string(),
            })
        })
    }

    fn inspect_container<'a>(
        &'a self,
        _container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>> {
        Box::pin(async move {
            Err(SandboxError::SandboxNotFound {
                name: "unused".to_string(),
            })
        })
    }

        fn resume<'a>(&'a self, _container_id: &'a str) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn delete<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn shell<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            command: &'a [String],
        ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>> {
            let result = self
                .shell_result
                .lock()
                .expect("shell result lock")
                .take()
                .unwrap_or_else(|| {
                    Err(SandboxError::SandboxNotFound {
                        name: "unused".to_string(),
                    })
                });
            let last_command = Arc::clone(&self.last_command);
            let command = command.to_vec();
            Box::pin(async move {
                *last_command.lock().expect("command lock") = Some(command);
                result
            })
        }

        fn upload_path<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            _src_path: &'a Path,
            _dest_path: &'a str,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }

        fn download_path<'a>(
            &'a self,
            _metadata: &'a SandboxMetadata,
            _src_path: &'a str,
            _dest_path: &'a Path,
        ) -> BoxFuture<'a, Result<(), SandboxError>> {
            Box::pin(async move {
                Err(SandboxError::SandboxNotFound {
                    name: "unused".to_string(),
                })
            })
        }
    }

    fn stub_metadata() -> SandboxMetadata {
        SandboxMetadata {
            name: "sandbox".to_string(),
            branch_name: "litterbox/sandbox".to_string(),
            container_id: "container".to_string(),
            status: SandboxStatus::Active,
            forwarded_ports: Vec::new(),
        }
    }

    #[test]
    fn read_file_full_content() {
        let mut file = NamedTempFile::new().expect("temp file");
        write!(file, "one\ntwo\nthree\n").expect("write");
        let content = read_file_content(file.path(), None, None).expect("read");
        assert_eq!(content, "one\ntwo\nthree\n");
    }

    #[test]
    fn read_file_slice_content() {
        let mut file = NamedTempFile::new().expect("temp file");
        write!(file, "one\ntwo\nthree\n").expect("write");
        let content = read_file_content(file.path(), Some(1), Some(1)).expect("read");
        assert_eq!(content, "two\n");
    }

    #[test]
    fn read_file_slice_out_of_range() {
        let mut file = NamedTempFile::new().expect("temp file");
        write!(file, "one\ntwo\n").expect("write");
        let content = read_file_content(file.path(), Some(5), Some(2)).expect("read");
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn read_in_sandbox_full_content() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "one\ntwo\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let content = read_in_sandbox(&provider, &stub_metadata(), "README.md", None, None)
            .await
            .expect("read");

        assert_eq!(content, "one\ntwo\n");
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert_eq!(command[0], "sh");
        assert_eq!(command[1], "-c");
        assert!(command[2].contains("cat --"));
        assert!(command[2].contains("/src/README.md"));
    }

    #[tokio::test]
    async fn read_in_sandbox_slice_content() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "one\ntwo\nthree\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let content = read_in_sandbox(&provider, &stub_metadata(), "README.md", Some(1), Some(1))
            .await
            .expect("read");

        assert_eq!(content, "two\n");
    }

    #[tokio::test]
    async fn read_in_sandbox_missing_file_returns_not_found() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/missing: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = read_in_sandbox(&provider, &stub_metadata(), "missing", None, None)
            .await
            .expect_err("missing file");
        match error {
            ReadError::NotFound { path } => assert_eq!(path, "/src/missing"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_in_sandbox_success() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        write_in_sandbox(&provider, &stub_metadata(), "file.txt", "hello")
            .await
            .expect("write");

        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("printf %s"));
        assert!(command[2].contains("'hello'"));
        assert!(command[2].contains("/src/file.txt"));
    }

    #[tokio::test]
    async fn write_in_sandbox_permission_denied() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "/src/file.txt: Permission denied".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = write_in_sandbox(&provider, &stub_metadata(), "file.txt", "hello")
            .await
            .expect_err("permission denied");
        match error {
            WriteError::PermissionDenied { path } => assert_eq!(path, "/src/file.txt"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_in_sandbox_missing_path() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "sh: /src/missing/file.txt: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = write_in_sandbox(&provider, &stub_metadata(), "missing/file.txt", "hello")
            .await
            .expect_err("missing path");
        match error {
            WriteError::NotFound { path } => assert_eq!(path, "/src/missing/file.txt"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn patch_in_sandbox_success() {
        // Mock read returning original content
        let read_result = ExecutionResult {
            exit_code: 0,
            stdout: "original\n".to_string(),
            stderr: String::new(),
        };
        // Mock write succeeding
        let write_result = ExecutionResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };

        let results = Arc::new(Mutex::new(vec![Ok(read_result), Ok(write_result)]));
        let provider = MultiResultProvider::new(results);
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-original\n+patched\n";
        patch_in_sandbox(&provider, &stub_metadata(), "file.txt", diff)
            .await
            .expect("patch");
    }

    #[tokio::test]
    async fn patch_in_sandbox_invalid_diff() {
        // Mock read returning content
        let read_result = ExecutionResult {
            exit_code: 0,
            stdout: "line1\nline2\n".to_string(),
            stderr: String::new(),
        };

        // The patch will fail to apply because it tries to replace text that doesn't exist
        // This will trigger the ApplyFailed error, not InvalidPatch
        let results = Arc::new(Mutex::new(vec![Ok(read_result)]));
        let provider = MultiResultProvider::new(results);

        // A diff that will parse but fail to apply
        let bad_diff =
            "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-nonexistent line\n+replacement\n";
        let error = patch_in_sandbox(&provider, &stub_metadata(), "file.txt", bad_diff)
            .await
            .expect_err("invalid diff");
        match error {
            PatchError::ApplyFailed { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn patch_in_sandbox_missing_path() {
        // Mock read failing with not found
        let read_result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/missing.txt: No such file or directory".to_string(),
        };

        let results = Arc::new(Mutex::new(vec![Ok(read_result)]));
        let provider = MultiResultProvider::new(results);
        let error = patch_in_sandbox(&provider, &stub_metadata(), "missing.txt", "diff")
            .await
            .expect_err("missing path");
        match error {
            PatchError::ReadFile { path, source } => {
                assert_eq!(path, "missing.txt");
                match *source {
                    ReadError::NotFound { path } => assert_eq!(path, "/src/missing.txt"),
                    other => panic!("unexpected read error: {other:?}"),
                }
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ls_in_sandbox_non_recursive() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "file.txt\nsubdir\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = ls_in_sandbox(&provider, &stub_metadata(), "dir", false)
            .await
            .expect("list");

        assert_eq!(entries, vec!["file.txt", "subdir"]);
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("ls -1A"));
        assert!(command[2].contains("/src/dir"));
    }

    #[tokio::test]
    async fn ls_in_sandbox_recursive() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "/src/dir/subdir\n/src/dir/subdir/child.txt\n/src/dir/file.txt\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = ls_in_sandbox(&provider, &stub_metadata(), "dir", true)
            .await
            .expect("list");

        assert_eq!(entries, vec!["file.txt", "subdir", "subdir/child.txt"]);
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("find"));
        assert!(command[2].contains("-mindepth 1"));
        assert!(command[2].contains("/src/dir"));
    }

    #[tokio::test]
    async fn ls_in_sandbox_empty_directory() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = ls_in_sandbox(&provider, &stub_metadata(), "empty", false)
            .await
            .expect("list");

        assert!(entries.is_empty());
    }

    #[test]
    fn classify_ls_failure_permission_denied() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "ls: /src/secret: Permission denied".to_string(),
        };
        let error = classify_ls_failure("/src/secret", &result);
        match error {
            LsError::PermissionDenied { path } => assert_eq!(path, "/src/secret"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn classify_ls_failure_not_found() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "ls: /src/missing: No such file or directory".to_string(),
        };
        let error = classify_ls_failure("/src/missing", &result);
        match error {
            LsError::NotFound { path } => assert_eq!(path, "/src/missing"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_ls_error_missing_sandbox() {
        let error = map_ls_error(
            "missing",
            LsError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[tokio::test]
    async fn glob_in_sandbox_matches_with_base() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "/src/dir/subdir\n/src/dir/subdir/child.txt\n/src/dir/root.txt\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = glob_in_sandbox(&provider, &stub_metadata(), "**/*.txt", Some("dir"))
            .await
            .expect("glob");

        assert_eq!(entries, vec!["root.txt", "subdir/child.txt"]);
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("find"));
        assert!(command[2].contains("/src/dir"));
    }

    #[tokio::test]
    async fn glob_in_sandbox_no_matches() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "/src/root.txt\n".to_string(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = glob_in_sandbox(&provider, &stub_metadata(), "*.md", None)
            .await
            .expect("glob");

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn glob_in_sandbox_invalid_pattern() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = glob_in_sandbox(&provider, &stub_metadata(), "[[", None)
            .await
            .expect_err("invalid pattern");
        match error {
            GlobError::InvalidPattern { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn grep_in_sandbox_matches() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "/src/dir/file.txt:1:hello\n/src/dir/sub/file.rs:2:hello\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = grep_in_sandbox(&provider, &stub_metadata(), "hello", "dir", None)
            .await
            .expect("grep");

        assert_eq!(
            entries,
            vec!["/src/dir/file.txt:1:hello", "/src/dir/sub/file.rs:2:hello"]
        );
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("grep -R -n"));
        assert!(command[2].contains("--"));
        assert!(command[2].contains("'hello'"));
        assert!(command[2].contains("/src/dir"));
    }

    #[tokio::test]
    async fn grep_in_sandbox_include_filter() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "/src/dir/main.rs:1:hello\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = grep_in_sandbox(&provider, &stub_metadata(), "hello", "dir", Some("*.rs"))
            .await
            .expect("grep");

        assert_eq!(entries, vec!["/src/dir/main.rs:1:hello"]);
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert!(command[2].contains("--include="));
        assert!(command[2].contains("*.rs"));
    }

    #[tokio::test]
    async fn grep_in_sandbox_no_matches() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = grep_in_sandbox(&provider, &stub_metadata(), "hello", "dir", None)
            .await
            .expect("grep");

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn grep_in_sandbox_invalid_pattern() {
        let result = ExecutionResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "grep: Unmatched [".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = grep_in_sandbox(&provider, &stub_metadata(), "[", "dir", None)
            .await
            .expect_err("invalid pattern");
        match error {
            GrepError::InvalidPattern { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn grep_in_sandbox_missing_path() {
        let result = ExecutionResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "grep: /src/dir: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = grep_in_sandbox(&provider, &stub_metadata(), "hello", "dir", None)
            .await
            .expect_err("missing path");
        match error {
            GrepError::NotFound { path } => assert_eq!(path, "/src/dir"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_grep_error_missing_sandbox() {
        let error = map_grep_error(
            "missing",
            GrepError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[test]
    fn map_glob_error_missing_sandbox() {
        let error = map_glob_tool_error(
            "missing",
            GlobError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[test]
    fn classify_read_failure_permission_denied() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/secret: Permission denied".to_string(),
        };
        let error = classify_read_failure("/src/secret", &result);
        match error {
            ReadError::PermissionDenied { path } => assert_eq!(path, "/src/secret"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn classify_read_failure_invalid_path() {
        let result = ExecutionResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/dir: Is a directory".to_string(),
        };
        let error = classify_read_failure("/src/dir", &result);
        match error {
            ReadError::Failed { path, message } => {
                assert_eq!(path, "/src/dir");
                assert!(message.contains("Is a directory"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_read_error_missing_sandbox() {
        let error = map_read_error(
            "missing",
            ReadError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[test]
    fn map_write_error_missing_sandbox() {
        let error = map_write_error(
            "missing",
            WriteError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[test]
    fn map_grep_error_invalid_pattern_message() {
        let error = map_grep_error(
            "sandbox",
            GrepError::InvalidPattern {
                pattern: "[".to_string(),
                message: "Unmatched [".to_string(),
            },
        );
        assert!(error.to_string().contains("invalid grep pattern"));
    }

    #[tokio::test]
    async fn bash_in_sandbox_success() {
        let result = ExecutionResult {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let output = bash_in_sandbox(&provider, &stub_metadata(), "echo ok", None, None)
            .await
            .expect("bash");

        assert_eq!(output.stdout, "ok");
        let command = last_command.lock().expect("command lock");
        let command = command.as_ref().expect("command captured");
        assert_eq!(command[0], "sh");
        assert_eq!(command[1], "-c");
        assert!(command[2].contains("echo ok"));
    }

    #[tokio::test]
    async fn bash_in_sandbox_non_zero_exit() {
        let result = ExecutionResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: "fail".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let output = bash_in_sandbox(&provider, &stub_metadata(), "false", None, None)
            .await
            .expect("bash");

        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "fail");
    }

    #[test]
    fn build_bash_command_with_workdir() {
        let command = build_bash_command("ls", Some("dir"), None);
        assert!(command.contains("cd '/src/dir'"));
        assert!(command.contains("&& ls"));
    }

    #[test]
    fn build_bash_command_with_timeout() {
        let command = build_bash_command("sleep 5", None, Some(3));
        assert!(command.starts_with("timeout 3s sh -c"));
        assert!(command.contains("sleep 5"));
    }

    #[test]
    fn build_bash_command_with_workdir_and_timeout() {
        let command = build_bash_command("ls -la", Some("dir"), Some(5));
        assert!(command.starts_with("timeout 5s sh -c"));
        assert!(command.contains("/src/dir"));
        assert!(command.contains("ls -la"));
    }

    #[test]
    fn resolve_container_path_relative() {
        assert_eq!(resolve_container_path("README.md"), "/src/README.md");
        assert_eq!(resolve_container_path("/etc/hosts"), "/etc/hosts");
    }

    #[test]
    fn shell_escape_handles_quotes() {
        assert_eq!(shell_escape("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn map_bash_error_missing_sandbox() {
        let error = map_bash_error(
            "missing",
            BashError::Sandbox(SandboxError::SandboxNotFound {
                name: "missing".to_string(),
            }),
        );
        assert!(error.to_string().contains("Sandbox 'missing' not found."));
    }

    #[test]
    fn snapshot_message_for_triggers() {
        assert_eq!(
            snapshot_message(&SnapshotTrigger::Write {
                path: "README.md".to_string()
            }),
            "write: README.md"
        );
        assert_eq!(
            snapshot_message(&SnapshotTrigger::Patch {
                path: "src/lib.rs".to_string()
            }),
            "patch: src/lib.rs"
        );
        assert_eq!(
            snapshot_message(&SnapshotTrigger::Bash {
                command: "cargo test".to_string()
            }),
            "bash: cargo test"
        );
    }

    #[test]
    fn snapshot_after_with_scm_skips_when_clean() {
        let scm = TestScm::new(false);
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Write {
                path: "a".to_string(),
            },
        )
        .expect("snapshot");
        let committed = scm.committed_messages.lock().expect("commit lock");
        assert!(committed.is_empty());
    }

    #[test]
    fn snapshot_after_with_scm_commits_when_dirty() {
        let scm = TestScm::new(true);
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Patch {
                path: "b".to_string(),
            },
        )
        .expect("snapshot");
        let committed = scm.committed_messages.lock().expect("commit lock");
        assert_eq!(committed.as_slice(), &["patch: b".to_string()]);
    }

    #[test]
    fn snapshot_after_with_scm_integration_commits() {
        let (tempdir, repo) = init_repo();
        fs::write(tempdir.path().join("README.md"), "updated").expect("write");
        let scm = ThreadSafeScm::open(tempdir.path()).expect("open scm");
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Write {
                path: "README.md".to_string(),
            },
        )
        .expect("snapshot");

        let snapshot_ref = repo
            .find_reference("refs/heads/litterbox-snapshots")
            .expect("snapshot ref");
        let snapshot_commit = snapshot_ref.peel_to_commit().expect("snapshot commit");
        assert_eq!(
            snapshot_commit.message().expect("message"),
            "write: README.md"
        );
        let head_commit = repo
            .head()
            .expect("head")
            .peel_to_commit()
            .expect("head commit");
        assert_ne!(snapshot_commit.id(), head_commit.id());
    }

    #[test]
    fn snapshot_after_with_scm_integration_skips_clean_repo() {
        let (tempdir, repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path()).expect("open scm");
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Write {
                path: "README.md".to_string(),
            },
        )
        .expect("snapshot");

        match repo.find_reference("refs/heads/litterbox-snapshots") {
            Ok(_) => panic!("unexpected snapshot ref"),
            Err(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        }
    }

    #[test]
    fn end_to_end_snapshot_workflow() {
        let (tempdir, repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path()).expect("open scm");

        fs::write(tempdir.path().join("README.md"), "write").expect("write");
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Write {
                path: "README.md".to_string(),
            },
        )
        .expect("snapshot write");

        fs::write(tempdir.path().join("README.md"), "patch").expect("write patch");
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Patch {
                path: "README.md".to_string(),
            },
        )
        .expect("snapshot patch");

        let status = Command::new("sh")
            .arg("-c")
            .arg("printf %s bash >>README.md")
            .current_dir(tempdir.path())
            .status()
            .expect("bash");
        assert!(status.success());
        snapshot_after_with_scm(
            &scm,
            SnapshotTrigger::Bash {
                command: "printf %s bash >>README.md".to_string(),
            },
        )
        .expect("snapshot bash");

        let snapshot_ref = repo
            .find_reference("refs/heads/litterbox-snapshots")
            .expect("snapshot ref");
        let head = snapshot_ref.peel_to_commit().expect("snapshot commit");
        assert_eq!(
            head.message().expect("message"),
            "bash: printf %s bash >>README.md"
        );
        let parent = head.parent(0).expect("parent");
        assert_eq!(parent.message().expect("message"), "patch: README.md");
        let grandparent = parent.parent(0).expect("parent");
        assert_eq!(grandparent.message().expect("message"), "write: README.md");
    }

    #[test]
    fn read_file_missing_returns_error() {
        let path = PathBuf::from("/tmp/this-file-should-not-exist-12345");
        let error = read_file_content(&path, None, None).expect_err("missing file");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn list_dir_non_recursive() {
        let dir = TempDir::new().expect("temp dir");
        let file_path = dir.path().join("file.txt");
        let subdir_path = dir.path().join("subdir");
        fs::create_dir(&subdir_path).expect("create dir");
        fs::write(&file_path, "data").expect("write");

        let entries = list_dir_entries(dir.path(), false).expect("list");
        assert_eq!(entries, vec!["file.txt", "subdir"]);
    }

    #[test]
    fn list_dir_recursive() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("root.txt"), "data").expect("write");
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).expect("create nested");
        fs::write(nested.join("child.txt"), "data").expect("write");

        let entries = list_dir_entries(dir.path(), true).expect("list");
        assert_eq!(entries, vec!["nested", "nested/child.txt", "root.txt"]);
    }

    #[test]
    fn glob_entries_with_base() {
        let dir = TempDir::new().expect("temp dir");
        fs::write(dir.path().join("root.txt"), "data").expect("write");
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).expect("create nested");
        fs::write(nested.join("child.txt"), "data").expect("write");

        let entries = glob_entries("**/*.txt", dir.path()).expect("glob");
        assert_eq!(entries, vec!["nested/child.txt", "root.txt"]);
    }

    #[test]
    fn glob_entries_invalid_pattern() {
        let dir = TempDir::new().expect("temp dir");
        let error = glob_entries("[[", dir.path()).expect_err("invalid pattern");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
