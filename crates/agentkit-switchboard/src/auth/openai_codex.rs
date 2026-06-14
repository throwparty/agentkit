use axum::{extract::Query, response::IntoResponse, routing::get, Router};
use oauth2::PkceCodeChallenge;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;
use crate::config::SwitchboardConfig;
use crate::credential::helper;
use crate::credential::{CredentialSource, OAuthState, ResolvedCredential};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OAUTH_PORT: u16 = 1455;

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

pub async fn login(identity: &str, config: &SwitchboardConfig) -> Result<String, String> {
    let provider = config
        .providers
        .get(identity)
        .ok_or_else(|| format!("provider '{identity}' not found in config"))?;

    let oauth_cfg = provider
        .auth
        .oauth
        .as_ref()
        .ok_or_else(|| format!("provider '{identity}' has no [auth.oauth] config"))?;

    let authorize_url = &oauth_cfg.authorize_url;
    let token_url = &oauth_cfg.token_url;
    let scopes = oauth_cfg.scopes.as_deref().unwrap_or("openid email");

    let (pkce_challenge, _pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let state = uuid::Uuid::new_v4().to_string();

    let (tx, rx) = oneshot::channel::<CallbackParams>();
    let shared_tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let app = Router::new().route(
        "/auth/callback",
        get({
            let tx = shared_tx.clone();
            move |Query(params): Query<CallbackParams>| async move {
                if let Some(tx) = tx.lock().await.take() {
                    let _ = tx.send(params);
                }
                (axum::http::StatusCode::OK, "Authorization complete. You may close this window.").into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{OAUTH_PORT}"))
        .await
        .map_err(|e| format!("failed to bind to port {OAUTH_PORT}: {e}"))?;

    let shutdown_tx = Arc::new(tokio::sync::Notify::new());
    let shutdown_rx = shutdown_tx.clone();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown_rx.notified().await })
            .await
            .ok();
    });

    let code_challenge = pkce_challenge.as_str().to_string();
    let code_challenge_method = "S256";

    let params: HashMap<&str, String> = [
        ("response_type", "code".into()),
        ("client_id", CLIENT_ID.into()),
        ("redirect_uri", REDIRECT_URI.into()),
        ("scope", scopes.into()),
        ("state", state.clone()),
        ("code_challenge", code_challenge),
        ("code_challenge_method", code_challenge_method.into()),
        ("id_token_add_organizations", "true".into()),
        ("codex_cli_simplified_flow", "true".into()),
        ("originator", "agentkit-switchboard".into()),
    ]
    .into_iter()
    .collect();

    let url = reqwest::Url::parse_with_params(authorize_url, &params)
        .map_err(|e| format!("failed to build authorize URL: {e}"))?;

    open::that(url.as_str()).map_err(|e| format!("failed to open browser: {e}"))?;

    let callback = tokio::time::timeout(std::time::Duration::from_secs(300), rx)
        .await
        .map_err(|_| "timeout waiting for OAuth callback (300s)".to_string())?
        .map_err(|_| "OAuth callback channel closed".to_string())?;

    if callback.state != state {
        return Err("OAuth state mismatch — possible CSRF attack".to_string());
    }

    shutdown_tx.notify_one();

    let token_response = exchange_code(&callback.code, token_url).await?;

    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    let cred = ResolvedCredential {
        value: token_response.access_token.clone(),
        source: CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
        oauth: Some(OAuthState {
            refresh_token: Some(token_response.refresh_token.clone()),
            expires_at: token_response.expires_at,
        }),
    };

    if helper::store(helper_name, identity, &cred) {
        Ok(format!(
            "✓ Authentication complete.\n  Token stored via agentkit-credential-{helper_name}.\n  For CI/script use, run: switchboard auth token {identity}\n  Account ID: {}",
            token_response.account_id.unwrap_or_default()
        ))
    } else {
        Err("failed to store credential via helper".to_string())
    }
}

pub async fn logout(identity: &str, config: &SwitchboardConfig) -> Result<String, String> {
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    if helper::erase(helper_name, identity) {
        Ok(format!("✓ Credentials removed for '{identity}'"))
    } else {
        Err(format!("failed to erase credential for '{identity}'"))
    }
}

struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    account_id: Option<String>,
}

async fn exchange_code(
    code: &str,
    token_url: &str,
) -> Result<TokenResponse, String> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", CLIENT_ID),
    ];

    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse token response: {e}"))?;

    if !status.is_success() {
        let msg = body
            .get("error_description")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        return Err(format!("token exchange failed ({status}): {msg}"));
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing access_token in response".to_string())?
        .to_string();

    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing refresh_token in response".to_string())?
        .to_string();

    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs));

    let account_id = extract_account_id_from_jwt(&body);

    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    })
}

fn extract_account_id_from_jwt(body: &serde_json::Value) -> Option<String> {
    for token_field in &["id_token", "access_token"] {
        if let Some(token) = body.get(*token_field).and_then(|v| v.as_str()) {
            let parts: Vec<&str> = token.split('.').collect();
            if parts.len() == 3 {
                if let Ok(decoded) = decode_jwt_payload(parts[1]) {
                    if let Some(aid) = decoded
                        .get("chatgpt_account_id")
                        .and_then(|v| v.as_str())
                    {
                        return Some(aid.to_string());
                    }
                    if let Some(auth) = decoded
                        .get("https://api.openai.com/auth")
                        .and_then(|v| v.get("chatgpt_account_id"))
                        .and_then(|v| v.as_str())
                    {
                        return Some(auth.to_string());
                    }
                    if let Some(orgs) = decoded.get("organizations").and_then(|v| v.as_array()) {
                        if let Some(first) = orgs.first() {
                            if let Some(oid) = first.get("id").and_then(|v| v.as_str()) {
                                return Some(oid.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn decode_jwt_payload(payload: &str) -> Result<serde_json::Value, String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decoded = engine
        .decode(payload)
        .map_err(|e| format!("base64 decode failed: {e}"))?;
    serde_json::from_slice(&decoded).map_err(|e| format!("JSON parse failed: {e}"))
}
