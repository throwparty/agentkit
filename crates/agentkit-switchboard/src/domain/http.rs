use crate::credential::ResolvedCredential;
use axum::http::HeaderMap;
use serde_json::Value;

pub trait HttpEndpoint: Send + Sync {
    fn build_url(&self, base_url: &str, body: &Value) -> String;

    /// Non-credential headers every request to this endpoint carries.
    ///
    /// Applied before request mutations run, so a mutation script may override
    /// any of them. Credentials belong in [`HttpEndpoint::inject_headers`],
    /// which runs afterwards and always wins.
    fn default_headers(&self, _headers: &mut HeaderMap) {}

    fn inject_headers(&self, headers: &mut HeaderMap, credential: &ResolvedCredential);
}
