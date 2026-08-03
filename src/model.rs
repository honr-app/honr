//! One node type.
//!
//! Project and Task are the only levels: Project is a container; Tasks are
//! flat siblings under it, linked by dependency edges. Only two facts about a
//! node are structural: whether it has children (container vs claimable leaf)
//! and where it sits relative to the commitment line. Everything else — the
//! badge, the colour, which gates apply — comes from the level schema.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub type ItemId = u64;

/// The lifecycle contract. The UI renders it; the agent API mutates it. Same
/// object — see `machine.rs` for the legal edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Draft,
    Shaping,
    /// Claimable pool — cockpit must explicitly dispatch; nothing auto-starts.
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
/// Replaces the old pin soup for policy the agent must always see.
pub const DEFAULT_PROJECT_PROMPT: &str = "\
Merging is a human action — approving in honr surfaces the PR; it never merges.\n\
Do not weaken machine.rs invariants, supervisor budget enforcement, or sandbox/policy.yaml; escalate instead.\n\
Sandbox stack failures present as hangs — treat silence as failure and escalate rather than looping.\n\
Finish via /sandbox/.honr/report.json with a real PR (implementation and Initial plan).\n\
Initial plan: also write /sandbox/.honr/plan.json (proposed Tasks); human Approve creates them.\n\
If impl work is bigger than one card, write /sandbox/.honr/split.json (same task shape); card goes to Review — Approve creates siblings. Never nest under a Task.\n\
When a fork is involved: origin is the fork (push); rebase onto upstream/<base>, never origin/<base> alone — the fork's base freezes at create time.\n\
";

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

/// Per-install forge/repo binding. Seeded once from `execution.agents.repo`
/// (and env); Board is source of truth afterward. See `docs/generalization.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBinding {
    /// Forge provider. Only `github` is implemented; `gitlab` is a future seam.
    #[serde(default = "default_forge")]
    pub forge: String,
    /// `owner/name` that PRs target.
    #[serde(default)]
    pub upstream: String,
    /// `owner/name` the agent clones and pushes to.
    #[serde(default)]
    pub fork: String,
    #[serde(default = "default_workspace_base")]
    pub base: String,
    /// Beads ↔ GitHub Issues sync target (`owner/name`). Empty → use `upstream`.
    #[serde(default)]
    pub beads_sync_repo: Option<String>,
}

fn default_forge() -> String {
    "github".into()
}

fn default_workspace_base() -> String {
    "main".into()
}

impl Default for WorkspaceBinding {
    fn default() -> Self {
        Self {
            forge: default_forge(),
            upstream: String::new(),
            fork: String::new(),
            base: default_workspace_base(),
            beads_sync_repo: None,
        }
    }
}

impl WorkspaceBinding {
    /// True when upstream and fork are both non-empty (usable as install default).
    /// Agents do not require this — work remotes resolve per card from `pr_url`
    /// (see `Board::resolve_card_repo`).
    pub fn is_complete(&self) -> bool {
        !self.upstream.trim().is_empty() && !self.fork.trim().is_empty()
    }

    /// Repo used for beads Issue URL construction / `bd github` env.
    pub fn beads_repo(&self) -> Option<String> {
        let explicit = self
            .beads_sync_repo
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(r) = explicit {
            return Some(r.to_string());
        }
        let upstream = self.upstream.trim();
        if upstream.is_empty() {
            None
        } else {
            Some(upstream.to_string())
        }
    }

    pub fn to_repo_config(&self) -> crate::schema::RepoConfig {
        crate::schema::RepoConfig {
            upstream: self.upstream.trim().to_string(),
            fork: self.fork.trim().to_string(),
            base: {
                let b = self.base.trim();
                if b.is_empty() {
                    default_workspace_base()
                } else {
                    b.to_string()
                }
            },
        }
    }

    pub fn from_repo_config(repo: &crate::schema::RepoConfig) -> Self {
        Self {
            forge: default_forge(),
            upstream: repo.upstream.clone(),
            fork: repo.fork.clone(),
            base: if repo.base.trim().is_empty() {
                default_workspace_base()
            } else {
                repo.base.clone()
            },
            beads_sync_repo: None,
        }
    }
}

/// Named create-spec for OpenShell sandboxes. Board-state catalog entries;
/// YAML `execution.agents` image/policy/cpu/memory is seed/fallback only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxProfile {
    pub id: String,
    pub name: String,
    /// Passed to `openshell sandbox create --from`.
    pub image: String,
    /// Inline OpenShell policy YAML text (not a host filesystem path).
    /// The supervisor writes a temp file from this at sandbox create.
    pub policy: String,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
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
            profile_id: Some(p.id.clone()),
        }
    }

    pub fn from_agents(agents: &crate::schema::AgentConfig) -> Self {
        Self {
            image: agents.image.clone(),
            // honr.yaml still names a path for bootstrap; profiles store content.
            policy: resolve_policy_yaml(&agents.policy),
            cpu: agents.cpu.clone(),
            memory: agents.memory.clone(),
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
/// Profiles persist the YAML text. The host path in honr.yaml is seed/fallback
/// only — never the board's source of truth after catalog seed.
pub fn resolve_policy_yaml(path_or_yaml: &str) -> String {
    if is_inline_policy_yaml(path_or_yaml) {
        return path_or_yaml.to_string();
    }
    match std::fs::read_to_string(path_or_yaml) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) => format!("# empty policy file at {path_or_yaml}\nversion: 1\n"),
        Err(_) => format!("# could not read policy at {path_or_yaml}\nversion: 1\n"),
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
        }
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
    pub cost_cents: u64,
    #[serde(default)]
    pub budget_cents: Option<u64>,

    #[serde(default)]
    pub escalation: Option<Escalation>,
    #[serde(default)]
    pub gates: Vec<GateRun>,
    #[serde(default)]
    pub gate_failures: u32,
    /// Runs that died before producing anything — sandbox wouldn't start, clone
    /// failed, agent overran. Distinct from `gate_failures`, which means the
    /// work arrived and was judged wrong. Both have a retry budget; this one
    /// exists because a card that fails early costs nothing, so no budget stops
    /// it looping forever.
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
    /// Beads issue hash ID (e.g. `bd-a1b2`).
    #[serde(default)]
    pub beads_id: Option<String>,
    /// Associated GitHub Issue URL (e.g. `https://github.com/owner/repo/issues/8`).
    #[serde(default)]
    pub github_issue_url: Option<String>,
    /// The pull request the agent opened. Review is a real PR: approving here
    /// surfaces it, merging stays a human action.
    #[serde(default)]
    pub pr_url: Option<String>,

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
            cost_cents: 0,
            budget_cents: None,
            escalation: None,
            gates: Vec::new(),
            gate_failures: 0,
            run_failures: 0,
            diff_added: 0,
            diff_removed: 0,
            notes: Vec::new(),
            project_prompt: None,
            sandbox_profile_id: None,
            last_bounce_reason: None,
            last_conflict_files: Vec::new(),
            release_target: None,
            environment: None,
            conversation_id: None,
            parked: false,
            awaiting_dispatch: false,
            rebase_requested: false,
            beads_id: None,
            github_issue_url: None,
            pr_url: None,
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
}
