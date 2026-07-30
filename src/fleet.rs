//! Simulated workers, behind the seam a real executor will slot into.
//!
//! Nothing here knows about HTTP or MCP — each worker drives the board through
//! exactly the seven verbs a real agent would use. When a real executor lands
//! (container? worktree? remote host? that's the open question) it implements
//! `Executor` and this loop is unchanged.

use crate::model::{EscalationOption, State, WorkItem};
use crate::schema::FleetConfig;
use crate::store::SharedBoard;

use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// What a worker decided to do this tick.
pub enum Outcome {
    Progress { progress: f32, cost_cents: u64 },
    Done { added: u32, removed: u32 },
    Escalate { question: String, options: Vec<EscalationOption>, recommended: usize },
    /// Title, intent, definition of done.
    Split(Vec<(String, String, String)>),
    /// Stop heartbeating and say nothing. No orphan-cleanup job needed — the
    /// lease is what makes this survivable.
    Die,
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn step(&self, item: &WorkItem) -> Outcome;
}

// ------------------------------------------------------------------ tiny PRNG

/// xorshift64*. A simulator needs variety, not entropy.
struct Rng(AtomicU64);

impl Rng {
    fn seeded(seed: u64) -> Self {
        Rng(AtomicU64::new(seed | 1))
    }
    fn next_u64(&self) -> u64 {
        let mut x = self.0.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.store(x, Ordering::Relaxed);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn chance(&self, p: f64) -> bool {
        self.unit() < p
    }
    fn range(&self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_u64() % (hi - lo)
    }
}

// ------------------------------------------------------------ the sim worker

pub struct SimulatedWorker {
    pub agent_id: String,
    pub capabilities: Vec<String>,
    pub model: String,
    cfg: FleetConfig,
    rng: Rng,
}

#[async_trait]
impl Executor for SimulatedWorker {
    async fn step(&self, item: &WorkItem) -> Outcome {
        if self.rng.chance(self.cfg.die_p) {
            return Outcome::Die;
        }
        // Splitting only makes sense early, before much work is sunk.
        if item.progress < 0.4 && self.rng.chance(self.cfg.split_p) {
            let base = item.title.clone();
            return Outcome::Split(vec![
                (
                    format!("{base} — happy path"),
                    format!("{} Core case only.", item.intent),
                    "Core case covered by tests.".into(),
                ),
                (
                    format!("{base} — edge cases"),
                    format!("{} Failure and retry paths.", item.intent),
                    "Failure and retry paths covered by tests.".into(),
                ),
            ]);
        }
        if self.rng.chance(self.cfg.escalate_p) {
            return Outcome::Escalate {
                question: format!(
                    "{} needs a call the contract doesn't settle: which behaviour is intended?",
                    item.title
                ),
                options: vec![
                    EscalationOption {
                        label: "Match the legacy path".into(),
                        detail: "No customer-visible change; carries the existing quirk forward."
                            .into(),
                    },
                    EscalationOption {
                        label: "Implement the documented behaviour".into(),
                        detail: "Correct, but changes behaviour for existing customers.".into(),
                    },
                ],
                recommended: 0,
            };
        }

        let step = 0.12 + self.rng.unit() as f32 * 0.22;
        let progress = (item.progress + step).min(1.0);
        let cost_cents = self.rng.range(4, 30);

        if progress >= 1.0 {
            Outcome::Done {
                added: self.rng.range(20, 480) as u32,
                removed: self.rng.range(0, 90) as u32,
            }
        } else {
            Outcome::Progress { progress, cost_cents }
        }
    }
}

// --------------------------------------------------------------- the harness

pub fn spawn(board: SharedBoard, cfg: FleetConfig) {
    for n in 1..=cfg.size {
        let worker = SimulatedWorker {
            agent_id: format!("agent-{n}"),
            // One specialist so capability-tagged cards have somewhere to go.
            capabilities: if n == 1 {
                vec!["any".into(), "writer".into()]
            } else {
                vec!["any".into()]
            },
            model: if n % 3 == 0 { "codex".into() } else { "opus".into() },
            cfg: cfg.clone(),
            rng: Rng::seeded(0x9E37_79B9_7F4A_7C15 ^ (n as u64).wrapping_mul(0x1234_5678_9ABC_DEF)),
        };
        tokio::spawn(agent_loop(board.clone(), worker));
    }

    tokio::spawn(sweeper_loop(board.clone(), cfg.clone()));
    tokio::spawn(verifier_loop(board, cfg));
}

async fn agent_loop(board: SharedBoard, worker: SimulatedWorker) {
    let tick = Duration::from_millis(worker.cfg.tick_ms);
    let lease = worker.cfg.lease_secs;
    let mut holding: Option<u64> = None;

    loop {
        tokio::time::sleep(tick).await;

        // Pull, don't wait to be pushed. Self-balancing: a dead agent simply
        // stops claiming.
        let Some(id) = holding else {
            // Adopt anything still leased to us — a card left mid-flight by a
            // restart is ours to finish, not to abandon until the lease lapses.
            if let Some(mine) = board.leased_to(&worker.agent_id) {
                holding = Some(mine);
                continue;
            }
            let ready = board.list_ready(&worker.capabilities);
            if ready.is_empty() {
                continue;
            }
            let pick = ready[(worker.rng.next_u64() as usize) % ready.len()].id;
            if board.claim(pick, &worker.agent_id, Some(worker.model.clone()), lease).is_ok() {
                holding = Some(pick);
            }
            continue;
        };

        // A human may have halted, re-routed or cut this card out from under us.
        let Some(item) = board.get(id) else {
            holding = None;
            continue;
        };
        if !matches!(item.state, State::Claimed | State::Running) {
            holding = None;
            continue;
        }
        if item.lease.as_ref().map(|l| l.agent_id != worker.agent_id).unwrap_or(true) {
            holding = None;
            continue;
        }

        match worker.step(&item).await {
            Outcome::Progress { progress, cost_cents } => {
                if board
                    .heartbeat(id, &worker.agent_id, progress, cost_cents, lease)
                    .is_err()
                {
                    holding = None;
                }
            }
            Outcome::Done { added, removed } => {
                let _ = board.report(id, &worker.agent_id, added, removed, vec![
                    "lint".into(),
                    "types".into(),
                    "tests".into(),
                ]);
                holding = None;
            }
            Outcome::Escalate { question, options, recommended } => {
                let _ = board.escalate(id, &worker.agent_id, question, options, recommended);
                holding = None;
            }
            Outcome::Split(children) => {
                // Depth and fan-out are explicit settings; exceeding them
                // escalates rather than failing silently.
                if let Err(e) = board.split(id, &worker.agent_id, children, 7, 5) {
                    tracing::debug!("split refused for #{id}: {e}");
                    let _ = board.escalate(
                        id,
                        &worker.agent_id,
                        format!("This work needs decomposing but the governor refused: {e}"),
                        vec![
                            EscalationOption {
                                label: "Raise the governor".into(),
                                detail: "Allow deeper decomposition on this branch.".into(),
                            },
                            EscalationOption {
                                label: "Re-scope the card".into(),
                                detail: "Rewrite the contract so it fits in one card.".into(),
                            },
                        ],
                        1,
                    );
                }
                holding = None;
            }
            Outcome::Die => {
                // Say nothing. The lease will expire and the sweeper will
                // requeue the card.
                tracing::debug!("{} went dark on #{id}", worker.agent_id);
                holding = None;
            }
        }
    }
}

async fn sweeper_loop(board: SharedBoard, cfg: FleetConfig) {
    let mut t = tokio::time::interval(Duration::from_millis(cfg.tick_ms));
    loop {
        t.tick().await;
        for id in board.sweep_leases() {
            tracing::info!("lease expired on #{id}; requeued");
        }
    }
}

/// The verifier. Gates for an LTS branch would not be the gates for main —
/// branch awareness is on the list, not in the POC.
async fn verifier_loop(board: SharedBoard, cfg: FleetConfig) {
    let rng = Rng::seeded(0xDEAD_BEEF_CAFE_F00D);
    let mut t = tokio::time::interval(Duration::from_millis(cfg.tick_ms));
    loop {
        t.tick().await;
        let now = chrono::Utc::now();
        let verifying: Vec<WorkItem> = board
            .snapshot()
            .items
            .into_iter()
            .filter(|i| i.state == State::Verifying)
            .filter(|i| i.time_in_state(now).num_seconds() >= 4)
            .collect();

        for item in verifying {
            let passed = !rng.chance(cfg.gate_fail_p);
            let detail = if passed {
                "lint, types, tests green".to_string()
            } else {
                let which = ["tests", "types", "lint"][(rng.next_u64() % 3) as usize];
                format!("{which} failed")
            };
            let _ = board.settle_gates(item.id, passed, &detail);
        }
    }
}
