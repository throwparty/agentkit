pub mod conversation;
pub mod quota;

use crate::config::BillingModel;
use crate::credential::ResolvedCredential;
use crate::domain::http::HttpEndpoint;
use axum::http::{HeaderMap, HeaderValue};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct OpenAiProvider;

impl HttpEndpoint for OpenAiProvider {
    fn build_url(&self, base_url: &str, _body: &Value, billing: &BillingModel) -> String {
        if matches!(billing, BillingModel::Subscription) {
            format!("{}/responses", base_url.trim_end_matches('/'))
        } else {
            format!("{}/chat/completions", base_url.trim_end_matches('/'))
        }
    }

    fn inject_headers(&self, headers: &mut HeaderMap, credential: &ResolvedCredential, billing: &BillingModel) {
        headers.remove("authorization");
        if !matches!(credential.source, crate::credential::CredentialSource::None) {
            headers.insert(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {}", credential.value)).unwrap(),
            );
        }

        if matches!(billing, BillingModel::Subscription) {
            headers.insert(
                "OpenAI-Beta",
                HeaderValue::from_static("responses=experimental"),
            );
            headers.insert(
                "originator",
                HeaderValue::from_static("agentkit-switchboard"),
            );
            if let Some(ref oauth) = credential.oauth {
                if let Some(ref account_id) = oauth.account_id {
                    if let Ok(hv) = HeaderValue::from_str(account_id) {
                        headers.insert("ChatGPT-Account-Id", hv);
                    }
                }
            }
        }
    }
}
