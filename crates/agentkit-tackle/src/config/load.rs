//! Layered configuration loading: merge precedence, trust gating, project-field rejection, and script-source resolution.

use super::schema::{
    Config, ConfigError, Loaded, ScriptConfig,
};
use super::trust;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub fn load_layered(
    user_dir: &Path,
    project_dir: Option<&Path>,
    gate: Option<&mut trust::Gate<'_>>,
) -> Result<Loaded, ConfigError> {
    let user_path = user_dir.join("config.toml");
    let user = load_file(&user_path)?;

    let project_path = project_dir.map(|d| d.join("config.toml"));
    let project_layer = project_path.filter(|p| p.is_file());
    let project = match &project_layer {
        Some(path) => {
            let project = load_file(path)?.unwrap_or_default();
            reject_project_security_fields(&project, path)?;
            if let Some(gate) = gate {
                let project_dir = path.parent().unwrap_or_else(|| Path::new("."));
                gate_project_entries(gate, project, project_dir)
            } else {
                project
            }
        }
        None => Config::default(),
    };

    Ok(Loaded {
        // The built-in registrations seed at the lowest precedence: a
        // user or project entry with the same name replaces one
        // wholesale (a disabling entry needs no file).
        config: builtin_registrations()
            .merge(user.unwrap_or_default())
            .merge(project),
        user_layer: user_path.is_file().then_some(user_path),
        project_layer,
    })
}

/// Loads the built-in registrations and the user layer, returning the raw
/// project layer separately. The project layer is *not* merged: its
/// executable entries are trust-gated, and consent is resolved at the
/// first session (there is no ACP connection at startup), so the caller
/// holds it until then.
pub fn load_layered_deferred(
    user_dir: &Path,
    project_dir: Option<&Path>,
) -> Result<(Loaded, Option<Config>), ConfigError> {
    let user_path = user_dir.join("config.toml");
    let user = load_file(&user_path)?;

    let project_path = project_dir.map(|d| d.join("config.toml"));
    let project_layer = project_path.filter(|p| p.is_file());
    let project = match &project_layer {
        Some(path) => {
            let project = load_file(path)?.unwrap_or_default();
            // Endpoint/credential rejection is not trust-gated: it holds
            // regardless of consent, so it fails at load.
            reject_project_security_fields(&project, path)?;
            Some(project)
        }
        None => None,
    };

    Ok((
        Loaded {
            config: builtin_registrations().merge(user.unwrap_or_default()),
            user_layer: user_path.is_file().then_some(user_path),
            project_layer,
        },
        project,
    ))
}

/// The shipped script registrations: compaction (automatic past the
/// utilisation threshold and the manual /compact summary) and titling.
/// Sources are built-in (`builtin:` references); user configuration
/// overrides any entry wholesale.
fn builtin_registrations() -> Config {
    let mut config = Config::default();
    config.scripts.insert(
        "compaction".to_owned(),
        ScriptConfig {
            events: vec!["post_turn".to_owned(), "compaction_requested".to_owned()],
            file: "builtin:compaction.rhai".to_owned(),
            enabled: true,
        },
    );
    config.scripts.insert(
        "titling".to_owned(),
        ScriptConfig {
            events: vec!["title_trigger".to_owned()],
            file: "builtin:titling.rhai".to_owned(),
            enabled: true,
        },
    );
    config
}

/// The trust entries for a project config's executable sections
/// (`mcp_servers` and `scripts`): relative path and SHA-256.
///
/// For scripts the hash covers both the serialised registration *and* the
/// script body it names. The body is executable and part of the trusted
/// unit, so a changed body — not just a changed registration — must
/// re-prompt; hashing the registration alone would let a trusted
/// repository swap a script's contents silently.
pub fn project_trust_entries(project: &Config, project_dir: &Path) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for (name, server) in &project.mcp_servers {
        let serialised = toml::to_string(server).unwrap_or_default();
        entries.insert(format!("mcp_servers/{name}"), trust::hash(&serialised));
    }
    for (name, script) in &project.scripts {
        let serialised = toml::to_string(script).unwrap_or_default();
        let body = project_script_source(script, project_dir).unwrap_or_default();
        entries.insert(
            format!("scripts/{name}"),
            trust::hash(&format!("{serialised}\u{0}{body}")),
        );
    }
    entries
}

/// Resolves a project script's body at trust time: a `builtin:` reference
/// or a file under the project's `scripts/` directory. A missing or
/// unreadable file hashes as empty, so its later appearance re-prompts.
fn project_script_source(script: &ScriptConfig, project_dir: &Path) -> Option<String> {
    if let Some(source) = crate::builtins::script_source(&script.file) {
        return Some(source.to_owned());
    }
    std::fs::read_to_string(project_dir.join("scripts").join(&script.file)).ok()
}

/// Drops the executable project config entries whose trust entries were
/// refused, and pins each retained project script to its project-layer
/// file. The pin records the script's origin explicitly: resolution reads
/// the approved file directly, so a user-layer file of the same name
/// cannot shadow it, and a trusted project script is no longer dropped.
pub fn retain_project_entries(
    mut project: Config,
    project_dir: &Path,
    approved: &BTreeSet<String>,
) -> Config {
    project
        .mcp_servers
        .retain(|name, _| approved.contains(&format!("mcp_servers/{name}")));
    project
        .scripts
        .retain(|name, _| approved.contains(&format!("scripts/{name}")));
    for script in project.scripts.values_mut() {
        // A `builtin:` reference is not a project file: leave it to the
        // built-in resolver.
        if !script.file.starts_with("builtin:") {
            script.file = project_dir
                .join("scripts")
                .join(&script.file)
                .to_string_lossy()
                .into_owned();
        }
    }
    project
}

/// Resolves a registered script's source.
///
/// The reference decides the layer: a `builtin:<name>.rhai` reference is
/// the embedded source, an absolute path is a project-layer script pinned
/// at trust resolution, and anything else is a filename under the user
/// layer's `scripts/` directory. An unreadable file is logged and skipped.
pub fn resolve_script_source(name: &str, file: &str, user_dir: Option<&Path>) -> Option<String> {
    if let Some(source) = crate::builtins::script_source(file) {
        return Some(source.to_owned());
    }
    let resolved = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        user_dir?.join("scripts").join(file)
    };
    match std::fs::read_to_string(&resolved) {
        Ok(source) => Some(source),
        Err(err) => {
            tracing::warn!(script = %name, path = %resolved.display(), "script unreadable: {err}");
            None
        }
    }
}

/// Resolves every enabled registration's script body once — built-in,
/// user-layer file, or a pinned project file — keyed by registration
/// name. Called when the configuration is set: process start for user and
/// built-in scripts, trust resolution for the project layer. The result is
/// cached and never re-read, so a changed body cannot take effect without
/// a restart and a re-prompt (loaded once, no hot reload).
pub fn resolve_script_sources(config: &Loaded) -> BTreeMap<String, String> {
    let user_dir = config.user_layer.as_deref().and_then(Path::parent);
    config
        .config
        .scripts
        .iter()
        .filter(|(_, script)| script.enabled)
        .filter_map(|(name, script)| {
            resolve_script_source(name, &script.file, user_dir)
                .map(|source| (name.clone(), source))
        })
        .collect()
}

/// Trust-gates the executable entries of a project layer (`mcp_servers`
/// and `scripts`); unapproved entries are dropped before the merge. The
/// hash covers the serialised entry, so any field change re-prompts.
fn gate_project_entries(gate: &mut trust::Gate<'_>, project: Config, project_dir: &Path) -> Config {
    let entries = project_trust_entries(&project, project_dir);
    if entries.is_empty() {
        return project;
    }
    let approved = gate.gate(&entries);
    retain_project_entries(project, project_dir, &approved)
}

fn load_file(path: &Path) -> Result<Option<Config>, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.into(),
                source,
            });
        }
    };
    toml::from_str(&content)
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })
}

fn reject_project_security_fields(project: &Config, path: &Path) -> Result<(), ConfigError> {
    if !project.endpoints.is_empty() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "endpoints",
        });
    }
    if project.credential_helper.is_some() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "credential_helper",
        });
    }
    if project.client_tools.is_some() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "client_tools",
        });
    }
    Ok(())
}

// --- Schema (the types below are the source of truth for the TOML shape) ---

