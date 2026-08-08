//! Host-mediated Google OAuth for the Antigravity (`agy`) OpenShell provider.
//!
//! Distinct from [`crate::mcp_client_oauth`] (outbound MCP servers). Here honr
//! is the OAuth client for Google's Antigravity / Cloud Code installed-app
//! client: PKCE + offline access → seal refresh on board provider `antigravity`
//! → gateway `oauth2_refresh_token` so the seat only sees `openshell:resolve:…`.

use crate::antigravity::{self, CONFIG_LOCATION, CONFIG_PROJECT};
use crate::model::{
    OpenShellProviderDesired, OpenShellProviderRefreshDesired, OpenShellProviderTypeDesired,
    CockpitSessionStatus, ANTIGRAVITY_PROVIDER,
};
use crate::provider_types;
use crate::secrets::{open_string_map, seal_string_map};
use crate::store::SharedBoard;
use crate::supervisor::setup_agy_auth;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const CALLBACK_PATH: &str = "/oauth/antigravity/callback";
const PENDING_TTL_SECS: u64 = 600;
const DEFAULT_RETURN_PATH: &str = "/settings/openshell/providers";

/// Google OAuth client id embedded in the Antigravity CLI (`agy` binary).
/// Installed-app credential — not a honr secret.
///
/// The CLI ships two clients. This is the consumer client the host `agy`
/// login uses. The other (`1071006060591-…`) is a Business/Cloud Code client
/// whose `fetchAvailableModels` rows for Gemini Flash are `MODEL_PLACEHOLDER_*`
/// without `vertexModelId`, which makes agy die with "Could not determine
/// Vertex model ID" before any aiplatform call — on the host and in the seat.
const AGY_CLIENT_ID: &str =
    "884354919052-36trc1jjb3tguiac32ov6cod268c5blh.apps.googleusercontent.com";
/// Matching client secret from the same installed-app client (public in `agy`).
const AGY_CLIENT_SECRET: &str = "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";

const AGY_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

fn pending() -> &'static Mutex<HashMap<String, PendingOAuth>> {
    static STORE: OnceLock<Mutex<HashMap<String, PendingOAuth>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone)]
struct PendingOAuth {
    code_verifier: String,
    redirect_uri: String,
    return_path: String,
    created_at: u64,
}

pub fn api_routes() -> Router<SharedBoard> {
    Router::new()
        .route("/start", post(oauth_start))
        .route("/disconnect", post(oauth_disconnect))
}

pub fn callback_routes() -> Router<SharedBoard> {
    Router::new().route("/callback", get(oauth_callback))
}

#[derive(Debug, Deserialize)]
pub struct OAuthStartReq {
    #[serde(default)]
    pub return_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OAuthStartOut {
    pub authorize_url: String,
}

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn api_err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg.into() })))
}

async fn oauth_start(
    headers: HeaderMap,
    Json(req): Json<OAuthStartReq>,
) -> Result<Json<OAuthStartOut>, ApiErr> {
    let origin = crate::mcp_oauth::public_origin(&headers);
    if origin.is_empty() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "cannot resolve public origin (Host / Origin / X-Forwarded-Host, or HONR_PUBLIC_URL)",
        ));
    }
    let redirect_uri = format!("{}{CALLBACK_PATH}", origin.trim_end_matches('/'));
    let return_path = sanitize_return_path(req.return_path.as_deref());

    let code_verifier = pkce_verifier();
    let code_challenge = pkce_challenge_s256(&code_verifier);
    let state = random_token(32);

    {
        let mut st = pending().lock();
        st.retain(|_, p| now_secs().saturating_sub(p.created_at) < PENDING_TTL_SECS);
        st.insert(
            state.clone(),
            PendingOAuth {
                code_verifier,
                redirect_uri: redirect_uri.clone(),
                return_path,
                created_at: now_secs(),
            },
        );
    }

    let scope = AGY_SCOPES.join(" ");
    let authorize_url = format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&scope={}&access_type=offline&prompt=consent",
        urlencoding(AGY_CLIENT_ID),
        urlencoding(&redirect_uri),
        urlencoding(&state),
        urlencoding(&code_challenge),
        urlencoding(&scope),
    );
    Ok(Json(OAuthStartOut { authorize_url }))
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
    let (pending, return_path) = {
        let Some(state) = q.state.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return redirect_err(DEFAULT_RETURN_PATH, "missing state");
        };
        let mut st = pending().lock();
        match st.remove(state) {
            Some(p) => {
                let path = p.return_path.clone();
                (p, path)
            }
            None => return redirect_err(DEFAULT_RETURN_PATH, "unknown or expired state"),
        }
    };

    if let Some(err) = q.error.as_deref().filter(|s| !s.is_empty()) {
        let detail = q
            .error_description
            .as_deref()
            .unwrap_or(err)
            .to_string();
        return redirect_err(&return_path, &detail);
    }

    let Some(code) = q.code.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return redirect_err(&return_path, "missing code");
    };

    let tokens = match exchange_code(code, &pending).await {
        Ok(t) => t,
        Err(e) => return redirect_err(&return_path, &e),
    };

    if let Err(e) = finish_oauth_connect(&board, &tokens).await {
        return redirect_err(&return_path, &e);
    }

    Redirect::temporary(&format!("{return_path}?agy_oauth=ok")).into_response()
}

async fn oauth_disconnect(
    State(board): State<SharedBoard>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    disconnect_oauth(&board)
        .await
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Clear Antigravity credentials + refresh; keep project/location config.
pub async fn disconnect_oauth(board: &SharedBoard) -> Result<(), String> {
    let existing = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == ANTIGRAVITY_PROVIDER);
    let config = existing
        .as_ref()
        .map(|p| p.config.clone())
        .unwrap_or_default();

    let desired = OpenShellProviderDesired {
        name: ANTIGRAVITY_PROVIDER.into(),
        provider_type: ANTIGRAVITY_PROVIDER.into(),
        config,
        credentials_sealed: None,
        credential_keys: vec![],
        refresh: None,
    }
    .normalized();
    let stored = board.upsert_openshell_provider(desired);

    // Re-apply without credentials when possible; otherwise leave gateway stale
    // until the next login (apply may fail without credentials — that's ok).
    let os = board.openshell_client();
    let _ = os
        .apply_provider(
            &stored.name,
            &stored.provider_type,
            BTreeMap::new(),
            stored.config.clone(),
            None,
        )
        .await;
    Ok(())
}

async fn finish_oauth_connect(board: &SharedBoard, tokens: &TokenResponse) -> Result<(), String> {
    let access = tokens
        .access_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "token response missing access_token".to_string())?;
    let refresh = tokens
        .refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "token response missing refresh_token (need access_type=offline + consent)".to_string()
        })?;

    // Ensure board type YAML includes refresh (shipped seed only inserts when missing).
    let yaml = include_str!("../sandbox/openshell/antigravity.yaml").trim();
    provider_types::parse_provider_type_yaml(yaml, Some(ANTIGRAVITY_PROVIDER))?;
    board.upsert_openshell_provider_type(OpenShellProviderTypeDesired {
        id: ANTIGRAVITY_PROVIDER.into(),
        yaml: yaml.to_string(),
        shipped: true,
        form_config_keys: vec![CONFIG_PROJECT.into(), CONFIG_LOCATION.into()],
    })?;

    let os = board.openshell_client();
    os.upsert_provider_type_yaml(ANTIGRAVITY_PROVIDER, yaml)
        .await
        .map_err(|e| format!("import antigravity provider type: {e}"))?;

    let existing = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == ANTIGRAVITY_PROVIDER);
    let mut config = existing
        .as_ref()
        .map(|p| p.config.clone())
        .unwrap_or_default();
    if config
        .get(CONFIG_PROJECT)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        // Prefer an existing board project; otherwise leave empty for Settings.
        let _ = config
            .entry(CONFIG_PROJECT.into())
            .or_insert_with(String::new);
    }
    if config
        .get(CONFIG_LOCATION)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        config.insert(CONFIG_LOCATION.into(), "global".into());
    }

    let mut creds = BTreeMap::new();
    creds.insert("ANTIGRAVITY_ACCESS_TOKEN".into(), access.to_string());
    let credentials_sealed = seal_string_map(&creds).map_err(|e| e.to_string())?;

    let mut material = BTreeMap::new();
    material.insert("client_id".into(), AGY_CLIENT_ID.to_string());
    material.insert("client_secret".into(), AGY_CLIENT_SECRET.to_string());
    material.insert("refresh_token".into(), refresh.to_string());
    let material_sealed = seal_string_map(&material).map_err(|e| e.to_string())?;

    let desired = OpenShellProviderDesired {
        name: ANTIGRAVITY_PROVIDER.into(),
        provider_type: ANTIGRAVITY_PROVIDER.into(),
        config,
        credentials_sealed: Some(credentials_sealed),
        credential_keys: vec!["ANTIGRAVITY_ACCESS_TOKEN".into()],
        refresh: Some(OpenShellProviderRefreshDesired {
            credential_key: "ANTIGRAVITY_ACCESS_TOKEN".into(),
            strategy: "oauth2_refresh_token".into(),
            material_sealed,
            secret_material_keys: vec!["client_secret".into(), "refresh_token".into()],
        }),
    }
    .normalized();
    let stored = board.upsert_openshell_provider(desired);

    let credentials = open_string_map(
        stored
            .credentials_sealed
            .as_deref()
            .ok_or_else(|| "missing sealed credentials".to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let refresh_spec = {
        let r = stored
            .refresh
            .as_ref()
            .ok_or_else(|| "missing refresh".to_string())?;
        let material = open_string_map(&r.material_sealed).map_err(|e| e.to_string())?;
        crate::openshell::ProviderRefreshSpec {
            credential_key: r.credential_key.clone(),
            strategy: r.strategy.clone(),
            material,
            secret_material_keys: r.secret_material_keys.clone(),
        }
    };
    os.apply_provider(
        &stored.name,
        &stored.provider_type,
        credentials,
        stored.config.clone(),
        Some(&refresh_spec),
    )
    .await
    .map_err(|e| format!("gateway apply antigravity: {e}"))?;

    // Refresh cockpit seat token file when agy is the live engine.
    if let Some(session) = board.cockpit_session() {
        if session.status == CockpitSessionStatus::Running {
            if let Some(env) = session
                .environment
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let resolved = board.resolve_cockpit_sandbox_create();
                if resolved.engine.as_deref().map(str::trim) == Some("agy") {
                    let _ = antigravity::attach_to_running_cockpit(board).await;
                    if let Err(e) = setup_agy_auth(&os, env, board).await {
                        tracing::warn!(error = %e, "agy oauth: setup_agy_auth after login failed");
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    token_type: Option<String>,
}

async fn exchange_code(code: &str, pending: &PendingOAuth) -> Result<TokenResponse, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let body = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("client_id", AGY_CLIENT_ID),
        ("client_secret", AGY_CLIENT_SECRET),
        ("code_verifier", pending.code_verifier.as_str()),
    ])
    .map_err(|e| format!("encode token body: {e}"))?;
    let resp = client
        .post(TOKEN_URL)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("token body: {e}"))?;
    if !status.is_success() {
        return Err(format!("token exchange {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("token json: {e}"))
}

fn redirect_err(return_path: &str, message: &str) -> Response {
    let msg = urlencoding(message);
    Redirect::temporary(&format!("{return_path}?agy_oauth=error&message={msg}")).into_response()
}

fn sanitize_return_path(raw: Option<&str>) -> String {
    let Some(r) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return DEFAULT_RETURN_PATH.into();
    };
    if r.starts_with("/settings/") && !r.contains("://") && !r.contains('\n') {
        r.to_string()
    } else {
        DEFAULT_RETURN_PATH.into()
    }
}

fn pkce_verifier() -> String {
    random_token(64)
}

fn pkce_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn random_token(nbytes: usize) -> String {
    let mut bytes = vec![0u8; nbytes];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
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
    fn pkce_challenge_is_s256() {
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge_s256(v),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn sanitize_return_path_settings_only() {
        assert_eq!(
            sanitize_return_path(Some("/settings/openshell")),
            "/settings/openshell"
        );
        assert_eq!(
            sanitize_return_path(Some("https://evil.example/")),
            DEFAULT_RETURN_PATH
        );
        assert_eq!(sanitize_return_path(None), DEFAULT_RETURN_PATH);
    }

    #[test]
    fn shipped_yaml_parses_with_refresh() {
        let yaml = include_str!("../sandbox/openshell/antigravity.yaml");
        let parsed = provider_types::parse_provider_type_yaml(yaml, Some(ANTIGRAVITY_PROVIDER))
            .expect("yaml");
        assert_eq!(parsed.id, ANTIGRAVITY_PROVIDER);
        assert!(parsed
            .credential_env_vars
            .iter()
            .any(|e| e == "ANTIGRAVITY_ACCESS_TOKEN"));
        assert!(yaml.contains("oauth2_refresh_token"));
        assert!(yaml.contains("oauth2.googleapis.com/token"));
    }
}
