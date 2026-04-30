use serde::{Deserialize, Serialize};

/// A search query request to be dispatched to a search engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SearchRequest {
    /// The search query string (400 chars max, 50 words max)
    pub query: String,
    /// The engine to use (e.g., "brave")
    #[serde(default)]
    pub engine: String,
    /// Page number for pagination, 1-based (default: 1)
    #[serde(default = "default_page")]
    pub page: u32,
    /// Number of results per page, 1-20 (default: 10)
    #[serde(default = "default_max_results")]
    #[schemars(range(min = "1", max = "20"))]
    pub max_results: u32,
    /// Region code, ISO 3166-1 alpha-2 (optional, engine-specific)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>, engine: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            engine: engine.into(),
            ..Default::default()
        }
    }

    /// Validate the request before sending to the engine.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.query.is_empty() {
            return Err("Query must not be empty");
        }
        if self.query.len() > 400 {
            return Err("Query exceeds 400 character limit");
        }
        let words = self.query.split_whitespace().count();
        if words > 50 {
            return Err("Query exceeds 50 word limit");
        }
        Ok(())
    }
}

fn default_page() -> u32 { 1 }
fn default_max_results() -> u32 { 10 }

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            engine: String::new(),
            page: default_page(),
            max_results: default_max_results(),
            region: None,
        }
    }
}

/// A search result returned by a search engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    pub position: u32,
}

/// A search response containing results and pagination metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub query: String,
    pub engine: String,
    pub page: u32,
    pub total_pages: u32,
    pub has_more: bool,
}

/// Information about a registered search engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct EngineInfo {
    pub name: String,
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A response from the `list-search-engines` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct EnginesResponse {
    pub engines: Vec<EngineInfo>,
}

/// Engine info for MCP tool responses (serializable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct EngineInfoOutput {
    pub name: String,
    pub configured: bool,
    pub hint: Option<String>,
}

impl From<EngineInfo> for EngineInfoOutput {
    fn from(info: EngineInfo) -> Self {
        EngineInfoOutput {
            name: info.name,
            configured: info.configured,
            hint: info.hint,
        }
    }
}

/// Engine registry for managing search engine instances.
pub struct EngineRegistry {
    engines: Vec<Box<dyn SearchEngine>>,
}

impl Default for EngineRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self { engines: Vec::new() }
    }

    pub fn register(&mut self, engine: Box<dyn SearchEngine>) {
        self.engines.push(engine);
    }

    pub fn get(&self, name: &str) -> Option<&dyn SearchEngine> {
        self.engines.iter().find(|e| e.name() == name).map(|e| e.as_ref())
    }

    pub fn list(&self) -> Vec<EngineInfo> {
        self.engines.iter().map(|e| EngineInfo {
            name: e.name().to_string(),
            configured: e.is_configured(),
            hint: e.config_hint(),
        }).collect()
    }
}

/// Error type used by search engines.
pub type SearchEngineError = Box<dyn std::error::Error + Send + Sync>;

/// Result type for search operations.
pub type SearchEngineResult<T = SearchResponse> = std::result::Result<T, SearchEngineError>;

/// Trait for all search engine backends.
///
/// For dyn-compatibility with async methods, this uses `#[async_trait]`
/// which generates a trait object-safe signature by boxing the future.
#[async_trait::async_trait]
pub trait SearchEngine: Send + Sync {
    fn name(&self) -> &str;
    fn is_configured(&self) -> bool { true }
    fn config_hint(&self) -> Option<String> { None }
    async fn search(&self, req: SearchRequest) -> SearchEngineResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_request_serde_roundtrip() {
        let req = SearchRequest::new("rust async runtime", "brave");
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: SearchRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.query, "rust async runtime");
        assert_eq!(deserialized.engine, "brave");
    }

    #[test]
    fn test_search_response_serde_roundtrip() {
        let resp = SearchResponse {
            results: vec![SearchResult {
                title: "Test".to_string(),
                link: "https://example.com".to_string(),
                snippet: "Snippet".to_string(),
                position: 1,
            }],
            query: "test".to_string(),
            engine: "brave".to_string(),
            page: 1,
            total_pages: 1,
            has_more: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: SearchResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.results.len(), 1);
        assert_eq!(deserialized.results[0].title, "Test");
    }

    #[test]
    fn test_search_request_defaults() {
        let json = r#"{"query":"test","engine":"brave"}"#;
        let req: SearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.page, 1);
        assert_eq!(req.max_results, 10);
        assert!(req.region.is_none());
    }

    #[test]
    fn test_json_schema_generation() {
        let _schema = schemars::schema_for!(SearchRequest);
        let _schema = schemars::schema_for!(EnginesResponse);
    }

    #[test]
    fn test_query_validation_too_long() {
        let req = SearchRequest {
            query: "a".repeat(401),
            engine: "brave".to_string(),
            page: 1, max_results: 10, region: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_query_validation_too_many_words() {
        let query: String = (0..51).map(|_| "word").collect::<Vec<_>>().join(" ");
        let req = SearchRequest {
            query, engine: "brave".to_string(),
            page: 1, max_results: 10, region: None,
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_query_validation_valid() {
        assert!(SearchRequest::new("valid query", "brave").validate().is_ok());
    }

    #[test]
    fn test_query_validation_empty() {
        let req = SearchRequest {
            query: String::new(), engine: "brave".to_string(),
            page: 1, max_results: 10, region: None,
        };
        assert!(req.validate().is_err());
    }
}
