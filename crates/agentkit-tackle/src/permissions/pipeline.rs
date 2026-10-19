//! The fixed-order permission pipeline and the trust-consent bridge.

use super::grants::{GrantKind, GrantStore};
use super::policy::{Decision, PolicyRequest, PolicyScript};
use super::request::{PermissionChoice, PermissionRequest, PermissionRequester};
use serde_json::Value;
use std::collections::BTreeSet;


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
        if let Some(record) = self
            .grants
            .held(&input.session_id, &input.actor, &input.invokable)
        {
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
            self.grants
                .held(&input.session_id, &input.actor, &input.invokable),
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
                self.grants
                    .allow_always(&input.session_id, &input.actor, &input.invokable);
                Verdict::Allow
            }
            PermissionChoice::AllowOnce => Verdict::Allow,
            PermissionChoice::RejectAlways => {
                self.grants
                    .deny_always(&input.session_id, &input.actor, &input.invokable, None);
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

/// Bridges the TOFU consent flow onto `session/request_permission`:
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

