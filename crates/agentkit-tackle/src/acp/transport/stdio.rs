//! Transport entry points shared by every protocol version.
//!
//! The stdio launch lives here rather than in a version module because the
//! transport is version-agnostic: it hands the connection to the negotiated
//! implementation and carries whichever version results.

use crate::acp::common::connection::ConnectionSetup;
use crate::acp::run_acp_over;
use crate::state::TackleState;
use agent_client_protocol::Lines;
use futures::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use futures::StreamExt as _;
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

/// Launches the ACP agent over stdio, one connection per process. Ctrl-C and
/// SIGTERM end the transport cleanly.
///
/// The first line is inspected before the version router sees it: a malformed
/// `initialize` must be answered with `-32602`, and the router's rejection path
/// only flushes when stdin reaches EOF, which a live client never does.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let setup = ConnectionSetup::load(&state).await;

    let mut reader = futures::io::BufReader::new(tokio::io::stdin().compat());
    let stdout = tokio::io::stdout().compat_write();

    let mut first = String::new();
    let read = tokio::select! {
        result = reader.read_line(&mut first) => result.map_err(io_error)?,
        _ = crate::cli::wait_for_shutdown() => return Ok(()),
    };
    if read == 0 {
        return Ok(());
    }
    if let Some(response) = malformed_initialize_response(&first) {
        let mut out = stdout;
        out.write_all(response.as_bytes()).await.map_err(io_error)?;
        out.flush().await.map_err(io_error)?;
        return Ok(());
    }

    let incoming = Box::pin(
        futures::stream::once(async move { Ok::<String, std::io::Error>(first) })
            .chain(reader.lines()),
    ) as std::pin::Pin<Box<dyn futures::Stream<Item = std::io::Result<String>> + Send>>;
    let outgoing = Box::pin(futures::sink::unfold(
        stdout,
        async move |mut writer, line: String| {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
            Ok::<_, std::io::Error>(writer)
        },
    )) as std::pin::Pin<Box<dyn futures::Sink<String, Error = std::io::Error> + Send>>;

    tokio::select! {
        result = run_acp_over(state, setup, Lines::new(outgoing, incoming)) => result,
        _ = crate::cli::wait_for_shutdown() => Ok(()),
    }
}

fn io_error(error: std::io::Error) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(error.to_string())
}

/// A `-32602` reply for an `initialize` whose `protocolVersion` is missing,
/// non-integer, or out of range; `None` for every other first line.
fn malformed_initialize_response(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("method").and_then(|method| method.as_str()) != Some("initialize") {
        return None;
    }
    let valid = value
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(|version| version.as_u64())
        .is_some_and(|version| version <= u16::MAX as u64);
    if valid {
        return None;
    }
    let id = value
        .get("id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32602,
                "message": "Invalid params",
                "data": "initialize.protocolVersion must be a valid ACP protocol version"
            }
        })
        .to_string()
            + "\n",
    )
}
