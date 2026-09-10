mod pdk;

use anyhow::{Result, anyhow};
use pdk::types::*;
use serde_json::json;
use std::fs;
use std::path::Path;

pub(crate) fn list_tools(_input: ListToolsRequest) -> Result<ListToolsResult> {
    use serde_json::Map;

    let mut properties = Map::new();
    properties.insert(
        "path".to_string(),
        json!({
            "type": "string",
            "description": "Absolute path to the target file"
        }),
    );
    properties.insert(
        "content".to_string(),
        json!({
            "type": "string",
            "description": "Content to write to the file"
        }),
    );

    Ok(ListToolsResult {
        tools: vec![Tool {
            name: "write_file".to_string(),
            description: Some("Write content to a file at the specified path".to_string()),
            input_schema: ToolSchema {
                r#type: ObjectType::Object,
                properties: Some(properties),
                required: Some(vec!["path".to_string(), "content".to_string()]),
            },
            annotations: None,
            output_schema: None,
            title: None,
        }],
        ..Default::default()
    })
}

pub(crate) fn call_tool(input: CallToolRequest) -> Result<CallToolResult> {
    match input.request.name.as_str() {
        "write_file" => {
            let arguments = input
                .request
                .arguments
                .as_ref()
                .ok_or_else(|| anyhow!("arguments required"))?;

            let path_str = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("path argument required"))?;

            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("content argument required"))?;

            // Validate path is absolute
            let path = Path::new(path_str);
            if !path.is_absolute() {
                return Err(anyhow!("path must be absolute"));
            }

            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }

            // Write the file
            fs::write(path, content)?;
            let bytes_written = content.len();

            Ok(CallToolResult {
                content: vec![ContentBlock::Text(TextContent {
                    text: format!("Successfully wrote {} bytes to {}", bytes_written, path_str),
                    r#type: TextType::Text,
                    meta: None,
                    annotations: None,
                })],
                meta: None,
                is_error: None,
                structured_content: None,
            })
        }
        _ => Err(anyhow!("Unknown tool: {}", input.request.name)),
    }
}

pub(crate) fn complete(_input: CompleteRequest) -> Result<CompleteResult> {
    Ok(CompleteResult::default())
}

pub(crate) fn get_prompt(_input: GetPromptRequest) -> Result<GetPromptResult> {
    Err(anyhow!("get_prompt not implemented"))
}

pub(crate) fn list_prompts(_input: ListPromptsRequest) -> Result<ListPromptsResult> {
    Ok(ListPromptsResult::default())
}

pub(crate) fn list_resource_templates(
    _input: ListResourceTemplatesRequest,
) -> Result<ListResourceTemplatesResult> {
    Ok(ListResourceTemplatesResult::default())
}

pub(crate) fn list_resources(_input: ListResourcesRequest) -> Result<ListResourcesResult> {
    Ok(ListResourcesResult::default())
}

pub(crate) fn on_roots_list_changed(_input: PluginNotificationContext) -> Result<()> {
    Ok(())
}

pub(crate) fn read_resource(_input: ReadResourceRequest) -> Result<ReadResourceResult> {
    Err(anyhow!("read_resource not implemented"))
}
