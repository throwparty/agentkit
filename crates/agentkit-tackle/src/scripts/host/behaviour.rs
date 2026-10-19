//! The behaviour host: the registered host functions and the fixed budgets they enforce.

use super::api::{Completion, HostError, SessionAccess};
use super::budgets::{
    MAX_LIVE_EPHEMERAL_FORKS, MAX_PROMPTS_PER_SCRIPT, MAX_PROMPTS_PER_SESSION,
    MAX_PROMPTS_PER_TURN, PARENT_COST_CAP_USD,
};
use crate::store::{SessionId, SessionKind};
use agentkit_script::ScriptEngine;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// The per-invocation budget state, process-local.
#[derive(Default)]
struct BudgetState {
    prompts_per_script: usize,
    prompts_per_turn: usize,
    prompts_per_session: HashMap<SessionId, usize>,
    live_ephemeral_forks: HashMap<SessionId, usize>,
}

/// One behaviour host: the budget-tracked acting API one script
/// invocation sees.
pub struct BehaviourHost {
    access: Arc<dyn SessionAccess>,
    known_actors: BTreeSet<String>,
    /// The script's own session: forks spawn from it and script-driven
    /// model calls attribute to it.
    origin: SessionId,
    /// The invoking script's name, for log attribution.
    script: String,
    budget: Arc<Mutex<BudgetState>>,
}

impl BehaviourHost {
    pub fn new(
        access: Arc<dyn SessionAccess>,
        known_actors: BTreeSet<String>,
        origin: SessionId,
        script: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            access,
            known_actors,
            origin,
            script,
            budget: Arc::new(Mutex::new(BudgetState::default())),
        })
    }

    pub(super) fn fork_session(&self, ephemeral: bool, actor: &str) -> Result<SessionId, HostError> {
        if !actor.is_empty() && !self.known_actors.contains(actor) {
            return Err(HostError::UnknownActor(actor.to_owned()));
        }
        // Depth-one nesting: a script inside an ephemeral fork cannot
        // fork again.
        if self.access.session_kind(&self.origin)? == SessionKind::Ephemeral {
            return Err(HostError::DepthExceeded);
        }
        let mut budget = self.budget.lock().unwrap();
        if ephemeral {
            let live = budget
                .live_ephemeral_forks
                .entry(self.origin.clone())
                .or_default();
            if *live >= MAX_LIVE_EPHEMERAL_FORKS {
                return Err(HostError::Budget("live ephemeral forks"));
            }
            *live += 1;
        }
        self.access.fork(&self.origin, ephemeral, actor)
    }

    pub(super) fn send_prompt(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
        {
            let mut budget = self.budget.lock().unwrap();
            if budget.prompts_per_script >= MAX_PROMPTS_PER_SCRIPT {
                return Err(HostError::Budget("prompts per script"));
            }
            if budget.prompts_per_turn >= MAX_PROMPTS_PER_TURN {
                return Err(HostError::Budget("prompts per turn"));
            }
            let per_session = budget
                .prompts_per_session
                .entry(session.clone())
                .or_default();
            if *per_session >= MAX_PROMPTS_PER_SESSION {
                return Err(HostError::Budget("prompts per session"));
            }
        }
        // The cost cap is enforced at the parent: script-driven model
        // calls attribute to it.
        if self.access.attributed_cost(&self.origin)? > PARENT_COST_CAP_USD {
            return Err(HostError::Budget("parent cost cap"));
        }
        {
            let mut budget = self.budget.lock().unwrap();
            budget.prompts_per_script += 1;
            budget.prompts_per_turn += 1;
            *budget
                .prompts_per_session
                .entry(session.clone())
                .or_default() += 1;
        }
        self.access.send_prompt(session, text)
    }

    pub(super) fn await_completion(
        &self,
        session: &SessionId,
        timeout_secs: i64,
    ) -> Result<Completion, HostError> {
        self.access.await_completion(session, timeout_secs)
    }

    fn log(&self, message: &str) {
        eprintln!("[script:{}] {message}", self.script);
    }
}

/// A required string argument, or a script error naming the position.
fn string_arg(args: &[Value], index: usize) -> Result<String, String> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("argument {index} must be a string"))
}

/// Registers one host function. Arities 0 to 3 are always supported, so a
/// failure is a programming error, not a script error.
fn host_fn<F>(engine: &mut ScriptEngine, name: &str, arity: usize, function: F)
where
    F: Fn(&[Value]) -> Result<Value, String> + Send + Sync + 'static,
{
    engine
        .register_host_fn(name, arity, function)
        .expect("host function arity is within 0..=3");
}

/// Registers the behaviour host functions on the engine: the full acting
/// API, budget-tracked. Policy engines never call this. Every host function
/// exchanges JSON with the script; a [`HostError`] surfaces as a script
/// runtime error.
pub fn register_host_api(engine: &mut ScriptEngine, host: Arc<BehaviourHost>) {
    let fork_host = Arc::clone(&host);
    host_fn(engine, "fork_session", 1, move |args: &[Value]| {
        let opts = &args[0];
        let ephemeral = opts
            .get("ephemeral")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let actor = opts
            .get("actor")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        fork_host
            .fork_session(ephemeral, &actor)
            .map(|session_id| serde_json::json!({ "session_id": session_id, "agent": actor }))
            .map_err(|err| err.to_string())
    });

    let send_host = Arc::clone(&host);
    host_fn(engine, "send_prompt", 2, move |args: &[Value]| {
        let session_id = string_arg(args, 0)?;
        let text = string_arg(args, 1)?;
        send_host
            .send_prompt(&session_id, &text)
            .map(|()| Value::Null)
            .map_err(|err| err.to_string())
    });

    let await_host = Arc::clone(&host);
    host_fn(engine, "await_completion", 2, move |args: &[Value]| {
        let session_id = string_arg(args, 0)?;
        let timeout_secs = args[1]
            .as_i64()
            .ok_or_else(|| "argument 1 must be an integer".to_owned())?;
        await_host
            .await_completion(&session_id, timeout_secs)
            .map(|completion| {
                serde_json::json!({
                    "stop_reason": completion.stop_reason,
                    "final_message": completion.final_message,
                    "input_tokens": completion.input_tokens,
                    "output_tokens": completion.output_tokens,
                })
            })
            .map_err(|err| err.to_string())
    });

    let seed_host = Arc::clone(&host);
    host_fn(engine, "insert_seed", 1, move |args: &[Value]| {
        let text = string_arg(args, 0)?;
        seed_host
            .access
            .insert_seed(&seed_host.origin, &text)
            .map(|()| Value::Null)
            .map_err(|err| err.to_string())
    });

    let compaction_host = Arc::clone(&host);
    host_fn(engine, "record_compaction", 2, move |args: &[Value]| {
        let summary = string_arg(args, 0)?;
        // `()` (JSON null) elides everything before the summary.
        let first_retained = args[1].as_str().map(str::to_owned);
        compaction_host
            .access
            .record_compaction(&compaction_host.origin, &summary, first_retained.as_ref())
            .map(|()| Value::Null)
            .map_err(|err| err.to_string())
    });

    let title_host = Arc::clone(&host);
    host_fn(engine, "set_session_title", 1, move |args: &[Value]| {
        let title = string_arg(args, 0)?;
        // Trimming here: titles arrive from model output.
        title_host
            .access
            .set_session_title(&title_host.origin, title.trim())
            .map(|()| Value::Null)
            .map_err(|err| err.to_string())
    });

    let history_host = Arc::clone(&host);
    host_fn(engine, "history_search", 1, move |args: &[Value]| {
        let query = string_arg(args, 0)?;
        history_host
            .access
            .history_search(&history_host.origin, &query)
            .map(|rows| {
                Value::Array(
                    rows.into_iter()
                        .map(|row| {
                            serde_json::json!({
                                "role": row.role,
                                "content": row.content,
                                "turn_id": row.turn_id,
                                "created_at": row.created_at,
                            })
                        })
                        .collect(),
                )
            })
            .map_err(|err| err.to_string())
    });

    let usage_host = Arc::clone(&host);
    host_fn(engine, "context_usage", 0, move |_args: &[Value]| {
        usage_host
            .access
            .context_usage(&usage_host.origin)
            .map(|usage| serde_json::json!({ "used": usage.used, "size": usage.size }))
            .map_err(|err| err.to_string())
    });

    let log_host = Arc::clone(&host);
    host_fn(engine, "log", 1, move |args: &[Value]| {
        log_host.log(args[0].as_str().unwrap_or_default());
        Ok(Value::Null)
    });
}

