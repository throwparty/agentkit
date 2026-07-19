use crate::config::ProviderConfig;
use crate::credential;
use crate::domain::quota::{handle_response_status, ProviderQuotaState};
use crate::provider::{ProviderRuntime, ProviderStatus, ProviderView};
use crate::providers::{build_provider_map, ProviderMap, ProviderPack};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ProviderRegistry {
    pub configs: HashMap<String, ProviderConfig>,
    pub provider_packs: ProviderMap,
    states: Arc<RwLock<HashMap<String, ProviderRuntime>>>,
    quotas: Arc<RwLock<HashMap<String, ProviderQuotaState>>>,
}

impl ProviderRegistry {
    pub fn new(configs: &HashMap<String, ProviderConfig>, helper_name: &str) -> Self {
        let mut states = HashMap::new();
        let mut quotas = HashMap::new();
        let provider_packs = build_provider_map(configs);

        for (identity, cfg) in configs {
            let has_credential = credential::resolve_provider(helper_name, identity, cfg).is_some();
            let status = if has_credential {
                ProviderStatus::Healthy
            } else {
                ProviderStatus::Unauthenticated
            };

            states.insert(
                identity.clone(),
                ProviderRuntime {
                    has_valid_credential: has_credential,
                    status,
                },
            );

            let quota = provider_packs
                .get(identity)
                .map(|p| ProviderQuotaState::new(p.new_quota.clone_box()))
                .unwrap_or_else(|| {
                    ProviderQuotaState::new(Box::new(crate::providers::openai::quota::OpenAiQuota::default()))
                });
            quotas.insert(identity.clone(), quota);
        }

        Self {
            configs: configs.clone(),
            provider_packs,
            states: Arc::new(RwLock::new(states)),
            quotas: Arc::new(RwLock::new(quotas)),
        }
    }

    pub fn get_provider_pack(&self, identity: &str) -> Option<&ProviderPack> {
        self.provider_packs.get(identity)
    }

    pub async fn get_states(&self) -> HashMap<String, ProviderView> {
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

        let mut out = HashMap::new();
        for (identity, runtime) in states.iter() {
            if let Some(config) = self.configs.get(identity) {
                let models = config.models.clone().unwrap_or_default();
                out.insert(
                    identity.clone(),
                    ProviderView {
                        identity: identity.clone(),
                        billing: config.billing.clone(),
                        models,
                        pricing: config.pricing.clone(),
                        has_valid_credential: runtime.has_valid_credential,
                        status: runtime.status.clone(),
                    },
                );
            }
        }
        out
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
            p.status = ProviderStatus::Unauthenticated;
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
                ProviderStatus::Unauthenticated
            };
        }
    }
}
