//! The board. Written to at machine speed, read by agents as their source of
//! truth, and moving a card *is* an action.
//!
//! Both faces — REST/SSE for humans, MCP for the cockpit and for agents — call
//! into here. Neither owns any state-machine logic, which is what keeps the two
//! renderings from drifting.

use crate::db::SqliteBoardStore;
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BoardState {
    pub next_id: ItemId,
    pub items: BTreeMap<ItemId, WorkItem>,
    /// Per-goal running narrative. A few lines appended at meaningful moments,
    /// not an event log — most people will read this instead of the board, and
    /// they will be right to.
    #[serde(default)]
    pub stories: BTreeMap<ItemId, Vec<StoryLine>>,
    #[serde(skip)]
    pub agent_logs: BTreeMap<ItemId, std::collections::VecDeque<String>>,
}

impl BoardState {
    /// Snapshot for durable flush — drops in-process-only agent log rings.
    fn clone_for_persist(&self) -> Self {
        Self {
            next_id: self.next_id,
            items: self.items.clone(),
            stories: self.stories.clone(),
            agent_logs: BTreeMap::new(),
        }
    }
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
    /// nothing. `7 in backlog · 2 blocked on #41 · oldest 40m` is smaller *and*
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
    /// Soft-retired Project — hidden from the default cockpit, available via
    /// "Show archived". Digests still omit these.
    pub archived: bool,
    pub columns: Vec<ColumnView>,
    pub story: Vec<StoryLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub items: Vec<WorkItem>,
    pub levels: Vec<Level>,
    pub goals: Vec<GoalView>,
    pub server_time: DateTime<Utc>,
    /// Wall-clock cap for a run (same as `agents.agent_timeout_secs`).
    pub agent_timeout_secs: u64,
    pub seq: u64,
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

#[derive(Debug, Clone, Default)]
struct ClaimPlanContext {
    project_title: Option<String>,
    project_prompt: Option<String>,
    plan_summary: Option<String>,
    plan_tasks: Vec<crate::model::PlanTaskBrief>,
    plan_task_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClaimGrant {
    pub item_id: ItemId,
    pub title: String,
    pub definition_of_done: Option<String>,
    /// Canonical beads hash id when mirrored (e.g. `honr-a1b2`).
    pub beads_id: Option<String>,
    /// Containing Project title (when this card is a Task).
    pub project_title: Option<String>,
    /// Project standing instructions (`project_prompt`).
    pub project_prompt: Option<String>,
    /// Plan summary from the Project artifact.
    pub plan_summary: Option<String>,
    /// Plan tasks (deps included); `current` marks this card's row when known.
    pub plan_tasks: Vec<crate::model::PlanTaskBrief>,
    /// Plan key for this card when matched via `item_id`.
    pub plan_task_key: Option<String>,
    pub notes: Vec<String>,
    /// Alias of `run_deadline_at` at claim — not extended by heartbeats.
    pub lease_expires_at: DateTime<Utc>,
    pub run_deadline_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ReadyCard {
    pub id: ItemId,
    pub title: String,
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
    pub backlog: usize,
    pub in_review: usize,
    pub ready_to_dispatch: Vec<ReadyCard>,
    pub latest_story: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Digest {
    pub since: DateTime<Utc>,
    pub goals: Vec<GoalDigest>,
}

pub const DEFAULT_EVENT_BUFFER_CAPACITY: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CatchUpResult {
    /// Replayed missed events in sequence order.
    Events(Vec<BoardEvent>),
    /// Lagged beyond buffer capacity or future sequence; client must reset state.
    Reset { seq: u64 },
}

// ------------------------------------------------------------------ the board

pub struct Board {
    state: RwLock<BoardState>,
    tx: broadcast::Sender<BoardEvent>,
    seq: AtomicU64,
    event_buffer: RwLock<std::collections::VecDeque<BoardEvent>>,
    buffer_capacity: usize,
    dirty: AtomicBool,
    pub schema: Schema,
    /// Legacy JSON path: import source when the DB is empty; also beads co-locate.
    /// When [`Self::store`] is set, flush no longer rewrites this file.
    path: PathBuf,
    /// SQLite (later Postgres) row store. `None` in unit tests that stay in-memory/JSON.
    store: Option<Arc<SqliteBoardStore>>,
    started_at: DateTime<Utc>,
    pub beads: Option<crate::beads::BeadsClient>,
    pub openshell: Option<crate::openshell::OpenShell>,
    in_flight_github_pushes: std::sync::Mutex<
        std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    >,
    pushed_beads_ids: std::sync::RwLock<std::collections::HashSet<String>>,
}

pub type SharedBoard = Arc<Board>;

/// The result of attempting a git rebase for a card in Review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// Rebase succeeded cleanly.
    Clean,
    /// Rebase encountered git merge conflicts.
    Conflict {
        conflicting_files: Vec<String>,
        reason: Option<String>,
    },
}

impl Board {
    pub fn new(schema: Schema, path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1024);
        // Co-locate beads with the board file when possible. Prefer an absolute
        // beads dir so `bd`'s current_dir is never the empty relative parent of
        // `.beads` (that used to make every `bd` spawn fail with ENOENT).
        // Production always co-locates beads with the board file. Under `cargo test`
        // leave beads detached by default — attaching a shared beads dir across
        // parallel Board::new callers races heal/mirror. Beads-focused tests
        // attach an isolated client explicitly.
        let beads = if cfg!(test) {
            None
        } else {
            let beads_dir = {
                let raw = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
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
            Some(crate::beads::BeadsClient::new(beads_dir))
        };
        Self {
            state: RwLock::new(BoardState { next_id: 1, ..Default::default() }),
            tx,
            seq: AtomicU64::new(0),
            event_buffer: RwLock::new(std::collections::VecDeque::new()),
            buffer_capacity: DEFAULT_EVENT_BUFFER_CAPACITY,
            dirty: AtomicBool::new(false),
            schema,
            path,
            store: None,
            started_at: Utc::now(),
            beads,
            openshell: Some(crate::openshell::OpenShell::default()),
            in_flight_github_pushes: std::sync::Mutex::new(std::collections::HashMap::new()),
            pushed_beads_ids: std::sync::RwLock::new(std::collections::HashSet::new()),
        }
    }

    #[allow(dead_code)]
    pub fn with_buffer_capacity(mut self, capacity: usize) -> Self {
        self.buffer_capacity = capacity;
        self
    }

    /// Apply legacy renames / beads placeholders. Returns whether the state was mutated.
    fn heal_loaded_state(state: &mut BoardState) -> (usize, usize) {
        let mut healed = 0usize;
        let mut renamed = 0usize;
        let rename_to: Vec<(ItemId, String)> = state
            .items
            .values()
            .filter(|i| i.title == crate::model::INITIAL_PLAN_TITLE_LEGACY)
            .filter_map(|i| {
                let parent = i.parent?;
                let pname = state.items.get(&parent)?.title.clone();
                Some((i.id, crate::model::initial_plan_title(&pname)))
            })
            .collect();
        for (id, title) in rename_to {
            if let Some(item) = state.items.get_mut(&id) {
                item.title = title;
                renamed += 1;
            }
        }
        for (id, item) in state.items.iter_mut() {
            if item.beads_id.is_none() {
                item.beads_id = Some(format!("bd-honr-{id}"));
            }
            // A brief experiment left Initial plan in Shaping; restore
            // them to Backlog so dedicated planning agents can claim.
            if item.is_initial_plan_task() && item.state == State::Shaping {
                item.state = State::Backlog;
                healed += 1;
            }
        }
        (healed, renamed)
    }

    /// Load a previously persisted board from `honr.json`, or start empty.
    ///
    /// Prefer [`Self::load_with_store`] in production — JSON is the one-shot
    /// import source once a `BoardStore` is attached.
    pub fn load_or_new(schema: Schema, path: PathBuf) -> Self {
        let board = Self::new(schema, path.clone());
        if let Ok(raw) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<BoardState>(&raw) {
                Ok(mut state) => {
                    let (healed, renamed) = Self::heal_loaded_state(&mut state);
                    tracing::info!(items = state.items.len(), "restored board from {path:?}");
                    if healed > 0 {
                        tracing::info!("healed {healed} Initial plan Task(s) Shaping → Backlog");
                    }
                    if renamed > 0 {
                        tracing::info!(
                            "renamed {renamed} Initial plan Task(s) to include Project name"
                        );
                    }
                    *board.state.write().unwrap() = state;
                    if healed > 0 || renamed > 0 {
                        board.dirty.store(true, Ordering::Relaxed);
                        board.flush();
                    }
                }
                Err(e) => tracing::warn!("ignoring unreadable {path:?}: {e}"),
            }
        }
        board
    }

    /// Boot from SQLite: one-shot import from `json_path` when the DB is empty,
    /// otherwise restore rows. Mutations flush as row updates, not a JSON rewrite.
    pub async fn load_with_store(
        schema: Schema,
        json_path: PathBuf,
        store: Arc<SqliteBoardStore>,
    ) -> Result<Self, crate::db::StoreError> {
        let imported = store.import_json_if_empty(&json_path).await?;
        if imported {
            tracing::info!(
                path = %json_path.display(),
                "imported board from JSON into database (one-shot)"
            );
        }
        let mut state = store.load_board_state().await?;
        let (healed, renamed) = Self::heal_loaded_state(&mut state);
        if healed > 0 {
            tracing::info!("healed {healed} Initial plan Task(s) Shaping → Backlog");
        }
        if renamed > 0 {
            tracing::info!("renamed {renamed} Initial plan Task(s) to include Project name");
        }
        tracing::info!(
            items = state.items.len(),
            "restored board from database"
        );
        let mut board = Self::new(schema, json_path);
        board.store = Some(store);
        *board.state.write().unwrap() = state;
        if healed > 0 || renamed > 0 {
            board.dirty.store(true, Ordering::Relaxed);
            board.flush();
        }
        Ok(board)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BoardEvent> {
        self.tx.subscribe()
    }

    /// Whether the supervisor may claim this Backlog card right now.
    ///
    /// Parked cards are never claimable until unparked (which also queues
    /// dispatch — Resume is Start).
    pub fn may_claim(&self, id: ItemId) -> bool {
        !self.state.read().unwrap().items.get(&id).is_some_and(|it| it.parked)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn record_and_send(&self, event: BoardEvent) {
        {
            let mut buffer = self.event_buffer.write().unwrap();
            if buffer.len() >= self.buffer_capacity {
                buffer.pop_front();
            }
            buffer.push_back(event.clone());
        }
        let _ = self.tx.send(event);
    }

    pub fn catch_up(&self, last_seq: u64) -> CatchUpResult {
        let current_seq = self.current_seq();
        if last_seq > current_seq {
            return CatchUpResult::Reset { seq: current_seq };
        }
        if last_seq == current_seq {
            return CatchUpResult::Events(Vec::new());
        }

        let buffer = self.event_buffer.read().unwrap();
        if buffer.is_empty() {
            return CatchUpResult::Reset { seq: current_seq };
        }

        let oldest_seq = buffer.front().unwrap().seq();
        let needed_seq = last_seq + 1;

        if needed_seq < oldest_seq {
            CatchUpResult::Reset { seq: current_seq }
        } else {
            let missed: Vec<BoardEvent> = buffer
                .iter()
                .filter(|ev| ev.seq() > last_seq)
                .cloned()
                .collect();
            CatchUpResult::Events(missed)
        }
    }

    fn emit(&self, item: &WorkItem) {
        let mut item = item.clone();
        {
            let s = self.state.read().unwrap();
            Self::populate_blockers(&s, &mut item);
        }
        self.record_and_send(BoardEvent::Upsert {
            seq: self.next_seq(),
            item: Box::new(item),
        });
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Flush durable state if anything changed. Called on an interval so a
    /// fleet of heartbeating agents doesn't turn into a write storm.
    ///
    /// With a [`SqliteBoardStore`] attached, this writes rows (not `honr.json`).
    /// Without a store (unit tests), the legacy whole-file JSON path remains.
    pub fn flush(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Some(store) = &self.store {
            let snapshot = self.state.read().unwrap().clone_for_persist();
            let result = match tokio::runtime::Handle::try_current() {
                Ok(handle)
                    if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread =>
                {
                    tokio::task::block_in_place(|| {
                        handle.block_on(store.save_board_state(&snapshot))
                    })
                }
                Ok(_) | Err(_) => {
                    // current-thread runtime (tests) or no runtime: own thread so
                    // we never call block_in_place / nest block_on on CurrentThread.
                    let store = Arc::clone(store);
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("board flush runtime");
                        rt.block_on(store.save_board_state(&snapshot))
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(crate::db::StoreError::Query(
                            "board flush thread panicked".into(),
                        ))
                    })
                }
            };
            if let Err(e) = result {
                tracing::error!("board database flush failed: {e}");
                self.dirty.store(true, Ordering::Relaxed);
            }
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
        if matches!(to, State::Backlog | State::NeedsHuman | State::Done | State::Retired | State::Shaping) {
            item.lease = None;
            item.run_deadline_at = None;
        }
        if to == State::Backlog {
            item.progress = 0.0;
            // Bounce / park / halt / deadline expiry all land here — never auto-start again.
            item.awaiting_dispatch = false;
            item.rebase_requested = false;
            if by == "human" {
                item.run_failures = 0;
                item.escalation = None;
                item.last_bounce_reason = None;
            }
        }
        // Terminal cards discard the LLM session and sandbox environment.
        if to.is_terminal() {
            item.conversation_id = None;
            item.parked = false;
            item.awaiting_dispatch = false;
            item.rebase_requested = false;
            item.environment = None;
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
        let (item, env_to_delete) = {
            let mut s = self.state.write().unwrap();
            let prev_env = s.items.get(&id).and_then(|i| i.environment.clone());
            let item = Self::transition_locked(&mut s, id, to, by, reason)?;
            let env_to_delete = if to.is_terminal() { prev_env } else { None };
            (item, env_to_delete)
        };
        self.emit(&item);

        if let Some(env) = env_to_delete {
            let os = self.openshell.clone().unwrap_or_default();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = os.delete(&env).await;
                });
            }
        }

        // Initial plan / split proposals become sibling Tasks when the card
        // reaches Done (typically PR merge via webhook) — not on Approve.
        if to == State::Done {
            if let Err(e) = self.materialize_proposal_on_done(id, by) {
                tracing::warn!(id, error = %e, "materialize proposal on Done failed");
            }
            let unblocked = self.newly_unblocked_siblings(id);
            if unblocked.len() == 1 {
                let next = &unblocked[0];
                self.story(
                    id,
                    format!(
                        "Unblocked next sibling #{} ({}) — ready to dispatch.",
                        next.id, next.title
                    ),
                );
            } else if unblocked.len() > 1 {
                let list = unblocked
                    .iter()
                    .map(|u| format!("#{} ({})", u.id, u.title))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.story(
                    id,
                    format!("Unblocked next siblings: {list} — ready to dispatch."),
                );
            }
        }

        if to == State::Done || to == State::Retired {
            let beads = self.beads.clone();
            let beads_id = item.beads_id.clone();
            let is_initial = item.is_initial_plan_task();
            let has_gh_url = item.github_issue_url.is_some();
            let reason_str = item
                .history
                .last()
                .and_then(|h| h.reason.clone())
                .unwrap_or_else(|| format!("Marked {to:?}"));
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    if let (Some(ref b), Some(ref bid)) = (&beads, &beads_id) {
                        if crate::beads::BeadsClient::is_real_id(bid) {
                            let _ = b.close(bid, Some(&reason_str)).await;
                            let has_beads_gh_url = if is_initial && !has_gh_url {
                                b.show(bid).await.ok().and_then(|s| s.github_issue_url()).is_some()
                            } else {
                                false
                            };
                            if !is_initial || has_gh_url || has_beads_gh_url {
                                let _ = b.github_push(std::slice::from_ref(bid)).await;
                            }
                            b.schedule_dolt_push();
                        }
                    }
                    if let Some(ref b) = beads {
                        let _ = b.close_completed_epics().await;
                    }
                });
            }
        } else if to == State::Backlog {
            // Only reopen when leaving an agent-held state. Shaping→Backlog (seed /
            // approve) must not race a later Done→close with a late `open`.
            let from = item.history.last().map(|h| h.from);
            if matches!(
                from,
                Some(State::Claimed | State::Running | State::NeedsHuman)
            ) {
                if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
                    if crate::beads::BeadsClient::is_real_id(&bid) {
                        if let Ok(handle) = tokio::runtime::Handle::try_current() {
                            handle.spawn(async move {
                                if let Err(e) = beads.set_status(&bid, "open").await {
                                    tracing::warn!(
                                        %bid,
                                        error = %e,
                                        "beads reopen on Backlog failed"
                                    );
                                }
                            });
                        }
                    }
                }
            }
        }

        // Re-read after Done-side materialize so callers see stamped item_ids.
        Ok(self.get(id).unwrap_or(item))
    }

    /// Create a Project (root) or a Task under a Project. Tasks are flat —
    /// nesting under another Task is refused.
    ///
    /// Beads dual-write is **asynchronous**: cards keep a `bd-honr-*`
    /// placeholder until [`Self::schedule_beads_mirror`] /
    /// [`Self::heal_placeholder_beads_ids`] bind a real id. Approve/materialize
    /// must not wait on `bd create` or an in-flight `bd dolt push`.
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
            // Placeholder until async mirror / heal binds a real beads hash.
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
                // Plan lives on the Initial plan Task (`proposal`), not the Project.
                item.plan = None;
                item.project_prompt = Some(crate::model::DEFAULT_PROJECT_PROMPT.to_string());
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
        Ok(self.get(item.id).unwrap_or(item))
    }

    fn seed_initial_plan_task(&self, project_id: ItemId, project_title: &str) -> Result<WorkItem, String> {
        let title = crate::model::initial_plan_title(project_title);
        let seed = self.create(
            Some(project_id),
            title.clone(),
            format!(
                "Propose sibling Tasks for «{project_title}» (plan.json: keys, deps, \
                 mechanically checkable DoDs). Open one plan/docs PR, then finish with \
                 report.json (Review). Merging the plan PR creates those Tasks — do not write \
                 split.json on this card."
            ),
            Some("plan.json + docs PR in Review; merge creates Tasks.".into()),
            Origin::Planner,
            false,
            None,
        )?;
        let _ = self.transition(seed.id, State::Shaping, "cockpit", Some("seed plan task".into()));
        let seed = self
            .transition(seed.id, State::Backlog, "cockpit", Some("seed plan task".into()))
            .map_err(|e| e.to_string())?;
        self.story(
            project_id,
            format!("Seeded {title} Task #{}.", seed.id),
        );
        Ok(seed)
    }

    /// Resolve a Project id or Initial plan id to the Initial plan Task id.
    pub fn resolve_initial_plan_id(&self, id: ItemId) -> Result<ItemId, String> {
        let item = self
            .get(id)
            .ok_or_else(|| format!("no work item #{id}"))?;
        if item.is_initial_plan_task() {
            return Ok(id);
        }
        if item.is_project() {
            return self
                .children_of(id)
                .into_iter()
                .find(|&cid| self.get(cid).is_some_and(|c| c.is_initial_plan_task()))
                .ok_or_else(|| format!("Project #{id} has no Initial plan Task"));
        }
        Err("plan operations require a Project or Initial plan Task".into())
    }

    fn initial_plan_of(&self, project_id: ItemId) -> Option<WorkItem> {
        self.children_of(project_id)
            .into_iter()
            .find_map(|cid| self.get(cid).filter(|c| c.is_initial_plan_task()))
    }

    /// GoalView label from the Initial plan card (not Project.plan).
    fn plan_status_label(&self, project_id: ItemId) -> String {
        let Some(seed) = self.initial_plan_of(project_id) else {
            return "no_plan".into();
        };
        let has_proposal = seed
            .proposal
            .as_ref()
            .is_some_and(|p| !p.tasks.is_empty());
        if seed.state == State::Done {
            return if has_proposal {
                "approved".into()
            } else {
                // Legacy boards: Tasks exist but proposal was cleared.
                let has_impl = self.children_of(project_id).into_iter().any(|cid| {
                    self.get(cid)
                        .is_some_and(|c| !c.is_initial_plan_task() && c.state != State::Retired)
                });
                if has_impl {
                    "approved".into()
                } else {
                    "no_plan".into()
                }
            };
        }
        if has_proposal {
            "awaiting_approval".into()
        } else {
            "no_plan".into()
        }
    }

    /// Write / revise the proposal on the Initial plan card. Does not create
    /// board Tasks — Approve materializes them. `id` may be the Project or the
    /// Initial plan Task. `cancel_keys` is ignored (replan is out of scope).
    pub fn propose_plan(
        &self,
        id: ItemId,
        summary: impl Into<String>,
        tasks: Vec<PlanTaskSpec>,
        _cancel_keys: Vec<String>,
    ) -> Result<TaskProposal, String> {
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
        let seed_id = self.resolve_initial_plan_id(id)?;
        let seed = self
            .get(seed_id)
            .ok_or_else(|| format!("no work item #{seed_id}"))?;
        if seed.state.is_terminal() {
            return Err("Initial plan already accepted — proposal is frozen".into());
        }
        let proposal = TaskProposal {
            summary: summary.into(),
            tasks,
        };
        self.set_proposal(seed_id, proposal.clone())?;
        let project_id = seed.parent.unwrap_or(seed_id);
        self.story(
            project_id,
            format!(
                "Plan proposed on Initial plan ({} tasks) — awaiting Approve.",
                proposal.tasks.len()
            ),
        );
        Ok(proposal)
    }

    /// Approve the Initial plan proposal: materialize Tasks and finish the card.
    /// `id` may be the Project or the Initial plan Task.
    pub fn approve_plan(&self, id: ItemId) -> Result<Vec<ItemId>, String> {
        let seed_id = self.resolve_initial_plan_id(id)?;
        let seed = self
            .get(seed_id)
            .ok_or_else(|| format!("no work item #{seed_id}"))?;
        if seed.state.is_terminal() {
            return Err("Initial plan already accepted".into());
        }

        // Legacy boards: proposal empty but Project still holds an awaiting Plan.
        if !seed
            .proposal
            .as_ref()
            .is_some_and(|p| !p.tasks.is_empty())
        {
            if let Some(project_id) = seed.parent {
                if let Some(project) = self.get(project_id) {
                    if let Some(plan) = project
                        .plan
                        .as_ref()
                        .filter(|p| !p.tasks.is_empty())
                    {
                        self.set_proposal(
                            seed_id,
                            TaskProposal {
                                summary: plan.summary.clone(),
                                tasks: plan.tasks.clone(),
                            },
                        )?;
                        {
                            let mut s = self.state.write().unwrap();
                            if let Some(p) = s.items.get_mut(&project_id) {
                                p.plan = None;
                                let snap = p.clone();
                                drop(s);
                                self.emit(&snap);
                            }
                        }
                    }
                }
            }
        }

        let seed = self
            .get(seed_id)
            .ok_or_else(|| format!("no work item #{seed_id}"))?;
        if !seed
            .proposal
            .as_ref()
            .is_some_and(|p| !p.tasks.is_empty())
        {
            return Err(
                "no proposal on Initial plan — run propose_breakdown or wait for plan.json"
                    .into(),
            );
        }

        let done = self.approve_review(seed_id)?;
        if done.state == State::Review {
            return Err(
                "Plan looks good — Tasks will be created when the plan PR merges \
                 (no separate Approve materialize step)"
                    .into(),
            );
        }
        let published: Vec<ItemId> = done
            .proposal
            .as_ref()
            .map(|p| p.tasks.iter().filter_map(|t| t.item_id).collect())
            .unwrap_or_default();
        if published.is_empty() {
            return Err("Initial plan reached Done but created no Tasks".into());
        }
        Ok(published)
    }

    pub fn is_beads_id_pushed(&self, beads_id: &str) -> bool {
        self.pushed_beads_ids.read().unwrap().contains(beads_id)
    }

    pub fn mark_beads_id_pushed(&self, beads_id: &str) {
        self.pushed_beads_ids
            .write()
            .unwrap()
            .insert(beads_id.to_string());
    }

    fn cleanup_in_flight_lock(&self, beads_id: &str, lock: &Arc<tokio::sync::Mutex<()>>) {
        let mut map = self.in_flight_github_pushes.lock().unwrap();
        if Arc::strong_count(lock) <= 2 {
            map.remove(beads_id);
        }
    }

    /// Push a single beads item to GitHub, single-flighted per `beads_id`.
    /// Concurrent callers for the same `beads_id` await the in-flight push,
    /// and no-op once the push has completed or `github_issue_url` exists.
    pub async fn push_beads_item_single_flight(self: &Arc<Self>, id: ItemId, beads_id: &str) {
        if !crate::beads::BeadsClient::is_real_id(beads_id) {
            return;
        }
        if let Some(item) = self.get(id) {
            if item.is_initial_plan_task() {
                return;
            }
        }
        let Some(beads) = self.beads.clone() else {
            return;
        };

        if self.is_beads_id_pushed(beads_id) {
            return;
        }
        if self.get(id).and_then(|i| i.github_issue_url.clone()).is_some() {
            self.mark_beads_id_pushed(beads_id);
            return;
        }
        if self.refresh_github_issue_url(id, beads_id).await {
            self.mark_beads_id_pushed(beads_id);
            return;
        }

        let lock = {
            let mut map = self.in_flight_github_pushes.lock().unwrap();
            map.entry(beads_id.to_string()).or_default().clone()
        };

        let _guard = lock.lock().await;

        if self.is_beads_id_pushed(beads_id) {
            self.cleanup_in_flight_lock(beads_id, &lock);
            return;
        }
        if self.get(id).and_then(|i| i.github_issue_url.clone()).is_some() {
            self.mark_beads_id_pushed(beads_id);
            self.cleanup_in_flight_lock(beads_id, &lock);
            return;
        }
        if self.refresh_github_issue_url(id, beads_id).await {
            self.mark_beads_id_pushed(beads_id);
            self.cleanup_in_flight_lock(beads_id, &lock);
            return;
        }

        match beads.github_push(std::slice::from_ref(&beads_id.to_string())).await {
            Ok(()) => {
                self.mark_beads_id_pushed(beads_id);
                beads.schedule_dolt_push();
                self.refresh_github_issue_url(id, beads_id).await;
            }
            Err(e) => {
                tracing::warn!(id, beads_id, error = %e, "beads github push failed in single_flight");
            }
        }

        self.cleanup_in_flight_lock(beads_id, &lock);
    }

    /// Dual-write a single board item into beads (Project→epic, Task→task with `--parent`).
    /// If successful, stores the real hash id, then pushes **that** bead to GitHub
    /// (`bd github push <id>`) without blocking other mirrors on the push.
    ///
    /// Tasks push their Project epic first so GitHub can attach child Issues.
    pub async fn mirror_beads_item(self: &Arc<Self>, id: ItemId) {
        let Some(beads_id) = self.mirror_beads_item_local(id).await else {
            return;
        };
        let parent = self.get(id).and_then(|i| i.parent);
        // Epic before Task — GH sub-issues need the parent Issue to exist.
        if let Some(pid) = parent {
            if let Some(p) = self.get(pid) {
                if let Some(pbid) = p
                    .beads_id
                    .filter(|b| crate::beads::BeadsClient::is_real_id(b))
                {
                    self.push_beads_item_single_flight(pid, &pbid).await;
                }
            }
        }
        self.push_beads_item_single_flight(id, &beads_id).await;
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
        let meta = crate::beads::BeadsClient::honr_metadata(id, item.pr_url.as_deref());

        match beads
            .create_linked(
                &title,
                2,
                issue_type,
                Some(&intent),
                parent,
                &blockers,
                Some(&meta),
            )
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

    /// Mirror many cards after Approve/materialize: suppress dolt push for the
    /// storm, bind real beads ids (parents before children), then write deps.
    /// Returns immediately — work runs on the runtime; cockpit must not wait.
    pub fn schedule_beads_mirror_batch(self: &Arc<Self>, ids: &[ItemId]) {
        if ids.is_empty() {
            return;
        }
        let board = Arc::clone(self);
        let mut ids = ids.to_vec();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Some(beads) = board.beads.clone() {
                    beads.begin_create_storm();
                }
                // Projects / lower ids first so `--parent` and blocker deps resolve.
                ids.sort_by_key(|id| {
                    let is_task = board.get(*id).is_some_and(|i| i.parent.is_some());
                    (is_task, *id)
                });
                for id in &ids {
                    if let Some(pid) = board.get(*id).and_then(|i| i.parent) {
                        board.mirror_beads_item(pid).await;
                    }
                    board.mirror_beads_item(*id).await;
                }
                for id in &ids {
                    board.sync_beads_blocked_by(*id).await;
                }
                if let Some(beads) = board.beads.clone() {
                    beads.end_create_storm();
                }
            });
        }
    }

    /// Write board `blocked_by` edges into beads once both sides have real ids.
    async fn sync_beads_blocked_by(self: &Arc<Self>, id: ItemId) {
        let Some(beads) = self.beads.clone() else {
            return;
        };
        let Some(item) = self.get(id) else {
            return;
        };
        let Some(bid) = item
            .beads_id
            .as_deref()
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .map(str::to_string)
        else {
            return;
        };
        let deps: Vec<String> = item
            .blocked_by
            .iter()
            .filter_map(|b| self.get(*b).and_then(|w| w.beads_id))
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .collect();
        for dep in deps {
            if let Err(e) = beads.dep_add(&bid, &dep, "blocks").await {
                tracing::warn!(%bid, %dep, error = %e, "beads dep sync failed");
            }
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
                if item.state == State::Retired || item.is_initial_plan_task() {
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
        if self.beads.is_none() {
            return 0;
        }

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
            for (id, beads_id) in &need_push {
                self.push_beads_item_single_flight(*id, beads_id).await;
                if self.get(*id).and_then(|i| i.github_issue_url).is_some() {
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

        // Local creates first (projects before tasks), then single-flight GitHub push per card
        let mut created: Vec<(ItemId, String)> = Vec::new();
        for id in projects.into_iter().chain(tasks) {
            if let Some(beads_id) = self.mirror_beads_item_local(id).await {
                created.push((id, beads_id));
            }
        }
        if !created.is_empty() {
            for (id, beads_id) in &created {
                self.push_beads_item_single_flight(*id, beads_id).await;
            }
            // Deps may have been set on the board while ids were still placeholders.
            for (id, _) in &created {
                self.sync_beads_blocked_by(*id).await;
            }
            let healed = created.len();
            tracing::info!("healed {healed} placeholder beads_id(s) with real beads IDs");
        }

        // Always run: real beads_id + missing URL is a separate failure mode from placeholders.
        self.backfill_missing_github_issue_urls().await;
        let healed_epics = self.heal_completed_epics().await;
        if healed_epics > 0 {
            tracing::info!("healed {healed_epics} completed epic(s)");
        }
        created.len()
    }

    /// Close open epics in beads whose children are all completed or superseded,
    /// and transition matching board Project cards to Done.
    pub async fn heal_completed_epics(self: &Arc<Self>) -> usize {
        let mut healed = 0usize;

        // 1. Heal beads graph
        if let Some(ref beads) = self.beads {
            if let Ok(closed_bids) = beads.close_completed_epics().await {
                healed += closed_bids.len();
                // Mark matching board Project cards as Done
                for bid in &closed_bids {
                    let matching_ids: Vec<(ItemId, State)> = {
                        let s = self.state.read().unwrap();
                        s.items
                            .values()
                            .filter(|i| {
                                i.parent.is_none()
                                    && i.state != State::Done
                                    && i.state != State::Retired
                                    && i.beads_id.as_deref() == Some(bid.as_str())
                            })
                            .map(|i| (i.id, i.state))
                            .collect()
                    };
                    for (pid, state) in matching_ids {
                        if state == State::Draft {
                            let _ = self.transition(pid, State::Shaping, "beads-epic-hygiene", None);
                        }
                        let _ = self.transition(
                            pid,
                            State::Done,
                            "beads-epic-hygiene",
                            Some("All children completed or superseded".into()),
                        );
                    }
                }
            }
        }

        // 2. Heal board projects whose child tasks on the board are all Done or Retired
        let project_ids: Vec<(ItemId, State)> = {
            let s = self.state.read().unwrap();
            s.items
                .values()
                .filter(|i| i.parent.is_none() && i.state != State::Done && i.state != State::Retired)
                .map(|i| (i.id, i.state))
                .collect()
        };

        for (pid, state) in project_ids {
            let (child_count, all_done) = {
                let s = self.state.read().unwrap();
                let children: Vec<&WorkItem> =
                    s.items.values().filter(|i| i.parent == Some(pid)).collect();
                let count = children.len();
                let done = count > 0
                    && children
                        .iter()
                        .all(|c| c.state == State::Done || c.state == State::Retired);
                (count, done)
            };

            if child_count > 0 && all_done {
                if state == State::Draft {
                    let _ = self.transition(pid, State::Shaping, "epic-hygiene", None);
                }
                let _ = self.transition(
                    pid,
                    State::Done,
                    "epic-hygiene",
                    Some("All child tasks completed".into()),
                );
                healed += 1;
            }
        }

        healed
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
    /// sandboxes. Same idea as a CI retry budget: fail a few times, then escalate.
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
                .transition(id, State::Backlog, "supervisor", Some(format!("run failed: {reason}")))
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

    /// Persist (or clear) the agy conversation id for park/resume.
    pub fn set_conversation_id(&self, id: ItemId, conversation_id: Option<String>) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            if it.conversation_id == conversation_id {
                return;
            }
            it.conversation_id = conversation_id;
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

        if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
            if crate::beads::BeadsClient::is_real_id(&bid) {
                let meta =
                    crate::beads::BeadsClient::honr_metadata(id, item.pr_url.as_deref());
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Err(e) = beads
                            .update_fields(&bid, None, None, Some(&meta))
                            .await
                        {
                            tracing::warn!(%bid, error = %e, "beads pr_url metadata sync failed");
                        }
                    });
                }
            }
        }
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

    /// Tweak an item's title, intent, definition of done, engine, or project prompt.
    /// Used from Shaping (pre-Backlog), Backlog (pre-dispatch), and Review (with
    /// Request changes) so humans can rewrite the contract the next agent sees.
    pub fn update_item(
        &self,
        id: ItemId,
        title: Option<String>,
        intent: Option<String>,
        definition_of_done: Option<String>,
        engine: Option<String>,
        project_prompt: Option<String>,
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
            if let Some(p) = project_prompt {
                if it.is_project() {
                    it.project_prompt = if p.trim().is_empty() {
                        None
                    } else {
                        Some(p)
                    };
                }
            }
            it.clone()
        };
        self.emit(&item);

        if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
            if crate::beads::BeadsClient::is_real_id(&bid) {
                let title = item.title.clone();
                let intent = item.intent.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Err(e) = beads
                            .update_fields(&bid, Some(&title), Some(&intent), None)
                            .await
                        {
                            tracing::warn!(%bid, error = %e, "beads title/intent sync failed");
                        }
                    });
                }
            }
        }
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

    /// Backlog leaves that are unblocked and match capabilities. Not a start
    /// queue — cockpit must `enqueue_dispatch` before the supervisor claims.
    pub fn list_backlog(&self, capabilities: &[String]) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        s.items
            .values()
            .filter(|i| i.state == State::Backlog)
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

    /// Legacy name for cockpit `list_ready` MCP tool.
    pub fn list_ready(&self, capabilities: &[String]) -> Vec<WorkItem> {
        self.list_backlog(capabilities)
    }

    /// Cards the cockpit asked to start, oldest first. Supervisor drains these.
    pub fn list_awaiting_dispatch(&self) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        let mut items: Vec<_> = s
            .items
            .values()
            .filter(|i| i.state == State::Backlog && i.awaiting_dispatch && !i.parked)
            .filter(|i| i.level.as_deref() != Some("Project"))
            .filter(|i| !Self::has_children(&s, i.id))
            .filter(|i| Self::unresolved_blockers(&s, i).is_empty())
            .cloned()
            .collect();
        items.sort_by_key(|i| i.entered_state_at);
        items
    }

    /// Cockpit asked the supervisor to start this Backlog card.
    pub fn enqueue_dispatch(&self, id: ItemId) -> Result<WorkItem, String> {
        if !self.may_claim(id) {
            return Err("card is parked; unpark before dispatch".into());
        }
        let item = {
            let mut s = self.state.write().unwrap();
            let has_children = Self::has_children(&s, id);
            let (state, parked, is_project, dod_missing, blockers) = {
                let it = s.items.get(&id).ok_or("no such item")?;
                (
                    it.state,
                    it.parked,
                    it.level.as_deref() == Some("Project"),
                    it.definition_of_done.is_none(),
                    Self::unresolved_blockers(&s, it),
                )
            };
            if state != State::Backlog {
                return Err("only a Backlog card can be dispatched".into());
            }
            if parked {
                return Err("card is parked; unpark before dispatch".into());
            }
            if is_project || has_children {
                return Err("containers are not dispatchable".into());
            }
            if !blockers.is_empty() {
                return Err(format!("card is blocked by {blockers:?}"));
            }
            if dod_missing {
                return Err("leaf needs a definition of done before dispatch".into());
            }
            let it = s.items.get_mut(&id).unwrap();
            it.awaiting_dispatch = true;
            let mut out = it.clone();
            Self::populate_blockers(&s, &mut out);
            out
        };
        self.emit(&item);
        self.story(
            id,
            format!("{}: queued for dispatch — supervisor will claim when a slot opens.", item.title),
        );
        Ok(item)
    }

    /// Cancel a pending start request without changing state.
    #[allow(dead_code)]
    pub fn clear_dispatch(&self, id: ItemId) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.awaiting_dispatch = false;
            it.rebase_requested = false;
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Beads "ready" tasks (`issue_type=task` only), mapped back to board items when present.
    pub async fn list_ready_beads(&self, parent: Option<&str>) -> Result<Vec<crate::beads::BeadsIssue>, String> {
        if let Some(b) = &self.beads {
            b.list_ready_focused(parent).await
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

    /// `claim` — assigns the card and a fixed run deadline (`timeout_secs`).
    /// The deadline is not extended by heartbeats.
    pub fn claim(
        &self,
        id: ItemId,
        agent_id: &str,
        model: Option<String>,
        timeout_secs: i64,
    ) -> Result<ClaimGrant, TransitionError> {
        let now = Utc::now();
        let deadline = now + Duration::seconds(timeout_secs.max(1));

        let item = {
            let mut s = self.state.write().unwrap();
            if s.items.get(&id).is_some_and(|it| it.parked) {
                return Err(TransitionError::Parked { id });
            }
            Self::transition_locked(&mut s, id, State::Claimed, agent_id, None)?;
            let it = s.items.get_mut(&id).unwrap();
            it.parked = false;
            it.awaiting_dispatch = false;
            it.rebase_requested = false;
            it.run_deadline_at = Some(deadline);
            it.lease = Some(Lease {
                agent_id: agent_id.to_string(),
                granted_at: now,
                last_heartbeat: now,
                expires_at: deadline,
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

        let ctx = self.claim_plan_context(id, &item);

        Ok(ClaimGrant {
            item_id: id,
            title: item.title.clone(),
            definition_of_done: item.definition_of_done.clone(),
            beads_id: item.beads_id.clone(),
            project_title: ctx.project_title,
            project_prompt: ctx.project_prompt,
            plan_summary: ctx.plan_summary,
            plan_tasks: ctx.plan_tasks,
            plan_task_key: ctx.plan_task_key,
            notes: item.notes.iter().map(|n| n.text.clone()).collect(),
            lease_expires_at: deadline,
            run_deadline_at: deadline,
            budget_remaining_cents: item.budget_cents.map(|b| b.saturating_sub(item.cost_cents)),
            engine: item.engine.clone(),
        })
    }

    /// Resolve Project prompt + Plan rows for the card being claimed.
    /// Plan rows come from the Initial plan card's (frozen) proposal.
    fn claim_plan_context(&self, id: ItemId, item: &WorkItem) -> ClaimPlanContext {
        let project = if item.is_project() {
            Some(item.clone())
        } else {
            item.parent.and_then(|pid| self.get(pid))
        };
        let Some(project) = project else {
            return ClaimPlanContext::default();
        };
        let project_title = Some(project.title.clone());
        let project_prompt = project.project_prompt.clone();

        let seed = self.initial_plan_of(project.id);
        let proposal = seed.as_ref().and_then(|s| s.proposal.as_ref());
        let Some(proposal) = proposal.filter(|p| !p.tasks.is_empty()) else {
            return ClaimPlanContext {
                project_title,
                project_prompt,
                ..Default::default()
            };
        };

        let plan_summary = if proposal.summary.trim().is_empty() {
            None
        } else {
            Some(proposal.summary.clone())
        };
        let title_key = Self::normalize_title(&item.title);
        let mut plan_task_key = None;
        let plan_tasks: Vec<crate::model::PlanTaskBrief> = proposal
            .tasks
            .iter()
            .map(|t| {
                let current = t.item_id == Some(id)
                    || (t.item_id.is_none()
                        && Self::normalize_title(&t.title) == title_key);
                if current {
                    plan_task_key = Some(t.key.clone());
                }
                crate::model::PlanTaskBrief {
                    key: t.key.clone(),
                    title: t.title.clone(),
                    intent: t.intent.clone(),
                    definition_of_done: t.definition_of_done.clone(),
                    blocked_by_keys: t.blocked_by_keys.clone(),
                    current,
                }
            })
            .collect();
        ClaimPlanContext {
            project_title,
            project_prompt,
            plan_summary,
            plan_tasks,
            plan_task_key,
        }
    }

    /// `heartbeat` — cost (and optional progress) only. Does **not** extend
    /// `run_deadline_at`. `lease_secs` is ignored (kept for MCP compatibility).
    pub fn heartbeat(
        &self,
        id: ItemId,
        agent_id: &str,
        progress: f32,
        cost_delta_cents: u64,
        _lease_secs: i64,
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
                // Do not touch expires_at / run_deadline_at — one fixed timeout.
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
    children: &[crate::model::SplitChildSpec],
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

    for child in children {
        let mut child_text = String::new();
        child_text.push_str(&child.title);
        child_text.push(' ');
        child_text.push_str(&child.intent);
        child_text.push(' ');
        child_text.push_str(&child.definition_of_done);

        let child_tokens = Self::tokenize_text(&child_text);

        if !Self::child_is_related(&child_tokens, &theme_tokens) {
            return Err(format!(
                "split child '{}' does not relate to parent card or project theme",
                child.title
            ));
        }
    }

    Ok(())
}

    /// Validate children and park them on the card as a proposal in Review.
    /// Does **not** create sibling Tasks — Approve materializes them.
    ///
    /// PR and proposal are mutually exclusive. Optional `key` / `blocked_by_keys`
    /// match the Plan task shape.
    pub fn propose_split(
        &self,
        id: ItemId,
        agent_id: &str,
        children: Vec<crate::model::SplitChildSpec>,
        max_children: usize,
    ) -> Result<WorkItem, String> {
        let card = self.get(id).ok_or("no such item")?;

        if card.is_initial_plan_task() {
            return Err(
                "Initial plan cannot use split.json; finish with plan.json + report.json + \
                 plan/docs PR (Review); Approve materializes sibling Tasks"
                    .into(),
            );
        }

        if let Some(ref pr_url) = card.pr_url.as_ref().filter(|s| !s.trim().is_empty()) {
            let msg = format!(
                "cannot propose split on card #{id}: a PR already exists ({pr_url}); \
                 split and publish are mutually exclusive"
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

        let mut seen_req = HashSet::new();
        let mut specs = Vec::new();
        for (idx, child) in children.into_iter().enumerate() {
            let title_key = Self::normalize_title(&child.title);
            if title_key.is_empty() || !seen_req.insert(title_key) {
                continue;
            }
            let key = child
                .key
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| format!("s{}", idx + 1));
            specs.push(PlanTaskSpec {
                key,
                title: child.title,
                intent: child.intent,
                definition_of_done: child.definition_of_done,
                blocked_by_keys: child.blocked_by_keys,
                capability: None,
                item_id: None,
            });
        }
        if specs.len() < 2 {
            return Err(
                "a split needs at least two distinct sibling titles; use report if the work is one card"
                    .into(),
            );
        }

        let summary = format!("Split of «{}»", card.title);
        self.set_proposal(
            id,
            TaskProposal {
                summary,
                tasks: specs.clone(),
            },
        )?;

        let item = self
            .transition(
                id,
                State::Review,
                agent_id,
                Some("proposed sibling Tasks — awaiting Approve".into()),
            )
            .map_err(|e| e.to_string())?;

        self.story(
            project_id,
            format!(
                "{} proposed {} sibling Tasks — Approve to create them: {}.",
                card.title,
                specs.len(),
                specs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join(", ")
            ),
        );
        Ok(item)
    }

    /// Store a TaskProposal on a card (Initial plan or impl split). Does not transition.
    /// Refuses once Initial plan is Done (frozen).
    pub fn set_proposal(&self, id: ItemId, proposal: TaskProposal) -> Result<WorkItem, String> {
        if proposal.tasks.is_empty() {
            return Err("proposal needs at least one task".into());
        }
        for t in &proposal.tasks {
            if t.key.trim().is_empty() {
                return Err(format!("proposal task '{}' needs a key", t.title));
            }
            if t.definition_of_done.trim().is_empty() {
                return Err(format!("proposal task '{}' needs a definition of done", t.title));
            }
        }
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s
                .items
                .get_mut(&id)
                .ok_or_else(|| format!("no work item #{id}"))?;
            if it.is_initial_plan_task() && it.state.is_terminal() {
                return Err("Initial plan already accepted — proposal is frozen".into());
            }
            it.proposal = Some(proposal);
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Materialize `item.proposal` into sibling Tasks under the parent Project.
    /// Initial plan: keep proposal and stamp `item_id`s (freeze for briefings).
    /// Impl splits: clear the proposal. Does not transition the parent card.
    fn materialize_proposal(
        &self,
        id: ItemId,
        by: &str,
        origin: Origin,
    ) -> Result<Vec<WorkItem>, String> {
        let card = self.get(id).ok_or("no such item")?;
        let keep_proposal = card.is_initial_plan_task();
        let proposal = card
            .proposal
            .clone()
            .filter(|p| !p.tasks.is_empty())
            .ok_or_else(|| format!("card #{id} has no proposal to materialize"))?;
        let project_id = card.parent.ok_or_else(|| {
            "cannot materialize proposal on a Project root".to_string()
        })?;

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
        let mut key_to_id: BTreeMap<String, ItemId> = BTreeMap::new();
        for spec in &proposal.tasks {
            let title_key = Self::normalize_title(&spec.title);
            if let Some(existing) = existing_by_title.get(&title_key) {
                key_to_id.insert(spec.key.clone(), existing.id);
                made.push(existing.clone());
                continue;
            }
            let sibling = self.create(
                Some(project_id),
                spec.title.clone(),
                spec.intent.clone(),
                Some(spec.definition_of_done.clone()),
                origin.clone(),
                false,
                spec.capability.clone().or_else(|| card.capability.clone()),
            )?;
            self.transition(sibling.id, State::Shaping, by, Some("from proposal".into()))
                .map_err(|e| e.to_string())?;
            let sibling = self
                .transition(sibling.id, State::Backlog, by, Some("from proposal".into()))
                .map_err(|e| e.to_string())?;
            key_to_id.insert(spec.key.clone(), sibling.id);
            made.push(sibling);
        }

        for spec in &proposal.tasks {
            let Some(&sid) = key_to_id.get(&spec.key) else { continue };
            let blockers: Vec<ItemId> = spec
                .blocked_by_keys
                .iter()
                .filter_map(|k| key_to_id.get(k).copied())
                .collect();
            if !blockers.is_empty() {
                self.set_blocked_by(sid, blockers);
            }
        }

        {
            let mut s = self.state.write().unwrap();
            if let Some(it) = s.items.get_mut(&id) {
                if keep_proposal {
                    if let Some(prop) = it.proposal.as_mut() {
                        for t in prop.tasks.iter_mut() {
                            if let Some(&sid) = key_to_id.get(&t.key) {
                                t.item_id = Some(sid);
                            }
                        }
                    }
                } else {
                    it.proposal = None;
                }
                let snap = it.clone();
                drop(s);
                self.emit(&snap);
            }
        }

        Ok(made)
    }

    /// Create sibling Tasks from a frozen proposal when a card reaches Done.
    /// Idempotent: title-matched siblings are reused. No-op without a proposal.
    fn materialize_proposal_on_done(&self, id: ItemId, by: &str) -> Result<usize, String> {
        let card = match self.get(id) {
            Some(c) => c,
            None => return Ok(0),
        };
        let has_proposal = card
            .proposal
            .as_ref()
            .is_some_and(|p| !p.tasks.is_empty());
        if !has_proposal {
            return Ok(0);
        }
        // Already linked every proposal row to a sibling — nothing to do.
        if card
            .proposal
            .as_ref()
            .is_some_and(|p| p.tasks.iter().all(|t| t.item_id.is_some()))
        {
            return Ok(0);
        }

        let origin = if card.is_initial_plan_task() {
            Origin::Planner
        } else {
            Origin::Split { from: id }
        };
        let made = self.materialize_proposal(id, by, origin)?;

        if card.is_initial_plan_task() {
            if let Some(project_id) = card.parent {
                let mut s = self.state.write().unwrap();
                if let Some(p) = s.items.get_mut(&project_id) {
                    if p.plan.is_some() {
                        p.plan = None;
                        let snap = p.clone();
                        drop(s);
                        self.emit(&snap);
                    }
                }
            }
        }

        self.story(
            id,
            format!(
                "{} — {} Tasks created from proposal.",
                card.title,
                made.len()
            ),
        );
        Ok(made.len())
    }

    /// Heal: materialize a Done card's proposal if siblings were never created
    /// (e.g. merge-to-Done before materialize-on-Done shipped).
    pub fn materialize_pending_proposal(&self, id: ItemId) -> Result<Vec<WorkItem>, String> {
        let card = self
            .get(id)
            .ok_or_else(|| format!("no work item #{id}"))?;
        if card.state != State::Done {
            return Err(format!("card #{id} is {:?}; expected Done", card.state));
        }
        let origin = if card.is_initial_plan_task() {
            Origin::Planner
        } else {
            Origin::Split { from: id }
        };
        let made = self.materialize_proposal(id, "cockpit", origin)?;
        if !made.is_empty() {
            self.story(
                id,
                format!(
                    "{} — {} Tasks created from proposal (heal).",
                    card.title,
                    made.len()
                ),
            );
        }
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
            it.rebase_requested = false;
            it.awaiting_dispatch = false;
        }
        let item = self
            .transition(id, State::NeedsHuman, agent_id, Some(question.clone()))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{} is blocked: {question}", item.title));
        Ok(item)
    }

    /// `report` — agent finished and opened a PR; card goes to Review.
    ///
    /// Mechanical checks are CI on the PR, not a honr Verify column. `gates` is
    /// kept as optional agent-side notes only.
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
                    .map(|name| GateRun {
                        name,
                        status: GateStatus::Passed,
                        detail: Some("CI on the PR is the real gate".into()),
                    })
                    .collect();
            }
        }
        self.transition(id, State::Review, agent_id, None)
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
            Self::transition_locked(&mut s, id, State::Backlog, agent_id, Some(reason_str))?
        };
        self.emit(&item);
        if let Some(r) = reason {
            self.story(id, format!("{}: released ({r})", item.title));
        }
        Ok(item)
    }

    /// Requeue runs past their fixed `run_deadline_at` (agent timeout).
    pub fn sweep_leases(&self) -> Vec<ItemId> {
        let now = Utc::now();
        let expired: Vec<ItemId> = {
            let s = self.state.read().unwrap();
            s.items
                .values()
                .filter(|i| matches!(i.state, State::Claimed | State::Running))
                .filter(|i| {
                    i.run_deadline_at
                        .map(|d| now > d)
                        .or_else(|| i.lease.as_ref().map(|l| l.is_expired(now)))
                        .unwrap_or(false)
                })
                .map(|i| i.id)
                .collect()
        };
        for id in &expired {
            let title = self.get(*id).map(|i| i.title).unwrap_or_default();
            let _ = self.transition(
                *id,
                State::Backlog,
                "deadline-sweeper",
                Some("run deadline exceeded".into()),
            );
            self.story(
                *id,
                format!("{title}: run deadline exceeded; card requeued."),
            );
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
            it.last_conflict_files.clear();
            // The answer becomes standing context for whoever picks it up next.
            it.notes.push(Note {
                at: Utc::now(),
                author: "human".into(),
                text: format!("Decision: {choice}"),
            });
            it.title.clone()
        };
        let item = self
            .transition(id, State::Backlog, "human", Some(format!("answered: {choice}")))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{title}: unblocked — {choice}"));
        Ok(item)
    }

    /// Park — stop the agent, return the card to Backlog, keep sandbox + conversation,
    /// and hold the card until [`Self::unpark`]. Prefer this over halt when the
    /// run is wedged or needs a human nudge without amnesia.
    pub fn park(&self, id: ItemId, reason: Option<String>) -> Result<WorkItem, String> {
        let reason = reason.filter(|r| !r.trim().is_empty());
        let title = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            if let Some(ref r) = reason {
                it.notes.push(Note {
                    at: Utc::now(),
                    author: "human".into(),
                    text: format!("Parked: {r}"),
                });
            }
            it.title.clone()
        };
        self.transition(
            id,
            State::Backlog,
            "human",
            Some(reason.clone().unwrap_or_else(|| "parked".into())),
        )
        .map_err(|e| e.to_string())?;
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.parked = true;
            it.clone()
        };
        self.emit(&item);
        let session = item
            .conversation_id
            .as_deref()
            .map(|c| format!(" session {c} kept"))
            .unwrap_or_default();
        self.story(
            id,
            format!(
                "{title}: parked — agent stopped, sandbox kept;{session} \
                 unpark to resume."
            ),
        );
        Ok(item)
    }

    /// Clear the park hold and queue the card for the supervisor — same as Start.
    /// Park exists to pause without amnesia; making the human click Start again
    /// after Resume is just ceremony.
    pub fn unpark(&self, id: ItemId) -> Result<WorkItem, String> {
        {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            if it.state != State::Backlog {
                return Err("only a Backlog card can be resumed from park".into());
            }
            if !it.parked {
                return Err("that card is not parked".into());
            }
            it.parked = false;
        }
        let item = self.enqueue_dispatch(id)?;
        self.story(
            id,
            format!(
                "{}: unparked and queued — supervisor will resume{}.",
                item.title,
                if item.conversation_id.is_some() {
                    " the same conversation"
                } else {
                    ""
                }
            ),
        );
        Ok(item)
    }

    /// Halt — kill the agent, return the card to Backlog, discard the LLM session.
    /// Sandbox may still be kept for caches/inspection; the conversation is not.
    pub fn halt(&self, id: ItemId, reason: Option<String>) -> Result<WorkItem, String> {
        {
            let mut s = self.state.write().unwrap();
            if let Some(it) = s.items.get_mut(&id) {
                it.conversation_id = None;
                it.parked = false;
            }
        }
        let item = self
            .transition(id, State::Backlog, "human", reason.or(Some("halted".into())))
            .map_err(|e| e.to_string())?;
        self.story(
            item.id,
            format!("{}: halted — session discarded; next claim starts a new conversation.", item.title),
        );
        Ok(item)
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
                let is_initial = it.is_initial_plan_task();
                let has_gh_url = it.github_issue_url.is_some();
                let env = it.environment.clone();
                let os = self.openshell.clone().unwrap_or_default();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Some(env_name) = env {
                            let _ = os.delete(&env_name).await;
                        }
                        if let (Some(b), Some(bid)) = (beads, beads_id) {
                            if crate::beads::BeadsClient::is_real_id(&bid) {
                                let _ = b.close(&bid, Some("Deleted from honr board")).await;
                                let has_beads_gh_url = if is_initial && !has_gh_url {
                                    b.show(&bid).await.ok().and_then(|s| s.github_issue_url()).is_some()
                                } else {
                                    false
                                };
                                if !is_initial || has_gh_url || has_beads_gh_url {
                                    let _ = b.github_push(&[bid]).await;
                                }
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

        self.record_and_send(BoardEvent::Delete {
            seq: self.next_seq(),
            id,
        });
        Ok(())
    }

    pub fn approve_review(&self, id: ItemId) -> Result<WorkItem, String> {
        let item = self
            .get(id)
            .ok_or_else(|| format!("no work item #{id}"))?;

        let has_proposal = item
            .proposal
            .as_ref()
            .is_some_and(|p| !p.tasks.is_empty());

        if has_proposal {
            // With a plan/docs PR: stay in Review until merge → Done materializes
            // Tasks. Without a PR (rare): Done now, which materializes in transition.
            if item.pr_url.as_ref().is_some_and(|u| !u.trim().is_empty()) {
                self.story(
                    id,
                    format!(
                        "{} approved — waiting for GitHub merge to create Tasks ({}).",
                        item.title,
                        item.pr_url.as_deref().unwrap_or("")
                    ),
                );
                return self
                    .get(id)
                    .ok_or_else(|| format!("no work item #{id}"));
            }

            let done = self
                .transition(id, State::Done, "human", Some("proposal approved".into()))
                .map_err(|e| e.to_string())?;
            let n = done
                .proposal
                .as_ref()
                .map(|p| p.tasks.len())
                .unwrap_or(0);
            self.story(
                id,
                format!("{} approved — {} Tasks created (no PR).", done.title, n),
            );
            return Ok(done);
        }

        // Legacy: Initial plan with Project Plan awaiting but no card proposal.
        if item.is_initial_plan_task() {
            if let Some(parent) = item.parent {
                if let Some(project) = self.get(parent) {
                    let awaiting = project.plan.as_ref().is_some_and(|p| !p.tasks.is_empty());
                    if awaiting {
                        let published = self.approve_plan(parent)?;
                        let done = self
                            .get(id)
                            .ok_or_else(|| format!("no work item #{id}"))?;
                        self.story(
                            id,
                            format!(
                                "{} approved — {} Tasks created from legacy Project Plan.",
                                done.title,
                                published.len()
                            ),
                        );
                        return Ok(done);
                    }
                }
            }
        }

        // Impl cards with a PR stay in Review until GitHub merge completes them
        // (webhook → complete_for_merged_pr). Approve only means "looks good".
        if item.pr_url.as_ref().is_some_and(|u| !u.trim().is_empty()) {
            self.story(
                id,
                format!(
                    "{} approved — waiting for GitHub merge ({}).",
                    item.title,
                    item.pr_url.as_deref().unwrap_or("")
                ),
            );
            return self
                .get(id)
                .ok_or_else(|| format!("no work item #{id}"));
        }

        let item = self
            .transition(id, State::Done, "human", Some("approved".into()))
            .map_err(|e| e.to_string())?;
        self.story(id, format!("{} approved — no PR; marked Done.", item.title));
        Ok(item)
    }

    pub fn request_changes(&self, id: ItemId, note: String) -> Result<WorkItem, String> {
        self.steer(id, format!("Changes requested: {note}"))?;
        {
            let mut s = self.state.write().unwrap();
            if let Some(it) = s.items.get_mut(&id) {
                it.proposal = None;
            }
        }
        let item = self
            .transition(id, State::Backlog, "human", Some(format!("changes requested: {note}")))
            .map_err(|e| e.to_string())?;
        self.emit(&item);
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
        self.record_and_send(BoardEvent::Story {
            seq: self.next_seq(),
            goal,
            at: line.at.to_rfc3339(),
            text: line.text,
        });
    }

    #[allow(dead_code)]
    pub fn stories_for(&self, near: ItemId) -> Vec<StoryLine> {
        let s = self.state.read().unwrap();
        let goal = Self::goal_of(&s, near);
        s.stories.get(&goal).cloned().unwrap_or_default()
    }

    /// Notify connected subscribers that the main branch advanced (via push or PR merge).
    pub fn notify_main_advanced(&self, ref_name: &str, commit_sha: Option<String>) {
        tracing::info!("main advanced: ref={ref_name}, commit={commit_sha:?}");
        self.record_and_send(BoardEvent::MainAdvanced {
            seq: self.next_seq(),
            ref_name: ref_name.to_string(),
            commit_sha,
        });
        self.trigger_rebase_for_all_behind_siblings();
    }

    /// Identify open sibling PRs in Review that are behind main for a given item's parent.
    pub fn identify_behind_sibling_prs(&self, near_id: ItemId) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        let mut results = Vec::new();
        if let Some(item) = s.items.get(&near_id) {
            let parent_id = item.parent.unwrap_or(item.id);
            for child_id in s.items.values().filter(|i| i.parent == Some(parent_id)).map(|i| i.id) {
                if child_id == near_id {
                    continue;
                }
                if let Some(child) = s.items.get(&child_id) {
                    if child.state == State::Review
                        && child
                            .pr_url
                            .as_ref()
                            .is_some_and(|u| !u.trim().is_empty())
                    {
                        results.push(child.clone());
                    }
                }
            }
        }
        results.sort_by_key(|i| i.entered_state_at);
        results
    }

    /// Identify all open sibling PRs in Review that are behind main across the entire board.
    pub fn identify_all_behind_sibling_prs(&self) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        let mut results = Vec::new();
        let parents_with_done: std::collections::HashSet<ItemId> = s
            .items
            .values()
            .filter(|i| i.state == State::Done)
            .filter_map(|i| i.parent)
            .collect();

        for parent_id in parents_with_done {
            for child_id in s.items.values().filter(|i| i.parent == Some(parent_id)).map(|i| i.id) {
                if let Some(child) = s.items.get(&child_id) {
                    if child.state == State::Review
                        && child
                            .pr_url
                            .as_ref()
                            .is_some_and(|u| !u.trim().is_empty())
                    {
                        results.push(child.clone());
                    }
                }
            }
        }
        results.sort_by_key(|i| i.entered_state_at);
        results.dedup_by_key(|i| i.id);
        results
    }

    /// Dispatch/queue a rebase request for a card in Review whose branch is behind main.
    pub fn dispatch_rebase(&self, id: ItemId) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item {id}"))?;
            if it.state != State::Review {
                return Err(format!(
                    "only Review cards can be rebased, #{id} is in {:?}",
                    it.state
                ));
            }
            if it.pr_url.as_ref().is_none_or(|u| u.trim().is_empty()) {
                return Err(format!("card #{id} has no PR URL to rebase"));
            }
            it.rebase_requested = true;
            it.awaiting_dispatch = true;
            let mut out = it.clone();
            Self::populate_blockers(&s, &mut out);
            out
        };
        self.emit(&item);
        self.story(
            id,
            format!("{}: queued rebase request — branch is behind main.", item.title),
        );
        Ok(item)
    }

    /// Identify sibling PRs in Review behind main for `near_id`'s parent and dispatch rebase requests.
    pub fn trigger_rebase_for_behind_siblings(&self, near_id: ItemId) -> Vec<WorkItem> {
        let siblings = self.identify_behind_sibling_prs(near_id);
        let mut dispatched = Vec::new();
        for s in siblings {
            if let Ok(item) = self.dispatch_rebase(s.id) {
                dispatched.push(item);
            }
        }
        dispatched
    }

    /// Identify all sibling PRs in Review behind main across the board and dispatch rebase requests.
    pub fn trigger_rebase_for_all_behind_siblings(&self) -> Vec<WorkItem> {
        let siblings = self.identify_all_behind_sibling_prs();
        let mut dispatched = Vec::new();
        for s in siblings {
            if let Ok(item) = self.dispatch_rebase(s.id) {
                dispatched.push(item);
            }
        }
        dispatched
    }

    /// List all cards in Review that have a pending rebase request.
    #[allow(dead_code)]
    pub fn list_awaiting_rebase(&self) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        let mut items: Vec<_> = s
            .items
            .values()
            .filter(|i| {
                i.state == State::Review
                    && (i.rebase_requested || i.awaiting_dispatch)
                    && !i.parked
            })
            .cloned()
            .collect();
        items.sort_by_key(|i| i.entered_state_at);
        items
    }

    /// Record the outcome of a rebase operation for a card in Review.
    ///
    /// If Clean: the card remains in Review, and `rebase_requested` & `awaiting_dispatch` are cleared.
    /// If Conflict: the card transitions to Backlog with `last_bounce_reason` set containing
    /// the failure reason and conflicting file details, and `rebase_requested` & `awaiting_dispatch` are cleared.
    pub fn record_rebase_outcome(&self, id: ItemId, outcome: RebaseOutcome) -> Result<WorkItem, String> {
        let (title, previous_files) = {
            let s = self.state.read().unwrap();
            let it = s.items.get(&id).ok_or_else(|| format!("no such item #{id}"))?;
            if it.state != State::Review {
                return Err(format!("only Review cards can record rebase outcome, #{id} is in {:?}", it.state));
            }
            (it.title.clone(), it.last_conflict_files.clone())
        };

        match outcome {
            RebaseOutcome::Clean => {
                let item = {
                    let mut s = self.state.write().unwrap();
                    let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item #{id}"))?;
                    it.rebase_requested = false;
                    it.awaiting_dispatch = false;
                    it.last_conflict_files.clear();
                    let mut out = it.clone();
                    Self::populate_blockers(&s, &mut out);
                    out
                };
                self.emit(&item);
                self.story(id, format!("{title}: rebase clean — retained in Review."));
                Ok(item)
            }
            RebaseOutcome::Conflict { conflicting_files, reason } => {
                let curr_files: Vec<String> = conflicting_files.iter().map(|f| f.trim().to_string()).collect();
                let has_overlap = !curr_files.is_empty()
                    && !previous_files.is_empty()
                    && curr_files.iter().any(|f| previous_files.contains(f));

                if has_overlap {
                    let overlapping: Vec<String> = curr_files
                        .iter()
                        .filter(|f| previous_files.contains(f))
                        .cloned()
                        .collect();

                    let base_reason = reason.unwrap_or_else(|| "git rebase conflict".to_string());
                    let bounce_reason = format!(
                        "{base_reason}: decomposition failure: repeated conflict on overlapping files: {}",
                        overlapping.join(", ")
                    );

                    {
                        let mut s = self.state.write().unwrap();
                        if let Some(it) = s.items.get_mut(&id) {
                            it.last_bounce_reason = Some(bounce_reason.clone());
                            it.last_conflict_files = curr_files;
                        }
                    }

                    let question = format!(
                        "Decomposition failure: repeated rebase conflict on overlapping files ({}) for card #{id}",
                        overlapping.join(", ")
                    );

                    let options = vec![
                        EscalationOption {
                            label: "Re-split tasks to isolate overlapping files".into(),
                            detail: "Return card to Shaping/Backlog or re-split the task so overlapping file boundaries are separated.".into(),
                        },
                        EscalationOption {
                            label: "Manually resolve conflict and approve".into(),
                            detail: "Manually rebase or merge the PR branch onto main and approve.".into(),
                        },
                        EscalationOption {
                            label: "Retire card".into(),
                            detail: "Cut scope and retire this card if the conflict cannot be resolved.".into(),
                        },
                    ];

                    self.escalate(id, "rebase", question, options, 0)
                } else {
                    let base_reason = reason.unwrap_or_else(|| "git rebase conflict".to_string());
                    let bounce_reason = if curr_files.is_empty() {
                        base_reason
                    } else {
                        format!("{base_reason}: conflicting files: {}", curr_files.join(", "))
                    };

                    let item = {
                        let mut s = self.state.write().unwrap();
                        if let Some(it) = s.items.get_mut(&id) {
                            it.last_bounce_reason = Some(bounce_reason.clone());
                            it.last_conflict_files = curr_files;
                        }
                        Self::transition_locked(&mut s, id, State::Backlog, "rebase", Some(bounce_reason.clone()))
                            .map_err(|e| format!("failed transition to Backlog on rebase conflict: {e}"))?;
                        let it_mut = s.items.get_mut(&id).unwrap();
                        it_mut.rebase_requested = false;
                        it_mut.awaiting_dispatch = false;
                        let mut out = it_mut.clone();
                        Self::populate_blockers(&s, &mut out);
                        out
                    };
                    self.emit(&item);
                    self.story(id, format!("{title}: rebase conflict — returned to Backlog ({bounce_reason})."));
                    Ok(item)
                }
            }
        }
    }

    /// Convenience wrapper to record a clean rebase for a card in Review.
    pub fn complete_rebase_clean(&self, id: ItemId) -> Result<WorkItem, String> {
        self.record_rebase_outcome(id, RebaseOutcome::Clean)
    }

    /// Convenience wrapper to record a rebase conflict for a card in Review.
    pub fn complete_rebase_conflict(
        &self,
        id: ItemId,
        conflicting_files: &[String],
        reason: Option<&str>,
    ) -> Result<WorkItem, String> {
        self.record_rebase_outcome(
            id,
            RebaseOutcome::Conflict {
                conflicting_files: conflicting_files.to_vec(),
                reason: reason.map(|r| r.to_string()),
            },
        )
    }


    /// Normalize a GitHub PR URL for matching board `pr_url` values.
    pub fn normalize_pr_url(url: &str) -> String {
        url.trim().trim_end_matches('/').to_ascii_lowercase()
    }

    /// When a PR merges on GitHub, complete the matching Review/NeedsHuman card.
    /// Done triggers the usual beads close + github_push (linked Issue close).
    /// Returns the completed item id, or `None` if no eligible card matched.
    pub fn complete_for_merged_pr(
        &self,
        pr_url: &str,
        pr_number: Option<u64>,
    ) -> Option<ItemId> {
        let needle = Self::normalize_pr_url(pr_url);
        if needle.is_empty() {
            return None;
        }

        let id = {
            let s = self.state.read().unwrap();
            s.items
                .values()
                .find(|i| {
                    matches!(i.state, State::Review | State::NeedsHuman)
                        && i.pr_url
                            .as_deref()
                            .is_some_and(|u| Self::normalize_pr_url(u) == needle)
                })
                .map(|i| i.id)?
        };

        let reason = match pr_number {
            Some(n) => format!("PR merged (#{n})"),
            None => "PR merged".into(),
        };
        match self.transition(id, State::Done, "github-webhook", Some(reason)) {
            Ok(item) => {
                self.story(id, format!("{} — PR merged; card Done.", item.title));
                self.trigger_rebase_for_behind_siblings(id);
                Some(id)
            }
            Err(e) => {
                tracing::warn!(id, error = %e, "complete_for_merged_pr transition failed");
                None
            }
        }
    }

    // -------------------------------------------------------- derived reads

    /// Returns active (non-terminal) siblings of `id` (sharing the same parent)
    /// that were blocked by `id` and have now become unblocked (0 unresolved blockers).
    pub fn newly_unblocked_siblings(&self, id: ItemId) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        let Some(item) = s.items.get(&id) else {
            return vec![];
        };
        s.items
            .values()
            .filter(|other| {
                other.id != id
                    && other.parent == item.parent
                    && !other.state.is_terminal()
                    && other.blocked_by.contains(&id)
                    && Self::unresolved_blockers(&s, other).is_empty()
            })
            .cloned()
            .collect()
    }

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
            agent_timeout_secs: self.schema.execution.agents.agent_timeout_secs,
            seq: self.seq.load(Ordering::Relaxed),
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
        let archived = goal.state == State::Retired;

        // Tasks under this Project only — the Project itself is never a Board card.
        let members: Vec<&WorkItem> = s.items.values().filter(|i| i.parent == Some(gid)).collect();

        // Active projects: retired leaves are out of scope (cut duplicates must
        // not inflate the bar). Archived projects: cut_scope retires the whole
        // subtree, so count every leaf — the scope is closed.
        let leaves: Vec<&&WorkItem> = members
            .iter()
            .filter(|i| archived || i.state != State::Retired)
            .filter(|i| !Self::has_children(s, i.id))
            .collect();
        let leaves_total = leaves.len();
        let leaves_done = if archived {
            leaves_total
        } else {
            leaves.iter().filter(|i| i.state == State::Done).count()
        };

        let spend_cents = members.iter().map(|i| i.cost_cents).sum();
        let agents_live = members
            .iter()
            .filter(|i| matches!(i.state, State::Claimed | State::Running | State::Splitting))
            .count();
        let needs_you = members.iter().filter(|i| i.state == State::NeedsHuman).count();

        let mut columns = Vec::new();
        for column in [
            Column::Backlog,
            Column::Running,
            Column::NeedsYou,
            Column::Review,
            Column::Done,
        ] {
            let in_col: Vec<&&WorkItem> =
                members.iter().filter(|i| i.state.column() == column).collect();
            columns.push(ColumnView {
                column,
                summary: Self::chunk(
                    column,
                    &in_col,
                    s,
                    now,
                    self.schema.execution.agents.agent_timeout_secs,
                ),
            });
        }

        let plan_status = self.plan_status_label(gid);

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
            archived,
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
        agent_timeout_secs: u64,
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
            Column::Backlog => {
                // Waiting for cockpit to dispatch — not a claim queue.
                let blocked: Vec<&&&WorkItem> = items
                    .iter()
                    .filter(|i| !Self::unresolved_blockers(s, i).is_empty())
                    .collect();
                let mut parts = vec![format!("{count} in backlog")];
                if !blocked.is_empty() {
                    let mut blocker_ids: Vec<ItemId> = blocked
                        .iter()
                        .flat_map(|i| Self::unresolved_blockers(s, i))
                        .collect();
                    blocker_ids.sort_unstable();
                    blocker_ids.dedup();

                    let on: Vec<String> = blocker_ids
                        .into_iter()
                        .map(|bid| {
                            if let Some(b_item) = s.items.get(&bid) {
                                let label = match &b_item.beads_id {
                                    Some(b) if !b.starts_with("bd-honr-") => b.clone(),
                                    _ => format!("#{bid}"),
                                };
                                let title = b_item.title.trim();
                                if title.is_empty() {
                                    label
                                } else {
                                    format!("{label}: {title}")
                                }
                            } else {
                                format!("#{bid}")
                            }
                        })
                        .collect();
                    parts.push(format!("{} blocked on {}", blocked.len(), on.join(", ")));
                }
                parts.push(format!("oldest {}", humanize(oldest)));
                parts.join(" · ")
            }
            Column::Running => {
                // Near deadline = remaining under 10% of the agent timeout.
                let warn_secs = (agent_timeout_secs as i64).max(1) / 10;
                let ending_soon = items
                    .iter()
                    .filter(|i| {
                        i.run_deadline_at
                            .or_else(|| i.lease.as_ref().map(|l| l.expires_at))
                            .map(|d| (d - now).num_seconds() <= warn_secs)
                            .unwrap_or(true)
                    })
                    .count();
                let spend: u64 = items.iter().map(|i| i.cost_cents).sum();
                if ending_soon == 0 {
                    format!("{count} running · ${:.2} so far", spend as f64 / 100.0)
                } else {
                    format!(
                        "{count} running · {ending_soon} ending soon · ${:.2} so far",
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
            Column::Review => {
                // Can I approve this in 30 seconds? CI is on the PR, not here.
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
                let warn_secs =
                    (self.schema.execution.agents.agent_timeout_secs as i64).max(1) / 10;

                let mut ready_items: Vec<&&WorkItem> = members
                    .iter()
                    .filter(|i| i.state == State::Backlog)
                    .filter(|i| i.level.as_deref() != Some("Project"))
                    .filter(|i| !Self::has_children(&s, i.id))
                    .filter(|i| Self::unresolved_blockers(&s, i).is_empty())
                    .filter(|i| !i.parked)
                    .filter(|i| !i.awaiting_dispatch)
                    .collect();
                ready_items.sort_by_key(|i| i.entered_state_at);
                let ready_to_dispatch = ready_items
                    .into_iter()
                    .map(|i| ReadyCard {
                        id: i.id,
                        title: i.title.clone(),
                    })
                    .collect();

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
                            i.run_deadline_at
                                .or_else(|| i.lease.as_ref().map(|l| l.expires_at))
                                .map(|d| (d - now).num_seconds() <= warn_secs)
                                .unwrap_or(true)
                        })
                        .count(),
                    backlog: members.iter().filter(|i| i.state == State::Backlog).count(),
                    in_review: members.iter().filter(|i| i.state == State::Review).count(),
                    ready_to_dispatch,
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

    /// Poll until `pred` succeeds. Prefer this over multi-second sleep loops —
    /// memory beads + Capture remotes usually settle within a few yields.
    async fn eventually(mut pred: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if pred() {
                return;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    async fn eventually_async<F, Fut>(mut pred: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if pred().await {
                return;
            }
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    /// A board with one leaf sitting in Backlog, claimed by `agent`.
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
        let _ = b.transition(leaf.id, State::Backlog, "t", None);
        b.claim(leaf.id, "agent", None, 45).expect("claim");
        (b, leaf.id)
    }

    #[test]
    fn claim_sets_run_deadline_from_timeout() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-deadline.json"));
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
        let _ = b.transition(leaf.id, State::Backlog, "t", None);
        let before = Utc::now();
        let grant = b.claim(leaf.id, "agent", None, 1800).expect("claim");
        let after = Utc::now();
        let item = b.get(leaf.id).unwrap();
        let deadline = item.run_deadline_at.expect("run_deadline_at set");
        assert_eq!(grant.run_deadline_at, deadline);
        assert_eq!(grant.lease_expires_at, deadline);
        assert!(deadline >= before + Duration::seconds(1799));
        assert!(deadline <= after + Duration::seconds(1801));
    }

    #[test]
    fn heartbeat_does_not_extend_run_deadline() {
        let (b, id) = claimed_leaf();
        let original = b.get(id).unwrap().run_deadline_at.expect("deadline");
        b.heartbeat(id, "agent", 0.5, 10, 9999).expect("heartbeat");
        let after = b.get(id).unwrap();
        assert_eq!(after.run_deadline_at, Some(original));
        assert_eq!(
            after.lease.as_ref().map(|l| l.expires_at),
            Some(original),
            "lease.expires_at must stay pinned to the claim deadline"
        );
        assert_eq!(after.cost_cents, 10);
        assert_eq!(after.state, State::Running);
    }

    #[test]
    fn sweep_requeues_past_run_deadline() {
        let (b, id) = claimed_leaf();
        {
            let mut s = b.state.write().unwrap();
            let it = s.items.get_mut(&id).unwrap();
            it.run_deadline_at = Some(Utc::now() - Duration::seconds(1));
            if let Some(l) = it.lease.as_mut() {
                l.expires_at = Utc::now() - Duration::seconds(1);
            }
        }
        let expired = b.sweep_leases();
        assert_eq!(expired, vec![id]);
        assert_eq!(b.get(id).unwrap().state, State::Backlog);
        assert!(b.get(id).unwrap().run_deadline_at.is_none());
    }

    #[test]
    fn park_keeps_conversation_and_environment() {
        let (b, id) = claimed_leaf();
        b.set_environment(id, Some("honr-card-1-a1".into()));
        b.set_conversation_id(id, Some("conv-xyz".into()));
        let it = b.park(id, Some("wedged on cargo".into())).expect("park");
        assert_eq!(it.state, State::Backlog);
        assert_eq!(it.environment.as_deref(), Some("honr-card-1-a1"));
        assert_eq!(it.conversation_id.as_deref(), Some("conv-xyz"));
        assert!(it.parked, "park must hold the card from reclaim");
        assert!(!b.may_claim(id), "parked card must not be claimable");
        assert!(
            it.notes.iter().any(|n| n.text.contains("Parked: wedged on cargo")),
            "park reason must become a resume note: {:?}",
            it.notes
        );
        let resumed = b.unpark(id).expect("unpark");
        assert!(!resumed.parked);
        assert!(
            resumed.awaiting_dispatch,
            "unpark should queue the supervisor (same as Start)"
        );
        assert!(b.may_claim(id), "unpark restores claimability");
    }

    #[test]
    fn enqueue_dispatch_marks_card_for_supervisor() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-dispatch.json"));
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
        let _ = b.transition(leaf.id, State::Backlog, "t", None);
        assert!(b.list_awaiting_dispatch().is_empty());
        let it = b.enqueue_dispatch(leaf.id).expect("enqueue");
        assert!(it.awaiting_dispatch);
        assert_eq!(b.list_awaiting_dispatch().len(), 1);
        b.claim(leaf.id, "agent", None, 45).expect("claim");
        assert!(
            !b.get(leaf.id).unwrap().awaiting_dispatch,
            "claim clears the dispatch flag"
        );
        b.halt(leaf.id, None).expect("halt");
        assert!(
            !b.get(leaf.id).unwrap().awaiting_dispatch,
            "halt bounce clears dispatch"
        );
        assert!(b.list_awaiting_dispatch().is_empty());
    }

    #[test]
    fn halt_clears_conversation_keeps_environment() {
        let (b, id) = claimed_leaf();
        b.set_environment(id, Some("honr-card-1-a1".into()));
        b.set_conversation_id(id, Some("conv-xyz".into()));
        let it = b.halt(id, Some("start over".into())).expect("halt");
        assert_eq!(it.state, State::Backlog);
        assert_eq!(it.environment.as_deref(), Some("honr-card-1-a1"));
        assert!(it.conversation_id.is_none(), "halt discards the LLM session");
        assert!(!it.parked);
    }

    /// Failures under the cap requeue, so a transient problem self-heals.
    #[test]
    fn early_failures_requeue_while_budget_remains() {
        let (b, id) = claimed_leaf();
        let it = b.record_run_failure(id, "sandbox would not start", 3).expect("recorded");
        assert_eq!(it.state, State::Backlog);
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
        assert_eq!(it.state, State::Backlog);
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
    fn propose_split_goes_to_review_approve_creates_siblings() {
        let (b, id) = claimed_leaf();
        let project_id = b.get(id).unwrap().parent.expect("task under project");
        let _ = b.transition(id, State::Running, "agent", None);
        let children = vec![
            SplitChildSpec::new("Leaf part 1", "Do leaf part 1", "Leaf part 1 done"),
            SplitChildSpec::new("Leaf part 2", "Do leaf part 2", "Leaf part 2 done"),
        ];
        let card = b
            .propose_split(id, "agent", children, 5)
            .expect("propose_split should succeed");
        assert_eq!(card.state, State::Review);
        assert_eq!(card.proposal.as_ref().unwrap().tasks.len(), 2);
        assert_eq!(
            b.children_of(project_id)
                .into_iter()
                .filter(|&cid| cid != id)
                .filter(|&cid| !b.get(cid).unwrap().is_initial_plan_task())
                .count(),
            0,
            "no siblings before Approve"
        );

        let done = b.approve_review(id).expect("approve");
        assert_eq!(done.state, State::Done);
        assert!(done.proposal.is_none());
        let siblings: Vec<_> = b
            .children_of(project_id)
            .into_iter()
            .filter_map(|cid| b.get(cid))
            .filter(|i| !i.is_initial_plan_task() && i.id != id)
            .collect();
        assert_eq!(siblings.len(), 2);
        assert!(siblings.iter().all(|s| s.state == State::Backlog));
    }

    #[test]
    fn propose_split_accepts_on_theme_children() {
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
        let _ = b.transition(task.id, State::Backlog, "t", None);
        let _ = b.claim(task.id, "agent", None, 60).expect("claim");
        let _ = b.transition(task.id, State::Running, "agent", None);

        let children = vec![
            SplitChildSpec::new("Google OAuth login endpoint", "Add endpoint for google auth callback", "Google auth done"),
            SplitChildSpec::new("GitHub OAuth token exchange", "Exchange code for github access token", "GitHub auth done"),
        ];

        let card = b
            .propose_split(task.id, "agent", children, 5)
            .expect("on-theme propose_split should succeed");
        assert_eq!(card.state, State::Review);
        assert_eq!(card.proposal.as_ref().unwrap().tasks.len(), 2);
    }

    #[test]
    fn propose_split_rejects_off_theme_children() {
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
        let _ = b.transition(task.id, State::Backlog, "t", None);
        let _ = b.claim(task.id, "agent", None, 60).expect("claim");
        let _ = b.transition(task.id, State::Running, "agent", None);

        let children = vec![
            SplitChildSpec::new("Google OAuth login endpoint", "Add endpoint for google auth callback", "Google auth done"),
            SplitChildSpec::new("Database connection pool", "Optimize postgres max connection limit", "DB config done"),
        ];

        let err = b.propose_split(task.id, "agent", children, 5).unwrap_err();
        assert!(err.contains("does not relate to parent card or project theme"), "got error: {err}");
        assert_eq!(b.get(task.id).unwrap().state, State::Running);
    }

    #[test]
    fn propose_split_refused_below_minimum_siblings() {
        let (b, id) = claimed_leaf();
        let _ = b.transition(id, State::Running, "agent", None);
        let children = vec![SplitChildSpec::new("Single", "Only one", "Done")];
        let err = b.propose_split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("at least two siblings"), "got error: {err}");
    }

    #[test]
    fn propose_split_refused_exceeding_fanout_governor() {
        let (b, id) = claimed_leaf();
        let _ = b.transition(id, State::Running, "agent", None);
        let children: Vec<_> = (1..=6)
            .map(|i| SplitChildSpec::new(format!("Child {i}"), format!("Intent {i}"), format!("DoD {i}")))
            .collect();
        let err = b.propose_split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("exceeds max_children_per_split=5"), "got error: {err}");
    }

    #[test]
    fn propose_split_refused_on_project_root() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-split-root.json"));
        let project = b
            .create(None, "proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let err = b
            .propose_split(
                project.id,
                "agent",
                vec![
                    SplitChildSpec::new("A", "a", "done"),
                    SplitChildSpec::new("B", "b", "done"),
                ],
                5,
            )
            .unwrap_err();
        assert!(err.contains("cannot split a Project"), "got error: {err}");
    }

    #[test]
    fn propose_split_refused_when_pr_exists() {
        let (b, id) = claimed_leaf();
        b.set_pr_url(id, Some("https://github.com/shanemcd/honr/pull/42".to_string()));
        let children = vec![
            SplitChildSpec::new("Part 1", "Do part 1", "Part 1 done"),
            SplitChildSpec::new("Part 2", "Do part 2", "Part 2 done"),
        ];
        let err = b.propose_split(id, "agent", children, 5).unwrap_err();
        assert!(err.contains("a PR already exists"), "got error: {err}");

        let item = b.get(id).expect("item exists");
        assert_eq!(item.state, State::NeedsHuman);
        assert!(item.escalation.is_some(), "escalation must be populated");
        let esc = item.escalation.unwrap();
        assert!(esc.question.contains("a PR already exists"));
        assert_eq!(esc.options.len(), 2);
    }

    #[test]
    fn initial_plan_refuses_propose_split() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-initial-plan-no-split.json"),
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
        let _ = b.claim(seed_id, "agent", None, 60).expect("claim initial plan");
        let _ = b.transition(seed_id, State::Running, "agent", None);

        let err = b
            .propose_split(
                seed_id,
                "agent",
                vec![
                    SplitChildSpec::new(
                        "API archive endpoint",
                        "Expose archive for board cards",
                        "Archive API works",
                    ),
                    SplitChildSpec::new(
                        "UI archive controls",
                        "Add archive actions in the board UI",
                        "Archive UI works",
                    ),
                ],
                5,
            )
            .unwrap_err();
        assert!(
            err.contains("Initial plan cannot") || err.contains("plan.json"),
            "got: {err}"
        );
        assert_eq!(b.get(seed_id).unwrap().state, State::Running);
    }

    #[test]
    fn approve_split_proposal_wires_blocked_by_keys() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-split-deps.json"),
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
        let task = b
            .create(
                Some(project.id),
                "Archive feature",
                "Archive completed work from the board UI",
                Some("Archive works".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("task");
        let _ = b.transition(task.id, State::Shaping, "t", None);
        let _ = b.transition(task.id, State::Backlog, "t", None);
        let _ = b.claim(task.id, "agent", None, 60).expect("claim");
        let _ = b.transition(task.id, State::Running, "agent", None);

        b.propose_split(
            task.id,
            "agent",
            vec![
                SplitChildSpec::new(
                    "API archive endpoint",
                    "Expose archive for board cards",
                    "Archive API works",
                )
                .with_deps("a", vec![]),
                SplitChildSpec::new(
                    "UI archive controls",
                    "Add archive actions in the board UI",
                    "Archive UI works",
                )
                .with_deps("b", vec!["a".into()]),
            ],
            5,
        )
        .expect("propose");
        b.approve_review(task.id).expect("approve");
        let siblings: Vec<_> = b
            .children_of(project.id)
            .into_iter()
            .filter_map(|cid| b.get(cid))
            .filter(|i| !i.is_initial_plan_task() && i.id != task.id)
            .collect();
        assert_eq!(siblings.len(), 2);
        let a = siblings.iter().find(|m| m.title.contains("API")).unwrap();
        let bb = siblings.iter().find(|m| m.title.contains("UI")).unwrap();
        assert_eq!(b.get(bb.id).unwrap().blocked_by, vec![a.id]);
    }

    #[test]
    fn approve_split_reuses_existing_siblings_by_title() {
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
        let card = b
            .create(
                Some(project.id),
                "Archive feature",
                "Archive completed work from the board",
                Some("Archive works".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("card");
        let _ = b.transition(card.id, State::Shaping, "t", None);
        let _ = b.transition(card.id, State::Backlog, "t", None);

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
            .transition(preexisting.id, State::Backlog, "t", None)
            .expect("ready");

        let before = b.children_of(project.id).len();
        let _ = b.claim(card.id, "agent", None, 60).expect("claim");
        let _ = b.transition(card.id, State::Running, "agent", None);
        b.propose_split(
            card.id,
            "agent",
            vec![
                SplitChildSpec::new(
                    "API archive endpoint",
                    "Expose archive for board cards",
                    "Archive API works",
                ),
                SplitChildSpec::new(
                    "UI archive controls",
                    "Add archive actions in the board UI",
                    "Archive UI works",
                ),
            ],
            5,
        )
        .expect("propose");
        b.approve_review(card.id).expect("approve");
        let siblings: Vec<_> = b
            .children_of(project.id)
            .into_iter()
            .filter_map(|cid| b.get(cid))
            .filter(|i| !i.is_initial_plan_task() && i.id != card.id)
            .collect();
        assert_eq!(siblings.len(), 2);
        assert!(
            siblings.iter().any(|s| s.id == preexisting.id),
            "matching title must be reused"
        );
        assert_eq!(
            b.children_of(project.id).len(),
            before + 1,
            "only the missing sibling should be created"
        );
    }

    #[test]
    fn request_changes_clears_proposal() {
        let (b, id) = claimed_leaf();
        let _ = b.transition(id, State::Running, "agent", None);
        b.propose_split(
            id,
            "agent",
            vec![
                SplitChildSpec::new("A", "a", "a done"),
                SplitChildSpec::new("B", "b", "b done"),
            ],
            5,
        )
        .expect("propose");
        assert!(b.get(id).unwrap().proposal.is_some());
        let item = b
            .request_changes(id, "narrow the split".into())
            .expect("request_changes");
        assert_eq!(item.state, State::Backlog);
        assert!(item.proposal.is_none());
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
    fn project_create_seeds_initial_plan_task() {
        let b = Board::new(Schema::default(), std::env::temp_dir().join("honr-test-seed-plan.json"));
        let project = b
            .create(None, "Phase X", "why", None, Origin::Human, true, None)
            .expect("project");
        assert!(project.plan.is_none(), "Plan lives on Initial plan, not Project");

        let kids = b.children_of(project.id);
        assert_eq!(kids.len(), 1, "exactly one seed Task");
        let seed = b.get(kids[0]).unwrap();
        assert_eq!(seed.title, initial_plan_title("Phase X"));
        assert_eq!(seed.state, State::Backlog, "Initial plan is dispatchable planning work");
        assert!(seed.is_initial_plan_task());
        assert!(b.may_claim(seed.id));
        assert!(
            b.list_ready(&["any".into()]).iter().any(|i| i.id == seed.id),
            "Initial plan must appear in list_ready"
        );
        // Project itself must not be Backlog / claimable.
        assert_ne!(b.get(project.id).unwrap().state, State::Backlog);
    }

    #[test]
    fn approve_plan_materializes_from_initial_plan_proposal() {
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

        let seed = b
            .children_of(project.id)
            .into_iter()
            .find_map(|id| b.get(id).filter(|i| i.is_initial_plan_task()))
            .expect("seed");
        assert!(seed.proposal.as_ref().is_some_and(|p| p.tasks.len() == 2));
        assert!(b.get(project.id).unwrap().plan.is_none());

        let published = b.approve_plan(project.id).expect("approve");
        assert_eq!(published.len(), 2);
        assert_ne!(b.get(project.id).unwrap().state, State::Backlog);
        assert!(b.get(project.id).unwrap().plan.is_none());

        let a = b.get(published[0]).unwrap();
        let b_item = b.get(published[1]).unwrap();
        assert_eq!(a.state, State::Backlog);
        assert_eq!(b_item.state, State::Backlog);
        assert_eq!(b_item.blocked_by, vec![published[0]]);

        let seed = b.get(seed.id).unwrap();
        assert!(seed.state.is_terminal());
        // Frozen proposal with stamped item_ids.
        let prop = seed.proposal.expect("frozen proposal");
        assert_eq!(prop.tasks.len(), 2);
        assert!(prop.tasks.iter().all(|t| t.item_id.is_some()));
    }

    #[tokio::test]
    async fn approve_plan_materialize_skips_sync_bd_create_and_heals_async() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-materialize-async-beads-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(crate::beads::RemoteCapture::new()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(
                None,
                "Async Materialize Project",
                "why",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("project");
        let _ = board.transition(project.id, State::Shaping, "t", None);

        // Simulate an in-flight dolt push / create storm — request path must not wait.
        beads_client.begin_create_storm();
        beads_client.schedule_dolt_push();

        let before_creates = beads_client.create_sync_call_count();
        assert_eq!(
            before_creates, 0,
            "Board::create must not call create_linked_sync"
        );

        board
            .propose_plan(
                project.id,
                "three tasks",
                vec![
                    PlanTaskSpec {
                        key: "a".into(),
                        title: "Task A".into(),
                        intent: "ia".into(),
                        definition_of_done: "da".into(),
                        blocked_by_keys: vec![],
                        capability: None,
                        item_id: None,
                    },
                    PlanTaskSpec {
                        key: "b".into(),
                        title: "Task B".into(),
                        intent: "ib".into(),
                        definition_of_done: "db".into(),
                        blocked_by_keys: vec!["a".into()],
                        capability: None,
                        item_id: None,
                    },
                    PlanTaskSpec {
                        key: "c".into(),
                        title: "Task C".into(),
                        intent: "ic".into(),
                        definition_of_done: "dc".into(),
                        blocked_by_keys: vec!["b".into()],
                        capability: None,
                        item_id: None,
                    },
                ],
                vec![],
            )
            .expect("propose");

        let started = std::time::Instant::now();
        let published = board.approve_plan(project.id).expect("approve");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "approve/materialize must return quickly even with dolt storm open; took {:?}",
            started.elapsed()
        );
        assert_eq!(
            beads_client.create_sync_call_count(),
            before_creates,
            "materialize must not call create_linked_sync on the request path"
        );
        assert_eq!(published.len(), 3);

        for &id in &published {
            let item = board.get(id).expect("sibling");
            assert_eq!(item.state, State::Backlog, "siblings land in Backlog, not draft");
            assert!(
                item.beads_id
                    .as_deref()
                    .is_some_and(|b| b.starts_with("bd-honr-")),
                "siblings keep placeholders until heal/mirror, got {:?}",
                item.beads_id
            );
        }

        beads_client.end_create_storm();

        let healed = board.heal_placeholder_beads_ids().await;
        assert!(
            healed >= 3,
            "heal should bind real beads ids for materialized siblings (and project/seed); got {healed}"
        );
        for &id in &published {
            let bid = board.get(id).and_then(|i| i.beads_id).expect("beads_id");
            assert!(
                crate::beads::BeadsClient::is_real_id(&bid),
                "sibling #{id} should have real beads_id after heal, got {bid}"
            );
        }

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn approve_plan_closes_initial_plan_in_review() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-approve-closes-review.json"),
        );
        let project = b
            .create(None, "Phase Review", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let seed_id = b
            .children_of(project.id)
            .into_iter()
            .find(|&id| b.get(id).is_some_and(|i| i.is_initial_plan_task()))
            .expect("seed");
        // Simulate finished Initial plan sitting in Review.
        let _ = b.claim(seed_id, "agent-1", None, 60).unwrap();
        let _ = b.transition(seed_id, State::Running, "agent-1", None);
        b.set_pr_url(seed_id, Some("https://example.com/pr/1".into()));
        let _ = b
            .report(seed_id, "agent-1", 1, 0, vec!["docs".into()])
            .expect("report");
        assert_eq!(b.get(seed_id).unwrap().state, State::Review);

        b.propose_plan(
            project.id,
            "from review",
            vec![PlanTaskSpec {
                key: "a".into(),
                title: "Task A".into(),
                intent: "do a".into(),
                definition_of_done: "a done".into(),
                blocked_by_keys: vec![],
                capability: None,
                item_id: None,
            }],
            vec![],
        )
        .expect("propose");

        let err = b.approve_plan(project.id).expect_err("waits for merge");
        assert!(
            err.contains("merges"),
            "approve_plan with PR should defer materialize: {err}"
        );
        assert_eq!(b.get(seed_id).unwrap().state, State::Review);
        b.complete_for_merged_pr("https://example.com/pr/1", Some(1))
            .expect("merge creates tasks");
        assert_eq!(b.get(seed_id).unwrap().state, State::Done);
        assert_eq!(
            b.children_of(project.id)
                .into_iter()
                .filter(|&id| !b.get(id).unwrap().is_initial_plan_task())
                .count(),
            1
        );
    }

    #[test]
    fn approve_review_on_initial_plan_materializes_awaiting_plan() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-approve-review-initial.json"),
        );
        let project = b
            .create(None, "Phase AR", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let seed_id = b
            .children_of(project.id)
            .into_iter()
            .find(|&id| b.get(id).is_some_and(|i| i.is_initial_plan_task()))
            .expect("seed");
        let _ = b.claim(seed_id, "agent-1", None, 60).unwrap();
        let _ = b.transition(seed_id, State::Running, "agent-1", None);
        b.set_pr_url(seed_id, Some("https://example.com/pr/2".into()));
        let _ = b
            .report(seed_id, "agent-1", 1, 0, vec!["docs".into()])
            .expect("report");

        b.propose_plan(
            project.id,
            "awaiting",
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

        // Approve with a PR only acknowledges — merge creates Tasks.
        let still = b.approve_review(seed_id).expect("approve_review");
        assert_eq!(still.state, State::Review);
        assert_eq!(
            b.children_of(project.id)
                .into_iter()
                .filter(|&id| !b.get(id).unwrap().is_initial_plan_task())
                .count(),
            0,
            "no siblings before merge"
        );

        let done_id = b
            .complete_for_merged_pr("https://example.com/pr/2", Some(2))
            .expect("merge completes card");
        assert_eq!(done_id, seed_id);
        let done = b.get(seed_id).unwrap();
        assert_eq!(done.state, State::Done);
        assert!(b.get(project.id).unwrap().plan.is_none());
        assert!(done.proposal.as_ref().is_some_and(|p| p.tasks.len() == 2));
        let tasks: Vec<_> = b
            .children_of(project.id)
            .into_iter()
            .filter_map(|id| b.get(id))
            .filter(|i| !i.is_initial_plan_task())
            .collect();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.state == State::Backlog));
        assert!(done.proposal.as_ref().unwrap().tasks.iter().all(|t| t.item_id.is_some()));
    }

    #[test]
    fn propose_plan_refused_after_initial_plan_accepted() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-plan-frozen.json"),
        );
        let project = b
            .create(None, "Phase Freeze", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        b.propose_plan(
            project.id,
            "once",
            vec![PlanTaskSpec {
                key: "a".into(),
                title: "Task A".into(),
                intent: "do a".into(),
                definition_of_done: "a done".into(),
                blocked_by_keys: vec![],
                capability: None,
                item_id: None,
            }],
            vec![],
        )
        .expect("propose");
        let _ = b.approve_plan(project.id).expect("approve");
        let err = b
            .propose_plan(
                project.id,
                "again",
                vec![PlanTaskSpec {
                    key: "b".into(),
                    title: "Task B".into(),
                    intent: "do b".into(),
                    definition_of_done: "b done".into(),
                    blocked_by_keys: vec![],
                    capability: None,
                    item_id: None,
                }],
                vec![],
            )
            .expect_err("frozen");
        assert!(err.contains("frozen") || err.contains("accepted"), "{err}");
    }

    #[test]
    fn claim_briefing_reads_frozen_initial_plan_proposal() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-claim-from-proposal.json"),
        );
        let project = b
            .create(None, "Phase Brief", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        b.propose_plan(
            project.id,
            "brief plan",
            vec![PlanTaskSpec {
                key: "a".into(),
                title: "Task A".into(),
                intent: "do a".into(),
                definition_of_done: "a done".into(),
                blocked_by_keys: vec![],
                capability: None,
                item_id: None,
            }],
            vec![],
        )
        .expect("propose");
        let published = b.approve_plan(project.id).expect("approve");
        let task_id = published[0];
        let grant = b.claim(task_id, "agent", None, 60).expect("claim");
        assert_eq!(grant.plan_summary.as_deref(), Some("brief plan"));
        assert_eq!(grant.plan_tasks.len(), 1);
        assert!(grant.plan_tasks[0].current);
        assert_eq!(grant.plan_task_key.as_deref(), Some("a"));
    }

    #[test]
    fn approve_plan_materializes_diamond_dag() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-approve-diamond-dag.json"),
        );
        let project = b
            .create(None, "Phase Diamond", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);

        b.propose_plan(
            project.id,
            "diamond plan",
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
                PlanTaskSpec {
                    key: "c".into(),
                    title: "Task C".into(),
                    intent: "do c".into(),
                    definition_of_done: "c done".into(),
                    blocked_by_keys: vec!["a".into()],
                    capability: None,
                    item_id: None,
                },
                PlanTaskSpec {
                    key: "d".into(),
                    title: "Task D".into(),
                    intent: "do d".into(),
                    definition_of_done: "d done".into(),
                    blocked_by_keys: vec!["b".into(), "c".into()],
                    capability: None,
                    item_id: None,
                },
            ],
            vec![],
        )
        .expect("propose");

        let published = b.approve_plan(project.id).expect("approve");
        assert_eq!(published.len(), 4);

        let id_a = published[0];
        let id_b = published[1];
        let id_c = published[2];
        let id_d = published[3];

        let item_a = b.get(id_a).unwrap();
        let item_b = b.get(id_b).unwrap();
        let item_c = b.get(id_c).unwrap();
        let item_d = b.get(id_d).unwrap();

        assert_eq!(item_a.blocked_by, Vec::<ItemId>::new());
        assert_eq!(item_b.blocked_by, vec![id_a]);
        assert_eq!(item_c.blocked_by, vec![id_a]);
        assert_eq!(item_d.blocked_by, vec![id_b, id_c]);

        // Verify Backlog column summary lists plain language blockers
        let s = b.state.read().unwrap();
        let goal_view = b.goal_view(&s, project.id, chrono::Utc::now()).expect("goal view");
        let ready_col = goal_view
            .columns
            .iter()
            .find(|c| c.column == Column::Backlog)
            .expect("ready col");
        assert!(ready_col.summary.text.contains("blocked on"));
    }

    /// Project + Backlog task under it. Returns (board, project_id, task_id).
    fn project_with_ready_task() -> (Board, ItemId, ItemId) {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-proj-task-{}.json", std::process::id())),
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
        let _ = b.transition(task.id, State::Backlog, "t", None);
        (b, project.id, task.id)
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

        assert_eq!(released.state, State::Backlog);
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
        assert_eq!(last_transition.to, State::Backlog);
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
    fn archived_project_marked_in_snapshot_omitted_from_digest() {
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
        let snap = b.snapshot();
        let archived_goal = snap
            .goals
            .iter()
            .find(|g| g.id == archive.id)
            .expect("retired Project stays in snapshot for Show archived");
        assert!(
            archived_goal.archived,
            "retired Project must be marked archived"
        );
        assert!(
            b.digest().goals.iter().all(|g| g.goal_id != archive.id),
            "retired Project must not appear in digest"
        );
        assert!(
            b.snapshot().goals.iter().any(|g| g.id == keep.id && !g.archived),
            "active Project still listed"
        );
    }

    #[test]
    fn board_digest_lists_ready_to_dispatch_cards() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-digest-ready-{}.json",
                std::process::id()
            )),
        );
        let project = b
            .create(None, "Test Project", "why", None, Origin::Human, true, None)
            .expect("project");
        let initial_id = b
            .children_of(project.id)
            .into_iter()
            .next()
            .expect("seeded initial plan");

        let digest_before = b.digest();
        let goal_before = digest_before
            .goals
            .iter()
            .find(|g| g.goal_id == project.id)
            .expect("goal");
        assert_eq!(goal_before.ready_to_dispatch.len(), 1);
        assert_eq!(goal_before.ready_to_dispatch[0].id, initial_id);
        assert_eq!(
            goal_before.ready_to_dispatch[0].title,
            "Initial Plan for Test Project"
        );


        b.enqueue_dispatch(initial_id).expect("dispatch");
        let digest_dispatched = b.digest();
        let goal_dispatched = digest_dispatched
            .goals
            .iter()
            .find(|g| g.goal_id == project.id)
            .expect("goal");
        assert!(goal_dispatched.ready_to_dispatch.is_empty());
    }

    #[test]
    fn retired_leaves_excluded_from_goal_progress() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-retired-leaves-{}.json",
                std::process::id()
            )),
        );
        let project = b
            .create(None, "Keep sandboxes", "why", None, Origin::Human, true, None)
            .expect("project");
        // create() seeds Initial plan — park it Done so it doesn't muddy the ratio.
        let initial_id = b
            .children_of(project.id)
            .into_iter()
            .next()
            .expect("seeded Initial plan");
        let done = b
            .create(
                Some(project.id),
                "Finished leaf",
                "why",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("done leaf");
        let cut = b
            .create(
                Some(project.id),
                "Cut duplicate",
                "why",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("cut leaf");

        for id in [initial_id, done.id, cut.id] {
            // Initial plan is already Backlog; others start Draft.
            let _ = b.transition(id, State::Shaping, "t", None);
            let _ = b.transition(id, State::Backlog, "t", None);
            let _ = b.transition(id, State::Claimed, "t", None);
            let _ = b.transition(id, State::Running, "t", None);
            let _ = b.transition(id, State::Review, "t", None);
            let _ = b.transition(id, State::Done, "t", None);
        }
        b.cut_scope(cut.id, Some("duplicate".into()))
            .expect("retire duplicate");

        let snap = b.snapshot();
        let goal = snap
            .goals
            .iter()
            .find(|g| g.id == project.id)
            .expect("goal");
        assert_eq!(
            (goal.leaves_done, goal.leaves_total),
            (2, 2),
            "retired leaf must not inflate denominator: got {}/{}",
            goal.leaves_done,
            goal.leaves_total
        );
        assert!((goal.progress - 1.0).abs() < f32::EPSILON);
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
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client);
        let board = Arc::new(board_raw);

        // Create leaves placeholders; mirror binds real beads ids.
        let project = board
            .create(None, "Test Mirror Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        assert!(
            project
                .beads_id
                .as_deref()
                .is_some_and(|b| b.starts_with("bd-honr-")),
            "create must leave a placeholder, got {:?}",
            project.beads_id
        );
        board.mirror_beads_item(project.id).await;
        assert!(
            crate::beads::BeadsClient::is_real_id(
                board
                    .get(project.id)
                    .and_then(|p| p.beads_id)
                    .as_deref()
                    .unwrap_or("")
            ),
            "mirror should assign a real beads id"
        );

        // Schedule github push / URL refresh for the epic.
        board.schedule_beads_mirror(project.id);

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
        assert!(
            task.beads_id
                .as_deref()
                .is_some_and(|b| b.starts_with("bd-honr-")),
            "task create must leave placeholder"
        );
        board.mirror_beads_item(task.id).await;
        assert!(
            crate::beads::BeadsClient::is_real_id(
                board
                    .get(task.id)
                    .and_then(|t| t.beads_id)
                    .as_deref()
                    .unwrap_or("")
            ),
            "task should have real beads id after mirror"
        );
        board.schedule_beads_mirror(task.id);

        // Propose split → Approve creates siblings (placeholders; mirror binds).
        let _ = board.transition(task.id, State::Shaping, "test", None);
        let _ = board.transition(task.id, State::Backlog, "test", None);
        let _ = board.claim(task.id, "agent", None, 45);

        let children = vec![
            SplitChildSpec::new("Sibling One", "intent 1", "dod 1"),
            SplitChildSpec::new("Sibling Two", "intent 2", "dod 2"),
        ];
        board
            .propose_split(task.id, "agent", children, 5)
            .expect("propose_split");
        board.approve_review(task.id).expect("approve");
        let made: Vec<_> = board
            .children_of(project.id)
            .into_iter()
            .filter_map(|cid| board.get(cid))
            .filter(|i| !i.is_initial_plan_task() && i.id != task.id)
            .collect();
        assert_eq!(made.len(), 2);

        for m in &made {
            assert!(
                m.beads_id
                    .as_deref()
                    .is_some_and(|b| b.starts_with("bd-honr-")),
                "split sibling #{} should start as placeholder",
                m.id
            );
            board.mirror_beads_item(m.id).await;
            assert!(
                crate::beads::BeadsClient::is_real_id(
                    board
                        .get(m.id)
                        .and_then(|i| i.beads_id)
                        .as_deref()
                        .unwrap_or("")
                ),
                "split sibling #{} should get a real beads id after mirror",
                m.id
            );
            board.schedule_beads_mirror(m.id);
        }

        remote_cap
            .wait_until(|ops| {
                ops.iter()
                    .any(|op| matches!(op, crate::beads::RemoteOp::GithubPush(ids) if !ids.is_empty()))
            })
            .await;
        assert!(
            remote_cap.ops().iter().any(
                |op| matches!(op, crate::beads::RemoteOp::GithubPush(ids) if !ids.is_empty())
            ),
            "expected captured github_push after create/split mirrors, got {:?}",
            remote_cap.ops()
        );

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

        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("init stealth");

        let mut board_raw = Board::new(Schema::default(), board_path);
        board_raw.beads = Some(beads_client.clone());
        let b = board_raw;

        let project = beads_client
            .create_linked("Test Project", 0, "epic", None, None, &[], None)
            .await
            .expect("create project");
        let task_done = beads_client
            .create_linked("Task Done", 1, "task", None, Some(&project.id), &[], None)
            .await
            .expect("create task done");
        let task_retired = beads_client
            .create_linked("Task Retired", 1, "task", None, Some(&project.id), &[], None)
            .await
            .expect("create task retired");

        // Create without sync (beads attached after… no — beads is attached).
        // Overwrite with the pre-created ids so close targets are known.
        let item1 = b
            .create(None, "Item Done", "why", None, Origin::Human, true, None)
            .expect("create item1");
        let item2 = b
            .create(None, "Item Retired", "why", None, Origin::Human, true, None)
            .expect("create item2");

        b.set_beads_id(item1.id, &task_done.id);
        b.set_beads_id(item2.id, &task_retired.id);

        let _ = b.transition(item1.id, State::Shaping, "t", None);
        let _ = b.transition(item1.id, State::Backlog, "t", None);
        let _ = b.transition(item1.id, State::Done, "human", Some("done".into()));

        let _ = b.transition(item2.id, State::Shaping, "t", None);
        let _ = b.transition(item2.id, State::Backlog, "t", None);
        let _ = b.transition(item2.id, State::Retired, "human", Some("retired".into()));

        eventually_async(|| async {
            let done_closed = beads_client
                .show(&task_done.id)
                .await
                .map(|s| s.status == "closed")
                .unwrap_or(false);
            let retired_closed = beads_client
                .show(&task_retired.id)
                .await
                .map(|s| s.status == "closed")
                .unwrap_or(false);
            done_closed && retired_closed
        })
        .await;
        assert_eq!(
            beads_client.show(&task_done.id).await.unwrap().status,
            "closed",
            "task_done status should be closed in beads"
        );
        assert_eq!(
            beads_client.show(&task_retired.id).await.unwrap().status,
            "closed",
            "task_retired status should be closed in beads"
        );
    }

    #[tokio::test]
    async fn done_and_retired_transitions_noop_for_placeholders() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-test-beads-placeholder-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let board_path = test_dir.join("board.json");

        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("init stealth");

        let mut board_raw = Board::new(Schema::default(), board_path);
        // Attach after the cards exist so create keeps placeholders.
        let item1 = board_raw
            .create(None, "Placeholder Item 1", "why", None, Origin::Human, true, None)
            .expect("create item1");
        let item2 = board_raw
            .create(None, "Placeholder Item 2", "why", None, Origin::Human, true, None)
            .expect("create item2");
        board_raw.beads = Some(beads_client.clone());
        let b = board_raw;

        let real_issue = beads_client
            .create_linked("Real Task", 1, "task", None, None, &[], None)
            .await
            .expect("create real task");

        assert!(item1.beads_id.as_ref().unwrap().starts_with("bd-honr-"));
        assert!(item2.beads_id.as_ref().unwrap().starts_with("bd-honr-"));

        let _ = b.transition(item1.id, State::Shaping, "t", None);
        let _ = b.transition(item1.id, State::Backlog, "t", None);
        let _ = b.transition(item1.id, State::Done, "human", Some("done".into()));

        let _ = b.transition(item2.id, State::Shaping, "t", None);
        let _ = b.transition(item2.id, State::Backlog, "t", None);
        let _ = b.transition(item2.id, State::Retired, "human", Some("retired".into()));

        // Placeholders must not close unrelated real beads — give the Done/Retired
        // spawn a moment to misbehave if it would.
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let shown_real = beads_client
            .show(&real_issue.id)
            .await
            .expect("show real task");
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

        // Mirror binds a real beads id; push fills the URL.
        board.mirror_beads_item(project.id).await;
        board.schedule_beads_mirror(project.id);
        let project_beads_id = board
            .get(project.id)
            .and_then(|p| p.beads_id)
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .expect("expected real beads_id after mirror");

        for _ in 0..200 {
            if board
                .get(project.id)
                .and_then(|p| p.github_issue_url)
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }

        // Override with a stable URL and refresh (exercises show → board write-through).
        let expected_url = "https://github.com/shanemcd/honr/issues/777";
        beads_client
            .set_external_ref(&project_beads_id, expected_url)
            .await
            .expect("update external ref");
        board
            .refresh_github_issue_url(project.id, &project_beads_id)
            .await;

        let found_url = board.get(project.id).and_then(|p| p.github_issue_url);

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

    static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    #[tokio::test]
    async fn test_single_flight_github_push_prevents_duplicate_pushes() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-single-flight-test-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(None, "Single Flight Project", "intent", None, Origin::Human, true, None)
            .expect("create project");

        let _ = remote_cap.take();

        // Concurrent mirrors: first binds + pushes; the rest single-flight / no-op.
        let mut handles = Vec::new();
        for _ in 0..10 {
            let b = Arc::clone(&board);
            let p_id = project.id;
            handles.push(tokio::spawn(async move {
                b.mirror_beads_item(p_id).await;
            }));
        }

        for h in handles {
            h.await.expect("join task");
        }

        let project_beads_id = board
            .get(project.id)
            .and_then(|p| p.beads_id)
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .expect("real beads_id after mirror");

        let ops = remote_cap.ops();
        let push_ops: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) if ids.contains(&project_beads_id) => {
                    Some(ids.clone())
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            push_ops.len(),
            1,
            "expected exactly one GitHub push for {project_beads_id}, got {:?}",
            push_ops
        );

        let project_item = board.get(project.id).unwrap();
        assert!(
            project_item.github_issue_url.is_some(),
            "github_issue_url should be set on board"
        );
        let show_issue = beads_client.show(&project_beads_id).await.unwrap();
        assert_eq!(
            project_item.github_issue_url,
            show_issue.github_issue_url(),
            "board github_issue_url should match beads external_ref"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_create_project_with_seeded_initial_plan_results_in_at_most_one_github_issue_per_beads_id() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-project-seed-single-flight-test-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let _ = remote_cap.take();

        let project = board
            .create(None, "Seeded Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        let children = board.children_of(project.id);
        assert!(!children.is_empty(), "expected seeded initial plan task");
        let seed_id = children[0];

        let _ = remote_cap.take();

        let b1 = Arc::clone(&board);
        let b2 = Arc::clone(&board);
        let p_id = project.id;
        let s_id = seed_id;

        let t1 = tokio::spawn(async move {
            b1.mirror_beads_item(p_id).await;
        });
        let t2 = tokio::spawn(async move {
            b2.mirror_beads_item(s_id).await;
        });

        t1.await.unwrap();
        t2.await.unwrap();

        let project_beads_id = board
            .get(project.id)
            .and_then(|p| p.beads_id)
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .expect("project beads_id");
        let seed_beads_id = board
            .get(seed_id)
            .and_then(|s| s.beads_id)
            .expect("seed beads_id");

        let ops = remote_cap.ops();

        let project_pushes: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) if ids.contains(&project_beads_id) => {
                    Some(ids.clone())
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            project_pushes.len(),
            1,
            "expected at most 1 push for project epic {project_beads_id}, got {:?}",
            project_pushes
        );

        let seed_pushes: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) if ids.contains(&seed_beads_id) => {
                    Some(ids.clone())
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            seed_pushes.len(),
            0,
            "expected 0 pushes for seed task {seed_beads_id}, got {:?}",
            seed_pushes
        );

        let proj_item = board.get(project.id).unwrap();
        let proj_show = beads_client.show(&project_beads_id).await.unwrap();
        assert_eq!(
            proj_item.github_issue_url,
            proj_show.github_issue_url(),
            "project board github_issue_url matches beads external_ref"
        );

        let seed_item = board.get(seed_id).unwrap();
        assert_eq!(
            seed_item.github_issue_url, None,
            "seed board github_issue_url should be None"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_skip_initial_plan_github_push() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-skip-initial-plan-gh-test-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        // 1. Create Project (seeds Initial plan)
        let project = board
            .create(None, "New Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        let children = board.children_of(project.id);
        assert!(!children.is_empty(), "expected seeded initial plan task");
        let seed_id = children[0];

        board.mirror_beads_item(project.id).await;
        board.mirror_beads_item(seed_id).await;

        let seed_item = board.get(seed_id).unwrap();
        assert!(
            seed_item.beads_id.is_some(),
            "Initial plan should have beads_id"
        );
        assert_eq!(
            seed_item.github_issue_url, None,
            "Initial plan github_issue_url should be None"
        );

        let seed_beads_id = seed_item.beads_id.unwrap();
        let ops = remote_cap.ops();
        let seed_pushes: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) if ids.contains(&seed_beads_id) => {
                    Some(ids.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            seed_pushes.len(),
            0,
            "no GH issue created/pushed for Initial plan seed"
        );

        // 2. Materialized / subsequent non-Initial plan task gets an Issue
        let task = board
            .create(
                Some(project.id),
                "Real Feature Task",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create task");
        board.mirror_beads_item(task.id).await;

        let task_item = board.get(task.id).unwrap();
        assert!(
            task_item.beads_id.is_some(),
            "Feature task should have beads_id"
        );
        assert!(
            task_item.github_issue_url.is_some(),
            "Feature task should get github_issue_url"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
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

        // Force placeholders on open cards (+ keep retired as placeholder) so heal
        // has work — create already leaves placeholders; overwrite for stable ids.
        let initial_plan = board
            .snapshot()
            .items
            .into_iter()
            .find(|i| i.parent == Some(project.id) && i.is_initial_plan_task())
            .expect("seeded initial plan");
        board.set_beads_id(project.id, "bd-honr-heal-project");
        board.set_beads_id(initial_plan.id, "bd-honr-heal-plan");
        board.set_beads_id(task.id, "bd-honr-heal-task");
        board.set_beads_id(retired.id, "bd-honr-heal-retired");

        // 2. Execute heal
        let healed_count = board.heal_placeholder_beads_ids().await;
        assert_eq!(
            healed_count, 3,
            "should heal project, initial plan, and task (not retired)"
        );

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
                None,
            )
            .await
            .expect("create bead");
        board.set_beads_id(project.id, &issue.id);
        assert!(board.get(project.id).unwrap().github_issue_url.is_none());

        let expected_url = "https://github.com/shanemcd/honr/issues/759";
        beads_client
            .set_external_ref(&issue.id, expected_url)
            .await
            .expect("update external ref");

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
        // SAFETY: Live remotes create real issues — point github.owner/repo at a throwaway.
        let test_dir = std::env::temp_dir().join(format!(
            "honr-e2e-live-sync-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Live,
        );
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
        let _ = board.transition(task.id, State::Backlog, "test", None);
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

    #[test]
    fn ready_column_summary_mentions_blockers_in_plain_language() {
        let b = Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-summary-blockers.json"),
        );
        let project = b
            .create(None, "Test Project", "Goal", None, Origin::Human, true, None)
            .unwrap();

        // Project creation seeds 1 initial plan task in Backlog.
        let t1 = b
            .create(Some(project.id), "Task One", "Unblocked", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        let _ = b.transition(t1.id, State::Shaping, "human", None);
        let _ = b.transition(t1.id, State::Backlog, "human", None);

        {
            let s = b.state.read().unwrap();
            let goal_view = b.goal_view(&s, project.id, chrono::Utc::now()).expect("goal view");
            let ready_col = goal_view
                .columns
                .iter()
                .find(|c| c.column == Column::Backlog)
                .expect("ready col");
            assert!(ready_col.summary.text.contains("2 in backlog"));
            assert!(!ready_col.summary.text.contains("blocked on"));
        }

        let t2 = b
            .create(Some(project.id), "Task Two", "Blocked", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        let _ = b.transition(t2.id, State::Shaping, "human", None);
        let _ = b.transition(t2.id, State::Backlog, "human", None);
        b.set_blocked_by(t2.id, vec![t1.id]);

        {
            let s = b.state.read().unwrap();
            let goal_view = b.goal_view(&s, project.id, chrono::Utc::now()).expect("goal view");
            let ready_col = goal_view
                .columns
                .iter()
                .find(|c| c.column == Column::Backlog)
                .expect("ready col");
            assert!(ready_col.summary.text.contains("3 in backlog"));
            assert!(
                ready_col.summary.text.contains(&format!("1 blocked on #{}: Task One", t1.id)),
                "Summary was: {}",
                ready_col.summary.text
            );
        }
    }

    #[tokio::test]
    async fn sandbox_deleted_on_done_retired_and_item_deletion() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("honr-sb-del-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let log_file = dir.join("openshell_calls.log");
        let log_path = log_file.clone();
        let os = crate::openshell::OpenShell::mock(
            move |args| {
                let line = args.join(" ");
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(f, "{line}")
                    });
                crate::openshell::Output {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            },
            std::time::Duration::from_secs(5),
        );

        let mut board_raw = Board::new(
            crate::schema::Schema::default(),
            dir.join("board.json"),
        );
        board_raw.openshell = Some(os);
        let b = Arc::new(board_raw);

        // 1. Transition to Review keeps sandbox environment
        let p = b
            .create(None, "Project", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(p.id), "Task 1", "intent", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        b.set_environment(t1.id, Some("honr-card-1-a1".into()));
        let _ = b.transition(t1.id, State::Shaping, "human", None);
        let _ = b.transition(t1.id, State::Backlog, "human", None);
        let _ = b.transition(t1.id, State::Claimed, "human", None);
        let _ = b.transition(t1.id, State::Running, "agent", None);
        let _ = b.transition(t1.id, State::Review, "agent", None);

        assert_eq!(b.get(t1.id).unwrap().environment.as_deref(), Some("honr-card-1-a1"));

        // 2. Transition to Done clears environment and triggers sandbox deletion
        let _ = b.transition(t1.id, State::Done, "human", None);
        assert_eq!(b.get(t1.id).unwrap().environment, None);

        eventually(|| {
            std::fs::read_to_string(&log_file)
                .unwrap_or_default()
                .contains("sandbox delete honr-card-1-a1")
        })
        .await;
        let log_content = std::fs::read_to_string(&log_file).unwrap_or_default();
        assert!(
            log_content.contains("sandbox delete honr-card-1-a1"),
            "expected 'sandbox delete honr-card-1-a1' in log, got: {log_content}"
        );

        // 3. Transition to Retired clears environment and triggers sandbox deletion
        let t2 = b
            .create(Some(p.id), "Task 2", "intent", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        b.set_environment(t2.id, Some("honr-card-2-a1".into()));
        let _ = b.transition(t2.id, State::Retired, "human", None);
        assert_eq!(b.get(t2.id).unwrap().environment, None);

        eventually(|| {
            std::fs::read_to_string(&log_file)
                .unwrap_or_default()
                .contains("sandbox delete honr-card-2-a1")
        })
        .await;
        let log_content = std::fs::read_to_string(&log_file).unwrap_or_default();
        assert!(
            log_content.contains("sandbox delete honr-card-2-a1"),
            "expected 'sandbox delete honr-card-2-a1' in log, got: {log_content}"
        );

        // 4. Item deletion triggers sandbox deletion
        let t3 = b
            .create(Some(p.id), "Task 3", "intent", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        b.set_environment(t3.id, Some("honr-card-3-a1".into()));
        b.delete_item(t3.id).expect("delete_item succeeds");
        assert!(b.get(t3.id).is_none());

        eventually(|| {
            std::fs::read_to_string(&log_file)
                .unwrap_or_default()
                .contains("sandbox delete honr-card-3-a1")
        })
        .await;
        let log_content = std::fs::read_to_string(&log_file).unwrap_or_default();
        assert!(
            log_content.contains("sandbox delete honr-card-3-a1"),
            "expected 'sandbox delete honr-card-3-a1' in log, got: {log_content}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_close_linked_github_issue_on_done_and_retired() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-close-gh-on-done-test-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(None, "Close GH Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        let task = board
            .create(
                Some(project.id),
                "Task to Close",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("create task");

        board.mirror_beads_item(project.id).await;
        board.mirror_beads_item(task.id).await;

        let task_item = board.get(task.id).unwrap();
        let task_beads_id = task_item.beads_id.clone().unwrap();
        assert!(task_item.github_issue_url.is_some(), "task should have github_issue_url");

        let _ = remote_cap.take();

        let _ = board.transition(task.id, State::Shaping, "test", None);
        let _ = board.transition(task.id, State::Backlog, "test", None);
        let _ = board.transition(task.id, State::Done, "human", Some("completed".into()));

        remote_cap
            .wait_until(|ops| {
                ops.iter().any(|op| match op {
                    crate::beads::RemoteOp::GithubPush(ids) => ids.contains(&task_beads_id),
                    _ => false,
                })
            })
            .await;
        eventually_async(|| async {
            beads_client
                .show(&task_beads_id)
                .await
                .map(|s| s.status == "closed")
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            beads_client.show(&task_beads_id).await.unwrap().status,
            "closed",
            "expected beads task closed on Done for {task_beads_id}"
        );
        assert!(
            remote_cap.ops().iter().any(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) => ids.contains(&task_beads_id),
                _ => false,
            }),
            "expected GithubPush recorded on Done transition for task {task_beads_id}"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_initial_plan_approve_and_done_does_not_leave_orphan_open_issues() {
        let tid = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let test_dir = std::env::temp_dir().join(format!(
            "honr-initial-plan-no-orphan-gh-test-{}-{}",
            std::process::id(),
            tid
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let board_file = test_dir.join("honr.json");
        let beads_dir = test_dir.join(".beads");
        let remote_cap = crate::beads::RemoteCapture::new();
        let beads_client = crate::beads::BeadsClient::with_remotes(
            &beads_dir,
            crate::beads::Remotes::Capture(remote_cap.clone()),
        );
        beads_client.init_stealth().await.expect("stealth init");

        let mut board_raw = Board::new(Schema::default(), board_file);
        board_raw.beads = Some(beads_client.clone());
        let board = Arc::new(board_raw);

        let project = board
            .create(None, "No Orphan GH Project", "intent", None, Origin::Human, true, None)
            .expect("create project");
        let seed_id = board.children_of(project.id)[0];

        board.mirror_beads_item(project.id).await;
        board.mirror_beads_item(seed_id).await;

        let seed_item = board.get(seed_id).unwrap();
        let seed_beads_id = seed_item.beads_id.clone().unwrap();
        assert_eq!(seed_item.github_issue_url, None, "seed task should have no github_issue_url");

        let _ = remote_cap.take();

        board
            .propose_plan(
                seed_id,
                "Plan breakdown",
                vec![
                    PlanTaskSpec {
                        key: "f1".into(),
                        title: "Feature One".into(),
                        intent: "intent 1".into(),
                        definition_of_done: "dod 1".into(),
                        blocked_by_keys: vec![],
                        capability: None,
                        item_id: None,
                    },
                    PlanTaskSpec {
                        key: "f2".into(),
                        title: "Feature Two".into(),
                        intent: "intent 2".into(),
                        definition_of_done: "dod 2".into(),
                        blocked_by_keys: vec![],
                        capability: None,
                        item_id: None,
                    },
                ],
                vec![],
            )
            .expect("propose plan on seed");
        let done_seed = board.approve_review(seed_id).expect("approve seed review");
        assert_eq!(done_seed.state, State::Done);

        eventually_async(|| async {
            beads_client
                .show(&seed_beads_id)
                .await
                .map(|s| s.status == "closed")
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            beads_client.show(&seed_beads_id).await.unwrap().status,
            "closed",
            "seed task in beads should be closed"
        );

        let ops = remote_cap.ops();
        let seed_pushes: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) if ids.contains(&seed_beads_id) => {
                    Some(ids.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(seed_pushes.len(), 0, "no GithubPush should be sent for Initial plan seed card");

        let made: Vec<_> = board
            .children_of(project.id)
            .into_iter()
            .filter_map(|cid| board.get(cid))
            .filter(|i| !i.is_initial_plan_task())
            .collect();
        assert_eq!(made.len(), 2, "expected 2 materialized tasks");

        for m in &made {
            board.mirror_beads_item(m.id).await;
            assert!(
                board.get(m.id).unwrap().github_issue_url.is_some(),
                "materialized task should get github_issue_url"
            );
        }

        let mat_task_id = made[0].id;
        let mat_beads_id = board
            .get(mat_task_id)
            .and_then(|t| t.beads_id)
            .filter(|b| crate::beads::BeadsClient::is_real_id(b))
            .expect("materialized task real beads_id");

        let _ = remote_cap.take();
        let _ = board.transition(mat_task_id, State::Shaping, "test", None);
        let _ = board.transition(mat_task_id, State::Backlog, "test", None);
        let _ = board.transition(mat_task_id, State::Done, "human", Some("done".into()));

        remote_cap
            .wait_until(|ops| {
                ops.iter().any(|op| match op {
                    crate::beads::RemoteOp::GithubPush(ids) => ids.contains(&mat_beads_id),
                    _ => false,
                })
            })
            .await;
        eventually_async(|| async {
            beads_client
                .show(&mat_beads_id)
                .await
                .map(|s| s.status == "closed")
                .unwrap_or(false)
        })
        .await;
        assert_eq!(
            beads_client.show(&mat_beads_id).await.unwrap().status,
            "closed",
            "materialized task should close bead on Done"
        );
        assert!(
            remote_cap.ops().iter().any(|op| match op {
                crate::beads::RemoteOp::GithubPush(ids) => ids.contains(&mat_beads_id),
                _ => false,
            }),
            "materialized task should push to GitHub on Done"
        );

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn approve_review_with_pr_stays_in_review_until_merge() {
        let b = Arc::new(Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-approve-waits-merge-{}.json",
                std::process::id()
            )),
        ));
        let p = b
            .create(None, "Wait Merge", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t = b
            .create(
                Some(p.id),
                "Impl",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = b.transition(t.id, State::Shaping, "human", None);
        let _ = b.transition(t.id, State::Backlog, "human", None);
        let _ = b.transition(t.id, State::Claimed, "agent", None);
        let _ = b.transition(t.id, State::Running, "agent", None);
        let _ = b.transition(t.id, State::Review, "agent", None);
        b.set_pr_url(t.id, Some("https://github.com/shanemcd/honr/pull/99".into()));

        let item = b.approve_review(t.id).expect("approve");
        assert_eq!(item.state, State::Review, "PR cards wait for merge");
        assert!(
            b.complete_for_merged_pr("https://github.com/shanemcd/honr/pull/99", Some(99))
                .is_some()
        );
        assert_eq!(b.get(t.id).unwrap().state, State::Done);
    }

    #[test]
    fn complete_for_merged_pr_matches_normalized_url_and_is_idempotent() {
        let b = Arc::new(Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-complete-merged-pr-{}.json",
                std::process::id()
            )),
        ));
        let p = b
            .create(None, "Merge Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t = b
            .create(
                Some(p.id),
                "Feature",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = b.transition(t.id, State::Shaping, "human", None);
        let _ = b.transition(t.id, State::Backlog, "human", None);
        let _ = b.transition(t.id, State::Claimed, "agent", None);
        let _ = b.transition(t.id, State::Running, "agent", None);
        let _ = b.transition(t.id, State::Review, "agent", None);
        b.set_pr_url(
            t.id,
            Some("https://github.com/shanemcd/honr/pull/55/".into()),
        );

        assert_eq!(
            Board::normalize_pr_url("https://GitHub.com/shanemcd/honr/pull/55/"),
            "https://github.com/shanemcd/honr/pull/55"
        );

        let done_id = b
            .complete_for_merged_pr("https://GitHub.com/shanemcd/honr/pull/55", Some(55))
            .expect("should complete Review card");
        assert_eq!(done_id, t.id);
        assert_eq!(b.get(t.id).unwrap().state, State::Done);

        assert!(
            b.complete_for_merged_pr("https://github.com/shanemcd/honr/pull/55", Some(55))
                .is_none(),
            "idempotent: already Done"
        );
        assert!(
            b.complete_for_merged_pr("https://github.com/shanemcd/honr/pull/56", Some(56))
                .is_none(),
            "no match"
        );
    }

    #[test]
    fn test_event_sequence_ordering_and_buffer_catchup() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-seq-catchup-{}.json",
                std::process::id()
            )),
        )
        .with_buffer_capacity(10);

        assert_eq!(b.current_seq(), 0);

        // 1. Create project emits Upsert for Project and Initial plan Task transitions + Story (5 events in total)
        let p = b
            .create(None, "Test Seq", "intent", None, Origin::Human, true, None)
            .unwrap();

        let initial_seq = b.current_seq();
        assert_eq!(initial_seq, 5);

        // 2. Story event (seq 6)
        b.story(p.id, "Story line 1".to_string());
        assert_eq!(b.current_seq(), 6);

        // At seq 6 with capacity 10, event_buffer contains seq 1..6
        match b.catch_up(0) {
            CatchUpResult::Events(events) => {
                assert_eq!(events.len(), 6);
                for (i, ev) in events.iter().enumerate() {
                    assert_eq!(ev.seq(), (i + 1) as u64);
                }
            }
            CatchUpResult::Reset { .. } => panic!("expected events for last_seq 0"),
        }

        // Request catchup from seq 4 (should return seq 5 and 6)
        match b.catch_up(4) {
            CatchUpResult::Events(events) => {
                assert_eq!(events.len(), 2);
                assert_eq!(events[0].seq(), 5);
                assert_eq!(events[1].seq(), 6);
            }
            CatchUpResult::Reset { .. } => panic!("expected events for last_seq 4"),
        }

        // Request catchup from current_seq (seq 6) (should return empty vec)
        match b.catch_up(6) {
            CatchUpResult::Events(events) => {
                assert!(events.is_empty());
            }
            CatchUpResult::Reset { .. } => panic!("expected empty events for last_seq 6"),
        }

        // 3. Emit 5 more events to overflow buffer capacity 10 (total 11 events: seq 1..11, buffer holds seq 2..11)
        for i in 0..5 {
            b.story(p.id, format!("Story line extra {i}"));
        }
        assert_eq!(b.current_seq(), 11);

        // Catchup from last_seq = 0 (needs seq 1, which was popped) -> should return Reset
        match b.catch_up(0) {
            CatchUpResult::Reset { seq } => {
                assert_eq!(seq, 11);
            }
            CatchUpResult::Events(_) => panic!("expected Reset frame for lagged last_seq 0"),
        }

        // Catchup from last_seq = 1 (needs seq 2, which is still in buffer) -> should return seq 2..11
        match b.catch_up(1) {
            CatchUpResult::Events(events) => {
                assert_eq!(events.len(), 10);
                assert_eq!(events[0].seq(), 2);
                assert_eq!(events.last().unwrap().seq(), 11);
            }
            CatchUpResult::Reset { .. } => panic!("expected events for last_seq 1"),
        }

        // Catchup from future seq (last_seq = 20 when current = 11) -> should return Reset
        match b.catch_up(20) {
            CatchUpResult::Reset { seq } => {
                assert_eq!(seq, 11);
            }
            CatchUpResult::Events(_) => panic!("expected Reset for future last_seq 20"),
        }
    }

    #[test]
    fn approve_next_linear_chain_handoff_surfaces_sibling() {
        let b = Arc::new(Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-approve-next-{}.json",
                std::process::id()
            )),
        ));
        let p = b
            .create(None, "Linear Chain Project", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(p.id), "Task 1", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let t2 = b
            .create(Some(p.id), "Task 2", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();
        let t3 = b
            .create(Some(p.id), "Task 3", "intent 3", Some("dod 3".into()), Origin::Human, false, None)
            .unwrap();

        b.set_blocked_by(t2.id, vec![t1.id]);
        b.set_blocked_by(t3.id, vec![t2.id]);

        let _ = b.transition(t1.id, State::Shaping, "test", None);
        let _ = b.transition(t1.id, State::Backlog, "test", None);
        let _ = b.transition(t1.id, State::Claimed, "agent", None);
        let _ = b.transition(t1.id, State::Running, "agent", None);
        let _ = b.transition(t1.id, State::Review, "agent", None);

        let done1 = b.approve_review(t1.id).expect("approve t1");
        assert_eq!(done1.state, State::Done);

        let unblocked1 = b.newly_unblocked_siblings(t1.id);
        assert_eq!(unblocked1.len(), 1);
        assert_eq!(unblocked1[0].id, t2.id);

        let stories = b.stories_for(p.id);
        assert!(
            stories.iter().any(|s| s.text.contains(&format!("Unblocked next sibling #{}", t2.id))),
            "Expected story line referencing unblocked sibling #{}",
            t2.id
        );

        let _ = b.transition(t2.id, State::Shaping, "test", None);
        let _ = b.transition(t2.id, State::Backlog, "test", None);
        let _ = b.transition(t2.id, State::Claimed, "agent", None);
        let _ = b.transition(t2.id, State::Running, "agent", None);
        let _ = b.transition(t2.id, State::Review, "agent", None);

        let done2 = b.approve_review(t2.id).expect("approve t2");
        assert_eq!(done2.state, State::Done);

        let unblocked2 = b.newly_unblocked_siblings(t2.id);
        assert_eq!(unblocked2.len(), 1);
        assert_eq!(unblocked2[0].id, t3.id);

        let stories2 = b.stories_for(p.id);
        assert!(
            stories2.iter().any(|s| s.text.contains(&format!("Unblocked next sibling #{}", t3.id))),
            "Expected story line referencing unblocked sibling #{}",
            t3.id
        );
    }

    #[tokio::test]
    async fn test_heal_completed_epics_auto_closes_project_when_child_tasks_done() {
        let path = std::env::temp_dir().join(format!(
            "honr-test-epic-hygiene-{}.json",
            std::process::id()
        ));
        let b = Arc::new(Board::new(Schema::default(), path));
        let p = b
            .create(None, "Epic Hygiene Project", "intent", None, Origin::Human, true, None)
            .unwrap();
        let _t1 = b
            .create(Some(p.id), "Child Task 1", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let _t2 = b
            .create(Some(p.id), "Child Task 2", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();

        // Project should be open
        assert_ne!(b.get(p.id).unwrap().state, State::Done);

        // Mark all child tasks (including seeded Initial plan task) as Done
        let children: Vec<ItemId> = {
            let s = b.state.read().unwrap();
            s.items
                .values()
                .filter(|i| i.parent == Some(p.id))
                .map(|i| i.id)
                .collect()
        };

        for cid in children {
            let _ = b.transition(cid, State::Shaping, "test", None);
            let _ = b.transition(cid, State::Done, "test", None);
        }

        // Run heal_completed_epics
        let healed = b.heal_completed_epics().await;
        assert!(healed > 0, "expected at least 1 project healed");

        assert_eq!(b.get(p.id).unwrap().state, State::Done);
    }

    #[test]
    fn identify_behind_sibling_prs_and_dispatch_rebase() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Rebase Proj", "intent", None, Origin::Human, true, None)
            .unwrap();

        let t1 = b
            .create(Some(project.id), "Task 1", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let t2 = b
            .create(Some(project.id), "Task 2", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t1.id, Some("https://github.com/shanemcd/honr/pull/101".into()));

        b.transition(t2.id, State::Shaping, "test", None).unwrap();
        b.transition(t2.id, State::Backlog, "test", None).unwrap();
        b.transition(t2.id, State::Claimed, "agent", None).unwrap();
        b.transition(t2.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t2.id, Some("https://github.com/shanemcd/honr/pull/102".into()));

        let behind = b.identify_behind_sibling_prs(t1.id);
        assert_eq!(behind.len(), 1);
        assert_eq!(behind[0].id, t2.id);

        let completed_id = b
            .complete_for_merged_pr("https://github.com/shanemcd/honr/pull/101", Some(101))
            .expect("t1 completed");
        assert_eq!(completed_id, t1.id);

        let t2_updated = b.get(t2.id).unwrap();
        assert!(t2_updated.rebase_requested, "t2 should have rebase_requested set");
        assert!(t2_updated.awaiting_dispatch, "t2 should have awaiting_dispatch set");

        let awaiting_rebase = b.list_awaiting_rebase();
        assert_eq!(awaiting_rebase.len(), 1);
        assert_eq!(awaiting_rebase[0].id, t2.id);
    }

    #[test]
    fn notify_main_advanced_dispatches_rebase_for_sibling_prs_in_review() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-main-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Rebase Main Proj", "intent", None, Origin::Human, true, None)
            .unwrap();

        let t1 = b
            .create(Some(project.id), "Merged Task", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let t2 = b
            .create(Some(project.id), "Behind Task", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Done, "test", None).unwrap();

        b.transition(t2.id, State::Shaping, "test", None).unwrap();
        b.transition(t2.id, State::Backlog, "test", None).unwrap();
        b.transition(t2.id, State::Claimed, "agent", None).unwrap();
        b.transition(t2.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t2.id, Some("https://github.com/shanemcd/honr/pull/202".into()));

        b.notify_main_advanced("refs/heads/main", Some("sha123".into()));

        let t2_updated = b.get(t2.id).unwrap();
        assert!(t2_updated.rebase_requested, "t2 rebase_requested should be true");
        assert!(t2_updated.awaiting_dispatch, "t2 awaiting_dispatch should be true");
    }

    #[test]
    fn rebase_clean_keeps_card_in_review() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-clean-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Rebase Clean Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(project.id), "Task Clean", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t1.id, Some("https://github.com/shanemcd/honr/pull/301".into()));

        b.dispatch_rebase(t1.id).unwrap();
        let item = b.get(t1.id).unwrap();
        assert!(item.rebase_requested);

        let updated = b.complete_rebase_clean(t1.id).unwrap();
        assert_eq!(updated.state, State::Review);
        assert!(!updated.rebase_requested);
        assert!(!updated.awaiting_dispatch);
        assert_eq!(updated.last_bounce_reason, None);
    }

    #[test]
    fn rebase_conflict_moves_card_to_backlog_with_bounce_reason_and_conflict_details() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-conflict-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Rebase Conflict Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(project.id), "Task Conflict", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t1.id, Some("https://github.com/shanemcd/honr/pull/302".into()));

        b.dispatch_rebase(t1.id).unwrap();

        let conflicting_files = vec!["src/main.rs".to_string(), "src/store.rs".to_string()];
        let updated = b
            .complete_rebase_conflict(t1.id, &conflicting_files, Some("git rebase conflict"))
            .unwrap();

        assert_eq!(updated.state, State::Backlog);
        assert!(!updated.rebase_requested);
        assert!(!updated.awaiting_dispatch);

        let bounce_reason = updated.last_bounce_reason.expect("bounce reason set");
        assert!(bounce_reason.contains("git rebase conflict"));
        assert!(bounce_reason.contains("src/main.rs"));
        assert!(bounce_reason.contains("src/store.rs"));
    }

    #[test]
    fn repeated_rebase_conflict_on_overlapping_files_escalates_to_needs_human() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-conflict-repeat-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Repeated Conflict Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(project.id), "Task Repeated Conflict", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t1.id, Some("https://github.com/shanemcd/honr/pull/303".into()));

        // First conflict -> Backlog
        b.dispatch_rebase(t1.id).unwrap();
        let first_conflict_files = vec!["src/main.rs".to_string(), "src/store.rs".to_string()];
        let updated1 = b
            .complete_rebase_conflict(t1.id, &first_conflict_files, Some("git rebase conflict"))
            .unwrap();
        assert_eq!(updated1.state, State::Backlog);
        assert_eq!(updated1.last_conflict_files, vec!["src/main.rs", "src/store.rs"]);

        // Card is claimed again and moves back to Review
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();

        // Second conflict on overlapping file "src/main.rs" -> NeedsHuman
        b.dispatch_rebase(t1.id).unwrap();
        let second_conflict_files = vec!["src/main.rs".to_string(), "src/api.rs".to_string()];
        let updated2 = b
            .complete_rebase_conflict(t1.id, &second_conflict_files, Some("git rebase conflict"))
            .unwrap();

        assert_eq!(updated2.state, State::NeedsHuman);
        assert!(!updated2.rebase_requested);
        assert!(!updated2.awaiting_dispatch);

        let esc = updated2.escalation.expect("escalation present");
        assert!(esc.question.contains("Decomposition failure"));
        assert!(esc.question.contains("src/main.rs"));
        assert!(esc.options.len() >= 2);
    }

    #[test]
    fn second_rebase_conflict_on_disjoint_files_returns_to_backlog() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-rebase-conflict-disjoint-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Disjoint Conflict Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(Some(project.id), "Task Disjoint Conflict", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();
        b.set_pr_url(t1.id, Some("https://github.com/shanemcd/honr/pull/304".into()));

        // First conflict -> Backlog
        b.dispatch_rebase(t1.id).unwrap();
        let first_conflict_files = vec!["src/main.rs".to_string()];
        let updated1 = b
            .complete_rebase_conflict(t1.id, &first_conflict_files, Some("git rebase conflict"))
            .unwrap();
        assert_eq!(updated1.state, State::Backlog);

        // Card is claimed again and moves back to Review
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Review, "agent", None).unwrap();

        // Second conflict on disjoint file "src/store.rs" -> Backlog (not NeedsHuman)
        b.dispatch_rebase(t1.id).unwrap();
        let second_conflict_files = vec!["src/store.rs".to_string()];
        let updated2 = b
            .complete_rebase_conflict(t1.id, &second_conflict_files, Some("git rebase conflict"))
            .unwrap();

        assert_eq!(updated2.state, State::Backlog);
        assert!(updated2.escalation.is_none());
        assert_eq!(updated2.last_conflict_files, vec!["src/store.rs"]);
    }
}
