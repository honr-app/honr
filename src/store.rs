//! The board. Written to at machine speed, read by agents as their source of
//! truth, and moving a card *is* an action.
//!
//! Both faces — REST/SSE for humans, MCP for the cockpit and for agents — call
//! into here. Neither owns any state-machine logic, which is what keeps the two
//! renderings from drifting.

use crate::db::DurableBoardStore;
use crate::events::BoardEvent;
use crate::machine::{self, TransitionError};
use crate::model::*;
use crate::schema::{AgentConfig, Level, RepoConfig, Schema};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
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
    /// Named sandbox create profiles. Seeded from YAML AgentConfig when empty.
    #[serde(default)]
    pub sandbox_profiles: BTreeMap<String, SandboxProfile>,
    /// Global default profile id. Projects may override via `sandbox_profile_id`.
    #[serde(default)]
    pub default_sandbox_profile_id: Option<String>,
    /// Per-install forge/repo binding. Seeded from yaml; Board is SoT after.
    #[serde(default)]
    pub workspace: Option<WorkspaceBinding>,
    /// Optional OpenShell CLI path override (Settings → OpenShell). Empty/None → `openshell` on PATH.
    #[serde(default)]
    pub openshell_bin: Option<String>,
    /// Process agent knobs (Settings → Agent runtime). Seeded from yaml; Board SoT after.
    #[serde(default)]
    pub agent_runtime: Option<AgentRuntimeConfig>,
    #[serde(skip)]
    pub agent_logs: BTreeMap<ItemId, std::collections::VecDeque<String>>,
    /// Parent → child ids. Rebuilt after load; maintained on create/delete.
    /// Avoids full `items` scans in `children_of` / `has_children` / snapshot members.
    #[serde(skip)]
    pub children_by_parent: BTreeMap<ItemId, BTreeSet<ItemId>>,
    /// State → item ids. Rebuilt after load; maintained on create/delete/transition.
    /// Avoids full `items` scans in list_backlog / list_awaiting_dispatch / sweep_leases.
    #[serde(skip)]
    pub ids_by_state: HashMap<State, BTreeSet<ItemId>>,
}

impl BoardState {
    /// Snapshot for durable flush — drops in-process-only agent log rings.
    fn clone_for_persist(&self) -> Self {
        Self {
            next_id: self.next_id,
            items: self.items.clone(),
            stories: self.stories.clone(),
            sandbox_profiles: self.sandbox_profiles.clone(),
            default_sandbox_profile_id: self.default_sandbox_profile_id.clone(),
            workspace: self.workspace.clone(),
            openshell_bin: self.openshell_bin.clone(),
            agent_runtime: self.agent_runtime.clone(),
            agent_logs: BTreeMap::new(),
            children_by_parent: BTreeMap::new(),
            ids_by_state: HashMap::new(),
        }
    }

    /// Rebuild secondary indexes from `items`. Call after JSON/DB load.
    pub fn rebuild_hot_indexes(&mut self) {
        self.children_by_parent.clear();
        self.ids_by_state.clear();
        let snapshot: Vec<(ItemId, Option<ItemId>, State)> = self
            .items
            .values()
            .map(|i| (i.id, i.parent, i.state))
            .collect();
        for (id, parent, state) in snapshot {
            if let Some(p) = parent {
                self.children_by_parent.entry(p).or_default().insert(id);
            }
            self.ids_by_state.entry(state).or_default().insert(id);
        }
    }

    fn index_link_item(&mut self, item: &WorkItem) {
        if let Some(p) = item.parent {
            self.children_by_parent.entry(p).or_default().insert(item.id);
        }
        self.ids_by_state.entry(item.state).or_default().insert(item.id);
    }

    fn index_unlink_item(&mut self, item: &WorkItem) {
        if let Some(p) = item.parent {
            if let Some(set) = self.children_by_parent.get_mut(&p) {
                set.remove(&item.id);
                if set.is_empty() {
                    self.children_by_parent.remove(&p);
                }
            }
        }
        if let Some(set) = self.ids_by_state.get_mut(&item.state) {
            set.remove(&item.id);
            if set.is_empty() {
                self.ids_by_state.remove(&item.state);
            }
        }
    }

    fn index_set_state(&mut self, id: ItemId, from: State, to: State) {
        if from == to {
            return;
        }
        if let Some(set) = self.ids_by_state.get_mut(&from) {
            set.remove(&id);
            if set.is_empty() {
                self.ids_by_state.remove(&from);
            }
        }
        self.ids_by_state.entry(to).or_default().insert(id);
    }

    /// Insert a new item and update hot indexes.
    pub fn insert_item(&mut self, item: WorkItem) {
        self.index_link_item(&item);
        self.items.insert(item.id, item);
    }

    /// Remove an item and update hot indexes.
    pub fn remove_item(&mut self, id: ItemId) -> Option<WorkItem> {
        let item = self.items.remove(&id)?;
        self.index_unlink_item(&item);
        Some(item)
    }

    /// Non-retired child count (denorm field mirrored into SQLite on flush).
    pub fn non_retired_child_count(&self, id: ItemId) -> u32 {
        self.children_by_parent
            .get(&id)
            .map(|kids| {
                kids.iter()
                    .filter(|cid| {
                        self.items
                            .get(cid)
                            .map(|i| i.state != State::Retired)
                            .unwrap_or(false)
                    })
                    .count() as u32
            })
            .unwrap_or(0)
    }

    /// Unresolved blocker count (denorm field mirrored into SQLite on flush).
    pub fn open_blocker_count(&self, item: &WorkItem) -> u32 {
        item.blocked_by
            .iter()
            .filter(|b| {
                self.items
                    .get(b)
                    .map(|i| !i.state.is_terminal())
                    .unwrap_or(false)
            })
            .count() as u32
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
    /// Project auto mode — supervisor queues claimable Backlog leaves.
    #[serde(default)]
    pub auto_dispatch: bool,
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
    /// SQLite or Postgres row store. `None` in unit tests that stay in-memory/JSON.
    store: Option<Arc<DurableBoardStore>>,
    started_at: DateTime<Utc>,
    pub beads: Option<crate::beads::BeadsClient>,
    pub openshell: Option<crate::openshell::OpenShell>,
    in_flight_github_pushes: Mutex<
        std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    >,
    pushed_beads_ids: RwLock<std::collections::HashSet<String>>,
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

/// Binding note appended when a Review rebase conflict returns the card to
/// Backlog. Must reach the next claim briefing so a reused conversation does
/// not hollow-report while GitHub still says CONFLICTING.
pub fn conflict_bounce_note(conflicting_files: &[String]) -> String {
    let files = if conflicting_files.is_empty() {
        "(unknown)".to_string()
    } else {
        conflicting_files.join(", ")
    };
    format!(
        "BINDING: rebase conflict — conflicting files: {files}. \
         do-not-re-report-while-CONFLICTING; resolve onto upstream base before finishing."
    )
}

/// Parse `https://github.com/{owner}/{repo}/pull/{n}` (optional scheme / trailing slash).
/// Returns `(owner/repo, pull_number)`.
pub fn parse_github_pr_url(url: &str) -> Option<(String, u64)> {
    let url = url.trim().trim_end_matches('/');
    let marker = "github.com/";
    let idx = url.to_ascii_lowercase().find(marker)?;
    let rest = &url[idx + marker.len()..];
    let mut parts = rest.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    let pull = parts.next()?;
    let num = parts.next()?;
    if !pull.eq_ignore_ascii_case("pull") || owner.is_empty() || repo.is_empty() {
        return None;
    }
    // Ignore query/fragment on the number segment.
    let num = num.split(&['?', '#'][..]).next().unwrap_or(num);
    let n: u64 = num.parse().ok()?;
    Some((format!("{owner}/{repo}"), n))
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
            in_flight_github_pushes: Mutex::new(std::collections::HashMap::new()),
            pushed_beads_ids: RwLock::new(std::collections::HashSet::new()),
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
            item.migrate_legacy_pr_url();
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
                    state.rebuild_hot_indexes();
                    *board.state.write() = state;
                    let migrated = board.migrate_sandbox_policies_to_inline();
                    if healed > 0 || renamed > 0 || migrated > 0 {
                        if migrated > 0 {
                            tracing::info!(
                                "migrated {migrated} sandbox profile polic(ies) from host path to inline YAML"
                            );
                        }
                        board.dirty.store(true, Ordering::Relaxed);
                        board.flush();
                    }
                }
                Err(e) => tracing::warn!("ignoring unreadable {path:?}: {e}"),
            }
        }
        if board.seed_sandbox_profiles_if_empty() {
            tracing::info!("seeded sandbox profile catalog from execution.agents");
            board.flush();
        }
        if board.seed_workspace_binding_if_empty() {
            tracing::info!("seeded workspace binding from execution.agents.repo");
            board.flush();
        }
        if board.seed_agent_runtime_if_empty() {
            tracing::info!("seeded agent runtime from execution.agents");
            board.flush();
        }
        board.sync_beads_github_repository();
        board
    }

    /// Boot from the configured board database: one-shot import from `json_path`
    /// when the DB is empty, otherwise restore rows. Mutations flush as row
    /// updates, not a JSON rewrite.
    pub async fn load_with_store(
        schema: Schema,
        json_path: PathBuf,
        store: Arc<DurableBoardStore>,
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
        state.rebuild_hot_indexes();
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
        *board.state.write() = state;
        let migrated = board.migrate_sandbox_policies_to_inline();
        if healed > 0 || renamed > 0 || migrated > 0 {
            if migrated > 0 {
                tracing::info!(
                    "migrated {migrated} sandbox profile polic(ies) from host path to inline YAML"
                );
            }
            board.dirty.store(true, Ordering::Relaxed);
            board.flush();
        }
        if board.seed_sandbox_profiles_if_empty() {
            tracing::info!("seeded sandbox profile catalog from execution.agents");
            board.flush();
        }
        if board.seed_workspace_binding_if_empty() {
            tracing::info!("seeded workspace binding from execution.agents.repo");
            board.flush();
        }
        if board.seed_agent_runtime_if_empty() {
            tracing::info!("seeded agent runtime from execution.agents");
            board.flush();
        }
        board.sync_beads_github_repository();
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
        !self.state.read().items.get(&id).is_some_and(|it| it.parked)
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn current_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn record_and_send(&self, event: BoardEvent) {
        {
            let mut buffer = self.event_buffer.write();
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

        let buffer = self.event_buffer.read();
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
            let s = self.state.read();
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
    /// With a [`DurableBoardStore`] attached, this writes rows (not `honr.json`).
    /// Without a store (unit tests), the legacy whole-file JSON path remains.
    pub fn flush(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Some(store) = &self.store {
            let snapshot = self.state.read().clone_for_persist();
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
        let json = { serde_json::to_string_pretty(&*self.state.read()) };
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
        let s = self.state.read();
        s.items.get(&id).map(|i| {
            let mut item = i.clone();
            Self::populate_blockers(&s, &mut item);
            item
        })
    }

    pub fn children_of(&self, id: ItemId) -> Vec<ItemId> {
        let s = self.state.read();
        Self::children_of_indexed(&s, id)
    }

    /// Children via `children_by_parent` (not a full items scan).
    fn children_of_indexed(s: &BoardState, id: ItemId) -> Vec<ItemId> {
        s.children_by_parent
            .get(&id)
            .map(|kids| kids.iter().copied().collect())
            .unwrap_or_default()
    }

    fn has_children(s: &BoardState, id: ItemId) -> bool {
        s.non_retired_child_count(id) > 0
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
        let s = self.state.read();
        Self::goal_of(&s, id)
    }

    pub fn ancestry(&self, id: ItemId) -> Vec<AncestryLine> {
        let s = self.state.read();
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
        let mut s = self.state.write();
        let logs = s.agent_logs.entry(id).or_default();
        if logs.len() >= 300 {
            logs.pop_front();
        }
        logs.push_back(line.into());
    }

    pub fn get_agent_logs(&self, id: ItemId) -> Vec<String> {
        let s = self.state.read();
        s.agent_logs.get(&id).map(|l| l.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn clear_agent_logs(&self, id: ItemId) {
        let mut s = self.state.write();
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
        let from = {
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
            from
        };
        if from != to {
            s.index_set_state(id, from, to);
        }
        let mut item_out = s.items.get(&id).unwrap().clone();
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
            let mut s = self.state.write();
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
        // reaches Done (Approve, or PR merge via webhook if Approve never ran).
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
            let s = self.state.read();
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
            let mut s = self.state.write();
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
            s.insert_item(item.clone());
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
        let s = self.state.read();
        Self::initial_plan_of_locked(&s, project_id)
    }

    fn initial_plan_of_locked(s: &BoardState, project_id: ItemId) -> Option<WorkItem> {
        s.items
            .values()
            .find(|i| i.parent == Some(project_id) && i.is_initial_plan_task())
            .cloned()
    }

    /// GoalView label from the Initial plan card (not Project.plan).
    ///
    /// Takes `&BoardState` so callers that already hold `state` (e.g. snapshot)
    /// do not re-enter `RwLock` — std's lock is not reentrant and that freezes
    /// the whole process on `/api/board`.
    fn plan_status_label(s: &BoardState, project_id: ItemId) -> String {
        let Some(seed) = Self::initial_plan_of_locked(s, project_id) else {
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
                let has_impl = s.items.values().any(|c| {
                    c.parent == Some(project_id)
                        && !c.is_initial_plan_task()
                        && c.state != State::Retired
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
                            let mut s = self.state.write();
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
        self.pushed_beads_ids.read().contains(beads_id)
    }

    pub fn mark_beads_id_pushed(&self, beads_id: &str) {
        self.pushed_beads_ids
            .write()
            .insert(beads_id.to_string());
    }

    fn cleanup_in_flight_lock(&self, beads_id: &str, lock: &Arc<tokio::sync::Mutex<()>>) {
        let mut map = self.in_flight_github_pushes.lock();
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
            let mut map = self.in_flight_github_pushes.lock();
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
        let meta = crate::beads::BeadsClient::honr_metadata(id, item.pr_url());

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
                let repo = self.beads_github_repository();
                if let Some(url) = show_issue.github_issue_url_for_repo(repo.as_deref()) {
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
            let s = self.state.read();
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
            let s = self.state.read();
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
                        let s = self.state.read();
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
            let s = self.state.read();
            s.items
                .values()
                .filter(|i| i.parent.is_none() && i.state != State::Done && i.state != State::Retired)
                .map(|i| (i.id, i.state))
                .collect()
        };

        for (pid, state) in project_ids {
            let (child_count, all_done) = {
                let s = self.state.read();
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
            let mut s = self.state.write();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.beads_id = Some(beads_id.to_string());
            it.clone()
        };
        self.emit(&item);
    }

    pub fn set_github_issue_url(&self, id: ItemId, url: &str) {
        let item = {
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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
            let mut s = self.state.write();
            let Some(it) = s.items.get_mut(&id) else { return };
            if it.run_failures == 0 {
                return;
            }
            it.run_failures = 0;
            it.clone()
        };
        self.emit(&item);
    }

    /// Backlog → Claimed so restart can adopt a sandbox that survived Ctrl-C.
    ///
    /// Older supervisors treated follower exit -1 as a card failure and bounced
    /// Running → Backlog while leaving the setsid agent alive. `environment`
    /// still names that sandbox; this re-opens the lease without a full claim
    /// briefing (adoption only needs Claimed + lease + deadline).
    pub fn reopen_for_adoption(
        &self,
        id: ItemId,
        agent_id: &str,
        timeout_secs: i64,
    ) -> Result<WorkItem, String> {
        let now = Utc::now();
        let deadline = now + Duration::seconds(timeout_secs.max(1));
        let item = {
            let mut s = self.state.write();
            let parked = s.items.get(&id).is_some_and(|it| it.parked);
            if parked {
                return Err(format!("#{id} is parked"));
            }
            let state = s.items.get(&id).map(|it| it.state);
            if state != Some(State::Backlog) {
                return Err(format!("#{id} is not Backlog (reopen needs Backlog)"));
            }
            Self::transition_locked(
                &mut s,
                id,
                State::Claimed,
                "supervisor",
                Some("reopened for sandbox re-adoption after supervisor restart".into()),
            )
            .map_err(|e| e.to_string())?;
            let it = s.items.get_mut(&id).unwrap();
            it.awaiting_dispatch = false;
            it.run_deadline_at = Some(deadline);
            it.lease = Some(Lease {
                agent_id: agent_id.to_string(),
                granted_at: now,
                last_heartbeat: now,
                expires_at: deadline,
            });
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Which sandbox this card is running in. Written before the agent starts,
    /// so a honr that dies mid-run can still find the sandbox on restart.
    pub fn set_environment(&self, id: ItemId, sandbox: Option<String>) {
        let item = {
            let mut s = self.state.write();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.environment = sandbox;
            it.clone()
        };
        self.emit(&item);
    }

    /// Persist (or clear) the agy conversation id for park/resume.
    pub fn set_conversation_id(&self, id: ItemId, conversation_id: Option<String>) {
        let item = {
            let mut s = self.state.write();
            let Some(it) = s.items.get_mut(&id) else { return };
            if it.conversation_id == conversation_id {
                return;
            }
            it.conversation_id = conversation_id;
            it.clone()
        };
        self.emit(&item);
    }

    /// Replace the card's [`crate::model::PullRequest`] (url + optional base/head).
    pub fn set_pull_request(&self, id: ItemId, pr: Option<crate::model::PullRequest>) {
        let item = {
            let mut s = self.state.write();
            let Some(it) = s.items.get_mut(&id) else {
                return;
            };
            it.pull_request = pr;
            it.legacy_pr_url = None;
            it.clone()
        };
        self.emit(&item);

        if let (Some(beads), Some(bid)) = (self.beads.clone(), item.beads_id.clone()) {
            if crate::beads::BeadsClient::is_real_id(&bid) {
                let meta = crate::beads::BeadsClient::honr_metadata(id, item.pr_url());
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Err(e) = beads
                            .update_fields(&bid, None, None, Some(&meta))
                            .await
                        {
                            tracing::warn!(%bid, error = %e, "beads pull_request metadata sync failed");
                        }
                    });
                }
            }
        }
    }

    /// Set or clear `pull_request.url`, preserving base/head when present.
    pub fn set_pr_url(&self, id: ItemId, url: Option<String>) {
        let url = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        let next = {
            let cur = self.get(id).and_then(|i| i.pull_request);
            match (url, cur) {
                (None, _) => None,
                (Some(u), Some(mut pr)) => {
                    pr.url = u;
                    Some(pr)
                }
                (Some(u), None) => Some(crate::model::PullRequest::from_url(u)),
            }
        };
        self.set_pull_request(id, next);
    }

    pub fn set_blocked_by(&self, id: ItemId, blockers: Vec<ItemId>) {
        let item = {
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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

    // ------------------------------------------------ sandbox profiles (board state)
    //
    // Public surface for the follow-on api-supervisor card. Unit tests exercise
    // it; production callers land with REST/MCP wiring.

    /// Seed one profile from YAML AgentConfig when the catalog is empty.
    /// Returns true when a profile was inserted. YAML remains fallback only
    /// after the catalog is populated. Policy is stored as YAML **content**
    /// (file at `agents.policy` is read once at seed).
    pub fn seed_sandbox_profiles_if_empty(&self) -> bool {
        self.seed_sandbox_profiles_from(&self.schema.execution.agents)
    }

    /// Same as [`Self::seed_sandbox_profiles_if_empty`] but with an explicit
    /// AgentConfig (tests and callers that don't want schema.agents).
    pub fn seed_sandbox_profiles_from(&self, agents: &AgentConfig) -> bool {
        let mut s = self.state.write();
        if !s.sandbox_profiles.is_empty() {
            return false;
        }
        let id = "default".to_string();
        s.sandbox_profiles.insert(
            id.clone(),
            SandboxProfile {
                id: id.clone(),
                name: "Default".into(),
                image: agents.image.clone(),
                policy: resolve_policy_yaml(&agents.policy),
                cpu: agents.cpu.clone(),
                memory: agents.memory.clone(),
            },
        );
        s.default_sandbox_profile_id = Some(id);
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        true
    }

    // ------------------------------------------------ workspace binding (board state)

    /// Seed Forge binding (beads sync) from env / yaml when unbound.
    /// Work remotes stay in yaml `execution.agents.repo` only — not Settings.
    pub fn seed_workspace_binding_if_empty(&self) -> bool {
        self.seed_workspace_binding_from(&self.schema.execution.agents)
    }

    /// Same as [`Self::seed_workspace_binding_if_empty`] with an explicit AgentConfig.
    pub fn seed_workspace_binding_from(&self, agents: &AgentConfig) -> bool {
        let mut s = self.state.write();
        if s.workspace.as_ref().is_some_and(|w| w.has_beads_sync()) {
            return false;
        }
        let mut beads = std::env::var("GITHUB_REPOSITORY")
            .ok()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty());
        if beads.is_none() {
            let up = agents.repo.upstream.trim();
            if !up.is_empty() {
                beads = Some(up.to_string());
            }
        }
        let Some(beads_sync_repo) = beads else {
            return false;
        };
        s.workspace = Some(WorkspaceBinding {
            forge: "github".into(),
            beads_sync_repo: Some(beads_sync_repo),
        });
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        true
    }

    pub fn workspace_binding(&self) -> Option<WorkspaceBinding> {
        self.state.read().workspace.clone()
    }

    /// Replace the durable Forge binding (provider + beads sync). REST:
    /// `GET`/`PUT /api/workspace` (Settings → Forge).
    pub fn set_workspace_binding(&self, binding: WorkspaceBinding) -> Result<WorkspaceBinding, String> {
        let forge = binding.forge.trim();
        if forge.is_empty() {
            return Err("forge provider must not be empty".into());
        }
        if forge != "github" {
            return Err(format!(
                "forge {forge:?} is not supported yet (only github)"
            ));
        }
        let stored = WorkspaceBinding {
            forge: forge.to_string(),
            beads_sync_repo: binding
                .beads_sync_repo
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        };
        {
            let mut s = self.state.write();
            s.workspace = Some(stored.clone());
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.sync_beads_github_repository();
        Ok(stored)
    }

    // ------------------------------------------------ OpenShell connectivity (board state)

    /// Effective OpenShell CLI binary: Settings override, else `openshell`.
    pub fn openshell_bin(&self) -> String {
        self.state
            .read()
            .openshell_bin
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::openshell::DEFAULT_BIN.to_string())
    }

    /// Optional override as stored (None / empty → use PATH default).
    pub fn openshell_bin_override(&self) -> Option<String> {
        self.state
            .read()
            .openshell_bin
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Persist optional OpenShell binary path. Empty clears the override.
    pub fn set_openshell_bin(&self, bin: Option<String>) -> Option<String> {
        let stored = bin
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        {
            let mut s = self.state.write();
            s.openshell_bin = stored.clone();
        }
        self.dirty.store(true, Ordering::Relaxed);
        stored
    }

    /// Client using the board's configured binary (Settings override or default).
    pub fn openshell_client(&self) -> crate::openshell::OpenShell {
        crate::openshell::OpenShell::new(
            self.openshell_bin(),
            std::time::Duration::from_secs(120),
        )
    }

    // ------------------------------------------------ agent runtime (board state)

    /// Seed Agent runtime from yaml when unset. Returns true when inserted.
    pub fn seed_agent_runtime_if_empty(&self) -> bool {
        self.seed_agent_runtime_from(&self.schema.execution.agents)
    }

    /// Same as [`Self::seed_agent_runtime_if_empty`] with an explicit AgentConfig.
    pub fn seed_agent_runtime_from(&self, agents: &AgentConfig) -> bool {
        let mut s = self.state.write();
        if s.agent_runtime.is_some() {
            return false;
        }
        s.agent_runtime = Some(AgentRuntimeConfig {
            enabled: agents.enabled,
            engine: agents.engine.clone(),
            providers: agents.providers.clone(),
            vertex: AgentRuntimeVertex {
                project: agents.vertex.project.clone(),
                location: agents.vertex.location.clone(),
                model: agents.vertex.model.clone(),
            },
            max_concurrent: agents.max_concurrent,
            per_card_budget_cents: agents.per_card_budget_cents,
            daily_budget_cents: agents.daily_budget_cents,
            agent_timeout_secs: agents.agent_timeout_secs,
            max_attempts: agents.max_attempts,
            branch_prefix: agents.branch_prefix.clone(),
            quality_gates: agents.quality_gates.clone(),
        });
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        true
    }

    /// Durable Agent runtime (None until seeded or set via Settings).
    pub fn agent_runtime(&self) -> Option<AgentRuntimeConfig> {
        self.state.read().agent_runtime.clone()
    }

    /// Persist Agent runtime from Settings. Board is SoT after save.
    pub fn set_agent_runtime(&self, runtime: AgentRuntimeConfig) -> AgentRuntimeConfig {
        let stored = runtime.normalized();
        {
            let mut s = self.state.write();
            s.agent_runtime = Some(stored.clone());
        }
        self.dirty.store(true, Ordering::Relaxed);
        stored
    }

    /// AgentConfig for supervisor / sandbox create: durable Settings overlay on
    /// yaml (image/policy/cpu/memory/repo still come from yaml / profiles).
    pub fn effective_agents(&self) -> AgentConfig {
        self.agents_with_workspace(&self.schema.execution.agents)
    }

    /// Push the beads sync repo into the attached BeadsClient (if any).
    pub fn sync_beads_github_repository(&self) {
        let repo = self.beads_github_repository();
        if let Some(beads) = &self.beads {
            beads.set_github_repository(repo);
        }
    }

    /// Effective beads Issue repo: `GITHUB_REPOSITORY` env → Settings beads
    /// sync → yaml upstream. Never invents a default.
    pub fn beads_github_repository(&self) -> Option<String> {
        crate::beads::resolve_github_repository(self.configured_beads_repo().as_deref())
    }

    fn configured_beads_repo(&self) -> Option<String> {
        if let Some(ws) = self.workspace_binding() {
            if let Some(r) = ws.beads_repo() {
                return Some(r);
            }
        }
        // Legacy yaml upstream as beads fallback only — not a work-remote binding.
        self.yaml_work_repo().map(|r| r.upstream)
    }

    /// Legacy yaml `execution.agents.repo` when upstream is set. Not used for
    /// work remotes once a card has [`crate::model::PullRequest`] facts.
    pub fn yaml_work_repo(&self) -> Option<RepoConfig> {
        let yaml = &self.schema.execution.agents.repo;
        if yaml.is_complete() {
            Some(yaml.clone().normalized())
        } else {
            None
        }
    }

    /// AgentConfig from yaml with durable Settings → Agent runtime overlay.
    /// Remotes for a run still come from [`Self::resolve_card_repo`].
    pub fn agents_with_workspace(&self, yaml_agents: &AgentConfig) -> AgentConfig {
        let rt = self.agent_runtime();
        Self::overlay_agent_runtime(yaml_agents, rt.as_ref())
    }

    /// Pure overlay — safe to call while already holding `state` (RwLock is
    /// not reentrant; [`Self::snapshot`] must not call [`Self::agent_runtime`]).
    fn overlay_agent_runtime(
        yaml_agents: &AgentConfig,
        rt: Option<&AgentRuntimeConfig>,
    ) -> AgentConfig {
        let mut cfg = yaml_agents.clone();
        let Some(rt) = rt else {
            return cfg;
        };
        cfg.enabled = rt.enabled;
        cfg.engine = rt.engine.clone();
        cfg.providers = rt.providers.clone();
        cfg.vertex = crate::schema::VertexConfig {
            project: rt.vertex.project.clone(),
            location: rt.vertex.location.clone(),
            model: rt.vertex.model.clone(),
        };
        cfg.max_concurrent = rt.max_concurrent;
        cfg.per_card_budget_cents = rt.per_card_budget_cents;
        cfg.daily_budget_cents = rt.daily_budget_cents;
        cfg.agent_timeout_secs = rt.agent_timeout_secs;
        cfg.max_attempts = rt.max_attempts;
        cfg.branch_prefix = rt.branch_prefix.clone();
        cfg.quality_gates = rt.quality_gates.clone();
        cfg
    }

    /// Per-card work remotes for clone / push / rebase / PR-lookup.
    ///
    /// - `Ok(Some(repo))` from `pull_request` base/head (or URL-only same-repo stub)
    /// - `Ok(None)` first run — no remotes; supervisor skips pre-clone
    /// - `Err` malformed URL
    ///
    /// Does **not** require yaml and does not invent a bot fork.
    pub fn resolve_card_repo(&self, item_id: ItemId) -> Result<Option<RepoConfig>, String> {
        let item = self
            .get(item_id)
            .ok_or_else(|| format!("no such item #{item_id}"))?;

        if let Some(pr) = item.pull_request.as_ref() {
            if let Some(repo) = pr.to_repo_config() {
                return Ok(Some(repo));
            }
            if let Some(url) = pr.url_str() {
                if let Some((upstream, _)) = parse_github_pr_url(url) {
                    return Ok(Some(
                        RepoConfig {
                            upstream: upstream.clone(),
                            fork: upstream,
                            base: "main".into(),
                        }
                        .normalized(),
                    ));
                }
                return Err(format!(
                    "card #{item_id} pull_request.url is not a parseable GitHub pull URL: {url}"
                ));
            }
        }

        Ok(None)
    }

    /// Upgrade catalog entries that still store a host path as `policy`.
    /// Returns how many profiles were rewritten.
    pub fn migrate_sandbox_policies_to_inline(&self) -> usize {
        let mut s = self.state.write();
        let mut n = 0usize;
        for profile in s.sandbox_profiles.values_mut() {
            if let Some(content) = migrate_profile_policy_to_inline(&profile.policy) {
                profile.policy = content;
                n += 1;
            }
        }
        if n > 0 {
            drop(s);
            self.dirty.store(true, Ordering::Relaxed);
        }
        n
    }

    pub fn list_sandbox_profiles(&self) -> Vec<SandboxProfile> {
        let s = self.state.read();
        s.sandbox_profiles.values().cloned().collect()
    }

    pub fn default_sandbox_profile_id(&self) -> Option<String> {
        self.state.read().default_sandbox_profile_id.clone()
    }

    pub fn get_sandbox_profile(&self, id: &str) -> Option<SandboxProfile> {
        self.state.read().sandbox_profiles.get(id).cloned()
    }

    /// Insert or replace a profile. Empty `id` means create: derive a slug from
    /// `name` and append `-2`, `-3`, … when that slug is already taken. Explicit
    /// ids still upsert in place (edit / seed). Does not change the global default.
    pub fn upsert_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<SandboxProfile, String> {
        if profile.name.trim().is_empty() {
            return Err("sandbox profile name must not be empty".into());
        }
        if profile.image.trim().is_empty() {
            return Err("sandbox profile image must not be empty".into());
        }
        // Empty-check with trim, but keep the YAML text as submitted (trailing
        // newline is normal for policy files / textareas).
        if profile.policy.trim().is_empty() {
            return Err("sandbox profile policy must not be empty".into());
        }
        let name = profile.name.trim().to_string();
        let image = profile.image.trim().to_string();
        // Keep YAML as submitted (trailing newline is normal for policy textareas).
        let policy = profile.policy;
        let cpu = profile.cpu.filter(|c| !c.trim().is_empty());
        let memory = profile.memory.filter(|m| !m.trim().is_empty());
        let mut s = self.state.write();
        let id = {
            let trimmed = profile.id.trim();
            if trimmed.is_empty() {
                let base = crate::model::slugify_sandbox_profile_id(&name);
                Self::allocate_unique_sandbox_profile_id(&s.sandbox_profiles, &base)
            } else {
                trimmed.to_string()
            }
        };
        let stored = SandboxProfile {
            id,
            name,
            image,
            policy,
            cpu,
            memory,
        };
        s.sandbox_profiles.insert(stored.id.clone(), stored.clone());
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(stored)
    }

    pub fn set_default_sandbox_profile(&self, id: &str) -> Result<(), String> {
        let mut s = self.state.write();
        if !s.sandbox_profiles.contains_key(id) {
            return Err(format!("no sandbox profile `{id}`"));
        }
        s.default_sandbox_profile_id = Some(id.to_string());
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Assign a Project's sandbox profile override. `None` clears (inherit global default).
    pub fn set_project_sandbox_profile(
        &self,
        project_id: ItemId,
        profile_id: Option<String>,
    ) -> Result<WorkItem, String> {
        let item = {
            let mut s = self.state.write();
            let it = s
                .items
                .get(&project_id)
                .ok_or_else(|| format!("no such item #{project_id}"))?;
            if !it.is_project() {
                return Err(format!("#{project_id} is not a Project"));
            }
            let resolved = match profile_id {
                None => None,
                Some(pid) => {
                    let pid = pid.trim().to_string();
                    if pid.is_empty() {
                        None
                    } else {
                        if !s.sandbox_profiles.contains_key(&pid) {
                            return Err(format!("no sandbox profile `{pid}`"));
                        }
                        Some(pid)
                    }
                }
            };
            let it = s.items.get_mut(&project_id).unwrap();
            it.sandbox_profile_id = resolved;
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Delete a profile. Refused while it is the global default or assigned to
    /// any Project — reassign / clear those first.
    pub fn delete_sandbox_profile(&self, id: &str) -> Result<(), String> {
        let mut s = self.state.write();
        if !s.sandbox_profiles.contains_key(id) {
            return Err(format!("no sandbox profile `{id}`"));
        }
        if s.default_sandbox_profile_id.as_deref() == Some(id) {
            return Err(format!(
                "cannot delete sandbox profile `{id}`: it is the global default; \
                 set another default first"
            ));
        }
        let in_use: Vec<ItemId> = s
            .items
            .values()
            .filter(|i| i.is_project() && i.sandbox_profile_id.as_deref() == Some(id))
            .map(|i| i.id)
            .collect();
        if !in_use.is_empty() {
            return Err(format!(
                "cannot delete sandbox profile `{id}`: in use by Project(s) {:?}; \
                 clear or reassign those overrides first",
                in_use
            ));
        }
        s.sandbox_profiles.remove(id);
        drop(s);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Resolve create knobs for a card at sandbox create.
    ///
    /// Order: Project `sandbox_profile_id` → board `default_sandbox_profile_id`
    /// → YAML `execution.agents` image/policy/cpu/memory. Missing catalog
    /// entries fall through to the next step (YAML is always last resort).
    pub fn resolve_sandbox_create(&self, item_id: ItemId) -> ResolvedSandboxCreate {
        let item = match self.get(item_id) {
            Some(i) => i,
            None => return ResolvedSandboxCreate::from_agents(&self.schema.execution.agents),
        };
        let project = if item.is_project() {
            Some(item)
        } else {
            item.parent.and_then(|pid| self.get(pid))
        };
        let override_id = project.as_ref().and_then(|p| p.sandbox_profile_id.clone());

        let s = self.state.read();
        if let Some(ref oid) = override_id {
            if let Some(p) = s.sandbox_profiles.get(oid) {
                return ResolvedSandboxCreate::from_profile(p);
            }
        }
        if let Some(ref did) = s.default_sandbox_profile_id {
            if let Some(p) = s.sandbox_profiles.get(did) {
                return ResolvedSandboxCreate::from_profile(p);
            }
        }
        drop(s);
        ResolvedSandboxCreate::from_agents(&self.schema.execution.agents)
    }

    // ------------------------------------------------------- the agent verbs

    /// A card still leased to this agent — survives a restart mid-flight. The
    /// supervisor's startup reconciliation is the next caller: honr restarts
    /// constantly while honr is what's being built, and sandboxes outlive it.
    #[allow(dead_code)]
    pub fn leased_to(&self, agent_id: &str) -> Option<ItemId> {
        let s = self.state.read();
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
    ///
    /// Uses `ids_by_state` + denormalized leaf/blocker checks (not a full scan).
    pub fn list_backlog(&self, capabilities: &[String]) -> Vec<WorkItem> {
        let s = self.state.read();
        let Some(ids) = s.ids_by_state.get(&State::Backlog) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| s.items.get(id))
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
    ///
    /// Uses `ids_by_state` + denormalized leaf/blocker checks (not a full scan).
    pub fn list_awaiting_dispatch(&self) -> Vec<WorkItem> {
        let s = self.state.read();
        let Some(ids) = s.ids_by_state.get(&State::Backlog) else {
            return Vec::new();
        };
        let mut items: Vec<_> = ids
            .iter()
            .filter_map(|id| s.items.get(id))
            .filter(|i| i.awaiting_dispatch && !i.parked)
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
            let mut s = self.state.write();
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
            let mut s = self.state.write();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.awaiting_dispatch = false;
            it.rebase_requested = false;
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Play/pause Project auto mode. On: queue claimable Backlog leaves now.
    /// Off: clear `awaiting_dispatch` on still-Backlog leaves (does not halt runners).
    pub fn set_auto_dispatch(&self, id: ItemId, enabled: bool) -> Result<WorkItem, String> {
        let (title, changed, item) = {
            let mut s = self.state.write();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            if it.level.as_deref() != Some("Project") && !it.is_project() {
                return Err("auto mode is only for Projects".into());
            }
            let changed = it.auto_dispatch != enabled;
            if changed {
                it.auto_dispatch = enabled;
            }
            let title = it.title.clone();
            let mut out = it.clone();
            Self::populate_blockers(&s, &mut out);
            (title, changed, out)
        };
        if !changed {
            return Ok(item);
        }
        self.emit(&item);
        if enabled {
            self.story(
                id,
                format!("{title}: auto mode on — claimable Backlog will start on its own."),
            );
            self.auto_enqueue_project(id);
        } else {
            let cleared = self.clear_awaiting_under_project(id);
            self.story(
                id,
                format!(
                    "{title}: auto mode off — cleared {cleared} queued Backlog card(s); \
                     in-flight runs continue."
                ),
            );
        }
        self.get(id).ok_or_else(|| "no such item".into())
    }

    /// Clear `awaiting_dispatch` on Backlog leaves under a Project. Returns count cleared.
    fn clear_awaiting_under_project(&self, project_id: ItemId) -> usize {
        let ids: Vec<ItemId> = {
            let s = self.state.read();
            s.items
                .values()
                .filter(|i| i.parent == Some(project_id))
                .filter(|i| i.state == State::Backlog && i.awaiting_dispatch)
                .map(|i| i.id)
                .collect()
        };
        for id in &ids {
            let _ = self.clear_dispatch(*id);
        }
        ids.len()
    }

    /// Queue every claimable Backlog leaf under Projects with `auto_dispatch`.
    /// Called each supervisor tick — skips cards already awaiting.
    pub fn auto_enqueue_all(&self) {
        let project_ids: Vec<ItemId> = {
            let s = self.state.read();
            s.items
                .values()
                .filter(|i| i.auto_dispatch && i.state != State::Retired)
                .filter(|i| i.level.as_deref() == Some("Project") || i.is_project())
                .map(|i| i.id)
                .collect()
        };
        for pid in project_ids {
            self.auto_enqueue_project(pid);
        }
    }

    /// Enqueue claimable Backlog leaves under one Project (already-queued skipped).
    pub fn auto_enqueue_project(&self, project_id: ItemId) {
        let candidates: Vec<ItemId> = {
            let s = self.state.read();
            let Some(project) = s.items.get(&project_id) else {
                return;
            };
            if !project.auto_dispatch {
                return;
            }
            s.items
                .values()
                .filter(|i| i.parent == Some(project_id))
                .filter(|i| i.state == State::Backlog && !i.awaiting_dispatch && !i.parked)
                .filter(|i| i.level.as_deref() != Some("Project"))
                .filter(|i| !Self::has_children(&s, i.id))
                .filter(|i| i.definition_of_done.is_some())
                .filter(|i| Self::unresolved_blockers(&s, i).is_empty())
                .map(|i| i.id)
                .collect()
        };
        for id in candidates {
            let _ = self.enqueue_dispatch(id);
        }
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
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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

/// Pick `base`, or `base-2`, `base-3`, … until unused in the catalog.
fn allocate_unique_sandbox_profile_id(
    existing: &BTreeMap<String, SandboxProfile>,
    base: &str,
) -> String {
    if !existing.contains_key(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !existing.contains_key(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
        if n == u32::MAX {
            // Pathological; still must terminate.
            return format!("{base}-{n}");
        }
    }
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

        if let Some(pr_url) = card.pr_url() {
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
            let s = self.state.read();
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
            let mut s = self.state.write();
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
            let s = self.state.read();
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
            let mut s = self.state.write();
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
                let mut s = self.state.write();
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
            let mut s = self.state.write();
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
    ///
    /// Clears stale bounce UI (`last_bounce_reason` / `last_conflict_files`) —
    /// a successful report means the conflict / infra bounce is no longer the
    /// story the drawer should tell.
    pub fn report(
        &self,
        id: ItemId,
        agent_id: &str,
        added: u32,
        removed: u32,
        gates: Vec<String>,
    ) -> Result<WorkItem, TransitionError> {
        {
            let mut s = self.state.write();
            if let Some(it) = s.items.get_mut(&id) {
                it.diff_added = added;
                it.diff_removed = removed;
                it.progress = 1.0;
                it.last_bounce_reason = None;
                it.last_conflict_files.clear();
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
            let mut s = self.state.write();
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
    ///
    /// Iterates only Claimed/Running via `ids_by_state` (not a full items scan).
    pub fn sweep_leases(&self) -> Vec<ItemId> {
        let now = Utc::now();
        let expired: Vec<ItemId> = {
            let s = self.state.read();
            let mut out = Vec::new();
            for state in [State::Claimed, State::Running] {
                let Some(ids) = s.ids_by_state.get(&state) else {
                    continue;
                };
                for id in ids {
                    let Some(i) = s.items.get(id) else {
                        continue;
                    };
                    let expired = i
                        .run_deadline_at
                        .map(|d| now > d)
                        .or_else(|| i.lease.as_ref().map(|l| l.is_expired(now)))
                        .unwrap_or(false);
                    if expired {
                        out.push(*id);
                    }
                }
            }
            out
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
            let mut s = self.state.write();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            it.notes.push(Note { at: Utc::now(), author: "human".into(), text });
            it.run_failures = 0;
            it.escalation = None;
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// True when the answer promises host-side work before the agent can finish,
    /// without embedding `pr_url=` proof facts yet.
    fn decision_defers_host_prerequisite(choice: &str) -> bool {
        let c = choice.to_ascii_lowercase();
        if c.contains("pr_url=") {
            return false;
        }
        let host_runs = c.contains("host runs")
            || c.contains("finish host")
            || c.contains("on the host board")
            || c.contains("on the host:");
        let reclaim_to_document = (c.contains("re-claim") || c.contains("reclaim"))
            && (c.contains("document") || c.contains("to document"));
        host_runs || (c.contains("host") && reclaim_to_document)
    }

    pub fn answer_escalation(&self, id: ItemId, choice: String) -> Result<WorkItem, String> {
        let title = {
            let mut s = self.state.write();
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
        // "Host runs X; re-claim to document" without Proof facts is a promise,
        // not evidence. Auto mode would reclaim immediately and the agent would
        // re-escalate (#174). Park until cockpit pastes `Proof: …` and unparks.
        // Already in Backlog — do not call `park()` (Backlog→Backlog is illegal).
        if Self::decision_defers_host_prerequisite(&choice) {
            let reason = "Decision defers a host-side prerequisite — parked until Proof \
facts are pasted, then unpark";
            let item = {
                let mut s = self.state.write();
                let it = s.items.get_mut(&id).ok_or("no such item")?;
                it.parked = true;
                it.awaiting_dispatch = false;
                it.notes.push(Note {
                    at: Utc::now(),
                    author: "human".into(),
                    text: format!("Parked: {reason}"),
                });
                it.clone()
            };
            self.emit(&item);
            self.story(
                id,
                format!("{title}: parked after host-deferred answer — {reason}."),
            );
            return Ok(item);
        }
        Ok(item)
    }

    /// Park — stop the agent, return the card to Backlog, keep sandbox + conversation,
    /// and hold the card until [`Self::unpark`]. Prefer this over halt when the
    /// run is wedged or needs a human nudge without amnesia.
    pub fn park(&self, id: ItemId, reason: Option<String>) -> Result<WorkItem, String> {
        let reason = reason.filter(|r| !r.trim().is_empty());
        let title = {
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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

    /// Halt — kill the agent, return the card to Backlog, discard the LLM session
    /// and delete the sandbox. Park is the keep-context path; halt starts clean.
    pub fn halt(&self, id: ItemId, reason: Option<String>) -> Result<WorkItem, String> {
        let env_to_delete = {
            let mut s = self.state.write();
            if let Some(it) = s.items.get_mut(&id) {
                it.conversation_id = None;
                it.parked = false;
                // Cleared before Backlog transition so the sweeper will not
                // preserve `honr-card-{id}-*` via the non-terminal prefix keep.
                it.environment.take()
            } else {
                None
            }
        };
        let item = self
            .transition(id, State::Backlog, "human", reason.or(Some("halted".into())))
            .map_err(|e| e.to_string())?;
        if let Some(env) = env_to_delete {
            let os = self.openshell.clone().unwrap_or_default();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = os.delete(&env).await;
                });
            }
        }
        self.story(
            item.id,
            format!(
                "{}: halted — session and sandbox discarded; next claim starts clean.",
                item.title
            ),
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
        let mut s = self.state.write();
        if !s.items.contains_key(&id) {
            return Err(format!("item #{id} not found"));
        }

        let mut to_delete = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            to_delete.push(cur);
            if let Some(kids) = s.children_by_parent.get(&cur) {
                for &cid in kids {
                    if !to_delete.contains(&cid) && !stack.contains(&cid) {
                        stack.push(cid);
                    }
                }
            }
        }

        for del_id in &to_delete {
            if let Some(it) = s.remove_item(*del_id) {
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
            // UI: "Approve — create Tasks". Materialize now even when a plan/docs
            // PR is attached — waiting on the merge webhook strands the cockpit
            // whenever the forwarder is down. Merge → Done stays idempotent.
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
                format!("{} approved — {} Tasks created.", done.title, n),
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

        // UI: "Approve & Move to Done". Waiting on the merge webhook alone strands
        // cards when the forwarder missed the event or the PR was already merged
        // while the card was still Running. Webhook → Done stays idempotent.
        let item = self
            .transition(id, State::Done, "human", Some("approved".into()))
            .map_err(|e| e.to_string())?;
        let story = match item.pr_url().filter(|u| !u.trim().is_empty()) {
            Some(url) => format!("{} approved — Done ({}).", item.title, url),
            None => format!("{} approved — no PR; marked Done.", item.title),
        };
        self.story(id, story);
        Ok(item)
    }

    pub fn request_changes(&self, id: ItemId, note: String) -> Result<WorkItem, String> {
        self.steer(id, format!("Changes requested: {note}"))?;
        {
            let mut s = self.state.write();
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
            let mut s = self.state.write();
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
        let s = self.state.read();
        let goal = Self::goal_of(&s, near);
        s.stories.get(&goal).cloned().unwrap_or_default()
    }

    /// Notify connected subscribers that the main branch advanced (via push or PR merge).
    ///
    /// Review cards with open PRs get a rebase dispatch. Claimed/Running cards get a
    /// steer note telling the agent to fetch and rebase onto upstream/main, then a
    /// park+unpark so the supervisor re-claims with that note — the supervisor does
    /// not touch the live worktree itself.
    pub fn notify_main_advanced(&self, ref_name: &str, commit_sha: Option<String>) {
        tracing::info!("main advanced: ref={ref_name}, commit={commit_sha:?}");
        self.record_and_send(BoardEvent::MainAdvanced {
            seq: self.next_seq(),
            ref_name: ref_name.to_string(),
            commit_sha: commit_sha.clone(),
        });
        self.trigger_rebase_for_all_behind_siblings();
        self.steer_live_cards_on_main_advanced(ref_name, commit_sha.as_deref());
    }

    /// Binding note for live runs when main moves under them.
    /// Uses the card's resolved base branch when available.
    fn main_advanced_steer_note(ref_name: &str, commit_sha: Option<&str>, base: &str) -> String {
        let where_main = match commit_sha {
            Some(sha) if !sha.is_empty() => format!("{ref_name} @ {sha}"),
            _ => ref_name.to_string(),
        };
        let base = if base.trim().is_empty() { "main" } else { base.trim() };
        format!(
            "Main advanced ({where_main}). First action: fetch upstream {base} and rebase \
             this card's branch onto upstream/{base} (not origin/{base} alone — the fork's \
             base freezes at create time), then continue the card."
        )
    }

    /// Steer every Claimed/Running card, then park+unpark so the resume briefing
    /// carries the rebase instruction. Steer alone does not inject mid-turn.
    /// Already-parked cards are left alone (no second park/unpark). Sandbox
    /// environment and conversation_id are preserved through the bounce.
    /// Each card gets a note using **its** resolved upstream/base.
    fn steer_live_cards_on_main_advanced(&self, ref_name: &str, commit_sha: Option<&str>) {
        let live_ids: Vec<ItemId> = {
            let s = self.state.read();
            s.items
                .values()
                .filter(|i| matches!(i.state, State::Claimed | State::Running))
                .map(|i| i.id)
                .collect()
        };
        for id in live_ids {
            let base = self
                .resolve_card_repo(id)
                .ok()
                .flatten()
                .map(|r| r.base)
                .unwrap_or_else(|| "main".into());
            let note = Self::main_advanced_steer_note(ref_name, commit_sha, &base);
            if let Err(e) = self.steer(id, note.clone()) {
                tracing::warn!("main-advanced steer failed for #{id}: {e}");
            }
            // Skip park/unpark for cards already parked — steer may have been a
            // no-op on a weird state, and a second park would be wrong.
            let already_parked = {
                let s = self.state.read();
                s.items.get(&id).is_some_and(|i| i.parked)
            };
            if already_parked {
                continue;
            }
            if let Err(e) = self.park(id, Some("main advanced".into())) {
                tracing::warn!("main-advanced park failed for #{id}: {e}");
                continue;
            }
            if let Err(e) = self.unpark(id) {
                tracing::warn!("main-advanced unpark failed for #{id}: {e}");
            }
        }
    }

    /// Identify open sibling PRs in Review that are behind main for a given item's parent.
    pub fn identify_behind_sibling_prs(&self, near_id: ItemId) -> Vec<WorkItem> {
        let s = self.state.read();
        let mut results = Vec::new();
        if let Some(item) = s.items.get(&near_id) {
            let parent_id = item.parent.unwrap_or(item.id);
            for child_id in s.items.values().filter(|i| i.parent == Some(parent_id)).map(|i| i.id) {
                if child_id == near_id {
                    continue;
                }
                if let Some(child) = s.items.get(&child_id) {
                    if child.state == State::Review && child.pr_url().is_some() {
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
        let s = self.state.read();
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
                    if child.state == State::Review && child.pr_url().is_some() {
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
            let mut s = self.state.write();
            let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item {id}"))?;
            if it.state != State::Review {
                return Err(format!(
                    "only Review cards can be rebased, #{id} is in {:?}",
                    it.state
                ));
            }
            if it.pr_url().is_none() {
                return Err(format!("card #{id} has no pull_request.url to rebase"));
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
        let s = self.state.read();
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
            let s = self.state.read();
            let it = s.items.get(&id).ok_or_else(|| format!("no such item #{id}"))?;
            if it.state != State::Review {
                return Err(format!("only Review cards can record rebase outcome, #{id} is in {:?}", it.state));
            }
            (it.title.clone(), it.last_conflict_files.clone())
        };

        match outcome {
            RebaseOutcome::Clean => {
                let item = {
                    let mut s = self.state.write();
                    let it = s.items.get_mut(&id).ok_or_else(|| format!("no such item #{id}"))?;
                    it.rebase_requested = false;
                    it.awaiting_dispatch = false;
                    it.last_bounce_reason = None;
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
                let binding_note = conflict_bounce_note(&curr_files);

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
                        let mut s = self.state.write();
                        if let Some(it) = s.items.get_mut(&id) {
                            it.last_bounce_reason = Some(bounce_reason.clone());
                            it.last_conflict_files = curr_files;
                            it.notes.push(Note {
                                at: Utc::now(),
                                author: "rebase".into(),
                                text: binding_note,
                            });
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
                        let mut s = self.state.write();
                        if let Some(it) = s.items.get_mut(&id) {
                            it.last_bounce_reason = Some(bounce_reason.clone());
                            it.last_conflict_files = curr_files;
                            it.notes.push(Note {
                                at: Utc::now(),
                                author: "rebase".into(),
                                text: binding_note,
                            });
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
            let s = self.state.read();
            s.items
                .values()
                .find(|i| {
                    matches!(i.state, State::Review | State::NeedsHuman)
                        && i.pr_url()
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
        let s = self.state.read();
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
        let s = self.state.read();
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

        // Project roots only (parent.is_none) — avoids goal_of over every item.
        let mut goal_ids: Vec<ItemId> = s
            .items
            .values()
            .filter(|i| i.parent.is_none())
            .map(|i| i.id)
            .collect();
        goal_ids.sort_unstable();

        let goals = goal_ids
            .into_iter()
            .filter_map(|gid| self.goal_view(&s, gid, now))
            .collect();

        // Overlay from `s` — do not call effective_agents()/agent_runtime()
        // while this read guard is held (RwLock is not reentrant; that freeze
        // made the UI show NOT LIVE after Agent runtime landed).
        let agents =
            Self::overlay_agent_runtime(&self.schema.execution.agents, s.agent_runtime.as_ref());
        Snapshot {
            items,
            levels: self.schema.levels.clone(),
            goals,
            server_time: now,
            agent_timeout_secs: agents.agent_timeout_secs,
            seq: self.seq.load(Ordering::Relaxed),
            default_engine: agents.engine,
            default_model: agents.vertex.model,
        }
    }

    fn goal_view(&self, s: &BoardState, gid: ItemId, now: DateTime<Utc>) -> Option<GoalView> {
        let goal = s.items.get(&gid)?;

        // Only Project roots are swimlanes. Nested nodes never get their own.
        if goal.parent.is_some() {
            return None;
        }
        let archived = goal.state == State::Retired;

        // Tasks under this Project — via children_by_parent (not a full scan).
        let member_ids = Self::children_of_indexed(s, gid);
        let members: Vec<&WorkItem> = member_ids
            .iter()
            .filter_map(|id| s.items.get(id))
            .collect();

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

        let plan_status = Self::plan_status_label(s, gid);

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
            auto_dispatch: goal.auto_dispatch,
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
        let s = self.state.read();
        let now = Utc::now();
        // Project roots only — avoids goal_of over every item.
        let mut goal_ids: Vec<ItemId> = s
            .items
            .values()
            .filter(|i| i.parent.is_none())
            .map(|i| i.id)
            .collect();
        goal_ids.sort_unstable();

        let goals = goal_ids
            .into_iter()
            .filter_map(|gid| {
                let goal = s.items.get(&gid)?;
                if goal.parent.is_some() {
                    return None;
                }
                if goal.state == State::Retired {
                    return None;
                }
                // Flat Tasks under Project + the Project itself (legacy goal_of set).
                let child_ids = Self::children_of_indexed(&s, gid);
                let mut members: Vec<&WorkItem> = child_ids
                    .iter()
                    .filter_map(|id| s.items.get(id))
                    .collect();
                members.push(goal);

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
    use crate::model::{AgentRuntimeConfig, AgentRuntimeVertex, Origin};

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

    /// Capture remotes invent Issue URLs only when a repo is configured
    /// (env / Workspace / client) — never via a Shane hardcode.
    fn bind_test_github_repo(client: &crate::beads::BeadsClient) {
        client.set_github_repository(Some("test-owner/test-repo".into()));
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
            let mut s = b.state.write();
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

    /// Filter semantics for hot paths that use denorm / secondary indexes.
    #[test]
    fn hot_path_filters_match_legacy_semantics() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-hot-filters.json"),
        );
        let project = b
            .create(None, "Proj", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);

        let leaf = b
            .create(
                Some(project.id),
                "leaf",
                "do",
                Some("done".into()),
                Origin::Human,
                false,
                Some("rust".into()),
            )
            .expect("leaf");
        let _ = b.transition(leaf.id, State::Shaping, "t", None);
        let _ = b.transition(leaf.id, State::Backlog, "t", None);

        let blocked = b
            .create(
                Some(project.id),
                "blocked",
                "wait",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("blocked");
        let _ = b.transition(blocked.id, State::Shaping, "t", None);
        let _ = b.transition(blocked.id, State::Backlog, "t", None);
        b.set_blocked_by(blocked.id, vec![leaf.id]);

        let container_child = b
            .create(
                Some(project.id),
                "container-slot",
                "placeholder",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("slot");
        let _ = b.transition(container_child.id, State::Shaping, "t", None);
        let _ = b.transition(container_child.id, State::Backlog, "t", None);
        // Project has children → not claimable; leaf with wrong capability excluded.
        let ready_rust = b.list_ready(&["rust".into()]);
        let ready_ids: Vec<_> = ready_rust.iter().map(|i| i.id).collect();
        assert!(
            ready_ids.contains(&leaf.id),
            "rust capability matches rust leaf: {ready_ids:?}"
        );
        assert!(
            !ready_ids.contains(&blocked.id),
            "blocked leaf must be excluded"
        );
        assert!(
            !ready_ids.contains(&project.id),
            "Project must be excluded"
        );

        let ready_python = b.list_ready(&["python".into()]);
        assert!(
            !ready_python.iter().any(|i| i.id == leaf.id),
            "capability mismatch excludes leaf"
        );

        // Parent/child helpers use children_by_parent.
        assert!(Board::has_children(
            &b.state.read(),
            project.id
        ));
        assert!(!Board::has_children(
            &b.state.read(),
            leaf.id
        ));
        let kids = b.children_of(project.id);
        assert!(kids.contains(&leaf.id));
        assert!(kids.contains(&blocked.id));
        assert_eq!(
            b.state.read().non_retired_child_count(project.id) as usize,
            kids.len()
        );

        // Dispatch queue: only unblocked leaf with flag.
        assert!(b.list_awaiting_dispatch().is_empty());
        b.enqueue_dispatch(leaf.id).expect("enqueue");
        assert_eq!(
            b.list_awaiting_dispatch()
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>(),
            vec![leaf.id]
        );
        assert!(
            b.enqueue_dispatch(blocked.id).is_err(),
            "blocked card cannot dispatch"
        );

        // Digest ready_to_dispatch excludes awaiting_dispatch and blocked.
        let digest = b.digest();
        let goal = digest
            .goals
            .iter()
            .find(|g| g.goal_id == project.id)
            .expect("goal");
        assert!(
            !goal.ready_to_dispatch.iter().any(|c| c.id == leaf.id),
            "enqueued leaf not in ready_to_dispatch"
        );
        assert!(
            !goal.ready_to_dispatch.iter().any(|c| c.id == blocked.id),
            "blocked not ready"
        );
        assert!(
            goal.ready_to_dispatch
                .iter()
                .any(|c| c.id == container_child.id),
            "idle unblocked leaf is ready: {:?}",
            goal.ready_to_dispatch
        );

        // Indexes stay consistent across transition + delete.
        b.transition(leaf.id, State::Done, "t", Some("done".into()))
            .expect("done");
        assert!(
            b.list_ready(&["rust".into(), "python".into(), "any".into()])
                .iter()
                .any(|i| i.id == blocked.id),
            "completing blocker unblocks dependent"
        );
        let before = b.children_of(project.id).len();
        b.delete_item(container_child.id).expect("delete");
        assert_eq!(b.children_of(project.id).len(), before - 1);
        assert_eq!(
            b.state.read().ids_by_state.get(&State::Backlog).map(|s| s.len()),
            Some(
                b.state
                    .read()
                    .items
                    .values()
                    .filter(|i| i.state == State::Backlog)
                    .count()
            )
        );
    }

    #[test]
    fn snapshot_uses_project_roots_and_child_index() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-snap-index.json"),
        );
        let p1 = b
            .create(None, "A", "a", None, Origin::Human, true, None)
            .unwrap();
        let p2 = b
            .create(None, "B", "b", None, Origin::Human, true, None)
            .unwrap();
        let _ = b.transition(p1.id, State::Shaping, "t", None);
        let _ = b.transition(p2.id, State::Shaping, "t", None);
        let t = b
            .create(
                Some(p1.id),
                "task",
                "x",
                Some("d".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let _ = b.transition(t.id, State::Shaping, "t", None);
        let _ = b.transition(t.id, State::Backlog, "t", None);

        let snap = b.snapshot();
        let goal_ids: Vec<_> = snap.goals.iter().map(|g| g.id).collect();
        assert!(goal_ids.contains(&p1.id));
        assert!(goal_ids.contains(&p2.id));
        let g1 = snap.goals.iter().find(|g| g.id == p1.id).unwrap();
        // Initial plan Task + our task = leaves under p1.
        assert!(g1.leaves_total >= 1);
    }

    #[test]
    fn project_auto_dispatch_queues_and_pauses() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-auto-dispatch.json"),
        );
        let project = b
            .create(None, "goal", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let ready = b
            .create(
                Some(project.id),
                "ready",
                "do it",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("ready");
        let parked = b
            .create(
                Some(project.id),
                "parked",
                "wait",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("parked");
        let blocked = b
            .create(
                Some(project.id),
                "blocked",
                "later",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("blocked");
        for id in [ready.id, parked.id, blocked.id] {
            let _ = b.transition(id, State::Shaping, "t", None);
            let _ = b.transition(id, State::Backlog, "t", None);
        }
        b.set_blocked_by(blocked.id, vec![ready.id]);
        {
            let mut s = b.state.write();
            s.items.get_mut(&parked.id).unwrap().parked = true;
        }

        // Retire the seeded Initial plan so it does not join the auto queue.
        if let Some(ip) = b.initial_plan_of(project.id) {
            let _ = b.transition(ip.id, State::Retired, "t", Some("test".into()));
        }

        assert!(b.list_awaiting_dispatch().is_empty());
        let proj = b.set_auto_dispatch(project.id, true).expect("auto on");
        assert!(proj.auto_dispatch);
        let awaiting = b.list_awaiting_dispatch();
        assert_eq!(awaiting.len(), 1, "only the unblocked unparked leaf");
        assert_eq!(awaiting[0].id, ready.id);
        assert!(!b.get(parked.id).unwrap().awaiting_dispatch);
        assert!(!b.get(blocked.id).unwrap().awaiting_dispatch);

        // Idempotent — already queued stays queued, no second phantom.
        b.auto_enqueue_all();
        assert_eq!(b.list_awaiting_dispatch().len(), 1);

        let snap = b.snapshot();
        let goal = snap.goals.iter().find(|g| g.id == project.id).expect("goal");
        assert!(goal.auto_dispatch);

        let proj = b.set_auto_dispatch(project.id, false).expect("auto off");
        assert!(!proj.auto_dispatch);
        assert!(
            b.list_awaiting_dispatch().is_empty(),
            "pause clears queued Backlog"
        );
        assert!(!b.get(ready.id).unwrap().awaiting_dispatch);

        let err = b.set_auto_dispatch(ready.id, true).unwrap_err();
        assert!(err.contains("Projects"), "{err}");
    }

    #[test]
    fn halt_clears_conversation_and_environment() {
        let (b, id) = claimed_leaf();
        b.set_environment(id, Some("honr-card-1-a1".into()));
        b.set_conversation_id(id, Some("conv-xyz".into()));
        let it = b.halt(id, Some("start over".into())).expect("halt");
        assert_eq!(it.state, State::Backlog);
        assert!(it.environment.is_none(), "halt deletes the sandbox binding");
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

    /// Ctrl-C used to bounce Running → Backlog while the sandbox agent lived;
    /// restart must be able to Claim again without a full dispatch claim.
    #[test]
    fn reopen_for_adoption_from_backlog_with_environment() {
        let (b, id) = claimed_leaf();
        b.set_environment(id, Some("honr-card-1-a1".into()));
        b.record_run_failure(id, "agent exited -1: follower died", 3)
            .expect("failed into backlog");
        assert_eq!(b.get(id).unwrap().state, State::Backlog);

        let it = b
            .reopen_for_adoption(id, "sandbox-1", 3600)
            .expect("reopened");
        assert_eq!(it.state, State::Claimed);
        assert_eq!(it.environment.as_deref(), Some("honr-card-1-a1"));
        assert!(it.lease.is_some());
        assert!(it.run_deadline_at.is_some());
        assert!(!it.awaiting_dispatch);
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
        assert!(!it.parked, "ordinary answers must not auto-park");

        // The decision survives as standing context for whoever picks it up.
        assert!(
            it.notes.iter().any(|n| n.text.contains("Investigate the environment")),
            "the decision must be preserved as a note: {:?}",
            it.notes
        );
    }

    /// Host-deferred answers without Proof facts must park — otherwise Project
    /// auto mode reclaims and the agent re-escalates (#174).
    #[test]
    fn answering_host_deferred_decision_parks_until_proof() {
        let (b, id) = claimed_leaf();
        b.record_run_failure(id, "boom", 1).expect("escalated");
        b.answer_escalation(
            id,
            "Host runs probe dispatch; re-claim this card to document".into(),
        )
        .expect("answered");
        let it = b.get(id).unwrap();
        assert_eq!(it.state, State::Backlog);
        assert!(it.parked, "host-deferred Decision must park");
        assert!(it.escalation.is_none());

        // Pasting pr_url= in the answer is evidence — do not park.
        let (b2, id2) = claimed_leaf();
        b2.record_run_failure(id2, "boom", 1).expect("escalated");
        b2.answer_escalation(
            id2,
            "Paste run facts now: card=#9 pr_url=https://github.com/clankrshq/honr-sandbox-probe/pull/2 upstream=clankrshq/honr-sandbox-probe"
                .into(),
        )
        .expect("answered");
        let it2 = b2.get(id2).unwrap();
        assert!(!it2.parked, "answers that already embed pr_url= must not park");
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
        bind_test_github_repo(&beads_client);

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

        let published = b.approve_plan(project.id).expect("approve creates tasks");
        assert_eq!(published.len(), 1);
        assert_eq!(b.get(seed_id).unwrap().state, State::Done);
        assert_eq!(
            b.children_of(project.id)
                .into_iter()
                .filter(|&id| !b.get(id).unwrap().is_initial_plan_task())
                .count(),
            1
        );
        // Late webhook must not invent a second set of Tasks.
        assert!(
            b.complete_for_merged_pr("https://example.com/pr/1", Some(1))
                .is_none(),
            "already-Done Initial plan ignores merge webhook"
        );
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

        // Approve creates Tasks even with a plan/docs PR attached.
        let done = b.approve_review(seed_id).expect("approve_review");
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
        let s = b.state.read();
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
    fn snapshot_effective_agents_does_not_reenter_rwlock() {
        // Regression: Agent runtime made snapshot call effective_agents() →
        // agent_runtime() while still holding state.read(); freeze → NOT LIVE.
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-snapshot-agents-reenter.json"),
        );
        assert!(b.seed_agent_runtime_if_empty());
        let snap = b.snapshot();
        assert!(!snap.default_engine.is_empty());
        assert!(snap.agent_timeout_secs > 0);
    }

    #[test]
    fn snapshot_plan_status_does_not_reenter_rwlock() {
        // Regression: goal_view → plan_status_label used to call children_of while
        // snapshot still held state.read(); std RwLock is not reentrant → freeze.
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-snapshot-reenter.json"),
        );
        let project = b
            .create(None, "Reenter", "why", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(project.id, State::Shaping, "t", None);
        let seed_id = b
            .children_of(project.id)
            .into_iter()
            .find(|&id| b.get(id).is_some_and(|i| i.is_initial_plan_task()))
            .expect("seed");
        let _ = b.transition(seed_id, State::Done, "t", Some("legacy".into()));
        // Clear proposal so plan_status walks the impl-children path.
        {
            let mut s = b.state.write();
            if let Some(seed) = s.items.get_mut(&seed_id) {
                seed.proposal = None;
            }
        }
        let _ = b
            .create(
                Some(project.id),
                "Impl",
                "do",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("impl");
        let snap = b.snapshot();
        let g = snap.goals.iter().find(|g| g.id == project.id).expect("goal");
        assert_eq!(g.plan_status, "approved");
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
        bind_test_github_repo(&beads_client);

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
        bind_test_github_repo(&beads_client);

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
        bind_test_github_repo(&beads_client);

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
        bind_test_github_repo(&beads_client);

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
            let s = b.state.read();
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
            let s = b.state.read();
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

        // 5. Halt clears environment and deletes the sandbox
        let t4 = b
            .create(Some(p.id), "Task 4", "intent", Some("DOD".into()), Origin::Human, false, None)
            .unwrap();
        let _ = b.transition(t4.id, State::Shaping, "human", None);
        let _ = b.transition(t4.id, State::Backlog, "human", None);
        let _ = b.transition(t4.id, State::Claimed, "human", None);
        b.set_environment(t4.id, Some("honr-card-4-a1".into()));
        b.halt(t4.id, Some("start over".into())).expect("halt");
        assert_eq!(b.get(t4.id).unwrap().environment, None);

        eventually(|| {
            std::fs::read_to_string(&log_file)
                .unwrap_or_default()
                .contains("sandbox delete honr-card-4-a1")
        })
        .await;
        let log_content = std::fs::read_to_string(&log_file).unwrap_or_default();
        assert!(
            log_content.contains("sandbox delete honr-card-4-a1"),
            "expected 'sandbox delete honr-card-4-a1' in log, got: {log_content}"
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
        bind_test_github_repo(&beads_client);

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
        bind_test_github_repo(&beads_client);

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
    fn approve_review_with_pr_moves_to_done() {
        let b = Arc::new(Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-approve-with-pr-{}.json",
                std::process::id()
            )),
        ));
        let p = b
            .create(None, "Approve PR", "intent", None, Origin::Human, true, None)
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
        assert_eq!(item.state, State::Done, "Approve & Move to Done must complete PR cards");
        // Webhook after Approve is a no-op (idempotent).
        assert!(
            b.complete_for_merged_pr("https://github.com/shanemcd/honr/pull/99", Some(99))
                .is_none()
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
            let s = b.state.read();
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
    fn notify_main_advanced_steers_running_cards_with_fetch_rebase_note() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-main-steer-{}.json",
                std::process::id()
            )),
        );
        let project = b
            .create(None, "Steer Main Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let running = b
            .create(
                Some(project.id),
                "Live Task",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let claimed = b
            .create(
                Some(project.id),
                "Claimed Task",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let backlog = b
            .create(
                Some(project.id),
                "Idle Task",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();

        for id in [running.id, claimed.id, backlog.id] {
            b.transition(id, State::Shaping, "test", None).unwrap();
            b.transition(id, State::Backlog, "test", None).unwrap();
        }
        b.transition(running.id, State::Claimed, "agent", None).unwrap();
        b.transition(running.id, State::Running, "agent", None).unwrap();
        b.transition(claimed.id, State::Claimed, "agent", None).unwrap();

        b.notify_main_advanced("refs/heads/main", Some("abcdeadbeef".into()));

        let running_after = b.get(running.id).unwrap();
        assert_eq!(
            running_after.state,
            State::Backlog,
            "park+unpark must bounce the card to Backlog for resume"
        );
        assert!(
            running_after.awaiting_dispatch,
            "unpark must queue the supervisor"
        );
        assert!(!running_after.parked, "unpark clears the park hold");
        assert!(
            running_after.notes.iter().any(|n| {
                n.text.contains("abcdeadbeef")
                    && n.text.contains("fetch")
                    && n.text.contains("upstream/main")
                    && n.text.to_lowercase().contains("rebase")
            }),
            "Running card should have a fetch/rebase steer note with sha: {:?}",
            running_after.notes
        );

        let claimed_after = b.get(claimed.id).unwrap();
        assert_eq!(claimed_after.state, State::Backlog);
        assert!(claimed_after.awaiting_dispatch);
        assert!(
            claimed_after
                .notes
                .iter()
                .any(|n| n.text.contains("abcdeadbeef") && n.text.contains("upstream/main")),
            "Claimed cards should also be steered: {:?}",
            claimed_after.notes
        );

        let backlog_after = b.get(backlog.id).unwrap();
        assert!(
            backlog_after.notes.is_empty(),
            "Backlog cards must not get a main-advanced steer: {:?}",
            backlog_after.notes
        );
    }

    #[test]
    fn notify_main_advanced_parks_and_unparks_running_so_steer_takes_effect() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-main-park-unpark-{}.json",
                std::process::id()
            )),
        );
        let project = b
            .create(
                None,
                "Park Unpark Main Proj",
                "intent",
                None,
                Origin::Human,
                true,
                None,
            )
            .unwrap();
        let running = b
            .create(
                Some(project.id),
                "Live Task",
                "intent",
                Some("dod".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        b.transition(running.id, State::Shaping, "test", None).unwrap();
        b.transition(running.id, State::Backlog, "test", None).unwrap();
        b.transition(running.id, State::Claimed, "agent", None).unwrap();
        b.transition(running.id, State::Running, "agent", None).unwrap();
        b.set_environment(running.id, Some("honr-card-150-sandbox".into()));
        b.set_conversation_id(running.id, Some("conv-main-adv".into()));

        b.notify_main_advanced("refs/heads/main", Some("def456abc".into()));

        let after = b.get(running.id).unwrap();
        assert_eq!(after.state, State::Backlog);
        assert!(
            after.awaiting_dispatch,
            "unpark must leave the card queued for the supervisor"
        );
        assert!(!after.parked);
        assert_eq!(
            after.environment.as_deref(),
            Some("honr-card-150-sandbox"),
            "sandbox environment must survive park+unpark"
        );
        assert_eq!(
            after.conversation_id.as_deref(),
            Some("conv-main-adv"),
            "conversation_id must survive park+unpark"
        );
        assert!(
            after.notes.iter().any(|n| {
                n.text.contains("def456abc")
                    && n.text.contains("upstream/main")
                    && n.text.to_lowercase().contains("rebase")
            }),
            "notes must include the main-advanced steer: {:?}",
            after.notes
        );
        // Board path only: no supervisor/openshell mid-run git rebase — the
        // agent rebases on resume from the steer note.
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

        {
            let mut s = b.state.write();
            let it = s.items.get_mut(&t1.id).unwrap();
            it.last_bounce_reason = Some("prior conflict bounce".into());
            it.last_conflict_files = vec!["src/stale.rs".into()];
        }

        let updated = b.complete_rebase_clean(t1.id).unwrap();
        assert_eq!(updated.state, State::Review);
        assert!(!updated.rebase_requested);
        assert!(!updated.awaiting_dispatch);
        assert_eq!(updated.last_bounce_reason, None);
        assert!(updated.last_conflict_files.is_empty());
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

        let note = updated
            .notes
            .iter()
            .find(|n| n.text.contains("do-not-re-report-while-CONFLICTING"))
            .expect("binding conflict note");
        assert!(note.text.contains("src/main.rs"));
        assert!(note.text.contains("src/store.rs"));
        assert!(note.text.contains("BINDING"));
    }

    #[test]
    fn report_clears_stale_bounce_fields() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!("honr-test-report-clear-bounce-{}.json", std::process::id())),
        );
        let project = b
            .create(None, "Clear Bounce Proj", "intent", None, Origin::Human, true, None)
            .unwrap();
        let t1 = b
            .create(
                Some(project.id),
                "Task Clear",
                "intent 1",
                Some("dod 1".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();

        b.transition(t1.id, State::Shaping, "test", None).unwrap();
        b.transition(t1.id, State::Backlog, "test", None).unwrap();
        b.transition(t1.id, State::Claimed, "agent", None).unwrap();
        b.transition(t1.id, State::Running, "agent", None).unwrap();

        {
            let mut s = b.state.write();
            let it = s.items.get_mut(&t1.id).unwrap();
            it.last_bounce_reason = Some("stale bounce".into());
            it.last_conflict_files = vec!["src/old.rs".into()];
        }

        let updated = b
            .report(t1.id, "agent", 3, 1, vec!["ci-on-pr".into()])
            .unwrap();
        assert_eq!(updated.state, State::Review);
        assert_eq!(updated.last_bounce_reason, None);
        assert!(updated.last_conflict_files.is_empty());
    }

    #[test]
    fn conflict_bounce_note_names_files_and_forbids_hollow_report() {
        let note = conflict_bounce_note(&["a.rs".into(), "b.rs".into()]);
        assert!(note.contains("BINDING"));
        assert!(note.contains("a.rs"));
        assert!(note.contains("b.rs"));
        assert!(note.contains("do-not-re-report-while-CONFLICTING"));
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

    const SEED_POLICY_YAML: &str = "version: 1\n# seed-policy\nfilesystem_policy:\n  include_workdir: true\n";

    fn agents_for_seed() -> AgentConfig {
        AgentConfig {
            image: "seed-image:test".into(),
            policy: SEED_POLICY_YAML.into(),
            cpu: Some("4".into()),
            memory: Some("8Gi".into()),
            ..Default::default()
        }
    }

    #[test]
    fn sandbox_profiles_seed_from_yaml_when_catalog_empty() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-sbx-seed.json"),
        );
        assert!(b.list_sandbox_profiles().is_empty());
        assert!(b.seed_sandbox_profiles_from(&agents_for_seed()));
        let profiles = b.list_sandbox_profiles();
        assert_eq!(profiles.len(), 1);
        let p = &profiles[0];
        assert_eq!(p.id, "default");
        assert_eq!(p.image, "seed-image:test");
        assert_eq!(p.policy, SEED_POLICY_YAML);
        assert_eq!(p.cpu.as_deref(), Some("4"));
        assert_eq!(p.memory.as_deref(), Some("8Gi"));
        assert_eq!(b.default_sandbox_profile_id().as_deref(), Some("default"));
        // Second seed is a no-op.
        assert!(!b.seed_sandbox_profiles_from(&agents_for_seed()));
        assert_eq!(b.list_sandbox_profiles().len(), 1);
    }

    fn agents_with_repo() -> AgentConfig {
        AgentConfig {
            enabled: true,
            providers: vec!["vertex".into(), "gh".into()],
            repo: crate::schema::RepoConfig {
                upstream: "acme/widgets".into(),
                fork: "bot/widgets".into(),
                base: "main".into(),
            },
            vertex: crate::schema::VertexConfig {
                project: "demo".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn workspace_binding_seeds_beads_from_yaml_when_unbound() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-ws-seed-{}.json",
                std::process::id()
            )),
        );
        assert!(b.workspace_binding().is_none());
        assert!(b.seed_workspace_binding_from(&agents_with_repo()));
        let ws = b.workspace_binding().expect("seeded");
        assert!(ws.has_beads_sync());
        assert_eq!(ws.forge, "github");
        assert_eq!(ws.beads_repo().as_deref(), Some("acme/widgets"));
        // Second seed is a no-op once beads sync is set.
        assert!(!b.seed_workspace_binding_from(&agents_with_repo()));
        // Work remotes still come from yaml, not Settings.
        let mut schema = Schema::default();
        schema.execution.agents = agents_with_repo();
        let b2 = Board::new(
            schema,
            std::env::temp_dir().join(format!(
                "honr-test-ws-yaml-repo-{}.json",
                std::process::id()
            )),
        );
        let repo = b2.yaml_work_repo().expect("yaml work repo");
        assert_eq!(repo.upstream, "acme/widgets");
        assert_eq!(repo.fork, "bot/widgets");
    }

    #[test]
    fn agents_overlay_is_yaml_passthrough_without_workspace_remotes() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-ws-fail-{}.json",
                std::process::id()
            )),
        );
        assert!(b.yaml_work_repo().is_none(), "empty yaml has no work remotes");

        b.set_workspace_binding(WorkspaceBinding {
            forge: "github".into(),
            beads_sync_repo: Some("acme/beads".into()),
        })
        .expect("forge binding");
        // Settings beads does not invent work remotes.
        assert!(b.yaml_work_repo().is_none());

        let agents = AgentConfig {
            enabled: true,
            ..Default::default()
        };
        let overlaid = b.agents_with_workspace(&agents);
        assert!(overlaid.repo.upstream.is_empty());
    }

    #[test]
    fn agent_runtime_seeds_from_yaml_and_overlays_effective_agents() {
        let mut schema = Schema::default();
        schema.execution.agents = AgentConfig {
            enabled: true,
            engine: "cursor".into(),
            providers: vec!["vertex".into(), "gh".into()],
            vertex: crate::schema::VertexConfig {
                project: "yaml-proj".into(),
                location: "global".into(),
                model: "yaml-model".into(),
            },
            max_concurrent: 2,
            agent_timeout_secs: 1800,
            max_attempts: 3,
            ..Default::default()
        };
        let b = Board::new(
            schema,
            std::env::temp_dir().join(format!(
                "honr-test-agent-rt-seed-{}.json",
                std::process::id()
            )),
        );
        assert!(b.agent_runtime().is_none());
        assert!(b.seed_agent_runtime_from(&b.schema.execution.agents.clone()));
        let seeded = b.agent_runtime().expect("seeded");
        assert_eq!(seeded.vertex.project, "yaml-proj");
        assert_eq!(seeded.providers, vec!["vertex".to_string(), "gh".to_string()]);
        assert!(!b.seed_agent_runtime_if_empty(), "second seed is a no-op");

        b.set_agent_runtime(AgentRuntimeConfig {
            enabled: true,
            engine: "agy".into(),
            providers: vec!["vertex".into(), "gh-bot".into()],
            vertex: AgentRuntimeVertex {
                project: "board-proj".into(),
                location: "us-east5".into(),
                model: "board-model".into(),
            },
            max_concurrent: 1,
            per_card_budget_cents: Some(100),
            daily_budget_cents: None,
            agent_timeout_secs: 900,
            max_attempts: 2,
            ..Default::default()
        });
        let eff = b.effective_agents();
        assert_eq!(eff.engine, "agy");
        assert_eq!(eff.providers, vec!["vertex".to_string(), "gh-bot".to_string()]);
        assert_eq!(eff.vertex.location, "us-east5");
        assert_eq!(eff.vertex.project, "board-proj");
        assert_eq!(eff.max_concurrent, 1);
        assert_eq!(eff.agent_timeout_secs, 900);
        // Image / policy still from yaml (sandbox profiles own create-spec).
        assert_eq!(eff.image, b.schema.execution.agents.image);
    }

    #[test]
    fn resolve_card_repo_url_only_is_same_repo_stub() {
        let mut schema = Schema::default();
        schema.execution.agents = agents_with_repo();
        // yaml default upstream differs from PR target — PR wins.
        schema.execution.agents.repo.upstream = "acme/default".into();
        schema.execution.agents.repo.fork = "bot/default".into();
        schema.execution.agents.repo.base = "develop".into();
        let b = Board::new(
            schema,
            std::env::temp_dir().join(format!(
                "honr-test-resolve-pr-{}.json",
                std::process::id()
            )),
        );
        b.set_workspace_binding(WorkspaceBinding {
            forge: "github".into(),
            beads_sync_repo: Some("acme/beads".into()),
        })
        .unwrap();

        let p = b
            .create(None, "Other Repo Proj", "why", None, Origin::Human, true, None)
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
        b.set_pr_url(
            t.id,
            Some("https://github.com/other/widgets/pull/99".into()),
        );

        let repo = b.resolve_card_repo(t.id).expect("resolve").expect("bound");
        // URL alone → same-repo stub until base/head reported.
        assert_eq!(repo.upstream, "other/widgets");
        assert_eq!(repo.fork, "other/widgets");
        assert!(!repo.uses_cross_fork());
        assert_eq!(repo.base, "main");
    }

    #[test]
    fn resolve_card_repo_uses_pull_request_base_head() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-resolve-prbind-{}.json",
                std::process::id()
            )),
        );
        let p = b
            .create(None, "P", "why", None, Origin::Human, true, None)
            .unwrap();
        let t = b
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
        b.set_pull_request(
            t.id,
            Some(crate::model::PullRequest {
                url: "https://github.com/other/widgets/pull/3".into(),
                base: Some(crate::model::PullRequestEnd::new("other/widgets", "develop")),
                head: Some(crate::model::PullRequestEnd::new("bot/widgets", "honr/card-1")),
            }),
        );
        let repo = b.resolve_card_repo(t.id).unwrap().unwrap();
        assert_eq!(repo.upstream, "other/widgets");
        assert_eq!(repo.fork, "bot/widgets");
        assert_eq!(repo.base, "develop");
        assert!(repo.uses_cross_fork());
    }

    #[test]
    fn resolve_card_repo_first_run_is_unbound() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-resolve-unbound-{}.json",
                std::process::id()
            )),
        );
        let p = b
            .create(None, "P", "why", None, Origin::Human, true, None)
            .unwrap();
        let t = b
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
        assert!(b.resolve_card_repo(t.id).unwrap().is_none());
    }

    #[test]
    fn parse_github_pr_url_ok() {
        assert_eq!(
            parse_github_pr_url("https://github.com/Acme/Widgets/pull/42"),
            Some(("Acme/Widgets".into(), 42))
        );
        assert_eq!(
            parse_github_pr_url("https://GitHub.com/acme/widgets/pull/7/"),
            Some(("acme/widgets".into(), 7))
        );
        assert_eq!(parse_github_pr_url("not-a-url"), None);
    }

    #[test]
    fn complete_for_merged_pr_two_owners_independent_of_workspace_upstream() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-multi-pr-complete-{}.json",
                std::process::id()
            )),
        );
        b.set_workspace_binding(WorkspaceBinding {
            forge: "github".into(),
            beads_sync_repo: Some("workspace/beads".into()),
        })
        .unwrap();

        let p = b
            .create(None, "Multi", "why", None, Origin::Human, true, None)
            .unwrap();
        let make_review = |title: &str, pr: &str| {
            let t = b
                .create(
                    Some(p.id),
                    title,
                    "intent",
                    Some("dod".into()),
                    Origin::Human,
                    false,
                    None,
                )
                .unwrap();
            b.transition(t.id, State::Shaping, "test", None).unwrap();
            b.transition(t.id, State::Backlog, "test", None).unwrap();
            b.transition(t.id, State::Claimed, "agent", None).unwrap();
            b.transition(t.id, State::Running, "agent", None).unwrap();
            b.transition(t.id, State::Review, "agent", None).unwrap();
            b.set_pr_url(t.id, Some(pr.into()));
            t.id
        };
        let a = make_review("A", "https://github.com/alpha/one/pull/1");
        let c = make_review("C", "https://github.com/charlie/two/pull/2");

        assert_eq!(
            b.complete_for_merged_pr("https://github.com/alpha/one/pull/1", Some(1)),
            Some(a)
        );
        assert_eq!(b.get(a).unwrap().state, State::Done);
        assert_eq!(b.get(c).unwrap().state, State::Review);

        assert_eq!(
            b.complete_for_merged_pr("https://github.com/charlie/two/pull/2", Some(2)),
            Some(c)
        );
        assert_eq!(b.get(c).unwrap().state, State::Done);
        // Settings beads sync was never consulted for PR completion.
        assert_eq!(
            b.workspace_binding().unwrap().beads_sync_repo.as_deref(),
            Some("workspace/beads")
        );
    }

    #[test]
    fn workspace_binding_beads_url_uses_configured_repo_not_shane_default() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join(format!(
                "honr-test-ws-beads-url-{}.json",
                std::process::id()
            )),
        );
        b.seed_workspace_binding_from(&agents_with_repo());
        let issue = crate::beads::BeadsIssue {
            id: "bd-1".into(),
            title: "t".into(),
            description: None,
            status: "open".into(),
            priority: 2,
            issue_type: "task".into(),
            owner: None,
            created_at: None,
            updated_at: None,
            external_ref: Some("42".into()),
            external_id: None,
            issue_url: None,
            url: None,
            parent: None,
        };
        // Explicit repo argument — never invents shanemcd/honr.
        assert_eq!(
            issue.github_issue_url_for_repo(Some("acme/widgets")).as_deref(),
            Some("https://github.com/acme/widgets/issues/42")
        );
        assert_eq!(issue.github_issue_url_for_repo(None), None);
        // Board beads helper resolves Settings beads (seeded from yaml upstream).
        if std::env::var("GITHUB_REPOSITORY").is_err() {
            assert_eq!(b.beads_github_repository().as_deref(), Some("acme/widgets"));
        }
    }

    #[test]
    fn workspace_binding_persists_in_json_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-ws-persist-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("honr.json");
        {
            let mut schema = Schema::default();
            schema.execution.agents = agents_with_repo();
            let b = Board::new(schema, path.clone());
            assert!(b.seed_workspace_binding_if_empty());
            b.dirty.store(true, Ordering::Relaxed);
            b.flush();
        }
        let restored = Board::load_or_new(Schema::default(), path);
        let ws = restored.workspace_binding().expect("restored workspace");
        assert_eq!(ws.beads_sync_repo.as_deref(), Some("acme/widgets"));
        assert!(!restored.seed_workspace_binding_if_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_binding_legacy_json_migrates_upstream_to_beads() {
        let raw = r#"{"forge":"github","upstream":"old/work","fork":"bot/work","base":"main"}"#;
        let ws: WorkspaceBinding = serde_json::from_str(raw).expect("legacy");
        assert_eq!(ws.beads_sync_repo.as_deref(), Some("old/work"));
        assert!(serde_json::to_string(&ws).unwrap().contains("beads_sync_repo"));
        assert!(!serde_json::to_string(&ws).unwrap().contains("\"upstream\""));
    }

    #[test]
    fn sandbox_profiles_seed_reads_policy_file_contents() {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-sbx-seed-file-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.yaml");
        let yaml = "version: 1\n# from-file\n";
        std::fs::write(&path, yaml).unwrap();

        let b = Board::new(Schema::default(), dir.join("board.json"));
        let agents = AgentConfig {
            image: "from-file:1".into(),
            policy: path.to_string_lossy().into(),
            ..Default::default()
        };
        assert!(b.seed_sandbox_profiles_from(&agents));
        assert_eq!(b.list_sandbox_profiles()[0].policy, yaml);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sandbox_profiles_set_default_and_project_override() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-sbx-override.json"),
        );
        b.seed_sandbox_profiles_from(&agents_for_seed());
        let heavy = b
            .upsert_sandbox_profile(SandboxProfile {
                id: "heavy".into(),
                name: "Heavy".into(),
                image: "heavy:latest".into(),
                policy: SEED_POLICY_YAML.into(),
                cpu: Some("8".into()),
                memory: Some("16Gi".into()),
            })
            .expect("upsert heavy");
        b.set_default_sandbox_profile(&heavy.id)
            .expect("set default");
        assert_eq!(b.default_sandbox_profile_id().as_deref(), Some("heavy"));

        let project = b
            .create(None, "Sbx Proj", "why", None, Origin::Human, true, None)
            .expect("project");
        assert!(project.sandbox_profile_id.is_none());

        let updated = b
            .set_project_sandbox_profile(project.id, Some("default".into()))
            .expect("set override");
        assert_eq!(updated.sandbox_profile_id.as_deref(), Some("default"));

        let cleared = b
            .set_project_sandbox_profile(project.id, None)
            .expect("clear override");
        assert!(cleared.sandbox_profile_id.is_none());

        assert!(
            b.set_project_sandbox_profile(project.id, Some("missing".into()))
                .is_err(),
            "unknown profile must be refused"
        );
        assert!(
            b.set_default_sandbox_profile("missing").is_err(),
            "unknown default must be refused"
        );
    }

    #[test]
    fn sandbox_profiles_refuse_delete_of_default_or_in_use() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-sbx-delete.json"),
        );
        b.seed_sandbox_profiles_from(&agents_for_seed());
        b.upsert_sandbox_profile(SandboxProfile {
            id: "alt".into(),
            name: "Alt".into(),
            image: "alt:latest".into(),
            policy: SEED_POLICY_YAML.into(),
            cpu: None,
            memory: None,
        })
        .unwrap();

        let err = b.delete_sandbox_profile("default").unwrap_err();
        assert!(
            err.contains("global default"),
            "expected default refusal, got {err}"
        );

        b.set_default_sandbox_profile("alt").unwrap();
        let project = b
            .create(None, "Uses Default", "why", None, Origin::Human, true, None)
            .unwrap();
        b.set_project_sandbox_profile(project.id, Some("default".into()))
            .unwrap();

        let err = b.delete_sandbox_profile("default").unwrap_err();
        assert!(
            err.contains("in use"),
            "expected in-use refusal, got {err}"
        );

        b.set_project_sandbox_profile(project.id, None).unwrap();
        b.delete_sandbox_profile("default")
            .expect("delete after reassignment");
        assert!(b.get_sandbox_profile("default").is_none());
    }

    #[test]
    fn sandbox_profiles_round_trip_json_flush_load() {
        let path = std::env::temp_dir().join(format!(
            "honr-test-sbx-json-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        let b = Board::new(Schema::default(), path.clone());
        b.seed_sandbox_profiles_from(&agents_for_seed());
        b.upsert_sandbox_profile(SandboxProfile {
            id: "ci".into(),
            name: "CI".into(),
            image: "ci:1".into(),
            policy: SEED_POLICY_YAML.into(),
            cpu: Some("1".into()),
            memory: None,
        })
        .unwrap();
        b.set_default_sandbox_profile("ci").unwrap();
        let project = b
            .create(None, "Persist Proj", "why", None, Origin::Human, true, None)
            .unwrap();
        b.set_project_sandbox_profile(project.id, Some("default".into()))
            .unwrap();
        b.flush();

        let restored = Board::load_or_new(Schema::default(), path.clone());
        assert_eq!(
            restored.default_sandbox_profile_id().as_deref(),
            Some("ci")
        );
        assert_eq!(restored.list_sandbox_profiles().len(), 2);
        let p = restored.get(project.id).expect("project");
        assert_eq!(p.sandbox_profile_id.as_deref(), Some("default"));
        // Catalog already populated — must not re-seed over existing profiles.
        assert!(!restored.seed_sandbox_profiles_from(&agents_for_seed()));
        assert!(
            crate::model::is_inline_policy_yaml(
                &restored.get_sandbox_profile("ci").unwrap().policy
            ),
            "persisted policy must remain inline YAML"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_sandbox_create_project_override_then_default_then_yaml() {
        let yaml_policy = "version: 1\n# yaml-fallback\n";
        let def_policy = "version: 1\n# default-profile\n";
        let alt_policy = "version: 1\n# alt-profile\n";
        let mut schema = Schema::default();
        schema.execution.agents = AgentConfig {
            image: "yaml-img".into(),
            policy: yaml_policy.into(),
            cpu: Some("1".into()),
            memory: Some("1Gi".into()),
            ..Default::default()
        };
        let b = Board::new(
            schema,
            std::env::temp_dir().join(format!(
                "honr-test-sbx-resolve-store-{}.json",
                std::process::id()
            )),
        );
        // Empty catalog → YAML (inline content from agents.policy).
        let project = b
            .create(None, "P", "why", None, Origin::Human, true, None)
            .unwrap();
        let task = b
            .create(
                Some(project.id),
                "T",
                "do",
                Some("done".into()),
                Origin::Human,
                false,
                None,
            )
            .unwrap();
        let yaml = b.resolve_sandbox_create(task.id);
        assert!(yaml.profile_id.is_none());
        assert_eq!(yaml.image, "yaml-img");
        assert_eq!(yaml.policy, yaml_policy);

        b.upsert_sandbox_profile(SandboxProfile {
            id: "default".into(),
            name: "Default".into(),
            image: "def-img".into(),
            policy: def_policy.into(),
            cpu: Some("2".into()),
            memory: None,
        })
        .unwrap();
        b.set_default_sandbox_profile("default").unwrap();
        let def = b.resolve_sandbox_create(task.id);
        assert_eq!(def.profile_id.as_deref(), Some("default"));
        assert_eq!(def.image, "def-img");
        assert_eq!(def.policy, def_policy);

        b.upsert_sandbox_profile(SandboxProfile {
            id: "alt".into(),
            name: "Alt".into(),
            image: "alt-img".into(),
            policy: alt_policy.into(),
            cpu: None,
            memory: Some("8Gi".into()),
        })
        .unwrap();
        b.set_project_sandbox_profile(project.id, Some("alt".into()))
            .unwrap();
        let over = b.resolve_sandbox_create(task.id);
        assert_eq!(over.profile_id.as_deref(), Some("alt"));
        assert_eq!(over.image, "alt-img");
        assert_eq!(over.policy, alt_policy);
        assert_eq!(over.memory.as_deref(), Some("8Gi"));
    }

    #[test]
    fn sandbox_profiles_create_without_id_slugs_from_name() {
        let b = Board::new(
            Schema::default(),
            std::env::temp_dir().join("honr-test-sbx-slug.json"),
        );
        b.seed_sandbox_profiles_from(&agents_for_seed());

        let created = b
            .upsert_sandbox_profile(SandboxProfile {
                id: String::new(),
                name: "Heavy CI".into(),
                image: "img:ci".into(),
                policy: SEED_POLICY_YAML.into(),
                cpu: None,
                memory: None,
            })
            .expect("create from name");
        assert_eq!(created.id, "heavy-ci");

        // Collision with seeded `default` slug from name "Default" would be
        // "default" — creating another "Default" must suffix.
        let again = b
            .upsert_sandbox_profile(SandboxProfile {
                id: String::new(),
                name: "Default".into(),
                image: "img:2".into(),
                policy: SEED_POLICY_YAML.into(),
                cpu: None,
                memory: None,
            })
            .expect("create colliding slug");
        assert_eq!(again.id, "default-2");

        // Explicit empty-ish punctuation falls back to `profile`.
        let punct = b
            .upsert_sandbox_profile(SandboxProfile {
                id: "".into(),
                name: "!!!".into(),
                image: "img:x".into(),
                policy: SEED_POLICY_YAML.into(),
                cpu: None,
                memory: None,
            })
            .expect("create punctuation name");
        assert_eq!(punct.id, "profile");

        // Seeded default still present and untouched.
        assert!(b.get_sandbox_profile("default").is_some());
        assert_eq!(b.default_sandbox_profile_id().as_deref(), Some("default"));
    }

    #[test]
    fn migrate_sandbox_policies_path_to_inline_yaml() {
        let dir = std::env::temp_dir().join(format!(
            "honr-test-sbx-migrate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let policy_path = dir.join("legacy.yaml");
        let yaml = "version: 1\n# legacy-migrated\n";
        std::fs::write(&policy_path, yaml).unwrap();

        let b = Board::new(Schema::default(), dir.join("board.json"));
        b.upsert_sandbox_profile(SandboxProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            image: "img:1".into(),
            policy: policy_path.to_string_lossy().into(),
            cpu: None,
            memory: None,
        })
        .unwrap();
        assert_eq!(b.migrate_sandbox_policies_to_inline(), 1);
        assert_eq!(b.get_sandbox_profile("legacy").unwrap().policy, yaml);
        assert_eq!(b.migrate_sandbox_policies_to_inline(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
