use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};

use crate::config::{ApiSurface, ProviderConfig};
use agentkit_script::{CompiledScript, ScriptEngine, ScriptError};

/// The function every mutation script must define.
const MUTATE_FUNCTION: &str = "mutate";

/// The compiled mutation scripts attached to a single provider.
///
/// Built once at startup and shared by reference for the lifetime of the
/// process; scripts are never re-read per request.
#[derive(Debug)]
pub struct MutationSet {
    engine: Arc<ScriptEngine>,
    scripts: Vec<CompiledScript>,
}

impl MutationSet {
    pub(crate) fn new(engine: Arc<ScriptEngine>, scripts: Vec<CompiledScript>) -> Self {
        Self { engine, scripts }
    }

    /// Run every script in declared order against the assembled outbound
    /// request.
    ///
    /// `headers` and `body` are read to build the request object, then replaced
    /// wholesale by whatever the scripts leave behind. Any failure fails the
    /// request: nothing is forwarded unmutated.
    pub fn apply(
        &self,
        context: &MutationContext,
        headers: &mut HeaderMap,
        body: &mut Vec<u8>,
    ) -> Result<(), MutationError> {
        if self.scripts.is_empty() {
            return Ok(());
        }

        let parsed = serde_json::from_slice::<Value>(body).ok();
        let body_is_json = parsed.is_some();
        let mutable = parsed.unwrap_or(Value::Null);

        let mut request = json!({
            "provider": context.provider_identity,
            "surface": context.surface.to_string(),
            "model": context.model,
            "session_id": context.session_id,
            "headers": headers_to_json(headers),
            "body": mutable,
        });

        let mut origin = String::from(agentkit_script::INLINE_ORIGIN);
        for script in &self.scripts {
            origin = script.origin().to_string();
            request = self
                .engine
                .call(script, MUTATE_FUNCTION, &request)
                .map_err(|error| MutationError::Script {
                    origin: origin.clone(),
                    error,
                })?;
        }

        let mutated = request
            .get("headers")
            .and_then(Value::as_object)
            .ok_or_else(|| MutationError::Headers {
                origin: origin.clone(),
                message: "the script returned no usable `headers` map".to_string(),
            })?;
        *headers = apply_headers(&origin, mutated)?;

        match request.get("body") {
            None => {}
            Some(Value::Null) if !body_is_json => {}
            Some(value) => {
                let serialised =
                    serde_json::to_vec(value).map_err(|error| MutationError::Body {
                        origin,
                        message: error.to_string(),
                    })?;
                *body = serialised;
            }
        }

        Ok(())
    }
}

/// Read-only request context handed to mutation scripts.
#[derive(Debug, Clone)]
pub struct MutationContext {
    pub provider_identity: String,
    pub surface: ApiSurface,
    pub model: String,
    pub session_id: Option<String>,
}

/// A mutation failed and the request must not be forwarded.
#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("provider '{provider}': {error}")]
    Provider {
        provider: String,
        #[source]
        error: Box<MutationError>,
    },
    #[error("mutation script '{origin}' failed: {error}")]
    Script {
        origin: String,
        #[source]
        error: ScriptError,
    },
    #[error("mutation script '{origin}' produced an invalid header ({name}): {message}")]
    InvalidHeader {
        origin: String,
        name: String,
        message: String,
    },
    #[error("mutation script '{origin}' produced unusable headers: {message}")]
    Headers { origin: String, message: String },
    #[error("mutation script '{origin}' produced an unusable body: {message}")]
    Body { origin: String, message: String },
}

/// Compile every provider's mutation scripts once.
///
/// A shared script path is compiled a single time and cloned into each
/// provider's set. A missing file, an unparseable script or a script without a
/// global `fn mutate(request)` aborts startup with the provider identity and
/// the script origin.
pub fn compile_provider_mutations(
    engine: &Arc<ScriptEngine>,
    providers: &HashMap<String, ProviderConfig>,
) -> Result<HashMap<String, Arc<MutationSet>>, MutationError> {
    let mut compiled: HashMap<PathBuf, CompiledScript> = HashMap::new();
    let mut sets = HashMap::new();

    for (identity, provider) in providers {
        let mut scripts = Vec::with_capacity(provider.mutations.len());
        for path in &provider.mutations {
            let key = cache_key(path);
            if let Some(script) = compiled.get(&key) {
                scripts.push(script.clone());
                continue;
            }
            let origin = path.display().to_string();
            let script = engine.compile_file(path).map_err(|error| {
                provider_error(
                    identity,
                    MutationError::Script {
                        origin: origin.clone(),
                        error,
                    },
                )
            })?;
            if !script.defines(MUTATE_FUNCTION, 1) {
                return Err(provider_error(
                    identity,
                    MutationError::Script {
                        origin,
                        error: ScriptError::MissingFunction {
                            origin: path.display().to_string(),
                            function: MUTATE_FUNCTION.to_string(),
                            arity: 1,
                        },
                    },
                ));
            }
            scripts.push(script.clone());
            compiled.insert(key, script);
        }
        sets.insert(
            identity.clone(),
            Arc::new(MutationSet::new(engine.clone(), scripts)),
        );
    }

    Ok(sets)
}

/// Replace the outgoing headers wholesale with those the scripts produced.
fn apply_headers(
    origin: &str,
    mutated: &serde_json::Map<String, Value>,
) -> Result<HeaderMap, MutationError> {
    let mut headers = HeaderMap::new();
    for (name, value) in mutated {
        let value = value.as_str().ok_or_else(|| MutationError::InvalidHeader {
            origin: origin.to_string(),
            name: name.clone(),
            message: "header value is not a string".to_string(),
        })?;
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            MutationError::InvalidHeader {
                origin: origin.to_string(),
                name: name.clone(),
                message: error.to_string(),
            }
        })?;
        let value = HeaderValue::from_str(value).map_err(|error| MutationError::InvalidHeader {
            origin: origin.to_string(),
            name: name.as_str().to_string(),
            message: error.to_string(),
        })?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn provider_error(identity: &str, error: MutationError) -> MutationError {
    MutationError::Provider {
        provider: identity.to_string(),
        error: Box::new(error),
    }
}

/// Identify a script by its canonical path so shared scripts compile once.
fn cache_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn headers_to_json(headers: &HeaderMap) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            map.insert(name.as_str().to_string(), Value::String(value.to_string()));
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const TOOLS_SCRIPT: &str = r#"
        fn mutate(request) {
            request.body["tools"] = [#{
                "type": "function",
                "function": #{
                    "name": "bash",
                    "description": "Run a shell command",
                    "parameters": #{
                        "type": "object",
                        "properties": #{ "command": #{ "type": "string" } },
                        "required": ["command"]
                    }
                }
            }];
            request.body["tool_choice"] = "none";
            request
        }
    "#;

    fn engine() -> Arc<ScriptEngine> {
        Arc::new(ScriptEngine::new())
    }

    fn write_script(dir: &Path, name: &str, source: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, source).unwrap();
        path
    }

    fn context() -> MutationContext {
        MutationContext {
            provider_identity: "test_provider".to_string(),
            surface: ApiSurface::OpenaiChatCompletions,
            model: "gpt-4o".to_string(),
            session_id: None,
        }
    }

    fn set(engine: &Arc<ScriptEngine>, sources: &[&str]) -> MutationSet {
        let dir = tempfile::tempdir().unwrap();
        let scripts = sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                engine
                    .compile(
                        write_script(dir.path(), &format!("{index}.rhai"), source)
                            .display()
                            .to_string(),
                        source,
                    )
                    .unwrap()
            })
            .collect();
        MutationSet::new(engine.clone(), scripts)
    }

    #[test]
    fn a_header_and_body_mutation_are_both_applied() {
        let engine = engine();
        let set = set(
            &engine,
            &[
                r#"
                fn mutate(request) {
                    request.headers["x-provider-quirk"] = "on";
                    request
                }
            "#,
                TOOLS_SCRIPT,
            ],
        );

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let mut body = br#"{"model":"gpt-4o","messages":[]}"#.to_vec();

        set.apply(&context(), &mut headers, &mut body).unwrap();

        assert_eq!(headers.get("x-provider-quirk").unwrap(), "on");
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["tool_choice"], "none");
        assert_eq!(parsed["tools"][0]["function"]["name"], "bash");
    }

    #[test]
    fn scripts_run_in_declared_order() {
        let engine = engine();
        let set = set(
            &engine,
            &[
                r#"fn mutate(request) { request.headers["x-order"] = "first"; request }"#,
                r#"fn mutate(request) { request.headers["x-order"] = request.headers["x-order"] + "-second"; request }"#,
            ],
        );

        let mut headers = HeaderMap::new();
        let mut body = Vec::new();

        set.apply(&context(), &mut headers, &mut body).unwrap();

        assert_eq!(headers.get("x-order").unwrap(), "first-second");
    }

    #[test]
    fn an_invalid_header_fails_the_request() {
        let engine = engine();
        let set = set(
            &engine,
            &[r#"fn mutate(request) { request.headers["bad header name!!"] = "value"; request }"#],
        );

        let mut headers = HeaderMap::new();
        let mut body = Vec::new();

        let error = set.apply(&context(), &mut headers, &mut body).unwrap_err();

        assert!(
            matches!(error, MutationError::InvalidHeader { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_non_json_body_is_passed_as_unit() {
        let engine = engine();
        let set = set(
            &engine,
            &[
                r#"fn mutate(request) { request.headers["x-body"] = (if request.body == () { "unit" } else { "json" }); request }"#,
            ],
        );

        let mut headers = HeaderMap::new();
        let mut body = b"not json at all".to_vec();

        set.apply(&context(), &mut headers, &mut body).unwrap();

        assert_eq!(headers.get("x-body").unwrap(), "unit");
        assert_eq!(body, b"not json at all");
    }

    #[test]
    fn a_script_that_mutates_a_unit_body_fails() {
        let engine = engine();
        let set = set(
            &engine,
            &[r#"fn mutate(request) { request.body["tools"] = []; request }"#],
        );

        let mut headers = HeaderMap::new();
        let mut body = b"not json at all".to_vec();

        let error = set.apply(&context(), &mut headers, &mut body).unwrap_err();

        assert!(matches!(error, MutationError::Script { .. }), "{error:?}");
        assert_eq!(body, b"not json at all");
    }

    #[test]
    fn a_script_error_fails_the_request() {
        let engine = engine();
        let set = set(&engine, &["fn mutate(request) { undefined_function() }"]);

        let mut headers = HeaderMap::new();
        let mut body = Vec::new();

        let error = set.apply(&context(), &mut headers, &mut body).unwrap_err();

        assert!(matches!(error, MutationError::Script { .. }), "{error:?}");
    }

    #[test]
    fn a_script_without_mutate_is_rejected_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script(dir.path(), "no-mutate.rhai", "fn other() { 1 }");

        let engine = engine();
        let providers = HashMap::from([(
            "test_provider".to_string(),
            provider("test_provider", vec![path]),
        )]);

        let error = compile_provider_mutations(&engine, &providers).unwrap_err();

        assert!(
            matches!(&error, MutationError::Provider { provider, .. } if provider == "test_provider"),
            "{error:?}"
        );
        assert!(error.to_string().contains("no-mutate.rhai"), "{error:?}");
    }

    #[test]
    fn a_missing_script_aborts_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.rhai");

        let engine = engine();
        let providers = HashMap::from([(
            "test_provider".to_string(),
            provider("test_provider", vec![path]),
        )]);

        let error = compile_provider_mutations(&engine, &providers).unwrap_err();

        assert!(error.to_string().contains("missing.rhai"), "{error:?}");
    }

    #[test]
    fn a_shared_script_is_compiled_once() {
        let dir = tempfile::tempdir().unwrap();
        let shared = write_script(dir.path(), "shared.rhai", "fn mutate(request) { request }");

        let engine = engine();
        let providers = HashMap::from([
            (
                "provider_a".to_string(),
                provider("provider_a", vec![shared.clone()]),
            ),
            (
                "provider_b".to_string(),
                provider("provider_b", vec![shared]),
            ),
        ]);

        let sets = compile_provider_mutations(&engine, &providers).unwrap();

        assert_eq!(sets.len(), 2);
    }

    #[test]
    fn a_provider_without_mutations_gets_an_empty_set() {
        let engine = engine();
        let providers = HashMap::from([("bare".to_string(), provider("bare", Vec::new()))]);

        let sets = compile_provider_mutations(&engine, &providers).unwrap();
        let bare = sets.get("bare").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let mut body = br#"{"model":"gpt-4o"}"#.to_vec();
        bare.apply(
            &MutationContext {
                provider_identity: "bare".to_string(),
                surface: ApiSurface::OpenaiChatCompletions,
                model: "gpt-4o".to_string(),
                session_id: None,
            },
            &mut headers,
            &mut body,
        )
        .unwrap();

        assert_eq!(body, br#"{"model":"gpt-4o"}"#);
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn an_empty_script_set_passes_the_request_through_byte_for_byte() {
        let engine = engine();
        let set = MutationSet::new(engine, Vec::new());

        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("x-opaque", HeaderValue::from_bytes(b"\xff\xfe").unwrap());
        let mut body = br#"{ "model" :   "gpt-4o" }"#.to_vec();

        set.apply(&context(), &mut headers, &mut body).unwrap();

        assert_eq!(body, br#"{ "model" :   "gpt-4o" }"#);
        assert_eq!(headers.get("x-opaque").unwrap().as_bytes(), b"\xff\xfe");
    }

    #[test]
    fn a_script_dropping_the_headers_map_fails_the_request() {
        let engine = engine();
        let set = set(
            &engine,
            &[r#"fn mutate(request) { #{ "body": #{ "x": 1 } } }"#],
        );

        let mut headers = HeaderMap::new();
        let mut body = Vec::new();

        let error = set.apply(&context(), &mut headers, &mut body).unwrap_err();

        assert!(matches!(error, MutationError::Headers { .. }), "{error:?}");
    }

    #[test]
    fn a_non_map_headers_value_fails_the_request() {
        let engine = engine();
        let set = set(
            &engine,
            &[r#"fn mutate(request) { request.headers = []; request }"#],
        );

        let mut headers = HeaderMap::new();
        let mut body = Vec::new();

        let error = set.apply(&context(), &mut headers, &mut body).unwrap_err();

        assert!(matches!(error, MutationError::Headers { .. }), "{error:?}");
    }

    #[test]
    fn a_json_body_cleared_by_a_script_is_serialised_as_null() {
        let engine = engine();
        let set = set(
            &engine,
            &[r#"fn mutate(request) { request.body = (); request }"#],
        );

        let mut headers = HeaderMap::new();
        let mut body = br#"{"model":"gpt-4o"}"#.to_vec();

        set.apply(&context(), &mut headers, &mut body).unwrap();

        assert_eq!(body, b"null");
    }

    fn provider(identity: &str, mutations: Vec<PathBuf>) -> ProviderConfig {
        ProviderConfig {
            identity: identity.to_string(),
            api_surface: ApiSurface::OpenaiChatCompletions,
            base_url: "https://api.example.com/v1".to_string(),
            billing: crate::config::BillingModel::PayAsYouGo,
            auth: crate::auth::AuthConfig {
                r#type: crate::auth::AuthType::None,
                oauth: None,
            },
            pricing: crate::config::PricingConfig {
                input_per_mtok: 0.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
                reasoning_per_mtok: None,
                models: HashMap::new(),
            },
            models: None,
            mutations,
        }
    }
}
