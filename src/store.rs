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
use std::collections::BTreeMap;
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
    /// Vision -> ... -> this card. Sixty words that stop an agent getting a
    /// decision wrong that nobody would catch until an install failed.
    pub ancestry: Vec<AncestryLine>,
    /// Standing constraints inherited from every ancestor.
    pub constraints: Vec<String>,
    pub notes: Vec<String>,
    pub lease_expires_at: DateTime<Utc>,
    pub budget_remaining_cents: Option<u64>,
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
}

pub type SharedBoard = Arc<Board>;

impl Board {
    pub fn new(schema: Schema, path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            state: RwLock::new(BoardState { next_id: 1, ..Default::default() }),
            tx,
            seq: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            schema,
            path,
            started_at: Utc::now(),
        }
    }

    /// Load a previously persisted board, or start empty.
    pub fn load_or_new(schema: Schema, path: PathBuf) -> Self {
        let board = Self::new(schema, path.clone());
        if let Ok(raw) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<BoardState>(&raw) {
                Ok(state) => {
                    tracing::info!(items = state.items.len(), "restored board from {path:?}");
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

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn emit(&self, item: &WorkItem) {
        let _ = self.tx.send(BoardEvent::Upsert {
            seq: self.next_seq(),
            item: Box::new(item.clone()),
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

    // ------------------------------------------------------------ tree reads

    pub fn get(&self, id: ItemId) -> Option<WorkItem> {
        self.state.read().unwrap().items.get(&id).cloned()
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

    /// The goal a card belongs to. Swimlanes go by goal, never by agent — you
    /// care about "is billing v2 moving", not "what is agent-7 up to".
    fn goal_of(s: &BoardState, id: ItemId) -> ItemId {
        let chain = Self::chain(s, id);
        // Depth 1 is the Project rung; fall back to the root for shallow trees.
        chain.get(1).copied().unwrap_or_else(|| chain.first().copied().unwrap_or(id))
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
        }
        Ok(item.clone())
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
        Ok(item)
    }

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
    ) -> WorkItem {
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
            let depth = parent.map(|p| Self::depth(&s, p) + 1).unwrap_or(0);
            item.level = self.schema.level_for_depth(depth).map(|l| l.name.clone());
            s.items.insert(id, item.clone());
            item
        };
        self.emit(&item);
        item
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

    /// Unused until real dependencies are declared through the cockpit.
    #[allow(dead_code)]
    pub fn set_blocked_by(&self, id: ItemId, blockers: Vec<ItemId>) {
        let item = {
            let mut s = self.state.write().unwrap();
            let Some(it) = s.items.get_mut(&id) else { return };
            it.blocked_by = blockers;
            it.clone()
        };
        self.emit(&item);
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

    /// `list_ready` — the pull queue made visible.
    pub fn list_ready(&self, capabilities: &[String]) -> Vec<WorkItem> {
        let s = self.state.read().unwrap();
        s.items
            .values()
            .filter(|i| i.state == State::Ready)
            .filter(|i| !Self::has_children(&s, i.id))
            .filter(|i| Self::unresolved_blockers(&s, i).is_empty())
            .filter(|i| match &i.capability {
                None => true,
                Some(c) if c == "any" => true,
                Some(c) => capabilities.iter().any(|have| have == c),
            })
            .cloned()
            .collect()
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

        Ok(ClaimGrant {
            item_id: id,
            title: item.title.clone(),
            definition_of_done: item.definition_of_done.clone(),
            ancestry: self.ancestry(id),
            constraints: self.inherited_pins(id),
            notes: item.notes.iter().map(|n| n.text.clone()).collect(),
            lease_expires_at: expires_at,
            budget_remaining_cents: item.budget_cents.map(|b| b.saturating_sub(item.cost_cents)),
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

    /// `split` — self-orchestration. The parent visibly hatches: it becomes a
    /// container and its children fan into Ready.
    pub fn split(
        &self,
        id: ItemId,
        agent_id: &str,
        children: Vec<(String, String, String)>, // title, intent, dod
        max_depth: usize,
        max_children: usize,
    ) -> Result<Vec<WorkItem>, String> {
        if children.len() < 2 {
            return Err("a split needs at least two children; use report if the work is one card".into());
        }
        if children.len() > max_children {
            return Err(format!(
                "split of {} children exceeds max_children_per_split={max_children}; escalating \
                 rather than fanning out",
                children.len()
            ));
        }
        {
            let s = self.state.read().unwrap();
            if Self::depth(&s, id) + 1 > max_depth {
                return Err(format!(
                    "split would exceed max_depth={max_depth}; escalating rather than failing silently"
                ));
            }
        }

        self.transition(id, State::Splitting, agent_id, Some("agent requested split".into()))
            .map_err(|e| e.to_string())?;

        let parent = self.get(id).ok_or("no such item")?;
        let mut made = Vec::new();
        for (title, intent, dod) in children {
            let child = self.create(
                Some(id),
                title,
                intent,
                Some(dod),
                Origin::Split { from: id },
                false,
                parent.capability.clone(),
            );
            self.transition(child.id, State::Shaping, agent_id, None).map_err(|e| e.to_string())?;
            let child = self
                .transition(child.id, State::Ready, agent_id, None)
                .map_err(|e| e.to_string())?;
            made.push(child);
        }

        // The parent is now a container: it shrinks to a rollup and stops being
        // claimable.
        self.transition(id, State::Shaping, agent_id, Some("hatched into children".into()))
            .map_err(|e| e.to_string())?;

        self.story(
            id,
            format!(
                "{} turned out bigger than one card — split into {} ({}).",
                parent.title,
                made.len(),
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
        self.transition(id, State::Ready, agent_id, Some("released by agent".into()))
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
            it.clone()
        };
        self.emit(&item);
        Ok(item)
    }

    /// Pin — becomes standing context for this item *and all descendants*.
    pub fn pin(&self, id: ItemId, text: String) -> Result<WorkItem, String> {
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

    pub fn answer_escalation(&self, id: ItemId, choice: String) -> Result<WorkItem, String> {
        let title = {
            let mut s = self.state.write().unwrap();
            let it = s.items.get_mut(&id).ok_or("no such item")?;
            let esc = it.escalation.as_mut().ok_or("that item is not waiting on anyone")?;
            esc.answer = Some(choice.clone());
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

    /// Cut scope — the subtree is retired, not deleted. It stays visible and
    /// greyed, because "we chose not to" is a fact you will need later.
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
        let items: Vec<WorkItem> = s.items.values().cloned().collect();

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
        }
    }

    fn goal_view(&self, s: &BoardState, gid: ItemId, now: DateTime<Utc>) -> Option<GoalView> {
        let goal = s.items.get(&gid)?;

        // The vision is the thing goals ladder up to, not a swimlane of its
        // own. A root with no children below it, though, *is* the goal.
        if Self::depth(s, gid) == 0 && Self::has_children(s, gid) {
            return None;
        }

        let members: Vec<&WorkItem> =
            s.items.values().filter(|i| Self::goal_of(s, i.id) == gid).collect();

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
                // Same rule as the board: the vision is what goals ladder up
                // to, not a lane of its own.
                if Self::depth(&s, gid) == 0 && Self::has_children(&s, gid) {
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
