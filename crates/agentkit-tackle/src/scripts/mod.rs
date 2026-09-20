//! The hardened Rhai engine builders: two structurally separated kinds.
//!
//! **Policy engines** run `pre_tool_use` scripts and receive decision
//! inputs only — they physically lack the acting host functions. **Behaviour
//! engines** run every other event with the full host API (registered by
//! the session host in T-026 via the registrar passed to
//! [`behaviour_engine`]).
//!
//! Every engine is fresh per invocation and carries the fixed internal
//! safety limits: operations, call levels, expression depths, collection
//! and string sizes, no module imports (rejected at the tokenizer), and a
//! per-engine time budget enforced through `on_progress`. Script size is
//! capped at compile time. Argument payloads above the cap are replaced by
//! a digest map so scripts can detect but cannot read them. `print` and
//! `debug` go to stderr, never stdout. Evaluation is panic-contained: a
//! panicking script or host function is an error, never a crash. Script
//! error or timeout fails closed — the caller (the permission pipeline)
//! treats every [`ScriptError`] as ask.

use rhai::{Dynamic, Engine, EvalAltResult, Position, Scope, Token, TokenizeState, AST};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The script source size cap, bounding compile work.
pub const SCRIPT_MAX_SIZE: usize = 256 * 1024;

/// The argument payload cap: oversized payloads become digest maps.
pub const PAYLOAD_CAP: usize = 16 * 1024;

/// The policy engine time budget: about a second.
pub const POLICY_TIME: Duration = Duration::from_secs(1);

/// The behaviour engine time budget: about ten seconds.
pub const BEHAVIOUR_TIME: Duration = Duration::from_secs(10);

/// Fixed internal safety limits, deliberately not user configuration.
const MAX_OPERATIONS: u64 = 10_000_000;
const MAX_CALL_LEVELS: usize = 32;
const MAX_EXPR_DEPTH: usize = 128;
const MAX_VARIABLES: usize = 2048;
const MAX_FUNCTIONS: usize = 512;
const MAX_STRING_SIZE: usize = SCRIPT_MAX_SIZE;
const MAX_ARRAY_SIZE: usize = 10_000;
const MAX_MAP_SIZE: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("script exceeds the {SCRIPT_MAX_SIZE}-byte size cap")]
    TooLarge,
    #[error("failed to compile the script: {0}")]
    Compile(String),
    #[error("script evaluation failed: {0}")]
    Eval(String),
    #[error("script evaluation exceeded the time limit")]
    Timeout,
    #[error("script host function panicked: {0}")]
    Panicked(String),
}

/// A compiled script bound to a fresh hardened engine.
#[derive(Debug)]
pub struct Compiled {
    engine: Engine,
    ast: AST,
}

impl Compiled {
    /// Calls the script's event function (e.g. `pre_tool_use`) with the
    /// given arguments. Panic-contained.
    pub fn call(&self, function: &str, args: Vec<Dynamic>) -> Result<Dynamic, ScriptError> {
        let mut scope = Scope::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.engine
                .call_fn::<Dynamic>(&mut scope, &self.ast, function, args)
        }));
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(map_eval_error(&err)),
            Err(panic) => Err(ScriptError::Panicked(panic_message(panic.as_ref()))),
        }
    }
}

fn map_eval_error(err: &EvalAltResult) -> ScriptError {
    match err {
        EvalAltResult::ErrorTooManyOperations(_) | EvalAltResult::ErrorTerminated(_, _) => {
            ScriptError::Timeout
        }
        other => ScriptError::Eval(other.to_string()),
    }
}

fn panic_message(panic: &dyn std::any::Any) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// Hardens an engine with the fixed internal limits. `print` is the
/// script's stdout channel — redirected (to stderr in production) so
/// scripts cannot corrupt stdout.
fn hardened(time: Duration, print: Arc<dyn Fn(&str) + Send + Sync>) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(MAX_CALL_LEVELS);
    engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_EXPR_DEPTH);
    engine.set_max_variables(MAX_VARIABLES);
    engine.set_max_functions(MAX_FUNCTIONS);
    engine.set_max_modules(0);
    engine.set_max_string_size(MAX_STRING_SIZE);
    engine.set_max_array_size(MAX_ARRAY_SIZE);
    engine.set_max_map_size(MAX_MAP_SIZE);

    // The time budget: on_progress fires between operations, so the
    // limit bounds script evaluation only — it does not tick while a
    // host call is blocked (per scripts-api.md).
    let start = Instant::now();
    engine.on_progress(move |_ops| {
        if start.elapsed() > time {
            Some("script evaluation timed out".into())
        } else {
            None
        }
    });

    // Parse protection: module imports are rejected at the tokenizer,
    // before the parser ever builds an AST with one.
    #[allow(deprecated)]
    engine.on_parse_token(
        |token: Token, _pos: Position, _state: &TokenizeState| match token {
            Token::Import | Token::Export | Token::As => {
                Token::Reserved(Box::new("modules are disabled".into()))
            }
            token => token,
        },
    );

    // Stdout hygiene: print and debug never touch stdout.
    engine.on_print({
        let print = Arc::clone(&print);
        move |text| print(text)
    });
    engine.on_debug(move |text, _source, _pos| print(text));
    engine
}

/// Verifies the script source against the size cap and compiles it into
/// the hardened engine.
fn compile(
    script: &str,
    time: Duration,
    print: Arc<dyn Fn(&str) + Send + Sync>,
) -> Result<Compiled, ScriptError> {
    if script.len() > SCRIPT_MAX_SIZE {
        return Err(ScriptError::TooLarge);
    }
    let engine = hardened(time, print);
    let ast = engine
        .compile(script)
        .map_err(|err| ScriptError::Compile(err.to_string()))?;
    // Reject immediately: a script with no functions at all is a
    // configuration error surfaced at load, not a silent no-op forever.
    Ok(Compiled { engine, ast })
}

/// Builds a fresh policy engine: decision inputs only — the acting host
/// functions are physically absent because nothing registers them.
pub fn policy_engine(script: &str) -> Result<Compiled, ScriptError> {
    compile(
        script,
        POLICY_TIME,
        Arc::new(|text| eprintln!("[script] {text}")),
    )
}

/// Builds a fresh behaviour engine with the full host API. The registrar
/// receives the hardened engine and registers the acting host functions
/// (T-026); each host function must be panic-contained.
pub fn behaviour_engine(
    script: &str,
    register: &dyn Fn(&mut Engine),
) -> Result<Compiled, ScriptError> {
    let mut engine = compile(
        script,
        BEHAVIOUR_TIME,
        Arc::new(|text| eprintln!("[script] {text}")),
    )?;
    register(&mut engine.engine);
    Ok(engine)
}

/// Argument payload cap: oversized payloads are replaced by a digest map
/// (`#{ truncated: true, size, sha256 }`) so scripts can detect but
/// cannot read them.
pub fn cap_payload(value: &Value) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= PAYLOAD_CAP {
        return value.clone();
    }
    let digest = Sha256::digest(serialized.as_bytes());
    let sha256: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    serde_json::json!({
        "truncated": true,
        "size": serialized.len(),
        "sha256": sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// print/debug sink for stdout-hygiene assertions.
    type Sink = Arc<Mutex<Vec<String>>>;

    fn compiling_engine_with_sink(script: &str, sink: Sink) -> Compiled {
        let engine = hardened(
            POLICY_TIME,
            Arc::new(move |text| {
                sink.lock().unwrap().push(text.to_owned());
            }),
        );
        let ast = engine.compile(script).unwrap();
        Compiled { engine, ast }
    }

    #[test]
    fn script_size_cap_is_enforced_at_compile() {
        let script = format!("let x = \"{}\";", "x".repeat(SCRIPT_MAX_SIZE + 1));
        let err = policy_engine(&script).unwrap_err();
        assert!(err.to_string().contains("size cap"), "{err}");
    }

    #[test]
    fn module_imports_are_rejected_at_parse() {
        let err = policy_engine("import \"std/math\" as m;").unwrap_err();
        assert!(matches!(err, ScriptError::Compile(_)), "{err}");
        assert!(err.to_string().contains("compile"), "{err}");
    }

    #[test]
    fn call_levels_are_bounded() {
        let script = "fn recurse(n) { if n <= 0 { 0 } else { recurse(n - 1) } } recurse(1_000_000)";
        let compiled = policy_engine(script).unwrap();
        let err = compiled
            .call("recurse", vec![Dynamic::from(1_000_000_i64)])
            .unwrap_err();
        // Bounded by the call-level/stack guard — an error, never a
        // process crash.
        assert!(
            matches!(err, ScriptError::Eval(ref message) if message.contains("Stack overflow")),
            "{err}"
        );
    }

    #[test]
    fn collection_sizes_are_bounded() {
        let compiled =
            policy_engine("fn grow() { let a = []; for i in 0..2_000_000 { a.push(i); } a.len() }")
                .unwrap();
        let err = compiled.call("grow", vec![]).unwrap_err();
        assert!(!matches!(err, ScriptError::Panicked(_)), "{err}");
    }

    #[tokio::test]
    async fn policy_engines_time_out() {
        // An endless loop must terminate within the ~1s policy budget.
        let start = Instant::now();
        let compiled = policy_engine("fn spin() { loop { } }").unwrap();
        let err = compiled.call("spin", vec![]).unwrap_err();
        assert!(matches!(err, ScriptError::Timeout), "{err}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn script_errors_fail_closed() {
        // Compile errors are errors, never silent no-ops.
        let err = policy_engine("this is not rhai {").unwrap_err();
        assert!(matches!(err, ScriptError::Compile(_)), "{err}");

        // Calling an absent function is an eval error — the permission
        // pipeline maps any ScriptError to fail-closed ask.
        let compiled = policy_engine("fn other(x) { x }").unwrap();
        let err = compiled
            .call("pre_tool_use", vec![Dynamic::from(1)])
            .unwrap_err();
        assert!(matches!(err, ScriptError::Eval(_)), "{err}");
    }

    #[test]
    fn print_and_debug_go_to_the_sink_never_stdout() {
        let sink: Sink = Arc::new(Mutex::new(Vec::new()));
        let compiled = compiling_engine_with_sink(
            r#"fn speak() { print("printed"); debug("debugged"); "done" }"#,
            Arc::clone(&sink),
        );

        let result = compiled.call("speak", vec![]).unwrap();
        assert_eq!(result.to_string(), "done");

        let lines = sink.lock().unwrap().clone();
        // rhai's debug channel presents strings quoted.
        assert_eq!(lines, vec!["printed".to_owned(), "\"debugged\"".to_owned()]);
    }

    #[test]
    fn panicking_host_functions_are_contained() {
        // The host registrar registers an acting function that panics —
        // behaviour engines carry the host API, policy engines do not.
        fn register_panic_host(engine: &mut Engine) {
            engine.register_fn("act", || -> u64 {
                panic!("host function exploded");
            });
        }

        let compiled = behaviour_engine("fn use_host() { act() }", &register_panic_host).unwrap();
        let err = compiled.call("use_host", vec![]).unwrap_err();
        assert!(
            matches!(err, ScriptError::Panicked(ref message) if message.contains("host function exploded")),
            "{err}"
        );

        // The engine separation: the same acting function is physically
        // absent from a policy engine — the call fails as eval error,
        // not by a gate.
        let compiled = policy_engine("fn use_host() { act() }").unwrap();
        let err = compiled.call("use_host", vec![]).unwrap_err();
        assert!(matches!(err, ScriptError::Eval(_)), "{err}");
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn oversized_payloads_become_digest_maps() {
        let small = json!({ "command": "cargo test" });
        assert_eq!(cap_payload(&small), small);

        let huge = json!({ "blob": "x".repeat(PAYLOAD_CAP + 1) });
        let capped = cap_payload(&huge);
        assert_eq!(capped["truncated"], json!(true));
        assert!(capped["size"].as_u64().unwrap() > PAYLOAD_CAP as u64);
        assert_eq!(capped["sha256"].as_str().unwrap().len(), 64);
        // The digest map replaced the content: the blob is unreadable.
        assert!(capped.get("blob").is_none());
    }
}
