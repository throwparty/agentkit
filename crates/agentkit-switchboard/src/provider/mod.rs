pub mod router;
pub mod quota;
pub mod registry;

use crate::config::{BillingModel, PricingConfig};

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    Healthy,
    Degraded,
    Unauthenticated,
}

#[derive(Debug, Clone)]
pub struct ProviderRuntime {
    pub has_valid_credential: bool,
    pub status: ProviderStatus,
}

#[derive(Debug, Clone)]
pub struct ProviderView {
    pub identity: String,
    pub billing: BillingModel,
    pub models: Vec<String>,
    pub pricing: PricingConfig,
    pub has_valid_credential: bool,
    pub status: ProviderStatus,
}

impl ProviderView {
    pub fn is_available(&self) -> bool {
        matches!(self.status, ProviderStatus::Healthy) && self.has_valid_credential
    }

    pub fn serves_model(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
    }
}
