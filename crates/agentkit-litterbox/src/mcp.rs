use glob::{MatchOptions, Pattern};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(test)]
use glob::glob as glob_paths;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::io;
#[cfg(test)]
use std::path::PathBuf;

mod docs;
mod error;

use crate::config_loader;
use crate::orchestrator::SandboxIo;
use crate::orchestrator::Sandboxes;
use crate::sandbox::contract::{Error, Execution, Forward};

use docs::TOOL_DOCS;
use error::{map_config, map_contract};

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

pub struct SandboxServer {
    tool_router: ToolRouter<Self>,
    project: std::sync::OnceLock<Sandboxes>,
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
            project: std::sync::OnceLock::new(),
        }
    }

    /// The project, built once for the life of the server.
    fn project(&self) -> Result<&Sandboxes, McpError> {
        if let Some(project) = self.project.get() {
            return Ok(project);
        }
        let project = build_project()?;
        let _ = self.project.set(project);
        tracing::info!(
            "the MCP server reads project and local configuration and consent once at startup; \
             restart it after changing either file"
        );
        Ok(self.project.get().expect("just set"))
    }

    #[tool(
        name = "sandbox-create",
        description = "Create a new sandbox based on the current repository HEAD"
    )]
    async fn sandbox_create(
        &self,
        Parameters(args): Parameters<SandboxCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.project()?;
        let record = project.create(&args.name).await.map_err(map_contract)?;
        let inspection = project.inspect(&record.name).await.map_err(map_contract)?;
        let content = ContentBlock::json(inspection)
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
        let project = self.project()?;
        let forwarded_ports = project
            .forwards(&args.sandbox)
            .await
            .map_err(map_contract)?;
        let response = SandboxPortsResponse {
            name: args.sandbox,
            forwarded_ports,
        };
        let content = ContentBlock::json(response)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "read", description = "Read a file from the sandbox")]
    async fn read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.project()?;
        let content = read_in_sandbox(project, &args.sandbox, &args.path, args.offset, args.limit)
            .await
            .map_err(ToolError::into_mcp)?;
        let content = ContentBlock::text(content);
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "write", description = "Write a file into the sandbox")]
    async fn write(
        &self,
        Parameters(args): Parameters<WriteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.project()?;
        write_in_sandbox(project, &args.sandbox, &args.path, &args.content)
            .await
            .map_err(ToolError::into_mcp)?;
        if let Err(error) = project
            .snapshot(
                &args.sandbox,
                &snapshot_message(&SnapshotTrigger::Write { path: args.path }),
            )
            .await
        {
            // The file landed; a failed snapshot must not read as a failed
            // write, or the caller will apply the edit twice.
            tracing::warn!(sandbox = %args.sandbox, %error, "the write landed but the snapshot failed");
        }
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
        let project = self.project()?;
        patch_in_sandbox(project, &args.sandbox, &args.path, &args.diff)
            .await
            .map_err(ToolError::into_mcp)?;
        if let Err(error) = project
            .snapshot(
                &args.sandbox,
                &snapshot_message(&SnapshotTrigger::Patch { path: args.path }),
            )
            .await
        {
            tracing::warn!(sandbox = %args.sandbox, %error, "the patch landed but the snapshot failed");
        }
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
        let project = self.project()?;
        let result = bash_in_sandbox(
            project,
            &args.sandbox,
            &args.command,
            args.workdir.as_deref(),
            args.timeout,
        )
        .await
        .map_err(ToolError::into_mcp)?;
        if let Err(error) = project
            .snapshot(&args.sandbox, &snapshot_message(&SnapshotTrigger::Bash))
            .await
        {
            tracing::warn!(sandbox = %args.sandbox, %error, "the command ran but the snapshot failed");
        }
        let content = ContentBlock::json(result)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "ls", description = "List directory entries")]
    async fn ls(&self, Parameters(args): Parameters<LsArgs>) -> Result<CallToolResult, McpError> {
        let recursive = args.recursive.unwrap_or(false);
        let project = self.project()?;
        let entries = ls_in_sandbox(project, &args.sandbox, &args.path, recursive)
            .await
            .map_err(ToolError::into_mcp)?;
        let content = ContentBlock::json(entries)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "glob", description = "Find files matching a glob pattern")]
    async fn glob(
        &self,
        Parameters(args): Parameters<GlobArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.project()?;
        let matches = glob_in_sandbox(project, &args.sandbox, &args.pattern, args.path.as_deref())
            .await
            .map_err(ToolError::into_mcp)?;
        let content = ContentBlock::json(matches)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(name = "grep", description = "Search file contents for a pattern")]
    async fn grep(
        &self,
        Parameters(args): Parameters<GrepArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.project()?;
        let matches = grep_in_sandbox(
            project,
            &args.sandbox,
            &args.pattern,
            &args.path,
            args.include.as_deref(),
        )
        .await
        .map_err(ToolError::into_mcp)?;
        let content = ContentBlock::json(matches)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SandboxServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Litterbox sandbox management")
    }
}

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
            let requirement = if param.required {
                "required"
            } else {
                "optional"
            };
            output.push_str(&format!(
                "- `{}` ({}, {}) {}\n",
                param.name, param.type_name, requirement, param.description
            ));
        }
        output.push('\n');
    }

    output.push_str(&crate::docgen::mcp_section());
    output
}

pub async fn run_stdio(backend: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(backend) = backend {
        let _ = BACKEND_OVERRIDE.set(backend);
    }
    let service = SandboxServer::new().serve(stdio()).await.inspect_err(|e| {
        eprintln!("Error starting MCP server: {e}");
    })?;
    service.waiting().await?;
    Ok(())
}

fn build_project() -> Result<Sandboxes, McpError> {
    let mut config = config_loader::load_final().map_err(map_config)?;
    if let Some(backend) = backend_override() {
        config.backend = Some(backend);
    }
    for notice in &config.notices {
        tracing::warn!("configuration: {notice}");
    }
    Sandboxes::open(config).map_err(|error| map_contract(error.into()))
}

/// The per-invocation backend selection, set once when the server starts.
static BACKEND_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn backend_override() -> Option<String> {
    BACKEND_OVERRIDE.get().cloned()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SandboxPortsArgs {
    pub sandbox: String,
}

#[derive(Debug, Serialize)]
struct SandboxPortsResponse {
    pub name: String,
    pub forwarded_ports: Vec<Forward>,
}

/// The tool that produced a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Ls,
    Read,
    Write,
    Patch,
    Bash,
    Glob,
    Grep,
}

/// Why a tool failed, independent of which tool it was.
#[derive(Debug)]
enum ToolFailure {
    Sandbox(Error),
    NotFound { path: String },
    PermissionDenied { path: String },
    Failed { path: String, message: String },
    InvalidPattern { pattern: String, message: String },
    InvalidPatch { source: String },
    PatchRead { path: String, source: String },
    PatchWrite { path: String, source: String },
    PatchApply { path: String, source: String },
}

/// One tool failure: the tool that raised it and what went wrong.
#[derive(Debug)]
struct ToolError {
    tool: Tool,
    failure: ToolFailure,
}

impl ToolError {
    fn sandbox(tool: Tool, error: Error) -> Self {
        Self {
            tool,
            failure: ToolFailure::Sandbox(error),
        }
    }

    fn classify(tool: Tool, path: &str, result: &Execution) -> Self {
        let stderr = result.stderr.trim();
        let stdout = result.stdout.trim();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        let failure = if message.contains("No such file or directory") {
            ToolFailure::NotFound {
                path: path.to_string(),
            }
        } else if message.contains("Permission denied") {
            ToolFailure::PermissionDenied {
                path: path.to_string(),
            }
        } else if message.is_empty() {
            ToolFailure::Failed {
                path: path.to_string(),
                message: format!("exit code {}", result.exit_code),
            }
        } else {
            ToolFailure::Failed {
                path: path.to_string(),
                message: message.to_string(),
            }
        };
        Self { tool, failure }
    }

    fn invalid_pattern(tool: Tool, pattern: &str, message: impl Into<String>) -> Self {
        Self {
            tool,
            failure: ToolFailure::InvalidPattern {
                pattern: pattern.to_string(),
                message: message.into(),
            },
        }
    }

    fn invalid_patch(source: impl Into<String>) -> Self {
        Self {
            tool: Tool::Patch,
            failure: ToolFailure::InvalidPatch {
                source: source.into(),
            },
        }
    }

    fn patch_read(path: &str, source: impl std::fmt::Debug) -> Self {
        Self {
            tool: Tool::Patch,
            failure: ToolFailure::PatchRead {
                path: path.to_string(),
                source: format!("{source:?}"),
            },
        }
    }

    fn patch_write(path: &str, source: impl std::fmt::Debug) -> Self {
        Self {
            tool: Tool::Patch,
            failure: ToolFailure::PatchWrite {
                path: path.to_string(),
                source: format!("{source:?}"),
            },
        }
    }

    fn patch_apply(path: &str, source: impl Into<String>) -> Self {
        Self {
            tool: Tool::Patch,
            failure: ToolFailure::PatchApply {
                path: path.to_string(),
                source: source.into(),
            },
        }
    }

    fn into_mcp(self) -> McpError {
        match self.failure {
            ToolFailure::Sandbox(error) => map_contract(error),
            ToolFailure::NotFound { path } => {
                let noun = if self.tool == Tool::Read {
                    "file not found"
                } else {
                    "path not found"
                };
                McpError::invalid_params(format!("{noun}: {path}"), None)
            }
            ToolFailure::PermissionDenied { path } => {
                McpError::invalid_params(format!("permission denied: {path}"), None)
            }
            ToolFailure::Failed { path, message } => {
                let prefix = match self.tool {
                    Tool::Ls => "failed to list",
                    Tool::Read => "failed to read",
                    Tool::Write => "failed to write",
                    Tool::Glob => "glob failed for",
                    Tool::Grep => "grep failed for",
                    Tool::Patch | Tool::Bash => "failed",
                };
                McpError::internal_error(format!("{prefix} {path}: {message}"), None)
            }
            ToolFailure::InvalidPattern { pattern, message } => {
                let kind = if self.tool == Tool::Glob {
                    "glob"
                } else {
                    "grep"
                };
                McpError::invalid_params(
                    format!("invalid {kind} pattern '{pattern}': {message}"),
                    None,
                )
            }
            ToolFailure::InvalidPatch { source } => {
                McpError::invalid_params(format!("invalid patch: {source}"), None)
            }
            ToolFailure::PatchRead { path, source } => McpError::internal_error(
                format!("failed to read file {path} for patching: {source}"),
                None,
            ),
            ToolFailure::PatchWrite { path, source } => McpError::internal_error(
                format!("failed to write patched file {path}: {source}"),
                None,
            ),
            ToolFailure::PatchApply { path, source } => {
                McpError::internal_error(format!("failed to apply patch to {path}: {source}"), None)
            }
        }
    }
}

#[allow(clippy::result_large_err)]
async fn ls_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    path: &str,
    recursive: bool,
) -> Result<Vec<String>, ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Ls, error))?;
    let sandbox_path = resolve_sandbox_path(&workspace, path);
    let command = if recursive {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("find {} -mindepth 1 -print", shell_escape(&sandbox_path)),
        ]
    } else {
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("ls -1A {}", shell_escape(&sandbox_path)),
        ]
    };
    let result = exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Ls, error))?;
    if result.exit_code != 0 {
        return Err(ToolError::classify(Tool::Ls, &sandbox_path, &result));
    }
    Ok(parse_ls_output(&result.stdout, &sandbox_path, recursive))
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

#[derive(Debug, Clone)]
enum SnapshotTrigger {
    Write { path: String },
    Patch { path: String },
    Bash,
}

fn snapshot_message(trigger: &SnapshotTrigger) -> String {
    // Raw tool input is never persisted: a command line can carry credentials,
    // and a path can be arbitrarily long.
    match trigger {
        SnapshotTrigger::Write { path } => format!("write: {}", bounded(path)),
        SnapshotTrigger::Patch { path } => format!("patch: {}", bounded(path)),
        SnapshotTrigger::Bash => "bash".to_string(),
    }
}

fn bounded(text: &str) -> String {
    text.chars().take(120).collect()
}

#[allow(clippy::result_large_err)]
async fn read_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Read, error))?;
    let sandbox_path = resolve_sandbox_path(&workspace, path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("cat -- {}", shell_escape(&sandbox_path)),
    ];
    let result = exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Read, error))?;
    if result.exit_code != 0 {
        return Err(ToolError::classify(Tool::Read, &sandbox_path, &result));
    }
    Ok(slice_content(&result.stdout, offset, limit))
}

#[allow(clippy::result_large_err)]
async fn write_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    path: &str,
    content: &str,
) -> Result<(), ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Write, error))?;
    let sandbox_path = resolve_sandbox_path(&workspace, path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "printf %s {} > {}",
            shell_escape(content),
            shell_escape(&sandbox_path)
        ),
    ];
    let result = exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Write, error))?;
    if result.exit_code != 0 {
        return Err(ToolError::classify(Tool::Write, &sandbox_path, &result));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn patch_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    path: &str,
    diff: &str,
) -> Result<(), ToolError> {
    // Read current file content
    let original_content = read_in_sandbox(io, name, path, None, None)
        .await
        .map_err(|error| ToolError::patch_read(path, error))?;

    // Parse and apply patch using diffy
    let patch = diffy::Patch::from_str(diff)
        .map_err(|error| ToolError::invalid_patch(error.to_string()))?;

    let patched_content = diffy::apply(&original_content, &patch)
        .map_err(|error| ToolError::patch_apply(path, error.to_string()))?;

    // Write patched content back
    write_in_sandbox(io, name, path, &patched_content)
        .await
        .map_err(|error| ToolError::patch_write(path, error))?;

    Ok(())
}

#[allow(clippy::result_large_err)]
async fn bash_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    command: &str,
    workdir: Option<&str>,
    timeout: Option<u64>,
) -> Result<Execution, ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Bash, error))?;
    let command = build_bash_command(command, workdir, timeout, &workspace);
    let command = vec!["sh".to_string(), "-c".to_string(), command];
    exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Bash, error))
}

fn build_bash_command(
    command: &str,
    workdir: Option<&str>,
    timeout: Option<u64>,
    workspace: &str,
) -> String {
    let command = if let Some(workdir) = workdir {
        let sandbox_path = resolve_sandbox_path(workspace, workdir);
        format!("cd {} && {}", shell_escape(&sandbox_path), command)
    } else {
        command.to_string()
    };

    if let Some(timeout) = timeout {
        format!("timeout {}s sh -c {}", timeout, shell_escape(&command))
    } else {
        command
    }
}

async fn exec_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    command: &[String],
) -> Result<Execution, Error> {
    io.exec(name, command).await
}

fn resolve_sandbox_path(workspace: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{workspace}/{path}")
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

#[allow(clippy::result_large_err)]
async fn glob_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    pattern: &str,
    base_path: Option<&str>,
) -> Result<Vec<String>, ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Glob, error))?;
    let base = base_path
        .map(|path| resolve_sandbox_path(&workspace, path))
        .unwrap_or_else(|| workspace.clone());
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("find {} -mindepth 1 -print", shell_escape(&base)),
    ];
    let result = exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Glob, error))?;
    if result.exit_code != 0 {
        return Err(ToolError::classify(Tool::Glob, &base, &result));
    }

    let pattern_is_absolute = Path::new(pattern).is_absolute();
    let pattern = Pattern::new(pattern)
        .map_err(|error| ToolError::invalid_pattern(Tool::Glob, pattern, error.to_string()))?;
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

#[allow(clippy::result_large_err)]
async fn grep_in_sandbox<P: SandboxIo + ?Sized>(
    io: &P,
    name: &str,
    pattern: &str,
    path: &str,
    include: Option<&str>,
) -> Result<Vec<String>, ToolError> {
    let workspace = io
        .workspace(name)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Grep, error))?;
    let sandbox_path = resolve_sandbox_path(&workspace, path);
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        build_grep_command(pattern, &sandbox_path, include),
    ];
    let result = exec_in_sandbox(io, name, &command)
        .await
        .map_err(|error| ToolError::sandbox(Tool::Grep, error))?;
    if result.exit_code == 0 {
        return Ok(parse_grep_output(&result.stdout));
    }
    if result.exit_code == 1 && result.stderr.trim().is_empty() {
        return Ok(Vec::new());
    }
    Err(classify_grep_failure(&sandbox_path, pattern, &result))
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

#[allow(clippy::result_large_err)]
fn classify_grep_failure(path: &str, pattern: &str, result: &Execution) -> ToolError {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    if message.contains("Unmatched") || message.contains("Invalid") {
        ToolError::invalid_pattern(Tool::Grep, pattern, message.to_string())
    } else {
        ToolError::classify(Tool::Grep, path, result)
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
    use futures_util::future::BoxFuture;
    use std::fs;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[test]
    fn the_error_data_carries_the_class_the_code_cannot() {
        let not_found = map_contract(Error::NotFound {
            name: "demo".to_string(),
        });
        let conflict = map_contract(Error::Conflict {
            name: "demo".to_string(),
        });
        assert_eq!(not_found.code, conflict.code, "rmcp collapses both");
        assert_eq!(not_found.data.as_ref().unwrap()["class"], "not-found");
        assert_eq!(conflict.data.as_ref().unwrap()["class"], "conflict");

        let unsupported = map_contract(Error::Unsupported {
            operation: "pause".to_string(),
            backend: "docker".to_string(),
            primitive: "a sandbox-level state".to_string(),
            reason: "it has none".to_string(),
            remedy: "stop the container".to_string(),
        });
        let denied = map_contract(Error::Denied {
            operation: "read".to_string(),
            reason: "outside the workspace".to_string(),
        });
        assert_eq!(unsupported.code, denied.code, "rmcp collapses both");
        assert_eq!(unsupported.data.as_ref().unwrap()["class"], "unsupported");
        assert_eq!(denied.data.as_ref().unwrap()["class"], "denied");
    }

    fn provisioning_error(
        class: crate::sandbox::contract::ProvisioningClass,
        backend: Option<String>,
        remedy: &str,
        exit_code: Option<i32>,
        output: Option<&str>,
    ) -> Error {
        Error::Provisioning {
            failure: Box::new(crate::sandbox::contract::ProvisioningFailure {
                class,
                backend,
                step: Some("fetch".to_string()),
                kind: Some("command".to_string()),
                reason: "it cannot".to_string(),
                remedy: remedy.to_string(),
                exit_code,
                output: output.map(str::to_string),
            }),
        }
    }

    #[test]
    fn a_provisioning_failure_carries_the_step_and_kind() {
        let unsupported = map_contract(provisioning_error(
            crate::sandbox::contract::ProvisioningClass::Unsupported,
            Some("docker".to_string()),
            "choose another backend",
            None,
            None,
        ));
        let data = unsupported.data.as_ref().expect("data");
        assert_eq!(data["class"], "unsupported");
        assert_eq!(data["step"], "fetch");
        assert_eq!(data["kind"], "command");
        assert_eq!(data["backend"], "docker");
        assert_eq!(data["reason"], "it cannot");
        assert_eq!(data["remedy"], "choose another backend");
    }

    #[test]
    fn a_misconfigured_failure_is_an_internal_error_with_no_backend() {
        let error = map_contract(provisioning_error(
            crate::sandbox::contract::ProvisioningClass::Misconfigured,
            None,
            "fix the sequence",
            None,
            None,
        ));
        let data = error.data.as_ref().expect("data");
        assert_eq!(data["class"], "misconfigured");
        assert!(data.get("backend").is_none(), "{data}");
        assert_eq!(data["remedy"], "fix the sequence");
        assert_eq!(error.code, McpError::internal_error("x", None).code);
    }

    #[test]
    fn a_denied_failure_is_an_invalid_request_with_consent_fields() {
        let error = map_contract(provisioning_error(
            crate::sandbox::contract::ProvisioningClass::Denied,
            Some("docker".to_string()),
            "add the project to allowed-projects",
            None,
            None,
        ));
        let data = error.data.as_ref().expect("data");
        assert_eq!(data["class"], "denied");
        assert_eq!(data["backend"], "docker");
        assert_eq!(data["remedy"], "add the project to allowed-projects");
        assert_eq!(error.code, McpError::invalid_request("x", None).code);
    }

    #[test]
    fn a_failed_command_carries_its_exit_code_and_output() {
        let error = map_contract(provisioning_error(
            crate::sandbox::contract::ProvisioningClass::Failed,
            Some("docker".to_string()),
            "inspect the output and fix the step",
            Some(7),
            Some("boom\n\u{1b}]0;pwned"),
        ));
        let data = error.data.as_ref().expect("data");
        assert_eq!(data["class"], "failed");
        assert_eq!(data["exit_code"], 7);
        assert_eq!(data["remedy"], "inspect the output and fix the step");
        assert_eq!(data["output_untrusted"], true);
        let output = data["output"].as_str().expect("output");
        assert!(output.contains("boom"), "{output}");
        assert!(
            !output.contains('\u{1b}'),
            "control characters are escaped: {output}"
        );
        assert_eq!(error.code, McpError::internal_error("x", None).code);
    }

    struct TestProvider {
        shell_result: Mutex<Option<Result<Execution, Error>>>,
        last_command: Arc<Mutex<Option<Vec<String>>>>,
    }

    impl TestProvider {
        fn new(
            result: Result<Execution, Error>,
            last_command: Arc<Mutex<Option<Vec<String>>>>,
        ) -> Self {
            Self {
                shell_result: Mutex::new(Some(result)),
                last_command,
            }
        }
    }

    struct MultiResultProvider {
        results: Arc<Mutex<Vec<Result<Execution, Error>>>>,
        commands: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl MultiResultProvider {
        fn new(results: Arc<Mutex<Vec<Result<Execution, Error>>>>) -> Self {
            Self {
                results,
                commands: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().expect("commands lock").clone()
        }
    }

    impl SandboxIo for MultiResultProvider {
        fn workspace<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Result<String, Error>> {
            Box::pin(async { Ok("/src".to_string()) })
        }

        fn exec<'a>(
            &'a self,
            _name: &'a str,
            command: &'a [String],
        ) -> BoxFuture<'a, Result<Execution, Error>> {
            self.commands
                .lock()
                .expect("commands lock")
                .push(command.to_vec());
            let results = Arc::clone(&self.results);
            Box::pin(async move {
                let mut results_lock = results.lock().expect("results lock");
                if results_lock.is_empty() {
                    return Err(Error::NotFound {
                        name: "no more results".to_string(),
                    });
                }
                results_lock.remove(0)
            })
        }

        fn upload<'a>(
            &'a self,
            _name: &'a str,
            _source: &'a Path,
            _destination: &'a str,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn download<'a>(
            &'a self,
            _name: &'a str,
            _source: &'a str,
            _destination: &'a Path,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl SandboxIo for TestProvider {
        fn workspace<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Result<String, Error>> {
            Box::pin(async { Ok("/src".to_string()) })
        }

        fn exec<'a>(
            &'a self,
            _name: &'a str,
            command: &'a [String],
        ) -> BoxFuture<'a, Result<Execution, Error>> {
            let result = self
                .shell_result
                .lock()
                .expect("shell result lock")
                .take()
                .unwrap_or_else(|| {
                    Err(Error::NotFound {
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

        fn upload<'a>(
            &'a self,
            _name: &'a str,
            _source: &'a Path,
            _destination: &'a str,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }

        fn download<'a>(
            &'a self,
            _name: &'a str,
            _source: &'a str,
            _destination: &'a Path,
        ) -> BoxFuture<'a, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn the_agent_facing_tool_names_are_unchanged() {
        let server = SandboxServer::new();
        let mut names: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        names.sort();

        assert_eq!(
            names,
            vec![
                "bash".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "ls".to_string(),
                "patch".to_string(),
                "read".to_string(),
                "sandbox-create".to_string(),
                "sandbox-ports".to_string(),
                "write".to_string(),
            ]
        );
    }

    #[test]
    fn the_sandbox_ports_response_carries_the_neutral_forward_shape() {
        let response = SandboxPortsResponse {
            name: "demo".to_string(),
            forwarded_ports: vec![Forward {
                name: "web".to_string(),
                target: 8080,
                host_port: 49152,
                bind: "127.0.0.1".to_string(),
                env_var: "LITTERBOX_FWD_PORT_WEB".to_string(),
            }],
        };
        let json = serde_json::to_value(&response).expect("serialises");
        assert_eq!(json["name"], "demo");
        assert_eq!(json["forwarded_ports"][0]["target"], 8080);
        assert_eq!(json["forwarded_ports"][0]["bind"], "127.0.0.1");
        assert!(
            json["forwarded_ports"][0].get("container").is_none(),
            "no mechanism vocabulary leaks: {json}"
        );
    }

    #[test]
    fn every_tool_exposes_the_documented_arguments_and_no_backend_setting() {
        fn expected(tool: &str) -> Vec<&'static str> {
            match tool {
                "bash" => vec!["command", "sandbox", "timeout", "workdir"],
                "glob" => vec!["path", "pattern", "sandbox"],
                "grep" => vec!["include", "path", "pattern", "sandbox"],
                "ls" => vec!["path", "recursive", "sandbox"],
                "patch" => vec!["diff", "path", "sandbox"],
                "read" => vec!["limit", "offset", "path", "sandbox"],
                "sandbox-create" => vec!["name"],
                "sandbox-ports" => vec!["sandbox"],
                "write" => vec!["content", "path", "sandbox"],
                other => panic!("unexpected tool {other}"),
            }
        }

        let server = SandboxServer::new();
        for tool in server.tool_router.list_all() {
            let mut arguments: Vec<String> = tool
                .input_schema
                .get("properties")
                .and_then(|properties| properties.as_object())
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default();
            arguments.sort();

            let expected: Vec<String> = expected(&tool.name)
                .into_iter()
                .map(str::to_string)
                .collect();
            assert_eq!(
                arguments, expected,
                "argument names for {} changed",
                tool.name
            );
            assert!(
                !arguments.iter().any(|argument| {
                    matches!(argument.as_str(), "backend" | "image" | "docker")
                }),
                "{} accepts a backend setting",
                tool.name
            );

            let doc = TOOL_DOCS
                .iter()
                .find(|doc| doc.name == tool.name)
                .unwrap_or_else(|| panic!("no documentation for {}", tool.name));
            let mut documented: Vec<String> = doc
                .params
                .iter()
                .map(|param| param.name.to_string())
                .collect();
            documented.sort();
            assert_eq!(
                documented, arguments,
                "the reference for {} drifted from its schema",
                tool.name
            );
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
        let result = Execution {
            exit_code: 0,
            stdout: "one\ntwo\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let content = read_in_sandbox(&provider, "sandbox", "README.md", None, None)
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
        let result = Execution {
            exit_code: 0,
            stdout: "one\ntwo\nthree\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let content = read_in_sandbox(&provider, "sandbox", "README.md", Some(1), Some(1))
            .await
            .expect("read");

        assert_eq!(content, "two\n");
    }

    #[tokio::test]
    async fn read_in_sandbox_missing_file_returns_not_found() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/missing: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = read_in_sandbox(&provider, "sandbox", "missing", None, None)
            .await
            .expect_err("missing file");
        match error.failure {
            ToolFailure::NotFound { path } => assert_eq!(path, "/src/missing"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_in_sandbox_success() {
        let result = Execution {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        write_in_sandbox(&provider, "sandbox", "file.txt", "hello")
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
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "/src/file.txt: Permission denied".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = write_in_sandbox(&provider, "sandbox", "file.txt", "hello")
            .await
            .expect_err("permission denied");
        match error.failure {
            ToolFailure::PermissionDenied { path } => assert_eq!(path, "/src/file.txt"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_in_sandbox_missing_path() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "sh: /src/missing/file.txt: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = write_in_sandbox(&provider, "sandbox", "missing/file.txt", "hello")
            .await
            .expect_err("missing path");
        match error.failure {
            ToolFailure::NotFound { path } => assert_eq!(path, "/src/missing/file.txt"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn patch_in_sandbox_success() {
        // Mock read returning original content
        let read_result = Execution {
            exit_code: 0,
            stdout: "original\n".to_string(),
            stderr: String::new(),
        };
        // Mock write succeeding
        let write_result = Execution {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };

        let results = Arc::new(Mutex::new(vec![Ok(read_result), Ok(write_result)]));
        let provider = MultiResultProvider::new(results);
        let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-original\n+patched\n";
        patch_in_sandbox(&provider, "sandbox", "file.txt", diff)
            .await
            .expect("patch");

        let commands = provider.commands();
        assert!(
            commands
                .last()
                .map(|command| command.join(" ").contains("patched"))
                .unwrap_or(false),
            "the write carried the patched text: {commands:?}"
        );
    }

    #[tokio::test]
    async fn patch_in_sandbox_invalid_diff() {
        // Mock read returning content
        let read_result = Execution {
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
        let error = patch_in_sandbox(&provider, "sandbox", "file.txt", bad_diff)
            .await
            .expect_err("invalid diff");
        match error.failure {
            ToolFailure::PatchApply { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn patch_in_sandbox_missing_path() {
        // Mock read failing with not found
        let read_result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/missing.txt: No such file or directory".to_string(),
        };

        let results = Arc::new(Mutex::new(vec![Ok(read_result)]));
        let provider = MultiResultProvider::new(results);
        let error = patch_in_sandbox(&provider, "sandbox", "missing.txt", "diff")
            .await
            .expect_err("missing path");
        match error.failure {
            ToolFailure::PatchRead { path, source } => {
                assert_eq!(path, "missing.txt");
                assert!(source.contains("NotFound"), "{source}");
                assert!(source.contains("/src/missing.txt"), "{source}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ls_in_sandbox_non_recursive() {
        let result = Execution {
            exit_code: 0,
            stdout: "file.txt\nsubdir\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = ls_in_sandbox(&provider, "sandbox", "dir", false)
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
        let result = Execution {
            exit_code: 0,
            stdout: "/src/dir/subdir\n/src/dir/subdir/child.txt\n/src/dir/file.txt\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = ls_in_sandbox(&provider, "sandbox", "dir", true)
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
        let result = Execution {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = ls_in_sandbox(&provider, "sandbox", "empty", false)
            .await
            .expect("list");

        assert!(entries.is_empty());
    }

    #[test]
    fn classify_ls_failure_permission_denied() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "ls: /src/secret: Permission denied".to_string(),
        };
        let error = ToolError::classify(Tool::Ls, "/src/secret", &result);
        match error.failure {
            ToolFailure::PermissionDenied { path } => assert_eq!(path, "/src/secret"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn classify_ls_failure_not_found() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "ls: /src/missing: No such file or directory".to_string(),
        };
        let error = ToolError::classify(Tool::Ls, "/src/missing", &result);
        match error.failure {
            ToolFailure::NotFound { path } => assert_eq!(path, "/src/missing"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_ls_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Ls,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn glob_in_sandbox_matches_with_base() {
        let result = Execution {
            exit_code: 0,
            stdout: "/src/dir/subdir\n/src/dir/subdir/child.txt\n/src/dir/root.txt\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = glob_in_sandbox(&provider, "sandbox", "**/*.txt", Some("dir"))
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
        let result = Execution {
            exit_code: 0,
            stdout: "/src/root.txt\n".to_string(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = glob_in_sandbox(&provider, "sandbox", "*.md", None)
            .await
            .expect("glob");

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn glob_in_sandbox_invalid_pattern() {
        let result = Execution {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = glob_in_sandbox(&provider, "sandbox", "[[", None)
            .await
            .expect_err("invalid pattern");
        match error.failure {
            ToolFailure::InvalidPattern { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn grep_in_sandbox_matches() {
        let result = Execution {
            exit_code: 0,
            stdout: "/src/dir/file.txt:1:hello\n/src/dir/sub/file.rs:2:hello\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = grep_in_sandbox(&provider, "sandbox", "hello", "dir", None)
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
        let result = Execution {
            exit_code: 0,
            stdout: "/src/dir/main.rs:1:hello\n".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let entries = grep_in_sandbox(&provider, "sandbox", "hello", "dir", Some("*.rs"))
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
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let entries = grep_in_sandbox(&provider, "sandbox", "hello", "dir", None)
            .await
            .expect("grep");

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn grep_in_sandbox_invalid_pattern() {
        let result = Execution {
            exit_code: 2,
            stdout: String::new(),
            stderr: "grep: Unmatched [".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = grep_in_sandbox(&provider, "sandbox", "[", "dir", None)
            .await
            .expect_err("invalid pattern");
        match error.failure {
            ToolFailure::InvalidPattern { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn grep_in_sandbox_missing_path() {
        let result = Execution {
            exit_code: 2,
            stdout: String::new(),
            stderr: "grep: /src/dir: No such file or directory".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let error = grep_in_sandbox(&provider, "sandbox", "hello", "dir", None)
            .await
            .expect_err("missing path");
        match error.failure {
            ToolFailure::NotFound { path } => assert_eq!(path, "/src/dir"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_grep_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Grep,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn map_glob_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Glob,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn classify_read_failure_permission_denied() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/secret: Permission denied".to_string(),
        };
        let error = ToolError::classify(Tool::Read, "/src/secret", &result);
        match error.failure {
            ToolFailure::PermissionDenied { path } => assert_eq!(path, "/src/secret"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn classify_read_failure_invalid_path() {
        let result = Execution {
            exit_code: 1,
            stdout: String::new(),
            stderr: "cat: /src/dir: Is a directory".to_string(),
        };
        let error = ToolError::classify(Tool::Read, "/src/dir", &result);
        match error.failure {
            ToolFailure::Failed { path, message } => {
                assert_eq!(path, "/src/dir");
                assert!(message.contains("Is a directory"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn map_read_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Read,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn map_write_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Write,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn map_grep_error_invalid_pattern_message() {
        let error = ToolError::invalid_pattern(Tool::Grep, "[", "Unmatched [").into_mcp();
        assert!(error.to_string().contains("invalid grep pattern"));
    }

    #[tokio::test]
    async fn bash_in_sandbox_success() {
        let result = Execution {
            exit_code: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
        };
        let last_command = Arc::new(Mutex::new(None));
        let provider = TestProvider::new(Ok(result), Arc::clone(&last_command));
        let output = bash_in_sandbox(&provider, "sandbox", "echo ok", None, None)
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
        let result = Execution {
            exit_code: 2,
            stdout: String::new(),
            stderr: "fail".to_string(),
        };
        let provider = TestProvider::new(Ok(result), Arc::new(Mutex::new(None)));
        let output = bash_in_sandbox(&provider, "sandbox", "false", None, None)
            .await
            .expect("bash");

        assert_eq!(output.exit_code, 2);
        assert_eq!(output.stderr, "fail");
    }

    #[test]
    fn build_bash_command_with_workdir() {
        let command = build_bash_command("ls", Some("dir"), None, "/src");
        assert!(command.contains("cd '/src/dir'"));
        assert!(command.contains("&& ls"));
    }

    #[test]
    fn build_bash_command_with_timeout() {
        let command = build_bash_command("sleep 5", None, Some(3), "/src");
        assert!(command.starts_with("timeout 3s sh -c"));
        assert!(command.contains("sleep 5"));
    }

    #[test]
    fn build_bash_command_with_workdir_and_timeout() {
        let command = build_bash_command("ls -la", Some("dir"), Some(5), "/src");
        assert!(command.starts_with("timeout 5s sh -c"));
        assert!(command.contains("/src/dir"));
        assert!(command.contains("ls -la"));
    }

    #[test]
    fn resolve_sandbox_path_relative() {
        assert_eq!(resolve_sandbox_path("/src", "README.md"), "/src/README.md");
        assert_eq!(resolve_sandbox_path("/src", "/etc/hosts"), "/etc/hosts");
    }

    #[test]
    fn shell_escape_handles_quotes() {
        assert_eq!(shell_escape("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn map_bash_error_missing_sandbox() {
        let error = ToolError::sandbox(
            Tool::Bash,
            Error::NotFound {
                name: "missing".to_string(),
            },
        )
        .into_mcp();
        assert!(error.to_string().contains("not found"));
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
            snapshot_message(&SnapshotTrigger::Bash),
            "bash",
            "a command line never enters the branch history"
        );
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
