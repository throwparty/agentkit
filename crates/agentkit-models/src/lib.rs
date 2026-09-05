use std::{collections::HashMap, sync::OnceLock};

static SNAPSHOT: OnceLock<&'static [u8]> = OnceLock::new();

pub fn bundled_snapshot() -> &'static [u8] {
    SNAPSHOT.get_or_init(|| {
        include_bytes!(env!("AGENTKIT_MODELS_DEV_JSON"))
    })
}

pub fn bundled_snapshot_parsed() -> ModelSnapshot {
    serde_json::from_slice(bundled_snapshot()).expect("invalid bundled models.dev.json")
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelSnapshot {
    pub models: HashMap<String, ModelSnapshotEntry>,
    pub providers: HashMap<String, ProviderSnapshotEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelSnapshotEntry {
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub capabilities: Option<ModelCapabilitiesEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelCapabilitiesEntry {
    pub tool_calling: Option<bool>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProviderSnapshotEntry {
    pub billing: Option<String>,
    pub models: HashMap<String, ModelPricingEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelPricingEntry {
    pub input_per_mtok: Option<f64>,
    pub output_per_mtok: Option<f64>,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    pub reasoning_per_mtok: Option<f64>,
}
