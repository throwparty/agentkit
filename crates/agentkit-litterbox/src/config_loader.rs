//! Loading and merging project configuration and its local override.
//!
//! Order: defaults, then the committed project file, then the untracked local
//! override. The person running the code owns the mechanism; the committed file
//! may declare what the project needs but cannot select or configure a backend.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use git2::Repository;

use crate::config::{Config, ConfigError, ProjectFile, merge};
use crate::sandbox::contract::forwarded_port_env_var;

const PROJECT_FILE: &str = ".agentkit/litterbox/config.toml";
const LOCAL_FILE: &str = ".agentkit/litterbox/config.local.toml";

/// Load the merged configuration.
pub fn load_final() -> Result<Config, ConfigError> {
    let project = match read_to_string(Path::new(PROJECT_FILE))? {
        Some(text) => parse_project(&text)?,
        None => return Err(ConfigError::FileNotFound(PathBuf::from(PROJECT_FILE))),
    };

    let local_path = Path::new(LOCAL_FILE);
    validate_local_file(local_path)?;
    let local = match read_to_string(local_path)? {
        Some(text) => parse_project(&text)?,
        None => ProjectFile::default(),
    };

    let config = merge(local, project);
    validate_forwarding(&config)?;
    validate_ports(&config)?;
    Ok(config)
}

/// Load only the local override, for commands that are host-scoped.
pub fn load_local_only() -> Result<Config, ConfigError> {
    let local_path = Path::new(LOCAL_FILE);
    validate_local_file(local_path)?;
    let local = match read_to_string(local_path)? {
        Some(text) => parse_project(&text)?,
        None => ProjectFile::default(),
    };
    let config = merge(local, ProjectFile::default());
    validate_forwarding(&config)?;
    Ok(config)
}

fn read_to_string(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            detail: error.to_string(),
        }),
    }
}

/// A local override must be a regular, untracked file. A repository can ship a
/// tracked `config.local.toml`, and a symlink can point at one; neither is a
/// user decision, so neither may grant setup consent or choose a backend.
fn validate_local_file(path: &Path) -> Result<(), ConfigError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                detail: error.to_string(),
            });
        }
    };

    if metadata.file_type().is_symlink() {
        return Err(ConfigError::ParseError(format!(
            "{} must not be a symlink; create a regular, untracked file",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(ConfigError::ParseError(format!(
            "{} must be a regular file",
            path.display()
        )));
    }

    let Ok(repository) = Repository::discover(path) else {
        return Ok(());
    };
    let Some(workdir) = repository.workdir() else {
        return Ok(());
    };
    let relative = path.strip_prefix(workdir).unwrap_or(path);
    let tracked = repository
        .index()
        .ok()
        .is_some_and(|index| index.get_path(relative, 0).is_some());
    if tracked {
        return Err(ConfigError::ParseError(format!(
            "{} is tracked by Git; keep the local override untracked",
            path.display()
        )));
    }
    Ok(())
}

fn parse_project(text: &str) -> Result<ProjectFile, ConfigError> {
    reject_moved_sections(text)?;
    toml::from_str(text).map_err(|error| ConfigError::ParseError(error.to_string()))
}

/// A project file whose keys are under a mechanism's name gets told where they
/// went, rather than an anonymous unknown-field error.
///
/// The loader names no backend: it reports the section it found.
fn reject_moved_sections(text: &str) -> Result<(), ConfigError> {
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return Ok(());
    };
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    // Every key ProjectFile defines is project-owned here; the backend and
    // forwarding keys are accepted so merge can report and ignore them.
    if let Some(section) = table.keys().find(|key| {
        !matches!(
            key.as_str(),
            "project"
                | "ports"
                | "git"
                | "backend"
                | "backends"
                | "forwarding"
                | "accept-degraded"
                | "allow-project-setup"
                | "allowed-projects"
        )
    }) {
        return Err(ConfigError::ParseError(format!(
            "the [{section}] section is not project configuration: project keys belong under \
             [project], and mechanism settings belong in the user configuration under \
             [backends.{section}]"
        )));
    }
    Ok(())
}

/// The most ports a project may ask the host to publish.
const MAX_PORTS: usize = 32;

fn validate_forwarding(config: &Config) -> Result<(), ConfigError> {
    config
        .forwarding
        .bind
        .parse::<std::net::IpAddr>()
        .map_err(|_| {
            ConfigError::ParseError(format!(
                "forwarding.bind '{}' is not an IP address; use one such as 127.0.0.1 or 0.0.0.0",
                config.forwarding.bind
            ))
        })?;
    Ok(())
}

fn validate_ports(config: &Config) -> Result<(), ConfigError> {
    if config.ports.len() > MAX_PORTS {
        return Err(ConfigError::ParseError(format!(
            "this project declares {} forwarded ports; the limit is {MAX_PORTS}",
            config.ports.len()
        )));
    }

    let mut seen = HashSet::new();
    let mut targets = HashSet::new();

    for port in &config.ports {
        if port.target == 0 {
            return Err(ConfigError::ParseError(format!(
                "Invalid forwarded port target: {}",
                port.target
            )));
        }
        let env_var = forwarded_port_env_var(&port.name).map_err(ConfigError::ParseError)?;
        if !seen.insert(env_var.clone()) {
            return Err(ConfigError::ParseError(format!(
                "Duplicate forwarded port name after normalisation: '{}'",
                port.name
            )));
        }
        if !targets.insert(port.target) {
            return Err(ConfigError::ParseError(format!(
                "Duplicate forwarded port target: {} (port '{}')",
                port.target, port.name
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::{
        parse_project, reject_moved_sections, validate_forwarding, validate_local_file,
        validate_ports,
    };
    use crate::config::{Config, PortRequest, ProjectFile, merge};

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
        assert!(err.to_string().contains("not a valid port name"), "{err}");
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
    fn validate_ports_rejects_duplicate_targets() {
        let config = config_with(vec![
            PortRequest {
                name: "web".to_string(),
                target: 8080,
            },
            PortRequest {
                name: "admin".to_string(),
                target: 8080,
            },
        ]);

        let err = validate_ports(&config).expect_err("duplicate target rejected");
        assert!(err.to_string().contains("Duplicate forwarded port target"));
    }

    #[test]
    fn validate_ports_caps_a_hostile_project() {
        let config = config_with(
            (0..33)
                .map(|index| PortRequest {
                    name: format!("port-{index}"),
                    target: 8000 + index as u16,
                })
                .collect(),
        );
        let err = validate_ports(&config).expect_err("too many ports rejected");
        assert!(err.to_string().contains("limit is 32"), "{err}");
    }

    #[test]
    fn the_example_project_configuration_still_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/litterbox-static-site/.agentkit/litterbox/config.toml");
        let text = std::fs::read_to_string(&path).expect("the example exists");
        let project = parse_project(&text).expect("the example parses");
        let config = merge(ProjectFile::default(), project);
        validate_ports(&config).expect("the example's ports validate");
        validate_forwarding(&config).expect("the example uses a valid bind");
    }

    #[test]
    fn a_bind_that_is_not_an_address_is_refused() {
        let mut config = Config::default();
        config.forwarding.bind = "127.0.0.1 ".to_string();
        let err = validate_forwarding(&config).expect_err("bad bind rejected");
        assert!(err.to_string().contains("forwarding.bind"), "{err}");

        config.forwarding.bind = "0.0.0.0".to_string();
        validate_forwarding(&config).expect("a valid bind");
    }

    #[test]
    fn every_project_key_loads_and_report_only_keys_still_reach_merge() {
        let text = r#"
backend = "bubblewrap"
accept-degraded = true
allow-project-setup = true
allowed-projects = ["project"]

[project]
setup-command = "echo hi"

[git]
snapshot-mode = "direct"

[backends.docker]
image = "busybox"

[forwarding]
bind = "0.0.0.0"
"#;
        let project = parse_project(text).expect("the documented project keys parse");
        let config = merge(ProjectFile::default(), project);
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("names a backend"))
        );
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("configures a backend"))
        );
        assert!(config.notices.iter().any(|notice| notice.contains("bind")));
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("setup consent"))
        );
    }

    #[test]
    fn a_tracked_local_override_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repository = git2::Repository::init(dir.path()).expect("repo");
        let relative = std::path::Path::new(".agentkit/litterbox/config.local.toml");
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        {
            let mut index = repository.index().expect("index");
            index.add_path(relative).expect("track");
            index.write().expect("write index");
        }

        let error = validate_local_file(&path).expect_err("tracked file refused");
        assert!(error.to_string().contains("tracked by Git"), "{error}");
    }

    #[test]
    fn a_regular_untracked_local_override_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.local.toml");
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        validate_local_file(&path).expect("untracked file accepted");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_local_override_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("elsewhere.toml");
        std::fs::write(&target, "backend = \"docker\"\n").expect("write");
        let link = dir.path().join("config.local.toml");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let error = validate_local_file(&link).expect_err("symlink refused");
        assert!(
            error.to_string().contains("must not be a symlink"),
            "{error}"
        );
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
    }
}
