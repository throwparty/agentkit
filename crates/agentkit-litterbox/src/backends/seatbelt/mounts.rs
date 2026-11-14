//! Validate contributed mounts and collapse a large contribution.
//!
//! A contribution is a set of host paths a workload may read. The backend
//! validates each one, refuses anything that would widen the boundary, and
//! collapses a step above 64 paths to one ancestor.

use std::path::{Component, Path, PathBuf};

use super::host::{FileKind, Host};
use crate::sandbox::contract::{Error, ResolvedStep};

/// The largest number of paths one step may contribute without collapsing.
pub(crate) const MAX_PATHS: usize = 64;

/// Roots a contribution may not name. `/Applications` is deliberately absent,
/// because a project may contribute an Xcode installation (T-053).
const PROTECTED_ROOTS: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/bin",
    "/sbin",
    "/usr",
    "/Users",
    "/private/etc",
    "/private/var/db",
    "/private/var/select",
    "/dev",
];

/// A step whose contribution was replaced by one ancestor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CollapsedStep {
    pub identity: String,
    pub contributed: usize,
    pub granted: PathBuf,
}

/// The validated reads and the steps that were collapsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Mounts {
    pub reads: Vec<PathBuf>,
    pub collapsed: Vec<CollapsedStep>,
}

/// What validation needs to know about this sandbox.
pub(crate) struct Context<'a> {
    pub workspace: &'a Path,
    pub state_dir: &'a Path,
    pub metadata_dir: &'a Path,
    pub credential_denies: &'a [PathBuf],
}

/// Validate every step's mounts and collapse the ones above the limit.
pub(crate) fn from_steps(
    steps: &[ResolvedStep],
    context: &Context<'_>,
    host: &dyn Host,
) -> Result<Mounts, Error> {
    let workspace = canonical_or_raw(context.workspace, host);
    let state_dir = canonical_or_raw(context.state_dir, host);
    let metadata_dir = canonical_or_raw(context.metadata_dir, host);
    let credential_denies: Vec<PathBuf> = context
        .credential_denies
        .iter()
        .map(|path| canonical_or_raw(path, host))
        .collect();
    let context = Context {
        workspace: &workspace,
        state_dir: &state_dir,
        metadata_dir: &metadata_dir,
        credential_denies: &credential_denies,
    };

    let mut mounts = Mounts::default();
    for step in steps {
        let contributed = &step.contributions.mounts;
        if contributed.is_empty() {
            continue;
        }
        let mut resolved = Vec::with_capacity(contributed.len());
        for path in contributed {
            resolved.push(validate(path, step.identity(), &context, host)?);
        }

        if resolved.len() > MAX_PATHS {
            let ancestor = common_ancestor(&resolved);
            let ancestor =
                validate(&ancestor, step.identity(), &context, host).map_err(|error| {
                    Error::Misconfigured {
                        message: format!(
                            "provisioning step '{}' collapses to '{}', which is refused: {error}",
                            step.identity(),
                            ancestor.display()
                        ),
                    }
                })?;
            mounts.collapsed.push(CollapsedStep {
                identity: step.identity().to_string(),
                contributed: resolved.len(),
                granted: ancestor.clone(),
            });
            push_unique(&mut mounts.reads, ancestor);
        } else {
            for path in resolved {
                push_unique(&mut mounts.reads, path);
            }
        }
    }
    Ok(mounts)
}

/// The longest common ancestor of a set of paths.
pub(crate) fn common_ancestor(paths: &[PathBuf]) -> PathBuf {
    let Some(first) = paths.first() else {
        return PathBuf::from("/");
    };
    let mut components: Vec<Component> = first.components().collect();
    for path in &paths[1..] {
        let other: Vec<Component> = path.components().collect();
        let shared = components
            .iter()
            .zip(&other)
            .take_while(|(left, right)| left == right)
            .count();
        components.truncate(shared);
    }
    components.iter().collect()
}

fn validate(
    path: &Path,
    step: &str,
    context: &Context<'_>,
    host: &dyn Host,
) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err(refuse(step, path, "is not absolute"));
    }
    let resolved = host
        .realpath(path)
        .map_err(|error| refuse(step, path, &format!("cannot be resolved ({error})")))?;

    match host.file_kind(&resolved) {
        Ok(FileKind::Socket) => return Err(refuse(step, path, "is a socket")),
        Ok(FileKind::Fifo) => return Err(refuse(step, path, "is a FIFO")),
        Ok(FileKind::Device) => return Err(refuse(step, path, "is a device")),
        Ok(FileKind::File | FileKind::Directory) => {}
        Err(error) => {
            return Err(refuse(
                step,
                path,
                &format!("cannot be inspected ({error})"),
            ));
        }
    }

    for root in PROTECTED_ROOTS {
        let root = Path::new(root);
        if resolved == root || (root != Path::new("/") && resolved.starts_with(root)) {
            return Err(refuse(step, path, "is a protected root"));
        }
    }

    if resolved == context.workspace || context.workspace.starts_with(&resolved) {
        return Err(refuse(step, path, "is the workspace or an ancestor of it"));
    }
    if overlaps(&resolved, context.state_dir) {
        return Err(refuse(step, path, "overlaps the backend state directory"));
    }
    if overlaps(&resolved, context.metadata_dir) {
        return Err(refuse(step, path, "overlaps the metadata directory"));
    }
    for deny in context.credential_denies {
        if resolved == *deny || resolved.starts_with(deny) {
            return Err(refuse(step, path, "is a credential path"));
        }
    }

    Ok(resolved)
}

fn canonical_or_raw(path: &Path, host: &dyn Host) -> PathBuf {
    host.realpath(path).unwrap_or_else(|_| path.to_path_buf())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn refuse(step: &str, path: &Path, reason: &str) -> Error {
    Error::Misconfigured {
        message: format!(
            "provisioning step '{step}' mount '{}' {reason}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{Context, MAX_PATHS, common_ancestor, from_steps};
    use crate::backends::seatbelt::host::SystemHost;
    use crate::sandbox::contract::{Contributions, Placement, ResolvedStep};

    fn fixture() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn step(identity: &str, mounts: &[PathBuf]) -> ResolvedStep {
        ResolvedStep {
            identity: identity.to_string(),
            kind: "command".to_string(),
            placement: Placement::Host,
            sandbox: None,
            contributions: Contributions {
                mounts: mounts.to_vec(),
                ..Contributions::default()
            },
        }
    }

    fn context<'a>(
        workspace: &'a Path,
        state: &'a Path,
        metadata: &'a Path,
        denies: &'a [PathBuf],
    ) -> Context<'a> {
        Context {
            workspace,
            state_dir: state,
            metadata_dir: metadata,
            credential_denies: denies,
        }
    }

    #[test]
    fn a_normal_contribution_is_resolved_and_deduplicated() {
        let root = fixture();
        let workspace = root.path().join("ws");
        let state = root.path().join("state");
        let metadata = root.path().join("meta");
        let tool = root.path().join("tool");
        for path in [&workspace, &state, &metadata, &tool] {
            std::fs::create_dir_all(path).expect("dir");
        }
        let steps = vec![
            step("first", std::slice::from_ref(&tool)),
            step("second", std::slice::from_ref(&tool)),
        ];
        let mounts = from_steps(
            &steps,
            &context(&workspace, &state, &metadata, &[]),
            &SystemHost::new(),
        )
        .expect("mounts");
        assert_eq!(mounts.reads, vec![tool.canonicalize().expect("canonical")]);
        assert!(mounts.collapsed.is_empty());
    }

    #[test]
    fn a_step_at_the_limit_stays_and_one_above_collapses() {
        let root = fixture();
        let workspace = root.path().join("ws");
        let state = root.path().join("state");
        let metadata = root.path().join("meta");
        let closure = root.path().join("closure");
        for path in [&workspace, &state, &metadata, &closure] {
            std::fs::create_dir_all(path).expect("dir");
        }
        for index in 0..=MAX_PATHS {
            std::fs::create_dir_all(closure.join(format!("p{index}"))).expect("package");
        }
        let at_limit: Vec<PathBuf> = (0..MAX_PATHS)
            .map(|index| closure.join(format!("p{index}")))
            .collect();
        let mounts = from_steps(
            &[step("limit", &at_limit)],
            &context(&workspace, &state, &metadata, &[]),
            &SystemHost::new(),
        )
        .expect("mounts");
        assert_eq!(mounts.reads.len(), MAX_PATHS);
        assert!(mounts.collapsed.is_empty());

        let above: Vec<PathBuf> = (0..=MAX_PATHS)
            .map(|index| closure.join(format!("p{index}")))
            .collect();
        let mounts = from_steps(
            &[step("above", &above)],
            &context(&workspace, &state, &metadata, &[]),
            &SystemHost::new(),
        )
        .expect("mounts");
        assert_eq!(mounts.reads.len(), 1);
        assert_eq!(mounts.reads[0], closure.canonicalize().expect("canonical"));
        assert_eq!(mounts.collapsed[0].contributed, MAX_PATHS + 1);
        assert_eq!(mounts.collapsed[0].identity, "above");
    }

    #[test]
    fn protected_paths_and_sockets_are_refused() {
        let root = fixture();
        let workspace = root.path().join("ws");
        let state = root.path().join("state");
        let metadata = root.path().join("meta");
        for path in [&workspace, &state, &metadata] {
            std::fs::create_dir_all(path).expect("dir");
        }
        let ctx = context(&workspace, &state, &metadata, &[]);
        let host = SystemHost::new();

        for protected in ["/System/Library", "/usr/bin", "/Users"] {
            let error = from_steps(
                &[step("protected", &[PathBuf::from(protected)])],
                &ctx,
                &host,
            )
            .expect_err("protected");
            assert!(error.to_string().contains("protected root"), "{error}");
        }

        let socket = root.path().join("sock");
        let _listener = UnixListener::bind(&socket).expect("socket");
        let error = from_steps(&[step("socket", &[socket])], &ctx, &host).expect_err("socket");
        assert!(error.to_string().contains("socket"), "{error}");
    }

    #[test]
    fn a_credential_workspace_ancestor_or_state_overlap_is_refused() {
        let root = fixture();
        let workspace = root.path().join("ws");
        let state = root.path().join("state");
        let metadata = root.path().join("meta");
        let home = root.path().join("home");
        let ssh = home.join(".ssh");
        for path in [&workspace, &state, &metadata, &ssh] {
            std::fs::create_dir_all(path).expect("dir");
        }
        std::fs::write(ssh.join("id_ed25519"), b"secret").expect("credential file");
        let denies = vec![ssh.canonicalize().expect("canonical")];
        let ctx = context(&workspace, &state, &metadata, &denies);
        let host = SystemHost::new();

        let error = from_steps(&[step("cred", &[ssh.join("id_ed25519")])], &ctx, &host)
            .expect_err("credential");
        assert!(error.to_string().contains("credential"), "{error}");

        let error = from_steps(
            &[step("ancestor", &[root.path().to_path_buf()])],
            &ctx,
            &host,
        )
        .expect_err("workspace ancestor");
        assert!(error.to_string().contains("workspace"), "{error}");

        let error = from_steps(&[step("state", std::slice::from_ref(&state))], &ctx, &host)
            .expect_err("state");
        assert!(error.to_string().contains("state directory"), "{error}");

        let error = from_steps(
            &[step("metadata", std::slice::from_ref(&metadata))],
            &ctx,
            &host,
        )
        .expect_err("metadata");
        assert!(error.to_string().contains("metadata"), "{error}");
    }

    #[test]
    fn a_symlink_is_resolved_before_the_rules_apply() {
        let root = fixture();
        let workspace = root.path().join("ws");
        let state = root.path().join("state");
        let metadata = root.path().join("meta");
        for path in [&workspace, &state, &metadata] {
            std::fs::create_dir_all(path).expect("dir");
        }
        let link = root.path().join("users");
        std::os::unix::fs::symlink("/Users", &link).expect("symlink");
        let error = from_steps(
            &[step("link", &[link])],
            &context(&workspace, &state, &metadata, &[]),
            &SystemHost::new(),
        )
        .expect_err("symlink target is protected");
        assert!(error.to_string().contains("protected root"), "{error}");
    }

    #[test]
    fn the_common_ancestor_is_by_component() {
        assert_eq!(
            common_ancestor(&[PathBuf::from("/a/b/one"), PathBuf::from("/a/b/two"),]),
            PathBuf::from("/a/b")
        );
        assert_eq!(
            common_ancestor(&[PathBuf::from("/a/b"), PathBuf::from("/c/d")]),
            PathBuf::from("/")
        );
        assert_eq!(common_ancestor(&[]), PathBuf::from("/"));
    }
}
