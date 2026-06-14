use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use crate::config::BillingModel;
use crate::credential::ResolvedCredential;
use crate::proxy::translation::{self, ProviderKind};

fn determine_provider_kind(billing: &BillingModel) -> ProviderKind {
    match billing {
        BillingModel::Subscription => ProviderKind::ResponsesApi,
        _ => ProviderKind::ChatCompletions,
    }
}

fn rewrite_url(base_url: &str, kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::ResponsesApi => format!("{}/responses", base_url.trim_end_matches('/')),
        ProviderKind::ChatCompletions => format!("{}/chat/completions", base_url.trim_end_matches('/')),
    }
}

fn rewrite_uri_path(base_url: &str, kind: &ProviderKind) -> String {
    rewrite_url(base_url, kind)
}

fn inject_headers(headers: &mut HeaderMap, credential: &ResolvedCredential, kind: &ProviderKind) {
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", credential.value)).unwrap(),
    );
    headers.remove("authorization");

    if *kind == ProviderKind::ResponsesApi {
        headers.insert("OpenAI-Beta", HeaderValue::from_static("responses=experimental"));
        headers.insert("originator", HeaderValue::from_static("agentkit-switchboard"));
        if let Some(ref oauth) = credential.oauth {
            if let Some(ref account_id) = oauth.refresh_token {
                if let Ok(hv) = HeaderValue::from_str(account_id) {
                    headers.insert("ChatGPT-Account-Id", hv);
                }
            }
        }
    }
}

pub async fn forward_request(
    method: Method,
    _uri: &str,
    headers: HeaderMap,
    body: axum::body::Bytes,
    credential: &ResolvedCredential,
    billing: &BillingModel,
    base_url: &str,
) -> Response {
    let kind = determine_provider_kind(billing);
    let target_url = rewrite_uri_path(base_url, &kind);

    let request_body = if kind == ProviderKind::ResponsesApi {
        let parsed: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response();
            }
        };
        match translation::translate_request(&parsed, ProviderKind::ResponsesApi) {
            Ok(translated) => serde_json::to_vec(&translated).unwrap_or_default(),
            Err(translation::TranslationError::StreamingNotSupported) => {
                return (StatusCode::BAD_REQUEST, "streaming not supported for subscription providers").into_response();
            }
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "request translation failed").into_response();
            }
        }
    } else {
        body.to_vec()
    };

    let mut out_headers = HeaderMap::new();
    for (key, value) in headers.iter() {
        let key_str = key.as_str().to_lowercase();
        if key_str != "authorization" && key_str != "host" {
            out_headers.insert(key.clone(), value.clone());
        }
    }
    inject_headers(&mut out_headers, credential, &kind);

    let client = reqwest::Client::new();
    let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST);

    let reqwest_resp = match client
        .request(req_method, &target_url)
        .headers(reqwest::header::HeaderMap::from_iter(
            out_headers
                .iter()
                .map(|(k, v)| {
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
            return (StatusCode::BAD_GATEWAY, format!("upstream request failed: {e}")).into_response();
        }
    };

    let status = reqwest_resp.status();
    let resp_headers = reqwest_resp.headers().clone();
    let resp_body = reqwest_resp.bytes().await.unwrap_or_default();

    let mut response_headers = HeaderMap::new();
    for (key, value) in resp_headers.iter() {
        let key_str = key.as_str().to_lowercase();
        if key_str != "transfer-encoding" && key_str != "connection" {
            response_headers.insert(key.clone(), value.clone());
        }
    }
    response_headers.insert(
        "X-Switchboard-Provider",
        HeaderValue::from_static(""),
    );
    response_headers.insert(
        "X-Switchboard-Billing",
        HeaderValue::from_str(&billing.to_string()).unwrap(),
    );

    let body_bytes = if kind == ProviderKind::ResponsesApi && status.is_success() {
        let parsed: serde_json::Value = match serde_json::from_slice(&resp_body) {
            Ok(v) => v,
            Err(_) => return (status, resp_body).into_response(),
        };
        match translation::translate_response(&parsed, ProviderKind::ResponsesApi) {
            Ok(translated) => serde_json::to_vec(&translated).unwrap_or_default(),
            Err(_) => resp_body.to_vec(),
        }
    } else {
        resp_body.to_vec()
    };

    (status, response_headers, axum::body::Body::from(body_bytes)).into_response()
}
