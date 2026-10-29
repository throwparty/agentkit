//! Tool name rendering for model API surfaces.
//!
//! The registry owns canonical [`InvokableName`]s. A [`ToolNameScheme`]
//! renders those names into [`WireToolName`]s for one API surface, and a
//! [`ToolNameMap`] keeps the per-request mapping needed to decode model tool
//! calls back to canonical names.

use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

const DEFAULT_MAX_LENGTH: usize = 64;

/// A canonical invokable name (`mcp.<server>.<tool>`, `prompt.<name>`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InvokableName(String);

impl InvokableName {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvokableName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<String> for InvokableName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for InvokableName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for InvokableName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for InvokableName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A tool name as it appears on a model API surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WireToolName(String);

impl WireToolName {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WireToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<String> for WireToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for WireToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for WireToolName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for WireToolName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// How namespace dots become wire characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceSeparator {
    Dot,
    SingleUnderscore,
    DoubleUnderscore,
}

impl NamespaceSeparator {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dot => ".",
            Self::SingleUnderscore => "_",
            Self::DoubleUnderscore => "__",
        }
    }
}

/// The scheme name accepted by `tool_names.scheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolNameSchemeName {
    Strict,
    Gemini,
    Meta,
    Cohere,
    Custom,
}

impl ToolNameSchemeName {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Gemini => "gemini",
            Self::Meta => "meta",
            Self::Cohere => "cohere",
            Self::Custom => "custom",
        }
    }

    /// The default scheme for a wire format.
    #[must_use]
    pub fn for_surface(surface: &str) -> Self {
        match surface {
            "google-generate-content" => Self::Gemini,
            _ => Self::Strict,
        }
    }

    /// Builds the scheme, applying an optional maximum length and custom pattern.
    pub fn build(
        self,
        pattern: Option<&str>,
        max_length: Option<usize>,
    ) -> Result<Arc<dyn ToolNameScheme>, ToolNameError> {
        let max_length = max_length.unwrap_or(DEFAULT_MAX_LENGTH);
        match self {
            Self::Strict => Ok(Arc::new(StrictToolNameScheme::new(max_length))),
            Self::Gemini => Ok(Arc::new(GeminiToolNameScheme::new(max_length))),
            Self::Meta => Ok(Arc::new(MetaToolNameScheme::new(max_length))),
            Self::Cohere => Ok(Arc::new(CohereToolNameScheme::new(max_length))),
            Self::Custom => {
                let pattern = pattern.ok_or(ToolNameError::MissingPattern)?;
                Ok(Arc::new(CustomToolNameScheme::new(pattern, max_length)?))
            }
        }
    }
}

/// A rendered name and whether rendering lost information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedToolName {
    pub wire: WireToolName,
    pub lossy: bool,
}

/// Renders canonical names for one API surface.
pub trait ToolNameScheme: Send + Sync + fmt::Debug {
    /// Stable scheme name used in diagnostics.
    fn name(&self) -> &'static str;

    /// Maximum wire-name length in characters.
    fn max_length(&self) -> usize;

    /// Renders a canonical name. The result is a readable base name and may
    /// already be truncated to the scheme maximum.
    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError>;

    /// Best-effort reverse of [`Self::encode`]. The request-local
    /// [`ToolNameMap`] is authoritative when names are ambiguous.
    fn decode(&self, name: &WireToolName) -> Option<InvokableName>;

    /// Whether `wire` differs from the lossless readable rendering of
    /// `canonical`.
    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool;

    /// Rejects a final rendered name that the scheme cannot accept.
    fn validate(&self, _name: &WireToolName) -> Result<(), ToolNameError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct SchemeRules {
    separator: NamespaceSeparator,
    allowed: fn(char) -> bool,
    max_length: usize,
    require_initial_letter: bool,
    reject_leading_digit: bool,
}

impl SchemeRules {
    fn readable(&self, name: &InvokableName) -> String {
        let mut out = String::new();
        for character in name.as_str().chars() {
            if character == '.' {
                out.push_str(self.separator.as_str());
            } else if (self.allowed)(character) {
                out.push(character);
            } else {
                out.push('_');
            }
        }
        if self.require_initial_letter
            && out
                .chars()
                .next()
                .is_some_and(|first| !first.is_ascii_alphabetic() && first != '_')
        {
            out.insert(0, '_');
        }
        if self.reject_leading_digit
            && out
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_digit())
        {
            out.insert(0, '_');
        }
        out
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        let readable = self.readable(name);
        if readable.is_empty() {
            return Err(ToolNameError::Empty);
        }
        truncate(&readable, name, self.max_length)
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        wire.as_str() != self.readable(canonical)
    }
}

fn truncate(
    readable: &str,
    canonical: &InvokableName,
    max_length: usize,
) -> Result<WireToolName, ToolNameError> {
    if readable.len() <= max_length {
        return Ok(WireToolName::new(readable));
    }
    let suffix = hash_suffix(canonical.as_str());
    let keep = max_length.saturating_sub(suffix.len() + 1);
    if keep == 0 {
        return Err(ToolNameError::TooLong {
            name: canonical.to_string(),
            max_length,
        });
    }
    let truncated = readable.chars().take(keep).collect::<String>();
    Ok(WireToolName::new(format!("{truncated}_{suffix}")))
}

fn hash_suffix(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_double_underscore(name: &WireToolName) -> Option<InvokableName> {
    let mut out = String::new();
    let mut characters = name.as_str().chars().peekable();
    while let Some(character) = characters.next() {
        if character == '_' && characters.peek() == Some(&'_') {
            characters.next();
            out.push('.');
        } else {
            out.push(character);
        }
    }
    Some(InvokableName::new(out))
}

/// OpenAI and Anthropic style names: `[A-Za-z0-9_]`, maximum 64.
#[derive(Debug, Clone)]
pub struct StrictToolNameScheme {
    rules: SchemeRules,
}

impl StrictToolNameScheme {
    #[must_use]
    pub fn new(max_length: usize) -> Self {
        Self {
            rules: SchemeRules {
                separator: NamespaceSeparator::DoubleUnderscore,
                allowed: |character| character.is_ascii_alphanumeric() || character == '_',
                max_length,
                require_initial_letter: false,
                reject_leading_digit: false,
            },
        }
    }
}

impl Default for StrictToolNameScheme {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LENGTH)
    }
}

impl ToolNameScheme for StrictToolNameScheme {
    fn name(&self) -> &'static str {
        "strict"
    }

    fn max_length(&self) -> usize {
        self.rules.max_length
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        self.rules.encode(name)
    }

    fn decode(&self, name: &WireToolName) -> Option<InvokableName> {
        decode_double_underscore(name)
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        self.rules.is_lossy(canonical, wire)
    }
}

/// Gemini style names: start with a letter or `_`, dots allowed, maximum 64.
#[derive(Debug, Clone)]
pub struct GeminiToolNameScheme {
    rules: SchemeRules,
}

impl GeminiToolNameScheme {
    #[must_use]
    pub fn new(max_length: usize) -> Self {
        Self {
            rules: SchemeRules {
                separator: NamespaceSeparator::Dot,
                allowed: |character| {
                    character.is_ascii_alphanumeric() || character == '_' || character == '-'
                },
                max_length,
                require_initial_letter: true,
                reject_leading_digit: false,
            },
        }
    }
}

impl Default for GeminiToolNameScheme {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LENGTH)
    }
}

impl ToolNameScheme for GeminiToolNameScheme {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn max_length(&self) -> usize {
        self.rules.max_length
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        self.rules.encode(name)
    }

    fn decode(&self, name: &WireToolName) -> Option<InvokableName> {
        Some(InvokableName::new(name.as_str()))
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        self.rules.is_lossy(canonical, wire)
    }
}

/// Meta style names: dots are flattened, underscore and hyphen allowed.
#[derive(Debug, Clone)]
pub struct MetaToolNameScheme {
    rules: SchemeRules,
}

impl MetaToolNameScheme {
    #[must_use]
    pub fn new(max_length: usize) -> Self {
        Self {
            rules: SchemeRules {
                separator: NamespaceSeparator::DoubleUnderscore,
                allowed: |character| {
                    character.is_ascii_alphanumeric() || character == '_' || character == '-'
                },
                max_length,
                require_initial_letter: false,
                reject_leading_digit: false,
            },
        }
    }
}

impl Default for MetaToolNameScheme {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LENGTH)
    }
}

impl ToolNameScheme for MetaToolNameScheme {
    fn name(&self) -> &'static str {
        "meta"
    }

    fn max_length(&self) -> usize {
        self.rules.max_length
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        self.rules.encode(name)
    }

    fn decode(&self, name: &WireToolName) -> Option<InvokableName> {
        decode_double_underscore(name)
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        self.rules.is_lossy(canonical, wire)
    }
}

/// Cohere style names: letters, digits, underscore; no leading digit.
#[derive(Debug, Clone)]
pub struct CohereToolNameScheme {
    rules: SchemeRules,
}

impl CohereToolNameScheme {
    #[must_use]
    pub fn new(max_length: usize) -> Self {
        Self {
            rules: SchemeRules {
                separator: NamespaceSeparator::DoubleUnderscore,
                allowed: |character| character.is_ascii_alphanumeric() || character == '_',
                max_length,
                require_initial_letter: false,
                reject_leading_digit: true,
            },
        }
    }
}

impl Default for CohereToolNameScheme {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LENGTH)
    }
}

impl ToolNameScheme for CohereToolNameScheme {
    fn name(&self) -> &'static str {
        "cohere"
    }

    fn max_length(&self) -> usize {
        self.rules.max_length
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        self.rules.encode(name)
    }

    fn decode(&self, name: &WireToolName) -> Option<InvokableName> {
        decode_double_underscore(name)
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        self.rules.is_lossy(canonical, wire)
    }
}

/// A caller-supplied pattern over the strict readable form.
#[derive(Debug, Clone)]
pub struct CustomToolNameScheme {
    rules: SchemeRules,
    pattern: Regex,
}

impl CustomToolNameScheme {
    pub fn new(pattern: &str, max_length: usize) -> Result<Self, ToolNameError> {
        let pattern = Regex::new(pattern).map_err(|_| ToolNameError::InvalidPattern {
            pattern: pattern.to_owned(),
        })?;
        Ok(Self {
            rules: SchemeRules {
                separator: NamespaceSeparator::DoubleUnderscore,
                allowed: |character| character.is_ascii_alphanumeric() || character == '_',
                max_length,
                require_initial_letter: false,
                reject_leading_digit: false,
            },
            pattern,
        })
    }
}

impl ToolNameScheme for CustomToolNameScheme {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn max_length(&self) -> usize {
        self.rules.max_length
    }

    fn encode(&self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        let wire = self.rules.encode(name)?;
        if !self.pattern.is_match(wire.as_str()) {
            return Err(ToolNameError::PatternMismatch {
                name: name.to_string(),
            });
        }
        Ok(wire)
    }

    fn decode(&self, name: &WireToolName) -> Option<InvokableName> {
        decode_double_underscore(name)
    }

    fn is_lossy(&self, canonical: &InvokableName, wire: &WireToolName) -> bool {
        self.rules.is_lossy(canonical, wire)
    }

    fn validate(&self, wire: &WireToolName) -> Result<(), ToolNameError> {
        if self.pattern.is_match(wire.as_str()) {
            return Ok(());
        }
        Err(ToolNameError::PatternMismatch {
            name: wire.as_str().to_owned(),
        })
    }
}

/// Identity and surface labels used for diagnostics.
#[derive(Debug, Clone)]
pub struct ToolNameContext {
    pub endpoint: String,
    pub model: String,
    pub surface: String,
}

impl ToolNameContext {
    #[must_use]
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, surface: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            surface: surface.into(),
        }
    }
}

/// The request-local mapping between canonical names and wire names.
#[derive(Debug, Clone)]
pub struct ToolNameMap {
    scheme: Arc<dyn ToolNameScheme>,
    context: ToolNameContext,
    current: BTreeSet<InvokableName>,
    forward: BTreeMap<InvokableName, WireToolName>,
    reverse: BTreeMap<WireToolName, InvokableName>,
}

impl ToolNameMap {
    #[must_use]
    pub fn new(scheme: Arc<dyn ToolNameScheme>, context: ToolNameContext) -> Self {
        Self {
            scheme,
            context,
            current: BTreeSet::new(),
            forward: BTreeMap::new(),
            reverse: BTreeMap::new(),
        }
    }

    /// Builds a map for the current model-invokable names.
    pub fn build(
        scheme: Arc<dyn ToolNameScheme>,
        context: ToolNameContext,
        names: impl IntoIterator<Item = InvokableName>,
    ) -> Result<Self, ToolNameError> {
        let mut map = Self::new(scheme, context);
        for name in names {
            map.current.insert(name.clone());
            map.encode(&name)?;
        }
        Ok(map)
    }

    #[must_use]
    pub fn scheme(&self) -> &dyn ToolNameScheme {
        &*self.scheme
    }

    /// Returns the wire name for a canonical name, allocating it if needed.
    /// History names are encoded but not added to the decodable current set.
    pub fn encode(&mut self, name: &InvokableName) -> Result<WireToolName, ToolNameError> {
        if let Some(wire) = self.forward.get(name) {
            return Ok(wire.clone());
        }

        let base = self.scheme.encode(name)?;
        let mut wire = base;
        let mut lossy = self.scheme.is_lossy(name, &wire);

        if self.reverse.contains_key(&wire) {
            let suffix = hash_suffix(name.as_str());
            let max_length = self.scheme.max_length();
            let keep = max_length.saturating_sub(suffix.len() + 1);
            if keep == 0 {
                return Err(ToolNameError::TooLong {
                    name: name.to_string(),
                    max_length,
                });
            }
            let readable = wire.as_str().chars().take(keep).collect::<String>();
            wire = WireToolName::new(format!("{readable}_{suffix}"));
            lossy = true;
            if self.reverse.contains_key(&wire) {
                return Err(ToolNameError::Collision {
                    name: name.to_string(),
                });
            }
        }

        self.scheme.validate(&wire)?;
        self.forward.insert(name.clone(), wire.clone());
        self.reverse.insert(wire.clone(), name.clone());
        if lossy {
            self.warn(name, &wire);
        }
        Ok(wire)
    }

    /// Decodes a model tool call. Only names in the current model-invokable set
    /// are decodable; history-only names return `None`.
    #[must_use]
    pub fn decode(&self, wire: &WireToolName) -> Option<&InvokableName> {
        let name = self.reverse.get(wire)?;
        self.current.contains(name).then_some(name)
    }

    #[must_use]
    pub fn get(&self, name: &InvokableName) -> Option<&WireToolName> {
        self.forward.get(name)
    }

    fn warn(&self, canonical: &InvokableName, wire: &WireToolName) {
        static WARNED: OnceLock<Mutex<HashSet<(String, String, String)>>> = OnceLock::new();
        let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
        let key = (
            self.context.endpoint.clone(),
            self.context.model.clone(),
            canonical.to_string(),
        );
        if !warned
            .lock()
            .expect("tool name warning lock poisoned")
            .insert(key)
        {
            return;
        }
        tracing::warn!(
            endpoint = %self.context.endpoint,
            model = %self.context.model,
            surface = %self.context.surface,
            canonical = %canonical,
            wire = %wire,
            "tool name rewritten lossily"
        );
    }
}

/// A naming failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolNameError {
    #[error("tool name is empty")]
    Empty,
    #[error("tool name `{name}` cannot fit within {max_length} characters")]
    TooLong { name: String, max_length: usize },
    #[error("tool name `{name}` does not match the configured pattern")]
    PatternMismatch { name: String },
    #[error("invalid tool name pattern `{pattern}`")]
    InvalidPattern { pattern: String },
    #[error("custom tool name scheme requires a pattern")]
    MissingPattern,
    #[error("tool name collision for `{name}`")]
    Collision { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone)]
    struct SharedBytes(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock poisoned")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn name(value: &str) -> InvokableName {
        InvokableName::new(value)
    }

    #[test]
    fn lossy_warnings_are_deduplicated() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = SharedBytes(Arc::clone(&bytes));
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false),
        );

        tracing::subscriber::with_default(subscriber, || {
            let context = ToolNameContext::new("t010", "dedupe", "openai-chat-completions");
            for _ in 0..2 {
                let scheme = Arc::new(StrictToolNameScheme::new(12));
                let mut map = ToolNameMap::build(
                    scheme,
                    context.clone(),
                    [name("mcp.t010.lossy")],
                )
                .unwrap();
                map.encode(&name("mcp.t010.lossy")).unwrap();
            }
        });

        let logs = String::from_utf8(bytes.lock().expect("capture lock poisoned").clone()).unwrap();
        assert_eq!(
            logs.matches("tool name rewritten lossily").count(),
            1,
            "{logs}"
        );
    }

    fn context() -> ToolNameContext {
        ToolNameContext::new("test", "model", "openai-chat-completions")
    }

    #[test]
    fn strict_renders_namespace_dots_readably() {
        let scheme = StrictToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.search_issues")).unwrap().as_str(),
            "mcp__github__search_issues"
        );
    }

    #[test]
    fn strict_preserves_literal_underscores() {
        let scheme = StrictToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.search__issues")).unwrap().as_str(),
            "mcp__github__search__issues"
        );
    }

    #[test]
    fn strict_sanitizes_non_ascii() {
        let scheme = StrictToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.café")).unwrap().as_str(),
            "mcp__github__caf_"
        );
    }

    #[test]
    fn empty_name_fails() {
        let scheme = StrictToolNameScheme::default();
        assert_eq!(scheme.encode(&name("")), Err(ToolNameError::Empty));
    }

    #[test]
    fn impossible_limit_fails() {
        let scheme = StrictToolNameScheme::new(4);
        assert!(matches!(
            scheme.encode(&name("mcp.github.search_issues")),
            Err(ToolNameError::TooLong { .. })
        ));
    }

    #[test]
    fn truncation_keeps_a_readable_prefix_and_hash() {
        let scheme = StrictToolNameScheme::new(16);
        let wire = scheme.encode(&name("mcp.github.search_issues")).unwrap();
        assert!(wire.as_str().len() <= 16);
        assert!(wire.as_str().starts_with("mcp__"));
        assert!(scheme.is_lossy(&name("mcp.github.search_issues"), &wire));
    }

    #[test]
    fn collision_gets_a_distinct_hash_suffix() {
        let scheme = Arc::new(StrictToolNameScheme::default());
        let mut map = ToolNameMap::build(
            scheme,
            context(),
            [name("mcp.a.b-c"), name("mcp.a.b_c")],
        )
        .unwrap();
        let first = map.encode(&name("mcp.a.b-c")).unwrap();
        let second = map.encode(&name("mcp.a.b_c")).unwrap();
        assert_ne!(first, second);
        assert_eq!(map.decode(&first).unwrap().as_str(), "mcp.a.b-c");
        assert_eq!(map.decode(&second).unwrap().as_str(), "mcp.a.b_c");
    }

    #[test]
    fn ambiguous_double_underscore_is_resolved_by_the_map() {
        let scheme = Arc::new(StrictToolNameScheme::default());
        let mut map = ToolNameMap::build(
            scheme,
            context(),
            [name("mcp.github.search__issues"), name("mcp.github.search.issues")],
        )
        .unwrap();
        let literal = map.encode(&name("mcp.github.search__issues")).unwrap();
        let namespace = map.encode(&name("mcp.github.search.issues")).unwrap();
        assert_ne!(literal, namespace);
        assert_eq!(
            map.decode(&literal).unwrap().as_str(),
            "mcp.github.search__issues"
        );
        assert_eq!(
            map.decode(&namespace).unwrap().as_str(),
            "mcp.github.search.issues"
        );
    }

    #[test]
    fn history_names_encode_but_do_not_decode() {
        let scheme = Arc::new(StrictToolNameScheme::default());
        let mut map = ToolNameMap::build(scheme, context(), [name("mcp.current.tool")]).unwrap();
        let history = map.encode(&name("mcp.history.tool")).unwrap();
        assert_eq!(map.decode(&history), None);
        assert!(map.get(&name("mcp.history.tool")).is_some());
    }

    #[test]
    fn gemini_preserves_dots_and_initial_letter() {
        let scheme = GeminiToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.search")).unwrap().as_str(),
            "mcp.github.search"
        );
        assert_eq!(
            scheme.encode(&name("1mcp.github.search")).unwrap().as_str(),
            "_1mcp.github.search"
        );
    }

    #[test]
    fn meta_flattens_dots() {
        let scheme = MetaToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.search")).unwrap().as_str(),
            "mcp__github__search"
        );
    }

    #[test]
    fn cohere_rejects_leading_digits_and_hyphens() {
        let scheme = CohereToolNameScheme::default();
        assert_eq!(
            scheme.encode(&name("mcp.github.search-tool")).unwrap().as_str(),
            "mcp__github__search_tool"
        );
        assert_eq!(
            scheme.encode(&name("1mcp.github.search")).unwrap().as_str(),
            "_1mcp__github__search"
        );
    }

    #[test]
    fn custom_pattern_validates_the_strict_rendering() {
        let scheme = CustomToolNameScheme::new("^[a-zA-Z0-9_]+$", 64).unwrap();
        assert!(scheme.encode(&name("mcp.github.search")).is_ok());
        let reject = CustomToolNameScheme::new("^[a-z]+$", 64).unwrap();
        assert!(matches!(
            reject.encode(&name("mcp.github.search")),
            Err(ToolNameError::PatternMismatch { .. })
        ));
    }

    #[test]
    fn invalid_custom_pattern_fails() {
        assert!(matches!(
            CustomToolNameScheme::new("(", 64),
            Err(ToolNameError::InvalidPattern { .. })
        ));
    }

    #[test]
    fn custom_collision_suffix_is_validated() {
        let scheme = Arc::new(CustomToolNameScheme::new("^[a-z_]+$", 64).unwrap());
        let mut map = ToolNameMap::new(scheme, context());
        assert!(map.encode(&name("mcp.a.b-c")).is_ok());
        assert!(matches!(
            map.encode(&name("mcp.a.b_c")),
            Err(ToolNameError::PatternMismatch { .. })
        ));
    }
}
