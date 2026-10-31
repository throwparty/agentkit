use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Read-only paths required for the dynamic linker and standard tooling.
///
/// `/nix/store` is here because the host toolchain is Nix: every binary may
/// dlopen a sibling store path, and a baseline of only system directories
/// breaks coreutils with a dyld abort. The baseline is a function of the host,
/// not a constant, and deliberately excludes `/System/Volumes/Data`, which is a
/// firmlink onto the data volume that also carries `/Users`.
const BASELINE_READ: &[&str] = &[
    "/usr/bin",
    "/usr/lib",
    "/usr/share",
    "/usr/libexec",
    "/System/Library",
    "/System/Applications",
    "/Library/Developer",
    "/Applications/Xcode.app/Contents/Developer",
    "/Library/Frameworks",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/select",
    "/dev",
    "/nix/store",
];

/// Mach services the workload is known to need. Started from deny: each name
/// here was observed to be required by a failing run, not guessed. Services
/// holding credentials (securityd, trustd, ocspd) are deliberately absent, so a
/// framework that needs them fails closed rather than becoming a confused
/// deputy for the path policy.
const MACH_SERVICES: &[&str] = &[
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.distributed_notifications@Uv3",
    "com.apple.logd",
    "com.apple.lsd.mapdb",
    "com.apple.fonts",
    "com.apple.FontObjectsServer",
    "com.apple.PowerManagement.control",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.bsd.dirhelper",
    "com.apple.SystemConfiguration.configd",
];

/// Reads the caller cannot grant, whatever it asks for. Denied for read,
/// write, link and unlink: a read-only deny is defeated by moving the file
/// into a writable root.
const CREDENTIAL_DENY: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".docker",
    ".kube",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".cargo/credentials.toml",
    ".config/gh",
    ".config/gcloud",
    ".config/containers",
    ".config/pip",
    ".pypirc",
    ".terraform.d",
    ".vault-token",
    ".password-store",
    ".local/share/keyrings",
    "Library/Keychains",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    "Library/Safari",
    "Library/Cookies",
];

#[derive(Debug)]
pub enum PathError {
    Relative(PathBuf),
    ParentTraversal(PathBuf),
    Escapes(PathBuf),
    Unresolvable { path: PathBuf, source: io::Error },
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relative(path) => write!(formatter, "{} is relative", path.display()),
            Self::ParentTraversal(path) => {
                write!(formatter, "{} contains a parent component", path.display())
            }
            Self::Escapes(path) => {
                write!(formatter, "{} resolves outside the workspace", path.display())
            }
            Self::Unresolvable { path, source } => {
                write!(formatter, "{} does not resolve: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for PathError {}

/// A network grant. Parsed and re-emitted from validated components so nothing
/// caller-supplied reaches the profile as raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    host: String,
    port: u16,
}

impl Connect {
    pub fn parse(value: &str) -> Result<Self, String> {
        let (host, port) = value
            .rsplit_once(':')
            .ok_or_else(|| format!("{value} is not host:port"))?;

        if host.is_empty() {
            return Err(format!("{value} has no host"));
        }
        // SBPL accepts exactly two host tokens: `*` and `localhost`. An IP
        // literal is rejected by the profile compiler with "host must be * or
        // localhost", so accepting one here would only move the failure later.
        // `*` compiles but grants every host on that port, which is the
        // opposite of a per-destination grant. Only loopback is exposed; the
        // wildcard is recorded in findings rather than offered.
        if host != "localhost" {
            return Err(format!(
                "{value}: host must be localhost; the profile language accepts only localhost or *, and * grants every host"
            ));
        }

        let port: u16 = port
            .parse()
            .map_err(|_| format!("{value}: port is not a number"))?;
        if port == 0 {
            return Err(format!("{value}: port must not be 0"));
        }

        Ok(Self {
            host: host.to_string(),
            port,
        })
    }

    fn render(&self) -> String {
        form(
            "allow network-outbound",
            &[format!("(remote ip \"{}:{}\")", self.host, self.port)],
        )
    }
}

/// A confinement request. Paths are canonicalised on the way in, because the
/// kernel matches rules against resolved paths and a rule written on a symlink
/// is inert.
#[derive(Debug, Clone)]
pub struct Policy {
    workspace: PathBuf,
    home: Option<PathBuf>,
    tag: String,
    reads: Vec<PathBuf>,
    writes: Vec<PathBuf>,
    connects: Vec<Connect>,
}

impl Policy {
    pub fn new(workspace: &Path) -> Result<Self, PathError> {
        let home = match std::env::var_os("HOME") {
            Some(home) => Some(PathBuf::from(home)),
            None => return Err(PathError::Relative(PathBuf::from("$HOME is unset"))),
        };
        Self::with_home(workspace, home.as_deref())
    }

    /// The home directory is explicit so golden profiles do not depend on the
    /// machine they are rendered on.
    pub fn with_home(workspace: &Path, home: Option<&Path>) -> Result<Self, PathError> {
        Ok(Self {
            workspace: grant_path(workspace, None)?,
            home: home.map(|home| grant_path(home, None)).transpose()?,
            tag: "litterbox".to_string(),
            reads: Vec::new(),
            writes: Vec::new(),
            connects: Vec::new(),
        })
    }

    pub fn tag(mut self, tag: &str) -> Result<Self, String> {
        if tag.is_empty() || tag.len() > 64
            || !tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("{tag} is not a valid tag"));
        }
        self.tag = tag.to_string();
        Ok(self)
    }

    pub fn read(mut self, path: &Path) -> Result<Self, PathError> {
        self.reads.push(grant_path(path, Some(&self.workspace))?);
        Ok(self)
    }

    pub fn write(mut self, path: &Path) -> Result<Self, PathError> {
        self.writes.push(grant_path(path, Some(&self.workspace))?);
        Ok(self)
    }

    pub fn connect(mut self, value: &str) -> Result<Self, String> {
        self.connects.push(Connect::parse(value)?);
        Ok(self)
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn render(&self) -> String {
        let mut blocks: Vec<String> = vec![
            "(version 1)".to_string(),
            form("deny default", &[format!("(with message \"{}\")", self.tag)]),
            baseline_block(),
            self.metadata_rule(),
        ];
        blocks.extend(self.grant_rules());
        blocks.extend(self.connects.iter().map(Connect::render));
        blocks.extend(self.invariant_rules());
        blocks.join("\n\n") + "\n"
    }

    /// Metadata is scoped to the read set: a global allow makes every denied
    /// path stat-able, which is both an oracle and what lets `link()` resolve a
    /// denied source. Scoping it is only safe because exec still works — that
    /// was retested after the change.
    fn metadata_rule(&self) -> String {
        let mut filters = vec!["(literal \"/\")".to_string()];
        filters.extend(BASELINE_READ.iter().map(|path| format!("(subpath \"{path}\")")));
        for path in std::iter::once(&self.workspace)
            .chain(self.reads.iter())
            .chain(self.writes.iter())
        {
            filters.push(format!("(subpath {})", quote(path)));
        }

        // Ancestors must be stat-able too: resolving `/a/b/c` stats `/a` and
        // `/a/b` first, and with metadata narrowed to the subtree the lookup
        // fails with ENOTDIR rather than EACCES.
        let mut ancestors: Vec<PathBuf> = Vec::new();
        for path in std::iter::once(&self.workspace)
            .chain(self.reads.iter())
            .chain(self.writes.iter())
        {
            let mut current = path.parent();
            while let Some(candidate) = current {
                if candidate == Path::new("/") {
                    break;
                }
                ancestors.push(candidate.to_path_buf());
                current = candidate.parent();
            }
        }
        ancestors.sort();
        ancestors.dedup();
        filters.extend(ancestors.iter().map(|path| format!("(literal {})", quote(path))));

        form("allow file-read-metadata", &filters)
    }

    fn grant_rules(&self) -> Vec<String> {
        let mut reads = vec!["(literal \"/\")".to_string()];
        reads.extend(self.reads.iter().map(|path| format!("(subpath {})", quote(path))));
        reads.push(format!("(subpath {})", quote(&self.workspace)));
        reads.extend(self.writes.iter().map(|path| format!("(subpath {})", quote(path))));
        let mut writes = vec!["(literal \"/dev/null\")".to_string()];
        writes.push(format!("(subpath {})", quote(&self.workspace)));
        writes.extend(self.writes.iter().map(|path| format!("(subpath {})", quote(path))));

        vec![form("allow file-read*", &reads), form("allow file-write*", &writes)]
    }

    /// Rules the caller cannot override. Emitted last: later rules win.
    fn invariant_rules(&self) -> Vec<String> {
        let Some(home) = &self.home else {
            return Vec::new();
        };

        let paths: Vec<String> = CREDENTIAL_DENY
            .iter()
            .map(|relative| home.join(relative))
            .map(|path| path.canonicalize().unwrap_or(path))
            .map(|path| format!("(subpath {})", quote(&path)))
            .collect();

        vec![
            form("deny file-read*", &paths),
            form("deny file-write*", &paths),
            form("deny file-write-unlink", &paths),
        ]
    }
}

fn baseline_block() -> String {
    let reads: Vec<String> = std::iter::once("(literal \"/\")".to_string())
        .chain(BASELINE_READ.iter().map(|path| format!("(subpath \"{path}\")")))
        .collect();
    let services: Vec<String> = MACH_SERVICES
        .iter()
        .map(|name| format!("(global-name \"{name}\")"))
        .collect();

    [
        "(allow process-exec)".to_string(),
        "(allow process-fork)".to_string(),
        form("allow process-info*", &["(target same-sandbox)".to_string()]),
        form("allow signal", &["(target same-sandbox)".to_string()]),
        form("allow file-read*", &reads),
        // The Rust runtime reads sysctls to size thread stacks; without this it
        // aborts at startup with "failed to allocate a guard page".
        "(allow sysctl-read)".to_string(),
        form("allow mach-lookup", &services),
    ]
    .join("\n")
}

/// One SBPL form: operator on its own line, one filter per line beneath it.
fn form(operator: &str, filters: &[String]) -> String {
    if filters.is_empty() {
        return format!("({operator})");
    }
    let body: String = filters
        .iter()
        .map(|filter| format!("\n  {filter}"))
        .collect();
    format!("({operator}{body})")
}

/// Canonicalise a grant, rejecting anything that could widen the policy
/// silently: relative paths, `..` components, and paths that start inside the
/// workspace but resolve outside it.
fn grant_path(path: &Path, workspace: Option<&Path>) -> Result<PathBuf, PathError> {
    if path.is_relative() {
        return Err(PathError::Relative(path.to_path_buf()));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PathError::ParentTraversal(path.to_path_buf()));
    }

    let resolved = path.canonicalize().map_err(|source| PathError::Unresolvable {
        path: path.to_path_buf(),
        source,
    })?;

    if let Some(workspace) = workspace {
        // A path that lives under the workspace must resolve under it. Walk the
        // ancestors by canonical form: comparing the raw spelling fails when
        // the caller's path uses /var and the workspace is /private/var, and
        // checking only the immediate parent misses `ws/link/file` where `link`
        // resolves elsewhere.
        if ancestor_reaches(path, workspace) && !resolved.starts_with(workspace) {
            return Err(PathError::Escapes(path.to_path_buf()));
        }
    }

    Ok(resolved)
}

/// True when canonicalising any ancestor of `path` lands on `workspace`.
fn ancestor_reaches(path: &Path, workspace: &Path) -> bool {
    let mut current = path.parent();
    while let Some(candidate) = current {
        if candidate.canonicalize().ok().as_deref() == Some(workspace) {
            return true;
        }
        current = candidate.parent();
    }
    false
}

fn quote(path: &Path) -> String {
    let mut escaped = String::with_capacity(path.as_os_str().len() + 2);
    escaped.push('"');
    for character in path.to_string_lossy().chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                escaped.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("policy-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create");
        path
    }

    #[test]
    fn relative_workspace_is_rejected() {
        assert!(matches!(
            Policy::new(Path::new("relative/dir")),
            Err(PathError::Relative(_))
        ));
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let root = temp("traversal");
        let path = root.join("..").join("elsewhere");
        assert!(matches!(
            Policy::with_home(&root, None).and_then(|policy| policy.read(&path)),
            Err(PathError::ParentTraversal(_))
        ));
    }

    #[test]
    fn symlink_out_of_workspace_is_rejected() {
        let root = temp("escape");
        let workspace = root.join("ws");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).expect("ws");
        std::fs::create_dir_all(&outside).expect("outside");
        let link = workspace.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        let policy = Policy::with_home(&workspace, None).expect("policy");
        assert!(matches!(policy.read(&link), Err(PathError::Escapes(_))));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn symlinked_directory_below_the_workspace_is_rejected() {
        let root = temp("nested");
        let workspace = root.join("ws");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).expect("ws");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("file"), b"x").expect("file");
        let link = workspace.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        let policy = Policy::with_home(&workspace, None).expect("policy");
        assert!(matches!(
            policy.read(&link.join("file")),
            Err(PathError::Escapes(_))
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn symlinked_workspace_renders_resolved() {
        let root = temp("symlink");
        let target = root.join("target");
        let link = root.join("link");
        std::fs::create_dir_all(&target).expect("target");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).expect("link");

        let policy = Policy::with_home(&link, None).expect("policy");
        let rendered = policy.render();

        assert_eq!(policy.workspace(), target.canonicalize().unwrap());
        assert!(rendered.contains(&target.canonicalize().unwrap().display().to_string()));
        assert!(!rendered.contains(&link.display().to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn connect_grants_are_validated_and_rendered() {
        assert!(Connect::parse("localhost:8765").is_ok());
        assert!(Connect::parse("*:8765").is_err());
        assert!(Connect::parse("127.0.0.1:8765").is_err());
        assert!(Connect::parse("example.com:443").is_err());
        assert!(Connect::parse("[::1]:8765").is_err());
        assert!(Connect::parse("example.com:443").is_err());
        assert!(Connect::parse("*:*").is_err());
        assert!(Connect::parse("127.0.0.1:0").is_err());
        assert!(Connect::parse("127.0.0.1").is_err());
        assert!(Connect::parse("x\") (allow file-write* (subpath \"/\"))").is_err());

        let workspace = std::env::temp_dir();
        let rendered = Policy::with_home(&workspace, None)
            .unwrap()
            .connect("localhost:8765")
            .unwrap()
            .render();
        assert!(rendered.contains("(remote ip \"localhost:8765\")"));
        assert!(rendered.contains("(allow network-outbound\n"));
    }

    #[test]
    fn tag_is_validated() {
        let workspace = std::env::temp_dir();
        let policy = Policy::with_home(&workspace, None).unwrap();
        assert!(policy.clone().tag("sandbox-1").is_ok());
        assert!(policy.tag("bad tag\")").is_err());
    }

    #[test]
    fn credential_denies_are_unconditional_and_ordered_last() {
        let home = temp("home");
        let workspace = temp("home-ws");
        std::fs::create_dir_all(home.join(".ssh")).expect("ssh");

        let policy = Policy::with_home(&workspace, Some(&home))
            .unwrap()
            .read(&home)
            .unwrap();
        let rendered = policy.render();

        let allow = rendered.find("(allow file-read*").expect("allow");
        let deny = rendered
            .find("(deny file-read*\n")
            .expect("credential deny");
        assert!(deny > allow, "denies must follow the grants");

        // Every credential path is denied, present or not.
        assert!(rendered.contains("(deny file-read*\n  (subpath"));
        assert!(rendered.contains("(deny file-write-unlink\n  (subpath"));

        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&workspace).ok();
    }

    fn golden_cases() -> Vec<(&'static str, String)> {
        let root = Path::new("/private/tmp/lbx-golden");
        for dir in ["ws", "home", "read", "write"] {
            std::fs::create_dir_all(root.join(dir)).expect("golden dir");
        }

        vec![
            (
                "empty-net",
                Policy::with_home(&root.join("ws"), None)
                    .expect("policy")
                    .render(),
            ),
            (
                "connect-grant",
                Policy::with_home(&root.join("ws"), None)
                    .expect("policy")
                    .connect("localhost:8765")
                    .expect("connect")
                    .render(),
            ),
            // The invariant block only appears when a home is supplied, and the
            // grants only when they are requested; both must be pinned.
            (
                "home-and-grants",
                Policy::with_home(&root.join("ws"), Some(&root.join("home")))
                    .expect("policy")
                    .read(&root.join("read"))
                    .expect("read")
                    .write(&root.join("write"))
                    .expect("write")
                    .render(),
            ),
        ]
    }

    #[test]
    fn golden_profiles() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures");
        for (name, rendered) in golden_cases() {
            let path = fixtures.join(format!("{name}.sbpl"));
            let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!(
                    "read {}: {err}. Bless deliberately with `cargo test -- --ignored bless_golden`",
                    path.display()
                )
            });
            assert_eq!(rendered, expected, "golden mismatch for {name}");
        }
    }

    #[test]
    #[ignore = "rewrites src/fixtures; run explicitly to bless a reviewed change"]
    fn bless_golden() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/fixtures");
        std::fs::create_dir_all(&fixtures).expect("fixture dir");
        for (name, rendered) in golden_cases() {
            std::fs::write(fixtures.join(format!("{name}.sbpl")), rendered).expect("write");
        }
    }
}
