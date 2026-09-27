use crate::config::BillingModel;
use crate::credential::ResolvedCredential;
use crate::domain::http::HttpEndpoint;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde_json::Value;
use std::sync::OnceLock;

fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

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
    pub provider_user_agent: Option<&'a str>,
    pub provider_headers: Option<&'a std::collections::HashMap<String, String>>,
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

#[tracing::instrument(skip_all, fields(provider_identity = %request.provider_identity))]
pub async fn forward_request(
    request: ForwardRequest<'_>,
    http: &dyn HttpEndpoint,
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
        provider_user_agent,
        provider_headers,
    } = request;

    let parsed_body = serde_json::from_slice::<serde_json::Value>(&body).ok();

    let target_url = http.build_url(base_url, parsed_body.as_ref().unwrap_or(&Value::Null));

    let request_body = body.to_vec();

       let mut out_headers = HeaderMap::new();
       for (key, value) in &headers {
           let key_str = key.as_str().to_ascii_lowercase();
           // Skip headers that are handled specially or could cause conflicts
           if key_str != "authorization"
               && key_str != "host"
               && key_str != "content-length"
           {
               out_headers.insert(key.clone(), value.clone());
           }
       }
      if !out_headers.contains_key("content-type") {
          out_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
      }
      // Use provider-specific user-agent if available, otherwise fall back to default
      let user_agent_value = if let Some(ref ua) = provider_user_agent {
          HeaderValue::from_str(ua).unwrap_or_else(|_| HeaderValue::from_static(concat!(
              "agentkit-switchboard/",
              env!("CARGO_PKG_VERSION")
          )))
      } else {
          HeaderValue::from_static(concat!(
              "agentkit-switchboard/",
              env!("CARGO_PKG_VERSION")
          ))
      };
      out_headers.insert("User-Agent", user_agent_value);

    if let Some(extra) = provider_headers {
        for (name, value) in extra {
            let parsed_name = match HeaderName::from_bytes(name.as_bytes()) {
                Ok(n) => n,
                Err(_) => {
                    tracing::warn!(
                        provider = provider_identity,
                        header = %name,
                        "invalid configured header name, skipping"
                    );
                    continue;
                }
            };
            match HeaderValue::from_str(value) {
                Ok(v) => {
                    out_headers.insert(parsed_name, v);
                }
                Err(_) => {
                    tracing::warn!(
                        provider = provider_identity,
                        header = %name,
                        "invalid configured header value, skipping"
                    );
                }
            }
        }
    }

    http.inject_headers(&mut out_headers, credential);

    let client = shared_client();
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
    let body_bytes = raw.to_vec();
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
    use std::sync::Arc;

    struct MockHttpEndpoint {
        captured_headers: Arc<std::sync::Mutex<Option<reqwest::header::HeaderMap>>>,
    }

    impl MockHttpEndpoint {
        fn new() -> Self {
            Self {
                captured_headers: Arc::new(std::sync::Mutex::new(None)),
            }
        }
    }

    impl HttpEndpoint for MockHttpEndpoint {
        fn build_url(&self, _base_url: &str, _parsed_body: &serde_json::Value) -> String {
            "http://mock-upstream.example.com".to_string()
        }

        fn inject_headers(&self, headers: &mut reqwest::header::HeaderMap, _credential: &ResolvedCredential) {
            *self.captured_headers.lock().unwrap() = Some(headers.clone());
        }
    }

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

    #[tokio::test]
    async fn forwards_correct_user_agent_header() {
        // Arrange
        let mock_endpoint = MockHttpEndpoint::new();
        let credential = ResolvedCredential {
            value: "test-token".to_string(),
            source: crate::credential::CredentialSource::None,
            oauth: None,
        };
        let billing = BillingModel::Subscription;
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("test-client/1.0"));
        headers.insert("authorization", HeaderValue::from_static("Bearer token"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let request = ForwardRequest {
            method: Method::POST,
            headers,
            body: axum::body::Bytes::from(r#"{"test": "data"}"#),
            credential: &credential,
            billing: &billing,
            base_url: "http://example.com",
            provider_identity: "test_provider",
            session_id: None,
            provider_user_agent: None,
            provider_headers: None,
        };

        // Act
        let _ = forward_request(request, &mock_endpoint).await;

        // Assert
        let captured_headers = mock_endpoint.captured_headers.lock().unwrap().clone().unwrap();
        let user_agent = captured_headers.get("user-agent").unwrap();
        let user_agent_str = user_agent.to_str().unwrap();
        assert!(user_agent_str.starts_with("agentkit-switchboard/"));
        // Ensure the original user-agent was filtered out
        assert!(!captured_headers.keys().any(|k| k == "user-agent" && captured_headers.get(k).unwrap() == "test-client/1.0"));
    }

    #[tokio::test]
    async fn forwards_provider_specific_user_agent() {
        // Arrange
        let mock_endpoint = MockHttpEndpoint::new();
        let credential = ResolvedCredential {
            value: "test-token".to_string(),
            source: crate::credential::CredentialSource::None,
            oauth: None,
        };
        let billing = BillingModel::Subscription;
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let request = ForwardRequest {
            method: Method::POST,
            headers,
            body: axum::body::Bytes::from(r#"{"test": "data"}"#),
            credential: &credential,
            billing: &billing,
            base_url: "http://example.com",
            provider_identity: "test_provider",
            session_id: None,
            provider_user_agent: Some("custom-agent/1.0"),
            provider_headers: None,
        };

        // Act
        let _ = forward_request(request, &mock_endpoint).await;

        // Assert
        let captured_headers = mock_endpoint.captured_headers.lock().unwrap().clone().unwrap();
        let user_agent = captured_headers.get("user-agent").unwrap();
        let user_agent_str = user_agent.to_str().unwrap();
        assert_eq!(user_agent_str, "custom-agent/1.0");
        // Ensure the default user-agent was not used
        assert!(!user_agent_str.starts_with("agentkit-switchboard/"));
    }

    fn configured(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    async fn forward_with_headers(
        client_headers: HeaderMap,
        configured: Option<&std::collections::HashMap<String, String>>,
    ) -> reqwest::header::HeaderMap {
        let mock_endpoint = MockHttpEndpoint::new();
        let credential = ResolvedCredential {
            value: "test-token".to_string(),
            source: crate::credential::CredentialSource::None,
            oauth: None,
        };
        let billing = BillingModel::Subscription;

        let request = ForwardRequest {
            method: Method::POST,
            headers: client_headers,
            body: axum::body::Bytes::from(r#"{"test": "data"}"#),
            credential: &credential,
            billing: &billing,
            base_url: "http://example.com",
            provider_identity: "test_provider",
            session_id: None,
            provider_user_agent: None,
            provider_headers: configured,
        };

        let _ = forward_request(request, &mock_endpoint).await;
        let captured = mock_endpoint.captured_headers.lock().unwrap().clone();
        captured.unwrap()
    }

    #[tokio::test]
    async fn injects_configured_headers() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));
        let configured = configured(&[("x-custom-header", "custom-value")]);

        let captured = forward_with_headers(client_headers, Some(&configured)).await;

        assert_eq!(
            captured.get("x-custom-header").unwrap().to_str().unwrap(),
            "custom-value"
        );
    }

    #[tokio::test]
    async fn configured_headers_override_client_headers() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("x-custom-header", HeaderValue::from_static("client-value"));
        let configured = configured(&[("x-custom-header", "configured-value")]);

        let captured = forward_with_headers(client_headers, Some(&configured)).await;

        assert_eq!(
            captured.get("x-custom-header").unwrap().to_str().unwrap(),
            "configured-value"
        );
    }

    #[tokio::test]
    async fn configured_headers_pass_through_unchanged() {
        let configured = configured(&[("x-static-value", "plain-value")]);
        let captured = forward_with_headers(HeaderMap::new(), Some(&configured)).await;

        assert_eq!(
            captured.get("x-static-value").unwrap().to_str().unwrap(),
            "plain-value"
        );
    }

    #[tokio::test]
    async fn invalid_configured_header_is_skipped_without_panicking() {
        let configured = configured(&[
            ("bad header name!!", "value"),
            ("x-valid-header", "value-2"),
        ]);

        let captured = forward_with_headers(HeaderMap::new(), Some(&configured)).await;

        assert!(captured.get("bad header name!!").is_none());
        assert_eq!(
            captured.get("x-valid-header").unwrap().to_str().unwrap(),
            "value-2"
        );
    }
}
