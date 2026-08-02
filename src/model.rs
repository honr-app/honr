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
    Ready,
    Claimed,
    Running,
    Splitting,
    NeedsHuman,
    Verifying,
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
            State::Ready => Column::Ready,
            State::Claimed | State::Running | State::Splitting => Column::Running,
            State::NeedsHuman => Column::NeedsYou,
            State::Verifying => Column::Verify,
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
    Ready,
    Running,
    NeedsYou,
    Verify,
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


/// What makes pull-based dispatch survivable: a dead agent simply stops
/// renewing, and the card returns to Ready with no supervisor involved.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Lease {
    pub agent_id: String,
    pub granted_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl Lease {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now > self.expires_at
    }

    /// The one number that tells you an agent is thinking versus hung.
    pub fn heartbeat_age_secs(&self, now: DateTime<Utc>) -> i64 {
        (now - self.last_heartbeat).num_seconds().max(0)
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

/// Source of truth for a Project's breakdown. Approve Plan applies this DAG.
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

/// Title of the seed Task created with every Project.
pub const INITIAL_PLAN_TITLE: &str = "Initial plan";

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
    /// Pinned constraints bind every descendant, forever. "We use Decimal for
    /// money, never float" said once should not need saying twice.
    #[serde(default)]
    pub pinned: Vec<String>,

    /// The bounce reason if this card was returned to Ready due to an infra or execution bounce.
    #[serde(default)]
    pub last_bounce_reason: Option<String>,

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
    /// Set by park: card is Ready but must not be claimed until the human
    /// explicitly resumes. Without this, park immediately re-dispatches.
    #[serde(default)]
    pub parked: bool,
    /// Beads issue hash ID (e.g. `bd-a1b2`).
    #[serde(default)]
    pub beads_id: Option<String>,
    /// Associated GitHub Issue URL (e.g. `https://github.com/shanemcd/honr/issues/8`).
    #[serde(default)]
    pub github_issue_url: Option<String>,
    /// The pull request the agent opened. Review is a real PR: approving here
    /// surfaces it, merging stays a human action.
    #[serde(default)]
    pub pr_url: Option<String>,

    /// Plan artifact — Projects only (Phase 1). Source of truth for Approve Plan.
    #[serde(default)]
    pub plan: Option<PlanArtifact>,

    /// Projects only: supervisor will not claim Ready Tasks under this Project
    /// while true. Independent of the global board `dispatch_paused` flag —
    /// both must be clear for a card to be claimable. Ignored on Tasks.
    #[serde(default)]
    pub dispatch_paused: bool,

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
            pinned: Vec::new(),
            last_bounce_reason: None,
            release_target: None,
            environment: None,
            conversation_id: None,
            parked: false,
            beads_id: None,
            github_issue_url: None,
            pr_url: None,
            plan: None,
            dispatch_paused: false,
            created_at: now,
            entered_state_at: now,
            history: Vec::new(),
        }
    }

    pub fn is_project(&self) -> bool {
        self.parent.is_none() && self.level.as_deref() != Some("Task")
    }

    pub fn is_initial_plan_task(&self) -> bool {
        self.title == INITIAL_PLAN_TITLE
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
