//! The v2 `session/request_permission` bridge: the `PermissionRequester` the
//! permission pipeline asks through, plus the `requires_action`/`running`
//! foreground transitions around a blocked prompt.

use crate::invokables::titles;
use crate::permissions::{PermissionChoice, PermissionRequest, PermissionRequester};
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    Content, ContentBlock, PermissionOption, PermissionOptionId, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionSubject,
    RequiresActionStateUpdate, RunningStateUpdate, SessionId, StateUpdate, TextContent,
    ToolCallContent, ToolCallId, ToolCallUpdate,
};
use agent_client_protocol::V2ConnectionTo;
use std::future::Future;
use std::pin::Pin;

/// The v2 `session/request_permission` bridge.
pub(super) struct AcpPermissionRequester {
    pub cx: V2ConnectionTo<Client>,
    pub session_id: SessionId,
}

fn option_id(choice: PermissionChoice) -> &'static str {
    use PermissionChoice as C;
    match choice {
        C::AllowOnce => "allow_once",
        C::AllowAlways => "allow_always",
        C::RejectOnce => "reject_once",
        C::RejectAlways => "reject_always",
        C::Cancelled => "cancelled",
    }
}

fn option_kind(choice: PermissionChoice) -> PermissionOptionKind {
    use PermissionChoice as C;
    match choice {
        C::AllowOnce => PermissionOptionKind::AllowOnce,
        C::AllowAlways => PermissionOptionKind::AllowAlways,
        C::RejectOnce => PermissionOptionKind::RejectOnce,
        C::RejectAlways => PermissionOptionKind::RejectAlways,
        C::Cancelled => PermissionOptionKind::RejectOnce,
    }
}

impl PermissionRequester for AcpPermissionRequester {
    fn request(
        &self,
        request: PermissionRequest,
    ) -> Pin<Box<dyn Future<Output = PermissionChoice> + Send + '_>> {
        let choices = crate::permissions::ask_options();
        let options: Vec<PermissionOption> = choices
            .iter()
            .map(|(label, choice)| {
                PermissionOption::new(
                    PermissionOptionId::new(option_id(*choice)),
                    (*label).to_owned(),
                    option_kind(*choice),
                )
            })
            .collect();
        let title = titles::tool_call_title(&request.invokable, &request.arguments);
        let tool_call_id = request
            .tool_call_id
            .clone()
            .unwrap_or_else(|| format!("ask-{}", request.invokable));
        let mut tool_call = ToolCallUpdate::new(ToolCallId::new(tool_call_id))
            .name(request.invokable.clone())
            .title(title.clone())
            .kind(super::sink::wire_kind(titles::tool_kind(&request.invokable)));
        if let Some(reason) = &request.ask_reason {
            tool_call = tool_call.content(vec![ToolCallContent::Content(Box::new(Content::new(
                ContentBlock::Text(TextContent::new(reason.clone())),
            )))]);
        }
        let acp_request = RequestPermissionRequest::new(self.session_id.clone(), title, options)
            .subject(RequestPermissionSubject::from(tool_call));
        let cx = self.cx.clone();
        let session_id = self.session_id.clone();
        Box::pin(async move {
            super::send_state(
                &cx,
                &session_id,
                StateUpdate::RequiresAction(RequiresActionStateUpdate::new()),
            );
            let outcome = cx.send_request(acp_request).block_task().await;
            match outcome {
                Ok(response) => match response.outcome {
                    RequestPermissionOutcome::Selected(selected) => {
                        let id = selected.option_id.0.to_string();
                        let choice = choices
                            .iter()
                            .find(|(_, choice)| option_id(*choice) == id)
                            .map(|(_, choice)| *choice)
                            .unwrap_or(PermissionChoice::Cancelled);
                        if choice != PermissionChoice::Cancelled {
                            super::send_state(
                                &cx,
                                &session_id,
                                StateUpdate::Running(RunningStateUpdate::new()),
                            );
                        }
                        choice
                    }
                    _ => PermissionChoice::Cancelled,
                },
                Err(_) => PermissionChoice::Cancelled,
            }
        })
    }
}
