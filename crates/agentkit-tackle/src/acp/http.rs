//! The ACP HTTP transport (T-036): JSON-RPC over HTTP POSTs with
//! Server-Sent Events for agent-initiated notifications, served by
//! axum. Multiple client connections live in one process — each
//! `initialize`-carrying POST without a connection header opens one.
//!
//! Wire shape: `POST /rpc` carries one JSON-RPC message and, for
//! requests, blocks on the response; `GET /events?connection=<id>`
//! streams the agent-initiated lines as SSE `data:` frames. The
//! response to a connection-opening POST carries the
//! `x-acp-connection` header; subsequent POSTs pass
//! `?connection=<id>`.

use crate::acp::{run_agent_over, ConnectionNegotiation, TackleState};
use axum::extract::{Query, State as AxumState};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::{get, post};
use futures::SinkExt as _;
use futures::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot};

/// An idle connection closes after this long without a POST; the
/// connection's agent task ends with it.
const CONNECTION_IDLE: std::time::Duration = std::time::Duration::from_secs(300);

struct Connection {
    /// Client → agent lines (POST bodies).
    incoming: mpsc::Sender<String>,
    /// Agent → client lines, broadcast to the SSE stream.
    outgoing: broadcast::Sender<String>,
    /// Requests awaiting their response line.
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    last_seen: Mutex<std::time::Instant>,
}

impl Connection {
    fn touch(&self) {
        *self.last_seen.lock().unwrap() = std::time::Instant::now();
    }
}

/// Routes each line the agent writes: a response resolves its pending
/// POST; anything else broadcasts to the SSE stream.
async fn outgoing_router(
    mut lines: futures::channel::mpsc::Receiver<String>,
    connection: Arc<Connection>,
) {
    while let Some(line) = lines.next().await {
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        if parsed.get("method").is_some() {
            // Agent-initiated notifications (and requests) broadcast.
            let _ = connection.outgoing.send(line);
            continue;
        }
        let id = match parsed.get("id") {
            Some(serde_json::Value::String(value)) => Some(value.clone()),
            Some(other) => Some(other.to_string()),
            None => None,
        };
        if let Some(id) = id {
            let pending = connection.pending.lock().unwrap().remove(&id);
            if let Some(responder) = pending {
                let _ = responder.send(line);
                continue;
            }
        }
        let _ = connection.outgoing.send(line);
    }
}

fn spawn_connection(server: &HttpServerState, id: String) -> Arc<Connection> {
    let (incoming, incoming_rx) = mpsc::channel::<String>(64);
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<String>(256);
    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let connection = Arc::new(Connection {
        incoming,
        outgoing: broadcast_tx,
        pending: Arc::clone(&pending),
        last_seen: Mutex::new(std::time::Instant::now()),
    });

    // The agent's outgoing bridge: a futures Sink into the router.
    let (agent_out_tx, agent_out_rx) = futures::channel::mpsc::channel::<String>(64);
    tokio::spawn(outgoing_router(agent_out_rx, Arc::clone(&connection)));

    // The incoming bridge: POST bodies as a line stream.
    let incoming_stream =
        tokio_stream::wrappers::ReceiverStream::new(incoming_rx).map(Ok::<_, std::io::Error>);

    let negotiation = Arc::new(ConnectionNegotiation::default());
    let sink = agent_out_tx.sink_map_err(send_error_to_io);
    let transport = agent_client_protocol::Lines::new(sink, incoming_stream);
    let state_for_connection = Arc::clone(&server.state);
    tokio::spawn(async move {
        let _ = run_agent_over(state_for_connection, negotiation, transport).await;
    });

    // Hold the incoming sender open in the registry: dropped senders
    // close the agent connection.
    server
        .connections
        .lock()
        .unwrap()
        .insert(id, Arc::clone(&connection));
    connection
}

async fn dispatch_and_await(connection: &Connection, body: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let id = match parsed.get("id") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(other) => Some(other.to_string()),
        None => None,
    };
    let (response_tx, response_rx) = oneshot::channel::<String>();
    if let Some(id) = &id {
        connection
            .pending
            .lock()
            .unwrap()
            .insert(id.clone(), response_tx);
    }
    let _ = connection.incoming.send(body.to_owned()).await;
    match id {
        Some(_) => response_rx.await.unwrap_or_else(|_| "{}".to_owned()),
        None => "{}".to_owned(),
    }
}

async fn handle_rpc(
    AxumState(server): AxumState<HttpServerState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> axum::response::Response {
    let existing = headers
        .get("x-acp-connection")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| params.get("connection").cloned());

    let (connection, new_id) =
        match existing.and_then(|id| server.connections.lock().unwrap().get(&id).cloned()) {
            Some(connection) => {
                connection.touch();
                (connection, None)
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let connection = spawn_connection(&server, id.clone());
                (connection, Some(id))
            }
        };

    let response = dispatch_and_await(&connection, &body).await;
    let mut builder = axum::http::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(id) = new_id {
        builder = builder.header("x-acp-connection", id);
    }
    builder
        .body(response)
        .map(|response| response.map(axum::body::Body::from))
        .unwrap_or_else(|_| axum::http::Response::new(axum::body::Body::from("{}".to_owned())))
}

async fn handle_events(
    AxumState(server): AxumState<HttpServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Sse<impl futures::Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let stream: futures::stream::BoxStream<'static, Result<SseEvent, std::convert::Infallible>> =
        match params
            .get("connection")
            .and_then(|id| server.connections.lock().unwrap().get(id).cloned())
        {
            Some(connection) => {
                let rx = connection.outgoing.subscribe();
                tokio_stream::wrappers::BroadcastStream::new(rx)
                    .filter_map(|line| {
                        futures::future::ready(
                            line.ok().map(|line| Ok(SseEvent::default().data(line))),
                        )
                    })
                    .boxed()
            }
            None => futures::stream::empty().boxed(),
        };
    Sse::new(stream)
}

#[derive(Clone)]
struct HttpServerState {
    state: Arc<TackleState>,
    connections: Arc<Mutex<HashMap<String, Arc<Connection>>>>,
}

fn send_error_to_io(_error: futures::channel::mpsc::SendError) -> std::io::Error {
    std::io::Error::other("connection closed")
}

/// Serves the ACP agent over HTTP until the listener closes: multiple
/// client connections in one process.
pub async fn run_http(
    state: Arc<TackleState>,
    bind: &str,
    port: u16,
) -> agent_client_protocol::Result<()> {
    let server_state = HttpServerState {
        state,
        connections: Arc::new(Mutex::new(HashMap::new())),
    };

    // The idle reaper: connections without POSTs for CONNECTION_IDLE
    // close (the registry entry's drop ends the agent connection).
    let reaper_state = server_state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let stale: Vec<String> = reaper_state
                .connections
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, connection)| {
                    connection.last_seen.lock().unwrap().elapsed() > CONNECTION_IDLE
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                reaper_state.connections.lock().unwrap().remove(&id);
            }
        }
    });

    let app = axum::Router::new()
        .route("/rpc", post(handle_rpc))
        .route("/events", get(handle_events))
        .with_state(server_state);

    let listener = tokio::net::TcpListener::bind((bind, port))
        .await
        .map_err(|err| agent_client_protocol::Error::internal_error().data(err.to_string()))?;
    axum::serve(listener, app)
        .await
        .map_err(|err| agent_client_protocol::Error::internal_error().data(err.to_string()))
}
