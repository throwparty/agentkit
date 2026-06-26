use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialBlob {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

pub fn read_stdin() -> Result<CredentialBlob, String> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str(&input).map_err(|e| format!("failed to parse JSON: {e}"))
}
