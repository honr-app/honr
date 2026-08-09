//! Host-mediated OpenShell gateway OIDC login (Authorization Code + PKCE).
//!
//! Operator browser completes the IdP flow; honr exchanges the code on a
//! public board callback (`/oauth/openshell/callback`). Same shape as
//! [`crate::mcp_client_oauth`] — no host `xdg-open`, no `127.0.0.1` listener.
//! Distinct from [`crate::antigravity_oauth`] (Google paste-code).

use crate::secrets::OpenShellOidcBundle;
use crate::store::SharedBoard;
use axum::extract::{Query, State};
use axum::http::{header::HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const CALLBACK_PATH: &str = "/oauth/openshell/callback";
const RETURN_PATH: &str = "/settings/openshell/connectivity";
const PENDING_TTL_SECS: u64 = 600;

/// Default scopes matching the OpenShell CLI when `oidc_scopes` is unset:
/// only `openid`. Extra scopes (profile/email/offline_access/openshell:all)
/// must be assigned on the IdP client or Keycloak returns `invalid_scope`.
const DEFAULT_SCOPES: &[&str] = &["openid"];

fn pending() -> &'static Mutex<HashMap<String, PendingOAuth>> {
    static STORE: OnceLock<Mutex<HashMap<String, PendingOAuth>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone)]
struct PendingOAuth {
    code_verifier: String,
    client_id: String,
    issuer: String,
    /// Exact redirect_uri used at authorize — must match token exchange.
    redirect_uri: String,
    auth_url: String,
    token_url: String,
    created_at: u64,
}

/// Session-gated API under `/api/openshell/oidc`.
pub fn routes() -> Router<SharedBoard> {
    Router::new()
        .route("/login", post(oauth_login))
        .route("/logout", post(oauth_logout))
}

/// Browser callback under `/oauth/openshell/…` (not `/api` — IdP redirect).
pub fn callback_routes() -> Router<SharedBoard> {
    Router::new().route("/callback", get(oauth_callback))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcLoginOut {
    pub authorize_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OidcLogoutOut {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum ApiErr {
    #[error("{0}")]
    Msg(String),
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}

async fn oauth_login(
    State(board): State<SharedBoard>,
    headers: HeaderMap,
) -> Result<Json<OidcLoginOut>, ApiErr> {
    if board.openshell_auth_mode() != Some(crate::model::OpenShellAuthMode::Oidc) {
        return Err(ApiErr::Msg(
            "auth mode must be OIDC before logging in (Settings → OpenShell)".into(),
        ));
    }
    let cfg = board.openshell_oidc_config().unwrap_or_default().trimmed();
    cfg.validate().map_err(ApiErr::Msg)?;

    let origin = crate::mcp_oauth::public_origin(&headers);
    if origin.is_empty() {
        return Err(ApiErr::Msg(
            "cannot resolve public origin (Host / Origin / X-Forwarded-Host, or HONR_PUBLIC_URL)"
                .into(),
        ));
    }
    let redirect_uri = callback_redirect_uri(&origin);

    let discovery = openshell_sdk::oidc::discover(&cfg.issuer, false)
        .await
        .map_err(|e| ApiErr::Msg(format!("OIDC discovery: {e}")))?;

    let client = BasicClient::new(ClientId::new(cfg.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(discovery.authorization_endpoint.clone())
                .map_err(|e| ApiErr::Msg(format!("auth URL: {e}")))?,
        )
        .set_token_uri(
            TokenUrl::new(discovery.token_endpoint.clone())
                .map_err(|e| ApiErr::Msg(format!("token URL: {e}")))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.clone())
                .map_err(|e| ApiErr::Msg(format!("redirect URL: {e}")))?,
        );

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut auth_request = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    for scope in DEFAULT_SCOPES {
        auth_request = auth_request.add_scope(Scope::new((*scope).into()));
    }
    let (mut auth_url, csrf_token) = auth_request.url();
    let audience = cfg.audience.trim();
    if !audience.is_empty() {
        auth_url
            .query_pairs_mut()
            .append_pair("audience", audience);
    }

    let state = csrf_token.secret().clone();
    {
        let mut st = pending().lock();
        st.retain(|_, p| now_secs().saturating_sub(p.created_at) < PENDING_TTL_SECS);
        st.insert(
            state,
            PendingOAuth {
                code_verifier: pkce_verifier.secret().clone(),
                client_id: cfg.client_id.clone(),
                issuer: cfg.issuer.trim_end_matches('/').to_string(),
                redirect_uri: redirect_uri.clone(),
                auth_url: discovery.authorization_endpoint,
                token_url: discovery.token_endpoint,
                created_at: now_secs(),
            },
        );
    }

    Ok(Json(OidcLoginOut {
        authorize_url: auth_url.to_string(),
        redirect_uri,
    }))
}

async fn oauth_logout(State(board): State<SharedBoard>) -> Json<OidcLogoutOut> {
    board.set_openshell_oidc_sealed(None);
    Json(OidcLogoutOut {
        ok: true,
        error: None,
    })
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn oauth_callback(
    State(board): State<SharedBoard>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(err) = q.error.as_deref() {
        let desc = q.error_description.as_deref().unwrap_or(err);
        return Redirect::to(&format!(
            "{RETURN_PATH}?openshell_oidc=error&message={}",
            urlencoding(desc)
        ))
        .into_response();
    }
    let (Some(code), Some(state)) = (q.code.as_deref(), q.state.as_deref()) else {
        return (StatusCode::BAD_REQUEST, "missing code or state").into_response();
    };

    let pending_row = {
        let mut st = pending().lock();
        st.remove(state)
    };
    let Some(p) = pending_row else {
        return Redirect::to(&format!(
            "{RETURN_PATH}?openshell_oidc=error&message=expired_or_unknown_state"
        ))
        .into_response();
    };

    if now_secs().saturating_sub(p.created_at) >= PENDING_TTL_SECS {
        return Redirect::to(&format!(
            "{RETURN_PATH}?openshell_oidc=error&message=login_expired"
        ))
        .into_response();
    }

    match exchange_and_seal(&board, &p, code).await {
        Ok(()) => Redirect::to(&format!("{RETURN_PATH}?openshell_oidc=ok")).into_response(),
        Err(e) => Redirect::to(&format!(
            "{RETURN_PATH}?openshell_oidc=error&message={}",
            urlencoding(&e)
        ))
        .into_response(),
    }
}

async fn exchange_and_seal(
    board: &SharedBoard,
    p: &PendingOAuth,
    code: &str,
) -> Result<(), String> {
    let client = BasicClient::new(ClientId::new(p.client_id.clone()))
        .set_auth_uri(AuthUrl::new(p.auth_url.clone()).map_err(|e| format!("auth URL: {e}"))?)
        .set_token_uri(TokenUrl::new(p.token_url.clone()).map_err(|e| format!("token URL: {e}"))?)
        .set_redirect_uri(
            RedirectUrl::new(p.redirect_uri.clone()).map_err(|e| format!("redirect URL: {e}"))?,
        );

    let http = openshell_sdk::oidc::http_client(false);
    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(p.code_verifier.clone()))
        .request_async(&http)
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?;

    let now = now_secs();
    let expires_at = token_response
        .expires_in()
        .map(|ei| now.saturating_add(ei.as_secs()))
        .unwrap_or(now.saturating_add(3600));
    let refresh = token_response
        .refresh_token()
        .map(|rt| rt.secret().clone())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "IdP did not return a refresh_token (need offline_access)".to_string())?;

    let bundle = OpenShellOidcBundle {
        access_token: token_response.access_token().secret().clone(),
        refresh_token: refresh,
        expires_at,
        issuer: p.issuer.clone(),
        client_id: p.client_id.clone(),
    };
    let sealed = crate::secrets::seal_oidc(&bundle).map_err(|e| e.to_string())?;
    board.set_openshell_oidc_sealed(Some(sealed));
    Ok(())
}

fn callback_redirect_uri(origin: &str) -> String {
    format!("{}{CALLBACK_PATH}", origin.trim_end_matches('/'))
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_redirect_uri_joins_origin() {
        assert_eq!(
            callback_redirect_uri("https://tot.example:5173"),
            "https://tot.example:5173/oauth/openshell/callback"
        );
        assert_eq!(
            callback_redirect_uri("https://tot.example:5173/"),
            "https://tot.example:5173/oauth/openshell/callback"
        );
    }

    #[test]
    fn pending_unknown_state_is_absent() {
        let mut st = pending().lock();
        st.clear();
        assert!(st.remove("no-such-state").is_none());
    }

    #[test]
    fn pending_ttl_evicts_stale() {
        let mut st = pending().lock();
        st.clear();
        st.insert(
            "old".into(),
            PendingOAuth {
                code_verifier: "v".into(),
                client_id: "openshell-cli".into(),
                issuer: "https://idp.example/realms/openshell".into(),
                redirect_uri: "https://board/oauth/openshell/callback".into(),
                auth_url: "https://idp.example/auth".into(),
                token_url: "https://idp.example/token".into(),
                created_at: now_secs().saturating_sub(PENDING_TTL_SECS + 1),
            },
        );
        st.insert(
            "fresh".into(),
            PendingOAuth {
                code_verifier: "v2".into(),
                client_id: "openshell-cli".into(),
                issuer: "https://idp.example/realms/openshell".into(),
                redirect_uri: "https://board/oauth/openshell/callback".into(),
                auth_url: "https://idp.example/auth".into(),
                token_url: "https://idp.example/token".into(),
                created_at: now_secs(),
            },
        );
        st.retain(|_, p| now_secs().saturating_sub(p.created_at) < PENDING_TTL_SECS);
        assert!(st.get("old").is_none());
        assert!(st.get("fresh").is_some());
        st.clear();
    }

    #[test]
    fn urlencoding_encodes_spaces() {
        assert_eq!(urlencoding("a b"), "a%20b");
        assert_eq!(urlencoding("ok-_.~"), "ok-_.~");
    }
}
