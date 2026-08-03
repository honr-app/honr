//! Encode / decode `WorkItem` rows for the board schema.
//!
//! Indexed scalars live in columns; nested and rarely filtered fields ride in
//! JSON text blobs (`origin_json`, `extras_json`, …). `blocked_by` is stored in
//! `item_blockers`, not in the item row. `blockers` summaries are never
//! persisted — they are recomputed in-process.

use crate::model::{
    Escalation, GateRun, ItemId, Lease, Note, Origin, PlanArtifact, State, TaskProposal,
    Transition, WorkItem,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use sqlx::Row;

use super::store::StoreError;

/// Meta key written after a successful one-shot `honr.json` import.
pub const META_JSON_IMPORTED: &str = "json_imported";

/// Meta key for the board's next item id allocator.
pub const META_NEXT_ID: &str = "next_id";

/// JSON blob: `BTreeMap<String, SandboxProfile>` catalog.
pub const META_SANDBOX_PROFILES: &str = "sandbox_profiles";

/// Global default sandbox profile id (empty string means unset).
pub const META_DEFAULT_SANDBOX_PROFILE_ID: &str = "default_sandbox_profile_id";

/// JSON blob: optional [`crate::model::WorkspaceBinding`].
pub const META_WORKSPACE_BINDING: &str = "workspace_binding";

/// Fields without dedicated columns — portable JSON blob on `items.extras_json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemExtras {
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
    pub gate_failures: u32,
    #[serde(default)]
    pub run_failures: u32,
    #[serde(default)]
    pub diff_added: u32,
    #[serde(default)]
    pub diff_removed: u32,
    #[serde(default)]
    pub project_prompt: Option<String>,
    #[serde(default)]
    pub sandbox_profile_id: Option<String>,
    #[serde(default)]
    pub last_bounce_reason: Option<String>,
    #[serde(default)]
    pub last_conflict_files: Vec<String>,
    #[serde(default)]
    pub release_target: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub beads_id: Option<String>,
    #[serde(default)]
    pub github_issue_url: Option<String>,
    #[serde(default)]
    pub pull_request: Option<crate::model::PullRequest>,
    /// Legacy extras field — migrated into `pull_request` on apply.
    #[serde(default)]
    pub pr_url: Option<String>,
}

impl ItemExtras {
    pub fn from_item(item: &WorkItem) -> Self {
        Self {
            engine: item.engine.clone(),
            model: item.model.clone(),
            progress: item.progress,
            cost_cents: item.cost_cents,
            budget_cents: item.budget_cents,
            gate_failures: item.gate_failures,
            run_failures: item.run_failures,
            diff_added: item.diff_added,
            diff_removed: item.diff_removed,
            project_prompt: item.project_prompt.clone(),
            sandbox_profile_id: item.sandbox_profile_id.clone(),
            last_bounce_reason: item.last_bounce_reason.clone(),
            last_conflict_files: item.last_conflict_files.clone(),
            release_target: item.release_target.clone(),
            environment: item.environment.clone(),
            conversation_id: item.conversation_id.clone(),
            beads_id: item.beads_id.clone(),
            github_issue_url: item.github_issue_url.clone(),
            pull_request: item.pull_request.clone(),
            pr_url: None,
        }
    }

    pub fn apply(self, item: &mut WorkItem) {
        item.engine = self.engine;
        item.model = self.model;
        item.progress = self.progress;
        item.cost_cents = self.cost_cents;
        item.budget_cents = self.budget_cents;
        item.gate_failures = self.gate_failures;
        item.run_failures = self.run_failures;
        item.diff_added = self.diff_added;
        item.diff_removed = self.diff_removed;
        item.project_prompt = self.project_prompt;
        item.sandbox_profile_id = self.sandbox_profile_id;
        item.last_bounce_reason = self.last_bounce_reason;
        item.last_conflict_files = self.last_conflict_files;
        item.release_target = self.release_target;
        item.environment = self.environment;
        item.conversation_id = self.conversation_id;
        item.beads_id = self.beads_id;
        item.github_issue_url = self.github_issue_url;
        item.pull_request = self.pull_request;
        item.legacy_pr_url = self.pr_url;
        item.migrate_legacy_pr_url();
    }
}

fn json_str<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Query(format!("serialize: {e}")))
}

fn json_opt_str<T: Serialize>(value: &Option<T>) -> Result<Option<String>, StoreError> {
    match value {
        None => Ok(None),
        Some(v) => Ok(Some(json_str(v)?)),
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(raw: &str, field: &str) -> Result<T, StoreError> {
    serde_json::from_str(raw)
        .map_err(|e| StoreError::Query(format!("decode {field}: {e}")))
}

fn parse_json_opt<T: for<'de> Deserialize<'de>>(
    raw: Option<&str>,
    field: &str,
) -> Result<Option<T>, StoreError> {
    match raw {
        None | Some("") => Ok(None),
        Some(s) => Ok(Some(parse_json(s, field)?)),
    }
}

fn parse_json_default<T: for<'de> Deserialize<'de> + Default>(
    raw: &str,
    field: &str,
) -> Result<T, StoreError> {
    if raw.is_empty() {
        return Ok(T::default());
    }
    parse_json(raw, field)
}

pub fn state_to_db(state: State) -> Result<String, StoreError> {
    // Wire form matches honr.json (`snake_case`), including legacy aliases on load.
    let v = serde_json::to_value(state)
        .map_err(|e| StoreError::Query(format!("state serialize: {e}")))?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| StoreError::Query("state did not serialize as string".into()))
}

pub fn state_from_db(raw: &str) -> Result<State, StoreError> {
    parse_json(&format!("\"{raw}\""), "state")
}

/// Bindable column bundle for an `items` upsert (blockers excluded).
pub struct ItemRow<'a> {
    pub id: ItemId,
    pub parent_id: Option<ItemId>,
    pub level: Option<&'a str>,
    pub title: &'a str,
    pub intent: &'a str,
    pub definition_of_done: Option<&'a str>,
    pub state: String,
    pub above_line: bool,
    pub capability: Option<&'a str>,
    pub run_deadline_at: Option<String>,
    pub parked: bool,
    pub awaiting_dispatch: bool,
    pub rebase_requested: bool,
    pub entered_state_at: String,
    pub created_at: String,
    pub origin_json: String,
    pub lease_json: Option<String>,
    pub escalation_json: Option<String>,
    pub gates_json: String,
    pub notes_json: String,
    pub history_json: String,
    pub plan_json: Option<String>,
    pub proposal_json: Option<String>,
    pub extras_json: String,
}

pub fn item_to_row(item: &WorkItem) -> Result<ItemRow<'_>, StoreError> {
    Ok(ItemRow {
        id: item.id,
        parent_id: item.parent,
        level: item.level.as_deref(),
        title: &item.title,
        intent: &item.intent,
        definition_of_done: item.definition_of_done.as_deref(),
        state: state_to_db(item.state)?,
        above_line: item.above_line,
        capability: item.capability.as_deref(),
        run_deadline_at: item.run_deadline_at.map(|t| t.to_rfc3339()),
        parked: item.parked,
        awaiting_dispatch: item.awaiting_dispatch,
        rebase_requested: item.rebase_requested,
        entered_state_at: item.entered_state_at.to_rfc3339(),
        created_at: item.created_at.to_rfc3339(),
        origin_json: json_str(&item.origin)?,
        lease_json: json_opt_str(&item.lease)?,
        escalation_json: json_opt_str(&item.escalation)?,
        gates_json: json_str(&item.gates)?,
        notes_json: json_str(&item.notes)?,
        history_json: json_str(&item.history)?,
        plan_json: json_opt_str(&item.plan)?,
        proposal_json: json_opt_str(&item.proposal)?,
        extras_json: json_str(&ItemExtras::from_item(item))?,
    })
}

fn parse_dt(raw: &str, field: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StoreError::Query(format!("parse {field}: {e}")))
}

pub fn item_from_row(row: &SqliteRow) -> Result<WorkItem, StoreError> {
    let id: i64 = row.try_get("id").map_err(|e| StoreError::Query(e.to_string()))?;
    let parent_id: Option<i64> = row
        .try_get("parent_id")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let level: Option<String> = row
        .try_get("level")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let title: String = row
        .try_get("title")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let intent: String = row
        .try_get("intent")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let definition_of_done: Option<String> = row
        .try_get("definition_of_done")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let state_raw: String = row
        .try_get("state")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let above_line: i64 = row
        .try_get("above_line")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let capability: Option<String> = row
        .try_get("capability")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let run_deadline_raw: Option<String> = row
        .try_get("run_deadline_at")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let parked: i64 = row
        .try_get("parked")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let awaiting_dispatch: i64 = row
        .try_get("awaiting_dispatch")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let rebase_requested: i64 = row
        .try_get("rebase_requested")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let entered_state_at: String = row
        .try_get("entered_state_at")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let created_at: String = row
        .try_get("created_at")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let origin_json: String = row
        .try_get("origin_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let lease_json: Option<String> = row
        .try_get("lease_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let escalation_json: Option<String> = row
        .try_get("escalation_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let gates_json: String = row
        .try_get("gates_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let notes_json: String = row
        .try_get("notes_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let history_json: String = row
        .try_get("history_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let plan_json: Option<String> = row
        .try_get("plan_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let proposal_json: Option<String> = row
        .try_get("proposal_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;
    let extras_json: String = row
        .try_get("extras_json")
        .map_err(|e| StoreError::Query(e.to_string()))?;

    let origin: Origin = parse_json(&origin_json, "origin_json")?;
    let lease: Option<Lease> = parse_json_opt(lease_json.as_deref(), "lease_json")?;
    let escalation: Option<Escalation> =
        parse_json_opt(escalation_json.as_deref(), "escalation_json")?;
    let gates: Vec<GateRun> = parse_json_default(&gates_json, "gates_json")?;
    let notes: Vec<Note> = parse_json_default(&notes_json, "notes_json")?;
    let history: Vec<Transition> = parse_json_default(&history_json, "history_json")?;
    let plan: Option<PlanArtifact> = parse_json_opt(plan_json.as_deref(), "plan_json")?;
    let proposal: Option<TaskProposal> =
        parse_json_opt(proposal_json.as_deref(), "proposal_json")?;
    let extras: ItemExtras = parse_json_default(&extras_json, "extras_json")?;

    let mut item = WorkItem {
        id: id as ItemId,
        parent: parent_id.map(|p| p as ItemId),
        level,
        title,
        intent,
        definition_of_done,
        state: state_from_db(&state_raw)?,
        origin,
        above_line: above_line != 0,
        blocked_by: Vec::new(),
        blockers: Vec::new(),
        capability,
        lease,
        run_deadline_at: match run_deadline_raw.as_deref() {
            None | Some("") => None,
            Some(s) => Some(parse_dt(s, "run_deadline_at")?),
        },
        engine: None,
        model: None,
        progress: 0.0,
        cost_cents: 0,
        budget_cents: None,
        escalation,
        gates,
        gate_failures: 0,
        run_failures: 0,
        diff_added: 0,
        diff_removed: 0,
        notes,
        project_prompt: None,
        sandbox_profile_id: None,
        last_bounce_reason: None,
        last_conflict_files: Vec::new(),
        release_target: None,
        environment: None,
        conversation_id: None,
        parked: parked != 0,
        awaiting_dispatch: awaiting_dispatch != 0,
        rebase_requested: rebase_requested != 0,
        beads_id: None,
        github_issue_url: None,
        pull_request: None,
        legacy_pr_url: None,
        plan,
        proposal,
        created_at: parse_dt(&created_at, "created_at")?,
        entered_state_at: parse_dt(&entered_state_at, "entered_state_at")?,
        history,
    };
    extras.apply(&mut item);
    Ok(item)
}

/// Parents before children so FK inserts succeed when foreign_keys are on.
pub fn parent_first(items: &[WorkItem]) -> Vec<&WorkItem> {
    let mut remaining: Vec<&WorkItem> = items.iter().collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut placed = std::collections::HashSet::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|item| {
            let ready = item
                .parent
                .map(|p| placed.contains(&p))
                .unwrap_or(true);
            if ready {
                placed.insert(item.id);
                ordered.push(*item);
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            // Cycle or missing parent — append rest so import still completes.
            ordered.append(&mut remaining);
            break;
        }
    }
    ordered
}
