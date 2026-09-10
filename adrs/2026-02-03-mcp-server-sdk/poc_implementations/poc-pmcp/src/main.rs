use async_trait::async_trait;
use pmcp::server::auth::{NoOpAuthProvider, ScopeBasedAuthorizer};
use pmcp::{RequestHandlerExtra, Server, ServerCapabilities, ToolHandler};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct WriteFileResult {
    message: String,
    bytes_written: usize,
}

struct WriteFileTool;

#[async_trait]
impl ToolHandler for WriteFileTool {
    async fn handle(&self, args: Value, _extra: RequestHandlerExtra) -> pmcp::Result<Value> {
        let params: WriteFileArgs = serde_json::from_value(args)
            .map_err(|e| pmcp::Error::validation(format!("Invalid arguments: {}", e)))?;

        let path_buf = Path::new(&params.path);
        if !path_buf.is_absolute() {
            return Err(pmcp::Error::validation("path must be absolute"));
        }

        if let Some(parent) = path_buf.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                pmcp::Error::internal(format!("Failed to create parent directories: {}", e))
            })?;
        }

        fs::write(&params.path, &params.content)
            .await
            .map_err(|e| pmcp::Error::internal(format!("Failed to write file: {}", e)))?;

        Ok(serde_json::to_value(WriteFileResult {
            message: format!(
                "Successfully wrote {} bytes to {}",
                params.content.len(),
                params.path
            ),
            bytes_written: params.content.len(),
        })?)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("🚀 Server starting...");

    let authorizer = ScopeBasedAuthorizer::new()
        .require_scopes("write_file", Vec::<String>::new()) // No scopes required
        .default_scopes(vec!["mcp:tools:use".to_string()]);

    let server = Server::builder()
        .name("poc-pmcp-write-file")
        .version("1.0.0")
        .capabilities(ServerCapabilities::tools_only())
        .auth_provider(NoOpAuthProvider)
        .tool_authorizer(authorizer)
        .tool("write_file", WriteFileTool)
        .build()?;

    eprintln!("✅ Server built, starting stdio...");
    server.run_stdio().await?;
    eprintln!("❌ run_stdio() unexpectedly returned");

    Ok(())
}
