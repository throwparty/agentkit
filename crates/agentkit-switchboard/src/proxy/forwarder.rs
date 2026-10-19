use crate::config::{ApiSurface, BillingModel};
use crate::credential::ResolvedCredential;
use crate::domain::http::HttpEndpoint;
use crate::proxy::mutation::{MutationContext, MutationError, MutationSet};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde_json::Value;
use std::sync::{Arc, OnceLock};

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
    /// Mutations run before credentials are injected. `None` for a provider
    /// with no configured scripts.
    pub mutations: Option<Arc<MutationSet>>,
    pub surface: ApiSurface,
    pub model: &'a str,
}

/// Fail the request rather than forward an unmutated one.
fn mutation_failed(
    provider_identity: &str,
    billing: &BillingModel,
    error: &MutationError,
) -> ForwardOutcome {
    tracing::warn!(provider = provider_identity, %error, "mutation failed");
    let body = format!("mutation failed: {error}");
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Switchboard-Provider",
        HeaderValue::from_str(provider_identity).unwrap(),
    );
    headers.insert(
        "X-Switchboard-Billing",
        HeaderValue::from_str(&billing.to_string()).unwrap(),
    );
    let mut response = Response::new(axum::body::Body::from(body.clone()));
    *response.status_mut() = StatusCode::BAD_GATEWAY;
    *response.headers_mut() = headers;
    ForwardOutcome {
        response,
        status: StatusCode::BAD_GATEWAY,
        headers: Vec::new(),
        body_text: Some(body),
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
        mutations,
        surface,
        model,
    } = request;

    let parsed_body = serde_json::from_slice::<serde_json::Value>(&body).ok();

    let target_url = http.build_url(base_url, parsed_body.as_ref().unwrap_or(&Value::Null));

    let mut request_body = body.to_vec();

    let mut out_headers = HeaderMap::new();
    for (key, value) in &headers {
        let key_str = key.as_str().to_ascii_lowercase();
        // Skip headers that are handled specially or could cause conflicts
        if key_str != "host" && key_str != "content-length" {
            out_headers.insert(key.clone(), value.clone());
        }
    }
    if !out_headers.contains_key("content-type") {
        out_headers.insert("Content-Type", HeaderValue::from_static("application/json"));
    }
    out_headers.insert(
        "User-Agent",
        HeaderValue::from_static(concat!("agentkit-switchboard/", env!("CARGO_PKG_VERSION"))),
    );
    http.default_headers(&mut out_headers);

    if let Some(mutations) = mutations {
        let context = MutationContext {
            provider_identity: provider_identity.to_string(),
            surface,
            model: model.to_string(),
            session_id: session_id.map(str::to_string),
        };
        let moved_headers = std::mem::take(&mut out_headers);
        let moved_body = std::mem::take(&mut request_body);
        let (result, headers, body) = tokio::task::spawn_blocking(move || {
            let mut headers = moved_headers;
            let mut body = moved_body;
            let outcome = mutations.apply(&context, &mut headers, &mut body);
            (outcome, headers, body)
        })
        .await
        .unwrap_or_else(|error| {
            (
                Err(MutationError::Script {
                    origin: provider_identity.to_string(),
                    error: agentkit_script::ScriptError::Runtime {
                        origin: provider_identity.to_string(),
                        message: error.to_string(),
                    },
                }),
                HeaderMap::new(),
                Vec::new(),
            )
        });
        out_headers = headers;
        request_body = body;
        if let Err(error) = result {
            return mutation_failed(provider_identity, billing, &error);
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
    use crate::proxy::mutation::MutationSet;
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

        fn default_headers(&self, headers: &mut reqwest::header::HeaderMap) {
            headers.insert(
                "x-endpoint-default",
                reqwest::header::HeaderValue::from_static("from-endpoint"),
            );
        }

        fn inject_headers(
            &self,
            headers: &mut reqwest::header::HeaderMap,
            credential: &ResolvedCredential,
        ) {
            let value = reqwest::header::HeaderValue::from_str(&credential.value).unwrap();
            headers.insert("authorization", value);
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

    fn request(
        client_headers: HeaderMap,
        mutations: Option<Arc<MutationSet>>,
    ) -> ForwardRequest<'static> {
        static CREDENTIAL: OnceLock<ResolvedCredential> = OnceLock::new();
        static BILLING: OnceLock<BillingModel> = OnceLock::new();
        static BODY: OnceLock<axum::body::Bytes> = OnceLock::new();

        ForwardRequest {
            method: Method::POST,
            headers: client_headers,
            body: BODY
                .get_or_init(|| {
                    axum::body::Bytes::from_static(br#"{"model":"gpt-4o","messages":[]}"#)
                })
                .clone(),
            credential: CREDENTIAL.get_or_init(|| ResolvedCredential {
                value: "test-token".to_string(),
                source: crate::credential::CredentialSource::None,
                oauth: None,
            }),
            billing: BILLING.get_or_init(|| BillingModel::Subscription),
            base_url: "http://example.com",
            provider_identity: "test_provider",
            session_id: None,
            mutations,
            surface: crate::config::ApiSurface::OpenaiChatCompletions,
            model: "gpt-4o",
        }
    }

    async fn forward(
        client_headers: HeaderMap,
        mutations: Option<Arc<MutationSet>>,
    ) -> reqwest::header::HeaderMap {
        let mock_endpoint = MockHttpEndpoint::new();
        let _ = forward_request(request(client_headers, mutations), &mock_endpoint).await;
        let captured = mock_endpoint.captured_headers.lock().unwrap().clone();
        captured.unwrap()
    }

    #[tokio::test]
    async fn forwards_the_default_user_agent() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("user-agent", HeaderValue::from_static("test-client/1.0"));
        client_headers.insert("authorization", HeaderValue::from_static("Bearer token"));

        let captured = forward(client_headers, None).await;

        let user_agent = captured.get("user-agent").unwrap().to_str().unwrap();
        assert!(user_agent.starts_with("agentkit-switchboard/"));
    }

    #[tokio::test]
    async fn client_headers_pass_through_when_no_mutations_are_configured() {
        let mut client_headers = HeaderMap::new();
        client_headers.insert("content-type", HeaderValue::from_static("application/json"));
        client_headers.insert("x-client-header", HeaderValue::from_static("client-value"));

        let captured = forward(client_headers, None).await;

        assert_eq!(captured.get("x-client-header").unwrap(), "client-value");
    }

    #[tokio::test]
    async fn mutations_run_before_credentials_are_injected() {
        let engine = Arc::new(agentkit_script::ScriptEngine::new());
        let script = engine
            .compile(
                agentkit_script::INLINE_ORIGIN,
                r#"fn mutate(request) { request.headers["x-from-mutation"] = "yes"; request.headers["authorization"] = "Bearer script-token"; request }"#,
            )
            .unwrap();

        let captured = forward(
            HeaderMap::new(),
            Some(Arc::new(MutationSet::new(engine, vec![script]))),
        )
        .await;

        assert_eq!(captured.get("x-from-mutation").unwrap(), "yes");
        assert_ne!(
            captured.get("authorization").map(|v| v.to_str().unwrap()),
            Some("Bearer script-token"),
            "credential injection must win over the mutation"
        );
    }

    #[tokio::test]
    async fn mutations_can_override_provider_defaults_but_not_credentials() {
        let engine = Arc::new(agentkit_script::ScriptEngine::new());
        let script = engine
            .compile(
                agentkit_script::INLINE_ORIGIN,
                r#"fn mutate(request) { request.headers["x-endpoint-default"] = "from-script"; request }"#,
            )
            .unwrap();

        let captured = forward(
            HeaderMap::new(),
            Some(Arc::new(MutationSet::new(engine, vec![script]))),
        )
        .await;

        assert_eq!(
            captured.get("x-endpoint-default").unwrap(),
            "from-script",
            "a mutation must be able to override a provider default"
        );
        assert_eq!(
            captured.get("authorization").unwrap(),
            "test-token",
            "credential injection must still win"
        );
    }

    #[tokio::test]
    async fn scripts_see_the_raw_client_headers() {
        let engine = Arc::new(agentkit_script::ScriptEngine::new());
        let script = engine
            .compile(
                agentkit_script::INLINE_ORIGIN,
                r#"fn mutate(request) { request.headers["x-echo-authorization"] = request.headers["authorization"]; request }"#,
            )
            .unwrap();

        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer client-token"),
        );

        let captured = forward(
            client_headers,
            Some(Arc::new(MutationSet::new(engine, vec![script]))),
        )
        .await;

        assert_eq!(
            captured.get("x-echo-authorization").unwrap(),
            "Bearer client-token",
            "switchboard does no inbound auth, so scripts see the raw client headers"
        );
    }

    #[tokio::test]
    async fn a_failing_mutation_fails_the_request() {
        let engine = Arc::new(agentkit_script::ScriptEngine::new());
        let script = engine
            .compile(
                agentkit_script::INLINE_ORIGIN,
                "fn mutate(request) { throw \"no\"; }",
            )
            .unwrap();

        let mock_endpoint = MockHttpEndpoint::new();
        let outcome = forward_request(
            request(
                HeaderMap::new(),
                Some(Arc::new(MutationSet::new(engine, vec![script]))),
            ),
            &mock_endpoint,
        )
        .await;

        assert_eq!(outcome.status, StatusCode::BAD_GATEWAY);
        assert!(mock_endpoint.captured_headers.lock().unwrap().is_none());
    }
}
