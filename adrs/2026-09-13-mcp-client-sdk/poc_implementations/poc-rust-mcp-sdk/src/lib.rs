use async_trait::async_trait;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_client::{ClientHandler, McpClientOptions, client_runtime};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, ContentBlock, Implementation,
};
use rust_mcp_sdk::{
    ClientDetails, McpClient, StdioTransport, ToMcpClientHandler, TransportOptions,
};

/// Structured result of a successful PoC run, matching the T-001 output shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PocReport {
    pub tool_count: usize,
    pub first_five_tool_names: Vec<String>,
    pub tool_descriptions: Vec<String>,
    pub tool_input_schemas: Vec<String>,
    pub echo_text: String,
    pub fail_tool_error: String,
    pub unknown_tool_error: String,
}

impl PocReport {
    /// Serialise the report in the fixed output shape shared by both PoCs.
    pub fn to_fixed_shape(&self) -> String {
        format!(
            "handshake=complete\ntool_count={}\nfirst_five_tools={}\ntool_descriptions={}\ntool_input_schemas={}\necho_result={}\nfail_tool_error={}\nunknown_tool_error={}\n",
            self.tool_count,
            self.first_five_tool_names.join(","),
            self.tool_descriptions.join(";"),
            self.tool_input_schemas.join("|"),
            self.echo_text,
            self.fail_tool_error,
            self.unknown_tool_error,
        )
    }
}

/// Client handler with no overridden behaviour; server-to-client requests are
/// answered with `method_not_found` by the SDK defaults.
pub struct DefaultClientHandler;

#[async_trait]
impl ClientHandler for DefaultClientHandler {}

/// Run the full PoC flow against the shared `mcp-server` binary.
///
/// The server is spawned via `StdioTransport::create_with_server_launch` with
/// an explicit argv array (no shell). The client starts, completes discovery
/// over the current protocol version, lists tools, and calls the echo tool.
pub async fn run_client() -> SdkResult<PocReport> {
    let program = mcp_server::server_program();
    let args = mcp_server::server_args();

    let client_details = ClientDetails {
        client_info: Implementation {
            name: "poc-rust-mcp-sdk".into(),
            version: "0.1.0".into(),
            description: None,
            icons: vec![],
            title: None,
            website_url: None,
        },
        capabilities: ClientCapabilities::default(),
    };

    let transport = StdioTransport::create_with_server_launch(
        program.to_string_lossy().to_string(),
        args,
        None,
        TransportOptions::default(),
    )?;

    let client = client_runtime::create_client(McpClientOptions::new(
        client_details,
        transport,
        DefaultClientHandler.to_mcp_client_handler(),
    ));
    client.clone().start().await?;

    client.request_discover(Default::default()).await?;

    let tools = client.request_tool_list(None).await?.tools;
    let tool_count = tools.len();

    let mut first_five_tool_names = Vec::new();
    let mut tool_descriptions = Vec::new();
    let mut tool_input_schemas = Vec::new();
    for tool in tools.iter().take(5) {
        first_five_tool_names.push(tool.name.clone());
        tool_descriptions.push(tool.description.clone().unwrap_or_default());
        tool_input_schemas.push(serde_json::to_string(&tool.input_schema).unwrap_or_default());
    }

    let echo_result = client
        .request_tool_call(CallToolRequestParams {
            name: "echo".into(),
            arguments: Some(
                serde_json::json!({ "message": "Hello, MCP!" })
                    .as_object()
                    .expect("literal json object")
                    .clone(),
            ),
            input_responses: None,
            request_state: None,
            meta: Default::default(),
        })
        .await?;
    let echo_text = extract_text(&echo_result)?;

    // Tool-level error: `fail` returns a `tools/call` result with `isError`,
    // so its text is surfaced rather than treated as a successful tool call.
    let fail_result = client
        .request_tool_call(CallToolRequestParams {
            name: "fail".into(),
            arguments: None,
            input_responses: None,
            request_state: None,
            meta: Default::default(),
        })
        .await?;
    let fail_tool_error = extract_text(&fail_result)?;

    // Protocol-level error: an unknown tool is rejected by the server and
    // surfaced as an error from the client.
    let unknown_tool_error = match client
        .request_tool_call(CallToolRequestParams {
            name: "no_such_tool".into(),
            arguments: None,
            input_responses: None,
            request_state: None,
            meta: Default::default(),
        })
        .await
    {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };

    client.shut_down().await?;

    Ok(PocReport {
        tool_count,
        first_five_tool_names,
        tool_descriptions,
        tool_input_schemas,
        echo_text,
        fail_tool_error,
        unknown_tool_error,
    })
}

/// Extract the first text content block from a `tools/call` result.
fn extract_text(result: &rust_mcp_sdk::schema::CallToolResult) -> SdkResult<String> {
    let Some(ContentBlock::TextContent(text)) = result.content.first() else {
        return Err(rust_mcp_sdk::error::McpSdkError::RpcError(
            rust_mcp_sdk::schema::RpcError::internal_error()
                .with_message("echo tool returned no text content".to_string()),
        ));
    };
    Ok(text.text.clone())
}
