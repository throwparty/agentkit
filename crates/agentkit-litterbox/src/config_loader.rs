use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::{Config, ConfigError, PortsConfig};
use crate::domain::slugify_name;

/// Loads and parses a single TOML configuration file into a Config struct.
pub fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|_| ConfigError::FileNotFound(path.to_path_buf()))?;

    toml::from_str(&contents)
        .map_err(|e| ConfigError::ParseError(e.to_string()))
}

/// Merges two Config structs, with values from `local` overriding `base`.
pub fn merge(base: Config, local: Config) -> Config {
    Config {
        project: crate::config::ProjectConfig {
            slug: local.project.slug.or(base.project.slug),
        },
        docker: crate::config::DockerConfig {
            image: local.docker.image.or(base.docker.image),
            setup_command: local.docker.setup_command.or(base.docker.setup_command),
        },
        ports: PortsConfig {
            ports: if local.ports.ports.is_empty() {
                base.ports.ports
            } else {
                local.ports.ports
            },
        },
    }
}

/// Creates a default configuration based on the current directory.
fn default_config() -> Config {
    let current_dir = std::env::current_dir().ok();
    let project_slug = current_dir
        .as_ref()
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .map(crate::domain::slugify)
        .filter(|slug| !slug.is_empty());

    Config {
        project: crate::config::ProjectConfig {
            slug: project_slug,
        },
        docker: crate::config::DockerConfig {
            image: None,
            setup_command: None,
        },
        ports: PortsConfig::default(),
    }
}

/// Loads the final merged configuration from defaults, .litterbox.toml, and .litterbox.local.toml.
pub fn load_final() -> Result<Config, ConfigError> {
    // Start with defaults
    let defaults = default_config();

    // Load project config
    let base_path = Path::new(".litterbox.toml");
    let base_config = load_file(base_path)?;

    // Load local config if it exists
    let local_path = Path::new(".litterbox.local.toml");
    let local_config = if local_path.exists() {
        load_file(local_path)?
    } else {
        // Empty config for merging
        Config {
            project: crate::config::ProjectConfig { slug: None },
            docker: crate::config::DockerConfig {
                image: None,
                setup_command: None,
            },
            ports: PortsConfig::default(),
        }
    };

    // Merge: defaults <- project <- local
    let merged = merge(merge(defaults, base_config), local_config);

    // Validate required keys
    if merged.docker.image.as_deref().unwrap_or("").is_empty() {
        return Err(ConfigError::MissingRequiredKey("docker.image".to_string()));
    }
    if merged.docker.setup_command.as_deref().unwrap_or("").is_empty() {
        return Err(ConfigError::MissingRequiredKey("docker.setup-command".to_string()));
    }

    validate_ports(&merged)?;

    Ok(merged)
}

fn validate_ports(config: &Config) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();

    for port in &config.ports.ports {
        if port.target == 0 {
            return Err(ConfigError::ParseError(format!(
                "Invalid forwarded port target: {}",
                port.target
            )));
        }
        let slug = slugify_name(&port.name).map_err(|err| ConfigError::ParseError(err.to_string()))?;
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
    use super::validate_ports;
    use crate::config::{Config, DockerConfig, PortsConfig, ProjectConfig, ForwardedPort};

    fn base_config(ports: Vec<ForwardedPort>) -> Config {
        Config {
            project: ProjectConfig { slug: None },
            docker: DockerConfig {
                image: Some("image".to_string()),
                setup_command: Some("setup".to_string()),
            },
            ports: PortsConfig { ports },
        }
    }

    #[test]
    fn validate_ports_allows_unique_slugs() {
        let config = base_config(vec![
            ForwardedPort {
                name: "Backend".to_string(),
                target: 8080,
            },
            ForwardedPort {
                name: "Frontend".to_string(),
                target: 8081,
            },
        ]);

        validate_ports(&config).expect("ports validate");
    }

    #[test]
    fn validate_ports_rejects_duplicate_slugs() {
        let config = base_config(vec![
            ForwardedPort {
                name: "My Service".to_string(),
                target: 8080,
            },
            ForwardedPort {
                name: "my-service".to_string(),
                target: 8081,
            },
        ]);

        let err = validate_ports(&config).expect_err("duplicate slug rejected");
        assert!(err.to_string().contains("Duplicate forwarded port name"));
    }

    #[test]
    fn validate_ports_rejects_invalid_names() {
        let config = base_config(vec![ForwardedPort {
            name: "----".to_string(),
            target: 8080,
        }]);

        let err = validate_ports(&config).expect_err("invalid slug rejected");
        assert!(err.to_string().contains("Invalid sandbox name"));
    }

    #[test]
    fn validate_ports_rejects_invalid_targets() {
        let config = base_config(vec![ForwardedPort {
            name: "backend".to_string(),
            target: 0,
        }]);

        let err = validate_ports(&config).expect_err("invalid target rejected");
        assert!(err.to_string().contains("Invalid forwarded port target"));
    }
}
