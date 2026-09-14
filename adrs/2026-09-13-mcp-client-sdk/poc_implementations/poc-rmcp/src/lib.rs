use std::process::Stdio;

use rmcp::model::{CallToolRequestParams, ContentBlock, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
use rmcp::transport::TokioChildProcess;
use serde_json::{Map, Value};
use tokio::process::Command;

/// Structured result of a successful PoC run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PocReport {
    pub tool_count: usize,
    pub first_five_tool_names: Vec<String>,
    pub echo_text: String,
}

impl PocReport {
    /// Serialise the report in the fixed output shape shared by both PoCs.
    pub fn to_fixed_shape(&self) -> String {
        format!(
            "handshake=complete\ntool_count={}\nfirst_five_tools={}\necho_result={}\n",
            self.tool_count,
            self.first_five_tool_names.join(","),
            self.echo_text,
        )
    }
}

/// Run the full PoC flow against a server spawned from `server`.
///
/// `server` must be a `tokio::process::Command` with stdin/stdout piped; it is
/// spawned via `TokioChildProcess` (explicit argv, no shell). The client
/// completes the handshake over the current protocol version (2026-07-28 via
/// `server/discover`), lists tools, and calls the echo tool with a JSON
/// message, returning the report.
pub async fn run_client(mut server: Command) -> Result<PocReport, Box<dyn std::error::Error>> {
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

    let tools = client.list_all_tools().await?;
    let tool_count = tools.len();
    let first_five_tool_names: Vec<String> =
        tools.iter().take(5).map(|t| t.name.to_string()).collect();

    let echo_result = client
        .call_tool_once(
            CallToolRequestParams::new("echo").with_arguments(Map::from_iter([(
                "message".to_string(),
                Value::String("Hello, MCP!".to_string()),
            )])),
        )
        .await?;

    let echo_text = extract_text(&echo_result)?;

    client.cancel().await?;

    Ok(PocReport {
        tool_count,
        first_five_tool_names,
        echo_text,
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
