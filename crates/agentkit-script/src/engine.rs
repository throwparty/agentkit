use std::cell::Cell;
use std::path::Path;
use std::time::Instant;

use rhai::{Dynamic, EvalAltResult, Scope, AST};
use rhai::packages::Package;
use serde_json::Value;

use crate::error::ScriptError;
use crate::limits::Limits;
use crate::script::CompiledScript;

/// Token returned from the progress callback once the wall-clock deadline has
/// passed. Its presence in an [`EvalAltResult::ErrorTerminated`] is what
/// distinguishes a timeout from any other termination.
const TIMEOUT_TOKEN: &str = "agentkit-script:timeout";

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// A sandboxed Rhai engine with no host capabilities.
///
/// The engine is built from [`rhai::Engine::new_raw`] plus the standard package,
/// so scripts get arrays, maps, strings and operators but no file module
/// resolver, no filesystem access and no network access. No host functions are
/// registered, so the only API a script sees is what its compiled JSON
/// arguments and return value provide. `eval`, `import` and `export` are
/// disabled, and every limit in [`Limits`] is enforced.
///
/// `ScriptEngine` is `Send + Sync` and intended to be shared as an
/// `Arc<ScriptEngine>`.
pub struct ScriptEngine {
    engine: rhai::Engine,
    limits: Limits,
}

impl std::fmt::Debug for ScriptEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptEngine")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptEngine {
    /// Create an engine with [`Limits::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// Create an engine with the supplied limits.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        let mut engine = rhai::Engine::new_raw();
        rhai::packages::StandardPackage::new().register_into_engine(&mut engine);
        engine.disable_symbol("eval");
        engine.disable_symbol("import");
        engine.disable_symbol("export");
        engine.set_max_operations(limits.max_operations);
        engine.set_max_call_levels(limits.max_call_levels);
        engine.set_max_expr_depths(limits.max_expr_depth, limits.max_function_expr_depth);
        engine.set_max_string_size(limits.max_string_size);
        engine.set_max_array_size(limits.max_array_size);
        engine.set_max_map_size(limits.max_map_size);
        engine.set_max_strings_interned(limits.max_strings_interned);
        engine.on_progress(|_| {
            let expired = DEADLINE
                .with(|deadline| deadline.get().is_some_and(|deadline| Instant::now() > deadline));
            expired.then(|| Dynamic::from(TIMEOUT_TOKEN))
        });

        Self { engine, limits }
    }

    /// The limits this engine enforces.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Compile `source` under `origin`.
    ///
    /// `origin` names the script in error messages; use a file path so failures
    /// can be traced back to the file the operator configured.
    pub fn compile(
        &self,
        origin: impl Into<String>,
        source: &str,
    ) -> Result<CompiledScript, ScriptError> {
        let origin = origin.into();
        self.engine
            .compile(source)
            .map(|ast| CompiledScript::new(std::sync::Arc::from(origin.as_str()), ast))
            .map_err(|error| ScriptError::Compile {
                origin,
                message: error.to_string(),
            })
    }

    /// Read and compile the script at `path`.
    pub fn compile_file(&self, path: &Path) -> Result<CompiledScript, ScriptError> {
        let source = std::fs::read_to_string(path).map_err(|source| ScriptError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        self.compile(path.display().to_string(), &source)
    }

    /// Call `function` on `script` with a JSON input, returning a JSON value.
    ///
    /// `function` must be a global function taking a single argument. JSON is
    /// converted to Rhai values on the way in and back again on the way out, so
    /// scripts have no access to host types. A script that returns anything
    /// other than a JSON object fails with [`ScriptError::InvalidReturn`].
    pub fn call(
        &self,
        script: &CompiledScript,
        function: &str,
        input: &Value,
    ) -> Result<Value, ScriptError> {
        let origin = script.origin_owned();
        if !script.defines(function, 1) {
            return Err(ScriptError::MissingFunction {
                origin,
                function: function.to_string(),
                arity: 1,
            });
        }

        let argument = rhai::serde::to_dynamic(input).map_err(|error| ScriptError::Runtime {
            origin: origin.clone(),
            message: format!("cannot convert input: {error}"),
        })?;

        let previous = DEADLINE.with(|deadline| {
            let previous = deadline.get();
            deadline.set(Some(Instant::now() + self.limits.timeout));
            previous
        });
        let outcome = self.call_dynamic(script, function, argument);
        DEADLINE.with(|deadline| deadline.set(previous));

        match outcome {
            Ok(value) => {
                let value: Value = rhai::serde::from_dynamic(&value.flatten()).map_err(|error| {
                    ScriptError::InvalidReturn {
                        origin: origin.clone(),
                        message: error.to_string(),
                    }
                })?;
                if !value.is_object() {
                    return Err(ScriptError::InvalidReturn {
                        origin,
                        message: "returned a non-map value".to_string(),
                    });
                }
                Ok(value)
            }
            Err(error) => Err(map_error(&origin, *error, self.limits.timeout)),
        }
    }

    fn call_dynamic(
        &self,
        script: &CompiledScript,
        function: &str,
        argument: Dynamic,
    ) -> Result<Dynamic, Box<EvalAltResult>> {
        let ast: &AST = script.ast();
        let mut scope = Scope::new();
        self.engine
            .call_fn::<Dynamic>(&mut scope, ast, function, (argument,))
    }
}

/// Translate a Rhai evaluation error into a [`ScriptError`].
fn map_error(origin: &str, error: EvalAltResult, timeout: std::time::Duration) -> ScriptError {
    match error {
        EvalAltResult::ErrorTooManyOperations(..) => ScriptError::LimitExceeded {
            origin: origin.to_string(),
            limit: "max_operations",
        },
        EvalAltResult::ErrorDataTooLarge(kind, ..) => ScriptError::LimitExceeded {
            origin: origin.to_string(),
            limit: data_limit(&kind),
        },
        EvalAltResult::ErrorStackOverflow(..) => ScriptError::LimitExceeded {
            origin: origin.to_string(),
            limit: "max_call_levels",
        },
        EvalAltResult::ErrorTerminated(ref token, ..) if is_timeout_token(token) => {
            ScriptError::Timeout {
                origin: origin.to_string(),
                timeout,
            }
        }
        other => ScriptError::Runtime {
            origin: origin.to_string(),
            message: other.to_string(),
        },
    }
}

fn is_timeout_token(token: &Dynamic) -> bool {
    token.clone().into_string().ok().as_deref() == Some(TIMEOUT_TOKEN)
}

/// Name the [`Limits`] field a data-size violation belongs to.
fn data_limit(kind: &str) -> &'static str {
    match kind {
        "Length of string" => "max_string_size",
        "Size of array/BLOB" => "max_array_size",
        "Size of object map" => "max_map_size",
        _ => "max_data_size",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INLINE_ORIGIN;

    #[test]
    fn compile_records_the_origin_and_defines_functions() {
        let engine = ScriptEngine::new();

        let script = engine
            .compile(INLINE_ORIGIN, "fn mutate(request) { request }")
            .unwrap();

        assert_eq!(script.origin(), INLINE_ORIGIN);
        assert!(script.defines("mutate", 1));
        assert!(!script.defines("mutate", 2));
    }

    #[test]
    fn eval_import_and_export_are_disabled() {
        let engine = ScriptEngine::new();

        for source in [
            "fn mutate(request) { eval(\"1 + 1\") }",
            "import \"some/module\" as m; fn mutate(request) { 1 }",
        ] {
            let error = engine.compile(INLINE_ORIGIN, source).unwrap_err();
            assert!(
                matches!(error, ScriptError::Compile { .. }),
                "{source} should fail to compile: {error:?}"
            );
        }
    }

    #[test]
    fn a_missing_file_is_a_read_error() {
        let engine = ScriptEngine::new();

        let error = engine
            .compile_file(std::path::Path::new("/nonexistent/nowhere.rhai"))
            .unwrap_err();

        assert!(matches!(error, ScriptError::Read { .. }), "{error:?}");
    }

    #[test]
    fn compile_file_uses_the_path_as_the_origin() {
        let dir = std::env::temp_dir().join(format!("agentkit-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mutate.rhai");
        std::fs::write(&path, "fn mutate(request) { request }").unwrap();

        let script = ScriptEngine::new().compile_file(&path).unwrap();
        assert_eq!(script.origin(), path.display().to_string());

        std::fs::remove_dir_all(&dir).ok();
    }
}
