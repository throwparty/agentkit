//! Resolution of the effective tool-name scheme for an endpoint and model.

use super::schema::EndpointConfig;
use crate::invokables::tool_name::{ToolNameError, ToolNameScheme, ToolNameSchemeName};
use std::sync::Arc;

/// A tool-name configuration rejection.
#[derive(Debug, thiserror::Error)]
pub enum ToolNamesConfigError {
    #[error("endpoint `{endpoint}` model `{model}`: `pattern` requires `scheme = custom`")]
    PatternWithoutCustom { endpoint: String, model: String },
    #[error("endpoint `{endpoint}` model `{model}`: `scheme = custom` requires `pattern`")]
    CustomWithoutPattern { endpoint: String, model: String },
    #[error(
        "endpoint `{endpoint}` model `{model}`: scheme `{scheme}` is not supported on `{surface}`"
    )]
    UnsupportedScheme {
        endpoint: String,
        model: String,
        scheme: String,
        surface: String,
    },
    #[error("endpoint `{endpoint}` model `{model}`: {source}")]
    ToolName {
        endpoint: String,
        model: String,
        #[source]
        source: ToolNameError,
    },
}

/// Resolves the scheme from model override, endpoint override, and wire-format
/// default, in that order.
pub fn resolve_tool_names(
    endpoint_name: &str,
    endpoint: &EndpointConfig,
    model: &str,
) -> Result<Arc<dyn ToolNameScheme>, ToolNamesConfigError> {
    let surface = endpoint.wire_format.as_str();
    let config = endpoint
        .models
        .get(model)
        .map(|model_config| &model_config.tool_names)
        .filter(|config| config.is_configured())
        .unwrap_or(&endpoint.tool_names);
    let scheme = config
        .scheme
        .unwrap_or_else(|| ToolNameSchemeName::for_surface(surface));

    if config.pattern.is_some() && scheme != ToolNameSchemeName::Custom {
        return Err(ToolNamesConfigError::PatternWithoutCustom {
            endpoint: endpoint_name.to_owned(),
            model: model.to_owned(),
        });
    }
    if scheme == ToolNameSchemeName::Custom && config.pattern.is_none() {
        return Err(ToolNamesConfigError::CustomWithoutPattern {
            endpoint: endpoint_name.to_owned(),
            model: model.to_owned(),
        });
    }
    if scheme == ToolNameSchemeName::Gemini && surface != "google-generate-content" {
        return Err(ToolNamesConfigError::UnsupportedScheme {
            endpoint: endpoint_name.to_owned(),
            model: model.to_owned(),
            scheme: scheme.as_str().to_owned(),
            surface: surface.to_owned(),
        });
    }

    scheme
        .build(config.pattern.as_deref(), config.max_length)
        .map_err(|source| ToolNamesConfigError::ToolName {
            endpoint: endpoint_name.to_owned(),
            model: model.to_owned(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{Auth, ModelConfig, ModelsConfig, ToolNamesConfig, WireFormat};
    use crate::invokables::tool_name::ToolNameSchemeName;
    use std::collections::BTreeMap;

    fn endpoint(tool_names: ToolNamesConfig, models: BTreeMap<String, ModelConfig>) -> EndpointConfig {
        EndpointConfig {
            base_url: "http://localhost/v1".to_owned(),
            wire_format: WireFormat::OpenaiChatCompletions,
            auth: Auth::None,
            models: ModelsConfig::from_models(models),
            tool_names,
        }
    }

    fn names(scheme: ToolNameSchemeName) -> ToolNamesConfig {
        ToolNamesConfig {
            scheme: Some(scheme),
            max_length: None,
            pattern: None,
        }
    }

    #[test]
    fn default_is_strict_for_openai_chat() {
        let scheme = resolve_tool_names("test", &endpoint(Default::default(), BTreeMap::new()), "m")
            .unwrap();
        assert_eq!(scheme.name(), "strict");
        assert_eq!(scheme.max_length(), 64);
    }

    #[test]
    fn endpoint_override_wins_over_default() {
        let mut config = names(ToolNameSchemeName::Meta);
        config.max_length = Some(32);
        let scheme = resolve_tool_names("test", &endpoint(config, BTreeMap::new()), "m").unwrap();
        assert_eq!(scheme.name(), "meta");
        assert_eq!(scheme.max_length(), 32);
    }

    #[test]
    fn model_override_wins_over_endpoint() {
        let mut tool_names = names(ToolNameSchemeName::Cohere);
        tool_names.max_length = Some(20);
        let models = BTreeMap::from([("m".to_owned(), ModelConfig { tool_names })]);
        let scheme = resolve_tool_names(
            "test",
            &endpoint(names(ToolNameSchemeName::Meta), models),
            "m",
        )
        .unwrap();
        assert_eq!(scheme.name(), "cohere");
        assert_eq!(scheme.max_length(), 20);
    }

    #[test]
    fn pattern_without_custom_is_rejected() {
        let mut config = names(ToolNameSchemeName::Strict);
        config.pattern = Some("^[a-z]+$".to_owned());
        let error = resolve_tool_names("test", &endpoint(config, BTreeMap::new()), "m").unwrap_err();
        assert!(matches!(
            error,
            ToolNamesConfigError::PatternWithoutCustom { .. }
        ));
    }

    #[test]
    fn custom_without_pattern_is_rejected() {
        let error = resolve_tool_names(
            "test",
            &endpoint(names(ToolNameSchemeName::Custom), BTreeMap::new()),
            "m",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ToolNamesConfigError::CustomWithoutPattern { .. }
        ));
    }

    #[test]
    fn gemini_is_rejected_on_openai_chat() {
        let error = resolve_tool_names(
            "test",
            &endpoint(names(ToolNameSchemeName::Gemini), BTreeMap::new()),
            "m",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ToolNamesConfigError::UnsupportedScheme { .. }
        ));
    }

    #[test]
    fn custom_pattern_builds() {
        let mut config = names(ToolNameSchemeName::Custom);
        config.pattern = Some("^[a-zA-Z0-9_]+$".to_owned());
        let scheme = resolve_tool_names("test", &endpoint(config, BTreeMap::new()), "m").unwrap();
        assert_eq!(scheme.name(), "custom");
    }
}
