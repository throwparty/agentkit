//! Permission pipeline tests: order, veto, fail-closed, headless, and timeout.

use super::*;
use crate::config::trust::ConsentPrompt;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn request(
        &self,
        request: PermissionRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionChoice> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().unwrap() = Some(request);
        let choice = self.choice;
        Box::pin(async move { choice })
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
        tool_call_id: None,
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
    grants.deny_always(
        "s1",
        "default",
        "mcp.litterbox.exec",
        Some("no shell".into()),
    );
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
    assert!(grants.held("s1", "default", "mcp.litterbox.exec").is_none());
}

#[tokio::test]
async fn script_ask_falls_through_to_allow_records_then_asking() {
    let mut grants = GrantStore::default();

    // With an allow record: ask falls through and the record
    // answers — the client is never asked.
    grants.allow_always("s1", "default", "mcp.litterbox.exec");
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
    grants.allow_always("s1", "default", "mcp.litterbox.exec");
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
    assert!(grants.held("s1", "default", "mcp.litterbox.exec").is_none());

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
    assert!(grants.held("s1", "default", "mcp.litterbox.exec").is_none());

    // Process exit and resume-after-restart: a fresh store is
    // empty — grants are in-memory only.
    let fresh = GrantStore::default();
    assert!(fresh.held("s1", "default", "mcp.litterbox.exec").is_none());
}

#[tokio::test]
async fn closing_a_session_clears_only_its_grants() {
    let mut grants = GrantStore::default();
    let policy = RecorderPolicy::verdict(Decision::Ask);

    let requester = StubRequester::choice(PermissionChoice::AllowAlways);
    assess_with(&mut grants, &policy, &requester).await;

    let mut other = assessment();
    other.session_id = "s2".to_owned();
    let requester = StubRequester::choice(PermissionChoice::AllowAlways);
    PermissionPipeline::new(&mut grants, &policy, &requester)
        .assess(&other)
        .await;

    grants.clear_session("s1");
    assert!(grants.held("s1", "default", "mcp.litterbox.exec").is_none());
    assert!(
        grants.held("s2", "default", "mcp.litterbox.exec").is_some(),
        "another session's grant survives"
    );
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
    fn request(
        &self,
        _request: PermissionRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PermissionChoice> + Send + '_>> {
        let choice = self.choice;
        Box::pin(async move { choice })
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
