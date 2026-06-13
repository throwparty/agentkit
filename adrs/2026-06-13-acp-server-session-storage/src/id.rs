use std::fmt;

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageId(String);

#[derive(Debug, Error)]
pub enum IdError {
    #[error("Invalid ID format: {0}")]
    InvalidFormat(String),
    #[error("Wrong prefix: expected '{expected}', got '{actual}'")]
    WrongPrefix { expected: String, actual: String },
}

macro_rules! impl_id_type {
    ($name:ident, $prefix:expr) => {
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn from_uuid(uuid: String) -> Self {
                Self(uuid)
            }

            pub fn encode(&self) -> String {
                format!("{}{}", $prefix, self.0)
            }

            pub fn decode(prefixed: &str) -> Result<String, IdError> {
                if prefixed.is_empty() {
                    return Err(IdError::InvalidFormat("empty string".to_string()));
                }
                if let Some(rest) = prefixed.strip_prefix($prefix) {
                    if rest.is_empty() {
                        return Err(IdError::InvalidFormat(format!(
                            "prefix '{}' with no UUID",
                            $prefix
                        )));
                    }
                    return Ok(rest.to_string());
                }
                for (other_prefix, _other_name) in [
                    ("sess_", "SessionId"),
                    ("turn_", "TurnId"),
                    ("msg_", "MessageId"),
                ] {
                    if other_prefix != $prefix && prefixed.starts_with(other_prefix) {
                        return Err(IdError::WrongPrefix {
                            expected: $prefix.to_string(),
                            actual: other_prefix.to_string(),
                        });
                    }
                }
                Ok(prefixed.to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

impl_id_type!(SessionId, "sess_");
impl_id_type!(TurnId, "turn_");
impl_id_type!(MessageId, "msg_");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_new_roundtrip() {
        let id = SessionId::new();
        let encoded = id.encode();
        let decoded = SessionId::decode(&encoded).unwrap();
        assert_eq!(id.as_str(), decoded);
    }

    #[test]
    fn session_id_from_uuid() {
        let uuid = "a1b2c3d4-e5f6-7890-abcd-ef0123456789".to_string();
        let id = SessionId::from_uuid(uuid.clone());
        assert_eq!(id.encode(), format!("sess_{}", uuid));
        assert_eq!(
            SessionId::decode("sess_a1b2c3d4-e5f6-7890-abcd-ef0123456789").unwrap(),
            uuid
        );
    }

    #[test]
    fn session_id_wrong_prefix_turn() {
        let err = SessionId::decode("turn_xxx").unwrap_err();
        assert!(matches!(
            &err,
            IdError::WrongPrefix { expected, actual }
            if expected == "sess_" && actual == "turn_"
        ));
    }

    #[test]
    fn session_id_wrong_prefix_msg() {
        let err = SessionId::decode("msg_xxx").unwrap_err();
        assert!(matches!(
            &err,
            IdError::WrongPrefix { expected, actual }
            if expected == "sess_" && actual == "msg_"
        ));
    }

    #[test]
    fn session_id_bare_uuid_accepted() {
        let result = SessionId::decode("abc123").unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn session_id_empty_rejected() {
        let err = SessionId::decode("").unwrap_err();
        assert!(matches!(err, IdError::InvalidFormat(_)));
    }

    #[test]
    fn session_id_prefix_only_rejected() {
        let err = SessionId::decode("sess_").unwrap_err();
        assert!(matches!(err, IdError::InvalidFormat(_)));
    }

    #[test]
    fn turn_id_roundtrip() {
        let id = TurnId::new();
        let encoded = id.encode();
        let decoded = TurnId::decode(&encoded).unwrap();
        assert_eq!(id.as_str(), decoded);
    }

    #[test]
    fn turn_id_decode_as_session_rejected() {
        let err = SessionId::decode("turn_xxx").unwrap_err();
        assert!(matches!(err, IdError::WrongPrefix { .. }));
    }

    #[test]
    fn message_id_roundtrip() {
        let id = MessageId::new();
        let encoded = id.encode();
        let decoded = MessageId::decode(&encoded).unwrap();
        assert_eq!(id.as_str(), decoded);
    }

    #[test]
    fn message_id_decode_as_session_rejected() {
        let err = SessionId::decode("msg_xxx").unwrap_err();
        assert!(matches!(err, IdError::WrongPrefix { .. }));
    }
}
