//! The ACP `session/request_permission` bridge: the PermissionRequester the permission pipeline asks through.

use super::wire_kind;
use agent_client_protocol::schema::v1::SessionId;

/// The ACP `session/request_permission` bridge.
///
/// The in-flight tool call id rides on the request (see
/// [`crate::permissions::PermissionRequest::tool_call_id`]), so clients
/// (Zed upserts tool calls by id) attach the permission prompt to the
/// existing card rather than inventing a new one; a request with no tool
/// call behind it (project trust consent) falls back to a synthetic,
/// per-invokable id.
pub(super) struct AcpPermissionRequester {
    pub(super) cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    pub(super) session_id: SessionId,
}

pub(super) fn permission_option_id(choice: crate::permissions::PermissionChoice) -> &'static str {
    use crate::permissions::PermissionChoice as C;
    match choice {
        C::AllowOnce => "allow_once",
        C::AllowAlways => "allow_always",
        C::RejectOnce => "reject_once",
        C::RejectAlways => "reject_always",
        C::Cancelled => "cancelled",
    }
}

pub(super) fn permission_option_kind(
    choice: crate::permissions::PermissionChoice,
) -> agent_client_protocol::schema::v1::PermissionOptionKind {
    use crate::permissions::PermissionChoice as C;
    use agent_client_protocol::schema::v1::PermissionOptionKind as K;
    match choice {
        C::AllowOnce => K::AllowOnce,
        C::AllowAlways => K::AllowAlways,
        C::RejectOnce => K::RejectOnce,
        C::RejectAlways => K::RejectAlways,
        C::Cancelled => K::RejectOnce,
    }
}

impl crate::permissions::PermissionRequester for AcpPermissionRequester {
    fn request(
        &self,
        request: crate::permissions::PermissionRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = crate::permissions::PermissionChoice> + Send + '_>,
    > {
        use agent_client_protocol::schema::v1;
        let choices = crate::permissions::ask_options();
        let options: Vec<v1::PermissionOption> = choices
            .iter()
            .map(|(label, choice)| {
                v1::PermissionOption::new(
                    permission_option_id(*choice),
                    (*label).to_owned(),
                    permission_option_kind(*choice),
                )
            })
            .collect();
        let mut fields = v1::ToolCallUpdateFields::new()
            .name(request.invokable.clone())
            .title(crate::invokables::titles::tool_call_title(
                &request.invokable,
                &request.arguments,
            ))
            .kind(wire_kind(crate::invokables::titles::tool_kind(
                &request.invokable,
            )));
        if let Some(reason) = &request.ask_reason {
            fields = fields.content(vec![v1::ToolCallContent::from(v1::ContentBlock::Text(
                v1::TextContent::new(reason.clone()),
            ))]);
        }
        // The real call id when one exists, so the client updates the
        // tool-call card it already announced; otherwise a synthetic id
        // (trust consent has no model tool call).
        let tool_call_id = request
            .tool_call_id
            .clone()
            .unwrap_or_else(|| format!("ask-{}", request.invokable));
        let tool_call = v1::ToolCallUpdate::new(v1::ToolCallId::new(tool_call_id), fields);
        let acp_request =
            v1::RequestPermissionRequest::new(self.session_id.clone(), tool_call, options);
        let cx = self.cx.clone();
        Box::pin(async move {
            match cx.send_request(acp_request).block_task().await {
                Ok(response) => match response.outcome {
                    v1::RequestPermissionOutcome::Selected(selected) => {
                        let id = selected.option_id.0.to_string();
                        choices
                            .iter()
                            .find(|(_, choice)| permission_option_id(*choice) == id)
                            .map(|(_, choice)| *choice)
                            .unwrap_or(crate::permissions::PermissionChoice::Cancelled)
                    }
                    _ => crate::permissions::PermissionChoice::Cancelled,
                },
                Err(_) => crate::permissions::PermissionChoice::Cancelled,
            }
        })
    }
}
