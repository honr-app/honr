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
pub struct CreateGoalArg {
    /// Short and distinct — you cannot chunk what you cannot name.
    pub title: String,
    /// One sentence of intent. This is the contract everything below inherits.
    pub intent: String,
    /// Projects are roots. Nesting a Project under another is refused.
    #[serde(default)]
    pub parent: Option<ItemId>,
    #[serde(default = "default_above_line")]
    pub above_line: bool,
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
    /// One of: ready, running, needs_you, verify, review, done, shaping, intake, retired.
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
pub struct ClaimArg {
    pub item_id: ItemId,
    pub agent_id: String,
    #[serde(default)]
    pub model: Option<String>,
    /// How long you promise to keep heartbeating. Expiry requeues the card.
    #[serde(default = "default_lease")]
    pub lease_secs: i64,
}
fn default_lease() -> i64 {
    45
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

        if a.column == Column::Ready {
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
                    State::Ready if !i.blocked_by.is_empty() => {
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
        description = "Everything about one card: the intent chain from the vision down, \
                       inherited constraints, cost, history and any pending question. Call this \
                       before answering an escalation or approving a review — the chain is what \
                       tells you whether the work actually serves the goal."
    )]
    fn item_detail(&self, Parameters(a): Parameters<IdArg>) -> Out<serde_json::Value> {
        let item = self.board.get(a.id).ok_or_else(|| bad(format!("no work item #{}", a.id)))?;
        Ok(ToolJson(serde_json::json!({
            "item": item,
            "ancestry": self.board.ancestry(a.id),
            "constraints": self.board.inherited_pins(a.id),
            "children": self.board.children_of(a.id),
        })))
    }

    #[tool(
        name = "create_goal",
        description = "Create a Project (top-level container). Seeds an Initial plan Task in \
                       Ready. An agent may open one plan/docs PR then split into sibling Tasks; \
                       cockpit may also propose_breakdown + Approve Plan."
    )]
    fn create_goal(&self, Parameters(a): Parameters<CreateGoalArg>) -> Out<Ack> {
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
        let _ = self.board.transition(item.id, State::Shaping, "cockpit", None);
        self.board.schedule_beads_mirror(item.id);
        for cid in self.board.children_of(item.id) {
            self.board.schedule_beads_mirror(cid);
        }
        self.ack(
            item.id,
            "Project created in shaping with Initial plan Task Ready (agent may plan-PR + split)",
        )
    }

    #[tool(
        name = "propose_breakdown",
        description = "Write a Plan artifact on a Project (flat Tasks + deps by plan key). Does \
                       not create board cards — Approve Plan materializes them. Every task needs \
                       a definition of done a verifier can mechanically check."
    )]
    fn propose_breakdown(&self, Parameters(a): Parameters<BreakdownArg>) -> Out<BreakdownOut> {
        use crate::model::PlanTaskSpec;

        let parent = self
            .board
            .get(a.parent)
            .ok_or_else(|| bad(format!("no work item #{}", a.parent)))?;
        if parent.parent.is_some() || parent.level.as_deref() == Some("Task") {
            return Err(bad("breakdown parent must be a Project"));
        }
        if a.children.is_empty() {
            return Err(bad("a breakdown needs at least one task"));
        }

        // Map legacy blocked_by ItemIds → keys of already-materialized plan tasks.
        let id_to_key: std::collections::BTreeMap<ItemId, String> = parent
            .plan
            .as_ref()
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
                    // Sibling not yet in plan — use synthetic key from board id.
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
        let plan = self
            .board
            .propose_plan(a.parent, summary, specs, a.cancel_keys)
            .map_err(bad)?;
        let linked: Vec<ItemId> = plan.tasks.iter().filter_map(|t| t.item_id).collect();
        Ok(ToolJson(BreakdownOut { items: linked }))
    }

    #[tool(
        name = "approve_plan",
        description = "Approve a Project's Plan artifact: materialize flat Tasks + deps and \
                       publish them to Ready. Never moves the Project itself to Ready. Only \
                       call this once the human has actually seen and approved the shape."
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
        name = "pin_constraint",
        description = "Make a correction permanent: the text becomes standing context for this \
                       item and every descendant, forever. Use this whenever you catch a mistake \
                       that could recur — a correction said once should not need saying twice. \
                       Pin high in the tree for constraints that are really project-wide."
    )]
    fn pin_constraint(&self, Parameters(a): Parameters<TextArg>) -> Out<Ack> {
        self.board.pin(a.id, a.text).map_err(bad)?;
        self.ack(a.id, "pinned; inherited by all descendants")
    }

    #[tool(
        name = "halt",
        description = "Kill the agent and return the card to Ready. This loses in-flight work — \
                       prefer steer unless the approach itself is wrong."
    )]
    fn halt(&self, Parameters(a): Parameters<ReasonArg>) -> Out<Ack> {
        self.board.halt(a.id, a.reason).map_err(bad)?;
        self.ack(a.id, "agent released, card requeued")
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
        description = "Merge a card that has passed its gates. Sort your way through Review by \
                       blast radius and novelty, not arrival order — a 400-line change to \
                       payment calculation and a README typo are not the same review."
    )]
    fn approve_review(&self, Parameters(a): Parameters<IdArg>) -> Out<Ack> {
        self.board.approve_review(a.id).map_err(bad)?;
        self.ack(a.id, "merged")
    }

    #[tool(
        name = "request_changes",
        description = "Send a reviewed card back to Ready with a note. The note is attached to \
                       the card, so whoever picks it up next sees why."
    )]
    fn request_changes(&self, Parameters(a): Parameters<TextArg>) -> Out<Ack> {
        self.board.request_changes(a.id, a.text).map_err(bad)?;
        self.ack(a.id, "returned to the queue with your note")
    }

    // =============================================================== worker

    #[tool(
        name = "list_ready",
        description = "WORKER VERB. The claimable pool, filtered to your capabilities. Poll this \
                       when you are not holding a card."
    )]
    fn list_ready(&self, Parameters(a): Parameters<ListReadyArg>) -> Out<ListReadyOut> {
        let rows = self
            .board
            .list_ready(&a.capabilities)
            .into_iter()
            .map(|i| CardLine {
                id: i.id,
                title: i.title.clone(),
                state: "Ready".into(),
                detail: i.intent.clone(),
            })
            .collect();
        Ok(ToolJson(ListReadyOut { items: rows }))
    }

    #[tool(
        name = "claim",
        description = "WORKER VERB. Take a lease on a ready card. Returns the full intent chain \
                       from the vision down plus every inherited constraint — read it before you \
                       start, because it is what stops you making a decision nobody would catch \
                       until a customer install failed. Keep heartbeating or the lease expires \
                       and the card is requeued."
    )]
    fn claim(&self, Parameters(a): Parameters<ClaimArg>) -> Out<crate::store::ClaimGrant> {
        let grant = self
            .board
            .claim(a.item_id, &a.agent_id, a.model, a.lease_secs)
            .map_err(|e| bad(e.to_string()))?;
        Ok(ToolJson(grant))
    }

    #[tool(
        name = "heartbeat",
        description = "WORKER VERB. Prove you are alive, report progress, and declare spend \
                       since the last beat. Call this on a regular interval while working."
    )]
    fn heartbeat(&self, Parameters(a): Parameters<HeartbeatArg>) -> Out<Ack> {
        self.board
            .heartbeat(a.item_id, &a.agent_id, a.progress, a.cost_cents, a.lease_secs)
            .map_err(|e| bad(e.to_string()))?;
        self.ack(a.item_id, "lease renewed")
    }

    #[tool(
        name = "split",
        description = "WORKER VERB. The work is bigger than this card: create children rather \
                       than heroically overrunning. The parent becomes a container and your \
                       children fan into Ready. Needs two or more children; if it is really one \
                       card, just report."
    )]
    fn split(&self, Parameters(a): Parameters<SplitArg>) -> Out<SplitOut> {
        let children = a
            .children
            .into_iter()
            .map(|c| (c.title, c.intent, c.definition_of_done))
            .collect();
        let made = self.board.split(a.item_id, &a.agent_id, children, 5).map_err(bad)?;
        for m in &made {
            self.board.schedule_beads_mirror(m.id);
        }
        Ok(ToolJson(SplitOut {
            items: made.into_iter().map(|i| i.id).collect(),
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
                       the verifier — gates decide, not you. Failing gates return it to Ready."
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
        description = "WORKER VERB. Graceful surrender — give the card back to Ready without \
                       waiting for your lease to expire."
    )]
    fn release(&self, Parameters(a): Parameters<AgentItemArg>) -> Out<Ack> {
        self.board
            .release(a.item_id, &a.agent_id)
            .map_err(|e| bad(e.to_string()))?;
        self.ack(a.item_id, "released back to the queue")
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
                 Start with board_snapshot. Triage in this order, because urgency differs:\n\
                 1. Needs You — an agent is stopped and burning nothing while it waits. Every \
                    minute costs throughput. Resolve these first.\n\
                 2. Review — finished and safe. It can wait until this evening. Sort by blast \
                    radius and novelty, not arrival time.\n\
                 3. Everything else waits for a digest.\n\n\
                 Interrupt the human for four things only: irreversible actions, budget breach, \
                 an ambiguity blocking several items, and repeated failure on the same card. \
                 Otherwise summarise and let them walk away.\n\n\
                 Prefer steer over halt — halt loses in-flight work. When you correct something \
                 that could recur, pin_constraint so it binds every descendant instead of being \
                 a one-off. Read item_detail's intent chain before approving or answering; a \
                 card that passes its gates can still be building the wrong thing, because \
                 coherence is not a property of any single card.",
        )
    }
}

/// Mounted on the same axum router, same port, same state as the human face.
pub fn service(board: SharedBoard) -> StreamableHttpService<Cockpit, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(Cockpit::new(board.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    )
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
        let _ = board.transition(card_ready.id, State::Ready, "test", None);

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
        let _ = board.transition(card_needs.id, State::Ready, "test", None);
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
        for col in [Column::NeedsYou, Column::Ready, Column::Shaping] {
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
        let _ = board.transition(c1.id, State::Ready, "test", None);

        // Card 2: blocked by Card 1
        let c2 = board.create(Some(goal_id), "Card 2", "Blocked", Some("DoD".into()), Origin::Human, false, None).expect("c2");
        let _ = board.transition(c2.id, State::Shaping, "test", None);
        let _ = board.transition(c2.id, State::Ready, "test", None);
        let _ = board.set_blocked_by(c2.id, vec![c1.id]);

        // Bounce Card 1 through claim and release so its entered_state_at is NEWER than Card 2
        let _ = board.claim(c1.id, "agent-1", None, 60).expect("claim");
        let _ = board.release(c1.id, "agent-1").expect("release");

        let res = cockpit
            .list_column(Parameters(ColumnArg { column: Column::Ready, goal: Some(goal_id) }))
            .expect("list_column should succeed");

        let pos_c1 = res.0.items.iter().position(|i| i.id == c1.id).expect("c1 present");
        let pos_c2 = res.0.items.iter().position(|i| i.id == c2.id).expect("c2 present");
        assert!(pos_c1 < pos_c2, "Unblocked card #1 must sort before blocked card #2");
    }
}
