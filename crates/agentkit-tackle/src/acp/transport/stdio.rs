//! Transport entry points shared by every protocol version.
//!
//! The stdio launch lives here rather than in a version module because the
//! transport is version-agnostic: it hands the connection to the negotiated
//! implementation and carries whichever version results.

use crate::acp::common::connection::ConnectionSetup;
use crate::acp::router::SUPPORTED_PROTOCOL_VERSIONS;
use crate::acp::run_acp_over;
use crate::state::TackleState;
use agent_client_protocol::Lines;
use futures::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};
use futures::StreamExt as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

/// Launches the ACP agent over stdio, one connection per process. Ctrl-C and
/// SIGTERM end the transport cleanly.
///
/// The first `initialize` is inspected before the version router sees it: a
/// malformed `protocolVersion` must be answered with `-32602` and one below
/// every version tackle speaks with `-32022`, and the router's rejection path
/// only flushes when stdin reaches EOF, which a live client never does. A
/// rejected `initialize` leaves the connection uninitialized — the client may
/// retry, and the router refuses any other request until one succeeds.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let setup = ConnectionSetup::load(&state).await;

    let mut reader = futures::io::BufReader::new(tokio::io::stdin().compat());
    let mut stdout = tokio::io::stdout().compat_write();

    let Some(first) = await_initialize(&mut reader, &mut stdout).await? else {
        return Ok(());
    };

    let incoming = Box::pin(
        futures::stream::once(async move { Ok::<String, std::io::Error>(first) })
            .chain(reader.lines()),
    )
        as std::pin::Pin<Box<dyn futures::Stream<Item = std::io::Result<String>> + Send>>;

    // A client that stops reading before the harness has finished writing
    // leaves the pipe closed, and the next line fails with `EPIPE`. The
    // client going away is a clean end to the connection, not a fault:
    // the SDK reports a non-zero exit after shutdown as an error, so exit
    // zero rather than surfacing a broken pipe nothing can read.
    let peer_gone = Arc::new(AtomicBool::new(false));
    let outgoing = Box::pin(futures::sink::unfold(
        (stdout, Arc::clone(&peer_gone)),
        async |(mut writer, peer_gone), line: String| {
            let written = async {
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
                Ok::<(), std::io::Error>(())
            }
            .await;
            match written {
                Ok(()) => Ok((writer, peer_gone)),
                Err(error) => {
                    if error.kind() == std::io::ErrorKind::BrokenPipe {
                        peer_gone.store(true, Ordering::Relaxed);
                    }
                    Err(error)
                }
            }
        },
    ))
        as std::pin::Pin<Box<dyn futures::Sink<String, Error = std::io::Error> + Send>>;

    let outcome = tokio::select! {
        result = run_acp_over(state, setup, Lines::new(outgoing, incoming)) => result,
        _ = crate::cli::wait_for_shutdown() => Ok(()),
    };
    if peer_gone.load(Ordering::Relaxed) {
        tracing::debug!("client stopped reading; ending the connection");
        return Ok(());
    }
    outcome
}

fn io_error(error: std::io::Error) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(error.to_string())
}

/// Reads until the client offers an `initialize` the version router can route,
/// answering the ones it cannot along the way.
///
/// A rejected `initialize` leaves the connection uninitialized rather than
/// closed: the spec lets the client retry, so the loop keeps reading. Anything
/// that is not a rejected `initialize` is handed to the router as the
/// connection's first message; the router refuses requests that arrive before
/// initialization and keeps reading for a valid `initialize`.
async fn await_initialize(
    reader: &mut (impl AsyncBufRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> agent_client_protocol::Result<Option<String>> {
    loop {
        let mut line = String::new();
        let read = tokio::select! {
            result = reader.read_line(&mut line) => result.map_err(io_error)?,
            _ = crate::cli::wait_for_shutdown() => return Ok(None),
        };
        if read == 0 {
            return Ok(None);
        }
        let Some(response) = initialize_rejection(&line) else {
            return Ok(Some(line));
        };
        writer
            .write_all(response.as_bytes())
            .await
            .map_err(io_error)?;
        writer.flush().await.map_err(io_error)?;
    }
}

/// The reply owed to an `initialize` the router must not see: `-32602` when
/// `protocolVersion` is missing, non-integer, or out of range, and `-32022`
/// when no version tackles speaks is at or below it. `None` for every other
/// line.
///
/// The router answers a version below v1 by falling back to v1, but a
/// successful reply may not name a version the client did not ask for, so a
/// zero has to be refused before it reaches the router.
fn initialize_rejection(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("method").and_then(|method| method.as_str()) != Some("initialize") {
        return None;
    }
    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let requested = value
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(|version| version.as_u64());
    let reply = |code, message, data| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message, "data": data },
        })
        .to_string()
            + "\n"
    };
    match requested {
        Some(version) if version < SUPPORTED_PROTOCOL_VERSIONS[0] => Some(reply(
            -32022,
            "Unsupported protocol version",
            serde_json::json!({
                "requested": version,
                "supported": SUPPORTED_PROTOCOL_VERSIONS,
            }),
        )),
        Some(version) if version <= u16::MAX as u64 => None,
        _ => Some(reply(
            -32602,
            "Invalid params",
            serde_json::json!("initialize.protocolVersion must be a valid ACP protocol version"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejection(line: serde_json::Value) -> Option<serde_json::Value> {
        initialize_rejection(&line.to_string())
            .map(|reply| serde_json::from_str(reply.trim_end()).expect("a JSON reply"))
    }

    fn initialize(params: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "initialize", "params": params })
    }

    #[test]
    fn a_served_version_reaches_the_router() {
        for version in SUPPORTED_PROTOCOL_VERSIONS {
            assert_eq!(
                rejection(initialize(
                    serde_json::json!({ "protocolVersion": version })
                )),
                None
            );
        }
    }

    #[test]
    fn a_version_below_every_supported_one_is_refused_with_the_supported_set() {
        let reply = rejection(initialize(serde_json::json!({ "protocolVersion": 0 })))
            .expect("an unsupported version is refused");
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["error"]["code"], -32022);
        assert_eq!(reply["error"]["data"]["requested"], 0);
        assert_eq!(
            reply["error"]["data"]["supported"],
            serde_json::json!([1, 2])
        );
    }

    #[test]
    fn a_malformed_version_is_invalid_params() {
        for params in [
            serde_json::json!({}),
            serde_json::json!({ "protocolVersion": "2" }),
            serde_json::json!({ "protocolVersion": -1 }),
            serde_json::json!({ "protocolVersion": 65536 }),
        ] {
            let reply = rejection(initialize(params.clone()))
                .unwrap_or_else(|| panic!("{params} is malformed"));
            assert_eq!(reply["error"]["code"], -32602, "{params}");
        }
    }

    #[test]
    fn anything_that_is_not_an_initialize_is_left_to_the_router() {
        assert_eq!(
            rejection(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "session/new" })),
            None
        );
        assert_eq!(
            rejection(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/update"
            })),
            None
        );
    }
}
