use super::types::*;
use crate::search::{SearchEngine, SearchRequest, SearchResponse, SearchResult};
use std::sync::Arc;

/// Options for configuring a Brave search engine instance.
#[derive(Debug, Clone)]
pub struct BraveOptions {
    /// Brave Search API key.
    pub api_key: String,
}

/// Brave search engine implementation.
pub struct BraveSearchEngine {
    api_key: Arc<str>,
    client: reqwest::Client,
}

impl BraveSearchEngine {
    /// Create a new Brave search engine with the given options.
    pub fn new(options: BraveOptions) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .gzip(true)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            api_key: Arc::from(options.api_key),
            client,
        }
    }

    /// Build the API URL for a search request.
    fn build_url(&self, req: &SearchRequest) -> String {
        let count = req.max_results.min(20);
        let offset = ((req.page - 1) * req.max_results).min(9);

        let mut url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}",
            urlencoding::encode(&req.query)
        );

        url.push_str(&format!("&count={}", count));
        url.push_str(&format!("&offset={}", offset));
        url.push_str("&text_decorations=false");

        if let Some(ref region) = req.region {
            url.push_str(&format!("&country={}", region));
        }

        url
    }

    /// Parse the Brave API response into our unified SearchResponse.
    fn parse_response(&self, api_response: WebSearchApiResponse, req: &SearchRequest) -> SearchResponse {
        let query = api_response.query;
        let results: Vec<SearchResult> = api_response
            .web
            .map(|web| {
                web.results
                    .into_iter()
                    .enumerate()
                    .map(|(i, result)| SearchResult {
                        title: result.title,
                        link: result.url,
                        snippet: result.description,
                        position: (i + 1) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let has_more = query.more_results_available;
        // Estimate total pages: if more_results_available, we have at least one more page
        let total_pages = if has_more {
            // Conservative estimate: current page + 1
            req.page + 1
        } else if results.is_empty() {
            0
        } else {
            req.page
        };

        SearchResponse {
            results,
            query: query.original,
            engine: self.name().to_string(),
            page: req.page,
            total_pages,
            has_more,
        }
    }
}

#[async_trait::async_trait]
impl SearchEngine for BraveSearchEngine {
    fn name(&self) -> &str {
        "brave"
    }

    fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn search(&self, req: SearchRequest) -> crate::search::SearchEngineResult {
        if let Err(e) = req.validate() {
            return Err(e.into());
        }

        let url = self.build_url(&req);

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", self.api_key.as_ref())
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SearchError::HttpError {
                        status: 0,
                        detail: "Request timed out after 30s".to_string(),
                    }
                } else {
                    SearchError::Network(e)
                }
            })?;

        let status = response.status().as_u16();

        // Handle specific error codes
        if status == 401 {
            return Err(SearchError::InvalidKey.into());
        }
        if status == 429 {
            return Err(SearchError::RateLimited.into());
        }

        // Try to parse the response as JSON
        let body = response.text().await?;

        if status >= 400 {
            // Try to parse error response, fall back to raw text
            let detail = if let Ok(error_resp) = serde_json::from_str::<RateLimitErrorResponse>(&body) {
                format!("{}: {}", error_resp.title, error_resp.detail.unwrap_or_default())
            } else {
                body.clone()
            };

            return Err(SearchError::HttpError { status, detail }.into());
        }

        // Parse the successful response
        let api_response: WebSearchApiResponse =
            serde_json::from_str(&body).map_err(|e| SearchError::Parse(e.to_string()))?;

        Ok(self.parse_response(api_response, &req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_brave_engine_name() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });
        assert_eq!(engine.name(), "brave");
    }

    #[test]
    fn test_brave_engine_configured() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });
        assert!(engine.is_configured());

        let empty_engine = BraveSearchEngine::new(BraveOptions {
            api_key: "".to_string(),
        });
        assert!(!empty_engine.is_configured());
    }

    #[test]
    fn test_build_url_basic() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });
        let req = SearchRequest {
            query: "rust async".to_string(),
            engine: "brave".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };
        let url = engine.build_url(&req);
        assert!(url.contains("q="));
        assert!(url.contains("rust"));
        assert!(url.contains("async"));
        assert!(url.contains("count=10"));
        assert!(url.contains("offset=0"));
        assert!(url.contains("text_decorations=false"));
    }

    #[test]
    fn test_build_url_with_region() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });
        let req = SearchRequest {
            query: "test".to_string(),
            engine: "brave".to_string(),
            page: 1,
            max_results: 10,
            region: Some("US".to_string()),
        };
        let url = engine.build_url(&req);
        assert!(url.contains("country=US"));
    }

    #[test]
    fn test_build_url_pagination() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });

        // Page 1: offset 0
        let req1 = SearchRequest {
            page: 1,
            max_results: 10,
            ..SearchRequest::new("test", "brave")
        };
        assert!(engine.build_url(&req1).contains("offset=0"));

        // Page 2: offset 10 (capped at 9)
        let req2 = SearchRequest {
            page: 2,
            max_results: 10,
            ..SearchRequest::new("test", "brave")
        };
        assert!(engine.build_url(&req2).contains("offset=9"));
    }

    #[test]
    fn test_parse_response_basic() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });

        let api_response = WebSearchApiResponse {
            search_type: "search".to_string(),
            query: BraveQuery {
                original: "test query".to_string(),
                more_results_available: false,
                altered: None,
                country: None,
                safesearch: None,
                bad_results: None,
            },
            web: Some(BraveWebSection {
                search_type: "search".to_string(),
                results: vec![BraveWebResult {
                    title: "Test Result".to_string(),
                    url: "https://example.com".to_string(),
                    description: "Test description".to_string(),
                    page_age: None,
                    language: None,
                    family_friendly: None,
                }],
            }),
        };

        let req = SearchRequest {
            query: "test".to_string(),
            engine: "brave".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);

        assert_eq!(response.query, "test query");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title, "Test Result");
        assert_eq!(response.results[0].link, "https://example.com");
        assert_eq!(response.results[0].snippet, "Test description");
        assert_eq!(response.results[0].position, 1);
        assert!(!response.has_more);
        assert_eq!(response.page, 1);
        assert_eq!(response.total_pages, 1);
    }

    #[test]
    fn test_parse_response_has_more() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });

        let api_response = WebSearchApiResponse {
            search_type: "search".to_string(),
            query: BraveQuery {
                original: "test".to_string(),
                more_results_available: true,
                altered: None,
                country: None,
                safesearch: None,
                bad_results: None,
            },
            web: Some(BraveWebSection {
                search_type: "search".to_string(),
                results: vec![BraveWebResult {
                    title: "Result 1".to_string(),
                    url: "https://example.com".to_string(),
                    description: "Desc".to_string(),
                    page_age: None,
                    language: None,
                    family_friendly: None,
                }],
            }),
        };

        let req = SearchRequest {
            query: "test".to_string(),
            engine: "brave".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);
        assert!(response.has_more);
        assert_eq!(response.total_pages, 2);
    }

    #[test]
    fn test_parse_response_empty() {
        let engine = BraveSearchEngine::new(BraveOptions {
            api_key: "test".to_string(),
        });

        let api_response = WebSearchApiResponse {
            search_type: "search".to_string(),
            query: BraveQuery {
                original: "test".to_string(),
                more_results_available: false,
                altered: None,
                country: None,
                safesearch: None,
                bad_results: None,
            },
            web: Some(BraveWebSection {
                search_type: "search".to_string(),
                results: vec![],
            }),
        };

        let req = SearchRequest {
            query: "test".to_string(),
            engine: "brave".to_string(),
            page: 1,
            max_results: 10,
            region: None,
        };

        let response = engine.parse_response(api_response, &req);
        assert!(response.results.is_empty());
        assert_eq!(response.total_pages, 0);
    }
}
