//! The ask fallback: the requester trait, the permission-request payload, the user's choice, and the presented options.

use serde_json::Value;

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
