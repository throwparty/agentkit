//! `SessionAccess` and the behaviour host functions — the acting API
//! behaviour scripts get and policy engines physically lack (T-026).
//!
//! Every host function goes through [`SessionAccess`], the seam between
//! the script host and the session engine. The fixed internal budgets —
//! live ephemeral forks, prompts per script and turn and session, the
//! cost cap enforced at the parent, the depth-one nesting rule — are
//! enforced here, never user-configurable. Script-driven model calls are
//! surfaced in the client by the dispatcher implementations. Host
//! functions are synchronous (rhai 1.26 has no async) and block on the
//! store through the ambient runtime; the script time budget does not
//! tick while a host call is blocked.

use crate::store::{Role, SessionId, SessionKind, SessionStore, TurnId, TurnKind};
use rhai::{Dynamic, Engine};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fixed internal budgets, deliberately not user configuration.
pub const MAX_LIVE_EPHEMERAL_FORKS: usize = 4;
pub const MAX_PROMPTS_PER_SCRIPT: usize = 2;
pub const MAX_PROMPTS_PER_TURN: usize = 8;
pub const MAX_PROMPTS_PER_SESSION: usize = 64;
/// The parent-session cost cap for script-driven model calls.
pub const PARENT_COST_CAP_USD: f64 = 5.0;
/// history_search row and byte caps.
pub const HISTORY_MAX_ROWS: usize = 20;
pub const HISTORY_MAX_BYTES: usize = 4 * 1024;
/// await_completion's poll interval.
const COMPLETION_POLL: Duration = Duration::from_millis(25);

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("store error: {0}")]
    Store(#[from] crate::store::StoreError),
    #[error("unknown actor `{0}`")]
    UnknownActor(String),
    #[error("budget exhausted: {0}")]
    Budget(&'static str),
    #[error("nesting beyond depth one is forbidden")]
    DepthExceeded,
    #[error("await timed out after {0}s")]
    Timeout(i64),
    #[error("invalid argument: {0}")]
    Invalid(&'static str),
}

/// A history_search row.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    pub role: String,
    pub content: String,
    pub turn_id: TurnId,
    pub created_at: i64,
}

/// What await_completion returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub stop_reason: String,
    pub final_message: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Current context utilisation for a session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextUsage {
    pub used: u64,
    pub size: u64,
}

/// Drives a user prompt through the session's normal agent loop —
/// model-mediated and pipeline-gated. The turn loop implements this
/// (T-015 wiring); scripts never invoke tools directly.
pub trait PromptDispatcher: Send + Sync {
    fn dispatch(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
}

/// Publishes turn completions; await_completion polls it. The turn
/// loop publishes (T-015 wiring).
pub trait CompletionSink: Send + Sync {
    fn publish(&self, session: &SessionId, completion: Completion);
    fn take(&self, session: &SessionId) -> Option<Completion>;
}

/// The seam between the script host and the session engine: history
/// search, fork, prompt, compaction recording, titling.
pub trait SessionAccess: Send + Sync {
    /// The session's kind — the depth-one rule reads it.
    fn session_kind(&self, session: &SessionId) -> Result<SessionKind, HostError>;
    /// Forks the session; usage of an ephemeral fork attributes to its
    /// parent (the DAG is shared).
    fn fork(
        &self,
        source: &SessionId,
        ephemeral: bool,
        actor: &str,
    ) -> Result<SessionId, HostError>;
    fn send_prompt(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
    fn await_completion(
        &self,
        session: &SessionId,
        timeout_secs: i64,
    ) -> Result<Completion, HostError>;
    fn insert_seed(&self, session: &SessionId, text: &str) -> Result<(), HostError>;
    /// Appends a compaction turn; `first_retained` clamped to be newer
    /// than any older compaction turn on the chain.
    fn record_compaction(
        &self,
        session: &SessionId,
        summary: &str,
        first_retained: Option<&TurnId>,
    ) -> Result<(), HostError>;
    fn set_session_title(&self, session: &SessionId, title: &str) -> Result<(), HostError>;
    fn history_search(
        &self,
        session: &SessionId,
        query: &str,
    ) -> Result<Vec<HistoryRow>, HostError>;
    fn context_usage(&self, session: &SessionId) -> Result<ContextUsage, HostError>;
    /// The session's cost plus its live ephemeral forks': the parent's
    /// attributable cost, which the cap is enforced against.
    fn attributed_cost(&self, session: &SessionId) -> Result<f64, HostError>;
}

/// Runs an async store operation to completion. Host functions are
/// synchronous; on a runtime this must be the multi-thread flavor
/// (block_in_place), which is the production shape.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("host runtime");
            runtime.block_on(future)
        }
    }
}

/// The store-backed [`SessionAccess`].
pub struct StoreAccess {
    store: SessionStore,
    dispatcher: Arc<dyn PromptDispatcher>,
    completions: Arc<dyn CompletionSink>,
    /// The model context window size, from agentkit-models metadata.
    pub window_size: u64,
    /// Live ephemeral forks per parent, process-local.
    pub live_forks: Arc<Mutex<HashMap<SessionId, Vec<SessionId>>>>,
}

impl StoreAccess {
    pub fn new(
        store: SessionStore,
        dispatcher: Arc<dyn PromptDispatcher>,
        completions: Arc<dyn CompletionSink>,
        window_size: u64,
    ) -> Self {
        Self {
            store,
            dispatcher,
            completions,
            window_size,
            live_forks: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SessionAccess for StoreAccess {
    fn session_kind(&self, session: &SessionId) -> Result<SessionKind, HostError> {
        let kind = block_on(self.store.get_session(session))?
            .ok_or(HostError::Invalid("no such session"))?
            .kind;
        Ok(kind)
    }

    fn fork(
        &self,
        source: &SessionId,
        ephemeral: bool,
        actor: &str,
    ) -> Result<SessionId, HostError> {
        let source_session = block_on(self.store.get_session(source))?
            .ok_or(HostError::Invalid("no such session"))?;
        let fork = block_on(self.store.create_session(
            if ephemeral {
                SessionKind::Ephemeral
            } else {
                SessionKind::Interactive
            },
            &source_session.cwd,
            Some(source),
            source_session.head_turn_id.as_ref(),
            &format!(r#"{{"actor": "{actor}"}}"#),
        ))?;
        if ephemeral {
            self.live_forks
                .lock()
                .unwrap()
                .entry(source.clone())
                .or_default()
                .push(fork.id.clone());
        }
        Ok(fork.id)
    }

    fn send_prompt(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
        block_on(async {
            // The prompt enters the target session as a normal user
            // message on a new interaction turn, chained from the head.
            let head = self
                .store
                .get_session(session)
                .await?
                .ok_or(HostError::Invalid("no such session"))?
                .head_turn_id;
            let turn = self
                .store
                .append_turn(
                    session,
                    head.as_ref(),
                    TurnKind::Interaction,
                    None,
                    Default::default(),
                )
                .await?;
            let content = serde_json::json!([{ "type": "text", "text": text }]).to_string();
            self.store
                .append_message(&turn.id, Role::User, &content, None, None, None, None)
                .await?;
            // ...then the dispatcher drives it through the agent loop.
            self.dispatcher.dispatch(session, text)
        })
    }

    fn await_completion(
        &self,
        session: &SessionId,
        timeout_secs: i64,
    ) -> Result<Completion, HostError> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(0) as u64);
        loop {
            if let Some(completion) = self.completions.take(session) {
                return Ok(completion);
            }
            if Instant::now() >= deadline {
                return Err(HostError::Timeout(timeout_secs));
            }
            std::thread::sleep(COMPLETION_POLL);
        }
    }

    fn insert_seed(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
        block_on(async {
            // Seeds are harness-authored static user-facing content with
            // their own messageId, on their own turn kind chained from
            // the head.
            let head = self
                .store
                .get_session(session)
                .await?
                .ok_or(HostError::Invalid("no such session"))?
                .head_turn_id;
            let turn = self
                .store
                .append_turn(
                    session,
                    head.as_ref(),
                    TurnKind::Seed,
                    None,
                    Default::default(),
                )
                .await?;
            let content = serde_json::json!([{ "type": "text", "text": text }]).to_string();
            self.store
                .append_message(&turn.id, Role::User, &content, None, None, None, None)
                .await?;
            Ok(())
        })
    }

    fn record_compaction(
        &self,
        session: &SessionId,
        summary: &str,
        first_retained: Option<&TurnId>,
    ) -> Result<(), HostError> {
        block_on(async {
            // Clamp: the retained boundary must be strictly newer than
            // any compaction turn on the chain, so summaries never
            // overlap.
            let walk = self.store.session_walk(session).await?;
            let clamped = match first_retained {
                None => None,
                Some(retained) => {
                    let position = walk.iter().position(|(id, _)| id == retained).ok_or(
                        HostError::Invalid("first_retained turn is not on the chain"),
                    )?;
                    match walk
                        .iter()
                        .position(|(_, kind)| *kind == TurnKind::Compaction)
                    {
                        None => Some(retained.clone()),
                        Some(newest_compaction) => {
                            if position < newest_compaction {
                                // Strictly newer than the newest
                                // compaction: keep as given.
                                Some(retained.clone())
                            } else if newest_compaction > 0 {
                                // Clamp to the turn immediately newer
                                // than the newest compaction.
                                Some(walk[newest_compaction - 1].0.clone())
                            } else {
                                // The newest compaction is the head:
                                // elide everything before the summary.
                                None
                            }
                        }
                    }
                }
            };
            let turn = self
                .store
                .append_turn(
                    session,
                    walk.first().map(|(id, _)| id),
                    TurnKind::Compaction,
                    clamped.as_ref(),
                    Default::default(),
                )
                .await?;
            let content = serde_json::json!([{ "type": "text", "text": summary }]).to_string();
            self.store
                .append_message(&turn.id, Role::System, &content, None, None, None, None)
                .await?;
            Ok(())
        })
    }

    fn set_session_title(&self, session: &SessionId, title: &str) -> Result<(), HostError> {
        block_on(self.store.set_title(session, title))?;
        Ok(())
    }

    fn history_search(
        &self,
        session: &SessionId,
        query: &str,
    ) -> Result<Vec<HistoryRow>, HostError> {
        let assembled = block_on(self.store.assemble_context(session))?;
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        for item in assembled {
            if rows.len() >= HISTORY_MAX_ROWS || bytes >= HISTORY_MAX_BYTES {
                break;
            }
            if !item.message.content.contains(query) {
                continue;
            }
            let content = item.message.content;
            bytes += content.len();
            rows.push(HistoryRow {
                role: format!("{:?}", item.message.role).to_lowercase(),
                content,
                turn_id: item.message.turn_id,
                created_at: item.message.created_at,
            });
        }
        Ok(rows)
    }

    fn context_usage(&self, session: &SessionId) -> Result<ContextUsage, HostError> {
        let assembled = block_on(self.store.assemble_context(session))?;
        let used: usize = assembled
            .iter()
            .map(|item| item.message.content.len())
            .sum();
        Ok(ContextUsage {
            used: used as u64,
            size: self.window_size,
        })
    }

    fn attributed_cost(&self, session: &SessionId) -> Result<f64, HostError> {
        let own = block_on(self.store.session_usage(session))?.cost_usd;
        let forks: Vec<SessionId> = self
            .live_forks
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .unwrap_or_default();
        let mut total = own;
        for fork in forks {
            total += block_on(self.store.session_usage(&fork))?.cost_usd;
        }
        Ok(total)
    }
}

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

    fn fork_session(&self, ephemeral: bool, actor: &str) -> Result<SessionId, HostError> {
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

    fn send_prompt(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
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

    fn await_completion(
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

fn eval_error(err: HostError) -> Box<rhai::EvalAltResult> {
    Box::new(rhai::EvalAltResult::ErrorRuntime(
        format!("{err}").into(),
        rhai::Position::NONE,
    ))
}

/// Registers the behaviour host functions on the engine: the full acting
/// API, budget-tracked. Policy engines never call this.
pub fn register_host_api(engine: &mut Engine, host: Arc<BehaviourHost>) {
    let fork_host = Arc::clone(&host);
    engine.register_fn(
        "fork_session",
        move |opts: rhai::Map| -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            let ephemeral = opts
                .get("ephemeral")
                .and_then(|value| value.clone().as_bool().ok())
                .unwrap_or(false);
            let actor = opts
                .get("actor")
                .and_then(|value| value.clone().into_string().ok())
                .unwrap_or_default();
            fork_host
                .fork_session(ephemeral, &actor)
                .map(|session_id| {
                    let mut map = rhai::Map::new();
                    map.insert("session_id".into(), session_id.into());
                    map.insert("agent".into(), actor.into());
                    map.into()
                })
                .map_err(eval_error)
        },
    );

    let send_host = Arc::clone(&host);
    engine.register_fn(
        "send_prompt",
        move |session_id: &str, text: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            send_host
                .send_prompt(&session_id.to_owned(), text)
                .map_err(eval_error)
        },
    );

    let await_host = Arc::clone(&host);
    engine.register_fn(
        "await_completion",
        move |session_id: &str, timeout_secs: i64| -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            await_host
                .await_completion(&session_id.to_owned(), timeout_secs)
                .map(|completion| {
                    let mut map = rhai::Map::new();
                    map.insert("stop_reason".into(), completion.stop_reason.into());
                    map.insert("final_message".into(), completion.final_message.into());
                    map.insert(
                        "input_tokens".into(),
                        Dynamic::from(completion.input_tokens),
                    );
                    map.insert(
                        "output_tokens".into(),
                        Dynamic::from(completion.output_tokens),
                    );
                    map.into()
                })
                .map_err(eval_error)
        },
    );

    let seed_host = Arc::clone(&host);
    engine.register_fn(
        "insert_seed",
        move |text: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            seed_host
                .access
                .insert_seed(&seed_host.origin, text)
                .map_err(eval_error)
        },
    );

    let compaction_host = Arc::clone(&host);
    engine.register_fn(
        "record_compaction",
        move |summary: &str, first_retained: Dynamic| -> Result<(), Box<rhai::EvalAltResult>> {
            // `()` elides everything before the summary.
            let first_retained = first_retained.into_string().ok();
            compaction_host
                .access
                .record_compaction(&compaction_host.origin, summary, first_retained.as_ref())
                .map_err(eval_error)
        },
    );

    let title_host = Arc::clone(&host);
    engine.register_fn(
        "set_session_title",
        move |title: &str| -> Result<(), Box<rhai::EvalAltResult>> {
            title_host
                .access
                .set_session_title(&title_host.origin, title)
                .map_err(eval_error)
        },
    );

    let history_host = Arc::clone(&host);
    engine.register_fn(
        "history_search",
        move |query: &str| -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            history_host
                .access
                .history_search(&history_host.origin, query)
                .map(|rows| {
                    let list: Vec<Dynamic> = rows
                        .into_iter()
                        .map(|row| {
                            let mut map = rhai::Map::new();
                            map.insert("role".into(), row.role.into());
                            map.insert("content".into(), row.content.into());
                            map.insert("turn_id".into(), row.turn_id.into());
                            map.insert("created_at".into(), Dynamic::from(row.created_at));
                            map.into()
                        })
                        .collect();
                    list.into()
                })
                .map_err(eval_error)
        },
    );

    let usage_host = Arc::clone(&host);
    engine.register_fn(
        "context_usage",
        move || -> Result<Dynamic, Box<rhai::EvalAltResult>> {
            usage_host
                .access
                .context_usage(&usage_host.origin)
                .map(|usage| {
                    let mut map = rhai::Map::new();
                    map.insert("used".into(), Dynamic::from(usage.used));
                    map.insert("size".into(), Dynamic::from(usage.size));
                    map.into()
                })
                .map_err(eval_error)
        },
    );

    let log_host = Arc::clone(&host);
    engine.register_fn("log", move |message: &str| {
        log_host.log(message);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TurnUsage;

    struct RecordingDispatcher {
        prompts: Mutex<Vec<(SessionId, String)>>,
    }

    impl PromptDispatcher for RecordingDispatcher {
        fn dispatch(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
            self.prompts
                .lock()
                .unwrap()
                .push((session.clone(), text.to_owned()));
            Ok(())
        }
    }

    struct RegistryCompletions {
        completions: Mutex<HashMap<SessionId, Completion>>,
    }

    impl CompletionSink for RegistryCompletions {
        fn publish(&self, session: &SessionId, completion: Completion) {
            self.completions
                .lock()
                .unwrap()
                .insert(session.clone(), completion);
        }

        fn take(&self, session: &SessionId) -> Option<Completion> {
            self.completions.lock().unwrap().remove(session)
        }
    }

    struct Fixture {
        access: Arc<StoreAccess>,
        dispatcher: Arc<RecordingDispatcher>,
        completions: Arc<RegistryCompletions>,
        origin: SessionId,
    }

    fn fixture() -> Fixture {
        let store = SessionStore::in_memory();
        let origin = block_on(store.create_session(
            SessionKind::Interactive,
            "/workspace",
            None,
            None,
            "{}",
        ))
        .unwrap()
        .id;
        let dispatcher = Arc::new(RecordingDispatcher {
            prompts: Mutex::new(Vec::new()),
        });
        let completions = Arc::new(RegistryCompletions {
            completions: Mutex::new(HashMap::new()),
        });
        let access = Arc::new(StoreAccess::new(
            store,
            Arc::clone(&dispatcher) as Arc<dyn PromptDispatcher>,
            Arc::clone(&completions) as Arc<dyn CompletionSink>,
            100_000,
        ));
        Fixture {
            access,
            dispatcher,
            completions,
            origin,
        }
    }

    fn host(fixture: &Fixture) -> Arc<BehaviourHost> {
        BehaviourHost::new(
            Arc::clone(&fixture.access) as Arc<dyn SessionAccess>,
            BTreeSet::from(["summariser".to_owned()]),
            fixture.origin.clone(),
            "test-script".to_owned(),
        )
    }

    #[test]
    fn fork_session_creates_the_fork_and_enforces_the_depth_one_rule() {
        let fixture = fixture();
        let host = host(&fixture);

        let fork_id = host.fork_session(true, "summariser").unwrap();
        let fork = block_on(fixture.access.store.get_session(&fork_id))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, SessionKind::Ephemeral);
        assert_eq!(
            fork.forked_from_session_id.as_deref(),
            Some(fixture.origin.as_str())
        );

        // An unknown actor is a script error.
        let err = host.fork_session(true, "rogue").unwrap_err();
        assert!(err.to_string().contains("unknown actor `rogue`"), "{err}");

        // Depth one: a script inside an ephemeral fork cannot fork again.
        let nested_host = BehaviourHost::new(
            Arc::clone(&fixture.access) as Arc<dyn SessionAccess>,
            BTreeSet::new(),
            fork_id.clone(),
            "test-script".to_owned(),
        );
        let err = nested_host.fork_session(false, "").unwrap_err();
        assert!(
            err.to_string().contains("nesting beyond depth one"),
            "{err}"
        );
    }

    #[test]
    fn live_ephemeral_forks_are_budget_capped() {
        let fixture = fixture();
        let host = host(&fixture);

        for _ in 0..MAX_LIVE_EPHEMERAL_FORKS {
            host.fork_session(true, "").unwrap();
        }
        let err = host.fork_session(true, "").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: live ephemeral forks"),
            "{err}"
        );
    }

    #[test]
    fn send_prompt_dispatches_records_and_enforces_budgets() {
        let fixture = fixture();
        let host = host(&fixture);
        let target = host.fork_session(true, "").unwrap();

        host.send_prompt(&target, "do the thing").unwrap();
        assert_eq!(
            fixture.dispatcher.prompts.lock().unwrap().clone(),
            vec![(target.clone(), "do the thing".to_owned())]
        );
        // The prompt entered the target session as a user message.
        let assembled = block_on(fixture.access.store.assemble_context(&target)).unwrap();
        assert!(assembled
            .iter()
            .any(|item| item.message.content.contains("do the thing")));

        // Budget exhaustion: prompts per script.
        host.send_prompt(&target, "again").unwrap();
        let err = host.send_prompt(&target, "once more").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: prompts per script"),
            "{err}"
        );
    }

    #[test]
    fn the_cost_cap_is_enforced_at_the_parent() {
        let fixture = fixture();
        let host = host(&fixture);

        // Attribute cost above the cap to the parent.
        let turn = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap();
        block_on(fixture.access.store.set_turn_usage(
            &turn.id,
            TurnUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: PARENT_COST_CAP_USD + 0.01,
            },
        ))
        .unwrap();

        let target = host.fork_session(true, "").unwrap();
        let err = host.send_prompt(&target, "spend more").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: parent cost cap"),
            "{err}"
        );
    }

    #[test]
    fn fork_usage_attributes_to_the_parent() {
        let fixture = fixture();
        let host = host(&fixture);
        let fork = host.fork_session(true, "").unwrap();

        assert_eq!(
            fixture.access.attributed_cost(&fixture.origin).unwrap(),
            0.0
        );

        // The fork's model calls land on its turns; the parent's
        // attributable cost includes them.
        let turn = block_on(fixture.access.store.append_turn(
            &fork,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap();
        block_on(fixture.access.store.set_turn_usage(
            &turn.id,
            TurnUsage {
                input_tokens: 10,
                output_tokens: 5,
                cost_usd: 0.75,
            },
        ))
        .unwrap();

        assert_eq!(
            fixture.access.attributed_cost(&fixture.origin).unwrap(),
            0.75
        );
        // The fork's own usage is its own: no double counting.
        assert_eq!(fixture.access.attributed_cost(&fork).unwrap(), 0.75);
    }

    #[test]
    fn await_completion_polls_the_registry_and_times_out() {
        let fixture = fixture();
        let host = host(&fixture);

        fixture.completions.publish(
            &fixture.origin,
            Completion {
                stop_reason: "end_turn".to_owned(),
                final_message: "done".to_owned(),
                input_tokens: 1,
                output_tokens: 1,
            },
        );
        let completion = host.await_completion(&fixture.origin, 5).unwrap();
        assert_eq!(completion.stop_reason, "end_turn");
        assert_eq!(completion.final_message, "done");

        // Nothing published: the timeout expires.
        let err = host.await_completion(&fixture.origin, 0).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[test]
    fn insert_seed_appends_harness_authored_content() {
        let fixture = fixture();
        fixture
            .access
            .insert_seed(&fixture.origin, "Forked for a fresh start.")
            .unwrap();

        let assembled = block_on(fixture.access.store.assemble_context(&fixture.origin)).unwrap();
        let seed = assembled
            .iter()
            .find(|item| item.turn_kind == TurnKind::Seed)
            .unwrap();
        assert!(seed.message.content.contains("Forked for a fresh start."));
    }

    #[test]
    fn record_compaction_clamps_the_retained_boundary() {
        let fixture = fixture();

        // Chain: t1 -> t2 -> c1(first_retained=t1) -> t3 (head).
        let t1 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;
        let t2 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&t1),
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;
        let c1 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&t2),
            TurnKind::Compaction,
            Some(&t1),
            Default::default(),
        ))
        .unwrap()
        .id;
        let t3 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&c1),
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;

        // A compaction whose retained boundary sits at-or-older than
        // c1 clamps to the turn immediately newer than c1 — t3.
        fixture
            .access
            .record_compaction(&fixture.origin, "summary", Some(&t2))
            .unwrap();
        let walk = block_on(fixture.access.store.session_walk(&fixture.origin)).unwrap();
        // The walk from the new head: [c2, t3] — c2's boundary is t3.
        assert_eq!(walk.len(), 2);
        assert_eq!(walk[0].1, TurnKind::Compaction);
        assert_eq!(walk[1].0, t3);
        let c2 = block_on(fixture.access.store.get_turn(&walk[0].0))
            .unwrap()
            .unwrap();
        assert_eq!(c2.first_retained_turn_id.as_deref(), Some(t3.as_str()));

        // A boundary strictly newer than the newest compaction is kept;
        // here the only boundary strictly newer than c2 is none — the
        // compaction elides everything before the summary.
        fixture
            .access
            .record_compaction(&fixture.origin, "second summary", Some(&t3))
            .unwrap();
        let walk = block_on(fixture.access.store.session_walk(&fixture.origin)).unwrap();
        assert_eq!(walk[0].1, TurnKind::Compaction);
        let c3 = block_on(fixture.access.store.get_turn(&walk[0].0))
            .unwrap()
            .unwrap();
        assert_eq!(c3.first_retained_turn_id, None);

        // A foreign turn id is refused.
        let err = fixture
            .access
            .record_compaction(&fixture.origin, "x", Some(&"not-a-turn".to_owned()))
            .unwrap_err();
        assert!(err.to_string().contains("not on the chain"), "{err}");
    }

    #[test]
    fn set_session_title_updates_the_session() {
        let fixture = fixture();
        fixture
            .access
            .set_session_title(&fixture.origin, "Fork lineage")
            .unwrap();
        let session = block_on(fixture.access.store.get_session(&fixture.origin))
            .unwrap()
            .unwrap();
        assert_eq!(session.title, "Fork lineage");
    }

    #[test]
    fn history_search_is_row_and_byte_capped() {
        let fixture = fixture();
        for i in 0..30 {
            fixture
                .access
                .send_prompt(&fixture.origin, &format!("needle {i}"))
                .unwrap();
        }
        let rows = fixture
            .access
            .history_search(&fixture.origin, "needle")
            .unwrap();
        assert_eq!(rows.len(), HISTORY_MAX_ROWS);
        assert!(rows.iter().all(|row| row.content.contains("needle")));

        let rows = fixture
            .access
            .history_search(&fixture.origin, "missing")
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn context_usage_reports_used_and_size() {
        let fixture = fixture();
        fixture
            .access
            .send_prompt(&fixture.origin, "hello context")
            .unwrap();
        let usage = fixture.access.context_usage(&fixture.origin).unwrap();
        assert!(usage.used > 0);
        assert_eq!(usage.size, 100_000);
    }

    #[test]
    fn the_host_api_is_registered_on_a_behaviour_engine() {
        let fixture = fixture();
        let store_host = host(&fixture);
        let mut engine = Engine::new();
        register_host_api(&mut engine, Arc::clone(&store_host));

        let fork_id: String = engine
            .eval(
                r#"
                let fork = fork_session(#{ ephemeral: true, actor: "summariser" });
                fork.session_id
            "#,
            )
            .unwrap();
        let fork = block_on(fixture.access.store.get_session(&fork_id))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, SessionKind::Ephemeral);

        // Budgets surface as script errors through the rhai boundary.
        let err: Result<String, Box<rhai::EvalAltResult>> = engine.eval(
            r#"
                let fork = fork_session(#{ ephemeral: true });
                send_prompt(fork.session_id, "one");
                send_prompt(fork.session_id, "two");
                send_prompt(fork.session_id, "three");
                "unreachable"
            "#,
        );
        let err = err.unwrap_err();
        assert!(err.to_string().contains("prompts per script"), "{err}");
    }
}
