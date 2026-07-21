use crate::config::BillingModel;
use crate::credential::ResolvedCredential;
use crate::domain::conversation::ConversationHandler;
use crate::domain::http::HttpEndpoint;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde_json::Value;

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

fn build_response(
    status: StatusCode,
    upstream: &reqwest::header::HeaderMap,
    provider_identity: &str,
    billing: &BillingModel,
    session_id: Option<&str>,
    body: axum::body::Body,
    content_type: Option<&'static str>,
) -> Response {
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
            HeaderName::from_bytes(key_str.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            headers.insert(name, value);
        }
    }

    if let Some(ct) = content_type {
        headers.insert("Content-Type", HeaderValue::from_static(ct));
    }
    let prov_val = HeaderValue::from_str(provider_identity).unwrap();
    let bill_val = HeaderValue::from_str(&billing.to_string()).unwrap();
    headers.insert("X-Switchboard-Provider", prov_val);
    headers.insert("X-Switchboard-Billing", bill_val);
    if let Some(sid) = session_id {
        if let Ok(value) = HeaderValue::from_str(sid) {
            headers.insert("X-Switchboard-Session", value);
        }
    }

    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    *resp.headers_mut() = headers;
    resp
}

fn local_outcome(
    status: StatusCode,
    body: String,
    provider_identity: Option<&str>,
    billing: Option<&BillingModel>,
) -> ForwardOutcome {
    let mut headers = HeaderMap::new();
    if let Some(identity) = provider_identity {
        headers.insert(
            "X-Switchboard-Provider",
            HeaderValue::from_str(identity).unwrap(),
        );
    }
    if let Some(b) = billing {
        headers.insert(
            "X-Switchboard-Billing",
            HeaderValue::from_str(&b.to_string()).unwrap(),
        );
    }
    let mut resp = Response::new(axum::body::Body::from(body.clone()));
    *resp.status_mut() = status;
    *resp.headers_mut() = headers;
    ForwardOutcome {
        response: resp,
        status,
        headers: Vec::new(),
        body_text: Some(body),
    }
}

pub async fn forward_request(
    request: ForwardRequest<'_>,
    http: &dyn HttpEndpoint,
    conversation: &dyn ConversationHandler,
) -> ForwardOutcome {
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

    let parsed_body = serde_json::from_slice::<serde_json::Value>(&body).ok();

    let target_url = http.build_url(base_url, parsed_body.as_ref().unwrap_or(&Value::Null), billing);

    let request_body = match parsed_body {
        Some(ref parsed) => match conversation.prepare_request(parsed.clone(), billing) {
            Ok(translated) => serde_json::to_vec(&translated).unwrap_or_default(),
            Err(msg) => {
                return local_outcome(
                    StatusCode::BAD_REQUEST,
                    format!("request translation failed: {msg}"),
                    Some(provider_identity),
                    Some(billing),
                );
            }
        },
        None => body.to_vec(),
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
    http.inject_headers(&mut out_headers, credential, billing);

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
            let mut headers = HeaderMap::new();
            headers.insert(
                "X-Switchboard-Provider",
                HeaderValue::from_str(provider_identity).unwrap(),
            );
            headers.insert(
                "X-Switchboard-Billing",
                HeaderValue::from_str(&billing.to_string()).unwrap(),
            );
            let mut resp = Response::new(axum::body::Body::from(body.clone()));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            *resp.headers_mut() = headers;
            return ForwardOutcome {
                response: resp,
                status: StatusCode::BAD_GATEWAY,
                headers: Vec::new(),
                body_text: Some(body),
            };
        }
    };

    let status = reqwest_resp.status();
    let upstream_headers = upstream_headers(reqwest_resp.headers());
    let upstream_header_map = reqwest_resp.headers().clone();

    let request_streaming = parsed_body
        .as_ref()
        .and_then(|value| value.get("stream"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    if request_streaming && status.is_success() {
        let body = axum::body::Body::from_stream(reqwest_resp.bytes_stream());
        let response = build_response(
            status,
            &upstream_header_map,
            provider_identity,
            billing,
            session_id,
            body,
            None,
        );
        return ForwardOutcome {
            response,
            status,
            headers: upstream_headers,
            body_text: None,
        };
    }

    let raw = reqwest_resp.bytes().await.unwrap_or_default();
    let body_bytes = if status.is_success() {
        serde_json::from_slice::<serde_json::Value>(&raw)
            .ok()
            .and_then(|parsed| conversation.prepare_response(parsed, billing).ok())
            .and_then(|translated| serde_json::to_vec(&translated).ok())
            .unwrap_or_else(|| raw.to_vec())
    } else {
        raw.to_vec()
    };
    let body_text = String::from_utf8_lossy(&body_bytes).to_string();
    let response = build_response(
        status,
        &upstream_header_map,
        provider_identity,
        billing,
        session_id,
        axum::body::Body::from(body_bytes),
        Some("application/json"),
    );

    ForwardOutcome {
        response,
        status,
        headers: upstream_headers,
        body_text: Some(body_text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderMap as ReqwestHeaderMap;

    #[test]
    fn verify_build_response_includes_switchboard() {
        let upstream = ReqwestHeaderMap::new();
        let resp = build_response(
            StatusCode::OK,
            &upstream,
            "test_provider",
            &BillingModel::Subscription,
            None,
            axum::body::Body::empty(),
            Some("application/json"),
        );
        assert_eq!(
            resp.headers().get("x-switchboard-provider").unwrap(),
            "test_provider"
        );
        assert_eq!(
            resp.headers().get("x-switchboard-billing").unwrap(),
            "subscription"
        );
    }
}
