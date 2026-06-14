pub mod router;
pub mod quota;
pub mod registry;

use crate::config::{ApiSurface, BillingModel, PricingConfig};

#[derive(Debug, Clone)]
pub enum ProviderStatus {
    Healthy,
    Degraded,
    Unconfigured,
}

#[derive(Debug, Clone)]
pub struct ProviderState {
    pub identity: String,
    pub api_surface: ApiSurface,
    pub billing: BillingModel,
    pub models: Vec<String>,
    pub has_valid_credential: bool,
    pub status: ProviderStatus,
    pub pricing: PricingConfig,
}

impl ProviderState {
    pub fn is_available(&self) -> bool {
        matches!(self.status, ProviderStatus::Healthy) && self.has_valid_credential
    }

    pub fn serves_model(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
    }
}
