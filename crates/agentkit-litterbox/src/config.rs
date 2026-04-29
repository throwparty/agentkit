use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub ports: PortsConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub slug: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerConfig {
    pub image: Option<String>,
    #[serde(rename = "setup-command")]
    pub setup_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardedPort {
    pub name: String,
    pub target: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PortsConfig {
    pub ports: Vec<ForwardedPort>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Missing required key: {0}")]
    MissingRequiredKey(String),
}

#[cfg(test)]
mod tests {
    use super::{Config, ForwardedPort, PortsConfig};

    #[test]
    fn forwarded_port_instantiates() {
        let port = ForwardedPort {
            name: "backend".to_string(),
            target: 8080,
        };

        assert_eq!(port.name, "backend");
        assert_eq!(port.target, 8080);
    }

    #[test]
    fn ports_config_instantiates() {
        let ports = PortsConfig {
            ports: vec![ForwardedPort {
                name: "frontend".to_string(),
                target: 8081,
            }],
        };

        assert_eq!(ports.ports.len(), 1);
        assert_eq!(ports.ports[0].name, "frontend");
        assert_eq!(ports.ports[0].target, 8081);
    }

    #[test]
    fn ports_config_defaults_to_empty() {
        let ports = PortsConfig::default();

        assert!(ports.ports.is_empty());
    }

    #[test]
    fn config_deserializes_without_ports() {
        let input = r#"
docker = { image = "image", setup-command = "setup" }
"#;
        let config: Config = toml::from_str(input).expect("config parses");

        assert!(config.ports.ports.is_empty());
    }

    #[test]
    fn config_deserializes_with_ports() {
        let input = r#"
docker = { image = "image", setup-command = "setup" }

[[ports]]
name = "backend"
target = 8080

[[ports]]
name = "frontend"
target = 8081
"#;
        let config: Config = toml::from_str(input).expect("config parses");

        assert_eq!(config.ports.ports.len(), 2);
        assert_eq!(config.ports.ports[0].name, "backend");
        assert_eq!(config.ports.ports[0].target, 8080);
        assert_eq!(config.ports.ports[1].name, "frontend");
        assert_eq!(config.ports.ports[1].target, 8081);
    }
}
