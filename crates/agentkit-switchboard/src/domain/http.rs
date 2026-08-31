use crate::credential::ResolvedCredential;
use axum::http::HeaderMap;
use serde_json::Value;

pub trait HttpEndpoint: Send + Sync {
    fn build_url(&self, base_url: &str, body: &Value) -> String;
    fn inject_headers(&self, headers: &mut HeaderMap, credential: &ResolvedCredential);
}
