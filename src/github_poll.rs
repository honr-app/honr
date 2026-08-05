//! Webhook polling fallback — same Board effects as `POST /api/webhooks/github`.
//!
//! When Settings → Forge enables polling, a background loop mints a GitHub App
//! installation token and scans Review/NeedsHuman PRs plus default-branch tips
//! for repos on live cards. Merges call `complete_for_merged_pr_by(..., "github-poll")`;
//! tip changes call `notify_main_advanced`. Webhooks keep working in parallel.

use crate::github_app;
use crate::model::{State, MIN_WEBHOOK_POLL_INTERVAL_SECS};
use crate::store::{parse_github_pr_url, SharedBoard};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

/// Actor string written into transition history for poll-driven Done.
pub const POLL_BY: &str = "github-poll";

/// How often the loop re-checks Settings when polling is off.
const DISABLED_RECHECK_SECS: u64 = 60;

/// One warned failure class at a time (config / auth / network).
static LAST_WARN_CLASS: Mutex<Option<&'static str>> = Mutex::new(None);

fn warn_once(class: &'static str, msg: impl std::fmt::Display) {
    let mut g = LAST_WARN_CLASS.lock().unwrap_or_else(|e| e.into_inner());
    if *g == Some(class) {
        return;
    }
    *g = Some(class);
    tracing::warn!(class, "{msg}");
}

fn clear_warn(class: &'static str) {
    let mut g = LAST_WARN_CLASS.lock().unwrap_or_else(|e| e.into_inner());
    if *g == Some(class) {
        *g = None;
    }
}

/// Background loop: sleep → re-read config → tick when enabled.
pub async fn poll_loop(board: SharedBoard) {
    loop {
        let cfg = board.webhook_poll_config();
        let sleep_secs = if cfg.enabled {
            cfg.interval_secs.max(MIN_WEBHOOK_POLL_INTERVAL_SECS)
        } else {
            DISABLED_RECHECK_SECS
        };
        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
        if !board.webhook_poll_config().enabled {
            continue;
        }
        match tick(&board).await {
            Ok(()) => {
                clear_warn("config");
                clear_warn("auth");
                clear_warn("tick");
            }
            Err(e) => warn_once("tick", format!("webhook poll tick failed: {e}")),
        }
    }
}

/// One poll pass. No-op when disabled or App tokens unavailable.
pub async fn tick(board: &SharedBoard) -> Result<(), String> {
    let cfg = board.webhook_poll_config();
    if !cfg.enabled {
        return Ok(());
    }

    let token = match github_app::host_installation_token(board).await {
        Ok(Some(t)) => {
            clear_warn("config");
            clear_warn("auth");
            t
        }
        Ok(None) => {
            warn_once(
                "config",
                "webhook poll enabled but GitHub App / installation not configured; skipping",
            );
            return Ok(());
        }
        Err(e) => {
            warn_once("auth", format!("webhook poll token mint failed: {e}"));
            return Err(e.to_string());
        }
    };

    let targets = collect_targets(board);
    if targets.prs.is_empty() && targets.repos.is_empty() {
        return Ok(());
    }

    for pr in &targets.prs {
        match fetch_pull(&token, &pr.owner_repo, pr.number).await {
            Ok(Some(info)) if info.merged => {
                let url = info
                    .html_url
                    .unwrap_or_else(|| format!("https://github.com/{}/pull/{}", pr.owner_repo, pr.number));
                if let Some(id) =
                    board.complete_for_merged_pr_by(&url, Some(pr.number), POLL_BY)
                {
                    tracing::info!(id, pr = %pr.owner_repo, number = pr.number, "poll: PR merged → Done");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(
                    repo = %pr.owner_repo,
                    number = pr.number,
                    error = %e,
                    "poll: PR fetch failed"
                );
            }
        }
    }

    for repo in &targets.repos {
        match fetch_default_tip(&token, repo).await {
            Ok(Some((branch, sha))) => {
                let prev = board.webhook_poll_tip(repo);
                board.set_webhook_poll_tip(repo, &sha);
                if prev.as_deref() == Some(sha.as_str()) {
                    continue;
                }
                // First observation only seeds the tip — avoid MainAdvanced storms
                // on enable. Subsequent changes match webhook push semantics.
                if prev.is_none() {
                    tracing::debug!(%repo, %sha, "poll: seeded default-branch tip");
                    continue;
                }
                let ref_name = format!("refs/heads/{branch}");
                board.notify_main_advanced(&ref_name, Some(sha.clone()));
                tracing::info!(%repo, %branch, %sha, "poll: default branch advanced");
            }
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(%repo, error = %e, "poll: tip fetch failed");
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PrTarget {
    owner_repo: String,
    number: u64,
}

struct Targets {
    prs: Vec<PrTarget>,
    repos: BTreeSet<String>,
}

fn collect_targets(board: &SharedBoard) -> Targets {
    let mut prs = BTreeSet::new();
    let mut repos = BTreeSet::new();

    let items = board.snapshot().items;

    for item in items {
        match item.state {
            State::Review | State::NeedsHuman => {
                if let Some(url) = item.pr_url() {
                    if let Some((owner_repo, number)) = parse_github_pr_url(url) {
                        repos.insert(owner_repo.clone());
                        prs.insert(PrTarget { owner_repo, number });
                    }
                }
            }
            State::Claimed | State::Running => {
                if let Ok(Some(repo)) = board.resolve_card_repo(item.id) {
                    let upstream = repo.upstream.trim();
                    if !upstream.is_empty() {
                        repos.insert(upstream.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    Targets {
        prs: prs.into_iter().collect(),
        repos,
    }
}

#[derive(Debug)]
struct PullInfo {
    merged: bool,
    html_url: Option<String>,
}

async fn fetch_pull(
    token: &str,
    owner_repo: &str,
    number: u64,
) -> Result<Option<PullInfo>, String> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        merged: bool,
        html_url: Option<String>,
    }
    let url = format!(
        "{}/repos/{owner_repo}/pulls/{number}",
        github_app::github_api_base()
    );
    let resp = client()?
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GET pull: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "GET pull HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let body: Resp = resp
        .json()
        .await
        .map_err(|e| format!("GET pull json: {e}"))?;
    Ok(Some(PullInfo {
        merged: body.merged,
        html_url: body.html_url,
    }))
}

async fn fetch_default_tip(token: &str, owner_repo: &str) -> Result<Option<(String, String)>, String> {
    #[derive(Deserialize)]
    struct RepoResp {
        default_branch: Option<String>,
    }
    #[derive(Deserialize)]
    struct RefObject {
        sha: String,
    }
    #[derive(Deserialize)]
    struct RefResp {
        object: RefObject,
    }

    let repo_url = format!("{}/repos/{owner_repo}", github_app::github_api_base());
    let resp = client()?
        .get(&repo_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GET repo: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "GET repo HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let repo: RepoResp = resp
        .json()
        .await
        .map_err(|e| format!("GET repo json: {e}"))?;
    let branch = repo
        .default_branch
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());

    let ref_url = format!(
        "{}/repos/{owner_repo}/git/ref/heads/{branch}",
        github_app::github_api_base()
    );
    let resp = client()?
        .get(&ref_url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GET ref: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "GET ref HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let r: RefResp = resp
        .json()
        .await
        .map_err(|e| format!("GET ref json: {e}"))?;
    let sha = r.object.sha.trim().to_string();
    if sha.is_empty() {
        return Ok(None);
    }
    Ok(Some((branch, sha)))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("honr")
        .build()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Origin, WebhookPollConfig};
    use crate::secrets::seal_github_app;
    use crate::store::Board;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex, MutexGuard};

    mod github_api_env {
        use super::*;
        static LOCK: Mutex<()> = Mutex::new(());

        pub struct Guard {
            prev: Option<String>,
            _lock: MutexGuard<'static, ()>,
        }

        impl Guard {
            pub fn set(base: &str) -> Self {
                let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let prev = std::env::var("HONR_GITHUB_API").ok();
                std::env::set_var("HONR_GITHUB_API", base);
                Self { prev, _lock }
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

    fn test_rsa_pem() -> String {
        include_str!("testdata/github_app_test_rsa.pem").to_string()
    }

    fn test_board(tag: &str) -> (std::path::PathBuf, SharedBoard, crate::secrets::master_key_env::Guard) {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-ghpoll-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let key_path = dir.join("master.key");
        let env = crate::secrets::master_key_env::Guard::with_key_path(&key_path);
        let board: SharedBoard = Arc::new(Board::new(
            crate::schema::Schema::default(),
            dir.join("board.json"),
        ));
        (dir, board, env)
    }

    fn seal_test_app(board: &SharedBoard) {
        let sealed = seal_github_app(&crate::secrets::GitHubAppBundle {
            app_id: "123456".into(),
            private_key_pem: test_rsa_pem(),
            ..Default::default()
        })
        .expect("seal");
        board.set_github_app_sealed(Some(sealed));
        board.set_github_app_installation_id(Some(99));
    }

    async fn spawn_poll_mock(
        merged: bool,
        tip_sha: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/app/installations/{id}/access_tokens",
                axum::routing::post(|| async {
                    Json(serde_json::json!({
                        "token": "ghs_poll_token",
                        "expires_at": "2099-01-01T00:00:00Z"
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{number}",
                get(move || async move {
                    Json(serde_json::json!({
                        "merged": merged,
                        "html_url": "https://github.com/acme/widgets/pull/7"
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}",
                get(|| async {
                    Json(serde_json::json!({ "default_branch": "main" }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/git/ref/heads/{branch}",
                get(move || async move {
                    Json(serde_json::json!({
                        "object": { "sha": tip_sha, "type": "commit" }
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
    fn normalize_clamps_interval() {
        let cfg = WebhookPollConfig {
            enabled: true,
            interval_secs: 5,
        }
        .normalized();
        assert_eq!(cfg.interval_secs, MIN_WEBHOOK_POLL_INTERVAL_SECS);
        assert!(cfg.enabled);
    }

    #[tokio::test]
    async fn tick_disabled_is_noop() {
        let (dir, board, _env) = test_board("disabled");
        board.set_webhook_poll_config(WebhookPollConfig {
            enabled: false,
            interval_secs: 60,
        });
        // Dead API — must not be contacted.
        let _api = github_api_env::Guard::set("http://127.0.0.1:1");
        tick(&board).await.expect("disabled tick");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tick_merged_pr_completes_review_card() {
        let (dir, board, _env) = test_board("merge");
        seal_test_app(&board);
        board.set_webhook_poll_config(WebhookPollConfig {
            enabled: true,
            interval_secs: 60,
        });

        let p = board
            .create(None, "P", "why", None, Origin::Human, true, None)
            .unwrap();
        let t = board
            .create(
                Some(p.id),
                "T",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = board.transition(t.id, State::Shaping, "test", None);
        let _ = board.transition(t.id, State::Backlog, "test", None);
        let _ = board.transition(t.id, State::Claimed, "test", None);
        let _ = board.transition(t.id, State::Running, "test", None);
        let _ = board.transition(t.id, State::Review, "test", None);
        board.set_pull_request(
            t.id,
            Some(crate::model::PullRequest {
                url: "https://github.com/acme/widgets/pull/7".into(),
                base: Some(crate::model::PullRequestEnd::new("acme/widgets", "main")),
                head: Some(crate::model::PullRequestEnd::new("acme/widgets", "honr/t")),
            }),
        );

        let (base, handle) = spawn_poll_mock(true, "aaa111").await;
        let _api = github_api_env::Guard::set(&base);

        tick(&board).await.expect("tick");
        assert_eq!(board.get(t.id).unwrap().state, State::Done);
        let by = board
            .get(t.id)
            .unwrap()
            .history
            .last()
            .map(|h| h.by.clone())
            .unwrap_or_default();
        assert_eq!(by, POLL_BY);

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tip_change_notifies_main_advanced_once_after_seed() {
        let (dir, board, _env) = test_board("tip");
        seal_test_app(&board);
        board.set_webhook_poll_config(WebhookPollConfig {
            enabled: true,
            interval_secs: 60,
        });

        let p = board
            .create(None, "P", "why", None, Origin::Human, true, None)
            .unwrap();
        let t = board
            .create(
                Some(p.id),
                "T",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = board.transition(t.id, State::Shaping, "test", None);
        let _ = board.transition(t.id, State::Backlog, "test", None);
        let _ = board.transition(t.id, State::Claimed, "test", None);
        let _ = board.transition(t.id, State::Running, "test", None);
        let _ = board.transition(t.id, State::Review, "test", None);
        board.set_pull_request(
            t.id,
            Some(crate::model::PullRequest {
                url: "https://github.com/acme/widgets/pull/7".into(),
                base: Some(crate::model::PullRequestEnd::new("acme/widgets", "main")),
                head: Some(crate::model::PullRequestEnd::new("acme/widgets", "honr/t")),
            }),
        );

        // Seed tip without MainAdvanced.
        board.set_webhook_poll_tip("acme/widgets", "sha_old");

        let mut events = board.subscribe();
        let (base, handle) = spawn_poll_mock(false, "sha_new").await;
        let _api = github_api_env::Guard::set(&base);

        tick(&board).await.expect("tick");
        assert_eq!(
            board.webhook_poll_tip("acme/widgets").as_deref(),
            Some("sha_new")
        );

        let mut saw_main = false;
        while let Ok(ev) = events.try_recv() {
            if matches!(ev, crate::events::BoardEvent::MainAdvanced { .. }) {
                saw_main = true;
                break;
            }
        }
        assert!(saw_main, "expected MainAdvanced after tip change");

        // Second tick with same tip must not fire again.
        let mut events2 = board.subscribe();
        tick(&board).await.expect("tick2");
        let mut again = false;
        while let Ok(ev) = events2.try_recv() {
            if matches!(ev, crate::events::BoardEvent::MainAdvanced { .. }) {
                again = true;
                break;
            }
        }
        assert!(!again, "idempotent tip must not re-fire MainAdvanced");

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_github_pr_url_feeds_targets() {
        let (owner, n) = parse_github_pr_url("https://github.com/Acme/Widgets/pull/42").unwrap();
        assert_eq!(owner, "Acme/Widgets");
        assert_eq!(n, 42);
    }
}
