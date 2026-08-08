//! Host-mediated OpenShell gateway OIDC login (Authorization Code + PKCE).
//!
//! Mirrors the OpenShell CLI browser flow: bind `http://127.0.0.1:{ephemeral}/callback`,
//! open the IdP authorize URL, exchange the code, seal tokens into the board DB.
//! Distinct from [`crate::antigravity_oauth`] (Google / agy provider) and
//! [`crate::mcp_client_oauth`] (outbound MCP servers).

use crate::secrets::OpenShellOidcBundle;
use crate::store::SharedBoard;
use axum::extract::{Query, State as AxState};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};

const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

/// Default scopes matching the OpenShell CLI when `oidc_scopes` is unset:
/// only `openid`. Extra scopes (profile/email/offline_access/openshell:all)
/// must be assigned on the IdP client or Keycloak returns `invalid_scope`.
const DEFAULT_SCOPES: &[&str] = &["openid"];

pub fn routes() -> Router<SharedBoard> {
    Router::new()
        .route("/login", post(oauth_login))
        .route("/logout", post(oauth_logout))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OidcLoginOut {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum ApiErr {
    #[error("{0}")]
    Msg(String),
}

impl axum::response::IntoResponse for ApiErr {
    fn into_response(self) -> axum::response::Response {
        let body = OidcLoginOut {
            ok: false,
            error: Some(self.to_string()),
        };
        (axum::http::StatusCode::BAD_REQUEST, Json(body)).into_response()
    }
}

async fn oauth_login(AxState(board): AxState<SharedBoard>) -> Result<Json<OidcLoginOut>, ApiErr> {
    if board.openshell_auth_mode() != Some(crate::model::OpenShellAuthMode::Oidc) {
        return Err(ApiErr::Msg(
            "auth mode must be OIDC before logging in (Settings → OpenShell)".into(),
        ));
    }
    let cfg = board.openshell_oidc_config().unwrap_or_default();
    cfg.validate().map_err(ApiErr::Msg)?;
    let audience = cfg.audience.trim();
    let audience = (!audience.is_empty()).then_some(audience);

    let bundle = browser_auth_flow(&cfg.issuer, &cfg.client_id, audience, false)
        .await
        .map_err(ApiErr::Msg)?;

    let sealed = crate::secrets::seal_oidc(&bundle).map_err(|e| ApiErr::Msg(e.to_string()))?;
    board.set_openshell_oidc_sealed(Some(sealed));
    Ok(Json(OidcLoginOut {
        ok: true,
        error: None,
    }))
}

async fn oauth_logout(AxState(board): AxState<SharedBoard>) -> Json<OidcLoginOut> {
    board.set_openshell_oidc_sealed(None);
    Json(OidcLoginOut {
        ok: true,
        error: None,
    })
}

/// Authorization Code + PKCE against the gateway's IdP (same shape as OpenShell CLI).
pub async fn browser_auth_flow(
    issuer: &str,
    client_id: &str,
    audience: Option<&str>,
    insecure: bool,
) -> Result<OpenShellOidcBundle, String> {
    let discovery = openshell_sdk::oidc::discover(issuer, insecure)
        .await
        .map_err(|e| format!("OIDC discovery: {e}"))?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("bind OIDC callback: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("callback addr: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let client = BasicClient::new(ClientId::new(client_id.to_string()))
        .set_auth_uri(
            AuthUrl::new(discovery.authorization_endpoint)
                .map_err(|e| format!("auth URL: {e}"))?,
        )
        .set_token_uri(
            TokenUrl::new(discovery.token_endpoint).map_err(|e| format!("token URL: {e}"))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri).map_err(|e| format!("redirect URL: {e}"))?,
        );

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut auth_request = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    for scope in DEFAULT_SCOPES {
        auth_request = auth_request.add_scope(Scope::new((*scope).into()));
    }
    let (mut auth_url, csrf_token) = auth_request.url();
    if let Some(aud) = audience {
        auth_url.query_pairs_mut().append_pair("audience", aud);
    }

    let (tx, rx) = oneshot::channel::<Result<String, String>>();
    let expected_state = csrf_token.secret().clone();
    let server_handle = tokio::spawn(run_callback_server(listener, tx, expected_state));

    if let Err(e) = open_browser(auth_url.as_str()) {
        tracing::warn!(error = %e, %auth_url, "openshell oidc: open this URL to authenticate");
    }

    let code = tokio::select! {
        result = rx => {
            result.map_err(|_| "OIDC callback channel closed".to_string())??
        }
        () = tokio::time::sleep(AUTH_TIMEOUT) => {
            server_handle.abort();
            return Err(format!(
                "OIDC login timed out after {}s — try Log in again",
                AUTH_TIMEOUT.as_secs()
            ));
        }
    };
    server_handle.abort();

    let http = openshell_sdk::oidc::http_client(insecure);
    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http)
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expires_at = token_response
        .expires_in()
        .map(|ei| now.saturating_add(ei.as_secs()))
        .unwrap_or(now.saturating_add(3600));
    let refresh = token_response
        .refresh_token()
        .map(|rt| rt.secret().clone())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "IdP did not return a refresh_token (need offline_access)".to_string())?;

    Ok(OpenShellOidcBundle {
        access_token: token_response.access_token().secret().clone(),
        refresh_token: refresh,
        expires_at,
        issuer: issuer.trim_end_matches('/').to_string(),
        client_id: client_id.to_string(),
    })
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("no browser launcher on this platform".into())
    }
}

type CallbackTx = oneshot::Sender<Result<String, String>>;

#[derive(Clone)]
struct CallbackState {
    expected_state: String,
    tx: Arc<Mutex<Option<CallbackTx>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn run_callback_server(
    listener: TcpListener,
    tx: CallbackTx,
    expected_state: String,
) {
    let state = CallbackState {
        expected_state,
        tx: Arc::new(Mutex::new(Some(tx))),
    };
    let app = Router::new()
        .route("/callback", get(callback_handler))
        .with_state(state);
    let _ = axum::serve(listener, app).await;
}

async fn callback_handler(
    AxState(state): AxState<CallbackState>,
    Query(q): Query<CallbackQuery>,
) -> Html<&'static str> {
    let result = if let Some(err) = q.error {
        Err(format!("OIDC error: {err}"))
    } else if q.state.as_deref() != Some(state.expected_state.as_str()) {
        Err("state mismatch".into())
    } else if let Some(code) = q.code {
        Ok(code)
    } else {
        Err("missing code".into())
    };
    if let Some(sender) = state.tx.lock().await.take() {
        let _ = sender.send(result);
    }
    Html("<!doctype html><html><body><p>OpenShell login complete. You can close this tab.</p></body></html>")
}
