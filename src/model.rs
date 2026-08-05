//! One node type.
//!
//! Project and Task are the only levels: Project is a container; Tasks are
//! flat siblings under it, linked by dependency edges. Only two facts about a
//! node are structural: whether it has children (container vs claimable leaf)
//! and where it sits relative to the commitment line. Everything else — the
//! badge, the colour, which gates apply — comes from the level schema.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type ItemId = u64;

/// The lifecycle contract. The UI renders it; the agent API mutates it. Same
/// object — see `machine.rs` for the legal edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Draft,
    Shaping,
    /// Claimable pool — operator must explicitly dispatch; nothing auto-starts.
    ///
    /// `alias = "ready"` loads legacy boards/history that used Ready.
    #[serde(alias = "ready")]
    Backlog,
    Claimed,
    Running,
    Splitting,
    NeedsHuman,
    /// Human review of the PR. Mechanical checks belong in CI, not a board column.
    ///
    /// `alias = "verifying"` loads legacy history/boards that used the removed
    /// Verifying state (honr never ran real gates there).
    #[serde(alias = "verifying")]
    Review,
    Done,
    /// Cut scope. Retired, not deleted — "we chose not to" is a fact you will
    /// need later.
    Retired,
}

impl State {
    /// Which board column this state renders in. Several states collapse into
    /// one column because the question you're asking of them is the same.
    pub fn column(self) -> Column {
        match self {
            State::Draft => Column::Intake,
            State::Shaping => Column::Shaping,
            State::Backlog => Column::Backlog,
            State::Claimed | State::Running | State::Splitting => Column::Running,
            State::NeedsHuman => Column::NeedsYou,
            State::Review => Column::Review,
            State::Done => Column::Done,
            State::Retired => Column::Retired,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, State::Done | State::Retired)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Intake,
    Shaping,
    /// Formerly `ready` — serde alias keeps old snapshots loading.
    #[serde(alias = "ready")]
    Backlog,
    Running,
    NeedsYou,
    Review,
    Done,
    Retired,
}

/// Provenance — "why does this exist?" must be instantly answerable, so the
/// tree stays honest about what a person actually asked for versus what the
/// system decided on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Human,
    Planner,
    /// Machine-born: an agent discovered the work was bigger than its card.
    Split { from: ItemId },
    Reflection,
}


/// Who holds the card while a run is in flight. The hard stop is
/// [`WorkItem::run_deadline_at`] (agent timeout), not lease renewal.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Lease {
    pub agent_id: String,
    pub granted_at: DateTime<Utc>,
    /// Retained for older clients; not used for liveness or sweep.
    #[serde(default)]
    pub last_heartbeat: DateTime<Utc>,
    /// Mirrors `run_deadline_at` at claim time; not extended by heartbeats.
    pub expires_at: DateTime<Utc>,
}

impl Lease {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now > self.expires_at
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EscalationOption {
    pub label: String,
    pub detail: String,
}

/// An open-ended "what should I do?" transfers the whole problem back to the
/// human. Forcing concrete options with a recommendation turns a five-minute
/// think into a one-tap decision.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Escalation {
    pub question: String,
    pub options: Vec<EscalationOption>,
    pub recommended: usize,
    pub blocked_since: DateTime<Utc>,
    #[serde(default)]
    pub answer: Option<String>,
}

impl Escalation {
    pub fn blocked_secs(&self, now: DateTime<Utc>) -> i64 {
        (now - self.blocked_since).num_seconds().max(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pending,
    Running,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GateRun {
    pub name: String,
    pub status: GateStatus,
    #[serde(default)]
    pub detail: Option<String>,
}

/// A Steer note: injected into a running agent's next turn. Free — no restart,
/// no context loss.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Note {
    pub at: DateTime<Utc>,
    pub author: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Transition {
    pub at: DateTime<Utc>,
    pub from: State,
    pub to: State,
    pub by: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Summary of a resolved blocker item (id, title, state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BlockerSummary {
    pub id: ItemId,
    pub title: String,
    pub state: State,
}

/// Lifecycle of a Plan artifact attached to a Project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Empty shell — waiting for a Plan Task (or propose_breakdown) to fill it.
    Empty,
    /// Proposed revision awaiting human Approve Plan.
    AwaitingApproval,
    /// Last revision has been materialized; Tasks are on the Board.
    Approved,
}

/// One proposed Task inside a Plan artifact. Keys are stable within the plan
/// so deps and replans can refer to work before board ids exist.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanTaskSpec {
    pub key: String,
    pub title: String,
    pub intent: String,
    pub definition_of_done: String,
    #[serde(default)]
    pub blocked_by_keys: Vec<String>,
    #[serde(default)]
    pub capability: Option<String>,
    /// Optional wire field; materialize uses intent/DoD for clone targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<crate::schema::RepoConfig>,
    /// Set when Approve Plan materializes (or updates) a board Task.
    #[serde(default)]
    pub item_id: Option<ItemId>,
}

/// Legacy Project-level plan blob (ignored for new boards). Live plans are
/// `TaskProposal` on the Initial plan card.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanArtifact {
    pub revision: u32,
    #[serde(default)]
    pub summary: String,
    pub status: PlanStatus,
    #[serde(default)]
    pub tasks: Vec<PlanTaskSpec>,
    /// Keys to retire on the next approve (replan cancels).
    #[serde(default)]
    pub cancel_keys: Vec<String>,
    /// Board ids resolved from `cancel_keys` at propose time (keys drop out of `tasks`).
    #[serde(default)]
    pub cancel_item_ids: Vec<ItemId>,
    #[serde(default)]
    pub approved_revision: Option<u32>,
}

impl PlanArtifact {
    #[allow(dead_code)] // kept for legacy board JSON / future replan tooling
    pub fn empty() -> Self {
        Self {
            revision: 0,
            summary: String::new(),
            status: PlanStatus::Empty,
            tasks: Vec::new(),
            cancel_keys: Vec::new(),
            cancel_item_ids: Vec::new(),
            approved_revision: None,
        }
    }

    /// Compact status for Home / GoalView: `no_plan`, `awaiting_approval`, `approved_vN`.
    #[allow(dead_code)] // GoalView now derives status from Initial plan proposal
    pub fn status_label(&self) -> String {
        match self.status {
            PlanStatus::Empty => "no_plan".into(),
            PlanStatus::AwaitingApproval => "awaiting_approval".into(),
            PlanStatus::Approved => {
                format!("approved_v{}", self.approved_revision.unwrap_or(self.revision))
            }
        }
    }
}

/// Legacy exact title (pre–project-name seed). Still recognized by
/// [`title_is_initial_plan`].
pub const INITIAL_PLAN_TITLE_LEGACY: &str = "Initial plan";

/// Prefix for seed Task titles: `Initial Plan for <Project name>`.
pub const INITIAL_PLAN_TITLE_PREFIX: &str = "Initial Plan for ";

/// Title for a Project's Initial plan seed Task.
pub fn initial_plan_title(project_title: &str) -> String {
    format!("{INITIAL_PLAN_TITLE_PREFIX}{project_title}")
}

/// Whether a card title identifies an Initial plan Task.
pub fn title_is_initial_plan(title: &str) -> bool {
    title == INITIAL_PLAN_TITLE_LEGACY || title.starts_with(INITIAL_PLAN_TITLE_PREFIX)
}

#[cfg(test)]
mod initial_plan_title_tests {
    use super::*;

    #[test]
    fn title_includes_project_name() {
        assert_eq!(
            initial_plan_title("Webhook rebase"),
            "Initial Plan for Webhook rebase"
        );
        assert!(title_is_initial_plan("Initial Plan for Webhook rebase"));
        assert!(title_is_initial_plan(INITIAL_PLAN_TITLE_LEGACY));
        assert!(!title_is_initial_plan("Implement webhook handler"));
    }
}

/// Default standing instructions seeded on every new Project (`project_prompt`).
///
/// Clone targets are named in each Task's intent/DoD. After report, card
/// `pull_request` drives resume remotes. Keep quality gates here.
pub const DEFAULT_PROJECT_PROMPT: &str = "\
Merging is a human action — approving in honr surfaces the PR; it never merges.\n\
Do not weaken machine.rs invariants, supervisor budget enforcement, or the board sandbox-profile policy; escalate instead.\n\
Sandbox stack failures present as hangs — treat silence as failure and escalate rather than looping.\n\
Name the repository to clone in each Task's intent and/or definition of done \
(`owner/name`, and push remote when it differs). Do not invent an owner/name from context; \
if the card text is silent or ambiguous, escalate.\n\
Name this Project's quality gates (test/lint commands) here when agents should run them before \
publish — do not assume cargo or any other toolchain unless named.\n\
Initial plan: write /sandbox/.honr/plan.json; each proposed task names its \
clone target in intent/DoD; human Approve creates Tasks.\n\
If impl work is bigger than one card, write /sandbox/.honr/split.json (same task shape; name \
clone targets in each child's intent/DoD); card goes to Review — Approve creates siblings. \
Never nest under a Task.\n\
";

#[cfg(test)]
mod default_project_prompt_tests {
    use super::DEFAULT_PROJECT_PROMPT;

    #[test]
    fn clone_targets_are_named_in_task_prose() {
        let p = DEFAULT_PROJECT_PROMPT;
        assert!(
            p.contains("intent") && p.contains("definition of done"),
            "must point at task text for clone targets: {p}"
        );
        assert!(
            p.contains("plan.json") && p.contains("Approve creates Tasks"),
            "Initial plan must use plan.json then Approve: {p}"
        );
        assert!(
            p.contains("Name the repository to clone"),
            "must instruct naming the clone target: {p}"
        );
    }
}

/// One end of a pull request (GitHub `base` / `head`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct PullRequestEnd {
    /// `owner/name` (`full_name`).
    pub repo: String,
    /// Branch name (GitHub JSON field `ref`).
    #[serde(rename = "ref")]
    pub git_ref: String,
}

impl PullRequestEnd {
    pub fn new(repo: impl Into<String>, git_ref: impl Into<String>) -> Self {
        Self {
            repo: repo.into().trim().to_string(),
            git_ref: {
                let r = git_ref.into().trim().to_string();
                if r.is_empty() {
                    "main".into()
                } else {
                    r
                }
            },
        }
    }

    pub fn is_usable(&self) -> bool {
        !self.repo.trim().is_empty() && !self.git_ref.trim().is_empty()
    }
}

/// Pull request on a card — forge facts for resume/clone/rebase.
/// Shape matches `report.json` / GitHub base&head naming. URL lives here, not
/// as a top-level `pr_url` field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct PullRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<PullRequestEnd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<PullRequestEnd>,
}

impl PullRequest {
    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim().to_string(),
            base: None,
            head: None,
        }
    }

    pub fn url_str(&self) -> Option<&str> {
        let u = self.url.trim();
        if u.is_empty() {
            None
        } else {
            Some(u)
        }
    }

    /// Base+head present — enough to clone without inventing a fork.
    pub fn has_forge_ends(&self) -> bool {
        self.base.as_ref().is_some_and(PullRequestEnd::is_usable)
            && self.head.as_ref().is_some_and(PullRequestEnd::is_usable)
    }

    pub fn to_repo_config(&self) -> Option<crate::schema::RepoConfig> {
        let base = self.base.as_ref().filter(|b| b.is_usable())?;
        let head = self
            .head
            .as_ref()
            .filter(|h| h.is_usable())
            .unwrap_or(base);
        Some(
            crate::schema::RepoConfig {
                upstream: base.repo.clone(),
                fork: head.repo.clone(),
                base: base.git_ref.clone(),
            }
            .normalized(),
        )
    }

}

/// One Task row as shown to an agent from the Project Plan.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlanTaskBrief {
    pub key: String,
    pub title: String,
    pub intent: String,
    pub definition_of_done: String,
    #[serde(default)]
    pub blocked_by_keys: Vec<String>,
    /// True when this row is the card being claimed.
    #[serde(default)]
    pub current: bool,
}

/// Desired OpenShell provider (Settings → OpenShell → Providers).
///
/// Honr is source of truth; Sync/Apply pushes to the gateway via gRPC.
/// Credential values are sealed — GET APIs expose keys only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenShellProviderDesired {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    /// Sealed JSON object of credential key → value (never returned on GET).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_sealed: Option<String>,
    /// Plain keys present in the sealed credentials blob (safe to return).
    #[serde(default)]
    pub credential_keys: Vec<String>,
    /// Optional gateway-owned refresh bootstrap (e.g. gcloud ADC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<OpenShellProviderRefreshDesired>,
    /// When true, sandbox create attaches this provider by name.
    #[serde(default = "default_true")]
    pub attach_to_sandboxes: bool,
}

fn default_true() -> bool {
    true
}

/// Refresh material for [`OpenShellProviderDesired`] (Vertex ADC, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenShellProviderRefreshDesired {
    /// Provider credential env key that refresh writes into (e.g. `GOOGLE_VERTEX_AI_TOKEN`).
    pub credential_key: String,
    /// Proto strategy name: `oauth2_refresh_token`, `google_service_account_jwt`, …
    pub strategy: String,
    /// Sealed JSON object of refresh material key → value.
    pub material_sealed: String,
    /// Material keys treated as secret by the gateway.
    #[serde(default)]
    pub secret_material_keys: Vec<String>,
}

impl OpenShellProviderDesired {
    pub fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.provider_type = self.provider_type.trim().to_string();
        self.config = self
            .config
            .into_iter()
            .map(|(k, v)| (k.trim().to_string(), v))
            .filter(|(k, _)| !k.is_empty())
            .collect();
        self.credential_keys = self
            .credential_keys
            .into_iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        self.credential_keys.sort();
        self.credential_keys.dedup();
        if let Some(ref mut r) = self.refresh {
            r.credential_key = r.credential_key.trim().to_string();
            r.strategy = r.strategy.trim().to_string();
            r.secret_material_keys = r
                .secret_material_keys
                .iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect();
        }
        self
    }

    pub fn has_credentials(&self) -> bool {
        self.credentials_sealed
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
            || self.refresh.is_some()
    }
}

/// Per-install agent process knobs (Settings → Agent runtime).
///
/// Seeded from `honr.yaml` `execution.agents`; Board is source of truth after.
/// Image / policy / cpu / memory stay on sandbox profiles; work remotes stay
/// on card `pull_request` / yaml repo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRuntimeConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Primary agent CLI (`cursor`, `agy`, or `claude`).
    #[serde(default = "default_runtime_engine")]
    pub engine: String,
    #[serde(default = "default_runtime_concurrent")]
    pub max_concurrent: usize,
    #[serde(default = "default_runtime_timeout")]
    pub agent_timeout_secs: u64,
    #[serde(default = "default_runtime_attempts")]
    pub max_attempts: u32,
    /// Branch / sandbox name stem (default `honr` → `honr/card-N`).
    #[serde(default = "default_runtime_branch_prefix")]
    pub branch_prefix: String,
}

fn default_runtime_engine() -> String {
    "cursor".into()
}
fn default_runtime_concurrent() -> usize {
    2
}
fn default_runtime_timeout() -> u64 {
    1800
}
fn default_runtime_attempts() -> u32 {
    3
}
fn default_runtime_branch_prefix() -> String {
    "honr".into()
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            engine: default_runtime_engine(),
            max_concurrent: default_runtime_concurrent(),
            agent_timeout_secs: default_runtime_timeout(),
            max_attempts: default_runtime_attempts(),
            branch_prefix: default_runtime_branch_prefix(),
        }
    }
}

impl AgentRuntimeConfig {
    /// Trim string fields; normalize branch prefix and counters.
    pub fn normalized(mut self) -> Self {
        self.engine = self.engine.trim().to_string();
        if self.engine.is_empty() {
            self.engine = default_runtime_engine();
        }
        self.branch_prefix = crate::schema::normalize_branch_prefix(&self.branch_prefix);
        if self.max_concurrent == 0 {
            self.max_concurrent = 1;
        }
        if self.agent_timeout_secs == 0 {
            self.agent_timeout_secs = default_runtime_timeout();
        }
        if self.max_attempts == 0 {
            self.max_attempts = default_runtime_attempts();
        }
        self
    }
}

/// Settings → Forge: poll GitHub when webhooks are missing or delayed.
///
/// When enabled, honr polls on `interval_secs` **in addition to** webhooks.
/// Both paths call the same Board methods (merge → Done, tip → MainAdvanced).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookPollConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Seconds between ticks. Clamped to ≥ [`MIN_WEBHOOK_POLL_INTERVAL_SECS`].
    #[serde(default = "default_webhook_poll_interval_secs")]
    pub interval_secs: u64,
}

/// Floor for poll interval (Settings + loop). Below this, GitHub rate limits hurt.
pub const MIN_WEBHOOK_POLL_INTERVAL_SECS: u64 = 15;

fn default_webhook_poll_interval_secs() -> u64 {
    60
}

impl Default for WebhookPollConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_webhook_poll_interval_secs(),
        }
    }
}

impl WebhookPollConfig {
    /// Clamp interval; leave `enabled` as given.
    pub fn normalized(mut self) -> Self {
        if self.interval_secs < MIN_WEBHOOK_POLL_INTERVAL_SECS {
            self.interval_secs = MIN_WEBHOOK_POLL_INTERVAL_SECS;
        }
        self
    }
}

/// Per-install forge identity (Settings → Forge).
/// Work remotes live on each card's [`PullRequest`] after the agent reports.
/// See `docs/architecture.md`.
///
/// Legacy wire keys (`beads_sync_repo`, `upstream`, `branching_prompt`) are
/// ignored on deserialize so old Settings payloads still load.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBinding {
    /// Forge provider. Only `github` is implemented; `gitlab` is a future seam.
    #[serde(default = "default_forge")]
    pub forge: String,
}

fn default_forge() -> String {
    "github".into()
}

impl Default for WorkspaceBinding {
    fn default() -> Self {
        Self {
            forge: default_forge(),
        }
    }
}

/// Hold for the durable control-plane cockpit. Distinct from card `parked`:
/// this is not claim/heartbeat/report lifecycle — it is the Board record that
/// lets chat/TTY reconnect keep the same sandbox + conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CockpitSessionStatus {
    /// Cockpit agent may be live in the sandbox.
    #[default]
    Running,
    /// Park-like hold: sandbox + conversation kept; agent stopped until resume.
    Parked,
}

/// Durable cockpit-session singleton on the Board. Chat and TTY are faces over this
/// record — they must not grow a second lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CockpitSession {
    /// OpenShell sandbox environment name (e.g. `honr-cockpit`).
    #[serde(default)]
    pub environment: Option<String>,
    /// agy conversation id for reconnect (`--conversation`).
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub status: CockpitSessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CockpitSession {
    pub fn new(environment: Option<String>, conversation_id: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            environment: normalize_cockpit_field(environment),
            conversation_id: normalize_cockpit_field(conversation_id),
            status: CockpitSessionStatus::Running,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Trim; empty → `None`.
pub fn normalize_cockpit_field(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Named create-spec for OpenShell sandboxes. Board-state catalog entries;
/// yaml `execution.agents` seeds the catalog when empty at boot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxProfile {
    pub id: String,
    pub name: String,
    /// Passed to `openshell sandbox create --from`.
    pub image: String,
    /// Inline OpenShell policy YAML text.
    /// The supervisor writes a temp file from this at sandbox create.
    pub policy: String,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    /// Agent CLI for cards using this profile (`cursor`, `agy`, `claude`).
    /// When unset, claim/run falls back to Settings → Agent runtime engine.
    #[serde(default)]
    pub engine: Option<String>,
}

/// Catalog id for the privileged cockpit control-plane seat (not the card worker).
pub const COCKPIT_SANDBOX_PROFILE_ID: &str = "cockpit";

/// Host-path seed source for the cockpit profile policy (inline after seed).
pub const COCKPIT_SANDBOX_POLICY_PATH: &str = "sandbox/cockpit-policy.yaml";

/// Lighter create knobs than the worker default — chat seat, not a build box.
pub const COCKPIT_SANDBOX_CPU: &str = "1";
pub const COCKPIT_SANDBOX_MEMORY: &str = "2Gi";

/// Build the seedable `cockpit` catalog entry from AgentConfig image/engine and
/// [`COCKPIT_SANDBOX_POLICY_PATH`]. Cpu/memory stay distinct from the worker default.
pub fn cockpit_sandbox_profile_from_agents(agents: &crate::schema::AgentConfig) -> SandboxProfile {
    let engine = {
        let e = agents.engine.trim();
        if e.is_empty() {
            None
        } else {
            Some(e.to_string())
        }
    };
    SandboxProfile {
        id: COCKPIT_SANDBOX_PROFILE_ID.into(),
        name: "Cockpit".into(),
        image: agents.image.clone(),
        policy: resolve_policy_yaml(COCKPIT_SANDBOX_POLICY_PATH),
        cpu: Some(COCKPIT_SANDBOX_CPU.into()),
        memory: Some(COCKPIT_SANDBOX_MEMORY.into()),
        engine,
    }
}

/// Stable id slug from a display name. Lowercase ASCII alphanumerics; runs of
/// whitespace/`_`/`-` become a single hyphen. Empty/punctuation-only names
/// fall back to `profile` so create never invents a blank key.
pub fn slugify_sandbox_profile_id(name: &str) -> String {
    let mut out = String::new();
    let mut pending_hyphen = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_hyphen && !out.is_empty() {
                out.push('-');
            }
            pending_hyphen = false;
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            pending_hyphen = true;
        }
        // other punctuation is dropped
    }
    if out.is_empty() {
        "profile".into()
    } else {
        out
    }
}

/// Create knobs after Project override → global default → YAML resolution.
/// `policy` is always YAML **content** ready to materialize as a temp file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSandboxCreate {
    pub image: String,
    pub policy: String,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    /// Profile engine when set; YAML fallback carries `agents.engine`.
    pub engine: Option<String>,
    /// Catalog profile that won, if any. `None` means YAML fallback.
    pub profile_id: Option<String>,
}

impl ResolvedSandboxCreate {
    pub fn from_profile(p: &SandboxProfile) -> Self {
        Self {
            image: p.image.clone(),
            policy: p.policy.clone(),
            cpu: p.cpu.clone(),
            memory: p.memory.clone(),
            engine: p
                .engine
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            profile_id: Some(p.id.clone()),
        }
    }

    pub fn from_agents(agents: &crate::schema::AgentConfig) -> Self {
        let engine = {
            let e = agents.engine.trim();
            if e.is_empty() {
                None
            } else {
                Some(e.to_string())
            }
        };
        Self {
            image: agents.image.clone(),
            // honr.yaml still names a path for bootstrap; profiles store content.
            policy: resolve_policy_yaml(&agents.policy),
            cpu: agents.cpu.clone(),
            memory: agents.memory.clone(),
            engine,
            profile_id: None,
        }
    }
}

/// Heuristic: already-inline YAML vs a short host path from older boards / YAML.
pub fn is_inline_policy_yaml(s: &str) -> bool {
    let t = s.trim();
    t.contains('\n') || t.starts_with('#') || t.starts_with("version:")
}

/// Turn `execution.agents.policy` (path or already-inline YAML) into content.
///
/// Live worker policy is the board sandbox profile. This resolves seed /
/// YAML-fallback text only: inline YAML, the built-in worker default
/// (`embedded` / legacy `sandbox/policy.yaml` / empty), or an optional path.
pub fn resolve_policy_yaml(path_or_yaml: &str) -> String {
    if is_inline_policy_yaml(path_or_yaml) {
        return path_or_yaml.to_string();
    }
    let t = path_or_yaml.trim();
    if t.is_empty() || t == "embedded" || t == "sandbox/policy.yaml" {
        return crate::seed_policies::DEFAULT_WORKER_SANDBOX_POLICY.to_string();
    }
    match std::fs::read_to_string(t) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => crate::seed_policies::DEFAULT_WORKER_SANDBOX_POLICY.to_string(),
    }
}

/// If a stored profile still holds a host path (pre–inline-policy boards),
/// replace it with file contents when the path is readable.
pub fn migrate_profile_policy_to_inline(policy: &str) -> Option<String> {
    if is_inline_policy_yaml(policy) {
        return None;
    }
    match std::fs::read_to_string(policy) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

/// Proposed sibling Tasks awaiting human Approve on a card (Initial plan or split).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskProposal {
    #[serde(default)]
    pub summary: String,
    pub tasks: Vec<PlanTaskSpec>,
}

/// Child spec for `Board::propose_split` / `split.json` (deps match PlanTaskSpec).
#[derive(Debug, Clone)]
pub struct SplitChildSpec {
    pub title: String,
    pub intent: String,
    pub definition_of_done: String,
    pub key: Option<String>,
    pub blocked_by_keys: Vec<String>,
    /// Optional per-child remotes; Approve defaults from the splitting parent Task.
    pub repo: Option<crate::schema::RepoConfig>,
}

impl SplitChildSpec {
    pub fn new(
        title: impl Into<String>,
        intent: impl Into<String>,
        definition_of_done: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            intent: intent.into(),
            definition_of_done: definition_of_done.into(),
            key: None,
            blocked_by_keys: Vec::new(),
            repo: None,
        }
    }

    #[must_use]
    #[allow(dead_code)] // used from unit tests; production builds via SplitChildSpec fields
    pub fn with_repo(mut self, repo: crate::schema::RepoConfig) -> Self {
        self.repo = Some(repo);
        self
    }

    #[must_use]
    #[allow(dead_code)] // used from unit tests; production builds via SplitChildSpec fields
    pub fn with_deps(
        mut self,
        key: impl Into<String>,
        blocked_by_keys: Vec<String>,
    ) -> Self {
        self.key = Some(key.into());
        self.blocked_by_keys = blocked_by_keys;
        self
    }
}

impl From<(String, String, String)> for SplitChildSpec {
    fn from((title, intent, definition_of_done): (String, String, String)) -> Self {
        Self::new(title, intent, definition_of_done)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkItem {
    pub id: ItemId,
    #[serde(default)]
    pub parent: Option<ItemId>,
    /// Label from the level schema. `None` for machine-created depth below the
    /// commitment line — it collapses into its nearest schema rung for display.
    #[serde(default)]
    pub level: Option<String>,

    /// Short and distinct. You cannot chunk what you cannot name.
    pub title: String,
    /// One sentence of intent. This chain is the highest-leverage payload in
    /// the system.
    pub intent: String,
    /// Every leaf must have one, mechanically checkable. Without it the tree is
    /// a wish list; with it, everything below the line is executable by
    /// construction.
    #[serde(default)]
    pub definition_of_done: Option<String>,

    pub state: State,
    pub origin: Origin,
    /// Above the line: human-approved, stable. Below: agents create, split and
    /// retire freely.
    #[serde(default)]
    pub above_line: bool,

    #[serde(default)]
    pub blocked_by: Vec<ItemId>,
    #[serde(default)]
    pub blockers: Vec<BlockerSummary>,
    #[serde(default)]
    pub capability: Option<String>,

    #[serde(default)]
    pub lease: Option<Lease>,
    /// Hard end of this run (`claim` + `agent_timeout_secs`). Not renewed.
    /// Sweeper requeues when past; UI shows countdown to this instant.
    #[serde(default)]
    pub run_deadline_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub progress: f32,

    #[serde(default)]
    pub escalation: Option<Escalation>,
    #[serde(default)]
    pub gates: Vec<GateRun>,
    #[serde(default)]
    pub gate_failures: u32,
    /// Runs that died before producing anything — sandbox wouldn't start, clone
    /// failed, agent overran. Distinct from `gate_failures`, which means the
    /// work arrived and was judged wrong. Both have a retry budget; this one
    /// exists because early failures otherwise requeue forever with no signal.
    #[serde(default)]
    pub run_failures: u32,
    #[serde(default)]
    pub diff_added: u32,
    #[serde(default)]
    pub diff_removed: u32,

    #[serde(default)]
    pub notes: Vec<Note>,

    /// Standing agent instructions for this Project (Tasks inherit via claim).
    /// Null on Tasks. Seeded from [`DEFAULT_PROJECT_PROMPT`] on Project create.
    #[serde(default)]
    pub project_prompt: Option<String>,

    /// Optional sandbox profile override for this Project. Null / unset means
    /// inherit [`crate::store::BoardState::default_sandbox_profile_id`].
    /// Null on Tasks.
    #[serde(default)]
    pub sandbox_profile_id: Option<String>,

    /// When true on a Project, the supervisor continuously queues claimable
    /// Backlog leaves under it (`awaiting_dispatch`). Tasks ignore this field.
    /// Default off — Backlog stays inert until Start/dispatch.
    #[serde(default)]
    pub auto_dispatch: bool,

    /// The bounce reason if this card was returned to Backlog due to an infra or execution bounce.
    #[serde(default)]
    pub last_bounce_reason: Option<String>,
    /// Conflicting file paths from the last rebase conflict.
    #[serde(default)]
    pub last_conflict_files: Vec<String>,

    /// The tree says *why*; the release target says *which shipped artifact*.
    /// These vary independently.
    #[serde(default)]
    pub release_target: Option<String>,
    /// The sandbox this card ran in, e.g. `honr-card-7`. Set by the supervisor
    /// at creation, and the key that lets a restarted honr find live sandboxes
    /// again instead of orphaning them.
    #[serde(default)]
    pub environment: Option<String>,
    /// agy conversation id for the current sandbox session. Park keeps it so
    /// the next claim can `--conversation` resume; halt clears it.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Set by park: card is Backlog but must not be claimed until unpark.
    /// Unpark clears this and queues dispatch.
    #[serde(default)]
    pub parked: bool,
    /// Supervisor should claim this Backlog card. Set by Start / dispatch, or
    /// by unpark (resume). Cleared on claim, bounce to Backlog, or cancel.
    #[serde(default)]
    pub awaiting_dispatch: bool,
    /// Indicates that main advanced and this card's PR branch needs a rebase.
    #[serde(default)]
    pub rebase_requested: bool,
    /// Pull request the agent opened (url + base/head). Approving surfaces it;
    /// merging stays a human action. Legacy top-level `pr_url` migrates here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<PullRequest>,
    /// Legacy wire field — read on load, never written.
    #[serde(default, rename = "pr_url", skip_serializing)]
    pub legacy_pr_url: Option<String>,

    /// Durable product remotes for a claimable Task (`upstream` required;
    /// optional `fork`; `base` defaults to `main`). Null on Projects — remotes
    /// are task-scoped, never a Project `product_repo`. After report,
    /// [`Self::pull_request`] still wins for resume (see `resolve_card_repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<crate::schema::RepoConfig>,

    /// Plan artifact — Projects only (Phase 1). Source of truth for Approve Plan.
    #[serde(default)]
    pub plan: Option<PlanArtifact>,

    /// Proposed sibling Tasks on this card (Initial plan or impl split). Approve
    /// materializes them; request_changes clears. Null when none.
    #[serde(default)]
    pub proposal: Option<TaskProposal>,

    pub created_at: DateTime<Utc>,
    pub entered_state_at: DateTime<Utc>,
    #[serde(default)]
    pub history: Vec<Transition>,
}

impl WorkItem {
    pub fn new(id: ItemId, title: impl Into<String>, intent: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id,
            parent: None,
            level: None,
            title: title.into(),
            intent: intent.into(),
            definition_of_done: None,
            state: State::Draft,
            origin: Origin::Human,
            above_line: false,
            blocked_by: Vec::new(),
            blockers: Vec::new(),
            capability: None,
            lease: None,
            run_deadline_at: None,
            engine: None,
            model: None,
            progress: 0.0,
            escalation: None,
            gates: Vec::new(),
            gate_failures: 0,
            run_failures: 0,
            diff_added: 0,
            diff_removed: 0,
            notes: Vec::new(),
            project_prompt: None,
            sandbox_profile_id: None,
            auto_dispatch: false,
            last_bounce_reason: None,
            last_conflict_files: Vec::new(),
            release_target: None,
            environment: None,
            conversation_id: None,
            parked: false,
            awaiting_dispatch: false,
            rebase_requested: false,
            pull_request: None,
            legacy_pr_url: None,
            repo: None,
            plan: None,
            proposal: None,
            created_at: now,
            entered_state_at: now,
            history: Vec::new(),
        }
    }

    pub fn is_project(&self) -> bool {
        self.parent.is_none() && self.level.as_deref() != Some("Task")
    }

    /// PR HTML URL, if any (`pull_request.url`).
    pub fn pr_url(&self) -> Option<&str> {
        self.pull_request.as_ref().and_then(PullRequest::url_str)
    }

    /// Fold legacy top-level `pr_url` into [`Self::pull_request`].
    pub fn migrate_legacy_pr_url(&mut self) {
        let Some(url) = self.legacy_pr_url.take() else {
            return;
        };
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        match &mut self.pull_request {
            Some(pr) if pr.url.trim().is_empty() => pr.url = url,
            None => self.pull_request = Some(PullRequest::from_url(url)),
            Some(_) => {}
        }
    }

    pub fn is_initial_plan_task(&self) -> bool {
        title_is_initial_plan(&self.title)
            || self
                .definition_of_done
                .as_deref()
                .is_some_and(|d| d.contains("Plan artifact approved"))
    }

    pub fn time_in_state(&self, now: DateTime<Utc>) -> Duration {
        now - self.entered_state_at
    }
}

/// Human-readable elapsed time. `4s`, `12m`, `3h 5m`.
pub fn humanize(d: Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_ready_wire_value_loads_as_backlog() {
        let json = r#"{"id":1,"title":"t","intent":"i","state":"ready","origin":{"kind":"human"},"created_at":"2026-01-01T00:00:00Z","entered_state_at":"2026-01-01T00:00:00Z"}"#;
        let item: WorkItem = serde_json::from_str(json).expect("deserialize");
        assert_eq!(item.state, State::Backlog);
        assert_eq!(item.state.column(), Column::Backlog);
        assert!(!item.awaiting_dispatch);
    }

    #[test]
    fn slugify_sandbox_profile_id_from_display_name() {
        assert_eq!(slugify_sandbox_profile_id("Heavy CI"), "heavy-ci");
        assert_eq!(slugify_sandbox_profile_id("  Default  "), "default");
        assert_eq!(slugify_sandbox_profile_id("Foo_Bar--Baz"), "foo-bar-baz");
        assert_eq!(slugify_sandbox_profile_id("!!!"), "profile");
        assert_eq!(slugify_sandbox_profile_id(""), "profile");
        assert_eq!(slugify_sandbox_profile_id("A"), "a");
    }

    #[test]
    fn cockpit_sandbox_policy_file_is_distinct_from_worker() {
        let cockpit_pol = std::fs::read_to_string(COCKPIT_SANDBOX_POLICY_PATH)
            .unwrap_or_else(|e| panic!("read {COCKPIT_SANDBOX_POLICY_PATH}: {e}"));
        let worker = crate::seed_policies::DEFAULT_WORKER_SANDBOX_POLICY;
        assert!(
            cockpit_pol.contains("honr-mcp") && cockpit_pol.contains("host.docker.internal"),
            "cockpit policy must allow host honr MCP egress"
        );
        assert!(
            cockpit_pol.contains("protocol: mcp")
                && cockpit_pol.contains("allow_all_known_mcp_methods: true"),
            "cockpit host MCP must use OpenShell protocol: mcp (rest+access:full 403s /mcp)"
        );
        assert!(
            !cockpit_pol.contains("name: github") && !cockpit_pol.contains("api.github.com"),
            "cockpit must not copy worker GitHub egress"
        );
        assert!(
            !cockpit_pol.contains("package-registries") && !cockpit_pol.contains("index.crates.io"),
            "cockpit must not copy worker package-registry egress"
        );
        assert!(
            !worker.contains("honr-mcp") && !worker.contains("host.docker.internal"),
            "worker policy stays air-gapped from honr MCP"
        );
        assert!(
            worker.contains("name: github") && worker.contains("api.github.com"),
            "worker keeps GitHub code egress"
        );
        assert!(
            worker.contains("/opt/rust/toolchains/**/bin/cargo"),
            "worker seed allows rustup toolchain cargo for github git deps"
        );
        openshell_policy::parse_sandbox_policy(&cockpit_pol).expect("cockpit-policy.yaml parses");
        openshell_policy::parse_sandbox_policy(worker).expect("embedded worker policy parses");

        let agents = crate::schema::AgentConfig {
            image: "honr-sandbox:latest".into(),
            engine: "cursor".into(),
            cpu: Some("2".into()),
            memory: Some("4Gi".into()),
            ..Default::default()
        };
        let profile = cockpit_sandbox_profile_from_agents(&agents);
        assert_eq!(profile.id, COCKPIT_SANDBOX_PROFILE_ID);
        assert_eq!(profile.cpu.as_deref(), Some(COCKPIT_SANDBOX_CPU));
        assert_eq!(profile.memory.as_deref(), Some(COCKPIT_SANDBOX_MEMORY));
        assert_ne!(profile.cpu, agents.cpu);
        assert_ne!(profile.memory, agents.memory);
        assert_eq!(profile.policy, cockpit_pol);
    }
}
