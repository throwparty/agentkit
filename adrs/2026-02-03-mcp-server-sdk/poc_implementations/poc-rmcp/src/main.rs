use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// Absolute path to the target file
    pub path: String,
    /// Full content to write to the file
    pub content: String,
}

#[derive(Clone)]
pub struct WriteFileServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl WriteFileServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Write content to a file at the specified path")]
    async fn write_file(
        &self,
        Parameters(args): Parameters<WriteFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Validate that path is absolute
        let path_buf = Path::new(&args.path);
        if !path_buf.is_absolute() {
            return Err(McpError::invalid_params("path must be absolute", None));
        }

        // Create parent directories if they don't exist
        if let Some(parent) = path_buf.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                McpError::internal_error(
                    format!("Failed to create parent directories: {}", e),
                    None,
                )
            })?;
        }

        // Write content to file
        fs::write(&args.path, &args.content)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to write file: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Successfully wrote {} bytes to {}",
            args.content.len(),
            args.path
        ))]))
    }
}

#[tool_handler]
impl ServerHandler for WriteFileServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("A simple file writing server for testing MCP protocol".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = WriteFileServer::new()
        .serve(stdio())
        .await
        .inspect_err(|e| {
            eprintln!("Error starting server: {}", e);
        })?;
    service.waiting().await?;
    Ok(())
}
