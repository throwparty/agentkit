use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::config::{ModelConfig, ProviderConfig, SwitchboardConfig};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("duplicate provider identity: {0}")]
    DuplicateIdentity(String),
}

#[derive(serde::Deserialize)]
struct RawConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    pub providers: Option<Vec<ProviderConfig>>,
    pub credential_helper: Option<String>,
    pub session_db_path: Option<PathBuf>,
}

pub fn load_config(path: &Path) -> Result<SwitchboardConfig, ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    let raw: RawConfig = toml::from_str(&contents)?;

    let mut providers = HashMap::new();
    if let Some(list) = raw.providers {
        for p in list {
            if providers.contains_key(&p.identity) {
                return Err(ConfigError::DuplicateIdentity(p.identity));
            }
            providers.insert(p.identity.clone(), p);
        }
    }

    Ok(SwitchboardConfig {
        models: raw.models,
        providers,
        credential_helper: raw.credential_helper,
        session_db_path: raw.session_db_path,
    })
}
