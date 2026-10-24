//! The `initialize`, `auth/login`, and `auth/logout` handlers for v2:
//! negotiation, the capability map, and the credential auth-method
//! advertisement.

use crate::agent::seed as first_run;
use crate::state::TackleState;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v2::{
    AuthMethod, AuthMethodAgent, Implementation, InitializeRequest, InitializeResponse,
    LoginAuthRequest, LoginAuthResponse, LogoutAuthRequest, LogoutAuthResponse,
};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;

/// `initialize`: respond with the v2 protocol version, the required
/// implementation info, the v2 capability map, and the credential auth method
/// when provider credentials are missing.
pub(super) async fn handle_initialize(
    state: Arc<TackleState>,
    _request: InitializeRequest,
    responder: Responder<InitializeResponse>,
) -> Result<(), Error> {
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
