use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    req.extensions_mut().insert(RequestId(request_id.clone()));
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("X-Request-Id", request_id.parse().unwrap());
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
