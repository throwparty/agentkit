//! The fixed-order permission pipeline — the only path to tool
//! execution aside from user direct invocation.
//!
//! Order (FR-014): grant-store deny records short-circuit; then the
//! `pre_tool_use` policy script (allow and deny are final, ask falls
//! through, the script sees a `previously_granted` flag); then grant
//! allow records are honoured; then the fixed ask fallback runs
//! regardless of the interactive indicator — the indicator informs
//! scripts only.
//!
//! The grant store is in-memory only, keyed by actor and invokable:
//! grants live for the session's active life in this process and are
//! gone on close, on process exit, and on resume after restart.
//! Nothing about a grant is ever persisted.
//!
//! There is no harness-side timeout and no headless auto-reject: a
//! cancelled or client-timed-out request resolves as reject-once — no
//! grant is written and the next ask asks again.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The policy-script decision source: implemented by the Rhai policy
/// host (T-025). The pipeline treats an absent function, malformed
/// return, or script error — any `None` — as fail-closed to ask.
pub trait PolicyScript: Send + Sync {
    fn pre_tool_use(&self, request: &PolicyRequest) -> Option<PolicyVerdict>;
}

/// The `pre_tool_use(request)` payload, per scripts-api.md.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyRequest {
    /// The namespaced invokable: `mcp.<server>.<tool>`, `prompt.<name>`,
    /// or `agent.<name>`.
    pub invokable: String,
    /// `mcp` | `prompt` | `agent`.
    pub kind: &'static str,
    /// Structured, named arguments.
    pub arguments: Value,
    /// Whether the session is currently user-attended. Informs scripts
    /// only; the pipeline never branches on it.
    pub interactive: bool,
    /// The grant store held an allow record for this invokable.
    pub previously_granted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyVerdict {
    pub decision: Decision,
    /// Surfaces in the permission prompt content on ask.
    pub reason: Option<String>,
}

/// Sends `session/request_permission` for the ask fallback. The ACP
/// layer implements this over the client connection; no timeout is
/// imposed — cancellation is the only non-selection outcome.
pub trait PermissionRequester {
    fn request(
        &self,
        request: PermissionRequest,
    ) -> impl std::future::Future<Output = PermissionChoice> + Send;
}

/// The ask-fallback payload: `ask_reason` carries the script's ask
/// reason (or the fallback's) into the prompt content.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub session_id: String,
    pub invokable: String,
    pub arguments: Value,
    pub ask_reason: Option<String>,
}

/// The user's (or headless client's) outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Cancelled,
}

/// The four options presented on the ask fallback. Labels are honest
/// about scope: grants live for this session in this process only.
pub fn ask_options() -> [(&'static str, PermissionChoice); 4] {
    [
        ("Allow once", PermissionChoice::AllowOnce),
        ("Allow for this session", PermissionChoice::AllowAlways),
        ("Reject once", PermissionChoice::RejectOnce),
        ("Reject for this session", PermissionChoice::RejectAlways),
    ]
}

/// The in-memory grant store, keyed by actor and invokable.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GrantStore {
    grants: BTreeMap<(String, String), GrantRecord>,
}

#[derive(Debug, Clone, PartialEq)]
struct GrantRecord {
    kind: GrantKind,
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantKind {
    Allow,
    Deny,
}

impl GrantStore {
    pub fn allow_always(&mut self, actor: &str, invokable: &str) {
        self.grants.insert(
            (actor.to_owned(), invokable.to_owned()),
            GrantRecord {
                kind: GrantKind::Allow,
                reason: None,
            },
        );
    }

    pub fn deny_always(&mut self, actor: &str, invokable: &str, reason: Option<String>) {
        self.grants.insert(
            (actor.to_owned(), invokable.to_owned()),
            GrantRecord {
                kind: GrantKind::Deny,
                reason,
            },
        );
    }

    fn held(&self, actor: &str, invokable: &str) -> Option<&GrantRecord> {
        self.grants.get(&(actor.to_owned(), invokable.to_owned()))
    }

    /// The session's active life is over: close, process exit, and
    /// resume after restart all start from an empty store.
    pub fn clear(&mut self) {
        self.grants.clear();
    }
}

/// The pipeline's final verdict — ask never escapes; it always
/// resolves through the requester.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Deny { reason: String },
}

/// What the pipeline needs to assess one model-initiated invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub session_id: String,
    pub actor: String,
    pub invokable: String,
    pub kind: &'static str,
    pub arguments: Value,
    pub interactive: bool,
}

/// Runs the fixed-order pipeline over one grant store, one policy
/// source, and one requester.
pub struct PermissionPipeline<'a, P: PermissionRequester> {
    grants: &'a mut GrantStore,
    policy: &'a dyn PolicyScript,
    requester: &'a P,
}

impl<'a, P: PermissionRequester> PermissionPipeline<'a, P> {
    pub fn new(grants: &'a mut GrantStore, policy: &'a dyn PolicyScript, requester: &'a P) -> Self {
        Self {
            grants,
            policy,
            requester,
        }
    }

    pub async fn assess(&mut self, input: &Assessment) -> Verdict {
        // 1. Grant deny records are authoritative and short-circuit.
        if let Some(record) = self.grants.held(&input.actor, &input.invokable) {
            if record.kind == GrantKind::Deny {
                return Verdict::Deny {
                    reason: record
                        .reason
                        .clone()
                        .unwrap_or_else(|| "denied earlier in this session".to_owned()),
                };
            }
        }

        let previously_granted = matches!(
            self.grants.held(&input.actor, &input.invokable),
            Some(record) if record.kind == GrantKind::Allow
        );

        // 2. The policy script: allow and deny are final; ask (and any
        //    script error, absent function, or malformed verdict —
        //    `None`) falls through.
        let policy_verdict = self.policy.pre_tool_use(&PolicyRequest {
            invokable: input.invokable.clone(),
            kind: input.kind,
            arguments: input.arguments.clone(),
            interactive: input.interactive,
            previously_granted,
        });
        let decision = policy_verdict.as_ref().map(|verdict| verdict.decision);
        match decision {
            Some(Decision::Allow) => return Verdict::Allow,
            Some(Decision::Deny) => {
                return Verdict::Deny {
                    reason: policy_verdict
                        .and_then(|verdict| verdict.reason)
                        .unwrap_or_else(|| "denied by policy".to_owned()),
                };
            }
            _ => {} // ask, or fail-closed
        }

        // 3. Grant allow records are honoured.
        if previously_granted {
            return Verdict::Allow;
        }

        // 4. The fixed ask fallback, regardless of the interactive
        //    indicator; the ask reason rides in the prompt content.
        match self
            .requester
            .request(PermissionRequest {
                session_id: input.session_id.clone(),
                invokable: input.invokable.clone(),
                arguments: input.arguments.clone(),
                ask_reason: policy_verdict.and_then(|verdict| verdict.reason),
            })
            .await
        {
            PermissionChoice::AllowAlways => {
                self.grants.allow_always(&input.actor, &input.invokable);
                Verdict::Allow
            }
            PermissionChoice::AllowOnce => Verdict::Allow,
            PermissionChoice::RejectAlways => {
                self.grants
                    .deny_always(&input.actor, &input.invokable, None);
                Verdict::Deny {
                    reason: "rejected for this session".to_owned(),
                }
            }
            // Reject-once: cancelled and client-timed-out requests write
            // nothing and may ask again.
            PermissionChoice::RejectOnce | PermissionChoice::Cancelled => Verdict::Deny {
                reason: "rejected".to_owned(),
            },
        }
    }
}

/// Bridges the TOFU consent flow (T-005) onto `session/request_permission`:
/// trust consent is a permission request of its own, with the project
/// identity and entries named in the prompt content. Consent responses
/// never write the grant store — trust lives in trust.toml, a different
/// record.
pub struct ConsentViaPermission<P: PermissionRequester> {
    pub requester: P,
    pub session_id: String,
}

impl<P: PermissionRequester> crate::config::trust::ConsentPrompt for ConsentViaPermission<P> {
    fn consent(&self, identity: &str, entries: &BTreeSet<String>) -> bool {
        let content = format!(
            "Trust project `{identity}` and adopt its configuration: {}",
            entries.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        // The consent flow runs during config load, outside the async
        // runtime; the sync ConsentPrompt trait bridges by running the
        // request on a local runtime.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("consent runtime");
        runtime.block_on(self.requester.request(PermissionRequest {
            session_id: self.session_id.clone(),
            invokable: "trust".to_owned(),
            arguments: Value::Null,
            ask_reason: Some(content),
        })) == PermissionChoice::AllowOnce
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::trust::ConsentPrompt;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Records every call: the order assertions hang on these counters.
    struct RecorderPolicy {
        verdict: Mutex<Option<PolicyVerdict>>,
        calls: AtomicUsize,
        last_request: Mutex<Option<PolicyRequest>>,
    }

    impl RecorderPolicy {
        fn verdict(decision: Decision) -> Self {
            Self {
                verdict: Mutex::new(Some(PolicyVerdict {
                    decision,
                    reason: (decision == Decision::Ask).then(|| "policy asks".to_owned()),
                })),
                calls: AtomicUsize::new(0),
                last_request: Mutex::new(None),
            }
        }

        fn failing() -> Self {
            Self {
                verdict: Mutex::new(None),
                calls: AtomicUsize::new(0),
                last_request: Mutex::new(None),
            }
        }
    }

    impl PolicyScript for RecorderPolicy {
        fn pre_tool_use(&self, request: &PolicyRequest) -> Option<PolicyVerdict> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().unwrap() = Some(request.clone());
            self.verdict.lock().unwrap().clone()
        }
    }

    struct StubRequester {
        choice: PermissionChoice,
        calls: AtomicUsize,
        last_request: Mutex<Option<PermissionRequest>>,
    }

    impl StubRequester {
        fn choice(choice: PermissionChoice) -> Self {
            Self {
                choice,
                calls: AtomicUsize::new(0),
                last_request: Mutex::new(None),
            }
        }
    }

    impl PermissionRequester for StubRequester {
        async fn request(&self, request: PermissionRequest) -> PermissionChoice {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().unwrap() = Some(request);
            self.choice
        }
    }

    fn assessment() -> Assessment {
        Assessment {
            session_id: "s1".to_owned(),
            actor: "default".to_owned(),
            invokable: "mcp.litterbox.exec".to_owned(),
            kind: "mcp",
            arguments: serde_json::json!({ "command": "cargo test" }),
            interactive: true,
        }
    }

    async fn assess_with(
        grants: &mut GrantStore,
        policy: &RecorderPolicy,
        requester: &StubRequester,
    ) -> Verdict {
        PermissionPipeline::new(grants, policy, requester)
            .assess(&assessment())
            .await
    }

    #[tokio::test]
    async fn deny_records_short_circuit_everything() {
        let mut grants = GrantStore::default();
        grants.deny_always("default", "mcp.litterbox.exec", Some("no shell".into()));
        let policy = RecorderPolicy::verdict(Decision::Allow);
        let requester = StubRequester::choice(PermissionChoice::AllowAlways);

        let verdict = assess_with(&mut grants, &policy, &requester).await;

        assert_eq!(
            verdict,
            Verdict::Deny {
                reason: "no shell".to_owned()
            }
        );
        assert_eq!(policy.calls.load(Ordering::SeqCst), 0);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn script_allow_and_deny_are_final() {
        let mut grants = GrantStore::default();

        let policy = RecorderPolicy::verdict(Decision::Deny);
        let requester = StubRequester::choice(PermissionChoice::AllowAlways);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(
            verdict,
            Verdict::Deny {
                reason: "denied by policy".to_owned()
            }
        );
        assert_eq!(requester.calls.load(Ordering::SeqCst), 0);

        let policy = RecorderPolicy::verdict(Decision::Allow);
        let requester = StubRequester::choice(PermissionChoice::RejectAlways);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 0);
        // A script allow writes nothing to the grant store.
        assert!(grants.held("default", "mcp.litterbox.exec").is_none());
    }

    #[tokio::test]
    async fn script_ask_falls_through_to_allow_records_then_asking() {
        let mut grants = GrantStore::default();

        // With an allow record: ask falls through and the record
        // answers — the client is never asked.
        grants.allow_always("default", "mcp.litterbox.exec");
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::RejectOnce);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 0);

        // Without a record: the client is asked.
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn script_errors_fail_closed_to_ask() {
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::failing();
        let requester = StubRequester::choice(PermissionChoice::AllowAlways);

        let verdict = assess_with(&mut grants, &policy, &requester).await;

        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn previously_granted_reaches_the_script() {
        let mut grants = GrantStore::default();
        grants.allow_always("default", "mcp.litterbox.exec");
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);

        assess_with(&mut grants, &policy, &requester).await;

        let request = policy.last_request.lock().unwrap().clone().unwrap();
        assert!(request.previously_granted);
        assert!(request.interactive);
    }

    #[tokio::test]
    async fn cancelled_is_reject_once_and_writes_nothing() {
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::Cancelled);

        let verdict = assess_with(&mut grants, &policy, &requester).await;

        assert!(matches!(verdict, Verdict::Deny { .. }));
        assert!(grants.held("default", "mcp.litterbox.exec").is_none());

        // Reject-once may ask again.
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(verdict, Verdict::Allow);
    }

    #[tokio::test]
    async fn allow_always_persists_for_the_session_and_is_actor_keyed() {
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::AllowAlways);
        assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

        // Same actor, same invokable: honoured without asking.
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);
        let verdict = assess_with(&mut grants, &policy, &requester).await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 0);

        // A different actor's grant does not authorise this one.
        let mut input = assessment();
        input.actor = "reviewer".to_owned();
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);
        let verdict = PermissionPipeline::new(&mut grants, &policy, &requester)
            .assess(&input)
            .await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

        // A different invokable is likewise unauthorised.
        let mut input = assessment();
        input.invokable = "mcp.other.tool".to_owned();
        let requester = StubRequester::choice(PermissionChoice::AllowOnce);
        let verdict = PermissionPipeline::new(&mut grants, &policy, &requester)
            .assess(&input)
            .await;
        assert_eq!(verdict, Verdict::Allow);
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn grants_are_gone_on_close_exit_and_resume() {
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::AllowAlways);
        assess_with(&mut grants, &policy, &requester).await;

        // Close.
        grants.clear();
        assert!(grants.held("default", "mcp.litterbox.exec").is_none());

        // Process exit and resume-after-restart: a fresh store is
        // empty — grants are in-memory only.
        let fresh = GrantStore::default();
        assert!(fresh.held("default", "mcp.litterbox.exec").is_none());
    }

    #[tokio::test]
    async fn the_fixed_fallback_asks_regardless_of_the_interactive_indicator() {
        let mut input = assessment();
        input.interactive = false;
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::RejectOnce);

        let verdict = PermissionPipeline::new(&mut grants, &policy, &requester)
            .assess(&input)
            .await;

        assert!(matches!(verdict, Verdict::Deny { .. }));
        assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ask_reasons_surface_in_the_request_content() {
        let mut grants = GrantStore::default();
        let policy = RecorderPolicy::verdict(Decision::Ask);
        let requester = StubRequester::choice(PermissionChoice::RejectOnce);

        assess_with(&mut grants, &policy, &requester).await;

        let request = requester.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.invokable, "mcp.litterbox.exec");
        assert_eq!(request.ask_reason.as_deref(), Some("policy asks"));
        assert_eq!(request.arguments, assessment().arguments);
    }

    struct ConsentRequester {
        choice: PermissionChoice,
    }

    impl PermissionRequester for ConsentRequester {
        async fn request(&self, _request: PermissionRequest) -> PermissionChoice {
            self.choice
        }
    }

    #[test]
    fn tofu_consent_rides_request_permission_and_never_writes_grants() {
        let entries = BTreeSet::from(["scripts/deny.rhai".to_owned()]);

        // Explicit consent.
        let consent = ConsentViaPermission {
            requester: ConsentRequester {
                choice: PermissionChoice::AllowOnce,
            },
            session_id: "s1".to_owned(),
        };
        assert!(consent.consent("https://example/repo", &entries));

        // Cancelled consent is refusal — and nothing touched a grant
        // store: trust lives in trust.toml.
        let consent = ConsentViaPermission {
            requester: ConsentRequester {
                choice: PermissionChoice::Cancelled,
            },
            session_id: "s1".to_owned(),
        };
        assert!(!consent.consent("https://example/repo", &entries));
    }
}
