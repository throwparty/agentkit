//! Loading and merging project configuration and its local override.
//!
//! Order: defaults, then the committed project file, then the untracked local
//! override. The person running the code owns the mechanism; the committed file
//! may declare what the project needs but cannot select or configure a backend.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use git2::Repository;

use crate::config::{Config, ConfigError, ProjectFile, escaped, merge};
use crate::sandbox::contract::forwarded_port_env_var;

const PROJECT_FILE: &str = ".agentkit/litterbox/config.toml";
const LOCAL_FILE: &str = ".agentkit/litterbox/config.local.toml";

/// The top-level keys a configuration file may use, kept beside the loader so a
/// new `ProjectFile` field has one place to declare its accepted key.
const CONFIG_KEYS: &[&str] = &[
    "project",
    "provision",
    "ports",
    "git",
    "backend",
    "backends",
    "forwarding",
    "accept-degraded",
    "allow-project-setup",
    "allowed-projects",
];

/// Load the merged configuration.
pub fn load_final() -> Result<Config, ConfigError> {
    let project = match read_config_file(Path::new(PROJECT_FILE))? {
        Some(text) => parse_project(&text)?,
        None => return Err(ConfigError::FileNotFound(PathBuf::from(PROJECT_FILE))),
    };

    let local_path = Path::new(LOCAL_FILE);
    validate_local_file(local_path)?;
    let local = match read_config_file(local_path)? {
        Some(text) => parse_project(&text)?,
        None => ProjectFile::default(),
    };

    let mut config = merge(local, project)?;
    validate_forwarding(&config)?;
    validate_ports(&config)?;
    warn_public_ports(&mut config);
    validate_provisioning(&mut config)?;
    Ok(config)
}

/// Load only the local override, for commands that are host-scoped.
pub fn load_local_only() -> Result<Config, ConfigError> {
    let local_path = Path::new(LOCAL_FILE);
    validate_local_file(local_path)?;
    let local = match read_config_file(local_path)? {
        Some(text) => parse_project(&text)?,
        None => ProjectFile::default(),
    };
    let mut config = merge(local, ProjectFile::default())?;
    validate_forwarding(&config)?;
    warn_public_ports(&mut config);
    // Provisioning is a project-level decision: the local file may replace it,
    // but only the merged project configuration can decide whether the
    // supported set and sequences agree.
    Ok(config)
}

/// The maximum size of a configuration file. Configuration is small; a committed
/// symlink to a large file or a device must not be read.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Read a configuration file, rejecting symlinks, non-files and oversized files.
fn read_config_file(path: &Path) -> Result<Option<String>, ConfigError> {
    reject_symlinked_components(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                detail: error.to_string(),
            });
        }
    };

    if metadata.file_type().is_symlink() {
        return Err(ConfigError::ParseError(format!(
            "{} must not be a symlink; create a regular file",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(ConfigError::ParseError(format!(
            "{} must be a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::ParseError(format!(
            "{} is larger than {MAX_CONFIG_BYTES} bytes; configuration must be small",
            path.display()
        )));
    }

    let contents = fs::read_to_string(path).map_err(|error| ConfigError::Io {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    Ok(Some(contents))
}

/// Reject a configuration path reached through any symlinked component.
fn reject_symlinked_components(path: &Path) -> Result<(), ConfigError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(ConfigError::ParseError(format!(
                "{} is reached through a symlinked path component; use a regular file",
                path.display()
            )));
        }
    }
    Ok(())
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

    let Ok(canonical) = fs::canonicalize(path) else {
        // Absent or unreadable: read_config_file reports the latter.
        return Ok(());
    };

    let repository = Repository::discover(&canonical).map_err(|error| {
        ConfigError::ParseError(format!(
            "{} is not inside a Git worktree, so its tracking status cannot be checked and \
             it cannot be trusted as the user's override: {error}",
            path.display()
        ))
    })?;
    let Some(workdir) = repository.workdir() else {
        return Err(ConfigError::ParseError(format!(
            "{} is inside a bare Git repository, so its tracking status cannot be checked",
            path.display()
        )));
    };
    let canonical_workdir = fs::canonicalize(workdir).map_err(|error| ConfigError::Io {
        path: workdir.to_path_buf(),
        detail: error.to_string(),
    })?;
    let relative = canonical.strip_prefix(&canonical_workdir).map_err(|_| {
        ConfigError::ParseError(format!(
            "{} is outside the repository workdir and cannot be checked for tracking",
            path.display()
        ))
    })?;

    // An ancestor symlink can point at a tracked file while the final path
    // component looks regular. Walk every component from the workdir down.
    let mut current = canonical_workdir.clone();
    for component in relative.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| ConfigError::Io {
            path: current.clone(),
            detail: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ConfigError::ParseError(format!(
                "{} is reached through a symlinked path component; create a regular, untracked file",
                path.display()
            )));
        }
    }

    let index = repository.index().map_err(|error| {
        ConfigError::ParseError(format!(
            "could not read the Git index to check {}: {error}",
            path.display()
        ))
    })?;
    if index.get_path(relative, 0).is_some() {
        return Err(ConfigError::ParseError(format!(
            "{} is tracked by Git; keep the local override untracked and run `git rm --cached {}` \
             if it was committed",
            path.display(),
            path.display()
        )));
    }

    // A path can be absent from the index but present in the commit the index
    // is built from, such as after a `git rm --cached`.
    if let Ok(head) = repository.head()
        && let Ok(tree) = head.peel_to_tree()
        && tree.get_path(relative).is_ok()
    {
        return Err(ConfigError::ParseError(format!(
            "{} is committed to Git; keep the local override untracked and run \
             `git rm --cached {}`",
            path.display(),
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
    if let Some(project) = table.get("project").and_then(|value| value.as_table())
        && project.contains_key("setup-command")
    {
        return Err(ConfigError::ParseError(
            "[project] setup-command has moved: declare a [[provision]] step with \
             kind = 'command' and its own identity, and list that identity in \
             [backends.<name>].provision for each backend that should run it"
                .to_string(),
        ));
    }
    // Every key ProjectFile defines is project-owned here; the backend and
    // forwarding keys are accepted so merge can report and ignore them.
    if let Some(section) = table
        .keys()
        .find(|key| !CONFIG_KEYS.contains(&key.as_str()))
    {
        let section = escaped(section);
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

/// Warn when project-declared ports are published beyond loopback.
fn warn_public_ports(config: &mut Config) {
    if !config.ports.is_empty()
        && config.forwarding.bind != "127.0.0.1"
        && config.forwarding.bind != "::1"
    {
        config.notices.push(format!(
            "project ports will be published on {} rather than loopback; set forwarding.bind to \
             127.0.0.1 unless the network exposure is deliberate",
            config.forwarding.bind
        ));
    }
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

/// Validate the provisioning schema after the local override is merged.
///
/// The rules live in `Config::validate_provisioning`, so the resolver applies
/// the same ones without the loader depending on the resolver.
fn validate_provisioning(config: &mut Config) -> Result<(), ConfigError> {
    config
        .validate_provisioning()
        .map_err(ConfigError::Provisioning)
}

#[cfg(test)]
mod tests {

    use super::{
        MAX_CONFIG_BYTES, parse_project, read_config_file, reject_moved_sections,
        validate_forwarding, validate_local_file, validate_ports, validate_provisioning,
    };
    use crate::config::{Config, ConfigError, PortRequest, ProjectFile, merge};
    use crate::sandbox::contract::ProvisionStep;

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
    fn the_committed_project_configurations_still_parse() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for relative in [
            ".agentkit/litterbox/config.toml",
            "examples/litterbox-static-site/.agentkit/litterbox/config.toml",
        ] {
            let path = root.join(relative);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let project =
                parse_project(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let mut config = merge(ProjectFile::default(), project)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            validate_ports(&config).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            validate_forwarding(&config)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            validate_provisioning(&mut config)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
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

[project]
slug = "project"
backends = ["docker"]

[[provision]]
identity = "fetch"
kind = "command"
run = "true"

[[ports]]
name = "web"
target = 8080

[git]
snapshot-mode = "direct"

[backends.docker]
image = "busybox"
provision = ["fetch"]

[forwarding]
bind = "0.0.0.0"
"#;
        for key in super::CONFIG_KEYS {
            if matches!(*key, "allow-project-setup" | "allowed-projects") {
                // These are accepted keys but merge refuses them in a project file.
                continue;
            }
            assert!(text.contains(key), "the fixture covers {key}");
        }
        let project = parse_project(text).expect("the documented project keys parse");
        let config = merge(ProjectFile::default(), project).expect("merge");
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
    }

    #[test]
    fn a_project_granting_itself_consent_is_refused() {
        let project: ProjectFile = toml::from_str("allow-project-setup = true\n").expect("project");
        let error = merge(ProjectFile::default(), project).expect_err("consent refused");
        assert!(
            error.to_string().contains("must not set setup consent"),
            "{error}"
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
        git2::Repository::init(dir.path()).expect("repo");
        let path = dir.path().join("config.local.toml");
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        validate_local_file(&path).expect("untracked file accepted");
    }

    #[test]
    fn a_local_override_outside_a_worktree_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.local.toml");
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        let error = validate_local_file(&path).expect_err("outside a worktree refused");
        assert!(
            error.to_string().contains("not inside a Git worktree"),
            "{error}"
        );
    }

    #[test]
    fn a_committed_local_override_is_refused_even_when_removed_from_the_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repository = git2::Repository::init(dir.path()).expect("repo");
        let relative = std::path::Path::new("config.local.toml");
        let path = dir.path().join(relative);
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        let signature = git2::Signature::now("Test", "test@example.com").expect("signature");
        {
            let mut index = repository.index().expect("index");
            index.add_path(relative).expect("track");
            index.write().expect("write index");
            let tree_id = index.write_tree().expect("tree");
            let tree = repository.find_tree(tree_id).expect("find tree");
            repository
                .commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
            index.remove_path(relative).expect("untrack");
            index.write().expect("write index");
        }

        let error = validate_local_file(&path).expect_err("committed file refused");
        assert!(error.to_string().contains("committed to Git"), "{error}");
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

    fn command(identity: &str) -> ProvisionStep {
        ProvisionStep::Command {
            identity: identity.to_string(),
            run: "true".to_string(),
        }
    }

    fn provisioning_config(
        definitions: Vec<ProvisionStep>,
        backends: Option<Vec<String>>,
        sequences: &[(&str, &[&str])],
    ) -> Config {
        Config {
            project: crate::config::ProjectConfig {
                backends,
                ..Default::default()
            },
            provision_definitions: definitions,
            backend_sequences: sequences
                .iter()
                .map(|(name, identities)| {
                    (
                        name.to_string(),
                        identities
                            .iter()
                            .map(|identity| identity.to_string())
                            .collect(),
                    )
                })
                .collect(),
            ..Config::default()
        }
    }

    #[test]
    fn provisioning_accepts_a_well_formed_schema() {
        let mut config = provisioning_config(
            vec![command("fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["fetch"])],
        );
        validate_provisioning(&mut config).expect("valid");
        assert!(config.notices.is_empty(), "{:?}", config.notices);
    }

    #[test]
    fn provisioning_caps_the_number_of_steps() {
        let at_limit: Vec<ProvisionStep> = (0..32)
            .map(|index| command(&format!("step-{index}")))
            .collect();
        let mut config = provisioning_config(at_limit, Some(vec!["bubblewrap".to_string()]), &[]);
        validate_provisioning(&mut config).expect("32 steps are accepted");

        let over: Vec<ProvisionStep> = (0..33)
            .map(|index| command(&format!("step-{index}")))
            .collect();
        let mut config = provisioning_config(over, Some(vec!["bubblewrap".to_string()]), &[]);
        let error = validate_provisioning(&mut config).expect_err("too many steps");
        assert!(error.to_string().contains("limit is 32"), "{error}");
    }

    #[test]
    fn provisioning_caps_one_backends_sequence() {
        let definitions: Vec<ProvisionStep> = (0..32)
            .map(|index| command(&format!("step-{index}")))
            .collect();
        let identities: Vec<String> = (0..32).map(|index| format!("step-{index}")).collect();
        let identities: Vec<&str> = identities.iter().map(String::as_str).collect();
        let mut config = provisioning_config(
            definitions,
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", identities.as_slice())],
        );
        validate_provisioning(&mut config).expect("32 sequence entries are accepted");

        let definitions: Vec<ProvisionStep> = (0..33)
            .map(|index| command(&format!("step-{index}")))
            .collect();
        let identities: Vec<String> = (0..33).map(|index| format!("step-{index}")).collect();
        let identities: Vec<&str> = identities.iter().map(String::as_str).collect();
        let mut config = provisioning_config(
            definitions,
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", identities.as_slice())],
        );
        let error = validate_provisioning(&mut config).expect_err("too many sequence steps");
        assert!(error.to_string().contains("limit is 32"), "{error}");
    }

    #[test]
    fn provisioning_rejects_an_invalid_identity() {
        let mut config = provisioning_config(
            vec![command("Fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["Fetch"])],
        );
        let error = validate_provisioning(&mut config).expect_err("invalid identity");
        let ConfigError::Provisioning(issue) = error else {
            panic!("expected a provisioning issue");
        };
        assert!(issue.reason.contains("lowercase letter"), "{issue}");
        assert!(issue.reason.contains("Fetch"), "{issue}");
        assert_eq!(issue.step.as_deref(), Some("Fetch"));

        let mut config = provisioning_config(
            vec![command("bad\nname")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["bad\nname"])],
        );
        let error = validate_provisioning(&mut config).expect_err("control character refused");
        let ConfigError::Provisioning(issue) = error else {
            panic!("expected a provisioning issue");
        };
        assert!(!issue.reason.contains('\n'), "{issue}");
        assert_eq!(issue.step.as_deref(), Some("bad\\nname"));
    }

    #[test]
    fn provisioning_rejects_a_duplicate_identity() {
        let mut config = provisioning_config(
            vec![command("fetch"), command("fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["fetch"])],
        );
        let error = validate_provisioning(&mut config).expect_err("duplicate identity");
        assert!(error.to_string().contains("duplicate"), "{error}");
    }

    #[test]
    fn provisioning_rejects_an_unknown_reference() {
        let mut config = provisioning_config(
            vec![command("fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["other"])],
        );
        let error = validate_provisioning(&mut config).expect_err("unknown reference");
        assert!(error.to_string().contains("does not define"), "{error}");
    }

    #[test]
    fn provisioning_rejects_a_duplicate_in_a_sequence() {
        let mut config = provisioning_config(
            vec![command("fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["fetch", "fetch"])],
        );
        let error = validate_provisioning(&mut config).expect_err("duplicate in sequence");
        assert!(error.to_string().contains("repeats"), "{error}");
    }

    #[test]
    fn provisioning_requires_a_supported_set() {
        let mut config = provisioning_config(vec![command("fetch")], None, &[]);
        let error = validate_provisioning(&mut config).expect_err("missing set");
        assert!(error.to_string().contains("[project] backends"), "{error}");
    }

    #[test]
    fn provisioning_rejects_an_empty_supported_set_with_steps() {
        let mut config = provisioning_config(vec![command("fetch")], Some(Vec::new()), &[]);
        let error = validate_provisioning(&mut config).expect_err("empty set");
        assert!(error.to_string().contains("name at least one"), "{error}");
    }

    #[test]
    fn provisioning_warns_about_a_dead_step() {
        let mut config = provisioning_config(
            vec![command("fetch"), command("unused")],
            Some(vec!["bubblewrap".to_string()]),
            &[("bubblewrap", &["fetch"])],
        );
        validate_provisioning(&mut config).expect("valid");
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("unused")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn provisioning_warns_about_a_sequence_for_an_unsupported_backend() {
        let mut config = provisioning_config(
            vec![command("fetch")],
            Some(vec!["bubblewrap".to_string()]),
            &[("docker", &["fetch"])],
        );
        validate_provisioning(&mut config).expect("valid");
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("docker") && notice.contains("ignored")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn setup_command_is_rejected_with_the_replacement() {
        let error = reject_moved_sections("[project]\nsetup-command = 'echo hi'\n")
            .expect_err("setup-command rejected");
        assert!(error.to_string().contains("[[provision]]"), "{error}");
        assert!(error.to_string().contains("backends"), "{error}");
    }

    #[test]
    fn a_missing_configuration_file_is_none_and_a_directory_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            read_config_file(&dir.path().join("absent.toml"))
                .expect("absent is not an error")
                .is_none()
        );

        let error = read_config_file(dir.path()).expect_err("a directory is refused");
        assert!(
            error.to_string().contains("must be a regular file"),
            "{error}"
        );
    }

    #[test]
    fn an_oversized_configuration_file_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, vec![b'a'; MAX_CONFIG_BYTES as usize + 1]).expect("write");
        let error = read_config_file(&path).expect_err("oversized refused");
        assert!(error.to_string().contains("larger than"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_of_the_project_config_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real-agentkit");
        std::fs::create_dir_all(real.join("litterbox")).expect("real dirs");
        std::fs::write(real.join("litterbox/config.toml"), "[project]\n").expect("write");
        std::os::unix::fs::symlink(&real, dir.path().join(".agentkit")).expect("symlink");
        let path = dir.path().join(".agentkit/litterbox/config.toml");
        let error = read_config_file(&path).expect_err("symlinked ancestor refused");
        assert!(
            error.to_string().contains("symlinked path component"),
            "{error}"
        );
    }

    #[test]
    fn a_local_override_in_a_bare_repository_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        git2::Repository::init_bare(dir.path()).expect("bare repo");
        let path = dir.path().join("config.local.toml");
        std::fs::write(&path, "backend = \"docker\"\n").expect("write");
        let error = validate_local_file(&path).expect_err("bare repository refused");
        assert!(error.to_string().contains("bare Git repository"), "{error}");
    }
}
