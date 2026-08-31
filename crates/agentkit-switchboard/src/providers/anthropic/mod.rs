pub mod quota;

use crate::credential::ResolvedCredential;
use crate::domain::http::HttpEndpoint;
use axum::http::{HeaderMap, HeaderValue};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct AnthropicProvider;

impl HttpEndpoint for AnthropicProvider {
    fn build_url(&self, base_url: &str, _body: &Value) -> String {
        format!("{}/messages", base_url.trim_end_matches('/'))
    }

    fn inject_headers(&self, headers: &mut HeaderMap, credential: &ResolvedCredential) {
        headers.remove("authorization");
        if !matches!(credential.source, crate::credential::CredentialSource::None) {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&credential.value).unwrap(),
            );
            headers.insert(
                "anthropic-version",
                HeaderValue::from_static("2023-06-01"),
            );
        }
    }
}
