//! The `initialize` and `authenticate` handlers: protocol-version
//! negotiation, the credential auth-method advertisement, and the
//! out-of-band login acknowledgement.

use super::negotiation::ConnectionNegotiation;
use crate::agent::seed as first_run;
use crate::state::TackleState;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodAgent, AuthenticateResponse, InitializeRequest, InitializeResponse,
};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;

/// `initialize`: protocol version negotiation (tackle implements v1, so it
/// responds with v1 regardless of the client's latest; a v1-only client
/// proceeds, a v2-only client disconnects) and the auth-method
/// advertisement when provider credentials are missing.
pub(super) async fn handle_initialize(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    request: InitializeRequest,
    responder: Responder<InitializeResponse>,
) -> Result<(), Error> {
    negotiation.capture(&request.client_capabilities);
    // Missing provider credentials surface as authMethods: the client
    // drives the ACP authenticate method.
    let mut response =
        InitializeResponse::new(ProtocolVersion::V1).agent_capabilities(state.capabilities());
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
