use axum::{
    routing::post,
    Router as AxumRouter,
    body::Body,
    extract::{State, Request},
    http::StatusCode,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::jsonrpc::JsonRpcRequest;
use crate::session::store::SessionStore;
use crate::handlers::Router;

#[derive(Clone)]
struct AppState {
    router: Router,
}

/// Run the HTTP transport using axum
pub async fn run_http(bind: String, port: u16) {
    let session_store = SessionStore::new();
    let router = Router::new(session_store);

    let app_state = AppState { router };
    let app = AxumRouter::new()
        .route("/", post(handle_request))
        .with_state(Arc::new(Mutex::new(app_state)));

    let address = format!("{}:{}", bind, port);
    eprintln!("Starting HTTP server on {}", address);

    let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handle_request(
    State(state): State<Arc<Mutex<AppState>>>,
    request: Request<Body>,
) -> (StatusCode, String) {
    let body_bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();

    let json_body: serde_json::Value = serde_json::from_slice(&body_bytes)
        .unwrap_or_else(|_| json!({}));

    let id = json_body.get("id").cloned();
    let method = json_body.get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let json_request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method,
        params: json_body.get("params").cloned(),
        id: id.clone(),
    };

    let response = state.lock().await.router.route(&json_request).await;
    let response_str = serde_json::to_string(&response).unwrap();
    (StatusCode::OK, response_str)
}
