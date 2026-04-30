use serde::{Deserialize, Serialize};

/// Brave API web search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct BraveWebResult {
    /// Result title
    pub title: String,
    /// Result URL
    pub url: String,
    /// Result description/snippet
    pub description: String,
    /// Human-readable age (e.g., "2 days ago")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,
    /// ISO 639 language code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Whether the result is family-friendly
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_friendly: Option<bool>,
}

/// Brave API query metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct BraveQuery {
    /// Original search query
    pub original: String,
    /// Whether more results are available
    pub more_results_available: bool,
    /// Potentially altered query
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altered: Option<String>,
    /// Country code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Safe search setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safesearch: Option<bool>,
    /// Whether there were bad results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bad_results: Option<bool>,
}

/// Brave API web search section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct BraveWebSection {
    /// Search type
    #[serde(rename = "type")]
    pub search_type: String,
    /// Array of search results
    pub results: Vec<BraveWebResult>,
}

/// Brave API full response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct WebSearchApiResponse {
    /// Search type
    #[serde(rename = "type")]
    pub search_type: String,
    /// Query metadata
    pub query: BraveQuery,
    /// Web search section (may be absent for FAQ/discussion-only results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<BraveWebSection>,
}

/// Brave API rate limit error structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct RateLimitErrorResponse {
    /// Error type code
    #[serde(rename = "type")]
    pub error_type: String,
    /// Human-readable error message
    pub title: String,
    /// HTTP status code
    pub status: u16,
    /// Detail about the error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Instance URI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

/// Search error variants returned by the Brave engine.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("Invalid or expired Brave API key")]
    InvalidKey,

    #[error("Rate limit exceeded")]
    RateLimited,

    #[error("HTTP error: {status} - {detail}")]
    HttpError { status: u16, detail: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Parse error: {0}")]
    Parse(String),
}
