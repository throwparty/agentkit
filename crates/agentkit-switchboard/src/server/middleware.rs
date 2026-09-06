use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::KeyValue;

pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    req.extensions_mut().insert(RequestId(request_id.clone()));
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("X-Request-Id", request_id.parse().unwrap());
    resp
}

pub async fn metrics_middleware(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    let status_code = resp.status().as_u16();
    crate::otel::metrics::metrics().http_requests.add(
        1,
        &[
            KeyValue::new("method", method),
            KeyValue::new("path", path),
            KeyValue::new("status_code", status_code.to_string()),
        ],
    );
    resp
}

#[derive(Clone)]
pub struct RequestId(pub String);

pub fn extract_session_id(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("X-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}
