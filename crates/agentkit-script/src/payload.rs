use serde_json::Value;
use sha2::{Digest, Sha256};

/// The argument payload cap: oversized payloads become digest maps.
pub const PAYLOAD_CAP: usize = 16 * 1024;

/// Argument payload cap: oversized payloads are replaced by a digest map
/// (`#{ truncated: true, size, sha256 }`) so scripts can detect but cannot
/// read them.
pub fn cap_payload(value: &Value) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= PAYLOAD_CAP {
        return value.clone();
    }
    let digest = Sha256::digest(serialized.as_bytes());
    let sha256: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    serde_json::json!({
        "truncated": true,
        "size": serialized.len(),
        "sha256": sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn small_payloads_pass_through() {
        let small = json!({ "command": "cargo test" });
        assert_eq!(cap_payload(&small), small);
    }

    #[test]
    fn oversized_payloads_become_digest_maps() {
        let huge = json!({ "blob": "x".repeat(PAYLOAD_CAP + 1) });
        let capped = cap_payload(&huge);
        assert_eq!(capped["truncated"], json!(true));
        assert!(capped["size"].as_u64().unwrap() > PAYLOAD_CAP as u64);
        assert_eq!(capped["sha256"].as_str().unwrap().len(), 64);
        // The digest map replaced the content: the blob is unreadable.
        assert!(capped.get("blob").is_none());
    }
}
