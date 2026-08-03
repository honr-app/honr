//! Integration module for gastownhall/beads (`bd`).
//!
//! Plan A: beads holds identity, Project→Task containment (`--parent`), and
//! task↔task dependency edges. Honr keeps the richer lifecycle machine and
//! runtime fields (lease, sandbox, cost) keyed by board id + `beads_id`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Notify;

/// Coalesce Dolt remote pushes after Issue-sync mutations so create storms
/// don't N× `bd dolt push` the whole DB.
const DOLT_PUSH_DEBOUNCE: Duration = Duration::from_secs(30);

/// A remote side-effect `BeadsClient` would perform (`bd github …` / `bd dolt push`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    GithubPush(Vec<String>),
    GithubSync,
    DoltPush { remote: String },
}

/// Shared log of [`RemoteOp`]s for tests (never shells out).
#[derive(Clone, Default)]
pub struct RemoteCapture {
    ops: Arc<Mutex<Vec<RemoteOp>>>,
}

impl std::fmt::Debug for RemoteCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.ops.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("RemoteCapture").field("ops", &n).finish()
    }
}

#[allow(dead_code)] // constructed from #[cfg(test)] seams; keep usable in dry-run later
impl RemoteCapture {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ops(&self) -> Vec<RemoteOp> {
        self.ops.lock().expect("remote capture lock").clone()
    }

    pub fn take(&self) -> Vec<RemoteOp> {
        std::mem::take(&mut *self.ops.lock().expect("remote capture lock"))
    }

    fn record(&self, op: RemoteOp) {
        self.ops.lock().expect("remote capture lock").push(op);
    }
}

/// Whether GitHub Issue sync / Dolt push may leave the machine.
///
/// [`BeadsClient::new`] defaults to [`Remotes::Capture`] under `cargo test`
/// and [`Remotes::Live`] in the real binary — ambient `gh` auth used to turn
/// temp-dir unit tests into real shanemcd/honr Issues. Assert on the capture
/// when you care about the remote edge; the ignored live e2e uses `Live`.
#[derive(Clone, Debug, Default)]
pub enum Remotes {
    /// Shell out to `bd` (production).
    #[default]
    Live,
    /// Drop remote ops (explicit opt-out; not the test default).
    #[allow(dead_code)]
    Disabled,
    /// Record ops; never leave the machine (test default via [`BeadsClient::new`]).
    #[allow(dead_code)]
    Capture(RemoteCapture),
}

enum RemoteGate {
    /// Do not shell out (Disabled recorded nothing; Capture already logged).
    Skip,
    /// Perform the real `bd` / git remote call.
    Proceed,
}

struct DoltPushDebouncer {
    pending: AtomicBool,
    worker_started: AtomicBool,
    notify: Notify,
}

impl Default for DoltPushDebouncer {
    fn default() -> Self {
        Self {
            pending: AtomicBool::new(false),
            worker_started: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct BeadsIssue {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub issue_type: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub external_ref: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub issue_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl BeadsIssue {
    pub fn github_issue_url(&self) -> Option<String> {
        let candidates = [
            self.issue_url.as_deref(),
            self.url.as_deref(),
            self.external_ref.as_deref(),
            self.external_id.as_deref(),
        ];
        for candidate in candidates.into_iter().flatten() {
            let trimmed = candidate.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                return Some(trimmed.to_string());
            }
            if let Some(num) = trimmed.strip_prefix("gh-") {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                    let repo = std::env::var("GITHUB_REPOSITORY")
                        .unwrap_or_else(|_| "shanemcd/honr".to_string());
                    return Some(format!("https://github.com/{repo}/issues/{num}"));
                }
            }
            if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
                let repo = std::env::var("GITHUB_REPOSITORY")
                    .unwrap_or_else(|_| "shanemcd/honr".to_string());
                return Some(format!("https://github.com/{repo}/issues/{trimmed}"));
            }
        }
        None
    }
}

#[derive(Clone)]
pub struct BeadsClient {
    pub beads_dir: PathBuf,
    remotes: Remotes,
    dolt_push: Arc<DoltPushDebouncer>,
}

impl std::fmt::Debug for BeadsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BeadsClient")
            .field("beads_dir", &self.beads_dir)
            .field("remotes", &self.remotes)
            .finish()
    }
}

#[allow(dead_code)] // Plan A API surface — not every verb is on the hot path yet.
impl BeadsClient {
    /// Production: live remotes. Under `cargo test`: capture (no network).
    pub fn new(beads_dir: impl Into<PathBuf>) -> Self {
        Self::with_remotes(beads_dir, Self::default_remotes())
    }

    pub fn with_remotes(beads_dir: impl Into<PathBuf>, remotes: Remotes) -> Self {
        Self {
            beads_dir: beads_dir.into(),
            remotes,
            dolt_push: Arc::new(DoltPushDebouncer::default()),
        }
    }

    fn default_remotes() -> Remotes {
        if cfg!(test) {
            Remotes::Capture(RemoteCapture::new())
        } else {
            Remotes::Live
        }
    }

    pub fn remotes(&self) -> &Remotes {
        &self.remotes
    }

    /// Shared capture log when remotes are [`Remotes::Capture`].
    pub fn remote_capture(&self) -> Option<&RemoteCapture> {
        match &self.remotes {
            Remotes::Capture(cap) => Some(cap),
            Remotes::Live | Remotes::Disabled => None,
        }
    }

    fn gate_remote(&self, op: RemoteOp) -> RemoteGate {
        match &self.remotes {
            Remotes::Live => RemoteGate::Proceed,
            Remotes::Disabled => RemoteGate::Skip,
            Remotes::Capture(cap) => {
                cap.record(op);
                RemoteGate::Skip
            }
        }
    }

    pub fn cmd(&self) -> Command {
        let mut c = Command::new("bd");
        c.env("BEADS_DIR", &self.beads_dir);
        // Prevent `bd` from walking up into a parent workspace's `.beads`.
        // Relative beads dirs like `.beads` have an empty Path parent (`""`);
        // setting that as current_dir makes the spawn fail with ENOENT even
        // when `bd` is on PATH — which blocked every github sync mirror.
        if let Some(parent) = self.beads_dir.parent() {
            if !parent.as_os_str().is_empty() {
                c.current_dir(parent);
            }
        }
        c
    }

    /// Sync `bd` spawn — used from `Board::create` so we never
    /// `Handle::block_on` an async create on the runtime that owns the board.
    fn sync_cmd(&self) -> std::process::Command {
        let mut c = std::process::Command::new("bd");
        c.env("BEADS_DIR", &self.beads_dir);
        if let Some(parent) = self.beads_dir.parent() {
            if !parent.as_os_str().is_empty() {
                c.current_dir(parent);
            }
        }
        c
    }

    /// Namespaced orchestrator metadata (`bd create/update --metadata`).
    pub fn honr_metadata(item_id: u64, pr_url: Option<&str>) -> String {
        let mut honr = serde_json::Map::new();
        honr.insert("item_id".into(), serde_json::json!(item_id));
        if let Some(url) = pr_url.filter(|u| !u.trim().is_empty()) {
            honr.insert("pr_url".into(), serde_json::json!(url));
        }
        serde_json::json!({ "honr": honr }).to_string()
    }

    /// True when `id` looks like a real beads hash, not a local placeholder.
    pub fn is_real_id(id: &str) -> bool {
        !id.is_empty() && !id.starts_with("bd-honr-")
    }

    fn db_ready(&self) -> bool {
        self.beads_dir.join("metadata.json").exists()
            || self.beads_dir.join("embeddeddolt").exists()
    }

    /// Apply GitHub auth env so `git+https://` Dolt remotes can push `refs/dolt/data`.
    fn apply_github_git_auth(cmd: &mut Command) {
        let token = Self::resolve_github_token().unwrap_or_default();
        if token.is_empty() {
            return;
        }
        cmd.env("GITHUB_TOKEN", &token);
        // Prefer `gh` on PATH as the credential helper (no hardcoded brew path).
        let gh_ok = std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if gh_ok {
            cmd.env("GIT_CONFIG_COUNT", "1");
            cmd.env("GIT_CONFIG_KEY_0", "credential.helper");
            cmd.env("GIT_CONFIG_VALUE_0", "!gh auth git-credential");
        }
    }

    /// Request a debounced `bd dolt push origin` (publishes `refs/dolt/data`).
    ///
    /// Safe to call from many mutation paths; overlapping requests coalesce.
    /// Never blocks the caller; failures are logged by the background worker.
    pub fn schedule_dolt_push(&self) {
        if !self.db_ready() {
            return;
        }
        match self.gate_remote(RemoteOp::DoltPush {
            remote: "origin".into(),
        }) {
            RemoteGate::Skip => return,
            RemoteGate::Proceed => {}
        }
        self.dolt_push.pending.store(true, Ordering::SeqCst);
        self.ensure_dolt_push_worker();
        self.dolt_push.notify.notify_one();
    }

    fn ensure_dolt_push_worker(&self) {
        if self
            .dolt_push
            .worker_started
            .swap(true, Ordering::SeqCst)
        {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No runtime (e.g. sync test context) — leave pending; next schedule
            // under a runtime will start the worker.
            self.dolt_push.worker_started.store(false, Ordering::SeqCst);
            return;
        };
        let client = self.clone();
        handle.spawn(async move {
            loop {
                client.dolt_push.notify.notified().await;
                // Trailing debounce: wait so create storms collapse to one push.
                tokio::time::sleep(DOLT_PUSH_DEBOUNCE).await;
                if !client.dolt_push.pending.swap(false, Ordering::SeqCst) {
                    continue;
                }
                match client.sync_remote(Some("origin")).await {
                    Ok(()) => tracing::info!("beads dolt push to origin ok"),
                    Err(e) => tracing::warn!(error = %e, "beads dolt push to origin failed"),
                }
            }
        });
    }

    /// Run `bd init --quiet --stealth` in the target directory.
    pub async fn init_stealth(&self) -> Result<(), String> {
        self.init_stealth_sync()
    }

    fn init_stealth_sync(&self) -> Result<(), String> {
        if self.beads_dir.join("metadata.json").exists()
            || self.beads_dir.join("embeddeddolt").exists()
        {
            return Ok(());
        }
        std::fs::create_dir_all(&self.beads_dir)
            .map_err(|e| format!("mkdir beads dir: {e}"))?;
        let status = self
            .sync_cmd()
            .args(["init", "--quiet", "--stealth", "--prefix", "honr", "--remote", ""])
            .status()
            .map_err(|e| format!("failed to execute bd init: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            // Stealth init can succeed with non-zero when already initialised.
            Ok(())
        }
    }

    /// Run `bd ready --json --exclude-type=epic` (and optional `--parent=<epic>`) to fetch task-only ready work.
    pub async fn list_ready_focused(&self, parent: Option<&str>) -> Result<Vec<BeadsIssue>, String> {
        let mut cmd = self.cmd();
        cmd.args(["ready", "--json", "--exclude-type=epic"]);
        if let Some(p) = parent.filter(|p| !p.trim().is_empty() && Self::is_real_id(p)) {
            cmd.arg("--parent").arg(p);
        }
        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd ready: {e}"))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd ready failed: {err}"));
        }

        if out.stdout.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<BeadsIssue> = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd ready JSON: {e}"))?;
        Ok(items.into_iter().filter(|i| i.issue_type != "epic").collect())
    }

    /// Run `bd ready --json --exclude-type=epic` to fetch unblocked ready tasks.
    pub async fn list_ready(&self) -> Result<Vec<BeadsIssue>, String> {
        self.list_ready_focused(None).await
    }

    /// List issues (`bd list --json --all -n 0`).
    pub async fn list_all(&self) -> Result<Vec<BeadsIssue>, String> {
        let out = self
            .cmd()
            .args(["list", "--json", "--all", "-n", "0"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd list: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd list failed: {err}"));
        }
        if out.stdout.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd list JSON: {e}"))
    }

    /// `bd show <id> --json`
    pub async fn show(&self, id: &str) -> Result<BeadsIssue, String> {
        let out = self
            .cmd()
            .args(["show", id, "--json"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd show: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd show failed: {err}"));
        }
        if let Ok(issues) = serde_json::from_slice::<Vec<BeadsIssue>>(&out.stdout) {
            return issues
                .into_iter()
                .next()
                .ok_or_else(|| format!("issue {id} not found"));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd show JSON: {e}"))
    }

    /// Create without parent/deps (compat).
    pub async fn create(
        &self,
        title: &str,
        priority: u32,
        issue_type: &str,
        description: Option<&str>,
    ) -> Result<BeadsIssue, String> {
        self.create_linked(title, priority, issue_type, description, None, &[], None)
            .await
    }

    /// Create with optional Project parent and blocker deps.
    ///
    /// - Projects → `issue_type=epic`
    /// - Tasks → `issue_type=task` with `--parent=<project beads id>`
    /// - `blocked_by` → `blocks:` deps at create time, plus `bd dep add` fallback
    /// - `metadata` → `--metadata` JSON (honr namespaced fields)
    #[allow(clippy::too_many_arguments)]
    pub async fn create_linked(
        &self,
        title: &str,
        priority: u32,
        issue_type: &str,
        description: Option<&str>,
        parent: Option<&str>,
        blocked_by: &[String],
        metadata: Option<&str>,
    ) -> Result<BeadsIssue, String> {
        // Prefer the sync path so create/mirror share one argv shape; spawn_blocking
        // keeps the async runtime free while `bd` talks to Dolt.
        let this = self.clone();
        let title = title.to_string();
        let issue_type = issue_type.to_string();
        let description = description.map(str::to_string);
        let parent = parent.map(str::to_string);
        let blocked_by = blocked_by.to_vec();
        let metadata = metadata.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            this.create_linked_sync(
                &title,
                priority,
                &issue_type,
                description.as_deref(),
                parent.as_deref(),
                &blocked_by,
                metadata.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("bd create join: {e}"))?
    }

    /// Synchronous create for `Board::create` (must not nest `block_on`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_linked_sync(
        &self,
        title: &str,
        priority: u32,
        issue_type: &str,
        description: Option<&str>,
        parent: Option<&str>,
        blocked_by: &[String],
        metadata: Option<&str>,
    ) -> Result<BeadsIssue, String> {
        let _ = self.init_stealth_sync();
        let mut cmd = self.sync_cmd();
        cmd.arg("create")
            .arg(title)
            .arg("-p")
            .arg(priority.to_string())
            .arg("-t")
            .arg(issue_type)
            .arg("--json");

        if let Some(desc) = description {
            cmd.arg("-d").arg(desc);
        }
        if let Some(p) = parent.filter(|p| Self::is_real_id(p)) {
            cmd.arg("--parent").arg(p);
        }
        let deps: Vec<String> = blocked_by
            .iter()
            .filter(|b| Self::is_real_id(b))
            .map(|b| format!("blocks:{b}"))
            .collect();
        if !deps.is_empty() {
            cmd.arg("--deps").arg(deps.join(","));
        }
        if let Some(meta) = metadata.filter(|m| !m.trim().is_empty()) {
            cmd.arg("--metadata").arg(meta);
        }

        let out = cmd
            .output()
            .map_err(|e| format!("failed to execute bd create: {e}"))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd create failed: {err}"));
        }

        let issue: BeadsIssue = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd create output: {e}"))?;

        // `--deps` should have wired blockers; if bd ignored them, add explicitly.
        for blocker in blocked_by.iter().filter(|b| Self::is_real_id(b)) {
            let _ = self.dep_add_sync(&issue.id, blocker, "blocks");
        }

        Ok(issue)
    }

    fn dep_add_sync(&self, issue_id: &str, depends_on: &str, dep_type: &str) -> Result<(), String> {
        if !Self::is_real_id(issue_id) || !Self::is_real_id(depends_on) {
            return Ok(());
        }
        let out = self
            .sync_cmd()
            .args(["dep", "add", issue_id, depends_on, "-t", dep_type])
            .output()
            .map_err(|e| format!("failed to execute bd dep add: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd dep add failed: {err}"))
        }
    }

    /// `bd update <id>` for title / description / metadata write-through.
    pub async fn update_fields(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        if title.is_none() && description.is_none() && metadata.is_none() {
            return Ok(());
        }
        let mut cmd = self.cmd();
        cmd.arg("update").arg(id);
        if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
            cmd.arg("--title").arg(t);
        }
        if let Some(d) = description {
            cmd.arg("-d").arg(d);
        }
        if let Some(m) = metadata.filter(|m| !m.trim().is_empty()) {
            cmd.arg("--metadata").arg(m);
        }
        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd update: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd update fields failed: {err}"))
        }
    }

    /// `bd update <id> --status <status>`
    pub async fn set_status(&self, id: &str, status: &str) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["update", id, "--status", status])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd update: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd update status failed: {err}"))
        }
    }

    /// Resolve GitHub token from `GITHUB_TOKEN`, else `gh auth token` on PATH.
    pub fn resolve_github_token() -> Option<String> {
        Self::resolve_github_token_with(
            || std::env::var("GITHUB_TOKEN").ok(),
            || {
                std::process::Command::new("gh")
                    .args(["auth", "token"])
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if stdout.is_empty() {
                                None
                            } else {
                                Some(stdout)
                            }
                        } else {
                            None
                        }
                    })
            },
        )
    }

    fn resolve_github_token_with<E, C>(get_env: E, run_gh: C) -> Option<String>
    where
        E: Fn() -> Option<String>,
        C: FnOnce() -> Option<String>,
    {
        if let Some(token) = get_env().filter(|t| !t.trim().is_empty()) {
            return Some(token.trim().to_string());
        }
        run_gh()
    }

    /// Push specific beads to GitHub (`bd github push <ids…>`).
    ///
    /// Prefer this over a full `github sync --push-only`: the beads docs make
    /// `push` / `sync --issues` the selective path. A whole-graph sync after
    /// every card create is what made Issue creation crawl.
    pub async fn github_push(&self, ids: &[String]) -> Result<(), String> {
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| Self::is_real_id(id))
            .cloned()
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        // Record Capture/Disabled before db_ready so tests see the intent even
        // when a temp beads dir is mid-init.
        match self.gate_remote(RemoteOp::GithubPush(ids.clone())) {
            RemoteGate::Skip => {
                if self.db_ready() {
                    let repo = std::env::var("GITHUB_REPOSITORY")
                        .unwrap_or_else(|_| "shanemcd/honr".to_string());
                    for id in &ids {
                        let _ = self
                            .cmd()
                            .args([
                                "update",
                                id,
                                "--external-ref",
                                &format!("https://github.com/{repo}/issues/{id}"),
                            ])
                            .output()
                            .await;
                    }
                }
                return Ok(());
            }
            RemoteGate::Proceed => {}
        }
        if !self.db_ready() {
            return Ok(());
        }
        let token = Self::resolve_github_token().unwrap_or_default();

        let mut cmd = self.cmd();
        cmd.arg("github").arg("push");
        for id in &ids {
            cmd.arg(id);
        }

        if !token.is_empty() {
            cmd.env("GITHUB_TOKEN", token);
        }

        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd github push failed: {err}"))
        }
    }

    /// Full-graph push (`bd github sync --push-only`). Prefer [`Self::github_push`].
    pub async fn github_sync(&self) -> Result<(), String> {
        if !self.db_ready() {
            return Ok(());
        }
        match self.gate_remote(RemoteOp::GithubSync) {
            RemoteGate::Skip => return Ok(()),
            RemoteGate::Proceed => {}
        }

        let repo_full = std::env::var("GITHUB_REPOSITORY")
            .unwrap_or_else(|_| "shanemcd/honr".to_string());

        let mut cmd = Command::new("sh");
        cmd.env("BEADS_DIR", &self.beads_dir);
        if let Some(parent) = self.beads_dir.parent() {
            cmd.current_dir(parent);
        }

        if let Some((owner, repo)) = repo_full.split_once('/') {
            cmd.env("GITHUB_OWNER", std::env::var("GITHUB_OWNER").unwrap_or_else(|_| owner.to_string()));
            cmd.env("GITHUB_REPO", std::env::var("GITHUB_REPO").unwrap_or_else(|_| repo.to_string()));
        }

        cmd.args(["-c", "bd github sync --push-only"]);

        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd github sync failed: {err}"))
        }
    }

    /// Run `bd update <id> --claim` to claim a task.
    pub async fn claim(&self, id: &str) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["update", id, "--claim"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd claim: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd claim failed: {err}"))
        }
    }

    /// `bd dep add <issue> <depends-on> -t <type>`
    ///
    /// `issue` depends on / is blocked by `depends_on` when type is `blocks`.
    pub async fn dep_add(
        &self,
        issue_id: &str,
        depends_on: &str,
        dep_type: &str,
    ) -> Result<(), String> {
        if !Self::is_real_id(issue_id) || !Self::is_real_id(depends_on) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["dep", "add", issue_id, depends_on, "-t", dep_type])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd dep add: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd dep add failed: {err}"))
        }
    }

    /// Run `bd close <id> --reason <reason>` to mark a task completed.
    pub async fn close(&self, id: &str, reason: Option<&str>) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let mut cmd = self.cmd();
        cmd.arg("close").arg(id);

        if let Some(r) = reason {
            cmd.arg("--reason").arg(r);
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd close: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd close failed: {err}"))
        }
    }

    /// Run `bd remember "insight"` to store persistent project memories.
    pub async fn remember(&self, insight: &str) -> Result<(), String> {
        let out = self
            .cmd()
            .args(["remember", insight])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd remember: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd remember failed: {err}"))
        }
    }

    /// Run `bd prime` to get system prompt context injection for agents.
    pub async fn prime(&self) -> Result<String, String> {
        let out = self
            .cmd()
            .arg("prime")
            .output()
            .await
            .map_err(|e| format!("failed to execute bd prime: {e}"))?;

        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd prime failed: {err}"))
        }
    }

    /// Run `bd dolt push <remote>` to publish Dolt state to `refs/dolt/data`.
    pub async fn sync_remote(&self, remote: Option<&str>) -> Result<(), String> {
        if !self.db_ready() {
            return Ok(());
        }
        let target_remote = remote.unwrap_or("origin");
        match self.gate_remote(RemoteOp::DoltPush {
            remote: target_remote.to_string(),
        }) {
            RemoteGate::Skip => return Ok(()),
            RemoteGate::Proceed => {}
        }
        let mut cmd = self.cmd();
        cmd.args(["dolt", "push", target_remote]);
        Self::apply_github_git_auth(&mut cmd);

        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd dolt push: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd dolt push failed: {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[tokio::test]
    async fn beads_client_lifecycle_test() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-test-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let client = BeadsClient::new(&test_dir);
        client.init_stealth().await.expect("init stealth");

        let project = client
            .create_linked(
                "Project Test",
                0,
                "epic",
                Some("Test Project Description"),
                None,
                &[],
                Some(r#"{"honr":{"item_id":1}}"#),
            )
            .await
            .expect("create project");
        assert_eq!(project.title, "Project Test");

        let task = client
            .create_linked(
                "Task Test",
                1,
                "task",
                None,
                Some(&project.id),
                &[],
                Some(r#"{"honr":{"item_id":2}}"#),
            )
            .await
            .expect("create task");

        let ready_before_claim = client.list_ready().await.expect("list ready");
        assert!(
            ready_before_claim.iter().any(|i| i.id == task.id),
            "expected open task in ready work before claim"
        );
        assert!(
            !ready_before_claim.iter().any(|i| i.id == project.id),
            "epics must be excluded from ready work"
        );

        client.claim(&task.id).await.expect("claim task");
        client
            .remember("Test insight for beads")
            .await
            .expect("remember insight");

        let prime_out = client.prime().await.expect("prime");
        assert!(prime_out.contains("Test insight for beads"));

        client
            .close(&task.id, Some("Done"))
            .await
            .expect("close task");
    }

    #[test]
    fn test_token_env_wins() {
        let mut gh_called = false;
        let token = BeadsClient::resolve_github_token_with(
            || Some("env_token_secret".to_string()),
            || {
                gh_called = true;
                Some("gh_token".to_string())
            },
        );
        assert_eq!(token, Some("env_token_secret".to_string()));
        assert!(
            !gh_called,
            "gh command should not be executed when GITHUB_TOKEN env is set"
        );
    }

    #[test]
    fn test_gh_auth_token_fallback() {
        let token =
            BeadsClient::resolve_github_token_with(|| None, || Some("gh_token_123".to_string()));
        assert_eq!(token, Some("gh_token_123".to_string()));
    }

    #[test]
    fn test_token_resolution_returns_none_when_all_fail() {
        let token = BeadsClient::resolve_github_token_with(|| None, || None);
        assert_eq!(token, None);
    }

    #[test]
    fn relative_beads_dir_parent_is_empty_so_cmd_must_skip_chdir() {
        // Rust Path: parent of `.beads` is `""`. chdir("") ⇒ ENOENT on spawn.
        let parent = std::path::Path::new(".beads").parent().expect("parent");
        assert!(parent.as_os_str().is_empty());
        let client = BeadsClient::new(".beads");
        // Constructing the command must not panic; spawn is covered by integration
        // once honr resolves an absolute beads_dir from the board path.
        let _ = client.cmd();
    }

    #[test]
    fn test_new_captures_remotes_under_cargo_test() {
        let client = BeadsClient::new(temp_dir().join("honr-beads-default-remotes"));
        assert!(
            client.remote_capture().is_some(),
            "BeadsClient::new must Capture remotes under cargo test (not Live)"
        );
    }

    #[tokio::test]
    async fn test_github_sync_skips_when_no_beads_dir() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-empty-test-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let client = BeadsClient::new(&test_dir);
        assert!(client.github_sync().await.is_ok());
        assert!(client.github_push(&["honr-abc".into()]).await.is_ok());
    }

    #[tokio::test]
    async fn test_github_push_skips_placeholder_ids() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-push-skip-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        // Even with a fake beads dir present, placeholders must not invoke `bd`.
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("metadata.json"), "{}").unwrap();
        let cap = RemoteCapture::new();
        let client = BeadsClient::with_remotes(&test_dir, Remotes::Capture(cap.clone()));
        assert!(client.github_push(&["bd-honr-1".into()]).await.is_ok());
        assert!(
            cap.ops().is_empty(),
            "placeholders must not record a remote push"
        );
    }

    #[tokio::test]
    async fn test_github_push_capture_records_real_ids() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-push-cap-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("metadata.json"), "{}").unwrap();
        let cap = RemoteCapture::new();
        let client = BeadsClient::with_remotes(&test_dir, Remotes::Capture(cap.clone()));
        assert!(
            client
                .github_push(&["honr-abc".into(), "bd-honr-1".into()])
                .await
                .is_ok()
        );
        assert_eq!(
            cap.take(),
            vec![RemoteOp::GithubPush(vec!["honr-abc".into()])]
        );
    }

    #[tokio::test]
    async fn test_sync_remote_skips_when_no_beads_dir() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-dolt-skip-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let client = BeadsClient::new(&test_dir);
        assert!(client.sync_remote(Some("origin")).await.is_ok());
        // No DB → schedule is a no-op (must not start a worker that pushes).
        client.schedule_dolt_push();
        assert!(!client.dolt_push.pending.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_schedule_dolt_push_capture_when_db_ready() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-dolt-sched-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("metadata.json"), "{}").unwrap();
        let cap = RemoteCapture::new();
        let client = BeadsClient::with_remotes(&test_dir, Remotes::Capture(cap.clone()));
        client.schedule_dolt_push();
        assert_eq!(
            cap.take(),
            vec![RemoteOp::DoltPush {
                remote: "origin".into()
            }]
        );
        // Capture must not arm the live debouncer.
        assert!(!client.dolt_push.pending.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_schedule_dolt_push_marks_pending_when_live() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-dolt-live-sched-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("metadata.json"), "{}").unwrap();
        // Live arms the worker; the eventual push will fail (no real remote) —
        // we only assert the pending bit, then drop the client with the test.
        let client = BeadsClient::with_remotes(&test_dir, Remotes::Live);
        client.schedule_dolt_push();
        assert!(client.dolt_push.pending.load(Ordering::SeqCst));
    }

    #[test]
    fn test_beads_issue_github_issue_url_parsing() {
        let mut issue = BeadsIssue {
            id: "bd-1".into(),
            title: "Title".into(),
            description: None,
            status: "open".into(),
            priority: 2,
            issue_type: "task".into(),
            owner: None,
            created_at: None,
            updated_at: None,
            external_ref: None,
            external_id: None,
            issue_url: None,
            url: None,
        };

        assert_eq!(issue.github_issue_url(), None);

        issue.external_ref = Some("https://github.com/shanemcd/honr/issues/100".into());
        assert_eq!(
            issue.github_issue_url(),
            Some("https://github.com/shanemcd/honr/issues/100".into())
        );

        issue.external_ref = Some("gh-101".into());
        assert_eq!(
            issue.github_issue_url(),
            Some("https://github.com/shanemcd/honr/issues/101".into())
        );

        issue.external_ref = Some("102".into());
        assert_eq!(
            issue.github_issue_url(),
            Some("https://github.com/shanemcd/honr/issues/102".into())
        );

        issue.issue_url = Some("https://github.com/shanemcd/honr/issues/103".into());
        assert_eq!(
            issue.github_issue_url(),
            Some("https://github.com/shanemcd/honr/issues/103".into())
        );
    }

    #[tokio::test]
    async fn test_list_ready_focused_excludes_epics() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-ready-epic-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let client = BeadsClient::new(&test_dir);
        client.init_stealth().await.expect("init stealth");

        let project = client
            .create_linked(
                "Epic Project",
                0,
                "epic",
                Some("Epic description"),
                None,
                &[],
                None,
            )
            .await
            .expect("create epic");

        let task = client
            .create_linked(
                "Task Work Item",
                1,
                "task",
                Some("Task description"),
                Some(&project.id),
                &[],
                None,
            )
            .await
            .expect("create task");

        let ready = client.list_ready_focused(None).await.expect("list_ready_focused");
        assert!(ready.iter().any(|i| i.id == task.id), "ready tasks should include task item");
        assert!(!ready.iter().any(|i| i.issue_type == "epic"), "ready tasks MUST NOT include epics");

        let scoped = client.list_ready_focused(Some(&project.id)).await.expect("scoped ready");
        assert!(scoped.iter().any(|i| i.id == task.id), "scoped ready should include child task");
        assert!(!scoped.iter().any(|i| i.issue_type == "epic"), "scoped ready MUST NOT include epics");
    }
}
