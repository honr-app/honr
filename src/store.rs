//! The board. Written to at machine speed, read by agents as their source of
//! truth, and moving a card *is* an action.
//!
//! Both faces — REST/SSE for humans, MCP for the cockpit and for agents — call
//! into here. Neither owns any state-machine logic, which is what keeps the two
//! renderings from drifting.

use crate::events::BoardEvent;
use crate::machine::{self, TransitionError};
use crate::model::*;
use crate::schema::{Level, Schema};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

// ---------------------------------------------------------------- persistence

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BoardState {
    pub next_id: ItemId,
    pub items: BTreeMap<ItemId, WorkItem>,
    /// Per-goal running narrative. A few lines appended at meaningful moments,
    /// not an event log — most people will read this instead of the board, and
    /// they will be right to.
    #[serde(default)]
    pub stories: BTreeMap<ItemId, Vec<StoryLine>>,
    /// When true, the supervisor skips claiming new Ready cards. In-flight
    /// Claimed/Running work continues. Persists across restarts.
    #[serde(default)]
    pub dispatch_paused: bool,
    #[serde(skip)]
    pub agent_logs: BTreeMap<ItemId, std::collections::VecDeque<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryLine {
    pub at: DateTime<Utc>,
    pub text: String,
}

// ------------------------------------------------------------------ read views

#[derive(Debug, Clone, Serialize)]
pub struct ChunkSummary {
    pub count: usize,
    /// Chunked, never compressed: `+7 more` hides seven items and tells you
    /// nothing. `7 ready · 2 blocked on #41 · oldest 40m` is smaller *and*
    /// answers the question.
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnView {
    pub column: Column,
    pub summary: ChunkSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalView {
    pub id: ItemId,
    pub title: String,
    pub intent: String,
    pub progress: f32,
    pub leaves_done: usize,
    pub leaves_total: usize,
    pub spend_cents: u64,
    pub budget_cents: Option<u64>,
    pub agents_live: usize,
    pub needs_you: usize,
    /// `no_plan` | `awaiting_approval` | `approved_vN`
    pub plan_status: String,
    /// Project-level dispatch pause (independent of global `dispatch_paused`).
    pub dispatch_paused: bool,
    pub columns: Vec<ColumnView>,
    pub story: Vec<StoryLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub items: Vec<WorkItem>,
    pub levels: Vec<Level>,
    pub goals: Vec<GoalView>,
    pub server_time: DateTime<Utc>,
    pub heartbeat_expect_secs: i64,
    pub seq: u64,
    /// Supervisor will not claim new Ready cards while true.
    pub dispatch_paused: bool,
    pub default_engine: String,
    pub default_model: String,
}

/// What `claim` hands back. The card alone would be a title; the chain is why
/// the agent knows "air-gapped" rules out the SDK's phone-home telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AncestryLine {
    pub level: String,
    pub title: String,
    pub intent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClaimGrant {
    pub item_id: ItemId,
    pub title: String,
    pub definition_of_done: Option<String>,
    /// Canonical beads hash id when mirrored (e.g. `honr-a1b2`).
    pub beads_id: Option<String>,
    /// Project → this Task. Short why-chain for the agent.
    pub ancestry: Vec<AncestryLine>,
    /// Standing constraints inherited from every ancestor.
    pub constraints: Vec<String>,
    pub notes: Vec<String>,
    pub lease_expires_at: DateTime<Utc>,
    pub budget_remaining_cents: Option<u64>,
    pub engine: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct NeedsYou {
    pub id: ItemId,
    pub title: String,
    pub question: String,
    pub options: Vec<String>,
    pub recommended: usize,
    pub blocked_secs: i64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GoalDigest {
    pub goal_id: ItemId,
    pub goal: String,
    pub merged: usize,
    pub spend_cents: u64,
    pub budget_cents: Option<u64>,
    pub needs_you: Vec<NeedsYou>,
    pub running: usize,
    pub running_stalled: usize,
    pub ready: usize,
    pub in_review: usize,
    pub latest_story: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Digest {
    pub since: DateTime<Utc>,
    pub goals: Vec<GoalDigest>,
}

// ------------------------------------------------------------------ the board

pub struct Board {
    state: RwLock<BoardState>,
    tx: broadcast::Sender<BoardEvent>,
    seq: AtomicU64,
    dirty: AtomicBool,
    pub schema: Schema,
    path: PathBuf,
    started_at: DateTime<Utc>,
    pub beads: Option<crate::beads::BeadsClient>,
}

pub type SharedBoard = Arc<Board>;

impl Board {
    pub fn new(schema: Schema, path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1024);
        // Co-locate beads with the board file when possible. Prefer an absolute
        // beads dir so `bd`'s current_dir is never the empty relative parent of
        // `.beads` (that used to make every `bd` spawn fail with ENOENT).
        let beads_dir = {
            let raw = path
                .parent()
                .map(|p| p.join(".beads"))
                .unwrap_or_else(|| PathBuf::from(".beads"));
            if raw.is_absolute() {
                raw
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&raw))
                    .unwrap_or(raw)
            }
        };
        let beads = Some(crate::beads::BeadsClient::new(beads_dir));
        Self {
            state: RwLock::new(BoardState { next_id: 1, ..Default::default() }),
            tx,
            seq: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            schema,
            path,
            started_at: Utc::now(),
            beads,
        }
    }

    /// Load a previously persisted board, or start empty.
    pub fn load_or_new(schema: Schema, path: PathBuf) -> Self {
        let board = Self::new(schema, path.clone());
        if let Ok(raw) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<BoardState>(&raw) {
                Ok(mut state) => {
                    let mut healed = 0usize;
                    for (id, item) in state.items.iter_mut() {
                        if item.beads_id.is_none() {
                            item.beads_id = Some(format!("bd-honr-{id}"));
                        }
                        // A brief experiment left Initial plan in Shaping; restore
                        // them to Ready so dedicated planning agents can claim.
                        if item.is_initial_plan_task() && item.state == State::Shaping {
                            item.state = State::Ready;
                            healed += 1;
                        }
                    }
                    tracing::info!(items = state.items.len(), "restored board from {path:?}");
                    if healed > 0 {
                        tracing::info!("healed {healed} Initial plan Task(s) Shaping → Ready");
                    }
                    *board.state.write().unwrap() = state;
                }
                Err(e) => tracing::warn!("ignoring unreadable {path:?}: {e}"),
            }
        }
        board
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BoardEvent> {
        self.tx.subscribe()
    }

    pub fn dispatch_paused(&self) -> bool {
        self.state.read().unwrap().dispatch_paused
    }

    /// Pause or resume supervisor dispatch globally. Does not touch in-flight runs.
    ///
    /// Pause stamps every Project as paused. Resume clears the global flag and
    /// every Project pause. While globally paused, Resume on an individual
    /// Project is an exception — that subtree may claim again.
    pub fn set_dispatch_paused(&self, paused: bool) {
        let stamped: Vec<WorkItem> = {
            let mut s = self.state.write().unwrap();
            let global_changed = s.dispatch_paused != paused;
            s.dispatch_paused = paused;
            let mut changed = Vec::new();
            for it in s.items.values_mut() {
                if !it.is_project() {
                    continue;
                }
                if it.dispatch_paused == paused {
                    continue;
                }
                it.dispatch_paused = paused;
                changed.push(it.clone());
            }
            if !global_changed && changed.is_empty() {
                return;
            }
            changed
        };
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.tx.send(BoardEvent::DispatchPaused {
            seq: self.next_seq(),
            paused,
        });
        for item in stamped {
            self.emit(&item);
        }
        tracing::info!(
            paused,
            "dispatch {}",
            if paused {
                "paused (all projects stamped)"
            } else {
                "resumed (all project pauses cleared)"
            }
        );
    }

    /// Pause or resume claiming under one Project. Does not touch in-flight runs.
    ///
    /// While the board is globally paused, `paused: false` is an allowlist
    /// exception — that Project may claim even though the header still says
    /// Resume (global).
    pub fn set_project_dispatch_paused(
        &self,
        id: ItemId,
        paused: bool,
    ) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item #{id}"))?;
            if !it.is_project() {
                return Err(format!("#{id} is not a Project"));
            }
            if it.dispatch_paused == paused {
                return Ok(it.clone());
            }
            it.dispatch_paused = paused;
            it.clone()
        };
        self.emit(&item);
        tracing::info!(
            id,
            paused,
            "project dispatch {}",
            if paused { "paused" } else { "resumed" }
        );
        Ok(item)
    }

    /// Whether the supervisor may claim this Ready card right now.
    ///
    /// A card is claimable when its Project is not paused. Global pause does
    /// not block by itself — it stamps every Project paused; Resume on a
    /// Project clears that stamp and becomes an exception. Orphan tasks (no
    /// Project) are blocked only while the global flag is set.
    pub fn may_claim(&self, id: ItemId) -> bool {
        let s = self.state.read().unwrap();
        let mut cur = Some(id);
        while let Some(cid) = cur {
            let Some(it) = s.items.get(&cid) else {
                break;
            };
            if it.is_project() {
                return !it.dispatch_paused;
            }
            cur = it.parent;
        }
        // No Project ancestor.
        !s.dispatch_paused
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn emit(&self, item: &WorkItem) {
        let mut item = item.clone();
        {
            let s = self.state.read().unwrap();
            Self::populate_blockers(&s, &mut item);
        }
        let _ = self.tx.send(BoardEvent::Upsert {
            seq: self.next_seq(),
            item: Box::new(item),
        });
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Flush to disk if anything changed. Called on an interval so a fleet of
    /// heartbeating agents doesn't turn into a write storm.
    pub fn flush(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let json = { serde_json::to_string_pretty(&*self.state.read().unwrap()) };
        let Ok(json) = json else { return };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    pub fn populate_blockers(s: &BoardState, item: &mut WorkItem) {
        item.blockers = item
            .blocked_by
            .iter()
            .filter_map(|&bid| {
                s.items.get(&bid).map(|b| BlockerSummary {
                    id: b.id,
                    title: b.title.clone(),
                    state: b.state,
                })
            })
            .collect();
    }

    // ------------------------------------------------------------ tree reads

    pub fn get(&self, id: ItemId) -> Option<WorkItem> {
        let s = self.state.read().unwrap();
        s.items.get(&id).map(|i| {
            let mut item = i.clone();
            Self::populate_blockers(&s, &mut item);
            item
        })
    }

    pub fn children_of(&self, id: ItemId) -> Vec<ItemId> {
        let s = self.state.read().unwrap();
        s.items.values().filter(|i| i.parent == Some(id)).map(|i| i.id).collect()
    }

    fn has_children(s: &BoardState, id: ItemId) -> bool {
        s.items.values().any(|i| i.parent == Some(id) && i.state != State::Retired)
    }

    fn depth(s: &BoardState, id: ItemId) -> usize {
        let mut depth = 0;
        let mut cur = s.items.get(&id).and_then(|i| i.parent);
        while let Some(p) = cur {
            depth += 1;
            if depth > 32 {
                break; // cycle guard
            }
            cur = s.items.get(&p).and_then(|i| i.parent);
        }
        depth
    }

    fn chain(s: &BoardState, id: ItemId) -> Vec<ItemId> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            out.push(c);
            if out.len() > 32 {
                break;
            }
            cur = s.items.get(&c).and_then(|i| i.parent);
        }
        out.reverse();
        out
    }

    /// The Project a card belongs to. Swimlanes go by Project, never by agent.
    fn goal_of(s: &BoardState, id: ItemId) -> ItemId {
        let chain = Self::chain(s, id);
        // Roots are Projects; every Task's swimlane is its Project root.
        chain.first().copied().unwrap_or(id)
    }

    fn level_name(&self, s: &BoardState, id: ItemId) -> String {
        if let Some(l) = s.items.get(&id).and_then(|i| i.level.clone()) {
            return l;
        }
        // Machine-created depth collapses into the nearest declared rung.
        self.schema
            .level_for_depth(Self::depth(s, id))
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "Item".into())
    }

    /// Which goal swimlane a card belongs to.
    pub fn goal_for(&self, id: ItemId) -> ItemId {
        let s = self.state.read().unwrap();
        Self::goal_of(&s, id)
    }

    pub fn ancestry(&self, id: ItemId) -> Vec<AncestryLine> {
        let s = self.state.read().unwrap();
        Self::chain(&s, id)
            .into_iter()
            .filter_map(|cid| {
                s.items.get(&cid).map(|i| AncestryLine {
                    level: self.level_name(&s, cid),
                    title: i.title.clone(),
                    intent: i.intent.clone(),
                })
            })
            .collect()
    }

    /// A correction that stops being a one-off: pins bind every descendant.
    pub fn inherited_pins(&self, id: ItemId) -> Vec<String> {
        let s = self.state.read().unwrap();
        Self::chain(&s, id)
            .into_iter()
            .filter_map(|cid| s.items.get(&cid))
            .flat_map(|i| i.pinned.iter().cloned())
            .collect()
    }

    pub fn append_agent_log(&self, id: ItemId, line: impl Into<String>) {
        let mut s = self.state.write().unwrap();
        let logs = s.agent_logs.entry(id).or_default();
        if logs.len() >= 300 {
            logs.pop_front();
        }
        logs.push_back(line.into());
    }

    pub fn get_agent_logs(&self, id: ItemId) -> Vec<String> {
        let s = self.state.read().unwrap();
        s.agent_logs.get(&id).map(|l| l.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn clear_agent_logs(&self, id: ItemId) {
        let mut s = self.state.write().unwrap();
        s.agent_logs.remove(&id);
    }

    fn unresolved_blockers(s: &BoardState, item: &WorkItem) -> Vec<ItemId> {
        item.blocked_by
            .iter()
            .copied()
            .filter(|b| s.items.get(b).map(|i| !i.state.is_terminal()).unwrap_or(false))
            .collect()
    }

    // ------------------------------------------------------------- mutations

    /// The single write path. Everything else funnels through here so the
    /// invariants can't be routed around.
    fn transition_locked(
        s: &mut BoardState,
        id: ItemId,
        to: State,
        by: &str,
        reason: Option<String>,
    ) -> Result<WorkItem, TransitionError> {
        let has_children = Self::has_children(s, id);
        let item = s.items.get(&id).ok_or(TransitionError::NoSuchItem(id))?;
        let blockers = Self::unresolved_blockers(s, item);
        machine::check(item, to, has_children, &blockers)?;

        let now = Utc::now();
        let item = s.items.get_mut(&id).unwrap();
        let from = item.state;
        item.history.push(Transition { at: now, from, to, by: by.to_string(), reason });
        item.state = to;
        if from != to {
            item.entered_state_at = now;
        }

        // States that imply no agent is holding the card.
        if matches!(to, State::Ready | State::NeedsHuman | State::Done | State::Retired | State::Shaping) {
            item.lease = None;
        }
        if to == State::Ready {
            item.progress = 0.0;
            if by == "human" {
                item.run_failures = 0;
                item.escalation = None;
                item.last_bounce_reason = None;
            }
        }
        let mut item_out = item.clone();
        Self::populate_blockers(s, &mut item_out);
        Ok(item_out)
    }

    pub fn transition(
        &self,
        id: ItemId,
        to: State,
        by: &str,
        reason: Option<String>,
    ) -> Result<WorkItem, TransitionError> {
        let item = {
            let mut s = self.state.write().unwrap();
            Self::transition_locked(&mut s, id, to, by, reason)?
        };
        self.emit(&item);

        if to == State::Done || to == State::Retired {
            let beads = self.beads.clone();
            let beads_id = item.beads_id.clone();
            let reason_str = item
                .history
                .last()
                .and_then(|h| h.reason.clone())
                .unwrap_or_else(|| format!("Marked {to:?}"));
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    if let (Some(b), Some(bid)) = (beads, beads_id) {
                        if crate::beads::BeadsClient::is_real_id(&bid) {
                            let _ = b.close(&bid, Some(&reason_str)).await;
                            let _ = b.github_push(&[bid]).await;
                            b.schedule_dolt_push();
                        }
                    }
                });
            }
        }

        Ok(item)
    }

    /// Create a Project (root) or a Task under a Project. Tasks are flat —
    /// nesting under another Task is refused.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        parent: Option<ItemId>,
        title: impl Into<String>,
        intent: impl Into<String>,
        definition_of_done: Option<String>,
        origin: Origin,
        above_line: bool,
        capability: Option<String>,
    ) -> Result<WorkItem, String> {
        if let Some(pid) = parent {
            let s = self.state.read().unwrap();
            let Some(p) = s.items.get(&pid) else {
                return Err(format!("no parent #{pid}"));
            };
            if p.parent.is_some() {
                return Err(
                    "tasks are flat under a Project; cannot nest under another task".into(),
                );
            }
            if p.level.as_deref() == Some("Task") {
                return Err("cannot add children to a Task; parent must be a Project".into());
            }
        }

        let item = {
            let mut s = self.state.write().unwrap();
            let id = s.next_id;
            s.next_id += 1;

            let mut item = WorkItem::new(id, title, intent);
            item.parent = parent;
            item.definition_of_done = definition_of_done;
            item.origin = origin;
            item.above_line = above_line;
            item.capability = capability;
            item.beads_id = Some(format!("bd-honr-{id}"));
            item.level = if parent.is_none() {
                self.schema
                    .project_level()
                    .map(|l| l.name.clone())
                    .or_else(|| Some("Project".into()))
            } else {
                self.schema
                    .task_level()
                    .map(|l| l.name.clone())
                    .or_else(|| Some("Task".into()))
            };
            if parent.is_none() {
                item.plan = Some(PlanArtifact::empty());
                // Global pause stamps existing projects; new ones must start
                // paused too or they'd slip through as accidental exceptions.
                if s.dispatch_paused {
                    item.dispatch_paused = true;
                }
            }
            s.items.insert(id, item.clone());
            let mut item_out = item;
            Self::populate_blockers(&s, &mut item_out);
            item_out
        };
        self.emit(&item);

        // Every Project gets a claimable Initial Plan Task so planning can run
        // as a dedicated sandbox job (optional plan PR + split into siblings).
        if parent.is_none() {
            self.seed_initial_plan_task(item.id, &item.title)?;
        }
        Ok(item)
    }

    fn seed_initial_plan_task(&self, project_id: ItemId, project_title: &str) -> Result<WorkItem, String> {
        let seed = self.create(
            Some(project_id),
            INITIAL_PLAN_TITLE,
            format!(
                "Produce a Plan for «{project_title}» — flat sibling Tasks with deps and \
                 mechanically checkable DoDs. You may open one plan/docs PR, then finish by \
                 writing split.json (PR + split is allowed on this card only)."
            ),
            Some("Sibling Tasks materialized via split (or Approve Plan).".into()),
            Origin::Planner,
            false,
            None,
        )?;
        let _ = self.transition(seed.id, State::Shaping, "cockpit", Some("seed plan task".into()));
        let seed = self
            .transition(seed.id, State::Ready, "cockpit", Some("seed plan task".into()))
            .map_err(|e| e.to_string())?;
        self.story(
            project_id,
            format!("Seeded {INITIAL_PLAN_TITLE} Task #{}.", seed.id),
        );
        Ok(seed)
    }

    /// Write / revise the Plan artifact on a Project. Does not create board Tasks
    /// — Approve Plan materializes them.
    pub fn propose_plan(
        &self,
        project_id: ItemId,
        summary: impl Into<String>,
        tasks: Vec<PlanTaskSpec>,
        cancel_keys: Vec<String>,
    ) -> Result<PlanArtifact, String> {
        if tasks.is_empty() {
            return Err("a plan needs at least one task".into());
        }
        for t in &tasks {
            if t.definition_of_done.trim().is_empty() {
                return Err(format!(
                    "task '{}' has no definition of done; without one the board is a wish list",
                    t.title
                ));
            }
            if t.key.trim().is_empty() {
                return Err(format!("task '{}' needs a stable plan key", t.title));
            }
        }
        let plan = {
            let mut s = self.state.write().unwrap();
            let project = s
                .items
                .get_mut(&project_id)
                .ok_or_else(|| format!("no work item #{project_id}"))?;
            if !project.is_project() {
                return Err("plan parent must be a Project".into());
            }
            let mut next = project.plan.clone().unwrap_or_else(PlanArtifact::empty);
            // Preserve item_id links for keys that already materialized.
            let prev_ids: BTreeMap<String, ItemId> = next
                .tasks
                .iter()
                .filter_map(|t| t.item_id.map(|id| (t.key.clone(), id)))
                .collect();
            let cancel_item_ids: Vec<ItemId> = cancel_keys
                .iter()
                .filter_map(|k| prev_ids.get(k).copied())
                .collect();
            next.revision = next.revision.saturating_add(1);
            next.summary = summary.into();
            next.status = PlanStatus::AwaitingApproval;
            next.cancel_keys = cancel_keys;
            next.cancel_item_ids = cancel_item_ids;
            next.tasks = tasks
                .into_iter()
                .map(|mut t| {
                    if t.item_id.is_none() {
                        t.item_id = prev_ids.get(&t.key).copied();
                    }
                    t
                })
                .collect();
            project.plan = Some(next.clone());
            let snap = project.clone();
            drop(s);
            self.emit(&snap);
            next
        };
        self.story(
            project_id,
            format!(
                "Plan v{} proposed ({} tasks) — awaiting Approve Plan.",
                plan.revision,
                plan.tasks.len()
            ),
        );
        Ok(plan)
    }

    /// Materialize the Project's Plan artifact into flat Tasks + deps, publish
    /// them to Ready, and close open Initial Plan Tasks. Never moves the Project
    /// to Ready.
    pub fn approve_plan(&self, project_id: ItemId) -> Result<Vec<ItemId>, String> {
        let project = self
            .get(project_id)
            .ok_or_else(|| format!("no work item #{project_id}"))?;
        if !project.is_project() {
            return Err("approve_plan requires a Project".into());
        }

        if let Some(plan) = project.plan.clone().filter(|p| !p.tasks.is_empty()) {
            return self.materialize_and_publish_plan(project_id, plan);
        }

        // Legacy: shaping children with no artifact (pre-plan boards).
        let mut published = Vec::new();
        for cid in self.children_of(project_id) {
            let Some(child) = self.get(cid) else { continue };
            if child.is_initial_plan_task() {
                continue;
            }
            if child.state == State::Shaping
                && self
                    .transition(cid, State::Ready, "human", Some("plan approved".into()))
                    .is_ok()
            {
                published.push(cid);
            }
        }
        if published.is_empty() {
            return Err(
                "no Plan artifact to approve — run propose_breakdown (or wait for the Initial plan Task)"
                    .into(),
            );
        }
        self.close_plan_tasks(project_id);
        self.story(
            project_id,
            format!("Plan approved: {} tasks published to Ready (legacy path).", published.len()),
        );
        Ok(published)
    }

    fn materialize_and_publish_plan(
        &self,
        project_id: ItemId,
        mut plan: PlanArtifact,
    ) -> Result<Vec<ItemId>, String> {
        for id in &plan.cancel_item_ids {
            let _ = self.transition(*id, State::Retired, "human", Some("cancelled by replan".into()));
        }

        // Create / update Tasks for each plan spec.
        for spec in plan.tasks.iter_mut() {
            if let Some(existing_id) = spec.item_id {
                if self.get(existing_id).is_some() {
                    let _ = self.update_item(
                        existing_id,
                        Some(spec.title.clone()),
                        Some(spec.intent.clone()),
                        Some(spec.definition_of_done.clone()),
                        None,
                    );
                    continue;
                }
            }
            let child = self.create(
                Some(project_id),
                spec.title.clone(),
                spec.intent.clone(),
                Some(spec.definition_of_done.clone()),
                Origin::Planner,
                false,
                spec.capability.clone(),
            )?;
            let _ = self.transition(child.id, State::Shaping, "planner", Some("from plan".into()));
            spec.item_id = Some(child.id);
        }

        // Resolve key → id, then wire blocked_by.
        let key_to_id: BTreeMap<String, ItemId> = plan
            .tasks
            .iter()
            .filter_map(|t| t.item_id.map(|id| (t.key.clone(), id)))
            .collect();
        for spec in &plan.tasks {
            let Some(id) = spec.item_id else { continue };
            let blockers: Vec<ItemId> = spec
                .blocked_by_keys
                .iter()
                .filter_map(|k| key_to_id.get(k).copied())
                .collect();
            self.set_blocked_by(id, blockers);
        }

        // Publish to Ready.
        let mut published = Vec::new();
        for spec in &plan.tasks {
            let Some(id) = spec.item_id else { continue };
            if let Some(child) = self.get(id) {
                if (child.state == State::Shaping
                    && self
                        .transition(id, State::Ready, "human", Some("plan approved".into()))
                        .is_ok())
                    || child.state == State::Ready
                {
                    published.push(id);
                }
            }
        }

        plan.status = PlanStatus::Approved;
        plan.approved_revision = Some(plan.revision);
        {
            let mut s = self.state.write().unwrap();
            if let Some(p) = s.items.get_mut(&project_id) {
                p.plan = Some(plan.clone());
                let snap = p.clone();
                drop(s);
                self.emit(&snap);
            }
        }

        self.close_plan_tasks(project_id);
        self.story(
            project_id,
            format!(
                "Plan v{} approved: {} tasks published to Ready.",
                plan.revision,
                published.len()
            ),
        );
        Ok(published)
    }

    fn close_plan_tasks(&self, project_id: ItemId) {
        for cid in self.children_of(project_id) {
            let Some(child) = self.get(cid) else { continue };
            if !child.is_initial_plan_task() || child.state.is_terminal() {
                continue;
            }
            // Prefer Done from shaping/ready/running paths the machine allows.
            if child.state == State::Ready
                || child.state == State::Shaping
                || child.state == State::NeedsHuman
            {
                let _ = self.transition(
                    cid,
                    State::Done,
                    "human",
                    Some("plan approved; tasks materialized".into()),
                );
            } else if matches!(child.state, State::Claimed | State::Running) {
                // Halt-ish: Ready then Done if needed.
                let _ = self.transition(cid, State::Ready, "human", Some("plan approved".into()));
                let _ = self.transition(
                    cid,
                    State::Done,
                    "human",
                    Some("plan approved; tasks materialized".into()),
                );
            }
        }
    }

    /// Dual-write a single board item into beads (Project→epic, Task→task with `--parent`).
    /// If successful, stores the real hash id, then pushes **that** bead to GitHub
    /// (`bd github push <id>`) without blocking other mirrors on the push.
    pub async fn mirror_beads_item(self: &Arc<Self>, id: ItemId) {
        let Some(beads_id) = self.mirror_beads_item_local(id).await else {
            return;
        };
        let needs_url = self
            .get(id)
            .map(|i| i.github_issue_url.is_none())
            .unwrap_or(true);
        if !needs_url {
            return;
        }
        let board = Arc::clone(self);
        tokio::spawn(async move {
            if let Some(beads) = board.beads.clone() {
                if let Err(e) = beads.github_push(std::slice::from_ref(&beads_id)).await {
                    tracing::warn!(id, error = %e, "beads github push after mirror create failed");
                }
                beads.schedule_dolt_push();
            }
            board.refresh_github_issue_url(id, &beads_id).await;
        });
    }

    /// Create/link the beads issue and store `beads_id`, without talking to GitHub.
    /// No-ops (returns existing id) when the card already has a real beads id.
    async fn mirror_beads_item_local(self: &Arc<Self>, id: ItemId) -> Option<String> {
        let item = self.get(id)?;
        if item.state == State::Retired {
            return None;
        }
        if let Some(bid) = item
            .beads_id
            .as_deref()
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
        {
            return Some(bid.to_string());
        }
        let beads = self.beads.clone()?;
        let title = item.title.clone();
        let intent = item.intent.clone();
        let is_project = item.parent.is_none();
        let parent_beads = item
            .parent
            .and_then(|pid| self.get(pid).and_then(|p| p.beads_id))
            .filter(|bid| crate::beads::BeadsClient::is_real_id(bid));
        let blockers: Vec<String> = item
            .blocked_by
            .iter()
            .filter_map(|bid| self.get(*bid).and_then(|b| b.beads_id))
            .filter(|bid| crate::beads::BeadsClient::is_real_id(bid))
            .collect();

        let issue_type = if is_project { "epic" } else { "task" };
        let parent = parent_beads.as_deref();

        match beads
            .create_linked(&title, 2, issue_type, Some(&intent), parent, &blockers)
            .await
        {
            Ok(issue) => {
                self.set_beads_id(id, &issue.id);
                if crate::beads::BeadsClient::is_real_id(&issue.id) {
                    Some(issue.id)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!(id, error = %e, "beads mirror create failed");
                None
            }
        }
    }

    /// Copy `external_ref` / issue URL from beads onto the board card.
    /// Returns true when `github_issue_url` was set.
    async fn refresh_github_issue_url(self: &Arc<Self>, id: ItemId, beads_id: &str) -> bool {
        let Some(beads) = self.beads.clone() else {
            return false;
        };
        match beads.show(beads_id).await {
            Ok(show_issue) => {
                if let Some(url) = show_issue.github_issue_url() {
                    self.set_github_issue_url(id, &url);
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                tracing::warn!(
                    id,
                    beads_id,
                    error = %e,
                    "beads show for github_issue_url failed"
                );
                false
            }
        }
    }

    /// Dual-write a board item into beads (Project→epic, Task→task with `--parent`).
    /// Call after `create` when you hold a `SharedBoard` so the real hash id can be stored.
    pub fn schedule_beads_mirror(self: &Arc<Self>, id: ItemId) {
        let board = Arc::clone(self);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                board.mirror_beads_item(id).await;
            });
        }
    }

    /// For non-retired cards that already have a real beads id but no
    /// `github_issue_url`, copy the URL from beads (push first if needed).
    ///
    /// Covers the gap heal used to leave: beads_id assigned without URL, then
    /// skipped forever because it was no longer a placeholder.
    pub async fn backfill_missing_github_issue_urls(self: &Arc<Self>) -> usize {
        let missing: Vec<(ItemId, String)> = {
            let s = self.state.read().unwrap();
            let mut missing = Vec::new();
            for (id, item) in s.items.iter() {
                if item.state == State::Retired {
                    continue;
                }
                if item.github_issue_url.is_some() {
                    continue;
                }
                if let Some(bid) = item
                    .beads_id
                    .as_deref()
                    .filter(|b| crate::beads::BeadsClient::is_real_id(b))
                {
                    missing.push((*id, bid.to_string()));
                }
            }
            missing.sort_by_key(|(id, _)| *id);
            missing
        };
        if missing.is_empty() {
            return 0;
        }
        let Some(beads) = self.beads.clone() else {
            return 0;
        };

        let mut need_push: Vec<(ItemId, String)> = Vec::new();
        let mut filled = 0usize;
        for (id, beads_id) in &missing {
            if self.refresh_github_issue_url(*id, beads_id).await {
                filled += 1;
            } else {
                need_push.push((*id, beads_id.clone()));
            }
        }
        if !need_push.is_empty() {
            let ids: Vec<String> = need_push.iter().map(|(_, bid)| bid.clone()).collect();
            if let Err(e) = beads.github_push(&ids).await {
                tracing::warn!(error = %e, "beads github push during url backfill failed");
            }
            beads.schedule_dolt_push();
            for (id, beads_id) in &need_push {
                if self.refresh_github_issue_url(*id, beads_id).await {
                    filled += 1;
                }
            }
        }
        if filled > 0 {
            tracing::info!("backfilled {filled} missing github_issue_url(s)");
        }
        filled
    }

    /// Re-run the create+sync mirror for open (non-retired) cards carrying placeholder beads_ids.
    /// Projects are mirrored before Tasks so children receive real parent beads_ids.
    /// Also backfills missing `github_issue_url` on cards that already have real beads ids.
    pub async fn heal_placeholder_beads_ids(self: &Arc<Self>) -> usize {
        let (projects, tasks) = {
            let s = self.state.read().unwrap();
            let mut projects = Vec::new();
            let mut tasks = Vec::new();
            for (id, item) in s.items.iter() {
                if item.state == State::Retired {
                    continue;
                }
                let is_placeholder = item
                    .beads_id
                    .as_deref()
                    .is_none_or(|bid| !crate::beads::BeadsClient::is_real_id(bid));
                if is_placeholder {
                    if item.parent.is_none() {
                        projects.push(*id);
                    } else {
                        tasks.push(*id);
                    }
                }
            }
            projects.sort();
            tasks.sort();
            (projects, tasks)
        };

        // Local creates first (projects before tasks), then one selective GitHub
        // push for the whole batch — not a full-graph sync per card.
        let mut created: Vec<(ItemId, String)> = Vec::new();
        for id in projects.into_iter().chain(tasks) {
            if let Some(beads_id) = self.mirror_beads_item_local(id).await {
                created.push((id, beads_id));
            }
        }
        if !created.is_empty() {
            if let Some(beads) = self.beads.clone() {
                let ids: Vec<String> = created.iter().map(|(_, bid)| bid.clone()).collect();
                if let Err(e) = beads.github_push(&ids).await {
                    tracing::warn!(error = %e, "beads github push after heal batch failed");
                }
                beads.schedule_dolt_push();
                for (id, beads_id) in &created {
                    self.refresh_github_issue_url(*id, beads_id).await;
                }
            }
            let healed = created.len();
            tracing::info!("healed {healed} placeholder beads_id(s) with real beads IDs");
        }

        // Always run: real beads_id + missing URL is a separate failure mode from placeholders.
        self.backfill_missing_github_issue_urls().await;
        created.len()
    }

    pub fn set_beads_id(&self, id: ItemId, beads_id: &str) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.beads_id = Some(beads_id.to_string());
            it.clone()
        };
        self.emit(&item);
    }

    pub fn set_github_issue_url(&self, id: ItemId, url: &str) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.github_issue_url = Some(url.to_string());
            it.clone()
        };
        self.emit(&item);
    }

    /// Unused until the supervisor enforces a per-card cents cap.
    #[allow(dead_code)]
    pub fn set_budget(&self, id: ItemId, cents: u64) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.budget_cents = Some(cents);
            it.clone()
        };
        self.emit(&item);
    }

    /// A run died without producing work. Requeue while there is budget left,
    /// then hand it to a human.
    ///
    /// This exists because the money caps do not cover it: a card that fails
    /// *early* — sandbox won't start, clone rejected — spends nothing, so
    /// nothing stops the sweeper requeueing it every lease period forever.
    /// Left alone overnight that is an infinite loop building and destroying
    /// sandboxes. Mirrors the retry budget `settle_gates` already applies to
    /// gate failures.
    pub fn record_run_failure(
        &self,
        id: ItemId,
        reason: &str,
        max_attempts: u32,
    ) -> Result<WorkItem, String> {
        let (failures, title, state) = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.run_failures += 1;
            (it.run_failures, it.title.clone(), it.state)
        };

        // A run that died before its first heartbeat is still Claimed, and
        // Claimed -> NeedsHuman is not a legal edge. Promote first so the
        // escalation below has somewhere to go.
        if state == State::Claimed {
            let _ = self.transition(id, State::Running, "supervisor", None);
        }

        if failures < max_attempts {
            let item = self
                .transition(id, State::Ready, "supervisor", Some(format!("run failed: {reason}")))
                .map_err(|e| e.to_string())?;
            self.story(
                id,
                format!("{title} failed to run ({failures}/{max_attempts}): {reason}"),
            );
            return Ok(item);
        }

        self.escalate(
            id,
            "supervisor",
            format!(
                "{title} failed to run {failures} times without producing any work. \
                 Last failure: {reason}"
            ),
            vec![
                EscalationOption {
                    label: "Investigate the environment".into(),
                    detail: "Repeated early failure usually means the sandbox, policy or \
                             credentials are wrong rather than the card being wrong. \
                             `openshell logs` on the kept sandbox is the place to start."
                        .into(),
                },
                EscalationOption {
                    label: "Cut scope".into(),
                    detail: "Retire the card if it is not worth the environment work it is \
                             asking for."
                        .into(),
                },
            ],
            0,
        )
    }

    /// A run produced work, so the retry budget resets. Without this a card
    /// that failed twice long ago would escalate on its next single failure.
    pub fn clear_run_failures(&self, id: ItemId) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            if it.run_failures == 0 {
                return;
            }
            it.run_failures = 0;
            it.clone()
        };
        self.emit(&item);
    }

    /// Which sandbox this card is running in. Written before the agent starts,
    /// so a honr that dies mid-run can still find the sandbox on restart.
    pub fn set_environment(&self, id: ItemId, sandbox: Option<String>) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.environment = sandbox;
            it.clone()
        };
        self.emit(&item);
    }

    /// The PR an agent opened. Set before `report`, so the card arrives in
    /// Review with somewhere to go.
    pub fn set_pr_url(&self, id: ItemId, url: Option<String>) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.pr_url = url;
            it.clone()
        };
        self.emit(&item);
    }

    pub fn set_blocked_by(&self, id: ItemId, blockers: Vec<ItemId>) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.blocked_by = blockers;
            it.clone()
        };
        self.emit(&item);

        // Mirror blocks edges into beads when both sides have real hash ids.
        if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
            let deps: Vec<String> = item
                .blocked_by
                .iter()
                .filter_map(|b| self.get(*b).and_then(|w| w.beads_id))
                .filter(|b| crate::beads::BeadsClient::is_real_id(b))
                .collect();
            if crate::beads::BeadsClient::is_real_id(&bid) && !deps.is_empty() {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        for dep in deps {
                            if let Err(e) = beads.dep_add(&bid, &dep, "blocks").await {
                                tracing::warn!(%bid, %dep, error = %e, "beads dep sync failed");
                            }
                        }
                    });
                }
            }
        }
    }

    /// Tweak an item's title, intent, definition of done, or engine.
    /// Used from Shaping (pre-Ready), Ready (pre-claim), and Review (with
    /// Request changes) so humans can rewrite the contract the next agent sees.
    pub fn update_item(
        &self,
        id: ItemId,
        title: Option<String>,
        intent: Option<String>,
        definition_of_done: Option<String>,
        engine: Option<String>,
    ) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item #{id}"))?;
            if let Some(t) = title {
                if !t.trim().is_empty() {
                    it.title = t;
                }
            }
            if let Some(i) = intent {
                if !i.trim().is_empty() {
                    // Keep Plan summary aligned with Project Why.
                    if it.is_project() {
                        if let Some(plan) = it.plan.as_mut() {
                            plan.summary = i.trim().to_string();
                        }
                    }
                    it.intent = i;
                }
            }
            if let Some(d) = definition_of_done {
                it.definition_of_done = if d.trim().is_empty() { None } else { Some(d) };
            }
            if let Some(e) = engine {
                it.engine = if e.trim().is_empty() { None } else { Some(e) };
            }
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    // ------------------------------------------------------- the agent verbs

    /// A card still leased to this agent — survives a restart mid-flight. The
    /// supervisor's startup reconciliation is the next caller: honr restarts
    /// constantly while honr is what's being built, and sandboxes outlive it.
    #[allow(dead_code)]
    pub fn leased_to(&self, agent_id: &str) -> Option<ItemId> {
        let s = self.state.read().unwrap();
        s.items
            .values()
            .find(|i| {
                matches!(i.state, State::Claimed | State::Running)
                    && i.lease.as_ref().map(|l| l.agent_id == agent_id).unwrap_or(false)
            })
            .map(|i| i.id)
    }

    /// `list_ready` — the pull queue made visible. Projects are never claimable.
    pub fn list_ready(&self, capabilities: &[String]) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        s.items
            .values()
            .filter(|i| i.state == State::Ready)
            .filter(|i| i.level.as_deref() != Some("Project"))
            .filter(|i| !Self::has_children(&s, i.id))
            .filter(|i| Self::unresolved_blockers(&s, i).is_empty())
            .filter(|i| match &i.capability {
                None => true,
                Some(c) if c == "any" => true,
                Some(c) => capabilities.iter().any(|have| have == c),
            })
            .map(|i| {
                let mut item = i.clone();
                Self::populate_blockers(&s, &mut item);
                item
            })
            .collect()
    }

    /// Ready tasks from beads (`issue_type=task` only), mapped back to board items when present.
    #[allow(dead_code)]
    pub async fn list_ready_beads(&self) -> Result<Vec<crate::beads::BeadsIssue>, String> {
        if let Some(b) = &self.beads {
            let ready = b.list_ready().await?;
            Ok(ready.into_iter().filter(|i| i.issue_type == "task").collect())
        } else {
            Err("beads client not initialized".into())
        }
    }

    /// `sync_beads_remote` — pushes beads database state to GitHub remote (refs/dolt/data).
    #[allow(dead_code)]
    pub async fn sync_beads_remote(&self) -> Result<(), String> {
        if let Some(b) = &self.beads {
            b.sync_remote(Some("origin")).await
        } else {
            Err("beads client not initialized".into())
        }
    }

    /// `claim` — takes a lease and returns the full goal ancestry, not just the
    /// card.
    pub fn claim(
        &self,
        id: ItemId,
        agent_id: &str,
        model: Option<String>,
        lease_secs: i64,
    ) -> Result<ClaimGrant, TransitionError> {
        let now = Utc::now();
        let expires_at = now + Duration::seconds(lease_secs);

        let item = {
            let mut s = self.state.write().unwrap();
            Self::transition_locked(&mut s, id, State::Claimed, agent_id, None)?;
            let it = s.items.get_mut(&id).unwrap();
            it.lease = Some(Lease {
                agent_id: agent_id.to_string(),
                granted_at: now,
                last_heartbeat: now,
                expires_at,
            });
            if model.is_some() {
                it.model = model;
            }
            it.clone()
        };
        self.emit(&item);

        if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    if let Err(e) = beads.claim(&bid).await {
                        tracing::warn!(%bid, error = %e, "beads claim sync failed");
                    }
                });
            }
        }

        Ok(ClaimGrant {
            item_id: id,
            title: item.title.clone(),
            definition_of_done: item.definition_of_done.clone(),
            beads_id: item.beads_id.clone(),
            ancestry: self.ancestry(id),
            constraints: self.inherited_pins(id),
            notes: item.notes.iter().map(|n| n.text.clone()).collect(),
            lease_expires_at: expires_at,
            budget_remaining_cents: item.budget_cents.map(|b| b.saturating_sub(item.cost_cents)),
            engine: item.engine.clone(),
        })
    }

    /// `heartbeat` — carries cost, because budget enforcement lives in the
    /// control plane and not in agent good behaviour.
    pub fn heartbeat(
        &self,
        id: ItemId,
        agent_id: &str,
        progress: f32,
        cost_delta_cents: u64,
        lease_secs: i64,
    ) -> Result<WorkItem, TransitionError> {
        let item = {
            let mut s = self.state.write().unwrap();
            // First heartbeat promotes Claimed -> Running.
            if s.items.get(&id).map(|i| i.state) == Some(State::Claimed) {
                Self::transition_locked(&mut s, id, State::Running, agent_id, None)?;
            }
            let now = Utc::now();
            let it = s.items.get_mut(&id).ok_or(TransitionError::NoSuchItem(id))?;
            it.progress = progress.clamp(0.0, 1.0);
            it.cost_cents += cost_delta_cents;
            if let Some(l) = it.lease.as_mut() {
                l.last_heartbeat = now;
                l.expires_at = now + Duration::seconds(lease_secs);
            }
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

fn normalize_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn tokenize_text(text: &str) -> HashSet<String> {
    let stop_words: HashSet<&'static str> = [
        "a", "an", "the", "and", "or", "but", "if", "because", "as", "until", "while", "of", "at",
        "by", "for", "with", "about", "against", "between", "into", "through", "during", "before",
        "after", "above", "below", "to", "from", "up", "down", "in", "out", "on", "off", "over",
        "under", "again", "further", "then", "once", "here", "there", "when", "where", "why", "how",
        "all", "any", "both", "each", "few", "more", "most", "other", "some", "such", "no", "nor",
        "not", "only", "own", "same", "so", "than", "too", "very", "s", "t", "can", "will", "just",
        "don", "should", "now", "is", "are", "was", "were", "be", "been", "being", "have", "has",
        "had", "do", "does", "did", "doing", "would", "could", "this", "that", "these", "those",
        "i", "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them", "my", "your",
        "his", "their", "our", "its",
    ]
    .into_iter()
    .collect();

    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2 && !stop_words.contains(s))
        .map(|s| s.to_string())
        .collect()
}

fn is_token_related(child_token: &str, theme_token: &str) -> bool {
    if child_token == theme_token {
        return true;
    }
    let min_len = child_token.len().min(theme_token.len());
    if min_len >= 3 {
        if child_token.starts_with(theme_token) || theme_token.starts_with(child_token) {
            return true;
        }
        if min_len >= 4
            && child_token.is_char_boundary(4)
            && theme_token.is_char_boundary(4)
            && child_token[..4] == theme_token[..4]
        {
            return true;
        }
    }
    false
}

fn child_is_related(child_tokens: &HashSet<String>, theme_tokens: &HashSet<String>) -> bool {
    if theme_tokens.is_empty() {
        return true;
    }
    for c_tok in child_tokens {
        for t_tok in theme_tokens {
            if Self::is_token_related(c_tok, t_tok) {
                return true;
            }
        }
    }
    false
}

fn check_split_relatedness(
    card: &WorkItem,
    project: &WorkItem,
    children: &[(String, String, String)],
) -> Result<(), String> {
    let mut theme_text = String::new();
    theme_text.push_str(&project.title);
    theme_text.push(' ');
    theme_text.push_str(&project.intent);
    theme_text.push(' ');
    theme_text.push_str(&card.title);
    theme_text.push(' ');
    theme_text.push_str(&card.intent);
    if let Some(ref dod) = card.definition_of_done {
        theme_text.push(' ');
        theme_text.push_str(dod);
    }

    let theme_tokens = Self::tokenize_text(&theme_text);

    for (title, intent, dod) in children {
        let mut child_text = String::new();
        child_text.push_str(title);
        child_text.push(' ');
        child_text.push_str(intent);
        child_text.push(' ');
        child_text.push_str(dod);

        let child_tokens = Self::tokenize_text(&child_text);

        if !Self::child_is_related(&child_tokens, &theme_tokens) {
            return Err(format!(
                "split child '{title}' does not relate to parent card or project theme"
            ));
        }
    }

    Ok(())
}

    /// `split` — self-orchestration. Creates **sibling** Tasks under the same
    /// Project (flat model); the original card is Done, not nested into.
    ///
    /// Implementation cards: PR and split are mutually exclusive.
    /// Initial plan cards: a plan/docs PR is allowed, then split materializes
    /// the Tasks. Sibling titles already present under the Project are reused
    /// (idempotent) so a second split or a cockpit plan cannot duplicate work.
    pub fn split(
        &self,
        id: ItemId,
        agent_id: &str,
        children: Vec<(String, String, String)>, // title, intent, dod
        max_children: usize,
    ) -> Result<Vec<WorkItem>, String> {
        let card = self.get(id).ok_or("no such item")?;
        let allow_pr = card.is_initial_plan_task();

        if let Some(ref pr_url) = card.pr_url.as_ref().filter(|s| !s.trim().is_empty()) {
            if !allow_pr {
                let msg = format!(
                    "cannot split card #{id}: a PR already exists ({pr_url}); split and publish are mutually exclusive"
                );
                let _ = self.escalate(
                    id,
                    agent_id,
                    msg.clone(),
                    vec![
                        EscalationOption {
                            label: "Finish card via report".into(),
                            detail: "Complete the card with the existing PR using report.".into(),
                        },
                        EscalationOption {
                            label: "Close PR and retry split".into(),
                            detail: "Close or abandon the existing PR before splitting the card.".into(),
                        },
                    ],
                    0,
                );
                return Err(msg);
            }
        }

        if children.len() < 2 {
            return Err("a split needs at least two siblings; use report if the work is one card".into());
        }
        if children.len() > max_children {
            return Err(format!(
                "split of {} siblings exceeds max_children_per_split={max_children}; escalating \
                 rather than fanning out",
                children.len()
            ));
        }

        let project_id = card.parent.ok_or_else(|| {
            "cannot split a Project; only Tasks under a Project can split into siblings".to_string()
        })?;
        let project = self.get(project_id).ok_or_else(|| "project not found".to_string())?;

        {
            let s = self.state.read().unwrap();
            if s.items.get(&project_id).and_then(|p| p.parent).is_some() {
                return Err("split target is not under a Project root".into());
            }
        }

        Self::check_split_relatedness(&card, &project, &children)?;

        // Dedupe titles within the request (first wins).
        let mut seen_req = HashSet::new();
        let mut unique_children = Vec::new();
        for child in children {
            let key = Self::normalize_title(&child.0);
            if key.is_empty() || !seen_req.insert(key) {
                continue;
            }
            unique_children.push(child);
        }
        if unique_children.len() < 2 {
            return Err(
                "a split needs at least two distinct sibling titles; use report if the work is one card"
                    .into(),
            );
        }

        self.transition(id, State::Splitting, agent_id, Some("agent requested split".into()))
            .map_err(|e| e.to_string())?;

        // Existing non-retired siblings under the Project, keyed by normalized title.
        let existing_by_title: HashMap<String, WorkItem> = {
            let s = self.state.read().unwrap();
            s.items
                .values()
                .filter(|i| i.parent == Some(project_id))
                .filter(|i| i.id != id)
                .filter(|i| i.state != State::Retired)
                .map(|i| (Self::normalize_title(&i.title), i.clone()))
                .filter(|(k, _)| !k.is_empty())
                .collect()
        };

        let mut made = Vec::new();
        let mut created = 0usize;
        let mut reused = 0usize;
        for (title, intent, dod) in unique_children {
            let key = Self::normalize_title(&title);
            if let Some(existing) = existing_by_title.get(&key) {
                reused += 1;
                made.push(existing.clone());
                continue;
            }
            let sibling = self.create(
                Some(project_id),
                title,
                intent,
                Some(dod),
                Origin::Split { from: id },
                false,
                card.capability.clone(),
            )?;
            self.transition(sibling.id, State::Shaping, agent_id, None)
                .map_err(|e| e.to_string())?;
            let sibling = self
                .transition(sibling.id, State::Ready, agent_id, None)
                .map_err(|e| e.to_string())?;
            created += 1;
            made.push(sibling);
        }

        self.transition(
            id,
            State::Done,
            agent_id,
            Some("split into sibling tasks under the Project".into()),
        )
        .map_err(|e| e.to_string())?;

        self.story(
            project_id,
            format!(
                "{} turned out bigger than one card — {} siblings ({} created, {} reused): {}.",
                card.title,
                made.len(),
                created,
                reused,
                made.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join(", ")
            ),
        );
        Ok(made)
    }

    /// `escalate` — must carry options. An agent that hands back an open
    /// question has transferred the whole problem.
    pub fn escalate(
        &self,
        id: ItemId,
        agent_id: &str,
        question: String,
        options: Vec<EscalationOption>,
        recommended: usize,
    ) -> Result<WorkItem, String> {
        if options.len() < 2 {
            return Err(
                "escalate requires at least two concrete options and a recommendation — an \
                 open-ended question is not a decision a human can make in one tap"
                    .into(),
            );
        }
        if recommended >= options.len() {
            return Err(format!("recommended index {recommended} is out of range"));
        }

        {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.escalation = Some(Escalation {
                question: question.clone(),
                options,
                recommended,
                blocked_since: Utc::now(),
                answer: None,
            });
        }
        let item = self
            .transition(id, State::NeedsHuman, agent_id, Some(question.clone()))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{} is blocked: {question}", item.title));
        Ok(item)
    }

    /// `report` — agent says done; gates decide.
    pub fn report(
        &self,
        id: ItemId,
        agent_id: &str,
        added: u32,
        removed: u32,
        gates: Vec<String>,
    ) -> Result<WorkItem, TransitionError> {
        {
            let mut s = self.state.write().unwrap();
            if let Some(it) = s.items.get_mut(&id) {
                it.diff_added = added;
                it.diff_removed = removed;
                it.progress = 1.0;
                it.gates = gates
                    .into_iter()
                    .map(|name| GateRun { name, status: GateStatus::Pending, detail: None })
                    .collect();
            }
        }
        self.transition(id, State::Verifying, agent_id, None)
    }

    /// `release` — graceful surrender.
    pub fn release(&self, id: ItemId, agent_id: &str) -> Result<WorkItem, TransitionError> {
        self.release_with_reason(id, agent_id, None)
    }

    /// `release_with_reason` — graceful surrender with an explicit bounce reason recorded on WorkItem and transition history.
    pub fn release_with_reason(
        &self,
        id: ItemId,
        agent_id: &str,
        reason: Option<&str>,
    ) -> Result<WorkItem, TransitionError> {
        let reason_str = reason
            .map(|r| r.to_string())
            .unwrap_or_else(|| "released by agent".to_string());

        let item = {
            let mut s = self.state.write().unwrap();
            if let Some(r) = reason {
                if let Some(it) = s.items.get_mut(&id) {
                    it.last_bounce_reason = Some(r.to_string());
                }
            }
            Self::transition_locked(&mut s, id, State::Ready, agent_id, Some(reason_str))?
        };
        self.emit(&item);
        if let Some(r) = reason {
            self.story(id, format!("{}: released ({r})", item.title));
        }
        Ok(item)
    }

    // --------------------------------------------------------- the verifier

    /// Unused until the supervisor runs honr's real gates (`cargo test`,
    /// `clippy`, the web build) in the sandbox and reports the outcome.
    #[allow(dead_code)]
    pub fn settle_gates(&self, id: ItemId, passed: bool, detail: &str) -> Result<WorkItem, String> {
        let (retries_left, title) = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            for g in it.gates.iter_mut() {
                g.status = if passed { GateStatus::Passed } else { GateStatus::Failed };
                g.detail = Some(detail.to_string());
            }
            if !passed {
                it.gate_failures += 1;
            }
            (it.gate_failures < 3, it.title.clone())
        };

        if passed {
            return self.transition(id, State::Review, "verifier", None).map_err(|e| e.to_string());
        }
        if retries_left {
            let item = self
                .transition(id, State::Ready, "verifier", Some(format!("gates failed: {detail}")))
                .map_err(|e| e.to_string())?;
            self.story(id, format!("{title} failed its gates ({detail}); back in the queue."));
            Ok(item)
        } else {
            // Retry budget spent — this is now a human's problem.
            let opts = vec![
                EscalationOption {
                    label: "Investigate the gate".into(),
                    detail: "The gate may be flaky rather than the change being wrong.".into(),
                },
                EscalationOption {
                    label: "Re-route to a stronger model".into(),
                    detail: "Full re-run on a different model.".into(),
                },
            ];
            self.escalate(
                id,
                "verifier",
                format!("{title} has failed its gates three times ({detail}). How should this proceed?"),
                opts,
                0,
            )
        }
    }

    /// Dead agents need no cleanup job: the lease is what makes pull-based
    /// dispatch survivable.
    pub fn sweep_leases(&self) -> Vec<ItemId> {
        let now = Utc::now();
        let expired: Vec<ItemId> = {
            let s = self.state.read().unwrap();
            s.items
                .values()
                .filter(|i| matches!(i.state, State::Claimed | State::Running))
                .filter(|i| i.lease.as_ref().map(|l| l.is_expired(now)).unwrap_or(false))
                .map(|i| i.id)
                .collect()
        };
        for id in &expired {
            let title = self.get(*id).map(|i| i.title).unwrap_or_default();
            let _ = self.transition(*id, State::Ready, "lease-sweeper", Some("lease expired".into()));
            self.story(*id, format!("{title}: agent stopped heartbeating; lease expired, card requeued."));
        }
        expired
    }

    // ----------------------------------------------------------- human verbs

    /// Steer — free. No restart, no context loss. Without it the only way to
    /// correct a slightly-off agent is to kill it, which makes people hover.
    pub fn steer(&self, id: ItemId, text: String) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.notes.push(Note { at: Utc::now(), author: "human".into(), text });
            it.run_failures = 0;
            it.escalation = None;
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Pin — becomes standing context for this item *and all descendants*.
    pub fn pin(&self, id: ItemId, text: String) -> Result<WorkItem, String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("constraint text is empty".into());
        }
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.pinned.push(text.clone());
            it.clone()
        };
        self.emit(&item);
        self.story(id, format!("Constraint pinned on {}: {text}", item.title));
        Ok(item)
    }

    /// Remove a pin by index on this item (does not touch ancestor pins).
    pub fn unpin(&self, id: ItemId, index: usize) -> Result<WorkItem, String> {
        let (item, removed) = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            if index >= it.pinned.len() {
                return Err(format!("no constraint at index {index}"));
            }
            let removed = it.pinned.remove(index);
            (it.clone(), removed)
        };
        self.emit(&item);
        self.story(id, format!("Constraint removed on {}: {removed}", item.title));
        Ok(item)
    }

    pub fn answer_escalation(&self, id: ItemId, choice: String) -> Result<WorkItem, String> {
        let title = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            if it.escalation.is_none() {
                return Err("that item is not waiting on anyone".into());
            }
            // Clear it, don't just annotate it. An answered escalation that
            // stays attached keeps rendering as a live question — the card goes
            // on saying "blocked 15m" while the agent is happily working, which
            // is the board lying about the one thing it exists to tell you.
            // The decision is not lost: it becomes a note below, and the
            // transition history records it.
            it.escalation = None;
            // A human has looked at it, so the run budget starts over —
            // otherwise the next single failure would escalate again
            // immediately and the answer would have bought nothing.
            it.run_failures = 0;
            // The answer becomes standing context for whoever picks it up next.
            it.notes.push(Note {
                at: Utc::now(),
                author: "human".into(),
                text: format!("Decision: {choice}"),
            });
            it.title.clone()
        };
        let item = self
            .transition(id, State::Ready, "human", Some(format!("answered: {choice}")))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{title}: unblocked — {choice}"));
        Ok(item)
    }

    /// Halt — kill the agent, return the card to Ready. Loses in-flight work.
    pub fn halt(&self, id: ItemId, reason: Option<String>) -> Result<WorkItem, String> {
        self.transition(id, State::Ready, "human", reason.or(Some("halted".into())))
            .map_err(|e| e.to_string())
    }

    /// Cut scope — the subtree is retired, not deleted. Archived Projects drop
    /// off the board/digest; items remain in state because "we chose not to"
    /// is a fact you will need later.
    pub fn cut_scope(&self, id: ItemId, reason: Option<String>) -> Result<Vec<ItemId>, String> {
        let mut stack = vec![id];
        let mut touched = Vec::new();
        while let Some(cur) = stack.pop() {
            stack.extend(self.children_of(cur));
            if self
                .transition(cur, State::Retired, "human", reason.clone())
                .is_ok()
            {
                touched.push(cur);
            }
        }
        if let Some(t) = self.get(id) {
            self.story(id, format!("Scope cut: {} retired ({} items).", t.title, touched.len()));
        }
        Ok(touched)
    }

    /// Delete item — removes the item (and its subtree) permanently from the board.
    pub fn delete_item(&self, id: ItemId) -> Result<(), String> {
        let mut s = self.state.write().unwrap();
        if !s.items.contains_key(&id) {
            return Err(format!("item #{id} not found"));
        }

        let mut to_delete = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            to_delete.push(cur);
            for (cid, item) in &s.items {
                if item.parent == Some(cur) && !to_delete.contains(cid) && !stack.contains(cid) {
                    stack.push(*cid);
                }
            }
        }

        for del_id in &to_delete {
            if let Some(it) = s.items.remove(del_id) {
                let beads = self.beads.clone();
                let beads_id = it.beads_id.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let (Some(b), Some(bid)) = (beads, beads_id) {
                            if crate::beads::BeadsClient::is_real_id(&bid) {
                                let _ = b.close(&bid, Some("Deleted from honr board")).await;
                                let _ = b.github_push(&[bid]).await;
                                b.schedule_dolt_push();
                            }
                        }
                    });
                }
            }
        }

        for item in s.items.values_mut() {
            item.blocked_by.retain(|b| !to_delete.contains(b));
        }

        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        self.flush();

        let _ = self.tx.send(BoardEvent::Delete {
            seq: self.next_seq(),
            id,
        });
        Ok(())
    }

    pub fn approve_review(&self, id: ItemId) -> Result<WorkItem, String> {
        let item = self
            .transition(id, State::Done, "human", Some("approved".into()))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{} approved and merged.", item.title));
        Ok(item)
    }

    pub fn request_changes(&self, id: ItemId, note: String) -> Result<WorkItem, String> {
        self.steer(id, format!("Changes requested: {note}"))?;
        let item = self
            .transition(id, State::Ready, "human", Some(format!("changes requested: {note}")))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{}: changes requested — {note}", item.title));
        Ok(item)
    }

    // ------------------------------------------------------------ narrative

    pub fn story(&self, near: ItemId, text: String) {
        let (goal, line) = {
            let mut s = self.state.write().unwrap();
            let goal = Self::goal_of(&s, near);
            let line = StoryLine { at: Utc::now(), text };
            let entries = s.stories.entry(goal).or_default();
            entries.push(line.clone());
            // A story, not an event log.
            if entries.len() > 200 {
                let excess = entries.len() - 200;
                entries.drain(0..excess);
            }
            (goal, line)
        };
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.tx.send(BoardEvent::Story {
            seq: self.next_seq(),
            goal,
            at: line.at.to_rfc3339(),
            text: line.text,
        });
    }

    // -------------------------------------------------------- derived reads

    pub fn snapshot(&self) -> Snapshot {
        let s = self.state.read().unwrap();
        let now = Utc::now();
        let items: Vec<WorkItem> = s
            .items
            .values()
            .map(|i| {
                let mut item = i.clone();
                Self::populate_blockers(&s, &mut item);
                item
            })
            .collect();

        let mut goal_ids: Vec<ItemId> =
            items.iter().map(|i| Self::goal_of(&s, i.id)).collect();
        goal_ids.sort_unstable();
        goal_ids.dedup();

        let goals = goal_ids
            .into_iter()
            .filter_map(|gid| self.goal_view(&s, gid, now))
            .collect();

        Snapshot {
            items,
            levels: self.schema.levels.clone(),
            goals,
            server_time: now,
            heartbeat_expect_secs: self.schema.execution.heartbeat_expect_secs,
            seq: self.seq.load(Ordering::Relaxed),
            dispatch_paused: s.dispatch_paused,
            default_engine: self.schema.execution.agents.engine.clone(),
            default_model: self.schema.execution.agents.vertex.model.clone(),
        }
    }

    fn goal_view(&self, s: &BoardState, gid: ItemId, now: DateTime<Utc>) -> Option<GoalView> {
        let goal = s.items.get(&gid)?;

        // Only Project roots are swimlanes. Nested nodes never get their own.
        if Self::depth(s, gid) != 0 {
            return None;
        }
        // Archived / cut Projects stay in state for history but leave the board.
        if goal.state == State::Retired {
            return None;
        }

        // Tasks under this Project only — the Project itself is never a Board card.
        let members: Vec<&WorkItem> = s.items.values().filter(|i| i.parent == Some(gid)).collect();

        let leaves: Vec<&&WorkItem> =
            members.iter().filter(|i| !Self::has_children(s, i.id)).collect();
        let leaves_total = leaves.len();
        let leaves_done = leaves.iter().filter(|i| i.state == State::Done).count();

        let spend_cents = members.iter().map(|i| i.cost_cents).sum();
        let agents_live = members
            .iter()
            .filter(|i| matches!(i.state, State::Claimed | State::Running | State::Splitting))
            .count();
        let needs_you = members.iter().filter(|i| i.state == State::NeedsHuman).count();

        let mut columns = Vec::new();
        for column in [
            Column::Ready,
            Column::Running,
            Column::NeedsYou,
            Column::Verify,
            Column::Review,
            Column::Done,
        ] {
            let in_col: Vec<&&WorkItem> =
                members.iter().filter(|i| i.state.column() == column).collect();
            columns.push(ColumnView {
                column,
                summary: Self::chunk(column, &in_col, s, now, self.schema.execution.heartbeat_expect_secs),
            });
        }

        let plan_status = goal
            .plan
            .as_ref()
            .map(|p| p.status_label())
            .unwrap_or_else(|| "no_plan".into());

        Some(GoalView {
            id: gid,
            title: goal.title.clone(),
            intent: goal.intent.clone(),
            progress: if leaves_total == 0 { 0.0 } else { leaves_done as f32 / leaves_total as f32 },
            leaves_done,
            leaves_total,
            spend_cents,
            budget_cents: goal.budget_cents,
            agents_live,
            needs_you,
            plan_status,
            dispatch_paused: goal.dispatch_paused,
            columns,
            story: s.stories.get(&gid).cloned().unwrap_or_default(),
        })
    }

    /// Compression makes less stuff; chunking makes stuff that holds. Every
    /// rollup here has to answer the column's question, not just count it.
    fn chunk(
        column: Column,
        items: &[&&WorkItem],
        s: &BoardState,
        now: DateTime<Utc>,
        hb_expect: i64,
    ) -> ChunkSummary {
        let count = items.len();
        if count == 0 {
            return ChunkSummary { count, text: "empty".into() };
        }
        let oldest = items
            .iter()
            .map(|i| i.time_in_state(now))
            .max()
            .unwrap_or_else(Duration::zero);

        let text = match column {
            Column::Ready => {
                // Is this actually ready?
                let blocked: Vec<&&&WorkItem> = items
                    .iter()
                    .filter(|i| !Self::unresolved_blockers(s, i).is_empty())
                    .collect();
                let mut parts = vec![format!("{count} ready")];
                if !blocked.is_empty() {
                    let on: Vec<String> = blocked
                        .iter()
                        .flat_map(|i| Self::unresolved_blockers(s, i))
                        .map(|b| format!("#{b}"))
                        .collect();
                    parts.push(format!("{} blocked on {}", blocked.len(), on.join(", ")));
                }
                parts.push(format!("oldest {}", humanize(oldest)));
                parts.join(" · ")
            }
            Column::Running => {
                // Is it alive, and is it worth it?
                let stalled = items
                    .iter()
                    .filter(|i| {
                        i.lease
                            .as_ref()
                            .map(|l| l.heartbeat_age_secs(now) > hb_expect)
                            .unwrap_or(true)
                    })
                    .count();
                let spend: u64 = items.iter().map(|i| i.cost_cents).sum();
                if stalled == 0 {
                    format!("{count} running · all healthy · ${:.2} so far", spend as f64 / 100.0)
                } else {
                    format!(
                        "{count} running · {stalled} stalled · ${:.2} so far",
                        spend as f64 / 100.0
                    )
                }
            }
            Column::NeedsYou => {
                // How fast must I act?
                let longest = items
                    .iter()
                    .filter_map(|i| i.escalation.as_ref())
                    .map(|e| e.blocked_secs(now))
                    .max()
                    .unwrap_or(0);
                format!("{count} blocked on you · longest {}", humanize(Duration::seconds(longest)))
            }
            Column::Verify => {
                // Will it pass?
                let retried = items.iter().filter(|i| i.gate_failures > 0).count();
                if retried == 0 {
                    format!("{count} in gates · none retried · oldest {}", humanize(oldest))
                } else {
                    format!("{count} in gates · {retried} previously failed")
                }
            }
            Column::Review => {
                // Can I approve this in 30 seconds?
                let added: u32 = items.iter().map(|i| i.diff_added).sum();
                let removed: u32 = items.iter().map(|i| i.diff_removed).sum();
                format!("{count} awaiting review · +{added} −{removed} · oldest {}", humanize(oldest))
            }
            Column::Done => format!("{count} merged"),
            _ => format!("{count} items"),
        };
        ChunkSummary { count, text }
    }

    /// The primary interface for most sessions. Two taps to resolve both
    /// blockers from a phone is the actual product; the board is where you go
    /// when something has gone wrong.
    pub fn digest(&self) -> Digest {
        let s = self.state.read().unwrap();
        let now = Utc::now();
        let mut goal_ids: Vec<ItemId> = s.items.values().map(|i| Self::goal_of(&s, i.id)).collect();
        goal_ids.sort_unstable();
        goal_ids.dedup();

        let goals = goal_ids
            .into_iter()
            .filter_map(|gid| {
                let goal = s.items.get(&gid)?;
                // Only Project roots are digest lanes.
                if Self::depth(&s, gid) != 0 {
                    return None;
                }
                if goal.state == State::Retired {
                    return None;
                }
                let members: Vec<&WorkItem> =
                    s.items.values().filter(|i| Self::goal_of(&s, i.id) == gid).collect();

                let needs_you = members
                    .iter()
                    .filter(|i| i.state == State::NeedsHuman)
                    .filter_map(|i| {
                        i.escalation.as_ref().map(|e| NeedsYou {
                            id: i.id,
                            title: i.title.clone(),
                            question: e.question.clone(),
                            options: e.options.iter().map(|o| o.label.clone()).collect(),
                            recommended: e.recommended,
                            blocked_secs: e.blocked_secs(now),
                        })
                    })
                    .collect();

                let running: Vec<&&WorkItem> = members
                    .iter()
                    .filter(|i| matches!(i.state, State::Claimed | State::Running | State::Splitting))
                    .collect();
                let hb = self.schema.execution.heartbeat_expect_secs;

                Some(GoalDigest {
                    goal_id: gid,
                    goal: goal.title.clone(),
                    merged: members.iter().filter(|i| i.state == State::Done).count(),
                    spend_cents: members.iter().map(|i| i.cost_cents).sum(),
                    budget_cents: goal.budget_cents,
                    needs_you,
                    running: running.len(),
                    running_stalled: running
                        .iter()
                        .filter(|i| {
                            i.lease
                                .as_ref()
                                .map(|l| l.heartbeat_age_secs(now) > hb)
                                .unwrap_or(true)
                        })
                        .count(),
                    ready: members.iter().filter(|i| i.state == State::Ready).count(),
                    in_review: members.iter().filter(|i| i.state == State::Review).count(),
                    latest_story: s.stories.get(&gid).and_then(|v| v.last()).map(|l| l.text.clone()),
                })
            })
            .collect();

        Digest { since: self.started_at, goals }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Origin;

    /// A board with one leaf sitting in Ready, claimed by `agent`.
    fn claimed_leaf() -> (Board, ItemId) {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-nowrite.json"));
        let parent = b
            .create(None, "goal", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(parent.id, State::Shaping, "t", None);
        let leaf = b
            .create(
                Some(parent.id),
                "leaf",
                "do a thing",
                Some("it is done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task");
        let _ = b.transition(leaf.id, State::Shaping, "t", None);
        let _ = b.transition(leaf.id, State::Ready, "t", None);
        b.claim(leaf.id, "agent", None, 45).expect("claim");
        (b, leaf.id)
    }

    /// Failures under the cap requeue, so a transient problem self-heals.
    #[test]
    fn early_failures_requeue_while_budget_remains() {
        let (b, id) = claimed_leaf();
        let it = b.record_run_failure(id, "sandbox would not start", 3).expect("recorded");
        assert_eq!(it.state, State::Ready);
        assert_eq!(it.run_failures, 1);
    }

    /// The whole point: without this a card that fails early spends nothing,
    /// so no money cap stops it looping every lease period, forever.
    #[test]
    fn repeated_failures_become_a_humans_problem() {
        let (b, id) = claimed_leaf();
        for _ in 0..2 {
            b.record_run_failure(id, "clone refused", 3).expect("requeued");
            b.claim(id, "agent", None, 45).expect("reclaim");
        }
        let it = b.record_run_failure(id, "clone refused", 3).expect("escalated");
        assert_eq!(it.state, State::NeedsHuman);
        assert_eq!(it.run_failures, 3);

        // An escalation a human cannot act on in one tap is not a decision.
        let esc = it.escalation.expect("escalation present");
        assert!(esc.options.len() >= 2, "needs at least two concrete options");
        assert!(esc.question.contains("clone refused"), "must say what went wrong");
    }

    /// A run that dies before its first heartbeat is still Claimed, and
    /// Claimed -> NeedsHuman is not a legal edge. Escalating must still work.
    #[test]
    fn escalation_works_from_claimed_without_a_heartbeat() {
        let (b, id) = claimed_leaf();
        assert_eq!(b.get(id).unwrap().state, State::Claimed, "no heartbeat yet");
        let it = b.record_run_failure(id, "died instantly", 1).expect("escalated");
        assert_eq!(it.state, State::NeedsHuman);
    }

    #[test]
    fn success_resets_the_retry_budget() {
        let (b, id) = claimed_leaf();
        b.record_run_failure(id, "flake", 3).expect("requeued");
        b.clear_run_failures(id);
        assert_eq!(b.get(id).unwrap().run_failures, 0);
    }

    /// Answering must buy a fresh budget, or the next single failure
    /// re-escalates immediately and the human's answer bought nothing.
    #[test]
    fn answering_an_escalation_resets_the_retry_budget() {
        let (b, id) = claimed_leaf();
        b.record_run_failure(id, "boom", 1).expect("escalated");
        assert_eq!(b.get(id).unwrap().run_failures, 1);
        b.answer_escalation(id, "Investigate the environment".into()).expect("answered");
        let it = b.get(id).unwrap();
        assert_eq!(it.state, State::Ready);
        assert_eq!(it.run_failures, 0);
    }

    /// An answered escalation must not stay attached. Leaving it there made a
    /// running card keep reporting "blocked 15m" against a question that had
    /// already been resolved — the board contradicting itself about the one
    /// thing it exists to tell you.
    #[test]
    fn answering_clears_the_escalation_and_keeps_the_decision() {
        let (b, id) = claimed_leaf();
        b.record_run_failure(id, "boom", 1).expect("escalated");
        assert!(b.get(id).unwrap().escalation.is_some());

        b.answer_escalation(id, "Investigate the environment".into()).expect("answered");
        let it = b.get(id).unwrap();
        assert!(it.escalation.is_none(), "resolved escalation must not linger");

        // The decision survives as standing context for whoever picks it up.
        assert!(
            it.notes.iter().any(|n| n.text.contains("Investigate the environment")),
            "the decision must be preserved as a note: {:?}",
            it.notes
        );
    }

    #[test]
    fn answering_nothing_is_refused() {
        let (b, id) = claimed_leaf();
        assert!(b.answer_escalation(id, "whatever".into()).is_err());
    }

    #[test]
    fn delete_item_removes_item_and_descendants() {
        let b = std::sync::Arc::new(Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-del.json"),
        ));
        let p = b
            .create(None, "Parent", "intent", None, Origin::Human, false, None)
            .expect("project");
        let c = b
            .create(Some(p.id), "Child", "intent", None, Origin::Human, false, None)
            .expect("task");
        assert!(b.get(p.id).is_some());
        assert!(b.get(c.id).is_some());

        b.delete_item(p.id).expect("delete parent");
        assert!(b.get(p.id).is_none());
        assert!(b.get(c.id).is_none());
    }

    #[test]
    fn split_creates_siblings_under_project() {
        let (b, id) = claimed_leaf();
        let project_id = b.get(id).unwrap().parent.expect("task under project");
        let _ = b.transition(id, State::Running, "agent", None);
        let children = vec![
            ("Leaf part 1".into(), "Do leaf part 1".into(), "Leaf part 1 done".into()),
            ("Leaf part 2".into(), "Do leaf part 2".into(), "Leaf part 2 done".into()),
        ];
        let made = b.split(id, "agent", children, 5).expect("split should succeed");
        assert_eq!(made.len(), 2);
        assert_eq!(made[0].parent, Some(project_id));
        assert_eq!(made[0].state, State::Ready);
        assert_eq!(made[1].parent, Some(project_id));
        assert_eq!(made[1].state, State::Ready);

        let original = b.get(id).expect("original exists");
        assert_eq!(original.state, State::Done);
    }

    #[test]
    fn split_accepts_on_theme_children() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-split-theme-accept.json"));
        let project = b
            .create(None, "User Authentication System", "Manage user logins and tokens", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let task = b
            .create(
                Some(project.id),
                "Implement OAuth2 login flow",
                "Support Google and GitHub auth",
                Some("OAuth login working".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task");
        let _ = b.transition(task.id, State::Shaping, "t", None);
        let _ = b.transition(task.id, State::Ready, "t", None);
        let _ = b.claim(task.id, "agent", None, 60).expect("claim");
        let _ = b.transition(task.id, State::Running, "agent", None);

        let children = vec![
            ("Google OAuth login endpoint".into(), "Add endpoint for google auth callback".into(), "Google auth done".into()),
            ("GitHub OAuth token exchange".into(), "Exchange code for github access token".into(), "GitHub auth done".into()),
        ];

        let made = b.split(task.id, "agent", children, 5).expect("on-theme split should succeed");
        assert_eq!(made.len(), 2);
        assert_eq!(b.get(task.id).unwrap().state, State::Done);
    }

    #[test]
    fn split_rejects_off_theme_children() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-split-theme-reject.json"));
        let project = b
            .create(None, "User Authentication System", "Manage user logins and tokens", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let task = b
            .create(
                Some(project.id),
                "Implement OAuth2 login flow",
                "Support Google and GitHub auth",
                Some("OAuth login working".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task");
        let _ = b.transition(task.id, State::Shaping, "t", None);
        let _ = b.transition(task.id, State::Ready, "t", None);
        let _ = b.claim(task.id, "agent", None, 60).expect("claim");
        let _ = b.transition(task.id, State::Running, "agent", None);

        let children = vec![
            ("Google OAuth login endpoint".into(), "Add endpoint for google auth callback".into(), "Google auth done".into()),
            ("Database connection pool".into(), "Optimize postgres max connection limit".into(), "DB config done".into()),
        ];

        let err = b.split(task.id, "agent", children, 5).unwrap_err();
        assert!(err.contains("does not relate to parent card or project theme"), "got error: {err}");
        assert_ne!(b.get(task.id).unwrap().state, State::Done);
    }

    #[test]
    fn split_refused_below_minimum_siblings() {
        let (b, id) = claimed_leaf();
        let _ = b.transition(id, State::Running, "agent", None);
        let children = vec![("Single".into(), "Only one".into(), "Done".into())];
        let err = b.split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("at least two siblings"), "got error: {err}");
    }

    #[test]
    fn split_refused_exceeding_fanout_governor() {
        let (b, id) = claimed_leaf();
        let _ = b.transition(id, State::Running, "agent", None);
        let children: Vec<_> = (1..=6)
            .map(|i| (format!("Child {i}"), format!("Intent {i}"), format!("DoD {i}")))
            .collect();
        let err = b.split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("exceeds max_children_per_split=5"), "got error: {err}");
    }

    #[test]
    fn split_refused_on_project_root() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-split-root.json"));
        let project = b
            .create(None, "proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        // Put project in a claimable path only to exercise the guard — claim a fake run.
        // Projects aren't Ready-claimable; call split directly after forcing Splitting-capable state.
        let err = b
            .split(
                project.id,
                "agent",
                vec![
                    ("A".into(), "a".into(), "done".into()),
                    ("B".into(), "b".into(), "done".into()),
                ],
                5,
            )
            .unwrap_err();
        assert!(err.contains("cannot split a Project"), "got error: {err}");
    }

    #[test]
    fn split_refused_when_pr_exists() {
        let (b, id) = claimed_leaf();
        b.set_pr_url(id, Some("https://github.com/shanemcd/honr/pull/42".to_string()));
        let children = vec![
            ("Part 1".into(), "Do part 1".into(), "Part 1 done".into()),
            ("Part 2".into(), "Do part 2".into(), "Part 2 done".into()),
        ];
        let err = b.split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("a PR already exists"), "got error: {err}");

        let item = b.get(id).expect("item exists");
        assert_eq!(item.state, State::NeedsHuman);
        assert!(item.escalation.is_some(), "escalation must be populated");
        let esc = item.escalation.unwrap();
        assert!(esc.question.contains("a PR already exists"));
        assert_eq!(esc.options.len(), 2);
    }

    #[test]
    fn initial_plan_may_split_after_pr() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-initial-plan-pr-split.json"),
        );
        let project = b
            .create(
                None,
                "Archive UI",
                "Archive completed work from the board",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("project");
        let seed_id = b
            .children_of(project.id)
            .into_iter()
            .find(|&id| b.get(id).unwrap().is_initial_plan_task())
            .expect("initial plan");
        b.set_pr_url(seed_id, Some("https://github.com/shanemcd/honr/pull/41".into()));
        let _ = b.claim(seed_id, "agent", None, 60).expect("claim initial plan");
        let _ = b.transition(seed_id, State::Running, "agent", None);

        let made = b
            .split(
                seed_id,
                "agent",
                vec![
                    (
                        "API archive endpoint".into(),
                        "Expose archive for board cards".into(),
                        "Archive API works".into(),
                    ),
                    (
                        "UI archive controls".into(),
                        "Add archive actions in the board UI".into(),
                        "Archive UI works".into(),
                    ),
                ],
                5,
            )
            .expect("Initial plan must split even with a PR");
        assert_eq!(made.len(), 2);
        assert_eq!(b.get(seed_id).unwrap().state, State::Done);
    }

    #[test]
    fn split_reuses_existing_siblings_by_title() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-split-idempotent.json"),
        );
        let project = b
            .create(
                None,
                "Archive UI",
                "Archive completed work from the board",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("project");
        let seed_id = b
            .children_of(project.id)
            .into_iter()
            .find(|&id| b.get(id).unwrap().is_initial_plan_task())
            .expect("initial plan");

        let preexisting = b
            .create(
                Some(project.id),
                "API archive endpoint",
                "already shaped",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("preexisting");
        let _ = b.transition(preexisting.id, State::Shaping, "t", None);
        let preexisting = b
            .transition(preexisting.id, State::Ready, "t", None)
            .expect("ready");

        let before = b.children_of(project.id).len();
        let _ = b.claim(seed_id, "agent", None, 60).expect("claim");
        let _ = b.transition(seed_id, State::Running, "agent", None);
        let made = b
            .split(
                seed_id,
                "agent",
                vec![
                    (
                        "API archive endpoint".into(),
                        "Expose archive for board cards".into(),
                        "Archive API works".into(),
                    ),
                    (
                        "UI archive controls".into(),
                        "Add archive actions in the board UI".into(),
                        "Archive UI works".into(),
                    ),
                ],
                5,
            )
            .expect("split");
        assert_eq!(made.len(), 2);
        assert_eq!(made[0].id, preexisting.id, "matching title must be reused");
        assert_eq!(
            b.children_of(project.id).len(),
            before + 1,
            "only the missing sibling should be created"
        );
    }

    #[test]
    fn nest_under_task_is_refused() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-nest.json"));
        let project = b
            .create(None, "proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let task = b
            .create(Some(project.id), "task", "do", Some("done".into()), Origin::Human, false, None)
            .expect("task");
        let err = b
            .create(Some(task.id), "nested", "no", None, Origin::Human, false, None)
            .unwrap_err();
        assert!(err.contains("flat under a Project"), "got error: {err}");
    }

    #[test]
    fn pin_and_unpin_round_trip() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-unpin.json"));
        let project = b
            .create(None, "P", "why", None, Origin::Human, true, None)
            .expect("project");
        b.pin(project.id, "gates offline".into()).expect("pin");
        b.pin(project.id, "human merges".into()).expect("pin");
        assert_eq!(b.get(project.id).unwrap().pinned.len(), 2);
        b.unpin(project.id, 0).expect("unpin");
        let pins = b.get(project.id).unwrap().pinned;
        assert_eq!(pins, vec!["human merges".to_string()]);
    }

    #[test]
    fn project_create_seeds_initial_plan_task() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-seed-plan.json"));
        let project = b
            .create(None, "Phase X", "why", None, Origin::Human, true, None)
            .expect("project");
        assert!(project.plan.is_some());
        assert_eq!(project.plan.as_ref().unwrap().status, PlanStatus::Empty);

        let kids = b.children_of(project.id);
        assert_eq!(kids.len(), 1, "exactly one seed Task");
        let seed = b.get(kids[0]).unwrap();
        assert_eq!(seed.title, INITIAL_PLAN_TITLE);
        assert_eq!(seed.state, State::Ready, "Initial plan is dispatchable planning work");
        assert!(seed.is_initial_plan_task());
        assert!(b.may_claim(seed.id));
        assert!(
            b.list_ready(&["any".into()]).iter().any(|i| i.id == seed.id),
            "Initial plan must appear in list_ready"
        );
        // Project itself must not be Ready / claimable.
        assert_ne!(b.get(project.id).unwrap().state, State::Ready);
    }

    #[test]
    fn approve_plan_materializes_from_artifact_not_project() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-approve-plan.json"));
        let project = b
            .create(None, "Phase Y", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);

        b.propose_plan(
            project.id,
            "first cut",
            vec![
                PlanTaskSpec {
                    key: "a".into(),
                    title: "Task A".into(),
                    intent: "do a".into(),
                    definition_of_done: "a done".into(),
                    blocked_by_keys: vec![],
                    capability: None,
                    item_id: None,
                },
                PlanTaskSpec {
                    key: "b".into(),
                    title: "Task B".into(),
                    intent: "do b".into(),
                    definition_of_done: "b done".into(),
                    blocked_by_keys: vec!["a".into()],
                    capability: None,
                    item_id: None,
                },
            ],
            vec![],
        )
        .expect("propose");

        let published = b.approve_plan(project.id).expect("approve");
        assert_eq!(published.len(), 2);
        assert_ne!(b.get(project.id).unwrap().state, State::Ready);
        assert_eq!(
            b.get(project.id).unwrap().plan.as_ref().unwrap().status,
            PlanStatus::Approved
        );

        let a = b.get(published[0]).unwrap();
        let b_item = b.get(published[1]).unwrap();
        assert_eq!(a.state, State::Ready);
        assert_eq!(b_item.state, State::Ready);
        assert_eq!(b_item.blocked_by, vec![published[0]]);

        // Initial plan Task closed.
        let seed = b
            .children_of(project.id)
            .into_iter()
            .find_map(|id| b.get(id).filter(|i| i.is_initial_plan_task()));
        assert!(seed.unwrap().state.is_terminal());
    }

    #[test]
    fn dispatch_paused_defaults_false_and_toggles() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-pause-toggle.json"));
        assert!(!b.dispatch_paused());
        assert!(!b.snapshot().dispatch_paused);

        b.set_dispatch_paused(true);
        assert!(b.dispatch_paused());
        assert!(b.snapshot().dispatch_paused);

        b.set_dispatch_paused(false);
        assert!(!b.dispatch_paused());
        assert!(!b.snapshot().dispatch_paused);
    }

    #[test]
    fn dispatch_paused_persists_across_load() {
        let path = std::env::temp_dir().join(format!(
            "honr-pause-persist-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let b = Board::new(Schema::default(), path.clone());
        b.set_dispatch_paused(true);
        b.flush();

        let restored = Board::load_or_new(Schema::default(), path.clone());
        assert!(restored.dispatch_paused());
        assert!(restored.snapshot().dispatch_paused);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn old_board_json_without_pause_field_loads_unpaused() {
        let path = std::env::temp_dir().join(format!(
            "honr-pause-legacy-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"next_id":1,"items":{},"stories":{}}"#,
        )
        .expect("write");
        let b = Board::load_or_new(Schema::default(), path.clone());
        assert!(!b.dispatch_paused());
        let _ = std::fs::remove_file(&path);
    }

    /// Project + Ready task under it. Returns (board, project_id, task_id).
    fn project_with_ready_task() -> (Board, ItemId, ItemId) {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-proj-pause-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let task = b
            .create(
                Some(project.id),
                "task",
                "do it",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task");
        let _ = b.transition(task.id, State::Shaping, "t", None);
        let _ = b.transition(task.id, State::Ready, "t", None);
        (b, project.id, task.id)
    }

    #[test]
    fn project_pause_blocks_only_that_subtree() {
        let (b, project_a, task_a) = project_with_ready_task();
        let project_b = b
            .create(None, "other", "why", None, Origin::Human, true, None)
            .expect("project b");
        let _ = b.transition(project_b.id, State::Shaping, "t", None);
        let task_b = b
            .create(
                Some(project_b.id),
                "task b",
                "do it",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task b");
        let task_b = task_b.id;
        let _ = b.transition(task_b, State::Shaping, "t", None);
        let _ = b.transition(task_b, State::Ready, "t", None);

        assert!(b.may_claim(task_a));
        assert!(b.may_claim(task_b));

        b.set_project_dispatch_paused(project_a, true).expect("pause a");
        assert!(!b.may_claim(task_a));
        assert!(b.may_claim(task_b), "sibling project still claimable");
        assert!(
            b.snapshot()
                .goals
                .iter()
                .find(|g| g.id == project_a)
                .expect("goal a")
                .dispatch_paused
        );

        b.set_project_dispatch_paused(project_a, false).expect("resume a");
        assert!(b.may_claim(task_a));
    }

    #[test]
    fn global_pause_stamps_projects_and_allows_exceptions() {
        let (b, project_a, task_a) = project_with_ready_task();
        let project_b = b
            .create(None, "other", "why", None, Origin::Human, true, None)
            .expect("project b");
        let _ = b.transition(project_b.id, State::Shaping, "t", None);
        let task_b = b
            .create(
                Some(project_b.id),
                "task b",
                "do it",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task b");
        let task_b = task_b.id;
        let _ = b.transition(task_b, State::Shaping, "t", None);
        let _ = b.transition(task_b, State::Ready, "t", None);

        b.set_dispatch_paused(true);
        assert!(b.get(project_a).unwrap().dispatch_paused);
        assert!(b.get(project_b.id).unwrap().dispatch_paused);
        assert!(!b.may_claim(task_a));
        assert!(!b.may_claim(task_b));

        // Exception: resume one project while global stays paused.
        b.set_project_dispatch_paused(project_a, false).expect("resume a");
        assert!(b.dispatch_paused());
        assert!(b.may_claim(task_a), "resumed project is an exception");
        assert!(!b.may_claim(task_b), "other projects stay stamped");

        // Global resume clears every project pause.
        b.set_dispatch_paused(false);
        assert!(!b.get(project_a).unwrap().dispatch_paused);
        assert!(!b.get(project_b.id).unwrap().dispatch_paused);
        assert!(b.may_claim(task_a));
        assert!(b.may_claim(task_b));
    }

    #[test]
    fn new_project_while_globally_paused_starts_paused() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-pause-new-{}.json", std::process::id())),
        );
        b.set_dispatch_paused(true);
        let project = b
            .create(None, "late", "why", None, Origin::Human, true, None)
            .expect("project");
        assert!(project.dispatch_paused);
    }

    #[test]
    fn cannot_project_pause_a_task() {
        let (b, _project, task) = project_with_ready_task();
        assert!(b.set_project_dispatch_paused(task, true).is_err());
    }

    #[test]
    fn project_pause_persists_across_load() {
        let path = std::env::temp_dir().join(format!(
            "honr-proj-pause-persist-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let b = Board::new(Schema::default(), path.clone());
        let project = b
            .create(None, "proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        b.set_project_dispatch_paused(project.id, true).expect("pause");
        b.flush();

        let restored = Board::load_or_new(Schema::default(), path.clone());
        assert!(restored.get(project.id).unwrap().dispatch_paused);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn release_with_reason_records_bounce_reason_and_history() {
        let (b, _project, task_id) = project_with_ready_task();
        let agent_id = "agent-infra-test";

        // Claim task
        let _grant = b.claim(task_id, agent_id, None, 300).expect("claim task");
        let claimed = b.get(task_id).expect("claimed item");
        assert_eq!(claimed.state, State::Claimed);
        assert_eq!(claimed.last_bounce_reason, None);

        // Release with reason
        let bounce_msg = "infra failure: podman socket connection refused";
        let released = b
            .release_with_reason(task_id, agent_id, Some(bounce_msg))
            .expect("release with reason");

        assert_eq!(released.state, State::Ready);
        assert_eq!(
            released.last_bounce_reason.as_deref(),
            Some(bounce_msg)
        );

        // Verify transition history
        let last_transition = released
            .history
            .last()
            .expect("has transition history");
        assert_eq!(last_transition.from, State::Claimed);
        assert_eq!(last_transition.to, State::Ready);
        assert_eq!(last_transition.by, agent_id);
        assert_eq!(
            last_transition.reason.as_deref(),
            Some(bounce_msg)
        );

        // Verify state store persistence/get
        let fetched = b.get(task_id).expect("fetched item");
        assert_eq!(
            fetched.last_bounce_reason.as_deref(),
            Some(bounce_msg)
        );
    }

    #[test]
    fn archived_project_omitted_from_snapshot_and_digest() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-archive-hide.json"),
        );
        let keep = b
            .create(None, "Keep me", "why", None, Origin::Human, true, None)
            .expect("keep");
        let archive = b
            .create(None, "Archive me", "why", None, Origin::Human, true, None)
            .expect("archive");
        let _ = b.transition(keep.id, State::Shaping, "t", None);
        let _ = b.transition(archive.id, State::Shaping, "t", None);

        assert!(b.snapshot().goals.iter().any(|g| g.id == archive.id));
        assert!(b.digest().goals.iter().any(|g| g.goal_id == archive.id));

        b.cut_scope(archive.id, Some("archived".into()))
            .expect("cut");
        assert_eq!(b.get(archive.id).unwrap().state, State::Retired);
        assert!(
            b.snapshot().goals.iter().all(|g| g.id != archive.id),
            "retired Project must not appear in snapshot goals"
        );
        assert!(
            b.digest().goals.iter().all(|g| g.goal_id != archive.id),
            "retired Project must not appear in digest"
        );
        assert!(
            b.snapshot().goals.iter().any(|g| g.id == keep.id),
            "active Project still listed"
        );
    }

    #[tokio::test]
    async fn test_schedule_beads_mirror_invokes_github_push_on_create_and_split() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-store-beads-mirror-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client);
        let board = Arc::new(board_raw);

        // 1. Create a project and verify placeholder before mirror
        let project = board
            .create(None, "Test Mirror Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        assert_eq!(project.beads_id, Some("bd-honr-1".to_string()));
        assert!(
            !crate::beads::BeadsClient::is_real_id(
                project.beads_id.as_deref().unwrap_or("")
            ),
            "placeholder bd-honr-* ids must skip create/sync"
        );

        // Schedule beads mirror on create
        board.schedule_beads_mirror(project.id);

        // Wait for spawned async task in schedule_beads_mirror to assign real beads_id
        let mut real_project_id = None;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(p) = board.get(project.id) {
                if let Some(ref bid) = p.beads_id {
                    if crate::beads::BeadsClient::is_real_id(bid) {
                        real_project_id = Some(bid.clone());
                        break;
                    }
                }
            }
        }
        let project_beads_id = real_project_id.expect("expected real beads_id after schedule_beads_mirror on create");
        assert!(crate::beads::BeadsClient::is_real_id(&project_beads_id));

        // 2. Create a task under project
        let task = board
            .create(
                Some(project.id),
                "Task to split",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create task");

        // Mirror task
        board.schedule_beads_mirror(task.id);
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(t) = board.get(task.id) {
                if let Some(ref bid) = t.beads_id {
                    if crate::beads::BeadsClient::is_real_id(bid) {
                        break;
                    }
                }
            }
        }

        // 3. Transition task to Claimed and split into siblings
        let _ = board.transition(task.id, State::Shaping, "test", None);
        let _ = board.transition(task.id, State::Ready, "test", None);
        let _ = board.claim(task.id, "agent", None, 45);

        let children = vec![
            ("Sibling One".to_string(), "intent 1".to_string(), "dod 1".to_string()),
            ("Sibling Two".to_string(), "intent 2".to_string(), "dod 2".to_string()),
        ];
        let made = board.split(task.id, "agent", children, 5).expect("split");
        assert_eq!(made.len(), 2);

        // Mirror each split sibling
        for m in &made {
            board.schedule_beads_mirror(m.id);
        }

        // Wait for spawned async tasks on split siblings to assign real beads_ids
        for m in &made {
            let mut sibling_real_id = None;
            for _ in 0..50 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if let Some(item) = board.get(m.id) {
                    if let Some(ref bid) = item.beads_id {
                        if crate::beads::BeadsClient::is_real_id(bid) {
                            sibling_real_id = Some(bid.clone());
                            break;
                        }
                    }
                }
            }
            let sib_id = sibling_real_id.expect("expected real beads_id after schedule_beads_mirror on split");
            assert!(crate::beads::BeadsClient::is_real_id(&sib_id));
        }

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn done_and_retired_transitions_call_beads_close_and_sync_for_real_ids() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-test-beads-done-retired-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let board_path = test_dir.join("board.json");

        let b = Board::new(Schema::default(), board_path);
        let beads = b.beads.as_ref().expect("beads client");
        beads.init_stealth().await.expect("init stealth");

        let project = beads
            .create_linked("Test Project", 0, "epic", None, None, &[])
            .await
            .expect("create project");
        let task_done = beads
            .create_linked("Task Done", 1, "task", None, Some(&project.id), &[])
            .await
            .expect("create task done");
        let task_retired = beads
            .create_linked("Task Retired", 1, "task", None, Some(&project.id), &[])
            .await
            .expect("create task retired");

        let item1 = b
            .create(None, "Item Done", "why", None, Origin::Human, true, None)
            .expect("create item1");
        let item2 = b
            .create(None, "Item Retired", "why", None, Origin::Human, true, None)
            .expect("create item2");

        b.set_beads_id(item1.id, &task_done.id);
        b.set_beads_id(item2.id, &task_retired.id);

        let _ = b.transition(item1.id, State::Shaping, "t", None);
        let _ = b.transition(item1.id, State::Ready, "t", None);
        let _ = b.transition(item1.id, State::Done, "human", Some("done".into()));

        let _ = b.transition(item2.id, State::Shaping, "t", None);
        let _ = b.transition(item2.id, State::Ready, "t", None);
        let _ = b.transition(item2.id, State::Retired, "human", Some("retired".into()));

        let mut done_closed = false;
        let mut retired_closed = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if !done_closed {
                if let Ok(s) = beads.show(&task_done.id).await {
                    if s.status == "closed" {
                        done_closed = true;
                    }
                }
            }
            if !retired_closed {
                if let Ok(s) = beads.show(&task_retired.id).await {
                    if s.status == "closed" {
                        retired_closed = true;
                    }
                }
            }
            if done_closed && retired_closed {
                break;
            }
        }
        assert!(done_closed, "task_done status should be closed in beads");
        assert!(retired_closed, "task_retired status should be closed in beads");
    }

    #[tokio::test]
    async fn done_and_retired_transitions_noop_for_placeholders() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-test-beads-placeholder-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let board_path = test_dir.join("board.json");

        let b = Board::new(Schema::default(), board_path);
        let beads = b.beads.as_ref().expect("beads client");
        beads.init_stealth().await.expect("init stealth");

        let real_issue = beads
            .create_linked("Real Task", 1, "task", None, None, &[])
            .await
            .expect("create real task");

        let item1 = b
            .create(None, "Placeholder Item 1", "why", None, Origin::Human, true, None)
            .expect("create item1");
        let item2 = b
            .create(None, "Placeholder Item 2", "why", None, Origin::Human, true, None)
            .expect("create item2");

        assert!(item1.beads_id.as_ref().unwrap().starts_with("bd-honr-"));
        assert!(item2.beads_id.as_ref().unwrap().starts_with("bd-honr-"));

        let _ = b.transition(item1.id, State::Shaping, "t", None);
        let _ = b.transition(item1.id, State::Ready, "t", None);
        let _ = b.transition(item1.id, State::Done, "human", Some("done".into()));

        let _ = b.transition(item2.id, State::Shaping, "t", None);
        let _ = b.transition(item2.id, State::Ready, "t", None);
        let _ = b.transition(item2.id, State::Retired, "human", Some("retired".into()));

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let shown_real = beads.show(&real_issue.id).await.expect("show real task");
        assert_eq!(shown_real.status, "open");
    }

    #[tokio::test]
    async fn test_schedule_beads_mirror_persists_github_issue_url() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-store-github-url-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(None, "Test URL Project", "intent", None, Origin::Human, true, None)
            .expect("create project");

        board.schedule_beads_mirror(project.id);

        let mut real_project_id = None;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(p) = board.get(project.id) {
                if let Some(ref bid) = p.beads_id {
                    if crate::beads::BeadsClient::is_real_id(bid) {
                        real_project_id = Some(bid.clone());
                        break;
                    }
                }
            }
        }
        let project_beads_id = real_project_id.expect("expected real beads_id");

        // Update issue in beads with an external_ref / issue URL
        let expected_url = "https://github.com/shanemcd/honr/issues/777";
        beads_client
            .cmd()
            .args(["update", &project_beads_id, "--external-ref", expected_url])
            .output()
            .await
            .expect("update external ref");

        // Re-run schedule_beads_mirror to pick up external_ref from beads
        board.schedule_beads_mirror(project.id);

        let mut found_url = None;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(p) = board.get(project.id) {
                if let Some(ref url) = p.github_issue_url {
                    found_url = Some(url.clone());
                    break;
                }
            }
        }

        assert_eq!(
            found_url,
            Some(expected_url.to_string()),
            "github_issue_url should be persisted after sync/reading linked issue"
        );

        // Verify exposed on snapshot item
        let snap = board.snapshot();
        let item = snap.items.iter().find(|i| i.id == project.id).expect("item in snapshot");
        assert_eq!(item.github_issue_url.as_deref(), Some(expected_url));
    }

    #[tokio::test]
    async fn test_heal_placeholder_beads_ids_replaces_placeholders_and_syncs() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-store-heal-placeholders-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        // 1. Create open project and task, plus a retired item
        let project = board
            .create(None, "Project to Heal", "intent", None, Origin::Human, true, None)
            .expect("create project");
        let task = board
            .create(
                Some(project.id),
                "Task to Heal",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create task");
        let retired = board
            .create(
                Some(project.id),
                "Retired Card",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create retired");
        let _ = board.transition(retired.id, State::Retired, "test", None);

        // Verify all 3 start with bd-honr-* placeholder IDs
        assert!(project.beads_id.as_deref().unwrap().starts_with("bd-honr-"));
        assert!(task.beads_id.as_deref().unwrap().starts_with("bd-honr-"));
        assert!(retired.beads_id.as_deref().unwrap().starts_with("bd-honr-"));

        // 2. Execute heal
        let healed_count = board.heal_placeholder_beads_ids().await;
        assert_eq!(healed_count, 3, "should heal all 3 open items (project, initial plan task, task)");

        // 3. Assert open cards have real beads IDs
        let project_after = board.get(project.id).unwrap();
        let task_after = board.get(task.id).unwrap();
        let retired_after = board.get(retired.id).unwrap();

        assert!(
            crate::beads::BeadsClient::is_real_id(project_after.beads_id.as_deref().unwrap_or("")),
            "project should have real beads ID"
        );
        assert!(
            crate::beads::BeadsClient::is_real_id(task_after.beads_id.as_deref().unwrap_or("")),
            "task should have real beads ID"
        );
        assert!(
            retired_after.beads_id.as_deref().unwrap().starts_with("bd-honr-"),
            "retired card should remain unhealed with placeholder ID"
        );

        // 4. Verify task exists in beads with open status
        let task_beads_issue = beads_client
            .show(task_after.beads_id.as_deref().unwrap())
            .await
            .expect("task in beads");
        assert_eq!(task_beads_issue.id, task_after.beads_id.unwrap());
        assert_eq!(task_beads_issue.status, "open");

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_heal_backfills_github_issue_url_for_real_beads_id() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-store-url-backfill-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(
                None,
                "URL Backfill Project",
                "intent",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("create project");

        // Simulate the #59 gap: real beads_id on the card, no github_issue_url.
        let issue = beads_client
            .create_linked(
                "URL Backfill Project",
                2,
                "epic",
                Some("intent"),
                None,
                &[],
            )
            .await
            .expect("create bead");
        board.set_beads_id(project.id, &issue.id);
        assert!(board.get(project.id).unwrap().github_issue_url.is_none());

        let expected_url = "https://github.com/shanemcd/honr/issues/759";
        let out = beads_client
            .cmd()
            .args(["update", &issue.id, "--external-ref", expected_url])
            .output()
            .await
            .expect("update external ref");
        assert!(out.status.success(), "bd update --external-ref failed");

        // Project already has a real beads_id (the #59 gap). Heal may still
        // create beads for the seeded Initial Plan task; the important part is
        // backfilling this project's missing github_issue_url.
        let _ = board.heal_placeholder_beads_ids().await;

        let after = board.get(project.id).unwrap();
        assert_eq!(after.beads_id.as_deref(), Some(issue.id.as_str()));
        assert_eq!(after.github_issue_url.as_deref(), Some(expected_url));

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    #[ignore]
    async fn test_live_e2e_beads_github_auto_sync() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-e2e-live-sync-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("stealth init");

        let _ = beads_client
            .cmd()
            .args(["rename-prefix", "honr-"])
            .output()
            .await;
        let _ = beads_client
            .cmd()
            .args(["config", "set", "github.owner", "clankrshq"])
            .output()
            .await;
        let _ = beads_client
            .cmd()
            .args(["config", "set", "github.repo", "honr"])
            .output()
            .await;

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        // 1. Create a Project
        let project = board
            .create(
                None,
                "E2E: prove beads ↔ GitHub Issues auto-sync",
                "Project intent: Prove end-to-end auto-sync",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("create project");
        board.mirror_beads_item(project.id).await;

        let sync_p = beads_client
            .cmd()
            .args(["github", "sync", "--push-only", "-v"])
            .output()
            .await
            .unwrap();
        println!(
            "Project sync stdout = {}",
            String::from_utf8_lossy(&sync_p.stdout)
        );
        println!(
            "Project sync stderr = {}",
            String::from_utf8_lossy(&sync_p.stderr)
        );

        let project_bid = board.get(project.id).unwrap().beads_id.unwrap();
        println!("Project real beads_id = {project_bid}");
        let project_show = beads_client.show(&project_bid).await.expect("project show");
        println!("Project show = {project_show:?}");
        println!(
            "Project github_issue_url() = {:?}",
            project_show.github_issue_url()
        );

        // 2. Create a child Task under the Project
        let task = board
            .create(
                Some(project.id),
                "Verify create/close dual-writes beads + GitHub Issue",
                "Exercise live auto-sync path",
                Some("DOD".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create task");

        board.mirror_beads_item(task.id).await;

        let sync_t = beads_client
            .cmd()
            .args(["github", "sync", "--push-only", "-v"])
            .output()
            .await
            .unwrap();
        println!(
            "Task sync stdout = {}",
            String::from_utf8_lossy(&sync_t.stdout)
        );
        println!(
            "Task sync stderr = {}",
            String::from_utf8_lossy(&sync_t.stderr)
        );

        let task_item_before = board.get(task.id).expect("task item");
        let child_beads_id = task_item_before.beads_id.expect("task beads id");
        println!("Task beads_id = {child_beads_id}");

        let sync_task = beads_client.github_sync().await;
        println!("github_sync after task create = {sync_task:?}");

        let task_show = beads_client.show(&child_beads_id).await.expect("task show");
        println!("Task show = {task_show:?}");
        println!(
            "Task github_issue_url() = {:?}",
            task_show.github_issue_url()
        );

        let task_item = board.get(task.id).expect("task item");
        let child_github_url = task_item.github_issue_url.expect("task github issue url");

        println!("E2E EVIDENCE 1: child card beads_id = {child_beads_id}");
        println!("E2E EVIDENCE 2: github_issue_url = {child_github_url}");

        // 3. Mark Task as Done
        let _ = board.transition(task.id, State::Shaping, "test", None);
        let _ = board.transition(task.id, State::Ready, "test", None);
        let _ = board.claim(task.id, "agent", None, 45);
        let _ = board.transition(task.id, State::Done, "E2E verification completed", None);

        // Await beads close + github_sync directly to ensure sync finishes before checking
        let _ = beads_client
            .close(&child_beads_id, Some("E2E verification completed"))
            .await;
        let sync_res = beads_client.github_sync().await;
        println!("E2E EVIDENCE 3: github_sync after Done result: {sync_res:?}");

        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
