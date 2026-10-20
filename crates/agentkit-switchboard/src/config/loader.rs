use crate::config::{ProviderConfig, SwitchboardConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("duplicate provider identity: {0}")]
    DuplicateIdentity(String),
    #[error("invalid base_url for provider '{0}': {1}")]
    InvalidBaseUrl(String, String),
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    pub providers: Option<Vec<ProviderConfig>>,
    pub credential_helper: Option<String>,
    pub session_db_path: Option<PathBuf>,
}

pub fn load_config(path: &Path) -> Result<SwitchboardConfig, ConfigError> {
    let contents = std::fs::read_to_string(path)?;
    let raw: RawConfig = toml::from_str(&contents)?;

    let config_dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut providers = HashMap::new();
    if let Some(list) = raw.providers {
        for mut p in list {
            if providers.contains_key(&p.identity) {
                return Err(ConfigError::DuplicateIdentity(p.identity));
            }
            if url::Url::parse(&p.base_url).is_err() {
                return Err(ConfigError::InvalidBaseUrl(
                    p.identity.clone(),
                    p.base_url.clone(),
                ));
            }
            p.mutations = p
                .mutations
                .iter()
                .map(|mutation| resolve_script_path(&config_dir, mutation))
                .collect();
            providers.insert(p.identity.clone(), p);
        }
    }

    Ok(SwitchboardConfig {
        providers,
        credential_helper: raw.credential_helper,
        session_db_path: raw.session_db_path,
    })
}

/// Resolve a mutation script path against the config file's directory.
///
/// Absolute paths pass through untouched; relative paths are interpreted
/// relative to the directory holding the config file so a configuration can be
/// relocated wholesale.
fn resolve_script_path(config_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir.join(path)
    }
}
