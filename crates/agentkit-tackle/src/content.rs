//! Helpers for stored ACP content blocks (JSON-serialised in the store).

/// The text of a stored message: its text blocks joined.
pub fn prompt_text_of(content: &str) -> String {
    serde_json::from_str::<Vec<serde_json::Value>>(content)
        .map(|blocks| crate::agent::turn::prompt_text(&blocks))
        .unwrap_or_default()
}

/// The ordered model content of a stored message, including images.
pub fn prompt_parts_of(content: &str) -> Result<Vec<crate::providers::ChatContent>, String> {
    let blocks = serde_json::from_str::<Vec<serde_json::Value>>(content)
        .map_err(|err| format!("malformed stored content: {err}"))?;
    crate::agent::turn::prompt_parts(&blocks)
}

/// RFC 3339 rendering of a unix-seconds timestamp (ACP `updatedAt`).
pub fn timestamp(unix_seconds: i64) -> String {
    jiff::Timestamp::from_second(unix_seconds)
        .map(|ts| ts.to_string())
        .unwrap_or_default()
}
