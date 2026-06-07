use super::types::*;
use crate::search::{SearchEngine, SearchRequest, SearchResponse, SearchResult};
use std::sync::Arc;

/// Options for configuring a Kagi search engine instance.
#[derive(Debug, Clone)]
pub struct KagiOptions {
    /// Kagi API key.
    pub api_key: String,
}

/// Kagi search engine implementation.
pub struct KagiSearchEngine {
    api_key: Arc<str>,
    client: reqwest::Client,
    base_url: String,
}

impl KagiSearchEngine {
    /// Create a new Kagi search engine with the given options.
    pub fn new(options: KagiOptions) -> Self {
        Self::new_with_client(options, None, None)
    }

    /// Create a new Kagi search engine with a custom HTTP client and base URL.
    ///
    /// Used for testing with wiremock servers. Pass `None` for either parameter
    /// to use the default production value.
    pub fn new_with_client(
        options: KagiOptions,
        client: Option<reqwest::Client>,
        base_url: Option<String>,
    ) -> Self {
        let client = client.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .gzip(true)
                .build()
                .expect("Failed to build HTTP client")
        });

        Self {
            api_key: Arc::from(options.api_key),
            client,
            base_url: base_url.unwrap_or_else(|| "https://kagi.com".to_string()),
        }
    }

    /// Parse the Kagi API response into our unified SearchResponse.
    fn parse_response(&self, api_response: KagiResponse, req: &SearchRequest) -> SearchResponse {
        let results: Vec<SearchResult> = api_response
            .data
            .search
            .into_iter()
            .enumerate()
            .map(|(i, item)| SearchResult {
                title: item.title,
                link: item.url,
                snippet: item.snippet.unwrap_or_default(),
                position: (i + 1) as u32,
            })
            .collect();

        let has_more = false;
        let total_pages = if results.is_empty() { 0 } else { 1 };

        SearchResponse {
            results,
            query: req.query.clone(),
            engine: self.name().to_string(),
            page: req.page,
            total_pages,
            has_more,
        }
    }
}

/// Request body for the Kagi API.
#[derive(Debug, serde::Serialize)]
struct KagiRequest {
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<usize>,
    workflow: String,
}

#[async_trait::async_trait]
impl SearchEngine for KagiSearchEngine {
    fn name(&self) -> &str {
        "kagi"
    }

    fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn search(&self, req: SearchRequest) -> crate::search::SearchEngineResult {
        let auth_header = format!("Bearer {}", self.api_key);
        let max_results = req.max_results.min(20);

        if let Err(e) = req.validate() {
            return Err(e.into());
        }

        let kagi_req = KagiRequest {
            query: req.query.clone(),
            n: Some(max_results as usize),
            workflow: "search".to_string(),
        };

        let body = serde_json::to_string(&kagi_req)?;
        let url = format!("{}/api/v1/search", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Authorization", auth_header)
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                SearchError::Network(e)
            })?;

        let status = response.status().as_u16();

        let response_body = response.text().await?;

        if status == 401 {
            return Err(SearchError::InvalidKey.into());
        }
        if status == 429 {
            return Err(SearchError::RateLimited.into());
        }

        if status >= 400 {
            return Err(SearchError::HttpError { status, detail: response_body }.into());
        }

        let api_response: KagiResponse =
            serde_json::from_str(&response_body).map_err(|e| {
                SearchError::Parse(e.to_string())
            })?;

        let search_response = self.parse_response(api_response, &req);
        Ok(search_response)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn kagi_engine(api_key: &str, base_url: &str) -> KagiSearchEngine {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .gzip(true)
            .build()
            .unwrap();
        KagiSearchEngine::new_with_client(
            KagiOptions {
                api_key: api_key.to_string(),
            },
            Some(client),
            Some(base_url.to_string()),
        )
    }

    #[test]
    fn test_kagi_engine_name() {
        let engine = KagiSearchEngine::new(KagiOptions {
            api_key: "test".to_string(),
        });
        assert_eq!(engine.name(), "kagi");
    }

    #[test]
    fn test_kagi_engine_configured() {
        let engine = KagiSearchEngine::new(KagiOptions {
            api_key: "test".to_string(),
        });
        assert!(engine.is_configured());

        let empty_engine = KagiSearchEngine::new(KagiOptions {
            api_key: "".to_string(),
        });
        assert!(!empty_engine.is_configured());
    }

    #[test]
    fn test_parse_response_basic() {
        let engine = KagiSearchEngine::new(KagiOptions {
            api_key: "test".to_string(),
        });

        let api_response = KagiResponse {
            meta: KagiMeta {
                trace: "abc123".to_string(),
                node: "us-central1".to_string(),
                ms: 213,
            },
            data: KagiData {
                search: vec![
                    KagiItem {
                        url: "https://example.com".to_string(),
                        title: "Example Result".to_string(),
                        snippet: Some("An example description".to_string()),
                        time: None,
                    },
                ],
            },
        };

        let req = SearchRequest {
            query: "test query".to_string(),
            engine: "kagi".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);

        assert_eq!(response.query, "test query");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title, "Example Result");
        assert_eq!(response.results[0].link, "https://example.com");
        assert_eq!(response.results[0].snippet, "An example description");
        assert_eq!(response.results[0].position, 1);
        assert!(!response.has_more);
        assert_eq!(response.page, 1);
        assert_eq!(response.total_pages, 1);
    }

    #[test]
    fn test_parse_response_no_snippet() {
        let engine = KagiSearchEngine::new(KagiOptions {
            api_key: "test".to_string(),
        });

        let api_response = KagiResponse {
            meta: KagiMeta {
                trace: "def456".to_string(),
                node: "us-central1".to_string(),
                ms: 150,
            },
            data: KagiData {
                search: vec![
                    KagiItem {
                        url: "https://example.com/no-snippet".to_string(),
                        title: "No Snippet".to_string(),
                        snippet: None,
                        time: None,
                    },
                ],
            },
        };

        let req = SearchRequest {
            query: "no snippet".to_string(),
            engine: "kagi".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].snippet, "");
    }

    #[test]
    fn test_parse_response_empty() {
        let engine = KagiSearchEngine::new(KagiOptions {
            api_key: "test".to_string(),
        });

        let api_response = KagiResponse {
            meta: KagiMeta {
                trace: "ghi789".to_string(),
                node: "us-central1".to_string(),
                ms: 100,
            },
            data: KagiData {
                search: vec![],
            },
        };

        let req = SearchRequest {
            query: "empty".to_string(),
            engine: "kagi".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);

        assert!(response.results.is_empty());
        assert_eq!(response.total_pages, 0);
    }

    #[tokio::test]
    async fn test_kagi_search_success() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "meta": {
                        "trace": "t1",
                        "node": "us-central1",
                        "ms": 150
                    },
                    "data": {
                        "search": [
                            {
                                "url": "https://example.com/result1",
                                "title": "Kagi Result 1",
                                "snippet": "First result from Kagi"
                            },
                            {
                                "url": "https://example.com/result2",
                                "title": "Kagi Result 2",
                                "snippet": "Second result from Kagi"
                            }
                        ]
                    }
                })),
            )
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("test-key", &base);
        let req = SearchRequest::new("kagi test", "kagi");
        let response = engine.search(req).await.unwrap();

        assert_eq!(response.query, "kagi test");
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.results[0].title, "Kagi Result 1");
        assert_eq!(response.results[0].link, "https://example.com/result1");
        assert_eq!(response.results[0].snippet, "First result from Kagi");
        assert_eq!(response.results[0].position, 1);
        assert_eq!(response.results[1].position, 2);
        assert!(!response.has_more);
        assert_eq!(response.total_pages, 1);
    }

    #[tokio::test]
    async fn test_kagi_search_empty_results() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "meta": {
                        "trace": "t2",
                        "node": "us-central1",
                        "ms": 100
                    },
                    "data": {
                        "search": []
                    }
                })),
            )
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("test-key", &base);
        let req = SearchRequest::new("empty kagi", "kagi");
        let response = engine.search(req).await.unwrap();

        assert!(response.results.is_empty());
        assert_eq!(response.total_pages, 0);
    }

    #[tokio::test]
    async fn test_kagi_search_unauthorized() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("bad-key", &base);
        let req = SearchRequest::new("test", "kagi");
        let err = engine.search(req).await.unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("Invalid") || msg.contains("invalid") || msg.contains("key"),
            "error should mention invalid key: {msg}"
        );
    }

    #[tokio::test]
    async fn test_kagi_search_rate_limited() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("test-key", &base);
        let req = SearchRequest::new("test", "kagi");
        let err = engine.search(req).await.unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("Rate") || msg.contains("rate"),
            "error should mention rate limiting: {msg}"
        );
    }

    #[tokio::test]
    async fn test_kagi_search_server_error() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("test-key", &base);
        let req = SearchRequest::new("test", "kagi");
        let err = engine.search(req).await.unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("502") || msg.contains("error"),
            "error should mention HTTP error: {msg}"
        );
    }

    #[tokio::test]
    async fn test_kagi_search_without_snippet_field() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "meta": {
                        "trace": "t3",
                        "node": "us-central1",
                        "ms": 80
                    },
                    "data": {
                        "search": [
                            {
                                "url": "https://example.com/no-snippet",
                                "title": "No Snippet Here"
                            }
                        ]
                    }
                })),
            )
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("test-key", &base);
        let req = SearchRequest::new("no snippet", "kagi");
        let response = engine.search(req).await.unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].snippet, "");
    }

    #[tokio::test]
    async fn test_kagi_search_includes_auth_header() {
        let mock_server = MockServer::start().await;
        let base = mock_server.uri();

        Mock::given(method("POST"))
            .and(path("/api/v1/search"))
            .and(header("Authorization", "Bearer secret-kagi-key"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "meta": {
                        "trace": "t4",
                        "node": "us-central1",
                        "ms": 50
                    },
                    "data": {
                        "search": []
                    }
                })),
            )
            .mount(&mock_server)
            .await;

        let engine = kagi_engine("secret-kagi-key", &base);
        let req = SearchRequest::new("auth test", "kagi");
        assert!(engine.search(req).await.is_ok());
    }
}
