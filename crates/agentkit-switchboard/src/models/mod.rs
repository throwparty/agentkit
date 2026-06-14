pub mod db;

#[derive(Debug, Clone)]
pub struct EnrichedModel {
    pub id: String,
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub capabilities: Option<ModelCapabilities>,
    pub providers: Vec<ProviderModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelCapabilities {
    pub tool_calling: Option<bool>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ProviderModelInfo {
    pub identity: String,
    pub billing: String,
    pub pricing: Option<ModelPricing>,
}

#[derive(Debug, Clone)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    pub reasoning_per_mtok: Option<f64>,
}
