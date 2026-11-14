//! The SBPL renderer and its parameter table.
//!
//! No dynamic value is escaped into the profile text: a path, tag, mach
//! service or endpoint is a parameter passed separately to
//! `sandbox_init_with_parameters`. This module has no dependency on the rest of
//! the crate, because the helper includes it by path.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::CString;
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The largest profile text the renderer will produce.
pub(crate) const MAX_TEXT_BYTES: usize = 60 * 1024;

/// The largest parameter table the renderer will produce.
pub(crate) const MAX_PARAMETERS: usize = 4096;

/// What a sandbox reaches on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Posture {
    /// No network-outbound rule.
    Denied,
    /// Every host and port.
    Host,
    /// Loopback endpoints, each `localhost:<port>`. Not configured in this ADR.
    Loopback(Vec<String>),
}

/// Everything the writer needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileInput {
    pub(crate) workspace: PathBuf,
    pub(crate) reads: Vec<PathBuf>,
    pub(crate) writes: Vec<PathBuf>,
    pub(crate) posture: Posture,
    pub(crate) tag: String,
    pub(crate) real_home: PathBuf,
    pub(crate) baseline_reads: Vec<PathBuf>,
    pub(crate) mach_services: Vec<String>,
    pub(crate) credential_denies: Vec<PathBuf>,
}

/// A rendered profile and the values to pass with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedProfile {
    pub(crate) text: String,
    pub(crate) parameters: Vec<Parameter>,
}

/// One `KEY, VALUE` pair for `sandbox_init_with_parameters`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) value: CString,
}

/// Why a profile was not produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RenderError {
    /// The text is above the guard.
    TooLarge { bytes: usize },
    /// The parameter table is above the guard.
    TooManyParameters { count: usize },
    /// The tag does not match the allowed characters.
    InvalidTag { value: String },
    /// A mach service name does not match the allowed characters.
    InvalidService { value: String },
    /// An endpoint is not `localhost:<port>`.
    InvalidEndpoint { value: String },
    /// A path could not be resolved, or is not absolute.
    BadPath { value: String },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => {
                write!(
                    f,
                    "the profile is {bytes} bytes, above the {MAX_TEXT_BYTES} guard"
                )
            }
            Self::TooManyParameters { count } => write!(
                f,
                "the profile needs {count} parameters, above the {MAX_PARAMETERS} guard"
            ),
            Self::InvalidTag { value } => write!(f, "the tag '{value}' is not valid"),
            Self::InvalidService { value } => {
                write!(f, "the mach service '{value}' is not valid")
            }
            Self::InvalidEndpoint { value } => {
                write!(f, "the endpoint '{value}' is not localhost:<port>")
            }
            Self::BadPath { value } => write!(f, "the path '{value}' could not be resolved"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Render the profile for one sandbox.
pub(crate) fn render(input: &ProfileInput) -> Result<RenderedProfile, RenderError> {
    validate_tag(&input.tag).map_err(|_| RenderError::InvalidTag {
        value: input.tag.clone(),
    })?;

    let workspace = resolve(&input.workspace)?;

    let mut reads = Vec::new();
    for path in &input.baseline_reads {
        push_path(&mut reads, resolve(path)?);
    }
    push_path(&mut reads, workspace.clone());
    for path in &input.reads {
        push_path(&mut reads, resolve(path)?);
    }

    let mut writes = Vec::new();
    push_path(&mut writes, workspace.clone());
    for path in &input.writes {
        push_path(&mut writes, resolve(path)?);
    }

    let mut denies = Vec::new();
    for path in &input.credential_denies {
        if !path.is_absolute() {
            return Err(RenderError::BadPath {
                value: path.display().to_string(),
            });
        }
        push_path(&mut denies, path.clone());
    }

    let mut services = Vec::new();
    for service in &input.mach_services {
        validate_service(service).map_err(|_| RenderError::InvalidService {
            value: service.clone(),
        })?;
        if !services.iter().any(|value| value == service) {
            services.push(service.clone());
        }
    }

    let endpoints = match &input.posture {
        Posture::Loopback(endpoints) => {
            for endpoint in endpoints {
                validate_endpoint(endpoint).map_err(|_| RenderError::InvalidEndpoint {
                    value: endpoint.clone(),
                })?;
            }
            endpoints.clone()
        }
        _ => Vec::new(),
    };

    let mut metadata = Vec::new();
    for path in reads.iter().chain(writes.iter()).chain(denies.iter()) {
        for ancestor in path.ancestors() {
            push_path(&mut metadata, ancestor.to_path_buf());
        }
    }
    for ancestor in input.real_home.ancestors() {
        push_path(&mut metadata, ancestor.to_path_buf());
    }

    let mut parameters = Parameters::default();
    let mut text = String::with_capacity(4096);

    text.push_str("(version 1)\n");
    let tag = parameters.intern(&input.tag)?;
    text.push_str(&format!(
        "(deny default (with message (param \"{tag}\")))\n"
    ));
    text.push_str("(allow process-exec)\n");
    text.push_str("(allow process-fork)\n");
    text.push_str("(allow process-info* (target same-sandbox))\n");
    text.push_str("(allow signal (target same-sandbox))\n");

    let mut read_filters = vec!["(literal \"/\")".to_string()];
    for path in &reads {
        read_filters.push(subpath_filter(&mut parameters, path)?);
    }
    write_block(&mut text, "(allow file-read*", &read_filters);

    let mut metadata_filters = Vec::with_capacity(metadata.len());
    for path in &metadata {
        metadata_filters.push(literal_filter(&mut parameters, path)?);
    }
    write_block(&mut text, "(allow file-read-metadata", &metadata_filters);

    let mut write_filters = vec!["(literal \"/dev/null\")".to_string()];
    for path in &writes {
        write_filters.push(subpath_filter(&mut parameters, path)?);
    }
    write_block(&mut text, "(allow file-write*", &write_filters);

    text.push_str("(allow sysctl-read)\n");

    if !services.is_empty() {
        let mut filters = Vec::with_capacity(services.len());
        for service in &services {
            filters.push(global_name_filter(&mut parameters, service)?);
        }
        write_block(&mut text, "(allow mach-lookup", &filters);
    }

    match input.posture {
        Posture::Denied => {}
        Posture::Host => text.push_str("(allow network-outbound)\n"),
        Posture::Loopback(_) => {
            let mut filters = Vec::with_capacity(endpoints.len());
            for endpoint in &endpoints {
                filters.push(remote_ip_filter(&mut parameters, endpoint)?);
            }
            if !filters.is_empty() {
                write_block(&mut text, "(allow network-outbound", &filters);
            }
        }
    }

    if !denies.is_empty() {
        let mut filters = Vec::with_capacity(denies.len());
        for path in &denies {
            filters.push(subpath_filter(&mut parameters, path)?);
        }
        write_block(&mut text, "(deny file-read*", &filters);
        write_block(&mut text, "(deny file-write*", &filters);
        write_block(&mut text, "(deny file-write-unlink", &filters);
    }

    if parameters.values.len() > MAX_PARAMETERS {
        return Err(RenderError::TooManyParameters {
            count: parameters.values.len(),
        });
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(RenderError::TooLarge { bytes: text.len() });
    }

    Ok(RenderedProfile {
        text,
        parameters: parameters.values,
    })
}

/// A tag is the sandbox name and reaches a log query.
pub(crate) fn validate_tag(value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err("use 1 to 64 ASCII letters, digits, '.', '_' or '-'".to_string())
    }
}

/// A mach service name is a reverse-DNS token.
pub(crate) fn validate_service(value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 255
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '@' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err("use 1 to 255 ASCII letters, digits, '.', '_', '@' or '-'".to_string())
    }
}

fn validate_endpoint(value: &str) -> Result<(), String> {
    let Some((host, port)) = value.split_once(':') else {
        return Err("use localhost:<port>".to_string());
    };
    if host != "localhost" {
        return Err("the host must be localhost".to_string());
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| "the port must be 1 to 65535".to_string())?;
    if port == 0 {
        return Err("the port must be 1 to 65535".to_string());
    }
    Ok(())
}

fn write_block(text: &mut String, head: &str, filters: &[String]) {
    text.push_str(head);
    text.push('\n');
    for filter in filters {
        text.push_str("  ");
        text.push_str(filter);
        text.push('\n');
    }
    text.push_str(")\n");
}

fn subpath_filter(parameters: &mut Parameters, path: &Path) -> Result<String, RenderError> {
    let name = parameters.intern_path(path)?;
    Ok(format!("(subpath (param \"{name}\"))"))
}

fn literal_filter(parameters: &mut Parameters, path: &Path) -> Result<String, RenderError> {
    let name = parameters.intern_path(path)?;
    Ok(format!("(literal (param \"{name}\"))"))
}

fn global_name_filter(parameters: &mut Parameters, service: &str) -> Result<String, RenderError> {
    let name = parameters.intern(service)?;
    Ok(format!("(global-name (param \"{name}\"))"))
}

fn remote_ip_filter(parameters: &mut Parameters, endpoint: &str) -> Result<String, RenderError> {
    let name = parameters.intern(endpoint)?;
    Ok(format!("(remote ip (param \"{name}\"))"))
}

fn resolve(path: &Path) -> Result<PathBuf, RenderError> {
    std::fs::canonicalize(path).map_err(|_| RenderError::BadPath {
        value: path.display().to_string(),
    })
}

fn push_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[derive(Default)]
struct Parameters {
    names: HashMap<CString, String>,
    values: Vec<Parameter>,
}

impl Parameters {
    fn intern(&mut self, value: &str) -> Result<String, RenderError> {
        let value = CString::new(value.as_bytes()).map_err(|_| RenderError::InvalidTag {
            value: value.to_string(),
        })?;
        Ok(self.intern_cstring(value))
    }

    fn intern_path(&mut self, path: &Path) -> Result<String, RenderError> {
        let value =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| RenderError::BadPath {
                value: path.display().to_string(),
            })?;
        Ok(self.intern_cstring(value))
    }

    fn intern_cstring(&mut self, value: CString) -> String {
        match self.names.entry(value) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let name = format!("p{}", self.values.len());
                let value = entry.key().clone();
                entry.insert(name.clone());
                self.values.push(Parameter {
                    name: name.clone(),
                    value,
                });
                name
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{
        MAX_PARAMETERS, Posture, ProfileInput, RenderError, render, validate_service, validate_tag,
    };

    fn workspace() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn input(workspace: &Path) -> ProfileInput {
        ProfileInput {
            workspace: workspace.to_path_buf(),
            reads: Vec::new(),
            writes: Vec::new(),
            posture: Posture::Denied,
            tag: "demo".to_string(),
            real_home: PathBuf::from("/Users/tester"),
            baseline_reads: Vec::new(),
            mach_services: Vec::new(),
            credential_denies: Vec::new(),
        }
    }

    #[test]
    fn the_root_literal_process_rules_and_workspace_are_emitted() {
        let workspace = workspace();
        let rendered = render(&input(workspace.path())).expect("renders");
        assert!(rendered.text.starts_with("(version 1)\n"));
        assert!(
            rendered
                .text
                .contains("(deny default (with message (param \"p0\")))")
        );
        assert!(rendered.text.contains("(allow process-exec)"));
        assert!(rendered.text.contains("(literal \"/\")"));
        assert!(rendered.text.contains("(allow file-write*"));
        assert!(rendered.text.contains("(literal \"/dev/null\")"));
        assert!(!rendered.text.contains("/nix"));
    }

    #[test]
    fn every_dynamic_value_is_a_parameter() {
        let workspace = workspace();
        let odd = workspace.path().join("a\"b(c)");
        std::fs::create_dir_all(&odd).expect("odd dir");
        let mut input = input(workspace.path());
        input.reads.push(odd.clone());
        let rendered = render(&input).expect("renders");
        assert!(
            !rendered.text.contains("a\"b(c)"),
            "nothing dynamic is in the text"
        );
        let value = odd.canonicalize().expect("canonical");
        assert!(
            rendered
                .parameters
                .iter()
                .any(|parameter| parameter.value.to_bytes() == value.as_os_str().as_bytes()),
            "the path is in the table"
        );
    }

    #[test]
    fn a_value_used_twice_shares_one_parameter() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input.reads.push(workspace.path().to_path_buf());
        let rendered = render(&input).expect("renders");
        let canonical = workspace.path().canonicalize().expect("canonical");
        let count = rendered
            .parameters
            .iter()
            .filter(|parameter| parameter.value.to_bytes() == canonical.as_os_str().as_bytes())
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn metadata_lists_ancestors_as_literals() {
        let workspace = workspace();
        let rendered = render(&input(workspace.path())).expect("renders");
        assert!(rendered.text.contains("(allow file-read-metadata"));
        let canonical = workspace.path().canonicalize().expect("canonical");
        let root_name = rendered
            .parameters
            .iter()
            .find(|parameter| parameter.value.to_bytes() == b"/")
            .map(|parameter| parameter.name.clone())
            .expect("root parameter");
        assert!(
            rendered
                .text
                .contains(&format!("(literal (param \"{root_name}\"))")),
            "the root ancestor is a literal"
        );
        assert!(canonical.ancestors().count() > 1);
    }

    #[test]
    fn credential_denies_are_the_last_layer() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input
            .credential_denies
            .push(PathBuf::from("/Users/tester/.ssh"));
        let rendered = render(&input).expect("renders");
        let deny = rendered.text.rfind("(deny file-read*").expect("deny");
        let allow = rendered.text.rfind("(allow file-write*").expect("allow");
        assert!(deny > allow);
        assert!(rendered.text.contains("(deny file-write-unlink"));
    }

    #[test]
    fn the_network_posture_is_rendered_without_a_wildcard() {
        let workspace = workspace();
        let mut denied = input(workspace.path());
        denied.posture = Posture::Denied;
        assert!(
            !render(&denied)
                .expect("renders")
                .text
                .contains("network-outbound")
        );

        let mut host = input(workspace.path());
        host.posture = Posture::Host;
        assert!(
            render(&host)
                .expect("renders")
                .text
                .contains("(allow network-outbound)\n")
        );

        let mut loopback = input(workspace.path());
        loopback.posture = Posture::Loopback(vec!["localhost:8765".to_string()]);
        let rendered = render(&loopback).expect("renders");
        assert!(rendered.text.contains("(remote ip (param"));
        assert!(!rendered.text.contains("(allow network-outbound)\n"));
    }

    #[test]
    fn mach_rules_are_omitted_when_there_are_no_services() {
        let workspace = workspace();
        let mut empty = input(workspace.path());
        empty.mach_services = Vec::new();
        assert!(
            !render(&empty)
                .expect("renders")
                .text
                .contains("mach-lookup")
        );

        let mut with_service = input(workspace.path());
        with_service.mach_services = vec!["com.apple.example".to_string()];
        let rendered = render(&with_service).expect("renders");
        assert!(rendered.text.contains("(allow mach-lookup"));
        assert!(rendered.text.contains("(global-name (param"));
    }

    #[test]
    fn the_writes_layer_always_contains_the_workspace() {
        let workspace = workspace();
        let rendered = render(&input(workspace.path())).expect("renders");
        let writes = rendered
            .text
            .split("(allow file-write*")
            .nth(1)
            .expect("writes");
        assert!(writes.contains("(subpath (param"));
    }

    #[test]
    fn the_tag_service_and_endpoint_are_validated() {
        let workspace = workspace();
        let mut bad_tag = input(workspace.path());
        bad_tag.tag = "with space".to_string();
        assert!(matches!(
            render(&bad_tag),
            Err(RenderError::InvalidTag { .. })
        ));

        let mut bad_service = input(workspace.path());
        bad_service.mach_services = vec!["not a service".to_string()];
        assert!(matches!(
            render(&bad_service),
            Err(RenderError::InvalidService { .. })
        ));

        let mut bad_endpoint = input(workspace.path());
        bad_endpoint.posture = Posture::Loopback(vec!["example.com:443".to_string()]);
        assert!(matches!(
            render(&bad_endpoint),
            Err(RenderError::InvalidEndpoint { .. })
        ));
    }

    #[test]
    fn an_unresolvable_path_is_refused() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input.reads.push(PathBuf::from("/does/not/exist"));
        assert!(matches!(render(&input), Err(RenderError::BadPath { .. })));
    }

    #[test]
    fn too_many_parameters_are_refused() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input.mach_services = (0..=MAX_PARAMETERS)
            .map(|index| format!("com.apple.service{index}"))
            .collect();
        assert!(matches!(
            render(&input),
            Err(RenderError::TooManyParameters { .. })
        ));
    }

    #[test]
    fn oversized_text_is_refused() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input.mach_services = (0..4000)
            .map(|index| format!("com.apple.{index}{}", "x".repeat(100)))
            .collect();
        assert!(matches!(render(&input), Err(RenderError::TooLarge { .. })));
    }

    #[test]
    fn rendering_is_deterministic() {
        let workspace = workspace();
        let mut input = input(workspace.path());
        input.mach_services = vec!["com.apple.example".to_string()];
        let first = render(&input).expect("renders");
        let second = render(&input).expect("renders");
        assert_eq!(first, second);
    }

    #[test]
    fn the_shared_validators_agree_on_the_rules() {
        assert!(validate_tag("demo-1.2_x").is_ok());
        assert!(validate_tag("with/slash").is_err());
        assert!(validate_service("com.apple.example").is_ok());
        assert!(validate_service("not a service").is_err());
    }

    const FIXTURE_ROOT: &str = "/private/tmp/lbx-fixture";

    fn fixture_input(
        reads: &[&str],
        posture: Posture,
        services: &[&str],
        denies: &[&str],
    ) -> ProfileInput {
        std::fs::create_dir_all(format!("{FIXTURE_ROOT}/workspace")).expect("workspace");
        for path in reads {
            std::fs::create_dir_all(path).expect("read dir");
        }
        ProfileInput {
            workspace: PathBuf::from(format!("{FIXTURE_ROOT}/workspace")),
            reads: reads.iter().map(PathBuf::from).collect(),
            writes: Vec::new(),
            posture,
            tag: "demo".to_string(),
            real_home: PathBuf::from("/Users/tester"),
            baseline_reads: Vec::new(),
            mach_services: services.iter().map(|service| service.to_string()).collect(),
            credential_denies: denies.iter().map(PathBuf::from).collect(),
        }
    }

    fn golden_cases() -> Vec<(&'static str, ProfileInput)> {
        vec![
            ("empty", fixture_input(&[], Posture::Denied, &[], &[])),
            (
                "one-read",
                fixture_input(
                    &["/private/tmp/lbx-fixture/read"],
                    Posture::Denied,
                    &[],
                    &[],
                ),
            ),
            (
                "two-reads",
                fixture_input(
                    &[
                        "/private/tmp/lbx-fixture/a/one",
                        "/private/tmp/lbx-fixture/b/two",
                    ],
                    Posture::Denied,
                    &[],
                    &[],
                ),
            ),
            (
                "collapsed",
                fixture_input(&["/private/tmp"], Posture::Denied, &[], &[]),
            ),
            (
                "denied",
                fixture_input(
                    &["/private/tmp/lbx-fixture/read"],
                    Posture::Denied,
                    &["com.apple.example"],
                    &["/Users/tester/.ssh"],
                ),
            ),
            (
                "host",
                fixture_input(&["/private/tmp/lbx-fixture/read"], Posture::Host, &[], &[]),
            ),
            (
                "loopback",
                fixture_input(
                    &["/private/tmp/lbx-fixture/read"],
                    Posture::Loopback(vec!["localhost:8765".to_string()]),
                    &[],
                    &[],
                ),
            ),
        ]
    }

    fn fixture(name: &str) -> &'static str {
        match name {
            "empty" => include_str!("fixtures/empty.sbpl"),
            "one-read" => include_str!("fixtures/one-read.sbpl"),
            "two-reads" => include_str!("fixtures/two-reads.sbpl"),
            "collapsed" => include_str!("fixtures/collapsed.sbpl"),
            "denied" => include_str!("fixtures/denied.sbpl"),
            "host" => include_str!("fixtures/host.sbpl"),
            "loopback" => include_str!("fixtures/loopback.sbpl"),
            other => panic!("no fixture for {other}"),
        }
    }

    #[test]
    fn golden_profiles_match_their_fixtures() {
        for (name, input) in golden_cases() {
            let rendered = render(&input).expect("renders");
            assert_eq!(rendered.text, fixture(name), "{name}");
        }
    }

    #[test]
    fn the_parameter_table_is_sequential_and_used() {
        for (name, input) in golden_cases() {
            let rendered = render(&input).expect("renders");
            for (index, parameter) in rendered.parameters.iter().enumerate() {
                assert_eq!(parameter.name, format!("p{index}"), "{name}");
                assert!(
                    rendered
                        .text
                        .contains(&format!("(param \"{}\")", parameter.name)),
                    "{name}: {} is unused",
                    parameter.name
                );
            }
        }
    }

    #[test]
    fn the_parameter_values_are_the_inputs() {
        let input = golden_cases()
            .into_iter()
            .find(|(name, _)| *name == "one-read")
            .expect("one-read")
            .1;
        let rendered = render(&input).expect("renders");
        assert_eq!(rendered.parameters[0].value.to_bytes(), b"demo");
        assert_eq!(
            rendered.parameters[1].value.to_bytes(),
            b"/private/tmp/lbx-fixture/workspace"
        );
        assert_eq!(
            rendered.parameters[2].value.to_bytes(),
            b"/private/tmp/lbx-fixture/read"
        );
        assert!(
            rendered
                .parameters
                .iter()
                .any(|parameter| parameter.value.to_bytes() == b"/Users/tester")
        );
        assert!(!rendered.text.contains("/private/tmp"));
        assert!(!rendered.text.contains("demo"));
    }

    #[test]
    fn every_layer_changes_the_text() {
        assert_ne!(fixture("empty"), fixture("one-read"));
        assert_ne!(fixture("one-read"), fixture("two-reads"));
        assert_ne!(fixture("one-read"), fixture("collapsed"));
        assert_ne!(fixture("empty"), fixture("denied"));
        assert_ne!(fixture("denied"), fixture("host"));
        assert_ne!(fixture("host"), fixture("loopback"));

        for name in [
            "empty",
            "one-read",
            "two-reads",
            "collapsed",
            "denied",
            "host",
            "loopback",
        ] {
            assert!(fixture(name).contains("(literal \"/\")"), "{name}");
            assert!(
                fixture(name).contains("(allow file-read-metadata"),
                "{name}"
            );
            assert!(!fixture(name).contains("/nix"), "{name}");
        }
        assert!(fixture("denied").contains("(allow mach-lookup"));
        assert!(fixture("denied").contains("(deny file-read*"));
        assert!(!fixture("denied").contains("network-outbound"));
        assert!(fixture("host").contains("(allow network-outbound)\n"));
        assert!(fixture("loopback").contains("(remote ip (param"));
    }
}
