//! The `SessionAccess` implementation over the store: the acting surface behaviour scripts drive.

use super::api::{
    Completion, CompletionSink, ContextUsage, HistoryRow, HostError, PromptDispatcher,
    SessionAccess, block_on, estimate_tokens,
};
use super::budgets::{COMPLETION_POLL, HISTORY_MAX_BYTES, HISTORY_MAX_ROWS};
use crate::store::{Role, SessionId, SessionKind, SessionStore, TurnId, TurnKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct StoreAccess {
    pub(super) store: Arc<SessionStore>,
    dispatcher: Arc<dyn PromptDispatcher>,
    completions: Arc<dyn CompletionSink>,
    /// The model context window size, from agentkit-models metadata.
    pub window_size: u64,
    /// Live ephemeral forks per parent, process-local.
    pub live_forks: Arc<Mutex<HashMap<SessionId, Vec<SessionId>>>>,
}

impl StoreAccess {
    pub fn new(
        store: Arc<SessionStore>,
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
            Ok::<(), HostError>(())
        })?;
        // The dispatcher drives the turn through the agent loop. It runs
        // its own runtime entry (a fresh `block_on`, not a nested one),
        // so the message is persisted first.
        self.dispatcher.dispatch(session, text)
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
        let used_bytes: usize = assembled
            .iter()
            .map(|item| item.message.content.len())
            .sum();
        Ok(ContextUsage {
            used: estimate_tokens(used_bytes as u64),
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

