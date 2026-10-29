//! Tool name rendering for model API surfaces.
//!
//! The registry owns canonical [`InvokableName`]s. A [`ToolNameScheme`]
//! renders those names into [`WireToolName`]s for one API surface, and a
//! [`ToolNameMap`] keeps the per-request mapping needed to decode model tool
//! calls back to canonical names.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

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
        _pattern: Option<&str>,
        max_length: Option<usize>,
    ) -> Result<Arc<dyn ToolNameScheme>, ToolNameError> {
        let max_length = max_length.unwrap_or(DEFAULT_MAX_LENGTH);
        match self {
            Self::Strict => Ok(Arc::new(StrictToolNameScheme::new(max_length))),
            _ => Err(ToolNameError::UnsupportedScheme {
                scheme: self.as_str().to_owned(),
            }),
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
    #[error("tool name scheme `{scheme}` is not implemented")]
    UnsupportedScheme { scheme: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> InvokableName {
        InvokableName::new(value)
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
}
