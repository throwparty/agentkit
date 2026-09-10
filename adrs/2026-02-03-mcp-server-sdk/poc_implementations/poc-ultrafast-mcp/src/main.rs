use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use ultrafast_mcp::{
    ListToolsRequest, ListToolsResponse, MCPError, MCPResult, ServerCapabilities, ServerInfo, Tool,
    ToolCall, ToolContent, ToolHandler, ToolResult, ToolsCapability, UltraFastServer,
};

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct WriteFileResponse {
    message: String,
    bytes_written: usize,
}

struct WriteFileHandler;

#[async_trait::async_trait]
impl ToolHandler for WriteFileHandler {
    async fn handle_tool_call(&self, call: ToolCall) -> MCPResult<ToolResult> {
        match call.name.as_str() {
            "write_file" => {
                let args: WriteFileArgs =
                    serde_json::from_value(call.arguments.unwrap_or_default()).map_err(|e| {
                        MCPError::serialization_error(format!("Invalid arguments: {}", e))
                    })?;

                // Validate absolute path
                let path_buf = Path::new(&args.path);
                if !path_buf.is_absolute() {
                    return Err(MCPError::serialization_error(
                        "path must be absolute".to_string(),
                    ));
                }

                // Create parent directories
                if let Some(parent) = path_buf.parent() {
                    fs::create_dir_all(parent).await.map_err(|e| {
                        MCPError::serialization_error(format!(
                            "Failed to create parent directories: {}",
                            e
                        ))
                    })?;
                }

                // Write file
                fs::write(&args.path, &args.content).await.map_err(|e| {
                    MCPError::serialization_error(format!("Failed to write file: {}", e))
                })?;

                let response = WriteFileResponse {
                    message: format!(
                        "Successfully wrote {} bytes to {}",
                        args.content.len(),
                        args.path
                    ),
                    bytes_written: args.content.len(),
                };

                Ok(ToolResult {
                    content: vec![ToolContent::text(serde_json::to_string(&response).unwrap())],
                    is_error: Some(false),
                })
            }
            _ => Err(MCPError::serialization_error(format!(
                "Unknown tool: {}",
                call.name
            ))),
        }
    }

    async fn list_tools(&self, _request: ListToolsRequest) -> MCPResult<ListToolsResponse> {
        Ok(ListToolsResponse {
            tools: vec![Tool {
                name: "write_file".to_string(),
                description: "Write content to a file at the specified path".to_string(),
                input_schema: serde_json::json!({
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
                annotations: None,
                output_schema: None,
            }],
            next_cursor: None,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server_info = ServerInfo {
        name: "poc-ultrafast-mcp".to_string(),
        version: "1.0.0".to_string(),
        description: Some("Simple file writing server for testing MCP protocol".to_string()),
        authors: None,
        homepage: None,
        license: None,
        repository: None,
    };

    let capabilities = ServerCapabilities {
        tools: Some(ToolsCapability {
            list_changed: Some(true),
        }),
        ..Default::default()
    };

    let server = UltraFastServer::new(server_info, capabilities)
        .with_tool_handler(Arc::new(WriteFileHandler));

    server.run_stdio().await?;

    Ok(())
}
