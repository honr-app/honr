//! GitHub App JWT + installation access tokens for sandbox `GITHUB_TOKEN`.
//!
//! OpenShell has no App-native refresh strategy, so honr mints short-lived
//! installation tokens and upserts them into the gateway `github` provider.

use crate::model::OpenShellProviderDesired;
use crate::secrets::{seal_string_map, GitHubAppBundle};
use crate::store::SharedBoard;

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Desired OpenShell provider name (also attach name).
pub const PROVIDER_NAME: &str = "github";
/// Env / credential key sandboxes and `gh` expect.
pub const CREDENTIAL_KEY: &str = "GITHUB_TOKEN";
/// Remint when this close to expiry (installation tokens last ~1h).
pub const REFRESH_SKEW: Duration = Duration::minutes(10);

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

/// Credential map for the OpenShell `github` provider.
pub fn provider_credentials(token: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(CREDENTIAL_KEY.into(), token.to_string());
    m
}

/// Mint (if needed) and upsert the OpenShell `github` provider with a live token.
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
    if !cache.needs_mint(now) {
        // Still ensure desired attach flag / provider row exists.
        ensure_desired_row(board, None)?;
        return Ok(true);
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
    let desired = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME)
        .ok_or_else(|| Error::Config("github provider missing after upsert".into()))?;

    let credentials = provider_credentials(&minted.token);
    let os = board.openshell_client();
    os.apply_provider(
        PROVIDER_NAME,
        "github",
        credentials,
        desired.config.clone(),
        None,
    )
    .await
    .map_err(|e| Error::Api(format!("openshell apply github provider: {e}")))?;

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

fn ensure_desired_row(board: &SharedBoard, fresh_token: Option<&str>) -> Result<(), Error> {
    let existing = board
        .openshell_providers()
        .into_iter()
        .find(|p| p.name == PROVIDER_NAME);

    let (credentials_sealed, credential_keys) = if let Some(token) = fresh_token {
        let sealed = seal_string_map(&provider_credentials(token))
            .map_err(|e| Error::Config(format!("seal GITHUB_TOKEN: {e}")))?;
        (Some(sealed), vec![CREDENTIAL_KEY.to_string()])
    } else if let Some(ref e) = existing {
        (e.credentials_sealed.clone(), e.credential_keys.clone())
    } else {
        (None, vec![CREDENTIAL_KEY.to_string()])
    };

    let config = existing
        .as_ref()
        .map(|e| e.config.clone())
        .unwrap_or_default();

    board.upsert_openshell_provider(
        OpenShellProviderDesired {
            name: PROVIDER_NAME.into(),
            provider_type: "github".into(),
            config,
            credentials_sealed,
            credential_keys,
            refresh: None,
            attach_to_sandboxes: true,
        }
        .normalized(),
    );
    Ok(())
}

/// Whether minting is possible (sealed App + installation id).
pub fn configured_for_tokens(board: &SharedBoard) -> bool {
    board.github_app_status().complete && board.github_app_installation_id().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openshell::{OpenShell, Output};
    use crate::secrets::{open_string_map, seal_github_app};
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
        let board = std::sync::Arc::new(Board::new(
            crate::schema::Schema::default(),
            dir.join("board.json"),
        ));
        (dir, board, env)
    }

    fn seal_test_app(board: &SharedBoard) {
        let sealed = seal_github_app(&GitHubAppBundle {
            app_id: "123456".into(),
            private_key_pem: test_rsa_pem(),
            ..Default::default()
        })
        .expect("seal");
        board.set_github_app_sealed(Some(sealed));
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
    fn provider_credentials_sets_github_token() {
        let m = provider_credentials("ghs_test");
        assert_eq!(m.get(CREDENTIAL_KEY).map(String::as_str), Some("ghs_test"));
    }

    #[test]
    fn ensure_desired_row_seals_token_without_plaintext_on_board() {
        let (dir, board, _env) = test_board("desired");
        ensure_desired_row(&board, Some("ghs_secret_value")).expect("upsert");
        let providers = board.openshell_providers();
        assert_eq!(providers.len(), 1);
        let p = &providers[0];
        assert_eq!(p.name, PROVIDER_NAME);
        assert_eq!(p.provider_type, "github");
        assert!(p.attach_to_sandboxes);
        assert_eq!(p.credential_keys, vec![CREDENTIAL_KEY.to_string()]);
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
        board.set_github_app_token_cache(TokenCache {
            expires_at: Some(Utc::now() + Duration::hours(1)),
            last_error: None,
        });
        // Point at a dead base — mint must not be attempted.
        let _api = github_api_env::Guard::set("http://127.0.0.1:1");
        let minted = ensure_github_provider(&board).await.expect("ensure");
        assert!(minted);
        let p = board
            .openshell_providers()
            .into_iter()
            .find(|p| p.name == PROVIDER_NAME)
            .expect("desired github row");
        assert!(p.attach_to_sandboxes);
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
        assert!(p.attach_to_sandboxes);
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
}
