//! The `initialize` and `authenticate` handlers: protocol-version
//! negotiation, the credential auth-method advertisement, and the
//! out-of-band login acknowledgement.

use super::negotiation::ConnectionNegotiation;
use super::support::Cx;
use crate::agent::seed as first_run;
use crate::mcp::{DynamicElicitationSink, Elicitation, ElicitationResponse, ElicitationSink};
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodAgent, AuthenticateResponse, CreateElicitationRequest, ElicitationAction,
    ElicitationFormMode, ElicitationId, ElicitationMode, ElicitationRequestScope, ElicitationSchema,
    ElicitationScope, ElicitationUrlMode, Implementation, InitializeRequest, InitializeResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Error, Responder};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// `initialize`: protocol version negotiation (tackle implements v1, so it
/// responds with v1 regardless of the client's latest; a v1-only client
/// proceeds, a v2-only client disconnects) and the auth-method
/// advertisement when provider credentials are missing.
pub(super) async fn handle_initialize(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    elicitation: Arc<DynamicElicitationSink>,
    cx: Cx,
    request: InitializeRequest,
    responder: Responder<InitializeResponse>,
) -> Result<(), Error> {
    negotiation.capture(&request.client_capabilities);
    if request.client_capabilities.elicitation.is_some() {
        elicitation.set(Arc::new(AcpElicitationSink { cx }));
    }
    tracing::info!(protocol = "v1", "ACP protocol negotiated");
    // Missing provider credentials surface as authMethods: the client
    // drives the ACP authenticate method.
    let mut response = InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(state.capabilities())
        .agent_info(Implementation::new("tackle", env!("CARGO_PKG_VERSION")));
    if first_run::missing_credentials(&state) {
        response.auth_methods = vec![AuthMethod::Agent(
            AuthMethodAgent::new("credentials", "Set up model credentials").description(Some(
                "Resolve the configured model endpoints' credentials.".to_owned(),
            )),
        )];
    }
    responder.respond(response)
}

/// `authenticate`: credentials resolve out-of-band (the `tackle auth`
/// subcommand), so this only acknowledges the client's login attempt; the
/// next prompt re-resolves and surfaces any remaining failure.
pub(super) fn handle_authenticate(responder: Responder<AuthenticateResponse>) -> Result<(), Error> {
    responder.respond(AuthenticateResponse::new())
}

/// The v1 `elicitation/create` bridge: MCP elicitations become ACP requests
/// to the client. An unsupported schema or a transport error declines
/// (fail-closed); a cancelled request maps to cancel.
struct AcpElicitationSink {
    cx: Cx,
}

impl ElicitationSink for AcpElicitationSink {
    fn elicit(
        &self,
        elicitation: Elicitation,
    ) -> Pin<Box<dyn Future<Output = ElicitationResponse> + Send + '_>> {
        let cx = self.cx.clone();
        Box::pin(async move {
            let Some(request) = elicitation_request(elicitation) else {
                return ElicitationResponse::Decline;
            };
            match cx.send_request(request).block_task().await {
                Ok(response) => match response.action {
                    ElicitationAction::Accept(_) => ElicitationResponse::Accept,
                    ElicitationAction::Decline => ElicitationResponse::Decline,
                    _ => ElicitationResponse::Cancel,
                },
                Err(_) => ElicitationResponse::Decline,
            }
        })
    }
}

fn elicitation_request(elicitation: Elicitation) -> Option<CreateElicitationRequest> {
    let id = uuid::Uuid::new_v4().to_string();
    let scope = ElicitationScope::Request(ElicitationRequestScope::new(id.clone()));
    let mode = match (elicitation.url, elicitation.requested_schema) {
        (Some(url), _) => ElicitationMode::Url(ElicitationUrlMode::new(
            scope,
            ElicitationId::new(id),
            url,
        )),
        (None, Some(schema)) => {
            let schema: ElicitationSchema = serde_json::from_value(schema).ok()?;
            ElicitationMode::Form(ElicitationFormMode::new(scope, schema))
        }
        _ => return None,
    };
    Some(CreateElicitationRequest::new(mode, elicitation.message))
}
