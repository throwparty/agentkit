use std::sync::OnceLock;

static SNAPSHOT: OnceLock<&'static [u8]> = OnceLock::new();

pub fn bundled_snapshot() -> &'static [u8] {
    SNAPSHOT.get_or_init(|| {
        include_bytes!(env!("AGENTKIT_MODELS_DEV_JSON"))
    })
}

pub fn bundled_snapshot_parsed() -> serde_json::Value {
    serde_json::from_slice(bundled_snapshot()).expect("invalid bundled models.dev.json")
}
