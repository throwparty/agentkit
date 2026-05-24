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
}

impl KagiSearchEngine {
    /// Create a new Kagi search engine with the given options.
    pub fn new(options: KagiOptions) -> Self {
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
        let response = self
            .client
            .post("https://kagi.com/api/v1/search")
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
