pub mod loader;

use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchboardConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    pub providers: HashMap<String, ProviderConfig>,
    pub credential_helper: Option<String>,
    pub session_db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    pub context_window: Option<u32>,
    pub max_output: Option<u32>,
    pub capabilities: Option<Capabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub tool_calling: Option<bool>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub identity: String,
    pub api_surface: ApiSurface,
    pub base_url: String,
    pub billing: BillingModel,
    pub auth: AuthConfig,
    pub pricing: PricingConfig,
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApiSurface {
    #[serde(rename = "openai")]
    Openai,
}

impl std::fmt::Display for ApiSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Openai => write!(f, "openai"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BillingModel {
    #[serde(rename = "subscription")]
    Subscription,
    #[serde(rename = "pay_as_you_go")]
    PayAsYouGo,
    #[serde(rename = "free")]
    Free,
}

impl std::fmt::Display for BillingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscription => write!(f, "subscription"),
            Self::PayAsYouGo => write!(f, "pay_as_you_go"),
            Self::Free => write!(f, "free"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub r#type: AuthType,
    pub credential_env: Option<String>,
    pub oauth: Option<OAuthEndpointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    #[serde(rename = "bearer_token")]
    BearerToken,
    #[serde(rename = "openai_codex_oauth")]
    OpenAICodexOAuth,
    #[serde(rename = "anthropic_api_key")]
    AnthropicApiKey,
    #[serde(rename = "oauth_token")]
    OAuthToken,
    #[serde(rename = "none")]
    None,
}

impl std::fmt::Display for AuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken => write!(f, "bearer_token"),
            Self::OpenAICodexOAuth => write!(f, "openai_codex_oauth"),
            Self::AnthropicApiKey => write!(f, "anthropic_api_key"),
            Self::OAuthToken => write!(f, "oauth_token"),
            Self::None => write!(f, "none"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthEndpointConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    pub reasoning_per_mtok: Option<f64>,
    #[serde(default)]
    pub models: HashMap<String, PerModelPricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerModelPricing {
    pub input_per_mtok: Option<f64>,
    pub output_per_mtok: Option<f64>,
    pub cache_read_per_mtok: Option<f64>,
    pub cache_write_per_mtok: Option<f64>,
    pub reasoning_per_mtok: Option<f64>,
}
