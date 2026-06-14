use crate::config::SwitchboardConfig;
use crate::credential::helper;
use crate::credential::{CredentialSource, OAuthState, ResolvedCredential};
use axum::{extract::Query, response::IntoResponse, routing::get, Router};
use base64::Engine;
use oauth2::PkceCodeChallenge;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

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

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
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
                (
                    axum::http::StatusCode::OK,
                    "Authorization complete. You may close this window.",
                )
                    .into_response()
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{OAUTH_PORT}"))
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

    let token_response = exchange_code(&callback.code, pkce_verifier.secret(), token_url).await?;

    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    let account_id = token_response.account_id.clone();
    let cred = ResolvedCredential {
        value: token_response.access_token.clone(),
        source: CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
        oauth: Some(OAuthState {
            refresh_token: Some(token_response.refresh_token.clone()),
            expires_at: token_response.expires_at,
            account_id,
        }),
    };

    if helper_name == "file" {
        eprintln!("warning: credential helper 'file' stores credentials in plaintext on disk.");
        eprintln!("         consider using 'keychain' for better security.");
    }
    let location = match helper_name {
        "file" => "~/.local/state/agentkit/switchboard/credentials.json",
        "keychain" => "system keychain (service: agentkit-credential-keychain)",
        other => other,
    };
    if helper::store(helper_name, identity, &cred) {
        Ok(format!(
            "✓ Authentication complete.\n  Token stored in {location}.",
        ))
    } else {
        Err(format!(
            "credential helper 'agentkit-credential-{helper_name}' not found.\n  PATH:\n  {}\n  (target: {location})",
            crate::credential::helper::format_path_for_display(),
        ))
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

pub async fn refresh_if_needed(
    identity: &str,
    credential: ResolvedCredential,
    config: &SwitchboardConfig,
) -> Result<ResolvedCredential, String> {
    let Some(oauth) = credential.oauth.as_ref() else {
        return Ok(credential);
    };
    let Some(expires_at) = oauth.expires_at else {
        return Ok(credential);
    };
    if expires_at > chrono::Utc::now() + chrono::Duration::seconds(60) {
        return Ok(credential);
    }

    let refresh_token = oauth.refresh_token.as_deref().ok_or_else(|| {
        format!("credential for '{identity}' is expired and has no refresh token")
    })?;
    let provider = config
        .providers
        .get(identity)
        .ok_or_else(|| format!("provider '{identity}' not found in config"))?;
    let oauth_cfg = provider
        .auth
        .oauth
        .as_ref()
        .ok_or_else(|| format!("provider '{identity}' has no [auth.oauth] config"))?;

    let token_response = exchange_refresh_token(refresh_token, &oauth_cfg.token_url).await?;
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    let refreshed = ResolvedCredential {
        value: token_response.access_token,
        source: CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
        oauth: Some(OAuthState {
            refresh_token: Some(token_response.refresh_token),
            expires_at: token_response.expires_at,
            account_id: token_response
                .account_id
                .or_else(|| oauth.account_id.clone()),
        }),
    };

    if helper::store(helper_name, identity, &refreshed) {
        Ok(refreshed)
    } else {
        Err(format!(
            "failed to store refreshed credential for '{identity}'"
        ))
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
    code_verifier: &str,
    token_url: &str,
) -> Result<TokenResponse, String> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", CLIENT_ID),
        ("code_verifier", code_verifier),
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

    let id_token = body
        .get("id_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing refresh_token in response".to_string())?
        .to_string();

    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs));

    let account_id = id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&access_token));

    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    })
}

async fn exchange_refresh_token(
    refresh_token: &str,
    token_url: &str,
) -> Result<TokenResponse, String> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
    ];

    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {e}"))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse token refresh response: {e}"))?;

    if !status.is_success() {
        let msg = body
            .get("error_description")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error");
        return Err(format!("token refresh failed ({status}): {msg}"));
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing access_token in refresh response".to_string())?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or(refresh_token)
        .to_string();
    let expires_at = body
        .get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs));
    let id_token = body
        .get("id_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let account_id = id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&access_token));

    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    })
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let decoded = engine.decode(parts[1]).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    if let Some(aid) = payload.get("chatgpt_account_id").and_then(|v| v.as_str()) {
        return Some(aid.to_string());
    }
    if let Some(auth) = payload.get("https://api.openai.com/auth") {
        if let Some(aid) = auth.get("chatgpt_account_id").and_then(|v| v.as_str()) {
            return Some(aid.to_string());
        }
    }
    None
}
