use crate::config::ProviderConfig;
use crate::credential;
use crate::provider::quota::{
    handle_response_status, PayAsYouGoState, ProviderQuotaState, QuotaSource, SubscriptionState,
};
use crate::provider::{ProviderState, ProviderStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ProviderRegistry {
    states: Arc<RwLock<HashMap<String, ProviderState>>>,
    quotas: Arc<RwLock<HashMap<String, ProviderQuotaState>>>,
}

impl ProviderRegistry {
    pub fn new(configs: &HashMap<String, ProviderConfig>, helper_name: &str) -> Self {
        let mut states = HashMap::new();
        let mut quotas = HashMap::new();

        for (identity, cfg) in configs {
            let has_credential = credential::resolve_provider(helper_name, identity, cfg).is_some();
            let status = if has_credential {
                ProviderStatus::Healthy
            } else {
                ProviderStatus::Unconfigured
            };

            let models = cfg.models.clone().unwrap_or_default();
            let quota_source = match cfg.billing {
                crate::config::BillingModel::Subscription => {
                    QuotaSource::Subscription(SubscriptionState::default())
                }
                crate::config::BillingModel::Free => QuotaSource::Free,
                crate::config::BillingModel::PayAsYouGo => {
                    QuotaSource::PayAsYouGo(PayAsYouGoState::default())
                }
            };

            states.insert(
                identity.clone(),
                ProviderState {
                    identity: identity.clone(),
                    api_surface: cfg.api_surface.clone(),
                    base_url: cfg.base_url.clone(),
                    billing: cfg.billing.clone(),
                    models,
                    has_valid_credential: has_credential,
                    status,
                    pricing: cfg.pricing.clone(),
                },
            );
            quotas.insert(identity.clone(), ProviderQuotaState::new(quota_source));
        }

        Self {
            states: Arc::new(RwLock::new(states)),
            quotas: Arc::new(RwLock::new(quotas)),
        }
    }

    pub async fn get_states(&self) -> HashMap<String, ProviderState> {
        let mut quotas = self.quotas.write().await;
        let mut states = self.states.write().await;

        for (identity, quota) in quotas.iter_mut() {
            quota.check_expired();
            if let Some(state) = states.get_mut(identity) {
                if quota.is_degraded() {
                    state.status = ProviderStatus::Degraded;
                } else if matches!(state.status, ProviderStatus::Degraded)
                    && state.has_valid_credential
                {
                    state.status = ProviderStatus::Healthy;
                }
            }
        }

        states.clone()
    }

    pub async fn get_quota(&self, identity: &str) -> Option<ProviderQuotaState> {
        self.quotas.read().await.get(identity).cloned()
    }

    pub async fn update_quota(&self, identity: &str, quota: ProviderQuotaState) {
        self.quotas
            .write()
            .await
            .insert(identity.to_string(), quota);
    }

    pub async fn degrade_provider(&self, identity: &str) {
        let mut states = self.states.write().await;
        if let Some(p) = states.get_mut(identity) {
            p.has_valid_credential = false;
            p.status = crate::provider::ProviderStatus::Unconfigured;
        }
    }

    pub async fn record_response(
        &self,
        identity: &str,
        status: u16,
        headers: &[(String, String)],
        body: Option<&str>,
    ) {
        let mut quotas = self.quotas.write().await;
        let mut states = self.states.write().await;

        let Some(quota) = quotas.get_mut(identity) else {
            return;
        };

        handle_response_status(quota, status, headers, body);

        if let Some(state) = states.get_mut(identity) {
            state.status = if quota.is_degraded() {
                ProviderStatus::Degraded
            } else if state.has_valid_credential {
                ProviderStatus::Healthy
            } else {
                ProviderStatus::Unconfigured
            };
        }
    }
}
