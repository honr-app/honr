//! Project + Task — the only two rungs.
//!
//! Project is the container; Tasks are flat claimable leaves under it. The
//! engine still reads the tree (parent edges) for containment; task↔task
//! ordering lives in board dependency edges (`blocked_by`).

use crate::db::BoardDatabaseConfig;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Level {
    pub name: String,
    #[serde(default)]
    pub horizon: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub elaborate: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub claimable: bool,
}

/// How work actually gets executed. The run budget is
/// `agents.agent_timeout_secs`; `lease_secs` / `heartbeat_expect_secs` are
/// ignored leftovers kept so older `honr.yaml` files still parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Deprecated — ignored. Run deadline is `agents.agent_timeout_secs`.
    #[serde(default = "d_lease")]
    pub lease_secs: i64,
    /// Deprecated — ignored. UI shows countdown to `run_deadline_at`.
    #[serde(default = "d_hb")]
    pub heartbeat_expect_secs: i64,
    /// How often to check for overdue run deadlines.
    #[serde(default = "d_sweep")]
    pub sweep_interval_ms: u64,
    /// Real agents in real sandboxes. Off by default: the board must still run
    /// on a machine with no podman, no gateway and no credentials.
    #[serde(default)]
    pub agents: AgentConfig,
}

fn d_lease() -> i64 { 600 }
fn d_hb() -> i64 { 6 }
fn d_sweep() -> u64 { 2000 }

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            lease_secs: d_lease(),
            heartbeat_expect_secs: d_hb(),
            sweep_interval_ms: d_sweep(),
            agents: AgentConfig::default(),
        }
    }
}

/// Validate a GitHub-style `owner/name` (no URL, no trailing `.git`).
pub fn parse_owner_name(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("clone_repo is required (`owner/name`)".into());
    }
    if s.contains("://") || s.starts_with("git@") {
        return Err(format!(
            "clone_repo must be `owner/name`, not a URL ({s})"
        ));
    }
    let s = s.strip_suffix(".git").unwrap_or(s);
    let mut parts = s.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "clone_repo must be exactly `owner/name` (got {s:?})"
        ));
    };
    if owner.is_empty()
        || name.is_empty()
        || !owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "clone_repo must be a valid `owner/name` (got {s:?})"
        ));
    }
    Ok(format!("{owner}/{name}"))
}

/// Standing line stamped into Project intent / Initial plan prose.
pub fn clone_repo_prose_line(owner_name: &str) -> String {
    format!(
        "Clone repository: {owner_name} into /sandbox/repo for planning and as the default Task clone target."
    )
}

/// Pull `owner/name` out of stamped prose (`Clone repository: owner/name …`).
pub fn clone_repo_from_prose(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        let Some(rest) = t
            .strip_prefix("Clone repository:")
            .or_else(|| t.strip_prefix("clone repository:"))
        else {
            continue;
        };
        let Some(token) = rest.split_whitespace().next() else {
            continue;
        };
        if let Ok(name) = parse_owner_name(token) {
            return Some(name);
        }
    }
    None
}

/// Resolved remotes for one card run (from card `pull_request` base/head).
///
/// Before a PR exists, `resolve_card_repo` returns `None` and the agent clones
/// from card prose. `upstream` = PR base repo; `fork` = head/push repo (same
/// as upstream for same-repo). Yaml `execution.agents.repo` is legacy/optional.
/// Containment is forge token permissions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct RepoConfig {
    /// `owner/name` that PRs target.
    pub upstream: String,
    /// Optional distinct push remote (`owner/name`). Empty → same-repo.
    #[serde(default)]
    pub fork: String,
    #[serde(default = "d_base")]
    pub base: String,
}

fn d_base() -> String { "main".into() }

impl Default for RepoConfig {
    fn default() -> Self {
        Self { upstream: String::new(), fork: String::new(), base: d_base() }
    }
}

impl RepoConfig {
    /// Usable when the PR-target repo is known. Fork is optional.
    pub fn is_complete(&self) -> bool {
        !self.upstream.trim().is_empty()
    }

    /// Distinct push remote configured (cross-fork workflow).
    pub fn uses_cross_fork(&self) -> bool {
        let f = self.fork.trim();
        let u = self.upstream.trim();
        !f.is_empty() && !u.is_empty() && f != u
    }

    /// Clone and push target: fork when cross-fork, else upstream.
    pub fn clone_target(&self) -> &str {
        if self.uses_cross_fork() {
            self.fork.trim()
        } else {
            self.upstream.trim()
        }
    }

    /// Git ref to rebase onto / start from (`upstream/<base>` or `origin/<base>`).
    pub fn base_ref(&self) -> String {
        if self.uses_cross_fork() {
            format!("upstream/{}", self.base.trim())
        } else {
            format!("origin/{}", self.base.trim())
        }
    }

    /// Normalize empty base to `main`; trim owner/name fields.
    pub fn normalized(mut self) -> Self {
        if self.base.trim().is_empty() {
            self.base = d_base();
        }
        self.upstream = self.upstream.trim().to_string();
        self.fork = self.fork.trim().to_string();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Passed to `--from`. Needs a Rust toolchain to build honr; see
    /// `sandbox/Containerfile`.
    #[serde(default = "d_image")]
    pub image: String,
    /// Host path to OpenShell policy YAML used only to **seed** / fall back when
    /// the board catalog is empty. Catalog profiles store the YAML text itself.
    #[serde(default = "d_policy")]
    pub policy: String,
    #[serde(default)]
    pub repo: RepoConfig,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    /// Sandboxes are heavy and this is alpha software. Do not start at seven.
    /// Primary agent CLI engine (`cursor`, `agy`, or `claude`).
    #[serde(default = "d_engine")]
    pub engine: String,
    #[serde(default = "d_concurrent")]
    pub max_concurrent: usize,
    /// Hard ceiling on one agent run. Everything here fails as a hang.
    #[serde(default = "d_agent_timeout")]
    pub agent_timeout_secs: u64,
    /// Runs that die without producing work before the card becomes a human's
    /// problem. Without a count, early failures requeue forever.
    #[serde(default = "d_max_attempts")]
    pub max_attempts: u32,
    /// Git branch / sandbox name stem. Branches are `{prefix}/card-{id}`;
    /// sandboxes are `{slug}-card-{id}-a{n}`. Default `honr`.
    #[serde(default = "d_branch_prefix")]
    pub branch_prefix: String,
}

fn d_image() -> String { "honr-sandbox:latest".into() }
/// Marker: resolve to the built-in worker seed policy (not a host file).
fn d_policy() -> String { "embedded".into() }
fn d_engine() -> String { "cursor".into() }
fn d_concurrent() -> usize { 2 }
fn d_agent_timeout() -> u64 { 1800 }
fn d_max_attempts() -> u32 { 3 }
fn d_branch_prefix() -> String { "honr".into() }

/// Normalize a branch prefix: trim, strip surrounding `/`, fall back to `honr`.
pub fn normalize_branch_prefix(prefix: &str) -> String {
    let p = prefix.trim().trim_matches('/');
    if p.is_empty() {
        d_branch_prefix()
    } else {
        p.to_string()
    }
}

/// OpenShell-safe slug of the branch prefix (`/` → `-`).
pub fn sandbox_prefix_slug(prefix: &str) -> String {
    normalize_branch_prefix(prefix).replace('/', "-")
}

/// Card feature branch: `{prefix}/card-{id}`.
pub fn card_branch_name(prefix: &str, id: impl std::fmt::Display) -> String {
    format!("{}/card-{}", normalize_branch_prefix(prefix), id)
}

/// Sandbox name: `{slug}-card-{id}-a{attempt}`.
pub fn card_sandbox_name(prefix: &str, id: impl std::fmt::Display, attempt: u32) -> String {
    format!("{}-card-{}-a{}", sandbox_prefix_slug(prefix), id, attempt)
}

/// Prefix match stem for reconcile keep: `{slug}-card-{id}-`.
pub fn card_sandbox_stem(prefix: &str, id: impl std::fmt::Display) -> String {
    format!("{}-card-{}-", sandbox_prefix_slug(prefix), id)
}

/// Stable singleton name for the control-plane cockpit: `{slug}-cockpit`.
pub fn cockpit_sandbox_name(prefix: &str) -> String {
    format!("{}-cockpit", sandbox_prefix_slug(prefix))
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            image: d_image(),
            policy: d_policy(),
            repo: RepoConfig::default(),
            cpu: None,
            memory: None,
            engine: d_engine(),
            max_concurrent: d_concurrent(),
            agent_timeout_secs: d_agent_timeout(),
            max_attempts: d_max_attempts(),
            branch_prefix: d_branch_prefix(),
        }
    }
}

impl AgentConfig {
    /// Refuse to run rather than half-run. Every one of these presents as a
    /// hang if it's wrong at exec time, so check it at startup instead.
    ///
    /// Work remotes (`repo.upstream`, optional `fork`) are **not** required
    /// here: they resolve per card from `pr_url` and yaml (see
    /// `Board::resolve_card_repo`). An incomplete install default only fails
    /// when a card has no `pr_url` and no yaml upstream.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        // Live policy is the board sandbox profile. `agents.policy` is seed /
        // YAML-fallback only: embedded default, inline YAML, or an optional path.
        let p = self.policy.trim();
        if p.is_empty()
            || p == "embedded"
            || p == "sandbox/policy.yaml"
            || crate::model::is_inline_policy_yaml(p)
        {
            return Ok(());
        }
        if !std::path::Path::new(p).exists() {
            return Err(format!("execution.agents.policy {:?} does not exist", self.policy));
        }
        Ok(())
    }
}

/// Top-level `board:` stanza — persistence and related control-plane settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardConfig {
    #[serde(default)]
    pub database: BoardDatabaseConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub levels: Vec<Level>,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default)]
    pub board: BoardConfig,
}

impl Schema {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())?;
        let schema: Schema = serde_yaml::from_str(&raw)?;
        schema.validate();
        Ok(schema)
    }



    /// Depth 0 → Project, depth ≥1 → Task (flat under Project). Extra depth
    /// collapses to Task so a mistaken nest still labels correctly.
    pub fn level_for_depth(&self, depth: usize) -> Option<&Level> {
        if self.levels.is_empty() {
            return None;
        }
        if depth == 0 {
            self.levels.first()
        } else {
            self.levels.iter().find(|l| l.claimable).or_else(|| self.levels.last())
        }
    }

    pub fn project_level(&self) -> Option<&Level> {
        self.levels.iter().find(|l| !l.claimable).or_else(|| self.levels.first())
    }

    pub fn task_level(&self) -> Option<&Level> {
        self.levels.iter().find(|l| l.claimable).or_else(|| self.levels.last())
    }

    /// A schema that declares a rung a human can't occupy should say so at
    /// configuration time, not at 2am. We can't know child counts here, so this
    /// only catches the structural mistakes.
    fn validate(&self) {
        if self.levels.is_empty() {
            tracing::warn!("level schema declares no levels; the tree will render unlabelled");
        }
        if self.levels.len() > 2 {
            tracing::warn!(
                count = self.levels.len(),
                "more than 2 levels: Plan A is Project + Task only; deeper ladders are retired"
            );
        }
        if !self.levels.iter().any(|l| l.claimable) {
            tracing::warn!("no level is marked claimable; agents will find nothing to pick up");
        }
        if !self.levels.iter().any(|l| !l.claimable) {
            tracing::warn!("no non-claimable Project level; every node would look claimable");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workable() -> AgentConfig {
        AgentConfig {
            enabled: true,
            repo: RepoConfig {
                upstream: "shanemcd/honr".into(),
                fork: "clankrshq/honr".into(),
                base: "main".into(),
            },
            ..Default::default()
        }
    }

    /// Off by default. The board has to run on a machine with no podman, no
    /// gateway and no credentials.
    #[test]
    fn agents_are_off_unless_asked_for() {
        assert!(!AgentConfig::default().enabled);
        assert!(AgentConfig::default().validate().is_ok());
    }

    /// Every one of these presents as a hang if it's wrong at exec time, so
    /// they are checked at startup instead.
    #[test]
    fn enabling_agents_requires_a_complete_config() {
        assert!(workable().validate().is_ok(), "the reference config must pass");

        // Work remotes are resolved per card — empty fork is fine at process start.
        let mut no_fork = workable();
        no_fork.repo.fork = String::new();
        assert!(no_fork.validate().is_ok());

        let mut bad_policy = workable();
        bad_policy.policy = "sandbox/does-not-exist.yaml".into();
        assert!(bad_policy.validate().is_err());
    }

    /// honr.yaml must parse into the config the code expects — the file is the
    /// real contract, not the struct.
    #[test]
    fn shipped_honr_yaml_parses() {
        let s = Schema::load("honr.yaml").expect("honr.yaml parses");
        assert!(!s.levels.is_empty(), "levels should be declared");
        assert!(s.levels.iter().any(|l| l.claimable), "something must be claimable");
        s.execution.agents.validate().expect("shipped agent config is valid");
        let db = s.board.database.parsed().expect("board.database.url parses");
        assert_eq!(db.backend(), crate::db::DatabaseBackend::Sqlite);
    }

    #[test]
    fn card_branch_and_sandbox_names_use_prefix() {
        assert_eq!(card_branch_name("honr", 7), "honr/card-7");
        assert_eq!(card_branch_name("acme", 7), "acme/card-7");
        assert_eq!(card_sandbox_name("honr", 7, 2), "honr-card-7-a2");
        assert_eq!(card_sandbox_name("acme", 7, 1), "acme-card-7-a1");
        assert_eq!(card_branch_name("  /acme/  ", 3), "acme/card-3");
        assert_eq!(card_sandbox_name("acme/widgets", 3, 1), "acme-widgets-card-3-a1");
        assert_eq!(card_branch_name("", 1), "honr/card-1");
        assert_eq!(card_sandbox_stem("honr", 9), "honr-card-9-");
        assert_eq!(cockpit_sandbox_name("honr"), "honr-cockpit");
        assert_eq!(cockpit_sandbox_name("acme/widgets"), "acme-widgets-cockpit");
    }

    #[test]
    fn board_database_accepts_postgres_url_in_yaml() {
        let raw = r#"
levels:
  - name: Project
    claimable: false
  - name: Task
    claimable: true
board:
  database:
    url: postgres://honr:honr@127.0.0.1:5432/honr
"#;
        let s: Schema = serde_yaml::from_str(raw).expect("yaml");
        let db = s.board.database.parsed().expect("postgres url");
        assert_eq!(db.backend(), crate::db::DatabaseBackend::Postgres);
    }

    #[test]
    fn parse_owner_name_accepts_github_style() {
        assert_eq!(parse_owner_name(" shanemcd/honr ").unwrap(), "shanemcd/honr");
        assert_eq!(parse_owner_name("acme/widgets.git").unwrap(), "acme/widgets");
        assert!(parse_owner_name("").is_err());
        assert!(parse_owner_name("noslash").is_err());
        assert!(parse_owner_name("https://github.com/a/b").is_err());
        assert!(parse_owner_name("a/b/c").is_err());
    }

    #[test]
    fn clone_repo_from_prose_reads_stamped_line() {
        let text = "Rework settings.\n\nClone repository: shanemcd/honr into /sandbox/repo for planning and as the default Task clone target.\n";
        assert_eq!(
            clone_repo_from_prose(text).as_deref(),
            Some("shanemcd/honr")
        );
        assert!(clone_repo_from_prose("no stamp here").is_none());
    }
}
