//! Loading and merging user and project configuration.
//!
//! Order: defaults, then the user's file, then the project's file, then the
//! repository-local override. The user owns the mechanism; project files may
//! declare what the project needs but cannot select or configure a backend.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError, ProjectFile, UserFile, merge};
use crate::domain::slugify_name;

const PROJECT_FILE: &str = ".litterbox.toml";
const LOCAL_FILE: &str = ".litterbox.local.toml";

/// Where the user's configuration lives.
pub fn user_config_path() -> PathBuf {
    agentkit_common::config_dir("litterbox").join("config.toml")
}

/// Load the merged configuration.
pub fn load_final() -> Result<Config, ConfigError> {
    let user = match read_to_string(&user_config_path())? {
        Some(text) => parse_user(&text)?,
        None => UserFile::default(),
    };

    let project = match read_to_string(Path::new(PROJECT_FILE))? {
        Some(text) => parse_project(&text)?,
        None => return Err(ConfigError::FileNotFound(PathBuf::from(PROJECT_FILE))),
    };

    let local = match read_to_string(Path::new(LOCAL_FILE))? {
        Some(text) => Some(parse_project(&text)?),
        None => None,
    };

    let config = merge(user, project, local);
    validate_ports(&config)?;
    Ok(config)
}

fn read_to_string(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ConfigError::FileNotFound(path.to_path_buf())),
    }
}

fn parse_user(text: &str) -> Result<UserFile, ConfigError> {
    toml::from_str(text).map_err(|error| ConfigError::ParseError(error.to_string()))
}

fn parse_project(text: &str) -> Result<ProjectFile, ConfigError> {
    reject_moved_sections(text)?;
    toml::from_str(text).map_err(|error| ConfigError::ParseError(error.to_string()))
}

/// A project file that still carries `[docker]` gets told where its keys went,
/// rather than an anonymous unknown-field error.
fn reject_moved_sections(text: &str) -> Result<(), ConfigError> {
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return Ok(());
    };
    if value.get("docker").is_some() {
        return Err(ConfigError::ParseError(
            "the [docker] section has moved: setup-command belongs under [project], and image \
             belongs in the user configuration under [backends.docker]"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_ports(config: &Config) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();

    for port in &config.ports {
        if port.target == 0 {
            return Err(ConfigError::ParseError(format!(
                "Invalid forwarded port target: {}",
                port.target
            )));
        }
        let slug =
            slugify_name(&port.name).map_err(|err| ConfigError::ParseError(err.to_string()))?;
        if !seen.insert(slug.clone()) {
            return Err(ConfigError::ParseError(format!(
                "Duplicate forwarded port name after slugify: '{slug}'"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{reject_moved_sections, validate_ports};
    use crate::config::{Config, PortRequest};

    fn config_with(ports: Vec<PortRequest>) -> Config {
        Config {
            ports,
            ..Config::default()
        }
    }

    #[test]
    fn validate_ports_allows_unique_slugs() {
        let config = config_with(vec![
            PortRequest {
                name: "Backend".to_string(),
                target: 8080,
            },
            PortRequest {
                name: "Frontend".to_string(),
                target: 8081,
            },
        ]);

        validate_ports(&config).expect("ports validate");
    }

    #[test]
    fn validate_ports_rejects_duplicate_slugs() {
        let config = config_with(vec![
            PortRequest {
                name: "My Service".to_string(),
                target: 8080,
            },
            PortRequest {
                name: "my-service".to_string(),
                target: 8081,
            },
        ]);

        let err = validate_ports(&config).expect_err("duplicate slug rejected");
        assert!(err.to_string().contains("Duplicate forwarded port name"));
    }

    #[test]
    fn validate_ports_rejects_invalid_names() {
        let config = config_with(vec![PortRequest {
            name: "----".to_string(),
            target: 8080,
        }]);

        let err = validate_ports(&config).expect_err("invalid slug rejected");
        assert!(err.to_string().contains("Invalid sandbox name"));
    }

    #[test]
    fn validate_ports_rejects_invalid_targets() {
        let config = config_with(vec![PortRequest {
            name: "backend".to_string(),
            target: 0,
        }]);

        let err = validate_ports(&config).expect_err("invalid target rejected");
        assert!(err.to_string().contains("Invalid forwarded port target"));
    }

    #[test]
    fn a_moved_docker_section_is_explained() {
        let text = "[docker]\nimage = 'busybox'\n";
        let error = reject_moved_sections(text).expect_err("moved section rejected");
        let message = error.to_string();
        assert!(message.contains("[project]"), "{message}");
        assert!(message.contains("[backends.docker]"), "{message}");
    }

    #[test]
    fn an_empty_table_is_not_a_moved_section() {
        let text = "[project]\nslug = 'demo'\n";
        reject_moved_sections(text).expect("no moved section");
        let _ = BTreeMap::<String, toml::Value>::new();
    }
}
