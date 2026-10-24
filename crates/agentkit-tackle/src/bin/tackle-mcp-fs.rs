//! A **test-only** read-only filesystem MCP server fixture.
//!
//! This sits alongside `tackle-mcp-echo` as a fixture for integration tests and
//! manual validation. It is NOT part of Tackle's shipped tool surface: nothing
//! registers it automatically, and it must be configured explicitly as an MCP
//! server. It is read-only and confined to `--root`; paths outside the root are
//! rejected. Do not treat it as production tooling.

use clap::Parser;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ContentBlock, ListToolsResult, PaginatedRequestParams,
    TextContent, Tool,
};
use rmcp::{service, ErrorData, RoleServer, ServerHandler, ServiceExt};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The maximum number of `grep` matches returned in one call.
const GREP_LIMIT: usize = 200;
/// The maximum file size `grep` reads.
const GREP_MAX_FILE_BYTES: u64 = 1 << 20;
/// Directory names skipped by the recursive walk.
const SKIPPED_DIRS: &[&str] = &[
    ".git", ".hg", ".jj", ".svn", "target", "node_modules", ".direnv", ".cache",
];

#[derive(Debug, Parser)]
#[command(name = "tackle-mcp-fs", about = "Test-only read-only filesystem MCP fixture")]
struct Args {
    /// Workspace root the tools are confined to (defaults to the current directory).
    #[arg(long)]
    root: Option<PathBuf>,
}

struct FsServer {
    root: PathBuf,
}

impl FsServer {
    fn new(root: PathBuf) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    fn resolve(&self, raw: Option<&str>) -> Result<PathBuf, String> {
        let raw = raw.unwrap_or(".");
        let candidate = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            self.root.join(raw)
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|err| format!("{}: {err}", candidate.display()))?;
        if !canonical.starts_with(&self.root) {
            return Err(format!(
                "path escapes the workspace root: {}",
                canonical.display()
            ));
        }
        Ok(canonical)
    }

    fn read_file(
        &self,
        args: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        let path = self.resolve(args.get("path").and_then(|value| value.as_str()))?;
        let content =
            std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let offset = args
            .get("offset")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize);
        if offset == 0 && limit.is_none() {
            return Ok(content);
        }
        Ok(content
            .lines()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn list_directory(
        &self,
        args: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        let path = self.resolve(args.get("path").and_then(|value| value.as_str()))?;
        let mut entries: Vec<String> = std::fs::read_dir(&path)
            .map_err(|err| format!("{}: {err}", path.display()))?
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let suffix = if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    "/"
                } else {
                    ""
                };
                format!("{name}{suffix}")
            })
            .collect();
        entries.sort();
        Ok(entries.join("\n"))
    }

    fn grep(&self, args: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "pattern is required".to_owned())?;
        let base = self.resolve(args.get("path").and_then(|value| value.as_str()))?;
        let include = args.get("include").and_then(|value| value.as_str());
        let mut matches = Vec::new();
        walk(&base, &mut |path| {
            if matches.len() >= GREP_LIMIT {
                return;
            }
            if let Some(include) = include {
                if !path
                    .file_name()
                    .map(|name| name.to_string_lossy().ends_with(include))
                    .unwrap_or(false)
                {
                    return;
                }
            }
            if std::fs::metadata(path)
                .map(|meta| meta.len() > GREP_MAX_FILE_BYTES)
                .unwrap_or(true)
            {
                return;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                return;
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    matches.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        index + 1,
                        line.trim_end()
                    ));
                    if matches.len() >= GREP_LIMIT {
                        break;
                    }
                }
            }
        });
        Ok(matches.join("\n"))
    }
}

fn walk(path: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.is_file() {
        visit(path);
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if entry
            .file_type()
            .map(|kind| kind.is_dir())
            .unwrap_or(false)
            && SKIPPED_DIRS.iter().any(|skip| name == *skip)
        {
            continue;
        }
        walk(&entry.path(), visit);
    }
}

fn text_response(text: String) -> CallToolResponse {
    CallToolResponse::from(rmcp::model::CallToolResult::success(vec![
        ContentBlock::Text(TextContent::new(text)),
    ]))
}

impl ServerHandler for FsServer {
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![
            Tool::new(
                "read_file",
                "Reads a UTF-8 file from the workspace. Paths are relative to the fixture root.",
                Arc::new(
                    serde_json::from_str(
                        r#"{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"]}"#,
                    )
                    .unwrap(),
                ),
            ),
            Tool::new(
                "list_directory",
                "Lists the entries of a workspace directory. Paths are relative to the fixture root.",
                Arc::new(
                    serde_json::from_str(
                        r#"{"type":"object","properties":{"path":{"type":"string"}}}"#,
                    )
                    .unwrap(),
                ),
            ),
            Tool::new(
                "grep",
                "Searches workspace files for a literal string, returning path:line matches.",
                Arc::new(
                    serde_json::from_str(
                        r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"include":{"type":"string"}},"required":["pattern"]}"#,
                    )
                    .unwrap(),
                ),
            ),
        ])))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: service::RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        let result = match request.name.as_ref() {
            "read_file" => self.read_file(&args),
            "list_directory" => self.list_directory(&args),
            "grep" => self.grep(&args),
            other => Err(format!("unknown tool: {other}")),
        };
        match result {
            Ok(text) => Ok(text_response(text)),
            Err(message) => Err(ErrorData::invalid_params(Cow::Owned(message), None)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let root = args
        .root
        .unwrap_or(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    FsServer::new(root)
        .serve(rmcp::transport::io::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_outside_the_root_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let server = FsServer::new(dir.path().to_path_buf());
        assert!(server.resolve(Some("../escape")).is_err());
        assert!(server.resolve(Some("inside")).is_err(), "missing paths fail");
    }

    #[test]
    fn reads_and_lists_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello\nworld\n").unwrap();
        let server = FsServer::new(dir.path().to_path_buf());
        let args = serde_json::json!({ "path": "README.md" });
        let args = args.as_object().unwrap().clone();
        assert_eq!(server.read_file(&args).unwrap(), "hello\nworld\n");
        let listed = server.list_directory(&serde_json::Map::new()).unwrap();
        assert!(listed.contains("README.md"));
    }

    #[test]
    fn grep_returns_path_and_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let server = FsServer::new(dir.path().to_path_buf());
        let args = serde_json::json!({ "pattern": "two" });
        let args = args.as_object().unwrap().clone();
        let result = server.grep(&args).unwrap();
        assert!(result.contains("a.txt:2: two"), "{result}");
    }
}
