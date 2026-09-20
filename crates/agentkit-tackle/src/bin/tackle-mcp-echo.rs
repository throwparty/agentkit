//! A minimal MCP echo server: the pool's happy-path test fixture.
//! Speaks MCP over stdio (rmcp's ServerHandler); its one tool echoes
//! its `text` argument back.

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ContentBlock, ListToolsResult, PaginatedRequestParams,
    TextContent, Tool,
};
use rmcp::{service, ErrorData, RoleServer, ServerHandler, ServiceExt};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Default)]
struct Echo;

impl ServerHandler for Echo {
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "echo",
            "Echoes the provided text",
            Arc::new(serde_json::from_str(
                r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#,
            )
            .unwrap()),
        )])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
        std::future::ready(match request.name.as_ref() {
            "echo" => {
                let text = request
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("text"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_owned();
                Ok(CallToolResponse::from(
                    rmcp::model::CallToolResult::success(vec![ContentBlock::Text(
                        TextContent::new(text),
                    )]),
                ))
            }
            other => Err(ErrorData::invalid_params(
                Cow::Owned(format!("unknown tool: {other}")),
                None,
            )),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = Echo;
    service
        .serve(rmcp::transport::io::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
