//! GitHub App JWT + installation access tokens for sandbox `GH_TOKEN`.
//!
//! OpenShell has no App-native refresh strategy, so honr mints short-lived
//! installation tokens and upserts them into the gateway provider instance
//! [`PROVIDER_NAME`] (`github-app`, shipped type [`PROVIDER_TYPE`]).
//! Only `GH_TOKEN` is pushed to the gateway — never the App private key.

use crate::model::{OpenShellProviderDesired, GITHUB_APP_PROVIDER_TYPE};
use crate::secrets::{open_string_map, seal_string_map, GitHubAppBundle};
use crate::store::{Board, SharedBoard};

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// OpenShell provider **instance** name (sandbox attach name).
pub const PROVIDER_NAME: &str = "github-app";
/// Pre-rename instance name; board load rewrites attaches + provider rows.
pub const LEGACY_PROVIDER_NAME: &str = "github";
/// OpenShell builtin type used before the shipped `github-app` profile.
pub const LEGACY_BUILTIN_TYPE: &str = "github";
/// Shipped custom provider type (see `sandbox/openshell/github-app.yaml`).
pub const PROVIDER_TYPE: &str = GITHUB_APP_PROVIDER_TYPE;
/// Env / credential key sandboxes and `gh` expect (`gh` prefers this over `GITHUB_TOKEN`).
pub const CREDENTIAL_KEY: &str = "GH_TOKEN";
/// Board config: GitHub App numeric id (non-secret).
pub const CONFIG_APP_ID: &str = "GITHUB_APP_ID";
/// Board config: installation id that mints tokens (non-secret).
pub const CONFIG_INSTALLATION_ID: &str = "GITHUB_INSTALLATION_ID";
/// Board-only sealed credential: App private key PEM (never pushed to gateway).
pub const CRED_PRIVATE_KEY: &str = "GITHUB_APP_PRIVATE_KEY";
/// Board-only sealed: webhook secret (Access / Forge; not gateway).
pub const CRED_WEBHOOK_SECRET: &str = "GITHUB_APP_WEBHOOK_SECRET";
/// Board-only sealed: OAuth client id (Access; not gateway).
pub const CRED_CLIENT_ID: &str = "GITHUB_APP_CLIENT_ID";
/// Board-only sealed: OAuth client secret (Access; not gateway).
pub const CRED_CLIENT_SECRET: &str = "GITHUB_APP_CLIENT_SECRET";
/// Remint when this close to expiry (installation tokens last ~1h).
pub const REFRESH_SKEW: Duration = Duration::minutes(10);

/// Config keys that stay on the board and must not be sent to OpenShell.
pub fn board_only_config_keys() -> &'static [&'static str] {
    &[CONFIG_APP_ID, CONFIG_INSTALLATION_ID]
}

/// Credential keys that stay on the board and must not be sent to OpenShell.
pub fn board_only_credential_keys() -> &'static [&'static str] {
    &[
        CRED_PRIVATE_KEY,
        CRED_WEBHOOK_SECRET,
        CRED_CLIENT_ID,
        CRED_CLIENT_SECRET,
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("github app: {0}")]
    Config(String),
    #[error("jwt: {0}")]
    Jwt(String),
    #[error("github api: {0}")]
    Api(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallationInfo {
    pub id: u64,
    pub account_login: String,
    #[serde(default)]
    pub account_type: String,
}

#[derive(Debug, Clone)]
pub struct InstallationToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenCache {
    pub expires_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl TokenCache {
    pub fn needs_mint(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => true,
            Some(exp) => now + REFRESH_SKEW >= exp,
        }
    }
}

#[derive(Debug, Serialize)]
struct AppJwtClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

/// RS256 App JWT (≤10 minutes). Used only as Bearer to App APIs.
pub fn make_app_jwt(bundle: &GitHubAppBundle, now: DateTime<Utc>) -> Result<String, Error> {
    let app_id = bundle.app_id.trim();
    if app_id.is_empty() {
        return Err(Error::Config("app_id empty".into()));
    }
    if bundle.private_key_pem.trim().is_empty() {
        return Err(Error::Config("private_key empty".into()));
    }
    let iat = now.timestamp() - 60;
    let exp = now.timestamp() + 9 * 60;
    let claims = AppJwtClaims {
        iat,
        exp,
        iss: app_id.to_string(),
    };
    let key = EncodingKey::from_rsa_pem(bundle.private_key_pem.as_bytes())
        .map_err(|e| Error::Jwt(format!("rsa pem: {e}")))?;
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".into());
    encode(&header, &claims, &key).map_err(|e| Error::Jwt(e.to_string()))
}

fn api_base() -> String {
    std::env::var("HONR_GITHUB_API")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://api.github.com".into())
}

fn client() -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .user_agent("honr")
        .build()
        .map_err(|e| Error::Api(e.to_string()))
}

/// `GET /app/installations` — accounts where the App is installed.
pub async fn list_installations(jwt: &str) -> Result<Vec<InstallationInfo>, Error> {
    #[derive(Deserialize)]
    struct Account {
        login: String,
        #[serde(rename = "type")]
        account_type: Option<String>,
    }
    #[derive(Deserialize)]
    struct Row {
        id: u64,
        account: Option<Account>,
    }
    let url = format!("{}/app/installations", api_base());
    let resp = client()?
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Api(format!("list installations: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Api(format!(
            "list installations HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    let rows: Vec<Row> = resp
        .json()
        .await
        .map_err(|e| Error::Api(format!("list installations json: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|r| InstallationInfo {
            id: r.id,
            account_login: r
                .account
                .as_ref()
                .map(|a| a.login.clone())
                .unwrap_or_default(),
            account_type: r
                .account
                .and_then(|a| a.account_type)
                .unwrap_or_default(),
        })
        .collect())
}

/// `POST /app/installations/{id}/access_tokens`.
pub async fn create_installation_token(
    jwt: &str,
    installation_id: u64,
) -> Result<InstallationToken, Error> {
    #[derive(Deserialize)]
    struct Resp {
        token: String,
        expires_at: String,
    }
    let url = format!(
        "{}/app/installations/{installation_id}/access_tokens",
        api_base()
    );
    let resp = client()?
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {jwt}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| Error::Api(format!("installation token: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Api(format!(
            "installation token HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    let body: Resp = resp
        .json()
        .await
        .map_err(|e| Error::Api(format!("installation token json: {e}")))?;
    let expires_at = DateTime::parse_from_rfc3339(&body.expires_at)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| Error::Api(format!("expires_at: {e}")))?;
    if body.token.trim().is_empty() {
        return Err(Error::Api("empty installation token".into()));
    }
    Ok(InstallationToken {
        token: body.token,
        expires_at,
    })
}

/// Mint from sealed bundle + installation id.
pub async fn mint_installation_token(
    bundle: &GitHubAppBundle,
    installation_id: u64,
) -> Result<InstallationToken, Error> {
    let jwt = make_app_jwt(bundle, Utc::now())?;
    create_installation_token(&jwt, installation_id).await
}

/// Credential map pushed to the OpenShell `github-app` provider (`GH_TOKEN` only).
pub fn provider_credentials(token: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(CREDENTIAL_KEY.into(), token.to_string());
    m
}

/// Gateway config for `github-app`: strip board-only App mint fields.
pub fn gateway_config(config: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    config
        .iter()
        .filter(|(k, _)| !board_only_config_keys().contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Filter board sealed credentials down to what the gateway may see.
pub fn gateway_credentials(creds: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    creds
        .iter()
        .filter(|(k, v)| {
            !board_only_credential_keys().contains(&k.as_str()) && !v.trim().is_empty()
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Read App bundle from the `github-app` provider row (config + sealed map).
pub fn bundle_from_provider(board: &Board) -> Option<GitHubAppBundle> {
    let p = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME)?;
    let map = p
        .credentials_sealed
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| open_string_map(s).ok())
        .unwrap_or_default();
    let app_id = p
        .config
        .get(CONFIG_APP_ID)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let private_key_pem = map
        .get(CRED_PRIVATE_KEY)
        .map(|s| s.to_string())
        .unwrap_or_default();
    if app_id.is_empty() && private_key_pem.is_empty() {
        return None;
    }
    Some(GitHubAppBundle {
        app_id,
        private_key_pem,
        webhook_secret: map
            .get(CRED_WEBHOOK_SECRET)
            .cloned()
            .unwrap_or_default(),
        client_id: map.get(CRED_CLIENT_ID).cloned().unwrap_or_default(),
        client_secret: map
            .get(CRED_CLIENT_SECRET)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Installation id from provider config (preferred) or legacy board field.
pub fn installation_id_from_provider(board: &Board) -> Option<u64> {
    let p = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME)?;
    p.config
        .get(CONFIG_INSTALLATION_ID)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

/// Presence flags derived from the provider row (or legacy sealed blob).
pub fn status_from_board(board: &Board) -> crate::secrets::GitHubAppStatus {
    if let Some(b) = bundle_from_provider(board) {
        return crate::secrets::GitHubAppStatus::from(&b);
    }
    crate::secrets::github_app_status_from_sealed(board.github_app_sealed().as_deref())
}

/// Mint (if needed) and upsert the OpenShell `github-app` provider with a live token.
///
/// No-op when App credentials or installation_id are unset (returns Ok(false)).
/// Returns Ok(true) when the gateway provider was refreshed (or confirmed fresh).
pub async fn ensure_github_provider(board: &SharedBoard) -> Result<bool, Error> {
    let Some(bundle) = board.github_app_bundle() else {
        return Ok(false);
    };
    if !bundle.app_id.trim().is_empty() && bundle.private_key_pem.trim().is_empty() {
        return Err(Error::Config("GitHub App private key missing".into()));
    }
    if bundle.app_id.trim().is_empty() || bundle.private_key_pem.trim().is_empty() {
        return Ok(false);
    }
    let Some(installation_id) = board.github_app_installation_id() else {
        return Ok(false);
    };

    let cache = board.github_app_token_cache();
    let now = Utc::now();
    ensure_desired_row(board, None)?;

    // Sweeper calls this every tick — stay quiet when the cache is fresh and
    // the gateway already has GH_TOKEN without a leftover GITHUB_TOKEN.
    if !cache.needs_mint(now) {
        if !gateway_github_provider_needs_push(board).await? {
            return Ok(true);
        }
        if let Some(token) = sealed_github_token(board)? {
            ensure_desired_row(board, Some(&token))?;
            push_github_provider_on_gateway(board, &token).await?;
            tracing::info!(
                installation_id,
                "repaired OpenShell `{PROVIDER_NAME}` provider"
            );
            return Ok(true);
        }
        // Cache claimed fresh but nothing sealed — fall through to remint.
    }

    let minted = match mint_installation_token(&bundle, installation_id).await {
        Ok(t) => t,
        Err(e) => {
            board.set_github_app_token_cache(TokenCache {
                expires_at: cache.expires_at,
                last_error: Some(e.to_string()),
            });
            return Err(e);
        }
    };
    ensure_desired_row(board, Some(&minted.token))?;
    push_github_provider_on_gateway(board, &minted.token).await?;

    board.set_github_app_token_cache(TokenCache {
        expires_at: Some(minted.expires_at),
        last_error: None,
    });
    tracing::info!(
        installation_id,
        expires_at = %minted.expires_at,
        "synced GitHub App installation token to OpenShell provider `{PROVIDER_NAME}`"
    );
    Ok(true)
}

/// True when the gateway is missing `github-app`, lacks `GH_TOKEN`, or still has
/// a leftover `GITHUB_TOKEN` credential key.
async fn gateway_github_provider_needs_push(board: &SharedBoard) -> Result<bool, Error> {
    let os = board.openshell_client();
    let list = os
        .list_providers()
        .await
        .map_err(|e| Error::Api(format!("openshell list providers: {e}")))?;
    let Some(p) = list.iter().find(|p| p.name == PROVIDER_NAME) else {
        return Ok(true);
    };
    let has_gh = p.credential_keys.iter().any(|k| k == CREDENTIAL_KEY);
    let has_legacy = p.credential_keys.iter().any(|k| k == "GITHUB_TOKEN");
    Ok(!has_gh || has_legacy)
}

/// Create or update the gateway provider. Never create when it already exists
/// (that races the sweeper and logs "provider already exists").
async fn push_github_provider_on_gateway(board: &SharedBoard, token: &str) -> Result<(), Error> {
    let desired = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME)
        .ok_or_else(|| Error::Config("github-app provider missing after upsert".into()))?;
    let os = board.openshell_client();
    let exists = os
        .list_providers()
        .await
        .map_err(|e| Error::Api(format!("openshell list providers: {e}")))?
        .iter()
        .any(|p| p.name == PROVIDER_NAME);
    let config = gateway_config(&desired.config);

    if exists {
        let mut credentials = provider_credentials(token);
        // Empty value clears a merged leftover from older PAT / App syncs.
        credentials.insert("GITHUB_TOKEN".into(), String::new());
        os.update_provider(PROVIDER_NAME, PROVIDER_TYPE, credentials, config)
            .await
            .map_err(|e| Error::Api(format!("openshell update {PROVIDER_NAME} provider: {e}")))?;
    } else {
        os.create_provider(
            PROVIDER_NAME,
            PROVIDER_TYPE,
            provider_credentials(token),
            config,
        )
        .await
        .map_err(|e| Error::Api(format!("openshell create {PROVIDER_NAME} provider: {e}")))?;
    }
    Ok(())
}

fn sealed_github_token(board: &SharedBoard) -> Result<Option<String>, Error> {
    let Some(p) = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME)
    else {
        return Ok(None);
    };
    let Some(sealed) = p.credentials_sealed.as_deref() else {
        return Ok(None);
    };
    let map = open_string_map(sealed).map_err(|e| Error::Config(format!("open GH_TOKEN: {e}")))?;
    if let Some(t) = map.get(CREDENTIAL_KEY).filter(|t| !t.is_empty()) {
        return Ok(Some(t.clone()));
    }
    // Migrate one-shot from older App syncs that sealed GITHUB_TOKEN.
    if let Some(t) = map.get("GITHUB_TOKEN").filter(|t| !t.is_empty()) {
        return Ok(Some(t.clone()));
    }
    Ok(None)
}

/// Merge `fresh_token` into the existing sealed map (preserve App private key).
fn ensure_desired_row(board: &SharedBoard, fresh_token: Option<&str>) -> Result<(), Error> {
    let existing = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME);

    let mut map = existing
        .as_ref()
        .and_then(|e| e.credentials_sealed.as_deref())
        .filter(|s| !s.trim().is_empty())
        .map(|s| open_string_map(s).map_err(|e| Error::Config(format!("open credentials: {e}"))))
        .transpose()?
        .unwrap_or_default();

    if let Some(token) = fresh_token {
        map.insert(CREDENTIAL_KEY.into(), token.to_string());
        map.remove("GITHUB_TOKEN");
    }

    let credential_keys: Vec<String> = map.keys().cloned().collect();
    let credentials_sealed = if map.is_empty() {
        None
    } else {
        Some(seal_string_map(&map).map_err(|e| Error::Config(format!("seal credentials: {e}")))?)
    };

    let config = existing
        .as_ref()
        .map(|e| e.config.clone())
        .unwrap_or_default();
    let refresh = existing.as_ref().and_then(|e| e.refresh.clone());

    board.upsert_openshell_provider(
        OpenShellProviderDesired {
            name: PROVIDER_NAME.into(),
            provider_type: PROVIDER_TYPE.into(),
            config,
            credentials_sealed,
            credential_keys,
            refresh,
        }
        .normalized(),
    );
    Ok(())
}

/// Whether minting is possible (App material + installation id on the provider).
pub fn configured_for_tokens(board: &SharedBoard) -> bool {
    status_from_board(board).complete && board.github_app_installation_id().is_some()
}

/// Whether a desired provider can supply a host GitHub REST token.
pub fn provider_can_host_poll(p: &OpenShellProviderDesired) -> bool {
    if p.provider_type == PROVIDER_TYPE || p.name == PROVIDER_NAME {
        return true;
    }
    if p.provider_type == LEGACY_BUILTIN_TYPE {
        return true;
    }
    p.credential_keys
        .iter()
        .any(|k| k == CREDENTIAL_KEY || k == "GITHUB_TOKEN")
}

/// Host GitHub REST token for Forge poll from an **explicit** provider name.
///
/// No auto-selection — `provider_name` must be set under Forge. Returns
/// `Ok(None)` when unset, missing, or not yet credentialed.
///
/// - `github-app`: mint/reuse installation token (App JWT path).
/// - other rows: read sealed `GH_TOKEN` (or legacy `GITHUB_TOKEN`).
pub async fn host_poll_token(
    board: &SharedBoard,
    provider_name: Option<&str>,
) -> Result<Option<(String, String)>, Error> {
    let Some(name) = provider_name.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let Some(p) = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == name)
    else {
        return Ok(None);
    };
    if !provider_can_host_poll(&p) {
        return Ok(None);
    }

    if p.provider_type == PROVIDER_TYPE || p.name == PROVIDER_NAME {
        return match host_installation_token(board).await? {
            Some(t) => Ok(Some((name.to_string(), t))),
            None => Ok(None),
        };
    }

    let Some(sealed) = p.credentials_sealed.as_deref() else {
        return Ok(None);
    };
    let map = open_string_map(sealed).map_err(|e| Error::Config(format!("open poll token: {e}")))?;
    if let Some(t) = map
        .get(CREDENTIAL_KEY)
        .or_else(|| map.get("GITHUB_TOKEN"))
        .filter(|t| !t.is_empty())
    {
        return Ok(Some((name.to_string(), t.clone())));
    }
    Ok(None)
}

/// Host-side installation token for REST (webhook poll). Reuses the sealed
/// cache when fresh; mints without requiring an OpenShell gateway push.
///
/// Returns `Ok(None)` when App/installation are not configured.
pub async fn host_installation_token(board: &SharedBoard) -> Result<Option<String>, Error> {
    if !configured_for_tokens(board) {
        return Ok(None);
    }
    let Some(bundle) = board.github_app_bundle() else {
        return Ok(None);
    };
    if bundle.app_id.trim().is_empty() || bundle.private_key_pem.trim().is_empty() {
        return Ok(None);
    }
    let Some(installation_id) = board.github_app_installation_id() else {
        return Ok(None);
    };

    let cache = board.github_app_token_cache();
    let now = Utc::now();
    if !cache.needs_mint(now) {
        if let Some(token) = sealed_github_token(board)? {
            return Ok(Some(token));
        }
    }

    let minted = match mint_installation_token(&bundle, installation_id).await {
        Ok(t) => t,
        Err(e) => {
            board.set_github_app_token_cache(TokenCache {
                expires_at: cache.expires_at,
                last_error: Some(e.to_string()),
            });
            return Err(e);
        }
    };
    // Seal for reuse by poll + provider sync; do not push OpenShell here.
    ensure_desired_row(board, Some(&minted.token))?;
    board.set_github_app_token_cache(TokenCache {
        expires_at: Some(minted.expires_at),
        last_error: None,
    });
    Ok(Some(minted.token))
}

/// GitHub API base (override with `HONR_GITHUB_API` in tests).
pub fn github_api_base() -> String {
    api_base()
}

/// GitHub PR `mergeable` as returned by the pulls API (`true` / `false` / `null`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrMergeableState {
    Mergeable,
    Conflicting,
    /// `null` / missing — GitHub has not finished computing; retry later.
    Unknown,
}

/// Result of a host-side PR conflict check (no sandbox).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrConflictCheck {
    pub mergeable: PrMergeableState,
    /// Base branch name (`main`, etc.).
    pub base_ref: Option<String>,
}

/// `GET /repos/{owner}/{repo}/pulls/{n}` using the App installation token.
///
/// Returns `Ok(None)` when App/installation are not configured. Used by Review
/// catch-up after main advances — observe `mergeable` first; MERGEABLE is a
/// no-op, CONFLICTING bounces, UNKNOWN retries. No sandbox rebase.
pub async fn fetch_pr_conflict_check(
    board: &SharedBoard,
    pr_url: &str,
) -> Result<Option<PrConflictCheck>, Error> {
    let Some(token) = host_installation_token(board).await? else {
        return Ok(None);
    };
    let Some((owner_repo, number)) = crate::store::parse_github_pr_url(pr_url) else {
        return Err(Error::Config(format!(
            "not a github.com pull URL: {pr_url}"
        )));
    };
    fetch_pr_conflict_check_with_token(&token, &owner_repo, number).await
}

pub(crate) async fn fetch_pr_conflict_check_with_token(
    token: &str,
    owner_repo: &str,
    number: u64,
) -> Result<Option<PrConflictCheck>, Error> {
    #[derive(Deserialize)]
    struct Base {
        #[serde(rename = "ref")]
        base_ref: Option<String>,
    }
    #[derive(Deserialize)]
    struct Resp {
        /// `true` / `false` / omitted or null while GitHub computes.
        mergeable: Option<bool>,
        base: Option<Base>,
    }
    let url = format!(
        "{}/repos/{owner_repo}/pulls/{number}",
        api_base()
    );
    let resp = client()?
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Api(format!("GET pull: {e}")))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Api(format!(
            "GET pull HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }
    let body: Resp = resp
        .json()
        .await
        .map_err(|e| Error::Api(format!("GET pull json: {e}")))?;
    let mergeable = match body.mergeable {
        Some(true) => PrMergeableState::Mergeable,
        Some(false) => PrMergeableState::Conflicting,
        None => PrMergeableState::Unknown,
    };
    Ok(Some(PrConflictCheck {
        mergeable,
        base_ref: body.base.and_then(|b| b.base_ref),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell::{OpenShell, Output};
    use crate::secrets::open_string_map;
    use crate::store::Board;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration as StdDuration;

    /// Minimal RSA key for JWT unit tests (not a real GitHub App key).
    fn test_rsa_pem() -> String {
        // Generated once for tests; never used against GitHub.
        include_str!("testdata/github_app_test_rsa.pem").to_string()
    }

    /// Serialize `HONR_GITHUB_API` mutations across tests.
    mod github_api_env {
        use super::*;
        static LOCK: Mutex<()> = Mutex::new(());

        pub(crate) struct Guard {
            _lock: MutexGuard<'static, ()>,
            prev: Option<String>,
        }

        impl Guard {
            pub(crate) fn set(base: &str) -> Self {
                let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                let prev = std::env::var("HONR_GITHUB_API").ok();
                std::env::set_var("HONR_GITHUB_API", base);
                Self { _lock, prev }
            }
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.prev {
                    Some(v) => std::env::set_var("HONR_GITHUB_API", v),
                    None => std::env::remove_var("HONR_GITHUB_API"),
                }
            }
        }
    }

    fn test_board(label: &str) -> (std::path::PathBuf, SharedBoard, crate::secrets::master_key_env::Guard) {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-ghapp-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let key_path = dir.join("master.key");
        let env = crate::secrets::master_key_env::Guard::with_key_path(&key_path);
        let mut board_inner = Board::new(crate::schema::Schema::default(), dir.join("board.json"));
        // ensure_github_provider lists then create/updates the gateway provider.
        board_inner.openshell = Some(OpenShell::mock(
            |argv| {
                if argv.first().map(String::as_str) == Some("provider") {
                    Output {
                        code: 0,
                        stdout: "[]".into(),
                        stderr: String::new(),
                    }
                } else {
                    Output {
                        code: 1,
                        stdout: String::new(),
                        stderr: format!("unexpected mock argv: {argv:?}"),
                    }
                }
            },
            StdDuration::from_secs(5),
        ));
        let board = std::sync::Arc::new(board_inner);
        (dir, board, env)
    }

    fn seal_test_app(board: &SharedBoard) {
        board
            .set_github_app_bundle(&GitHubAppBundle {
                app_id: "123456".into(),
                private_key_pem: test_rsa_pem(),
                ..Default::default()
            })
            .expect("seal onto provider");
    }

    async fn spawn_github_mock() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/app/installations",
                get(|| async {
                    Json(serde_json::json!([{
                        "id": 99,
                        "account": { "login": "clankrshq", "type": "Organization" }
                    }]))
                }),
            )
            .route(
                "/app/installations/{id}/access_tokens",
                post(|| async {
                    let expires = (Utc::now() + Duration::hours(1)).to_rfc3339();
                    Json(serde_json::json!({
                        "token": "ghs_mock_installation_token",
                        "expires_at": expires,
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve mock");
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn token_cache_needs_mint_when_empty_or_near_expiry() {
        let now = Utc::now();
        assert!(TokenCache::default().needs_mint(now));
        let fresh = TokenCache {
            expires_at: Some(now + Duration::hours(1)),
            last_error: None,
        };
        assert!(!fresh.needs_mint(now));
        let soon = TokenCache {
            expires_at: Some(now + Duration::minutes(5)),
            last_error: None,
        };
        assert!(soon.needs_mint(now));
    }

    #[test]
    fn make_app_jwt_round_trips_header() {
        let pem = test_rsa_pem();
        if pem.trim().is_empty() || !pem.contains("BEGIN") {
            // File missing in sparse checkouts — skip rather than fail CI shape.
            eprintln!("skip jwt test: no testdata pem");
            return;
        }
        let bundle = GitHubAppBundle {
            app_id: "123456".into(),
            private_key_pem: pem,
            ..Default::default()
        };
        let jwt = make_app_jwt(&bundle, Utc::now()).expect("jwt");
        let parts: Vec<_> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(!jwt.contains("BEGIN"));
    }

    #[test]
    fn provider_credentials_sets_gh_token_only() {
        let m = provider_credentials("ghs_test");
        assert_eq!(m.get(CREDENTIAL_KEY).map(String::as_str), Some("ghs_test"));
        assert!(!m.contains_key("GITHUB_TOKEN"));
        assert_eq!(m.len(), 1);
    }

    #[tokio::test]
    async fn host_poll_token_requires_explicit_provider_name() {
        let (dir, board, _env) = test_board("poll-explicit");
        seal_test_app(&board);
        board.set_github_app_installation_id(Some(99));
        // Even with App ready, no auto-pick without Forge provider_name.
        assert!(host_poll_token(&board, None).await.expect("ok").is_none());
        assert!(host_poll_token(&board, Some("nope"))
            .await
            .expect("ok")
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gateway_credentials_strips_app_private_material() {
        let mut board = BTreeMap::new();
        board.insert(CREDENTIAL_KEY.into(), "ghs_live".into());
        board.insert(CRED_PRIVATE_KEY.into(), "-----BEGIN RSA PRIVATE KEY-----\nX\n-----END RSA PRIVATE KEY-----\n".into());
        board.insert(CRED_WEBHOOK_SECRET.into(), "whsec".into());
        let gw = gateway_credentials(&board);
        assert_eq!(gw.get(CREDENTIAL_KEY).map(String::as_str), Some("ghs_live"));
        assert!(!gw.contains_key(CRED_PRIVATE_KEY));
        assert!(!gw.contains_key(CRED_WEBHOOK_SECRET));
        let mut cfg = BTreeMap::new();
        cfg.insert(CONFIG_APP_ID.into(), "1".into());
        cfg.insert("OTHER".into(), "x".into());
        let gcfg = gateway_config(&cfg);
        assert!(!gcfg.contains_key(CONFIG_APP_ID));
        assert_eq!(gcfg.get("OTHER").map(String::as_str), Some("x"));
    }

    #[test]
    fn ensure_desired_row_seals_token_without_plaintext_on_board() {
        let (dir, board, _env) = test_board("desired");
        ensure_desired_row(&board, Some("ghs_secret_value")).expect("upsert");
        let providers = board.openshell_providers();
        assert_eq!(providers.len(), 1);
        let p = &providers[0];
        assert_eq!(p.name, PROVIDER_NAME);
        assert_eq!(p.provider_type, PROVIDER_TYPE);
        assert!(p.credential_keys.iter().any(|k| k == CREDENTIAL_KEY));
        let sealed = p.credentials_sealed.as_deref().expect("sealed");
        assert!(!sealed.contains("ghs_secret_value"));
        let opened = open_string_map(sealed).expect("open");
        assert_eq!(
            opened.get(CREDENTIAL_KEY).map(String::as_str),
            Some("ghs_secret_value")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ensure_skips_mint_when_token_cache_still_fresh() {
        let (dir, board, _env) = test_board("fresh");
        seal_test_app(&board);
        board.set_github_app_installation_id(Some(99));
        ensure_desired_row(&board, Some("ghs_cached_only")).expect("seed sealed");
        board.set_github_app_token_cache(TokenCache {
            expires_at: Some(Utc::now() + Duration::hours(1)),
            last_error: None,
        });
        // Point at a dead base — mint must not be attempted (sealed token reused).
        let _api = github_api_env::Guard::set("http://127.0.0.1:1");
        let minted = ensure_github_provider(&board).await.expect("ensure");
        assert!(minted);
        let p = board
            .openshell_providers()
            .into_iter()
            .find(|p| p.name == PROVIDER_NAME)
            .expect("desired github row");
        assert_eq!(p.name, PROVIDER_NAME);
        let opened = open_string_map(p.credentials_sealed.as_deref().unwrap()).expect("open");
        assert_eq!(
            opened.get(CREDENTIAL_KEY).map(String::as_str),
            Some("ghs_cached_only")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ensure_mints_and_upserts_via_mock_github_and_openshell() {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-ghapp-mint-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let key_path = dir.join("master.key");
        let _env = crate::secrets::master_key_env::Guard::with_key_path(&key_path);
        let mut board_inner = Board::new(crate::schema::Schema::default(), dir.join("board.json"));
        board_inner.openshell = Some(OpenShell::mock(
            |argv| {
                if argv.first().map(String::as_str) == Some("provider") {
                    Output {
                        code: 0,
                        stdout: "[]".into(),
                        stderr: String::new(),
                    }
                } else {
                    Output {
                        code: 1,
                        stdout: String::new(),
                        stderr: format!("unexpected mock argv: {argv:?}"),
                    }
                }
            },
            StdDuration::from_secs(5),
        ));
        let board: SharedBoard = std::sync::Arc::new(board_inner);
        seal_test_app(&board);
        board.set_github_app_installation_id(Some(99));

        let (base, handle) = spawn_github_mock().await;
        let _api = github_api_env::Guard::set(&base);

        let ok = ensure_github_provider(&board).await.expect("ensure");
        assert!(ok);
        let cache = board.github_app_token_cache();
        assert!(cache.expires_at.is_some());
        assert!(cache.last_error.is_none());
        let p = board
            .openshell_providers()
            .into_iter()
            .find(|p| p.name == PROVIDER_NAME)
            .expect("provider");
        assert_eq!(p.name, PROVIDER_NAME);
        let sealed = p.credentials_sealed.as_deref().expect("sealed");
        assert!(!sealed.contains("ghs_mock_installation_token"));
        let opened = open_string_map(sealed).expect("open");
        assert_eq!(
            opened.get(CREDENTIAL_KEY).map(String::as_str),
            Some("ghs_mock_installation_token")
        );

        // Second call with fresh cache must not remint (dead API would fail).
        drop(_api);
        let _dead = github_api_env::Guard::set("http://127.0.0.1:1");
        assert!(ensure_github_provider(&board).await.expect("fresh ensure"));

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_installations_parses_accounts() {
        let (base, handle) = spawn_github_mock().await;
        let _api = github_api_env::Guard::set(&base);
        let bundle = GitHubAppBundle {
            app_id: "123456".into(),
            private_key_pem: test_rsa_pem(),
            ..Default::default()
        };
        let jwt = make_app_jwt(&bundle, Utc::now()).expect("jwt");
        let list = list_installations(&jwt).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, 99);
        assert_eq!(list[0].account_login, "clankrshq");
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_pr_conflict_check_maps_mergeable_states() {
        let app = Router::new()
            .route(
                "/repos/{owner}/{repo}/pulls/{number}",
                get(|axum::extract::Path((_, _, number)): axum::extract::Path<(
                    String,
                    String,
                    u64,
                )>| async move {
                    if number == 404 {
                        return (
                            axum::http::StatusCode::NOT_FOUND,
                            Json(serde_json::json!({ "message": "Not Found" })),
                        );
                    }
                    let body = match number {
                        1 => serde_json::json!({
                            "mergeable": true,
                            "base": { "ref": "main" }
                        }),
                        2 => serde_json::json!({
                            "mergeable": false,
                            "base": { "ref": "main" }
                        }),
                        3 => serde_json::json!({
                            "mergeable": null,
                            "base": { "ref": "main" }
                        }),
                        _ => serde_json::json!({ "message": "Not Found" }),
                    };
                    (axum::http::StatusCode::OK, Json(body))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let _api = github_api_env::Guard::set(&format!("http://{addr}"));

        let m = fetch_pr_conflict_check_with_token("tok", "o/r", 1)
            .await
            .expect("ok")
            .expect("some");
        assert_eq!(m.mergeable, PrMergeableState::Mergeable);
        assert_eq!(m.base_ref.as_deref(), Some("main"));

        let c = fetch_pr_conflict_check_with_token("tok", "o/r", 2)
            .await
            .expect("ok")
            .expect("some");
        assert_eq!(c.mergeable, PrMergeableState::Conflicting);

        let u = fetch_pr_conflict_check_with_token("tok", "o/r", 3)
            .await
            .expect("ok")
            .expect("some");
        assert_eq!(u.mergeable, PrMergeableState::Unknown);

        let missing = fetch_pr_conflict_check_with_token("tok", "o/r", 404)
            .await
            .expect("ok");
        assert!(missing.is_none());

        handle.abort();
    }
}
