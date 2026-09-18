//! TOFU trust records for project-provided configuration.
//!
//! The project layer is executable in effect: its scripts run in the
//! harness, its personas and prompts enter model context, and its
//! `mcp_servers` entries spawn processes. Trust is granted per project —
//! keyed by the repository remote (falling back to the working-directory
//! path outside git) — per relative path, pinned by SHA-256. Any change
//! re-prompts. Records live in `trust.toml` in the user configuration
//! directory: human-editable (revocation is deleting an entry), readable
//! before any database exists.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The consent interface, satisfied by the ACP permission flow in T-022.
/// The stub implementation denies: fail-closed until real consent exists.
pub trait ConsentPrompt {
    /// Asks the user to trust `entries` (relative paths) for the project
    /// identified by `identity`. Returns true only on explicit consent.
    fn consent(&self, identity: &str, entries: &BTreeSet<String>) -> bool;
}

/// Fail-closed stub: denies all consent until T-022 wires the ACP
/// permission flow.
pub struct DenyPrompt;

impl ConsentPrompt for DenyPrompt {
    fn consent(&self, _identity: &str, _entries: &BTreeSet<String>) -> bool {
        tracing::warn!("project trust consent required but no prompt is wired; denying");
        false
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct TrustRecord {
    /// remote identity -> relative path -> sha256 hex
    #[serde(flatten)]
    remotes: BTreeMap<String, RemoteTrust>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct RemoteTrust {
    #[serde(flatten)]
    paths: BTreeMap<String, String>,
}

pub struct TrustStore {
    path: PathBuf,
    record: TrustRecord,
}

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

impl TrustStore {
    /// Loads the trust record from `path` (trust.toml); a missing file is
    /// an empty record.
    pub fn load(path: PathBuf) -> Result<Self, TrustError> {
        let record = match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).map_err(|err| TrustError::Parse {
                path: path.clone(),
                message: err.to_string(),
            })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => TrustRecord::default(),
            Err(source) => return Err(TrustError::Io { path, source }),
        };
        Ok(Self { path, record })
    }

    fn state(&self, identity: &str, rel_path: &str, hash: &str) -> TrustState {
        let pinned = self
            .record
            .remotes
            .get(identity)
            .and_then(|remote| remote.paths.get(rel_path));
        match pinned {
            None => TrustState::Unknown,
            Some(pinned) if pinned == hash => TrustState::Trusted,
            Some(_) => TrustState::Changed,
        }
    }

    fn record_all(&mut self, identity: &str, entries: &BTreeMap<String, String>) {
        let remote = self.record.remotes.entry(identity.to_owned()).or_default();
        for (rel_path, hash) in entries {
            remote.paths.insert(rel_path.clone(), hash.clone());
        }
    }

    /// Persists the record.
    pub fn save(&self) -> Result<(), TrustError> {
        let content = toml::to_string_pretty(&self.record).map_err(|err| TrustError::Parse {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| TrustError::Io {
                path: self.path.clone(),
                source,
            })?;
        }
        std::fs::write(&self.path, content).map_err(|source| TrustError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Gates `entries` (relative paths and their current hashes) for
    /// `identity`: already-trusted, unchanged entries pass; unknown or
    /// changed entries require consent — granted in one batch, refused
    /// entries are dropped by the caller.
    pub fn gate(
        &mut self,
        identity: &str,
        entries: &BTreeMap<String, String>,
        prompt: &dyn ConsentPrompt,
    ) -> BTreeSet<String> {
        let needs_consent: Vec<(&String, TrustState)> = entries
            .iter()
            .map(|(rel_path, hash)| (rel_path, self.state(identity, rel_path, hash)))
            .filter(|(_, state)| matches!(state, TrustState::Unknown | TrustState::Changed))
            .collect();

        let untrusted: BTreeSet<String> = needs_consent
            .iter()
            .map(|(path, _)| (*path).clone())
            .collect();
        if !untrusted.is_empty() && prompt.consent(identity, &untrusted) {
            self.record_all(identity, entries);
            if let Err(err) = self.save() {
                tracing::error!("{err}");
            }
        }

        entries
            .iter()
            .filter(|(rel_path, hash)| {
                matches!(self.state(identity, rel_path, hash), TrustState::Trusted)
            })
            .map(|(rel_path, _)| rel_path.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustState {
    Trusted,
    Changed,
    Unknown,
}

/// Bundles the store, project identity, and consent prompt into one
/// gate handle for the load pipeline.
pub struct Gate<'a> {
    pub store: &'a mut TrustStore,
    pub identity: &'a str,
    pub prompt: &'a dyn ConsentPrompt,
}

impl Gate<'_> {
    /// Gates a batch of (relative path, current hash) entries; returns the
    /// approved subset.
    pub fn gate(&mut self, entries: &BTreeMap<String, String>) -> BTreeSet<String> {
        self.store.gate(self.identity, entries, self.prompt)
    }
}

/// SHA-256 hex digest of the raw content.
pub fn hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Identifies the project for trust keying: the git remote URL when the
/// working directory is inside a repository, else the working directory
/// path itself.
pub fn project_identity(cwd: &Path) -> String {
    match git2::Repository::discover(cwd) {
        Ok(repo) => repo
            .find_remote("origin")
            .ok()
            .and_then(|remote| remote.url().ok().map(str::to_owned))
            .map(|url| normalize_remote(&url))
            .unwrap_or_else(|| cwd.to_string_lossy().to_string()),
        Err(_) => cwd.to_string_lossy().to_string(),
    }
}

/// Normalises a remote URL: `scp-like` syntax to URL form, trailing `/`
/// and `.git` stripped.
fn normalize_remote(url: &str) -> String {
    let url = if !url.contains("://") && url.contains(':') && !url.starts_with('/') {
        // git@github.com:owner/repo.git -> ssh://git@github.com/owner/repo.git
        let (host, path) = url.split_once(':').unwrap_or((url, ""));
        format!("ssh://{host}/{path}")
    } else {
        url.to_owned()
    };
    let trimmed = url.trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct StubPrompt {
        granted: AtomicBool,
        calls: AtomicBool,
    }

    impl StubPrompt {
        fn granted() -> Self {
            Self {
                granted: AtomicBool::new(true),
                calls: AtomicBool::new(false),
            }
        }
        fn denied() -> Self {
            Self {
                granted: AtomicBool::new(false),
                calls: AtomicBool::new(false),
            }
        }
    }

    impl ConsentPrompt for StubPrompt {
        fn consent(&self, _identity: &str, _entries: &BTreeSet<String>) -> bool {
            self.calls.store(true, Ordering::SeqCst);
            self.granted.load(Ordering::SeqCst)
        }
    }

    fn entries_one() -> BTreeMap<String, String> {
        BTreeMap::from([("scripts/deny.rhai".into(), hash("content"))])
    }

    #[test]
    fn unknown_entries_require_consent() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::load(dir.path().join("trust.toml")).unwrap();
        let prompt = StubPrompt::denied();
        let approved = store.gate("remote", &entries_one(), &prompt);
        assert!(prompt.calls.load(Ordering::SeqCst));
        assert!(approved.is_empty());
    }

    #[test]
    fn granted_entries_are_persisted_and_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.toml");
        let mut store = TrustStore::load(path.clone()).unwrap();
        let approved = store.gate("remote", &entries_one(), &StubPrompt::granted());
        assert_eq!(approved.len(), 1);

        // A fresh store over the same file trusts without prompting.
        let mut reloaded = TrustStore::load(path).unwrap();
        let denied = StubPrompt::denied();
        let approved = reloaded.gate("remote", &entries_one(), &denied);
        assert!(!denied.calls.load(Ordering::SeqCst));
        assert_eq!(approved.len(), 1);
    }

    #[test]
    fn changed_hash_re_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::load(dir.path().join("trust.toml")).unwrap();
        store.gate("remote", &entries_one(), &StubPrompt::granted());

        let changed = BTreeMap::from([("scripts/deny.rhai".into(), hash("new content"))]);
        let denied = StubPrompt::denied();
        let approved = store.gate("remote", &changed, &denied);
        assert!(denied.calls.load(Ordering::SeqCst));
        assert!(approved.is_empty());
    }

    #[test]
    fn trusted_and_unknown_entries_are_gated_separately() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::load(dir.path().join("trust.toml")).unwrap();
        store.gate("remote", &entries_one(), &StubPrompt::granted());

        let mut entries = entries_one();
        entries.insert("actors/new.md".into(), hash("new"));
        let prompt = StubPrompt::granted();
        let approved = store.gate("remote", &entries, &prompt);
        assert_eq!(approved.len(), 2);
    }

    #[test]
    fn identity_falls_back_to_path_outside_git() {
        let dir = tempfile::tempdir().unwrap();
        let identity = project_identity(dir.path());
        assert_eq!(identity, dir.path().to_string_lossy());
    }

    #[test]
    fn remotes_are_normalised() {
        assert_eq!(
            normalize_remote("git@github.com:Owner/Repo.git"),
            "ssh://git@github.com/Owner/Repo"
        );
        assert_eq!(
            normalize_remote("https://github.com/Owner/Repo.git"),
            "https://github.com/Owner/Repo"
        );
    }
}
