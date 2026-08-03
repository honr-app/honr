//! The other UI. It happens to be an API, but it's a designed surface with the
//! same care — the cockpit has to be able to do the right thing without the
//! human reading the board, and an agent has to without any human at all.
//!
//! Two families share one state machine:
//!   * cockpit tools — what a liaison agent needs to triage and decide
//!   * worker verbs  — `list_ready` `claim` `heartbeat` `split` `escalate`
//!     `report` `release`, and nothing else
//!
//! If the worker surface grows past roughly that size, the orchestrator has
//! started leaking its own complexity into the workers.

use crate::model::{Column, EscalationOption, ItemId, State};
use crate::store::SharedBoard;

use axum::extract::Request;
use axum::http::{header, HeaderValue, Method};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;

use rmcp::handler::server::wrapper::{Json as ToolJson, Parameters};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn bad(msg: impl Into<std::borrow::Cow<'static, str>>) -> ErrorData {
    ErrorData::invalid_params(msg, None)
}

type Out<T> = Result<ToolJson<T>, ErrorData>;

// ------------------------------------------------------------------ payloads

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdArg {
    /// Work item id, as shown on the card (`#41` is `41`).
    pub id: ItemId,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TextArg {
    pub id: ItemId,
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReasonArg {
    pub id: ItemId,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnswerArg {
    pub id: ItemId,
    /// The option label you are choosing, or free text if none of them fit.
    pub choice: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateProjectArg {
    /// Short and distinct — you cannot chunk what you cannot name.
    pub title: String,
    /// One sentence of intent. This is the contract everything below inherits.
    pub intent: String,
    /// Projects are roots. Nesting a Project under another is refused.
    #[serde(default)]
    pub parent: Option<ItemId>,
    #[serde(default = "default_above_line")]
    pub above_line: bool,
    /// Standing agent instructions for this Project (defaults on create if omitted).
    #[serde(default)]
    pub project_prompt: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateArg {
    pub id: ItemId,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub definition_of_done: Option<String>,
    /// Agent CLI engine for the next claim (`agy`, `claude`, `cursor`).
    #[serde(default)]
    pub engine: Option<String>,
    /// Standing instructions — Project cards only.
    #[serde(default)]
    pub project_prompt: Option<String>,
}

fn default_above_line() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChildSpec {
    pub title: String,
    /// One sentence. Not a restatement of the Project.
    pub intent: String,
    /// Must be mechanically checkable by a verifier.
    pub definition_of_done: String,
    #[serde(default)]
    pub capability: Option<String>,
    /// Stable key within the Plan (defaults to t1, t2, …).
    #[serde(default)]
    pub key: Option<String>,
    /// Plan keys this task is blocked by (sibling Tasks in the same Plan).
    #[serde(default)]
    pub blocked_by_keys: Vec<String>,
    /// Legacy: board item ids. Prefer `blocked_by_keys` for new Plans.
    #[serde(default)]
    pub blocked_by: Vec<ItemId>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BreakdownArg {
    /// Must be a Project.
    pub parent: ItemId,
    pub children: Vec<ChildSpec>,
    /// One-line summary of this Plan revision.
    #[serde(default)]
    pub summary: Option<String>,
    /// Plan keys to retire when this revision is approved.
    #[serde(default)]
    pub cancel_keys: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ColumnArg {
    /// One of: backlog, running, needs_you, review, done, shaping, intake, retired.
    pub column: Column,
    /// Restrict to one goal. Omit for all goals.
    #[serde(default)]
    pub goal: Option<ItemId>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListReadyArg {
    /// Capability tags this agent can serve, e.g. `["any"]` or `["any","writer"]`.
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BeadsReadyArg {
    /// Optional parent epic/bead id to restrict ready tasks to a single project/epic.
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimArg {
    pub item_id: ItemId,
    pub agent_id: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Ignored — run deadline is `agents.agent_timeout_secs` on the board.
    #[serde(default = "default_lease")]
    #[allow(dead_code)]
    pub lease_secs: i64,
}
fn default_lease() -> i64 {
    1800
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HeartbeatArg {
    pub item_id: ItemId,
    pub agent_id: String,
    /// 0.0 to 1.0.
    pub progress: f32,
    /// Spend since your last heartbeat. Budget is enforced in the control
    /// plane, not on your good behaviour.
    #[serde(default)]
    pub cost_cents: u64,
    /// Ignored — does not extend the run deadline.
    #[serde(default = "default_lease")]
    pub lease_secs: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SplitArg {
    pub item_id: ItemId,
    pub agent_id: String,
    /// Two or more. If it's really one card, use `report` instead.
    pub children: Vec<ChildSpec>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OptionSpec {
    pub label: String,
    /// What choosing this actually means, including the cost of being wrong.
    pub detail: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EscalateArg {
    pub item_id: ItemId,
    pub agent_id: String,
    pub question: String,
    /// At least two. An open-ended question hands the whole problem back.
    pub options: Vec<OptionSpec>,
    /// Index into `options` of the one you recommend.
    pub recommended: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReportArg {
    pub item_id: ItemId,
    pub agent_id: String,
    #[serde(default)]
    pub lines_added: u32,
    #[serde(default)]
    pub lines_removed: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AgentItemArg {
    pub item_id: ItemId,
    pub agent_id: String,
}

// ------------------------------------------------------------------ returns

#[derive(Debug, Serialize, JsonSchema)]
pub struct Ack {
    pub ok: bool,
    pub item: ItemId,
    pub state: String,
    pub note: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GoalLine {
    pub goal: ItemId,
    pub title: String,
    pub health: String,
    pub progress: String,
    pub spend: String,
    pub needs_you: usize,
    /// One chunked line per column — smaller than a list *and* answers the
    /// column's question.
    pub columns: Vec<String>,
    pub latest: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SnapshotOut {
    pub goals: Vec<GoalLine>,
    pub hint: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct CardLine {
    pub id: ItemId,
    pub title: String,
    pub state: String,
    pub detail: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct ListColumnOut {
    pub items: Vec<CardLine>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct BreakdownOut {
    pub items: Vec<ItemId>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct ApprovePlanOut {
    pub items: Vec<ItemId>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct CutScopeOut {
    pub items: Vec<ItemId>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct ListReadyOut {
    pub items: Vec<CardLine>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct SplitOut {
    pub items: Vec<ItemId>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct BeadsReadyOut {
    pub items: Vec<crate::beads::BeadsIssue>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq, Clone)]
pub struct HealEpicsOut {
    pub healed_count: usize,
    pub note: String,
}

// ---------------------------------------------------------------- the server

#[derive(Clone)]
pub struct Cockpit {
    board: SharedBoard,
    /// Read by the `#[tool_handler]` macro, not by us.
    #[allow(dead_code)]
    tool_router: rmcp::handler::server::tool::ToolRouter<Cockpit>,
}

#[tool_router]
impl Cockpit {
    pub fn new(board: SharedBoard) -> Self {
        Self { board, tool_router: Self::tool_router() }
    }

    fn ack(&self, id: ItemId, note: impl Into<String>) -> Out<Ack> {
        let state = self
            .board
            .get(id)
            .map(|i| format!("{:?}", i.state))
            .unwrap_or_else(|| "gone".into());
        Ok(ToolJson(Ack { ok: true, item: id, state, note: note.into() }))
    }

    // ============================================================== cockpit

    #[tool(
        name = "board_snapshot",
        description = "Start here. One chunked line per column per goal — is anything on fire, \
                       will it ship, what is left. Call this at the start of any board \
                       conversation, and again after you change something. It is deliberately \
                       not a list of cards: use list_column when you need the actual items."
    )]
    fn board_snapshot(&self) -> Out<SnapshotOut> {
        let snap = self.board.snapshot();
        let goals = snap
            .goals
            .iter()
            .map(|g| GoalLine {
                goal: g.id,
                title: g.title.clone(),
                health: if g.needs_you > 0 {
                    format!("{} blocked on you", g.needs_you)
                } else if g.agents_live > 0 {
                    format!("{} agents working", g.agents_live)
                } else {
                    "idle".into()
                },
                progress: format!(
                    "{}/{} leaves ({:.0}%)",
                    g.leaves_done,
                    g.leaves_total,
                    g.progress * 100.0
                ),
                spend: match g.budget_cents {
                    Some(b) => format!("${:.2} of ${:.2}", g.spend_cents as f64 / 100.0, b as f64 / 100.0),
                    None => format!("${:.2}", g.spend_cents as f64 / 100.0),
                },
                needs_you: g.needs_you,
                columns: g
                    .columns
                    .iter()
                    .filter(|c| c.summary.count > 0)
                    .map(|c| format!("{:?}: {}", c.column, c.summary.text))
                    .collect(),
                latest: g.story.last().map(|s| s.text.clone()),
            })
            .collect();

        Ok(ToolJson(SnapshotOut {
            goals,
            hint: "Anything in needs_you is stopping an agent and costing throughput. Review can \
                   wait until this evening."
                .into(),
        }))
    }

    #[tool(
        name = "board_digest",
        description = "What the human should read on their phone: merged count, spend, the \
                       specific questions blocking agents, and whether anything is stalled. Call \
                       this when asked 'what's the status', 'anything need me', or at the start \
                       of a session after time away."
    )]
    fn board_digest(&self) -> Out<crate::store::Digest> {
        Ok(ToolJson(self.board.digest()))
    }

    #[tool(
        name = "list_column",
        description = "The actual cards in one column, once the snapshot has told you which \
                       column matters. Call this before acting on individual items — never guess \
                       an item id."
    )]
    fn list_column(&self, Parameters(a): Parameters<ColumnArg>) -> Out<ListColumnOut> {
        let snap = self.board.snapshot();
        let now = snap.server_time;
        let mut items: Vec<&crate::model::WorkItem> = snap
            .items
            .iter()
            .filter(|i| i.state.column() == a.column)
            .filter(|i| match a.goal {
                None => true,
                Some(g) => self.board.goal_for(i.id) == g,
            })
            .collect();

        if a.column == Column::Backlog {
            items.sort_by(|a, b| {
                let a_blocked = a.blockers.iter().any(|blk| !blk.state.is_terminal())
                    || (a.blockers.is_empty() && !a.blocked_by.is_empty());
                let b_blocked = b.blockers.iter().any(|blk| !blk.state.is_terminal())
                    || (b.blockers.is_empty() && !b.blocked_by.is_empty());
                if a_blocked != b_blocked {
                    return a_blocked.cmp(&b_blocked);
                }
                a.entered_state_at.cmp(&b.entered_state_at)
            });
        }

        let rows = items
            .into_iter()
            .map(|i| CardLine {
                id: i.id,
                title: i.title.clone(),
                state: format!("{:?}", i.state),
                detail: match i.state {
                    State::NeedsHuman => i
                        .escalation
                        .as_ref()
                        .map(|e| format!("{} (blocked {})", e.question, crate::model::humanize(chrono::Duration::seconds(e.blocked_secs(now)))))
                        .unwrap_or_default(),
                    State::Running | State::Claimed => format!(
                        "{:.0}% · ${:.2} · agent {}",
                        i.progress * 100.0,
                        i.cost_cents as f64 / 100.0,
                        i.lease.as_ref().map(|l| l.agent_id.as_str()).unwrap_or("?")
                    ),
                    State::Review => format!("+{} −{} · gates passed", i.diff_added, i.diff_removed),
                    State::Backlog if !i.blocked_by.is_empty() => {
                        if !i.blockers.is_empty() {
                            let summaries: Vec<String> = i
                                .blockers
                                .iter()
                                .map(|b| format!("#{} \"{}\" ({:?})", b.id, b.title, b.state))
                                .collect();
                            format!("blocked by {}", summaries.join(", "))
                        } else {
                            format!("blocked by {:?}", i.blocked_by)
                        }
                    }
                    _ => i.intent.clone(),
                },
            })
            .collect();
        Ok(ToolJson(ListColumnOut { items: rows }))
    }

    #[tool(
        name = "item_detail",
        description = "Everything about one card: ancestry, Plan on the Project, project_prompt, \
                       cost, history and any pending question. Call this before answering an \
                       escalation or approving a review — the Plan says whether the work serves \
                       the goal."
    )]
    fn item_detail(&self, Parameters(a): Parameters<IdArg>) -> Out<serde_json::Value> {
        let item = self.board.get(a.id).ok_or_else(|| bad(format!("no work item #{}", a.id)))?;
        Ok(ToolJson(serde_json::json!({
            "item": item,
            "ancestry": self.board.ancestry(a.id),
            "children": self.board.children_of(a.id),
        })))
    }

    #[tool(
        name = "create_project",
        description = "Create a Project (top-level container). Seeds an Initial plan Task in \
                       Backlog — dispatch it; the agent writes plan.json + a plan/docs PR \
                       (Review). Approve creates sibling Tasks. Optional project_prompt overrides \
                       the default standing instructions."
    )]
    fn create_project(&self, Parameters(a): Parameters<CreateProjectArg>) -> Out<Ack> {
        if a.parent.is_some() {
            return Err(bad("Projects are roots; omit parent"));
        }
        let item = self
            .board
            .create(
                None,
                a.title,
                a.intent,
                None,
                crate::model::Origin::Human,
                a.above_line,
                None,
            )
            .map_err(bad)?;
        if let Some(prompt) = a.project_prompt {
            let _ = self.board.update_item(item.id, None, None, None, None, Some(prompt));
        }
        let _ = self.board.transition(item.id, State::Shaping, "cockpit", None);
        self.board.schedule_beads_mirror(item.id);
        for cid in self.board.children_of(item.id) {
            self.board.schedule_beads_mirror(cid);
        }
        self.ack(
            item.id,
            "Project created in shaping with Initial plan Task in Backlog — dispatch to start",
        )
    }

    #[tool(
        name = "propose_breakdown",
        description = "Write a Task proposal on the Project's Initial plan card (flat Tasks + \
                       deps by plan key). Does not create board cards — Approve on that card \
                       (or approve_plan) materializes them. Every task needs a definition of \
                       done a verifier can mechanically check. Parent may be the Project or \
                       the Initial plan Task id."
    )]
    fn propose_breakdown(&self, Parameters(a): Parameters<BreakdownArg>) -> Out<BreakdownOut> {
        use crate::model::PlanTaskSpec;

        let parent = self
            .board
            .get(a.parent)
            .ok_or_else(|| bad(format!("no work item #{}", a.parent)))?;
        let is_project = parent.is_project();
        let is_initial = parent.is_initial_plan_task();
        if !is_project && !is_initial {
            return Err(bad("breakdown parent must be a Project or Initial plan Task"));
        }
        if a.children.is_empty() {
            return Err(bad("a breakdown needs at least one task"));
        }

        // Map legacy blocked_by ItemIds → keys from an existing proposal (if any).
        let seed_id = self
            .board
            .resolve_initial_plan_id(a.parent)
            .map_err(bad)?;
        let id_to_key: std::collections::BTreeMap<ItemId, String> = self
            .board
            .get(seed_id)
            .and_then(|s| s.proposal)
            .map(|p| {
                p.tasks
                    .iter()
                    .filter_map(|t| t.item_id.map(|id| (id, t.key.clone())))
                    .collect()
            })
            .unwrap_or_default();

        let mut specs = Vec::new();
        for (idx, c) in a.children.into_iter().enumerate() {
            let key = c
                .key
                .filter(|k| !k.trim().is_empty())
                .unwrap_or_else(|| format!("t{}", idx + 1));
            let mut blocked_by_keys = c.blocked_by_keys;
            for bid in c.blocked_by {
                if let Some(k) = id_to_key.get(&bid) {
                    if !blocked_by_keys.contains(k) {
                        blocked_by_keys.push(k.clone());
                    }
                } else {
                    let k = format!("id-{bid}");
                    if !blocked_by_keys.contains(&k) {
                        blocked_by_keys.push(k);
                    }
                }
            }
            specs.push(PlanTaskSpec {
                key,
                title: c.title,
                intent: c.intent,
                definition_of_done: c.definition_of_done,
                blocked_by_keys,
                capability: c.capability,
                item_id: None,
            });
        }
        let summary = a.summary.unwrap_or_else(|| {
            format!("{} tasks proposed", specs.len())
        });
        let proposal = self
            .board
            .propose_plan(a.parent, summary, specs, a.cancel_keys)
            .map_err(bad)?;
        let linked: Vec<ItemId> = proposal.tasks.iter().filter_map(|t| t.item_id).collect();
        Ok(ToolJson(BreakdownOut { items: linked }))
    }

    #[tool(
        name = "approve_plan",
        description = "Approve the Initial plan proposal: materialize flat Tasks + deps to \
                       Backlog and finish the Initial plan card. Pass the Project id or the \
                       Initial plan Task id. Same gate as approve_review on Initial plan. \
                       Never moves the Project itself to Backlog. Does not start runs — \
                       dispatch each Task explicitly."
    )]
    fn approve_plan(&self, Parameters(a): Parameters<IdArg>) -> Out<ApprovePlanOut> {
        let published = self.board.approve_plan(a.id).map_err(bad)?;
        for cid in &published {
            self.board.schedule_beads_mirror(*cid);
        }
        Ok(ToolJson(ApprovePlanOut { items: published }))
    }

    #[tool(
        name = "answer_escalation",
        description = "Resolve a card sitting in Needs You. Do this first, before anything in \
                       Review — a blocked agent is burning throughput every minute, while \
                       finished work is safe and can wait. Read item_detail first; the answer is \
                       recorded as standing context for whoever picks the card up."
    )]
    fn answer_escalation(&self, Parameters(a): Parameters<AnswerArg>) -> Out<Ack> {
        self.board.answer_escalation(a.id, a.choice).map_err(bad)?;
        self.ack(a.id, "unblocked and requeued")
    }

    #[tool(
        name = "steer",
        description = "Inject a note into a running agent's next turn. Free — no restart, no \
                       context loss. Reach for this instead of halt whenever the agent is only \
                       slightly off course."
    )]
    fn steer(&self, Parameters(a): Parameters<TextArg>) -> Out<Ack> {
        self.board.steer(a.id, a.text).map_err(bad)?;
        self.ack(a.id, "note will reach the agent on its next turn")
    }

    #[tool(
        name = "update",
        description = "Edit fields on a card. Use `engine` to choose the sandbox CLI for the \
                       next claim (`agy`, `claude`, or `cursor`) — set before dispatch. \
                       `project_prompt` only applies to Project cards."
    )]
    fn update(&self, Parameters(a): Parameters<UpdateArg>) -> Out<Ack> {
        if a.title.is_none()
            && a.intent.is_none()
            && a.definition_of_done.is_none()
            && a.engine.is_none()
            && a.project_prompt.is_none()
        {
            return Err(bad("update needs at least one field"));
        }
        let item = self
            .board
            .update_item(
                a.id,
                a.title,
                a.intent,
                a.definition_of_done,
                a.engine,
                a.project_prompt,
            )
            .map_err(bad)?;
        let note = match item.engine.as_deref() {
            Some(e) => format!("updated (engine={e})"),
            None => "updated".into(),
        };
        self.ack(a.id, note)
    }

    #[tool(
        name = "dispatch",
        description = "Queue a Backlog card for the supervisor to claim and start a sandbox run. \
                       Nothing auto-starts from Backlog — call this (or the UI Start button) \
                       when the human wants work to begin. Requires unblocked and unparked. \
                       Does not start immediately if max_concurrent or budget is saturated; \
                       the supervisor drains the queue."
    )]
    fn dispatch(&self, Parameters(a): Parameters<IdArg>) -> Out<Ack> {
        self.board.enqueue_dispatch(a.id).map_err(bad)?;
        self.ack(a.id, "queued for dispatch")
    }

    #[tool(
        name = "park",
        description = "Stop the agent and return the card to Backlog, keep the sandbox and agy \
                       conversation, and hold the card until unpark. Prefer this when a run is \
                       wedged. Optional reason becomes a binding note on resume. Unpark queues \
                       the supervisor to resume (no separate dispatch)."
    )]
    fn park(&self, Parameters(a): Parameters<ReasonArg>) -> Out<Ack> {
        self.board.park(a.id, a.reason).map_err(bad)?;
        self.ack(a.id, "agent parked; unpark to resume")
    }

    #[tool(
        name = "unpark",
        description = "Clear a park hold and queue the card for the supervisor (same as Start). \
                       If a conversation id is still on the card, the next claim resumes that \
                       agy session."
    )]
    fn unpark(&self, Parameters(a): Parameters<IdArg>) -> Out<Ack> {
        self.board.unpark(a.id).map_err(bad)?;
        self.ack(a.id, "unparked and queued for resume")
    }

    #[tool(
        name = "halt",
        description = "Kill the agent, discard the LLM session, and return the card to Backlog. \
                       Does not auto-reclaim — dispatch again to restart. Prefer park when you \
                       want to resume the same conversation; prefer steer for a soft note that \
                       can wait until the next turn."
    )]
    fn halt(&self, Parameters(a): Parameters<ReasonArg>) -> Out<Ack> {
        self.board.halt(a.id, a.reason).map_err(bad)?;
        self.ack(a.id, "agent released, session discarded; dispatch to restart")
    }

    #[tool(
        name = "cut_scope",
        description = "Retire a card and its whole subtree. Retired, not deleted — it stays \
                       visible and greyed, because 'we chose not to' is a fact you will need \
                       later. Confirm with the human before calling this."
    )]
    fn cut_scope(&self, Parameters(a): Parameters<ReasonArg>) -> Out<CutScopeOut> {
        let ids = self.board.cut_scope(a.id, a.reason).map_err(bad)?;
        Ok(ToolJson(CutScopeOut { items: ids }))
    }

    #[tool(
        name = "approve_review",
        description = "Approve a Review card. Cards with a PR stay in Review until GitHub merge \
                       (webhook → Done); Initial plan / split proposals materialize into Tasks \
                       on that Done, not on Approve. Sort Review by blast radius and novelty."
    )]
    fn approve_review(&self, Parameters(a): Parameters<IdArg>) -> Out<Ack> {
        let before: std::collections::HashSet<_> = self
            .board
            .get(a.id)
            .and_then(|i| i.parent)
            .map(|p| self.board.children_of(p))
            .unwrap_or_default()
            .into_iter()
            .collect();
        let item = self.board.approve_review(a.id).map_err(bad)?;
        if let Some(parent) = item.parent {
            for cid in self.board.children_of(parent) {
                if !before.contains(&cid) {
                    self.board.schedule_beads_mirror(cid);
                }
            }
        }
        let unblocked = self.board.newly_unblocked_siblings(a.id);
        let note = if unblocked.len() == 1 {
            format!("approved — dispatch #{} next", unblocked[0].id)
        } else if unblocked.len() > 1 {
            let ids: Vec<_> = unblocked.iter().map(|u| format!("#{}", u.id)).collect();
            format!("approved — unblocked: {}", ids.join(", "))
        } else {
            "approved".to_string()
        };
        self.ack(a.id, &note)
    }

    #[tool(
        name = "request_changes",
        description = "Send a reviewed card back to Backlog with a note. The note is attached to \
                       the card, so the next run (after dispatch) sees why. Does not auto-start."
    )]
    fn request_changes(&self, Parameters(a): Parameters<TextArg>) -> Out<Ack> {
        self.board.request_changes(a.id, a.text).map_err(bad)?;
        self.ack(a.id, "returned to Backlog with your note — dispatch to restart")
    }

    // =============================================================== worker

    #[tool(
        name = "beads_ready",
        description = "Query beads for task-only ready work (epics excluded). Use this when \
                       asking beads 'what's next' for claimable tasks. Optional parent filters \
                       ready tasks to a specific epic."
    )]
    async fn beads_ready(&self, Parameters(a): Parameters<BeadsReadyArg>) -> Out<BeadsReadyOut> {
        let items = self
            .board
            .list_ready_beads(a.parent.as_deref())
            .await
            .map_err(bad)?;
        Ok(ToolJson(BeadsReadyOut { items }))
    }

    #[tool(
        name = "heal_epics",
        description = "One-shot heal for completed epics. Scans open beads epics and board projects, closing any whose children are all completed or superseded."
    )]
    async fn heal_epics(&self) -> Out<HealEpicsOut> {
        let healed_count = self.board.heal_completed_epics().await;
        Ok(ToolJson(HealEpicsOut {
            healed_count,
            note: format!("healed {healed_count} completed epic(s)"),
        }))
    }

    #[tool(
        name = "list_ready",
        description = "WORKER VERB / cockpit alias. Lists Backlog leaves filtered by capabilities. \
                       Not a start queue — cockpit must dispatch before the supervisor claims."
    )]
    fn list_ready(&self, Parameters(a): Parameters<ListReadyArg>) -> Out<ListReadyOut> {
        let rows = self
            .board
            .list_ready(&a.capabilities)
            .into_iter()
            .map(|i| CardLine {
                id: i.id,
                title: i.title.clone(),
                state: "Backlog".into(),
                detail: i.intent.clone(),
            })
            .collect();
        Ok(ToolJson(ListReadyOut { items: rows }))
    }

    #[tool(
        name = "claim",
        description = "WORKER VERB. Take a Backlog card (supervisor path after dispatch). \
                       Returns the full intent chain — read it before you start. The run \
                       ends at agent_timeout_secs; heartbeats do not extend that deadline."
    )]
    fn claim(&self, Parameters(a): Parameters<ClaimArg>) -> Out<crate::store::ClaimGrant> {
        let timeout = self.board.schema.execution.agents.agent_timeout_secs as i64;
        let grant = self
            .board
            .claim(a.item_id, &a.agent_id, a.model, timeout)
            .map_err(|e| bad(e.to_string()))?;
        Ok(ToolJson(grant))
    }

    #[tool(
        name = "heartbeat",
        description = "WORKER VERB. Report spend (and optional progress). Does not extend \
                       the run deadline — that was fixed at claim."
    )]
    fn heartbeat(&self, Parameters(a): Parameters<HeartbeatArg>) -> Out<Ack> {
        self.board
            .heartbeat(a.item_id, &a.agent_id, a.progress, a.cost_cents, a.lease_secs)
            .map_err(|e| bad(e.to_string()))?;
        self.ack(a.item_id, "cost recorded")
    }

    #[tool(
        name = "split",
        description = "WORKER VERB. The work is bigger than this card: propose sibling Tasks \
                       (Review). Human Approve creates them under the Project — nothing is \
                       created until then. Needs two or more children; if it is really one card, \
                       just report. Mutually exclusive with opening a PR."
    )]
    fn split(&self, Parameters(a): Parameters<SplitArg>) -> Out<SplitOut> {
        let children = a
            .children
            .into_iter()
            .map(|c| {
                let mut spec =
                    crate::model::SplitChildSpec::new(c.title, c.intent, c.definition_of_done);
                spec.key = c.key;
                spec.blocked_by_keys = c.blocked_by_keys;
                spec
            })
            .collect();
        let card = self
            .board
            .propose_split(a.item_id, &a.agent_id, children, 5)
            .map_err(bad)?;
        Ok(ToolJson(SplitOut {
            // Proposal card id — siblings do not exist until Approve.
            items: vec![card.id],
        }))
    }

    #[tool(
        name = "escalate",
        description = "WORKER VERB. You have hit a real decision. You must supply at least two \
                       concrete options and a recommendation — an open-ended 'what should I do?' \
                       transfers the whole problem back to the human, and turns a one-tap \
                       decision into a five-minute think. Escalate only when the contract \
                       genuinely does not settle the question; a high escalation rate is a \
                       quality signal, not just a workflow event."
    )]
    fn escalate(&self, Parameters(a): Parameters<EscalateArg>) -> Out<Ack> {
        let options = a
            .options
            .into_iter()
            .map(|o| EscalationOption { label: o.label, detail: o.detail })
            .collect();
        self.board
            .escalate(a.item_id, &a.agent_id, a.question, options, a.recommended)
            .map_err(bad)?;
        self.ack(a.item_id, "escalated; a human has been asked")
    }

    #[tool(
        name = "report",
        description = "WORKER VERB. You believe the definition of done is met. Hands the card to \
                       Review — CI on the PR is the mechanical gate."
    )]
    fn report(&self, Parameters(a): Parameters<ReportArg>) -> Out<Ack> {
        self.board
            .report(
                a.item_id,
                &a.agent_id,
                a.lines_added,
                a.lines_removed,
                vec!["lint".into(), "types".into(), "tests".into()],
            )
            .map_err(|e| bad(e.to_string()))?;
        self.ack(a.item_id, "handed to the verifier")
    }

    #[tool(
        name = "release",
        description = "WORKER VERB. Graceful surrender — give the card back to Backlog without \
                       waiting for your lease to expire. Cockpit must dispatch again to restart."
    )]
    fn release(&self, Parameters(a): Parameters<AgentItemArg>) -> Out<Ack> {
        self.board
            .release(a.item_id, &a.agent_id)
            .map_err(|e| bad(e.to_string()))?;
        self.ack(a.item_id, "released to Backlog")
    }
}

#[tool_handler]
impl ServerHandler for Cockpit {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info({
            let mut me = rmcp::model::Implementation::default();
            me.name = "honr".into();
            me.title = Some("honr — agent orchestrator".into());
            me.version = env!("CARGO_PKG_VERSION").into();
            me
        })
        .with_instructions(
                "honr — an agent orchestration board. You are the cockpit: the human's liaison, \
                 not a dashboard reader.\n\n\
                 When querying beads for available work or 'what's next', use beads_ready \
                 (or bd ready --exclude-type=epic with optional --parent=<epic>). Epics are \
                 containers, not claimable work.\n\n\
                 Start with board_snapshot. Triage in this order, because urgency differs:\n\
                 1. Needs You — an agent is stopped and burning nothing while it waits. Every \
                    minute costs throughput. Resolve these first.\n\
                 2. Review — finished and safe. It can wait until this evening. Sort by blast \
                    radius and novelty, not arrival time.\n\
                 3. Everything else waits for a digest.\n\n\
                 Interrupt the human for four things only: irreversible actions, budget breach, \
                 an ambiguity blocking several items, and repeated failure on the same card. \
                 Otherwise summarise and let them walk away.\n\n\
                 Backlog cards do not auto-start. Use dispatch (or the UI Start button) when the \
                 human wants a run. Park/halt/lease expiry/request_changes all return to Backlog \
                 without reclaim — dispatch again. Prefer park over halt when a run is wedged — \
                 park keeps the sandbox and agy session; unpark queues resume. Prefer \
                 steer for a soft note that can wait. Standing policy belongs in the Project \
                 project_prompt (edit via update); task inputs are the Plan. Initial plan and \
                 impl splits write a proposal on the card → Review; Approve creates sibling \
                 Tasks. Read item_detail's proposal/Plan before approving; a card that passes \
                 its gates can still be building the wrong thing, because coherence is not a \
                 property of any single card.",
        )
    }
}

/// Mounted on the same axum router, same port, same state as the human face.
pub fn service(board: SharedBoard) -> StreamableHttpService<Cockpit, LocalSessionManager> {
    // rmcp defaults to localhost/127.0.0.1/::1 only (DNS-rebinding guard).
    // Docker clients reach us as host.docker.internal — allow that Host.
    let mcp_http = StreamableHttpServerConfig::default().with_allowed_hosts([
        "localhost",
        "127.0.0.1",
        "::1",
        "host.docker.internal",
        "host.docker.internal:8080",
    ]);
    StreamableHttpService::new(
        move || Ok(Cockpit::new(board.clone())),
        Arc::new(LocalSessionManager::default()),
        mcp_http,
    )
}

async fn normalize_mcp_request(req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let method = parts.method.clone();
    let query_string = parts.uri.query().map(|q| q.to_owned());

    // Copy session id from query parameters if missing in headers.
    if let Some(query) = query_string {
        for pair in query.split('&') {
            let mut sub = pair.splitn(2, '=');
            if let (Some(k), Some(v)) = (sub.next(), sub.next()) {
                if (k.eq_ignore_ascii_case("sessionid")
                    || k.eq_ignore_ascii_case("mcp-session-id")
                    || k.eq_ignore_ascii_case("session_id"))
                    && !parts.headers.contains_key("mcp-session-id")
                {
                    if let Ok(hv) = HeaderValue::from_str(v) {
                        parts.headers.insert("mcp-session-id", hv);
                    }
                }
            }
        }
    }

    let mut body_bytes = None;

    if method == Method::POST {
        // `rmcp` strictly validates that Accept contains BOTH `application/json` AND `text/event-stream`.
        // Standard MCP clients (Cursor, VS Code, Claude, etc.) send `Accept: application/json` or `Accept: */*`.
        let needs_fix = match parts.headers.get(header::ACCEPT) {
            Some(val) => {
                if let Ok(s) = val.to_str() {
                    !(s.contains("application/json") && s.contains("text/event-stream"))
                } else {
                    true
                }
            }
            None => true,
        };
        if needs_fix {
            parts.headers.insert(
                header::ACCEPT,
                HeaderValue::from_static("application/json, text/event-stream"),
            );
        }

        // Buffer body to check if this is an `initialize` request or unsupported custom method.
        if let Ok(bytes) = axum::body::to_bytes(body, 4 * 1024 * 1024).await {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let method_str = json.get("method").and_then(|m| m.as_str());
                if method_str == Some("initialize") {
                    parts.headers.remove("mcp-session-id");
                    parts.headers.remove("x-mcp-session-id");
                } else if method_str == Some("subscriptions/listen") || method_str == Some("subscriptions/subscribe") {
                    let id = json.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let resp_json = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {}
                    });
                    let mut response = (
                        [
                            (header::CONTENT_TYPE, "application/json"),
                            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                            (header::ACCESS_CONTROL_EXPOSE_HEADERS, "*"),
                        ],
                        serde_json::to_string(&resp_json).unwrap_or_default(),
                    )
                        .into_response();
                    if let Some(sess_id) = parts.headers.get("mcp-session-id") {
                        response.headers_mut().insert("mcp-session-id", sess_id.clone());
                    }
                    return response;
                }
            }
            body_bytes = Some(bytes);
        }
    } else if method == Method::GET {
        // If GET request lacks mcp-session-id header, handle standard SSE endpoint discovery.
        if !parts.headers.contains_key("mcp-session-id") {
            let is_sse = parts
                .headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("text/event-stream"))
                .unwrap_or(false);

            if is_sse {
                return (
                    [
                        (header::CONTENT_TYPE, "text/event-stream"),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::CONNECTION, "keep-alive"),
                        (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                        (header::ACCESS_CONTROL_EXPOSE_HEADERS, "*"),
                    ],
                    "event: endpoint\ndata: /mcp\n\n",
                )
                    .into_response();
            }
        } else if !parts.headers.contains_key(header::ACCEPT) {
            parts.headers.insert(header::ACCEPT, HeaderValue::from_static("text/event-stream"));
        }
    }

    let req_body = body_bytes
        .map(axum::body::Body::from)
        .unwrap_or_else(axum::body::Body::empty);
    let req = Request::from_parts(parts, req_body);

    let mut response = next.run(req).await;
    let res_headers = response.headers_mut();
    res_headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res_headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("mcp-session-id, content-type, authorization"),
    );
    response
}

pub fn router<S>(board: SharedBoard) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .fallback_service(service(board))
        .layer(middleware::from_fn(normalize_mcp_request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, Origin};
    use crate::schema::Schema;
    use crate::store::Board;

    fn test_board() -> (SharedBoard, ItemId) {
        let path = std::env::temp_dir().join(format!(
            "honr-mcp-test-{}.json",
            std::process::id()
        ));
        let b = Arc::new(Board::new(Schema::default(), path));
        let goal = b
            .create(None, "Test Goal", "Test Intent", None, Origin::Human, true, None)
            .expect("project");
        let _ = b.transition(goal.id, State::Shaping, "test", None);
        (b, goal.id)
    }

    #[test]
    fn list_column_returns_record_with_items_for_triage_columns() {
        let (board, goal_id) = test_board();
        let cockpit = Cockpit::new(board.clone());

        // Ready card
        let card_ready = board
            .create(
                Some(goal_id),
                "Ready Card",
                "Ready Intent",
                Some("DoD".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("ready card");
        let _ = board.transition(card_ready.id, State::Shaping, "test", None);
        let _ = board.transition(card_ready.id, State::Backlog, "test", None);

        // NeedsYou card (escalated)
        let card_needs = board
            .create(
                Some(goal_id),
                "NeedsYou Card",
                "NeedsYou Intent",
                Some("DoD".into()),
                Origin::Human,
                false,
                None,
            )
            .expect("needs card");
        let _ = board.transition(card_needs.id, State::Shaping, "test", None);
        let _ = board.transition(card_needs.id, State::Backlog, "test", None);
        let _ = board.claim(card_needs.id, "agent-1", None, 60);
        let options = vec![
            crate::model::EscalationOption { label: "Opt A".into(), detail: "Detail A".into() },
            crate::model::EscalationOption { label: "Opt B".into(), detail: "Detail B".into() },
        ];
        let _ = board.escalate(card_needs.id, "agent-1", "Which option?".into(), options, 0);

        // Shaping card
        let card_shaping = board
            .create(
                Some(goal_id),
                "Shaping Card",
                "Shaping Intent",
                None,
                Origin::Human,
                false,
                None,
            )
            .expect("shaping card");
        let _ = board.transition(card_shaping.id, State::Shaping, "test", None);

        // Verify list_column for needs_you, ready, shaping returns a record object with "items"
        for col in [Column::NeedsYou, Column::Backlog, Column::Shaping] {
            let res = cockpit
                .list_column(Parameters(ColumnArg { column: col, goal: None }))
                .expect("list_column should succeed");
            let value = serde_json::to_value(&res.0).expect("serialize to value");

            assert!(value.is_object(), "structuredContent must be a JSON record/object, got: {:?}", value);
            let obj = value.as_object().unwrap();
            assert!(obj.contains_key("items"), "record must contain 'items' key");
            assert!(obj["items"].is_array(), "'items' value must be a JSON array");
        }
    }

    #[test]
    fn propose_breakdown_and_approve_plan_return_record_with_items() {
        let (board, goal_id) = test_board();
        let cockpit = Cockpit::new(board.clone());

        let breakdown_arg = BreakdownArg {
            parent: goal_id,
            children: vec![ChildSpec {
                title: "Subtask 1".into(),
                intent: "Intent 1".into(),
                definition_of_done: "DoD 1".into(),
                capability: None,
                key: Some("t1".into()),
                blocked_by_keys: vec![],
                blocked_by: vec![],
            }],
            summary: Some("one task".into()),
            cancel_keys: vec![],
        };

        let bd_res = cockpit
            .propose_breakdown(Parameters(breakdown_arg))
            .expect("propose_breakdown should succeed");
        let bd_val = serde_json::to_value(&bd_res.0).expect("serialize to value");
        assert!(bd_val.is_object(), "propose_breakdown response must be a JSON record object");
        assert!(bd_val.as_object().unwrap().contains_key("items"));

        let approve_res = cockpit
            .approve_plan(Parameters(IdArg { id: goal_id }))
            .expect("approve_plan should succeed");
        let app_val = serde_json::to_value(&approve_res.0).expect("serialize to value");
        assert!(app_val.is_object(), "approve_plan response must be a JSON record object");
        assert!(app_val.as_object().unwrap().contains_key("items"));
    }

    #[test]
    fn cut_scope_list_ready_and_split_return_record_with_items() {
        let (board, goal_id) = test_board();
        let cockpit = Cockpit::new(board.clone());

        // list_ready
        let ready_res = cockpit
            .list_ready(Parameters(ListReadyArg { capabilities: vec!["any".into()] }))
            .expect("list_ready should succeed");
        let ready_val = serde_json::to_value(&ready_res.0).expect("serialize to value");
        assert!(ready_val.is_object(), "list_ready response must be a JSON record object");
        assert!(ready_val.as_object().unwrap().contains_key("items"));

        // cut_scope
        let cut_res = cockpit
            .cut_scope(Parameters(ReasonArg { id: goal_id, reason: Some("retired".into()) }))
            .expect("cut_scope should succeed");
        let cut_val = serde_json::to_value(&cut_res.0).expect("serialize to value");
        assert!(cut_val.is_object(), "cut_scope response must be a JSON record object");
        assert!(cut_val.as_object().unwrap().contains_key("items"));
    }

    #[test]
    fn list_column_sorts_unblocked_ready_first() {
        let (board, goal_id) = test_board();
        let cockpit = Cockpit::new(board.clone());

        // Card 1: unblocked
        let c1 = board.create(Some(goal_id), "Card 1", "Unblocked", Some("DoD".into()), Origin::Human, false, None).expect("c1");
        let _ = board.transition(c1.id, State::Shaping, "test", None);
        let _ = board.transition(c1.id, State::Backlog, "test", None);

        // Card 2: blocked by Card 1
        let c2 = board.create(Some(goal_id), "Card 2", "Blocked", Some("DoD".into()), Origin::Human, false, None).expect("c2");
        let _ = board.transition(c2.id, State::Shaping, "test", None);
        let _ = board.transition(c2.id, State::Backlog, "test", None);
        board.set_blocked_by(c2.id, vec![c1.id]);

        // Bounce Card 1 through claim and release so its entered_state_at is NEWER than Card 2
        let _ = board.claim(c1.id, "agent-1", None, 60).expect("claim");
        let _ = board.release(c1.id, "agent-1").expect("release");

        let res = cockpit
            .list_column(Parameters(ColumnArg { column: Column::Backlog, goal: Some(goal_id) }))
            .expect("list_column should succeed");

        let pos_c1 = res.0.items.iter().position(|i| i.id == c1.id).expect("c1 present");
        let pos_c2 = res.0.items.iter().position(|i| i.id == c2.id).expect("c2 present");
        assert!(pos_c1 < pos_c2, "Unblocked card #1 must sort before blocked card #2");
    }

    #[tokio::test]
    async fn normalize_mcp_request_fixes_accept_header_and_handles_sse_discovery() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower_service::Service;

        let (board, _) = test_board();
        let mut app = router::<()>(board);

        // POST request with Accept: application/json should NOT return 406
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}},"id":1}"#,
            ))
            .unwrap();

        let response = app.call(req).await.unwrap();
        assert_ne!(response.status(), StatusCode::NOT_ACCEPTABLE);

        // GET request without session id should return SSE endpoint discovery
        let req = Request::builder()
            .method("GET")
            .uri("/mcp")
            .header("accept", "text/event-stream")
            .body(Body::empty())
            .unwrap();

        let response = app.call(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("text/event-stream"));

        // POST request with subscriptions/listen method should return JSON-RPC 200 result
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","method":"subscriptions/listen","id":99}"#,
            ))
            .unwrap();

        let response = app.call(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn approve_review_mcp_returns_dispatch_next_note() {
        let (board, goal_id) = test_board();
        let cockpit = Cockpit::new(board.clone());

        let t1 = board
            .create(Some(goal_id), "Task 1", "intent 1", Some("dod 1".into()), Origin::Human, false, None)
            .unwrap();
        let t2 = board
            .create(Some(goal_id), "Task 2", "intent 2", Some("dod 2".into()), Origin::Human, false, None)
            .unwrap();
        board.set_blocked_by(t2.id, vec![t1.id]);

        let _ = board.transition(t1.id, State::Shaping, "test", None);
        let _ = board.transition(t1.id, State::Backlog, "test", None);
        let _ = board.transition(t1.id, State::Claimed, "agent", None);
        let _ = board.transition(t1.id, State::Running, "agent", None);
        let _ = board.transition(t1.id, State::Review, "agent", None);

        let ack = cockpit.approve_review(Parameters(IdArg { id: t1.id })).expect("approve_review");
        assert_eq!(ack.0.note, format!("approved — dispatch #{} next", t2.id));
    }

    #[tokio::test]
    async fn test_mcp_beads_ready_excludes_epics() {
        let test_dir = std::env::temp_dir().join(format!(
            "honr-mcp-beads-ready-{}.json",
            std::process::id()
        ));
        let beads_dir = test_dir.join(".beads");
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&beads_dir).unwrap();

        let mut board = Board::new(Schema::default(), test_dir.join("board.json"));
        let beads_client = crate::beads::BeadsClient::new(&beads_dir);
        beads_client.init_stealth().await.expect("init stealth");
        board.beads = Some(beads_client.clone());
        let board = Arc::new(board);

        let epic = beads_client
            .create_linked("Project Epic", 0, "epic", Some("Epic desc"), None, &[], None)
            .await
            .expect("create epic");
        let task = beads_client
            .create_linked("Task Item", 1, "task", Some("Task desc"), Some(&epic.id), &[], None)
            .await
            .expect("create task");

        let cockpit = Cockpit::new(board);
        let res = cockpit
            .beads_ready(Parameters(BeadsReadyArg { parent: None }))
            .await
            .expect("beads_ready should succeed");

        assert!(res.0.items.iter().any(|i| i.id == task.id), "ready should include task");
        assert!(!res.0.items.iter().any(|i| i.issue_type == "epic"), "ready MUST NOT include epics");
    }
}
