//! The hardened Rhai engine builders: two structurally separated kinds.
//!
//! **Policy engines** run `pre_tool_use` scripts and receive decision
//! inputs only — they physically lack the acting host functions. **Behaviour
//! engines** run every other event with the full host API (registered by
//! the session host in T-026 via the registrar passed to
//! [`behaviour_engine`]).
//!
//! The Rhai machinery itself lives in `agentkit-script`; this module only
//! fixes the tackle-specific policy: fixed internal safety limits
//! (operations, call levels, expression depths, collection and string sizes,
//! a 256 KiB script size cap, a per-engine time budget), no acting host
//! functions on policy engines, and fail-closed handling of every
//! [`ScriptError`] by the caller (the permission pipeline treats error or
//! timeout as ask). Argument payloads above the cap are replaced by a digest
//! map so scripts can detect but cannot read them, and `print`/`debug` go to
//! stderr, never stdout.

use std::time::Duration;

use agentkit_script::{CompiledScript, INLINE_ORIGIN, Limits, ScriptEngine, ScriptError};
use serde_json::Value;

pub use agentkit_script::{PAYLOAD_CAP, cap_payload};

/// The script source size cap, bounding compile work.
pub const SCRIPT_MAX_SIZE: usize = 256 * 1024;

/// The policy engine time budget: about a second.
pub const POLICY_TIME: Duration = Duration::from_secs(1);

/// The behaviour engine time budget: about ten seconds.
pub const BEHAVIOUR_TIME: Duration = Duration::from_secs(10);

/// A compiled script bound to a fresh hardened engine.
#[derive(Debug)]
pub struct Compiled {
    engine: ScriptEngine,
    script: CompiledScript,
}

impl Compiled {
    /// Calls the script's event function (e.g. `pre_tool_use`) with the given
    /// JSON arguments, returning its JSON result. Panic-contained.
    pub fn call(&self, function: &str, input: &Value) -> Result<Value, ScriptError> {
        self.engine.call_value(&self.script, function, input)
    }
}

/// Builds a fresh policy engine: decision inputs only — the acting host
/// functions are physically absent because nothing registers them.
pub fn policy_engine(script: &str) -> Result<Compiled, ScriptError> {
    build(script, POLICY_TIME, &|_engine| {})
}

/// Builds a fresh behaviour engine with the full host API. The registrar
/// receives the hardened engine and registers the acting host functions
/// (T-026); a panicking host function is contained by the engine and surfaces
/// as [`ScriptError::Panicked`].
pub fn behaviour_engine(
    script: &str,
    register: &dyn Fn(&mut ScriptEngine),
) -> Result<Compiled, ScriptError> {
    build(script, BEHAVIOUR_TIME, register)
}

fn build(
    source: &str,
    time: Duration,
    register: &dyn Fn(&mut ScriptEngine),
) -> Result<Compiled, ScriptError> {
    let mut engine = ScriptEngine::with_limits(limits(time));
    register(&mut engine);
    let script = engine.compile(INLINE_ORIGIN, source)?;
    Ok(Compiled { engine, script })
}

/// The fixed internal safety limits, deliberately not user configuration.
fn limits(time: Duration) -> Limits {
    Limits {
        max_operations: 10_000_000,
        max_call_levels: 32,
        max_expr_depth: 128,
        max_function_expr_depth: 128,
        max_variables: 2048,
        max_functions: 512,
        max_string_size: SCRIPT_MAX_SIZE,
        max_array_size: 10_000,
        max_map_size: 10_000,
        max_strings_interned: 1_024,
        max_script_size: SCRIPT_MAX_SIZE,
        timeout: time,
    }
}

/// Runs the registered `pre_tool_use` policy scripts (FR-015). The script
/// sees the request as structured JSON; allow and deny are final (the
/// first decisive script wins); ask falls through; a script error,
/// timeout or malformed verdict is fail-closed (never an allow).
pub struct PolicyScripts {
    scripts: Vec<Compiled>,
}

impl PolicyScripts {
    /// Loads the policy scripts from their sources. A broken script is
    /// dropped (its absence is fail-closed, never a crash).
    #[must_use]
    pub fn load(sources: &[String]) -> Self {
        Self {
            scripts: sources
                .iter()
                .filter_map(|source| policy_engine(source).ok())
                .collect(),
        }
    }
}

impl crate::permissions::PolicyScript for PolicyScripts {
    fn pre_tool_use(
        &self,
        request: &crate::permissions::PolicyRequest,
    ) -> Option<crate::permissions::PolicyVerdict> {
        let input = serde_json::json!({
            "invokable": request.invokable,
            "kind": request.kind,
            "arguments": request.arguments,
            "interactive": request.interactive,
            "previously_granted": request.previously_granted,
        });
        let mut verdict = None;
        for script in &self.scripts {
            let Ok(value) = script.call("pre_tool_use", &input) else {
                continue;
            };
            let decision = match value.get("decision").and_then(serde_json::Value::as_str) {
                Some("allow") => crate::permissions::Decision::Allow,
                Some("deny") => crate::permissions::Decision::Deny,
                _ => crate::permissions::Decision::Ask,
            };
            let reason = value
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let current = crate::permissions::PolicyVerdict { decision, reason };
            if matches!(
                current.decision,
                crate::permissions::Decision::Allow | crate::permissions::Decision::Deny
            ) {
                return Some(current);
            }
            verdict = Some(current);
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn script_size_cap_is_enforced_at_compile() {
        let script = format!("let x = \"{}\";", "x".repeat(SCRIPT_MAX_SIZE + 1));
        let err = policy_engine(&script).unwrap_err();
        assert!(matches!(err, ScriptError::TooLarge { .. }), "{err}");
    }

    #[test]
    fn module_imports_are_rejected_at_compile() {
        let err = policy_engine("import \"std/math\" as m;").unwrap_err();
        assert!(matches!(err, ScriptError::Compile { .. }), "{err}");
    }

    #[test]
    fn policy_engines_lack_the_acting_host_functions() {
        // The engine separation: the acting function is physically absent
        // from a policy engine — the call fails as a runtime error, not by
        // a gate.
        let compiled = policy_engine("fn use_host(request) { act() }").unwrap();
        let err = compiled.call("use_host", &json!({})).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn behaviour_engines_expose_the_acting_host_functions() {
        let compiled = behaviour_engine("fn use_host(request) { act() }", &|engine| {
            engine
                .register_host_fn("act", 0, |_| Ok(json!({ "ok": true })))
                .unwrap();
        })
        .unwrap();
        let value = compiled.call("use_host", &json!({})).unwrap();
        assert_eq!(value["ok"], json!(true));
    }

    #[test]
    fn panicking_host_functions_are_contained() {
        let compiled = behaviour_engine("fn use_host(request) { act() }", &|engine| {
            engine
                .register_host_fn("act", 0, |_| panic!("host function exploded"))
                .unwrap();
        })
        .unwrap();
        let err = compiled.call("use_host", &json!({})).unwrap_err();
        assert!(
            matches!(err, ScriptError::Panicked { ref message, .. } if message.contains("host function exploded")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn runaway_policy_scripts_terminate_bounded() {
        // A spin loop must terminate under a fail-closed error, whether the
        // operation cap or the ~1s time budget binds first.
        let compiled = policy_engine("fn spin(request) { loop { } }").unwrap();
        let err = compiled.call("spin", &json!({})).unwrap_err();
        assert!(
            matches!(
                err,
                ScriptError::Timeout { .. } | ScriptError::LimitExceeded { .. }
            ),
            "{err}"
        );
    }

    #[test]
    fn script_errors_fail_closed() {
        // Compile errors are errors, never silent no-ops.
        let err = policy_engine("this is not rhai {").unwrap_err();
        assert!(matches!(err, ScriptError::Compile { .. }), "{err}");

        // Calling an absent function is an error — the permission pipeline
        // maps any ScriptError to fail-closed ask.
        let compiled = policy_engine("fn other(x) { x }").unwrap();
        let err = compiled.call("pre_tool_use", &json!({})).unwrap_err();
        assert!(matches!(err, ScriptError::MissingFunction { .. }), "{err}");
    }
}
