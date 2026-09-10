use async_trait::async_trait;
use prism_mcp_rs::prelude::*;
use prism_mcp_rs::server::McpServer;
use prism_mcp_rs::transport::StdioServerTransport;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;

#[derive(Clone)]
struct WriteFileHandler;

#[async_trait]
impl ToolHandler for WriteFileHandler {
    async fn call(&self, arguments: HashMap<String, Value>) -> McpResult<ToolResult> {
        // Extract arguments
        let path_str = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::validation("path argument required"))?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::validation("content argument required"))?;

        // Validate path is absolute
        let path = Path::new(path_str);
        if !path.is_absolute() {
            return Err(McpError::validation("path must be absolute"));
        }

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                McpError::internal(&format!("Failed to create parent directories: {}", e))
            })?;
        }

        // Write the file
        fs::write(path, content.as_bytes())
            .await
            .map_err(|e| McpError::internal(&format!("Failed to write file: {}", e)))?;

        let bytes_written = content.len();

        Ok(ToolResult {
            content: vec![ContentBlock::text(&format!(
                "Successfully wrote {} bytes to {}",
                bytes_written, path_str
            ))],
            is_error: Some(false),
            meta: None,
            structured_content: None,
        })
    }
}

#[tokio::main]
async fn main() -> McpResult<()> {
    let mut server = McpServer::new("poc-prism-mcp".to_string(), "1.0.0".to_string());

    // Add the write_file tool
    server
        .add_tool(
            "write_file",
            Some("Write content to a file at the specified path"),
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the target file"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
            WriteFileHandler,
        )
        .await?;

    // Start server with STDIO transport
    let transport = StdioServerTransport::new();
    server.start(transport).await
}
