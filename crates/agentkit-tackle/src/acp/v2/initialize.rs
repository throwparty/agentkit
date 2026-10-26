//! The `initialize`, `auth/login`, and `auth/logout` handlers for v2:
//! negotiation, the capability map, and the credential auth-method
//! advertisement.

use crate::agent::seed as first_run;
use crate::mcp::{DynamicElicitationSink, Elicitation, ElicitationResponse, ElicitationSink};
use crate::state::TackleState;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AuthMethod, AuthMethodAgent, CreateElicitationRequest, ElicitationAction, ElicitationCapabilities, ElicitationFormMode,
    ElicitationId, ElicitationMode, ElicitationRequestScope, ElicitationSchema, ElicitationScope,
    ElicitationUrlMode, Implementation, InitializeRequest, InitializeResponse, LoginAuthRequest,
    LoginAuthResponse, LogoutAuthRequest, LogoutAuthResponse,
};
use agent_client_protocol::{Error, Responder, V2ConnectionTo};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// `initialize`: respond with the v2 protocol version, the required
/// implementation info, the v2 capability map, and the credential auth method
/// when provider credentials are missing.
pub(super) async fn handle_initialize(
    state: Arc<TackleState>,
    elicitation: Arc<DynamicElicitationSink>,
    cx: V2ConnectionTo<Client>,
    request: InitializeRequest,
    responder: Responder<InitializeResponse>,
) -> Result<(), Error> {
    tracing::info!(protocol = "v2", "ACP protocol negotiated");
    if let Some(modes) = request.capabilities.elicitation.clone() {
        elicitation.set(Arc::new(AcpElicitationSink { modes, cx }));
    }
    let mut response = InitializeResponse::new(
        ProtocolVersion::V2,
        Implementation::new("tackle", env!("CARGO_PKG_VERSION")),
    )
    .capabilities(state.v2_capabilities());
    if first_run::missing_credentials(&state) {
        response = response.auth_methods(vec![AuthMethod::Agent(
            AuthMethodAgent::new("credentials", "Set up model credentials").description(Some(
                "Resolve the configured model endpoints' credentials.".to_owned(),
            )),
        )]);
    }
    responder.respond(response)
}

/// `auth/login`: credentials resolve out-of-band (the `tackle auth`
/// subcommand), so this only acknowledges the client's login attempt.
pub(super) fn handle_login(
    _request: LoginAuthRequest,
    responder: Responder<LoginAuthResponse>,
) -> Result<(), Error> {
    responder.respond(LoginAuthResponse::new())
}

/// `auth/logout`: nothing is held in-protocol, so this acknowledges the
/// client's logout attempt.
pub(super) fn handle_logout(
    _request: LogoutAuthRequest,
    responder: Responder<LogoutAuthResponse>,
) -> Result<(), Error> {
    responder.respond(LogoutAuthResponse::new())
}

/// The v2 `elicitation/create` bridge: MCP elicitations become ACP requests
/// to the client. An unsupported schema or a transport error declines
/// (fail-closed); a cancelled request maps to cancel.
struct AcpElicitationSink {
    modes: ElicitationCapabilities,
    cx: V2ConnectionTo<Client>,
}

impl ElicitationSink for AcpElicitationSink {
    fn elicit(
        &self,
        elicitation: Elicitation,
    ) -> Pin<Box<dyn Future<Output = ElicitationResponse> + Send + '_>> {
        let cx = self.cx.clone();
        Box::pin(async move {
            let Some(request) = elicitation_request(elicitation, &self.modes) else {
                return ElicitationResponse::Decline;
            };
            match cx.send_request(request).block_task().await {
                Ok(response) => match response.action {
                    ElicitationAction::Accept(accept) => ElicitationResponse::Accept {
                        content: accept.content.map(|values| {
                            serde_json::Value::Object(
                                values
                                    .into_iter()
                                    .map(|(key, value)| {
                                        (
                                            key,
                                            serde_json::to_value(value)
                                                .unwrap_or(serde_json::Value::Null),
                                        )
                                    })
                                    .collect(),
                            )
                        }),
                    },
                    ElicitationAction::Decline => ElicitationResponse::Decline,
                    _ => ElicitationResponse::Cancel,
                },
                Err(_) => ElicitationResponse::Decline,
            }
        })
    }
}

fn elicitation_request(
    elicitation: Elicitation,
    modes: &ElicitationCapabilities,
) -> Option<CreateElicitationRequest> {
    let id = uuid::Uuid::new_v4().to_string();
    let scope = ElicitationScope::Request(ElicitationRequestScope::new(id.clone()));
    let mode = match (elicitation.url, elicitation.requested_schema) {
        (Some(url), _) if modes.supports_url() => ElicitationMode::Url(ElicitationUrlMode::new(
            scope,
            ElicitationId::new(id),
            url,
        )),
        (None, Some(schema)) if modes.supports_form() => {
            let schema: ElicitationSchema = serde_json::from_value(schema).ok()?;
            ElicitationMode::Form(ElicitationFormMode::new(scope, schema))
        }
        _ => return None,
    };
    Some(CreateElicitationRequest::new(mode, elicitation.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v2::{
        ElicitationFormCapabilities, ElicitationUrlCapabilities,
    };

    fn form_request() -> Elicitation {
        Elicitation {
            server: "echo".to_owned(),
            message: "Name".to_owned(),
            requested_schema: Some(serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            })),
            url: None,
        }
    }

    fn url_request() -> Elicitation {
        Elicitation {
            server: "echo".to_owned(),
            message: "Open".to_owned(),
            requested_schema: None,
            url: Some("https://example.com".to_owned()),
        }
    }

    #[test]
    fn url_is_declined_when_only_form_is_advertised() {
        let modes = ElicitationCapabilities::new().form(ElicitationFormCapabilities::default());
        assert!(elicitation_request(url_request(), &modes).is_none());
    }

    #[test]
    fn form_is_declined_when_only_url_is_advertised() {
        let modes = ElicitationCapabilities::new().url(ElicitationUrlCapabilities::default());
        assert!(elicitation_request(form_request(), &modes).is_none());
    }

    #[test]
    fn empty_capabilities_decline_both_modes() {
        let modes = ElicitationCapabilities::new();
        assert!(elicitation_request(form_request(), &modes).is_none());
        assert!(elicitation_request(url_request(), &modes).is_none());
    }

    #[test]
    fn advertised_modes_are_honoured() {
        let both = ElicitationCapabilities::new()
            .form(ElicitationFormCapabilities::default())
            .url(ElicitationUrlCapabilities::default());
        assert!(elicitation_request(form_request(), &both).is_some());
        assert!(elicitation_request(url_request(), &both).is_some());
    }
}
