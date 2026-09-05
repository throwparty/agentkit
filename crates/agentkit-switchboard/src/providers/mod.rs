pub mod anthropic;
pub mod openai;

use crate::config::ProviderConfig;
use std::collections::HashMap;

pub type ProviderMap = HashMap<String, ProviderPack>;

pub struct ProviderPack {
    pub http: Box<dyn crate::domain::http::HttpEndpoint>,
    pub new_quota: Box<dyn crate::domain::quota::ProviderQuotaBehaviour>,
}

pub fn pack_for_provider(config: &ProviderConfig) -> ProviderPack {
    match config.api_surface {
        crate::config::ApiSurface::AnthropicMessages => ProviderPack {
            http: Box::new(anthropic::AnthropicProvider),
            new_quota: Box::new(anthropic::quota::AnthropicQuota::default()),
        },
        crate::config::ApiSurface::OpenaiChatCompletions => ProviderPack {
            http: Box::new(openai::OpenAiChatCompletionsProvider),
            new_quota: Box::new(openai::quota::OpenAiQuota::default()),
        },
        crate::config::ApiSurface::OpenaiResponses => ProviderPack {
            http: Box::new(openai::OpenAiResponsesProvider),
            new_quota: Box::new(openai::quota::OpenAiQuota::default()),
        },
    }
}

pub fn build_provider_map(configs: &HashMap<String, ProviderConfig>) -> ProviderMap {
    configs
        .iter()
        .map(|(identity, cfg)| (identity.clone(), pack_for_provider(cfg)))
        .collect()
}
