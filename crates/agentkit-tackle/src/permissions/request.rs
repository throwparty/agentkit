//! The ask fallback: the requester trait, the permission-request payload, the user's choice, and the presented options.

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Sends `session/request_permission` for the ask fallback. The ACP
/// layer implements this over the client connection; no timeout is
/// imposed — cancellation is the only non-selection outcome. Object-safe
/// so the core loop can hold a `&dyn PermissionRequester`.
pub trait PermissionRequester: Send + Sync {
    fn request(
        &self,
        request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = PermissionChoice> + Send + '_>>;
}

/// The ask-fallback payload: `ask_reason` carries the script's ask
/// reason (or the fallback's) into the prompt content. `tool_call_id`
/// correlates the ask with the in-flight tool call it authorises, when
/// there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub session_id: String,
    pub invokable: String,
    pub arguments: Value,
    pub ask_reason: Option<String>,
    pub tool_call_id: Option<String>,
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
