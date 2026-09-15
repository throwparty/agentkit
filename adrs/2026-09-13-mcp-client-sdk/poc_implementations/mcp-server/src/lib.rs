use std::path::PathBuf;
use std::process::Stdio;

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    service::serve_server,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EchoArgs {
    pub message: String,
}

pub struct EchoServer {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl EchoServer {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(name = "echo", description = "Echo the provided message")]
    async fn echo(
        &self,
        Parameters(args): Parameters<EchoArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "Echo: {}",
            args.message
        ))]))
    }

    #[tool(name = "fail", description = "Always fails with a tool-level error")]
    async fn fail(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::error(vec![ContentBlock::text(
            "fail tool always returns an error".to_string(),
        )]))
    }
}

/// A server with no tools, used to exercise a client listing an empty tool set.
pub struct EmptyServer;

#[tool_router(allow_empty, server_handler)]
impl EmptyServer {}

#[tool_handler]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Shared reference MCP server for the client-SDK PoCs")
    }
}

/// Serve the echo server over stdio until stdin closes.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let (stdin, stdout) = stdio();
    let running = serve_server(EchoServer::new(), (stdin, stdout)).await?;
    running.waiting().await?;
    Ok(())
}

/// Serve a tool-less server over stdio until stdin closes.
pub async fn serve_stdio_empty() -> Result<(), Box<dyn std::error::Error>> {
    let (stdin, stdout) = stdio();
    let running = serve_server(EmptyServer, (stdin, stdout)).await?;
    running.waiting().await?;
    Ok(())
}

/// Return a `tokio::process::Command` that spawns the shared `mcp-server`
/// binary with its stdio piped, ready to hand to any client transport.
///
/// Locates the compiled binary in the workspace target dir; the binary is
/// built ahead of time by `run-pocs.sh` (`cargo build --workspace`), never by
/// this library.
pub fn server_command() -> Command {
    let binary = mcp_server_binary();
    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // EC-006: a client-supplied server command is untrusted; do not forward
    // the harness's environment or secrets to the child.
    cmd.env_clear();
    cmd
}

/// Return a `tokio::process::Command` that spawns the shared `mcp-server`
/// binary serving streamable HTTP on a listener bound to `addr`.
pub fn server_http_command(addr: std::net::SocketAddr) -> Command {
    let binary = mcp_server_binary();
    let mut cmd = Command::new(binary);
    cmd.arg("--http");
    cmd.arg(addr.to_string());
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_clear();
    cmd
}

/// Serve the echo server over streamable HTTP on `listener`, returning the
/// base URL and a cancellation token that stops the server when cancelled.
pub async fn serve_http(listener: tokio::net::TcpListener) -> (String, CancellationToken) {
    let ct = CancellationToken::new();
    let addr = listener.local_addr().expect("listener has an address");
    let url = format!("http://{addr}/mcp");

    let service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_cancellation_token(ct.clone()),
        );
    let router = axum::Router::new().nest_service("/mcp", service);

    tokio::spawn({
        let ct = ct.clone();
        async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                .await;
        }
    });

    (url, ct)
}

/// Return the path to the shared `mcp-server` binary.
pub fn server_program() -> PathBuf {
    mcp_server_binary()
}

/// Return the argv the shared `mcp-server` binary expects (none).
pub fn server_args() -> Vec<String> {
    Vec::new()
}

/// Locate the compiled `mcp-server` binary in the workspace target dir.
///
/// The binary is built ahead of time by `run-pocs.sh` (`cargo build
/// --workspace`); this library never invokes the build.
fn mcp_server_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().expect("workspace parent");
    workspace_root.join("target").join("debug").join("mcp-server")
}
