pub mod db;

#[derive(Debug, Clone, serde::Serialize)]
pub struct EnrichedModel {
    /// The id clients use in requests: provider-qualified for a single
    /// provider/model instance, or the bare model id for a combined alias.
    pub id: String,
    /// The model id sent upstream after stripping any provider qualifier.
    pub model: String,
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub capabilities: Option<ModelCapabilities>,
    /// One provider for a qualified instance, several for a combined alias.
    pub providers: Vec<ProviderModelInfo>,
    /// True for the bare alias; false for a provider-qualified instance.
    pub alias: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ModelCapabilities {
    pub tool_calling: Option<bool>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderModelInfo {
    pub identity: String,
    pub billing: String,
    pub pricing: Option<ModelPricing>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    pub reasoning_per_mtok: Option<f64>,
}
