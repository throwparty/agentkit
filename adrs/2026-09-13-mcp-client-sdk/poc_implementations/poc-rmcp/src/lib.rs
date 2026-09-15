use std::process::Stdio;

use rmcp::model::{CallToolRequestParams, ContentBlock, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, RoleClient, RunningService, serve_client_with_lifecycle};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde_json::{Map, Value};
use tokio::process::Command;

/// Structured result of a successful PoC run.
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

/// Run the full PoC flow against a server spawned over stdio from `server`,
/// using the current protocol version for discovery.
pub async fn run_client(server: Command) -> Result<PocReport, Box<dyn std::error::Error>> {
    run_client_with_versions(server, vec![ProtocolVersion::V_2026_07_28]).await
}

/// Run the full PoC flow against a server spawned over stdio from `server`,
/// negotiating from the given preferred protocol versions.
///
/// `server` must be a `tokio::process::Command` with stdin/stdout piped; it is
/// spawned via `TokioChildProcess` (explicit argv, no shell). The client
/// completes the handshake, lists tools, and calls the echo tool with a JSON
/// message, returning the report.
pub async fn run_client_with_versions(
    mut server: Command,
    preferred_versions: Vec<ProtocolVersion>,
) -> Result<PocReport, Box<dyn std::error::Error>> {
    server.stdin(Stdio::piped());
    server.stdout(Stdio::piped());
    server.stderr(Stdio::piped());

    let transport = TokioChildProcess::new(server)?;
    let client = serve_client_with_lifecycle(
        (),
        transport,
        ClientLifecycleMode::Discover { preferred_versions },
    )
    .await?;

    let report = run_flow(&client).await;
    client.cancel().await?;
    report
}

/// Run the full PoC flow against a remote server over streamable HTTP.
pub async fn run_client_http(uri: &str) -> Result<PocReport, Box<dyn std::error::Error>> {
    let transport = StreamableHttpClientTransport::from_uri(uri);
    let client = serve_client_with_lifecycle(
        (),
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await?;

    let report = run_flow(&client).await;
    client.cancel().await?;
    report
}

/// Connect over stdio and return the names of every tool the server exposes.
pub async fn list_tool_names(
    mut server: Command,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    server.stdin(Stdio::piped());
    server.stdout(Stdio::piped());
    server.stderr(Stdio::piped());

    let transport = TokioChildProcess::new(server)?;
    let client = serve_client_with_lifecycle(
        (),
        transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    )
    .await?;

    let names = client
        .list_all_tools()
        .await?
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    client.cancel().await?;
    Ok(names)
}

/// Discover tools, list them, and call the echo tool on an initialised client.
async fn run_flow(
    client: &RunningService<RoleClient, ()>,
) -> Result<PocReport, Box<dyn std::error::Error>> {
    let tools = client.list_all_tools().await?;
    let tool_count = tools.len();

    let mut first_five_tool_names = Vec::new();
    let mut tool_descriptions = Vec::new();
    let mut tool_input_schemas = Vec::new();
    for tool in tools.iter().take(5) {
        first_five_tool_names.push(tool.name.to_string());
        tool_descriptions.push(tool.description.as_deref().unwrap_or_default().to_string());
        tool_input_schemas.push(serde_json::to_string(&*tool.input_schema).unwrap_or_default());
    }

    let echo_result = client
        .call_tool_once(
            CallToolRequestParams::new("echo").with_arguments(Map::from_iter([(
                "message".to_string(),
                Value::String("Hello, MCP!".to_string()),
            )])),
        )
        .await?;
    let echo_text = extract_text(&echo_result)?;

    // Tool-level error: `fail` returns a successful `tools/call` whose result
    // carries `isError: true`, so the error text must be surfaced distinctly.
    let fail_result = client
        .call_tool_once(CallToolRequestParams::new("fail"))
        .await?;
    let fail_tool_error = extract_text(&fail_result)?;

    // Protocol-level error: an unknown tool is rejected by the server with a
    // JSON-RPC error, which the client surfaces as `Err`.
    let unknown_tool_error = match client
        .call_tool_once(CallToolRequestParams::new("no_such_tool"))
        .await
    {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };

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
fn extract_text(
    result: &rmcp::model::CallToolResponse,
) -> Result<String, Box<dyn std::error::Error>> {
    let rmcp::model::CallToolResponse::Complete(result) = result else {
        return Err("echo tool returned a non-complete result".into());
    };
    let Some(ContentBlock::Text(text)) = result.content.first() else {
        return Err("echo tool returned no text content".into());
    };
    Ok(text.text.clone())
}
