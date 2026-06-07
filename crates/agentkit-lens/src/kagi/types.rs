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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kagi_response_serde_roundtrip() {
        let response = KagiResponse {
            meta: KagiMeta {
                trace: "abc123".to_string(),
                node: "us-central1".to_string(),
                ms: 213,
            },
            data: KagiData {
                search: vec![
                    KagiItem {
                        url: "https://example.com".to_string(),
                        title: "Example".to_string(),
                        snippet: Some("Description".to_string()),
                        time: Some("2024-09-30T00:00:00Z".to_string()),
                    },
                ],
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: KagiResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.meta.trace, "abc123");
        assert_eq!(deserialized.meta.ms, 213);
        assert_eq!(deserialized.data.search.len(), 1);
        assert_eq!(deserialized.data.search[0].url, "https://example.com");
        assert_eq!(
            deserialized.data.search[0].snippet,
            Some("Description".to_string())
        );
    }

    #[test]
    fn test_kagi_item_missing_snippet() {
        let json = r#"{
            "url": "https://example.com",
            "title": "No Snippet"
        }"#;

        let item: KagiItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.url, "https://example.com");
        assert_eq!(item.title, "No Snippet");
        assert!(item.snippet.is_none());
    }

    #[test]
    fn test_kagi_response_no_results() {
        let json = r#"{
            "meta": {
                "trace": "empty",
                "node": "us-central1",
                "ms": 100
            },
            "data": {
                "search": []
            }
        }"#;

        let response: KagiResponse = serde_json::from_str(json).unwrap();
        assert!(response.data.search.is_empty());
    }

    #[test]
    fn test_kagi_search_error_display() {
        let err = SearchError::InvalidKey;
        assert_eq!(err.to_string(), "Invalid or missing Kagi API key");

        let err = SearchError::RateLimited;
        assert_eq!(err.to_string(), "Kagi API rate limited");

        let err = SearchError::HttpError {
            status: 500,
            detail: "Internal error".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Kagi API HTTP error: 500, Internal error"
        );

        let err = SearchError::Parse("bad json".to_string());
        assert_eq!(err.to_string(), "Kagi API parse error: bad json");
    }
}
