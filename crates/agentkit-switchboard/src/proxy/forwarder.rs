use crate::config::BillingModel;
use crate::credential::{CredentialSource, ResolvedCredential};
use crate::proxy::translation::{self, ProviderKind};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

pub struct ForwardOutcome {
    pub response: Response,
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body_text: Option<String>,
}

pub struct ForwardRequest<'a> {
    pub method: Method,
    pub headers: HeaderMap,
    pub body: axum::body::Bytes,
    pub credential: &'a ResolvedCredential,
    pub billing: &'a BillingModel,
    pub base_url: &'a str,
    pub provider_identity: &'a str,
    pub session_id: Option<&'a str>,
}

fn determine_provider_kind(billing: &BillingModel) -> ProviderKind {
    match billing {
        BillingModel::Subscription => ProviderKind::ResponsesApi,
        _ => ProviderKind::ChatCompletions,
    }
}

fn rewrite_url(base_url: &str, kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::ResponsesApi => format!("{}/responses", base_url.trim_end_matches('/')),
        ProviderKind::ChatCompletions => {
            format!("{}/chat/completions", base_url.trim_end_matches('/'))
        }
    }
}

fn rewrite_uri_path(base_url: &str, kind: &ProviderKind) -> String {
    rewrite_url(base_url, kind)
}

fn inject_headers(headers: &mut HeaderMap, credential: &ResolvedCredential, kind: &ProviderKind) {
    headers.remove("authorization");
    if !matches!(credential.source, CredentialSource::None) {
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", credential.value)).unwrap(),
        );
    }

    if *kind == ProviderKind::ResponsesApi {
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

fn upstream_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn response_headers(
    upstream: &reqwest::header::HeaderMap,
    provider_identity: &str,
    billing: &BillingModel,
    session_id: Option<&str>,
    content_type: Option<&'static str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();

    for (key, value) in upstream {
        let key_str = key.as_str();
        if matches!(
            key_str.to_ascii_lowercase().as_str(),
            "transfer-encoding" | "connection" | "content-length"
        ) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            axum::http::HeaderName::from_bytes(key_str.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(name, value);
        }
    }

    if let Some(content_type) = content_type {
        headers.insert("Content-Type", HeaderValue::from_static(content_type));
    }
    headers.insert(
        "X-Switchboard-Provider",
        HeaderValue::from_str(provider_identity).unwrap(),
    );
    headers.insert(
        "X-Switchboard-Billing",
        HeaderValue::from_str(&billing.to_string()).unwrap(),
    );
    if let Some(session_id) = session_id {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            headers.insert("X-Switchboard-Session", value);
        }
    }

    headers
}

fn local_outcome(status: StatusCode, body: &'static str) -> ForwardOutcome {
    ForwardOutcome {
        response: (status, body).into_response(),
        status,
        headers: Vec::new(),
        body_text: Some(body.to_string()),
    }
}

pub async fn forward_request(request: ForwardRequest<'_>) -> ForwardOutcome {
    let ForwardRequest {
        method,
        headers,
        body,
        credential,
        billing,
        base_url,
        provider_identity,
        session_id,
    } = request;
    let kind = determine_provider_kind(billing);
    let target_url = rewrite_uri_path(base_url, &kind);

    tracing::debug!(%method, %target_url, %billing, "forwarding request");

    let parsed_body = serde_json::from_slice::<serde_json::Value>(&body).ok();
    let request_streaming = parsed_body
        .as_ref()
        .and_then(|value| value.get("stream"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let request_body = if kind == ProviderKind::ResponsesApi {
        let Some(parsed) = parsed_body.as_ref() else {
            return local_outcome(StatusCode::BAD_REQUEST, "invalid JSON body");
        };
        match translation::translate_request(parsed, ProviderKind::ResponsesApi) {
            Ok(translated) => serde_json::to_vec(&translated).unwrap_or_default(),
            Err(translation::TranslationError::StreamingNotSupported) => {
                return local_outcome(
                    StatusCode::BAD_REQUEST,
                    "streaming not supported for subscription providers",
                );
            }
            Err(_) => {
                return local_outcome(StatusCode::BAD_REQUEST, "request translation failed");
            }
        }
    } else {
        body.to_vec()
    };

    let mut out_headers = HeaderMap::new();
    for (key, value) in &headers {
        let key_str = key.as_str().to_ascii_lowercase();
        if key_str != "authorization" && key_str != "host" && key_str != "content-length" {
            out_headers.insert(key.clone(), value.clone());
        }
    }
    if !out_headers.contains_key("content-type") {
        out_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    }
    inject_headers(&mut out_headers, credential, &kind);

    let client = reqwest::Client::new();
    let req_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST);

    let reqwest_resp = match client
        .request(req_method, &target_url)
        .headers(reqwest::header::HeaderMap::from_iter(
            out_headers.iter().map(|(k, v)| {
                (
                    reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()).unwrap(),
                    reqwest::header::HeaderValue::from_bytes(v.as_bytes()).unwrap(),
                )
            }),
        ))
        .body(request_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let body = format!("upstream request failed: {e}");
            return ForwardOutcome {
                response: (StatusCode::BAD_GATEWAY, body.clone()).into_response(),
                status: StatusCode::BAD_GATEWAY,
                headers: Vec::new(),
                body_text: Some(body),
            };
        }
    };

    let status = reqwest_resp.status();
    let upstream_headers = upstream_headers(reqwest_resp.headers());
    let upstream_header_map = reqwest_resp.headers().clone();

    if kind == ProviderKind::ChatCompletions && request_streaming && status.is_success() {
        let response_headers = response_headers(
            reqwest_resp.headers(),
            provider_identity,
            billing,
            session_id,
            None,
        );
        let body = axum::body::Body::from_stream(reqwest_resp.bytes_stream());
        let response = (status, response_headers, body).into_response();
        return ForwardOutcome {
            response,
            status,
            headers: upstream_headers,
            body_text: None,
        };
    }

    let raw = reqwest_resp.bytes().await.unwrap_or_default();
    let body_bytes = if kind == ProviderKind::ResponsesApi && status.is_success() {
        match serde_json::from_slice::<serde_json::Value>(&raw)
            .ok()
            .and_then(|parsed| {
                translation::translate_response(&parsed, ProviderKind::ResponsesApi).ok()
            }) {
            Some(translated) => serde_json::to_vec(&translated).unwrap_or_default(),
            None => raw.to_vec(),
        }
    } else {
        raw.to_vec()
    };
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();
    let response_headers = response_headers(
        &upstream_header_map,
        provider_identity,
        billing,
        session_id,
        Some("application/json"),
    );
    let response = (status, response_headers, axum::body::Body::from(body_bytes)).into_response();

    ForwardOutcome {
        response,
        status,
        headers: upstream_headers,
        body_text: Some(body_text),
    }
}
