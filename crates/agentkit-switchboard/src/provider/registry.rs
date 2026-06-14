use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::config::ProviderConfig;
use crate::provider::{ProviderState, ProviderStatus};
use crate::provider::quota::{ProviderQuotaState, QuotaSource, PayAsYouGoState, SubscriptionState};
use crate::credential::env;

pub struct ProviderRegistry {
    states: Arc<RwLock<HashMap<String, ProviderState>>>,
    quotas: Arc<RwLock<HashMap<String, ProviderQuotaState>>>,
}

impl ProviderRegistry {
    pub fn new(configs: &HashMap<String, ProviderConfig>) -> Self {
        let mut states = HashMap::new();
        let mut quotas = HashMap::new();

        for (identity, cfg) in configs {
            let has_credential = match cfg.auth.r#type.to_string().as_str() {
                "none" => true,
                _ => {
                    let from_env = cfg.auth.credential_env.as_ref().and_then(|name| env::read(name));
                    let helper_name = ""; // checked at runtime
                    from_env.is_some() || helper_name == "keychain"
                }
            };

            let status = if has_credential {
                ProviderStatus::Healthy
            } else {
                ProviderStatus::Unconfigured
            };

            let models = cfg.models.clone().unwrap_or_default();
            let quota_source = match cfg.billing {
                crate::config::BillingModel::Subscription => QuotaSource::Subscription(SubscriptionState::default()),
                crate::config::BillingModel::Free => QuotaSource::Free,
                crate::config::BillingModel::PayAsYouGo => QuotaSource::PayAsYouGo(PayAsYouGoState::default()),
            };

            states.insert(identity.clone(), ProviderState {
                identity: identity.clone(),
                api_surface: cfg.api_surface.clone(),
                billing: cfg.billing.clone(),
                models,
                has_valid_credential: has_credential,
                status,
                pricing: cfg.pricing.clone(),
            });
            quotas.insert(identity.clone(), ProviderQuotaState::new(quota_source));
        }

        Self {
            states: Arc::new(RwLock::new(states)),
            quotas: Arc::new(RwLock::new(quotas)),
        }
    }

    pub async fn get_states(&self) -> HashMap<String, ProviderState> {
        self.states.read().await.clone()
    }

    pub async fn get_quota(&self, identity: &str) -> Option<ProviderQuotaState> {
        self.quotas.read().await.get(identity).cloned()
    }

    pub async fn update_quota(&self, identity: &str, quota: ProviderQuotaState) {
        self.quotas.write().await.insert(identity.to_string(), quota);
    }
}
