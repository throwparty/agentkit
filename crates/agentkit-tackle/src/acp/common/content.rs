//! The neutral content representation shared by the version modules.
//!
//! Stored messages are lists of blocks. Text is modelled semantically; every
//! other block (images, resources, future variants) is preserved as its raw
//! JSON so a version that understands it can render it and a version that does
//! not can fall back to a textual placeholder rather than dropping the message.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One stored content block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    /// A text block, with the original wire object preserved so optional
    /// annotations and metadata survive a round-trip.
    Text { text: String, raw: Value },
    /// Any non-text block, preserved verbatim.
    Other(Value),
}

impl Block {
    /// A plain text block.
    pub(crate) fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        let raw = serde_json::json!({ "type": "text", "text": text });
        Self::Text { text, raw }
    }

    /// Wraps a wire block (already serialised) in the neutral form.
    pub(crate) fn from_value(value: Value) -> Self {
        if value.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                return Self::Text {
                    text: text.to_owned(),
                    raw: value,
                };
            }
        }
        Self::Other(value)
    }

    /// The wire object for this block.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Text { raw, .. } | Self::Other(raw) => raw.clone(),
        }
    }

    /// The text payload, when this is a text block.
    pub(crate) fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Other(_) => None,
        }
    }
}

impl Serialize for Block {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from_value)
    }
}

/// Serialises a message's blocks for the store.
pub(crate) fn encode(blocks: &[Block]) -> String {
    serde_json::to_string(blocks).unwrap_or_else(|_| "[]".to_owned())
}

/// Parses a stored message's blocks. Unparseable content is an empty message,
/// never a replay failure.
pub(crate) fn decode(json: &str) -> Vec<Block> {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_with_its_metadata() {
        let raw = serde_json::json!({
            "type": "text",
            "text": "hello",
            "annotations": { "audience": ["user"] },
        });
        let block = Block::from_value(raw.clone());
        assert_eq!(block.as_text(), Some("hello"));
        assert_eq!(block.to_value(), raw);
        assert_eq!(decode(&encode(&[block])).len(), 1);
    }

    #[test]
    fn unknown_blocks_are_preserved_verbatim() {
        let raw = serde_json::json!({ "type": "_future", "payload": 7 });
        let block = Block::from_value(raw.clone());
        assert_eq!(block.as_text(), None);
        assert_eq!(block.to_value(), raw);
    }

    #[test]
    fn malformed_stored_content_decodes_to_nothing() {
        assert!(decode("not json").is_empty());
    }
}
