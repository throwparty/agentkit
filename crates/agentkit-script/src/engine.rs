use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rhai::packages::Package;
use rhai::{AST, Dynamic, EvalAltResult, Position, Scope};
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

/// A sandboxed Rhai engine with no ambient host capabilities.
///
/// The engine is built from [`rhai::Engine::new_raw`] plus the standard package,
/// so scripts get arrays, maps, strings and operators but no file module
/// resolver, no filesystem access and no network access. No host functions are
/// registered unless the host calls [`ScriptEngine::register_host_fn`], so by
/// default the only API a script sees is what its compiled JSON arguments and
/// return value provide. `eval`, `import` and `export` are disabled, and every
/// limit in [`Limits`] is enforced.
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
        engine.set_max_variables(limits.max_variables);
        engine.set_max_functions(limits.max_functions);
        engine.set_max_string_size(limits.max_string_size);
        engine.set_max_array_size(limits.max_array_size);
        engine.set_max_map_size(limits.max_map_size);
        engine.set_max_strings_interned(limits.max_strings_interned);
        engine.on_progress(|_| {
            let expired = DEADLINE.with(|deadline| {
                deadline
                    .get()
                    .is_some_and(|deadline| Instant::now() > deadline)
            });
            expired.then(|| Dynamic::from(TIMEOUT_TOKEN))
        });

        // Stdout hygiene: print and debug never touch stdout, so a script can
        // never corrupt a host's standard output stream.
        engine.on_print(|text| eprintln!("{text}"));
        engine.on_debug(|text, _source, _position| eprintln!("{text}"));

        Self { engine, limits }
    }

    /// The limits this engine enforces.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Redirect `print` and `debug` output.
    ///
    /// By default both channels are written to stderr; this replaces that sink,
    /// primarily so a host (or a test) can capture script output.
    pub fn set_output_handler<F>(&mut self, handler: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let debug = Arc::clone(&handler);
        self.engine.on_print(move |text| (*handler)(text));
        self.engine
            .on_debug(move |text, _source, _position| (*debug)(text));
    }

    /// Register a host function callable from scripts.
    ///
    /// `function` receives its arguments as JSON and returns a JSON value, or an
    /// error message that becomes a Rhai runtime error. Only arities 0 to 3 are
    /// supported. Host functions are how a script reaches host capabilities; a
    /// [`ScriptEngine`] built without any registrations physically exposes none,
    /// which is what makes policy and behaviour engines separable by
    /// construction.
    pub fn register_host_fn<F>(
        &mut self,
        name: &str,
        arity: usize,
        function: F,
    ) -> Result<(), ScriptError>
    where
        F: Fn(&[Value]) -> Result<Value, String> + Send + Sync + 'static,
    {
        let function = Arc::new(function);
        match arity {
            0 => {
                self.engine
                    .register_fn(name, move || host_call(&*function, Vec::new()));
            }
            1 => {
                self.engine
                    .register_fn(name, move |a: Dynamic| host_call(&*function, vec![a]));
            }
            2 => {
                self.engine
                    .register_fn(name, move |a: Dynamic, b: Dynamic| {
                        host_call(&*function, vec![a, b])
                    });
            }
            3 => {
                self.engine
                    .register_fn(name, move |a: Dynamic, b: Dynamic, c: Dynamic| {
                        host_call(&*function, vec![a, b, c])
                    });
            }
            other => {
                return Err(ScriptError::Compile {
                    origin: crate::INLINE_ORIGIN.to_string(),
                    message: format!("host functions with arity {other} are not supported"),
                });
            }
        }
        Ok(())
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
        if source.len() > self.limits.max_script_size {
            return Err(ScriptError::TooLarge {
                origin,
                limit: self.limits.max_script_size,
            });
        }
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

    /// Call `function` on `script` with a JSON input, returning a JSON object.
    ///
    /// `function` must be a global function taking a single argument. JSON is
    /// converted to Rhai values on the way in and back again on the way out, so
    /// scripts have no access to host types. A script that returns anything
    /// other than a JSON object fails with [`ScriptError::InvalidReturn`]; use
    /// [`ScriptEngine::call_value`] when any JSON return is acceptable.
    ///
    /// Evaluation is panic-contained: a panicking script or host function is a
    /// [`ScriptError::Panicked`], never a crash. A panic unwinds through Rhai
    /// and may leave the engine inconsistent, so a shared engine must treat a
    /// panic as terminal.
    pub fn call(
        &self,
        script: &CompiledScript,
        function: &str,
        input: &Value,
    ) -> Result<Value, ScriptError> {
        let origin = script.origin_owned();
        let value = self.call_value(script, function, input)?;
        if !value.is_object() {
            return Err(ScriptError::InvalidReturn {
                origin,
                message: "returned a non-map value".to_string(),
            });
        }
        Ok(value)
    }

    /// Call `function` on `script` with a JSON input, accepting any JSON return.
    ///
    /// Like [`ScriptEngine::call`] but does not require the return value to be a
    /// JSON object, so scripts may return `()`, a scalar or an array.
    pub fn call_value(
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
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.call_dynamic(script, function, argument)
        }));
        DEADLINE.with(|deadline| deadline.set(previous));

        match outcome {
            Ok(Ok(value)) => {
                rhai::serde::from_dynamic::<Value>(&value.flatten()).map_err(|error| {
                    ScriptError::InvalidReturn {
                        origin: origin.clone(),
                        message: error.to_string(),
                    }
                })
            }
            Ok(Err(error)) => Err(map_error(&origin, *error, self.limits.timeout)),
            Err(panic) => Err(ScriptError::Panicked {
                origin,
                message: panic_message(panic.as_ref()),
            }),
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

/// Bridge a native host function: convert Rhai arguments to JSON, invoke the
/// function and convert its JSON result back.
fn host_call<F>(function: &F, arguments: Vec<Dynamic>) -> Result<Dynamic, Box<EvalAltResult>>
where
    F: Fn(&[Value]) -> Result<Value, String>,
{
    let arguments = arguments
        .iter()
        .map(rhai::serde::from_dynamic::<Value>)
        .collect::<Result<Vec<Value>, _>>()?;
    let value = function(&arguments)
        .map_err(|message| Box::new(EvalAltResult::ErrorRuntime(message.into(), Position::NONE)))?;
    rhai::serde::to_dynamic(value)
}

/// Recover a human-readable message from a caught panic payload.
fn panic_message(panic: &dyn std::any::Any) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INLINE_ORIGIN;
    use serde_json::json;
    use std::sync::Mutex;

    fn engine() -> ScriptEngine {
        ScriptEngine::new()
    }

    fn compile(engine: &ScriptEngine, source: &str) -> CompiledScript {
        engine.compile(INLINE_ORIGIN, source).unwrap()
    }

    #[test]
    fn call_round_trips_a_json_map() {
        let engine = engine();
        let script = compile(
            &engine,
            "fn mutate(request) { request.headers[\"x-added\"] = \"yes\"; request }",
        );
        let input = json!({"headers": {}, "body": {"model": "gpt-4o"}});

        let output = engine.call(&script, "mutate", &input).unwrap();

        assert_eq!(output["headers"]["x-added"], "yes");
        assert_eq!(output["body"]["model"], "gpt-4o");
    }

    #[test]
    fn call_converts_json_null_to_unit() {
        let engine = engine();
        let script = compile(
            &engine,
            "fn mutate(request) { let r = #{}; r[\"was_unit\"] = (request == ()); r }",
        );

        let output = engine.call(&script, "mutate", &Value::Null).unwrap();

        assert_eq!(output["was_unit"], true);
    }

    #[test]
    fn call_rejects_a_missing_function() {
        let engine = engine();
        let script = compile(&engine, "fn other() { 1 }");

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(matches!(error, ScriptError::MissingFunction { .. }));
    }

    #[test]
    fn call_reports_runtime_errors() {
        let engine = engine();
        let script = compile(&engine, "fn mutate(request) { undefined_function() }");

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(matches!(error, ScriptError::Runtime { .. }), "{error:?}");
    }

    #[test]
    fn an_infinite_loop_hits_the_operation_limit() {
        let engine = engine();
        let script = compile(&engine, "fn mutate(request) { loop {} }");

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(
            matches!(error, ScriptError::LimitExceeded { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_slow_script_hits_the_timeout() {
        let engine = ScriptEngine::with_limits(Limits {
            max_operations: u64::MAX,
            timeout: std::time::Duration::from_millis(20),
            ..Limits::default()
        });
        let script = compile(&engine, "fn mutate(request) { loop {} }");

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(matches!(error, ScriptError::Timeout { .. }), "{error:?}");
    }

    #[test]
    fn an_oversized_string_is_rejected() {
        let engine = ScriptEngine::with_limits(Limits {
            max_string_size: 16,
            ..Limits::default()
        });
        let script = compile(
            &engine,
            "fn mutate(request) { let s = \"aaaa\"; for i in 0..20 { s += \"aaaabbbb\"; } #{ \"value\": s } }",
        );

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(
            matches!(
                &error,
                ScriptError::LimitExceeded { limit, .. } if *limit == "max_string_size"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn an_oversized_array_is_rejected() {
        let engine = ScriptEngine::with_limits(Limits {
            max_array_size: 8,
            ..Limits::default()
        });
        let script = compile(
            &engine,
            "fn mutate(request) { let a = []; for i in 0..100 { a.push(i); } a }",
        );

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(
            matches!(
                &error,
                ScriptError::LimitExceeded { limit, .. } if *limit == "max_array_size"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn an_oversized_map_is_rejected() {
        let engine = ScriptEngine::with_limits(Limits {
            max_map_size: 8,
            ..Limits::default()
        });
        let script = compile(
            &engine,
            "fn mutate(request) { let a = #{}; let b = #{}; for i in 0..100 { b[\"k\" + i.to_string()] = i; } a.mixin(b); a }",
        );
        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(
            matches!(
                &error,
                ScriptError::LimitExceeded { limit, .. } if *limit == "max_map_size"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn eval_is_rejected_at_compile_time() {
        let engine = engine();

        let error = engine
            .compile(INLINE_ORIGIN, "fn mutate(request) { eval(\"1 + 1\") }")
            .unwrap_err();

        assert!(matches!(error, ScriptError::Compile { .. }), "{error:?}");
    }

    #[test]
    fn import_is_rejected_at_compile_time() {
        let engine = engine();

        let error = engine
            .compile(
                INLINE_ORIGIN,
                "import \"some/module\" as m; fn mutate(request) { 1 }",
            )
            .unwrap_err();

        assert!(matches!(error, ScriptError::Compile { .. }), "{error:?}");
    }

    #[test]
    fn a_script_cannot_reach_the_filesystem_or_the_network() {
        let engine = engine();

        for source in [
            "import \"/etc/passwd\"; fn mutate(request) { 1 }",
            "fn mutate(request) { eval(\"import \\\"/etc/passwd\\\";\") }",
        ] {
            let error = engine.compile(INLINE_ORIGIN, source).unwrap_err();
            assert!(
                matches!(error, ScriptError::Compile { .. }),
                "{source} must not reach the host: {error:?}"
            );
        }
    }

    #[test]
    fn a_non_map_return_is_reported() {
        let engine = engine();
        let script = compile(&engine, "fn mutate(request) { 42 }");

        let error = engine.call(&script, "mutate", &json!({})).unwrap_err();

        assert!(
            matches!(error, ScriptError::InvalidReturn { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn syntax_errors_are_compile_errors() {
        let engine = engine();

        let error = engine
            .compile(INLINE_ORIGIN, "fn mutate(request) {")
            .unwrap_err();

        assert!(matches!(error, ScriptError::Compile { .. }), "{error:?}");
    }

    #[test]
    fn a_missing_file_is_a_read_error() {
        let engine = engine();

        let error = engine
            .compile_file(std::path::Path::new("/nonexistent/nowhere.rhai"))
            .unwrap_err();

        match error {
            ScriptError::Read { path, .. } => {
                assert_eq!(path, std::path::Path::new("/nonexistent/nowhere.rhai"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn compile_file_uses_the_path_as_the_origin() {
        let dir = std::env::temp_dir().join(format!("agentkit-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mutate.rhai");
        std::fs::write(&path, "fn mutate(request) { request }").unwrap();

        let script = engine().compile_file(&path).unwrap();
        assert_eq!(script.origin(), path.display().to_string());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_oversized_script_is_rejected_before_compiling() {
        let engine = ScriptEngine::with_limits(Limits {
            max_script_size: 16,
            ..Limits::default()
        });

        let error = engine
            .compile(INLINE_ORIGIN, "fn mutate(request) { request }")
            .unwrap_err();

        assert!(
            matches!(error, ScriptError::TooLarge { limit: 16, .. }),
            "{error:?}"
        );
    }

    #[test]
    fn call_value_accepts_non_map_returns() {
        let engine = engine();
        let script = compile(&engine, "fn mutate(request) { 42 }");

        let value = engine.call_value(&script, "mutate", &json!({})).unwrap();
        assert_eq!(value, json!(42));

        assert!(matches!(
            engine.call(&script, "mutate", &json!({})).unwrap_err(),
            ScriptError::InvalidReturn { .. }
        ));
    }

    #[test]
    fn host_functions_are_callable_with_json_arguments() {
        let mut engine = engine();
        engine
            .register_host_fn("add", 2, |args| {
                let a = args[0].as_i64().unwrap();
                let b = args[1].as_i64().unwrap();
                Ok(json!(a + b))
            })
            .unwrap();
        let script = compile(&engine, "fn run(request) { #{ total: add(2, 3) } }");

        let value = engine.call(&script, "run", &json!({})).unwrap();

        assert_eq!(value["total"], json!(5));
    }

    #[test]
    fn host_function_errors_become_runtime_errors() {
        let mut engine = engine();
        engine
            .register_host_fn("explode", 0, |_| Err("nope".to_string()))
            .unwrap();
        let script = compile(&engine, "fn run(request) { explode() }");

        let error = engine.call(&script, "run", &json!({})).unwrap_err();

        assert!(
            matches!(error, ScriptError::Runtime { ref message, .. } if message.contains("nope")),
            "{error:?}"
        );
    }

    #[test]
    fn panicking_host_functions_are_contained() {
        let mut engine = engine();
        engine
            .register_host_fn("boom", 0, |_| panic!("kaboom"))
            .unwrap();
        let script = compile(&engine, "fn run(request) { boom() }");

        let error = engine.call(&script, "run", &json!({})).unwrap_err();

        assert!(
            matches!(error, ScriptError::Panicked { ref message, .. } if message.contains("kaboom")),
            "{error:?}"
        );
    }

    #[test]
    fn print_and_debug_are_routed_to_the_output_handler() {
        let mut engine = engine();
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        engine.set_output_handler(move |text| sink.lock().unwrap().push(text.to_owned()));
        let script = compile(
            &engine,
            r#"fn run(request) { print("printed"); debug("debugged"); #{} }"#,
        );

        engine.call(&script, "run", &json!({})).unwrap();

        // Rhai's debug channel presents strings quoted.
        assert_eq!(
            &*lines.lock().unwrap(),
            &["printed".to_owned(), "\"debugged\"".to_owned()]
        );
    }
}
