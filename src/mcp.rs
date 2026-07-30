//! The other UI. It happens to be an API, but it's a designed surface with the
//! same care — the cockpit has to be able to do the right thing without the
//! human reading the board, and an agent has to without any human at all.
//!
//! Two families share one state machine:
//!   * cockpit tools — what a liaison agent needs to triage and decide
//!   * worker verbs  — `list_ready` `claim` `heartbeat` `split` `escalate`
//!                     `report` `release`, and nothing else
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
    #[serde(default)]
    pub parent: Option<ItemId>,
    #[serde(default)]
    pub above_line: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChildSpec {
    pub title: String,
    /// One sentence. Not a restatement of the parent.
    pub intent: String,
    /// Must be mechanically checkable by a verifier.
    pub definition_of_done: String,
    #[serde(default)]
    pub capability: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BreakdownArg {
    pub parent: ItemId,
    pub children: Vec<ChildSpec>,
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

#[derive(Debug, Serialize, JsonSchema)]
pub struct CardLine {
    pub id: ItemId,
    pub title: String,
    pub state: String,
    pub detail: String,
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
    fn list_column(&self, Parameters(a): Parameters<ColumnArg>) -> Out<Vec<CardLine>> {
        let snap = self.board.snapshot();
        let now = snap.server_time;
        let rows = snap
            .items
            .iter()
            .filter(|i| i.state.column() == a.column)
            .filter(|i| match a.goal {
                None => true,
                Some(g) => self.board.goal_for(i.id) == g,
            })
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
                        format!("blocked by {:?}", i.blocked_by)
                    }
                    _ => i.intent.clone(),
                },
            })
            .collect();
        Ok(ToolJson(rows))
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
        description = "Drop a goal in plain language. It lands in shaping, not ready — nothing \
                       is dispatched until a breakdown is proposed and approved."
    )]
    fn create_goal(&self, Parameters(a): Parameters<CreateGoalArg>) -> Out<Ack> {
        let item = self.board.create(
            a.parent,
            a.title,
            a.intent,
            None,
            crate::model::Origin::Human,
            a.above_line,
            None,
        );
        let _ = self.board.transition(item.id, State::Shaping, "cockpit", None);
        self.ack(item.id, "created in shaping; propose a breakdown next")
    }

    #[tool(
        name = "propose_breakdown",
        description = "Decompose a goal into leaves and hold them for approval. This is the one \
                       interruption worth defending: it is cheap now and the last moment a \
                       misunderstanding costs pennies instead of forty agent-hours. Every child \
                       needs a definition of done a verifier can mechanically check."
    )]
    fn propose_breakdown(&self, Parameters(a): Parameters<BreakdownArg>) -> Out<Vec<ItemId>> {
        if self.board.get(a.parent).is_none() {
            return Err(bad(format!("no work item #{}", a.parent)));
        }
        if a.children.is_empty() {
            return Err(bad("a breakdown needs at least one child"));
        }
        let mut ids = Vec::new();
        for c in a.children {
            if c.definition_of_done.trim().is_empty() {
                return Err(bad(format!(
                    "child '{}' has no definition of done; without one the tree is a wish list",
                    c.title
                )));
            }
            let child = self.board.create(
                Some(a.parent),
                c.title,
                c.intent,
                Some(c.definition_of_done),
                crate::model::Origin::Planner,
                false,
                c.capability,
            );
            let _ = self.board.transition(child.id, State::Shaping, "planner", None);
            ids.push(child.id);
        }
        Ok(ToolJson(ids))
    }

    #[tool(
        name = "approve_plan",
        description = "Publish a proposed breakdown to the ready queue. Only call this once the \
                       human has actually seen and approved the shape — this is the gate that \
                       lets them walk away afterwards."
    )]
    fn approve_plan(&self, Parameters(a): Parameters<IdArg>) -> Out<Vec<ItemId>> {
        let mut published = Vec::new();
        for cid in self.board.children_of(a.id) {
            if self.board.get(cid).map(|i| i.state) == Some(State::Shaping)
                && self.board.transition(cid, State::Ready, "human", Some("plan approved".into())).is_ok()
            {
                published.push(cid);
            }
        }
        self.board.story(a.id, format!("Plan approved: {} items published to Ready.", published.len()));
        Ok(ToolJson(published))
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
    fn cut_scope(&self, Parameters(a): Parameters<ReasonArg>) -> Out<Vec<ItemId>> {
        let ids = self.board.cut_scope(a.id, a.reason).map_err(bad)?;
        Ok(ToolJson(ids))
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
    fn list_ready(&self, Parameters(a): Parameters<ListReadyArg>) -> Out<Vec<CardLine>> {
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
        Ok(ToolJson(rows))
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
    fn split(&self, Parameters(a): Parameters<SplitArg>) -> Out<Vec<ItemId>> {
        let children = a
            .children
            .into_iter()
            .map(|c| (c.title, c.intent, c.definition_of_done))
            .collect();
        let made = self.board.split(a.item_id, &a.agent_id, children, 7, 5).map_err(bad)?;
        Ok(ToolJson(made.into_iter().map(|i| i.id).collect()))
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
