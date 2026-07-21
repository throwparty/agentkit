pub mod anthropic;
pub mod openai;

use crate::config::ProviderConfig;
use std::collections::HashMap;

pub type ProviderMap = HashMap<String, ProviderPack>;

pub struct ProviderPack {
    pub http: Box<dyn crate::domain::http::HttpEndpoint>,
    pub conversation: Box<dyn crate::domain::conversation::ConversationHandler>,
    pub new_quota: Box<dyn crate::domain::quota::ProviderQuotaBehaviour>,
}

fn pack_for_provider() -> ProviderPack {
    ProviderPack {
        http: Box::new(openai::OpenAiProvider),
        conversation: Box::new(openai::conversation::OpenAiConversation),
        new_quota: Box::new(openai::quota::OpenAiQuota::default()),
    }
}

pub fn build_provider_map(configs: &HashMap<String, ProviderConfig>) -> ProviderMap {
    configs.keys().map(|identity| (identity.clone(), pack_for_provider())).collect()
}
