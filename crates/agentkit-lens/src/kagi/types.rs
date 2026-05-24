use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Kagi API response metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct KagiMeta {
    /// Unique request trace ID
    pub trace: String,
    /// Kagi node identifier
    pub node: String,
    /// Response time in milliseconds
    pub ms: u64,
}

/// Kagi API search result item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct KagiItem {
    /// Result URL
    pub url: String,
    /// Result title
    pub title: String,
    /// Result snippet/description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Publication date (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
}

/// Kagi API search data container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct KagiData {
    /// Search results
    pub search: Vec<KagiItem>,
}

/// Kagi API full response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct KagiResponse {
    /// Response metadata
    pub meta: KagiMeta,
    /// Response data
    pub data: KagiData,
}

/// Errors that can occur during Kagi search.
#[derive(Error, Debug)]
pub enum SearchError {
    /// Invalid or missing API key.
    #[error("Invalid or missing Kagi API key")]
    InvalidKey,
    /// Rate limited by Kagi API.
    #[error("Kagi API rate limited")]
    RateLimited,
    /// HTTP error.
    #[error("Kagi API HTTP error: {status}, {detail}")]
    HttpError { status: u16, detail: String },
    /// Network error.
    #[error("Kagi API network error: {0}")]
    Network(#[from] reqwest::Error),
    /// Error parsing Kagi API response.
    #[error("Kagi API parse error: {0}")]
    Parse(String),
}
