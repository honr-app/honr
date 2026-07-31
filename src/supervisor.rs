//! The execution side of the board.
//!
//! An agent is **material, not a participant in the control plane**. It gets no
//! network path to honr; the supervisor calls `claim`/`heartbeat`/`report` on
//! its behalf. An agent that could reach honr's MCP could approve its own
//! review. (OpenShell only forwards host→sandbox anyway, which independently
//! forces this shape.)
//!
//! Three properties are load-bearing:
//!
//! - **Liveness and cost are observed, never self-reported.** Both come from
//!   parsing the agent's `stream-json` as it arrives, so a hung agent cannot
//!   claim to be fine and a chatty one cannot under-report spend.
//! - **Everything fails as a hang.** Every exec carries a deadline, and silence
//!   is treated as failure rather than patience.
//! - **The supervisor reads the run; it does not own it.** The agent is started
//!   detached and writes to a log, so watching is a thing a *different* honr
//!   process can pick up after a restart. See `reconcile`.

use crate::model::{ItemId, State, WorkItem};
use crate::openshell::{OpenShell, Output, SandboxSpec, LABEL_ITEM};
use crate::schema::{AgentConfig, ExecutionConfig};
use crate::store::{ClaimGrant, SharedBoard};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Where the agent works inside the sandbox. `/sandbox` is $HOME and writable;
/// the policy's `read_write` list has to agree with this.
const WORKDIR: &str = "/sandbox/repo";
const SHIM_LOCAL: &str = "sandbox/metadata-shim.py";
/// `sandbox upload` takes a destination **directory**, not a destination file:
/// uploading to `/tmp/metadata-shim.py` creates a *directory* of that name with
/// the file inside it, and python then reports
/// `can't find '__main__' module in '/tmp/metadata-shim.py'`.
const SHIM_DEST_DIR: &str = "/tmp";
const SHIM_REMOTE: &str = "/tmp/metadata-shim.py";

/// The agent's output, its process group, and its exit code — in `/tmp` rather
/// than the checkout, so the agent's own `git clean` cannot take the record of
/// its run with it. These three files are the entire contract between a run and
/// whichever supervisor happens to be watching it.
const AGENT_LOG: &str = "/tmp/agent.log";
const AGENT_PID: &str = "/tmp/agent.pid";
const AGENT_STATUS: &str = "/tmp/agent.status";

type Active = Arc<std::sync::Mutex<std::collections::HashSet<ItemId>>>;
type Cooldown = Arc<std::sync::Mutex<Option<std::time::Instant>>>;

pub fn spawn(board: SharedBoard, cfg: ExecutionConfig) {
    if !cfg.agents.enabled {
        tracing::info!("execution.agents.enabled = false; board runs with no executor");
        tokio::spawn(sweeper_loop(board, cfg));
        return;
    }
    if let Err(e) = cfg.agents.validate() {
        tracing::error!("agents enabled but misconfigured: {e}");
        tokio::spawn(sweeper_loop(board, cfg));
        return;
    }
    // The sweeper starts *inside* `dispatch_loop`, once reconciliation has
    // finished. A card that was mid-run when honr died has not been
    // heartbeaten since, so a sweep that lands first requeues a run that is
    // still going — and then dispatch starts a second agent on the same branch.
    tokio::spawn(dispatch_loop(board, cfg));
}

/// What makes pull-based dispatch survivable: a dead agent simply stops
/// renewing and the card returns to Ready. No orphan-cleanup job needed.
async fn sweeper_loop(board: SharedBoard, cfg: ExecutionConfig) {
    let mut t = tokio::time::interval(Duration::from_millis(cfg.sweep_interval_ms));
    loop {
        t.tick().await;
        for id in board.sweep_leases() {
            tracing::info!("lease expired on #{id}; requeued");
        }
    }
}

/// Spend since process start, in cents. Coarse on purpose — a daily ceiling
/// that resets on restart is a backstop against a runaway loop, not accounting.
static SPENT_TODAY: AtomicU64 = AtomicU64::new(0);

/// Wait this long after the infrastructure fails before trying again. The
/// podman machine stops on its own — three times in one session — and retrying
/// every 3s just converts an outage into a wall of identical errors.
const INFRA_COOLDOWN: Duration = Duration::from_secs(60);

/// Did this run fail because of the machinery rather than the card?
///
/// It matters because the two get different treatment. A card that genuinely
/// cannot be done should exhaust its retries and ask a human. A dead podman
/// socket must not burn those retries — otherwise the board reports "failed to
/// run 3 times without producing any work" about a card that never got the
/// chance to run at all, which is exactly what it did report.
fn is_infrastructure(err: &str) -> bool {
    const SIGNS: [&str; 5] = [
        "podman.sock",
        "connection error",
        "connection closed before message completed",
        "create sandbox failed",
        "gateway",
    ];
    SIGNS.iter().any(|s| err.contains(s))
}

/// What every run shares with the loop it belongs to.
///
/// Bundled rather than threaded through as six arguments, because dispatch and
/// adoption both need all of it and the bookkeeping around a run must not
/// differ by how the run started.
#[derive(Clone)]
struct Fleet {
    board: SharedBoard,
    os: Arc<OpenShell>,
    agents: Arc<AgentConfig>,
    in_flight: Arc<AtomicU64>,
    /// Which cards this process is actively running.
    ///
    /// The lease is time-based and cannot see in-process state: a long silent
    /// tool call lets the sweeper requeue a card whose supervisor task is still
    /// alive, and dispatch would then claim it again and race itself on one
    /// branch. A sandbox label is *not* the right evidence here — failed
    /// sandboxes are deliberately kept for inspection, so the label outlives
    /// the run and would deadlock every retry. `reconcile` is the one place
    /// that reads labels, and it cross-checks them against the card.
    active: Active,
    cooldown: Cooldown,
    lease_secs: i64,
}

impl Fleet {
    /// Everything that has to happen around a run, whichever way it started.
    ///
    /// Adopted runs go through here too, so a run that survived a restart
    /// cannot quietly get different failure accounting from a fresh one.
    fn supervise<F>(&self, id: ItemId, agent_id: String, work: F)
    where
        F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.active.lock().unwrap().insert(id);
        let f = self.clone();
        tokio::spawn(async move {
            match work.await {
                Ok(()) => f.board.clear_run_failures(id),
                Err(e) => {
                    let msg = e.to_string();
                    if is_infrastructure(&msg) {
                        // Not the card's fault. Give it back untouched and stop
                        // dispatching for a while rather than spending the
                        // card's retry budget on a broken machine.
                        tracing::warn!("#{id}: infrastructure failure, not counting it: {msg}");
                        *f.cooldown.lock().unwrap() =
                            Some(std::time::Instant::now() + INFRA_COOLDOWN);
                        let _ = f.board.release(id, &agent_id);
                    } else {
                        tracing::error!("#{id} failed: {msg}");
                        // Count it. A run that dies early spends nothing, so no
                        // money cap stops the sweeper requeueing it forever —
                        // after `max_attempts` this becomes a human's problem
                        // instead of an overnight loop.
                        if let Err(e2) =
                            f.board.record_run_failure(id, &msg, f.agents.max_attempts)
                        {
                            tracing::error!("#{id}: could not record failure: {e2}");
                        }
                    }
                }
            }
            f.active.lock().unwrap().remove(&id);
            f.in_flight.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

async fn dispatch_loop(board: SharedBoard, cfg: ExecutionConfig) {
    let fleet = Fleet {
        board: board.clone(),
        os: Arc::new(OpenShell::default()),
        agents: Arc::new(cfg.agents.clone()),
        in_flight: Arc::default(),
        active: Arc::default(),
        cooldown: Arc::default(),
        lease_secs: cfg.lease_secs,
    };

    // Pick up whatever survived the last process before anything else — the
    // sweeper included — gets a chance to act on those cards.
    for a in reconcile_once_reachable(&fleet.os, &board, cfg.lease_secs, GATEWAY_GRACE).await {
        let (id, agent_id) = (a.item_id, a.agent_id.clone());
        fleet.supervise(id, agent_id, adopt_card(fleet.clone(), a));
    }

    tokio::spawn(sweeper_loop(board.clone(), cfg.clone()));

    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tick.tick().await;

        if fleet.in_flight.load(Ordering::Relaxed) as usize >= fleet.agents.max_concurrent {
            continue;
        }
        if SPENT_TODAY.load(Ordering::Relaxed) >= fleet.agents.daily_budget_cents {
            continue;
        }
        if fleet.cooldown.lock().unwrap().is_some_and(|t| std::time::Instant::now() < t) {
            continue;
        }
        // The podman machine stops on its own. Claiming a card we can't run
        // would strand it until the lease lapsed.
        if !fleet.os.healthy().await {
            tracing::warn!("openshell gateway unhealthy; not claiming");
            continue;
        }

        let ready = board.list_ready(&["any".to_string()]);
        let Some(item) = ready
            .into_iter()
            .find(|i| !fleet.active.lock().unwrap().contains(&i.id))
        else {
            continue;
        };

        let agent_id = format!("sandbox-{}", item.id);
        let grant = match board.claim(
            item.id,
            &agent_id,
            Some(fleet.agents.vertex.model.clone()),
            cfg.lease_secs,
        ) {
            Ok(g) => g,
            Err(e) => {
                tracing::debug!("claim of #{} refused: {e}", item.id);
                continue;
            }
        };

        fleet.supervise(item.id, agent_id.clone(), run_card(fleet.clone(), agent_id, grant));
    }
}

// ------------------------------------------------------ surviving a restart

/// A run that outlived the process supervising it.
///
/// honr is rebuilt constantly while honr is what's being built, so a restart
/// mid-run is the normal case, not an incident. Killing the sandbox was the
/// safe stopgap: correct, and it threw away a five-minute run and its spend
/// every time. Re-adopting keeps the run going and the card Running.
#[derive(Debug, Clone)]
struct Adoption {
    item_id: ItemId,
    agent_id: String,
    sandbox: String,
    /// First log line this process has not already accounted for. Everything
    /// before it was streamed — and charged — by the process that died.
    from_line: u64,
    /// The run's cumulative spend as of that line. The stream reports a running
    /// total and the supervisor charges the *difference*, so without a starting
    /// point the next cost line would bill the whole run a second time.
    seen_cents: u64,
}

/// The card this sandbox belongs to, if the sandbox is worth adopting.
///
/// The card decides, not the sandbox. A failed sandbox is deliberately *kept*
/// for inspection, so its existence proves nothing about whether a run is live;
/// and a retry leaves the previous attempt's sandbox behind under the same
/// `honr.item` label, so the label alone cannot say which one to watch.
/// `environment` names the current attempt, and that is the only thing that
/// can. Everything this rejects gets reaped.
fn adoptable<'a>(item: Option<&'a WorkItem>, sandbox: &str) -> Option<&'a WorkItem> {
    item.filter(|i| {
        matches!(i.state, State::Claimed | State::Running)
            && i.environment.as_deref() == Some(sandbox)
    })
}

/// How long startup waits for the gateway before giving up on reconciling.
///
/// Generous, because the podman machine takes tens of seconds to come up and
/// honr and podman tend to start at the same time. Bounded, because a gateway
/// that is never coming back must not leave every Running card frozen.
const GATEWAY_GRACE: Duration = Duration::from_secs(180);
const GATEWAY_POLL: Duration = Duration::from_secs(5);

/// Reconcile, but only once the gateway can actually answer.
///
/// Skipping reconciliation is not the neutral choice it looks like. If honr
/// cannot enumerate sandboxes then it does not know which runs are still live,
/// and the sweeper — which starts immediately after this returns — requeues a
/// card whose agent is still working. Dispatch then claims it again and races a
/// second agent onto the branch the first one is already pushing to. That is
/// exactly the failure re-adoption exists to prevent, reached from the other
/// side, and "the podman machine stops on its own" makes it reachable.
///
/// Waiting costs nothing. Dispatch refuses to claim without a healthy gateway
/// anyway, so a sweep during an outage cannot produce work — it can only turn
/// live runs into lies about them.
async fn reconcile_once_reachable(
    os: &OpenShell,
    board: &SharedBoard,
    lease_secs: i64,
    grace: Duration,
) -> Vec<Adoption> {
    let deadline = std::time::Instant::now() + grace;
    let mut announced = false;
    while !os.healthy().await {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            // Loud, because the board is about to be less trustworthy than
            // usual: anything that survived the restart is now invisible to us.
            tracing::error!(
                "gateway unreachable after {}s; starting without reconciling. A run that \
                 survived the restart will not be adopted and may be requeued.",
                grace.as_secs()
            );
            return Vec::new();
        }
        if !announced {
            tracing::warn!(
                "gateway unreachable; holding dispatch and the sweeper until it answers"
            );
            announced = true;
        }
        // Never sleep past the deadline — the point is a bounded wait, not a
        // wait rounded up to the poll interval.
        tokio::time::sleep(GATEWAY_POLL.min(left)).await;
    }
    reconcile(os, board, lease_secs).await
}

/// Match live sandboxes back to the board, before anything else touches them.
async fn reconcile(os: &OpenShell, board: &SharedBoard, lease_secs: i64) -> Vec<Adoption> {
    let Ok(ours) = os.list_ours().await else {
        tracing::warn!("could not list sandboxes; skipping reconciliation");
        return Vec::new();
    };

    let mut adopted = Vec::new();
    for sb in ours {
        let Some(id) = sb.item_id() else { continue };
        let card = board.get(id);
        let Some(item) = adoptable(card.as_ref(), &sb.name) else {
            tracing::info!("reaping orphaned sandbox {}", sb.name);
            let _ = os.delete(&sb.name).await;
            continue;
        };

        match adopt(os, board, item, &sb.name, lease_secs).await {
            Some(a) => {
                tracing::info!("#{id}: re-attached to {} from line {}", sb.name, a.from_line);
                adopted.push(a);
            }
            None => {
                // The sandbox is up but nothing is running in it — honr died
                // during setup, or the agent exited and nothing cleaned up
                // after it. There is no run to watch, so give the card back.
                // A restart is not the card's fault, so it costs no retry
                // budget; it just gets dispatched again from the top.
                tracing::warn!("#{id}: {} has no live agent; requeueing", sb.name);
                let _ = os.delete(&sb.name).await;
                board.set_environment(id, None);
                let _ = board.transition(
                    id,
                    State::Ready,
                    "supervisor",
                    Some("honr restarted and found no live agent in the sandbox".into()),
                );
            }
        }
    }
    adopted
}

/// Ask a sandbox what its agent is doing, and take over if there is one.
async fn adopt(
    os: &OpenShell,
    board: &SharedBoard,
    item: &WorkItem,
    sandbox: &str,
    lease_secs: i64,
) -> Option<Adoption> {
    let id = item.id;
    // A probe that hangs is a sandbox we cannot reason about, and this stack
    // fails as a hang. Treat it as "no live run" and give the card back rather
    // than watching something that may not be there.
    let out = match os.exec(sandbox, &probe_script(), Duration::from_secs(30)).await {
        Ok(o) if o.ok() => o,
        Ok(o) => {
            tracing::warn!("#{id}: probe of {sandbox} failed: {}", outerr(&o));
            return None;
        }
        Err(e) => {
            tracing::warn!("#{id}: could not probe {sandbox}: {e}");
            return None;
        }
    };
    let (from_line, seen_cents) = probe_of(&out.stdout)?;

    let agent_id = item
        .lease
        .as_ref()
        .map(|l| l.agent_id.clone())
        .unwrap_or_else(|| format!("sandbox-{id}"));

    // Renew the lease now. It has not been touched since before the restart,
    // and the sweeper is seconds from deciding this card is dead. This is also
    // what puts a Claimed card back into Running.
    //
    // A failure here is not a reason to abandon the run: the agent is alive
    // either way, and watching it beats leaving it to spend unobserved. But it
    // does mean the sweeper may requeue a live card, so it must not pass
    // silently.
    if let Err(e) = board.heartbeat(id, &agent_id, item.progress, 0, lease_secs) {
        tracing::error!("#{id}: adopted {sandbox} but could not renew its lease: {e}");
    }
    board.story(id, format!("honr restarted; picked {sandbox} back up rather than killing it."));

    Some(Adoption {
        item_id: id,
        agent_id,
        sandbox: sandbox.to_string(),
        from_line,
        seen_cents,
    })
}

/// Where a live run had got to, or `None` if nothing is running.
fn probe_of(stdout: &str) -> Option<(u64, u64)> {
    if !stdout.contains(MARK_ALIVE) && !stdout.contains(MARK_EXITED) {
        return None;
    }
    let lines: u64 = stdout.lines().find_map(|l| l.strip_prefix(MARK_LINES))?.trim().parse().ok()?;
    let seen = stdout
        .lines()
        .find_map(|l| l.strip_prefix(MARK_COST))
        .and_then(parse_cost_cents)
        .unwrap_or(0);
    Some((lines + 1, seen))
}

// ----------------------------------------------------------- the lifecycle

async fn run_card(f: Fleet, agent_id: String, grant: ClaimGrant) -> anyhow::Result<()> {
    let (board, os, cfg) = (&f.board, &f.os, &f.agents);
    let id = grant.item_id;
    // Attempt-scoped, because a failed sandbox is *kept* for inspection and a
    // retry would otherwise collide with its name. The `honr.item` label is
    // what reconciliation matches on, so it stays stable across attempts.
    let attempt = board.get(id).map(|i| i.run_failures).unwrap_or(0) + 1;
    let name = format!("honr-card-{id}-a{attempt}");
    let branch = format!("honr/card-{id}");

    // Recorded before creation so a crash between here and `create` still
    // leaves a name to reconcile against — and, now, a name to re-adopt.
    board.set_environment(id, Some(name.clone()));

    let spec = SandboxSpec {
        name: name.clone(),
        from: cfg.image.clone(),
        providers: cfg.providers.clone(),
        policy: Some(cfg.policy.clone()),
        env: agent_env(cfg),
        labels: vec![(LABEL_ITEM.to_string(), id.to_string())],
        cpu: cfg.cpu.clone(),
        memory: cfg.memory.clone(),
    };

    let result =
        run_inside(board, os, cfg, &agent_id, &grant, &name, &branch, f.lease_secs, &spec).await;
    finalize(os, board, id, &name, &result).await;
    result
}

/// Take over a run this process did not start: join it at the watch step, with
/// the setup already done and the briefing already delivered.
async fn adopt_card(f: Fleet, a: Adoption) -> anyhow::Result<()> {
    let (board, os, cfg) = (&f.board, &f.os, &f.agents);
    let id = a.item_id;
    let branch = format!("honr/card-{id}");
    let result = async {
        let (run, spent) = watch_agent(
            board,
            os,
            cfg,
            &a.agent_id,
            id,
            &a.sandbox,
            a.from_line,
            a.seen_cents,
            f.lease_secs,
        )
        .await?;
        finish(board, os, cfg, &a.agent_id, id, &a.sandbox, &branch, &run, spent).await
    }
    .await;
    finalize(os, board, id, &a.sandbox, &result).await;
    result
}

async fn finalize(
    os: &OpenShell,
    board: &SharedBoard,
    id: ItemId,
    name: &str,
    result: &anyhow::Result<()>,
) {
    match result {
        Ok(_) => {
            let _ = os.delete(name).await;
            board.set_environment(id, None);
        }
        // Keep the sandbox on failure: `openshell logs` is the tool that
        // actually answers questions, and a deleted sandbox answers none. Stop
        // the agent first, though — it is detached now, so dropping the exec
        // that was watching it no longer stops it spending.
        Err(e) => {
            stop_agent(os, name).await;
            tracing::error!("#{id}: keeping sandbox {name} for inspection: {e}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_inside(
    board: &SharedBoard,
    os: &OpenShell,
    cfg: &AgentConfig,
    agent_id: &str,
    grant: &ClaimGrant,
    name: &str,
    branch: &str,
    lease_secs: i64,
    spec: &SandboxSpec,
) -> anyhow::Result<()> {
    let id = grant.item_id;
    let short = Duration::from_secs(180);

    // Setup emits no agent output, so without this the card looks dead from
    // the moment it is claimed until `claude` produces its first line — long
    // enough for the sweeper to requeue a healthy run. Each call marks a step
    // that actually completed, so this stays evidence of progress rather than
    // an assertion of liveness.
    let beat = |p: f32| {
        let _ = board.heartbeat(id, agent_id, p, 0, lease_secs);
    };

    os.create(spec).await?;
    beat(0.01);

    // Preamble. Without the shim there is no Vertex auth at all: google-auth
    // walks its ADC chain to the GCE metadata server, which OpenShell blocks
    // permanently as SSRF hardening. The shim must outlive the agent.
    os.upload(name, SHIM_LOCAL, SHIM_DEST_DIR).await?;
    let up = os
        .exec(
            name,
            &format!(
                r#"set -e
nohup python3 {SHIM_REMOTE} >/tmp/shim.log 2>&1 &
for i in $(seq 1 40); do
  if curl -sf -H 'Metadata-Flavor: Google' http://127.0.0.1:8127/ >/dev/null; then
    echo shim-up; exit 0
  fi
  sleep 0.25
done
echo shim-down >&2; cat /tmp/shim.log >&2; exit 1"#
            ),
            short,
        )
        .await?;
    anyhow::ensure!(up.ok(), "metadata shim never came up: {}", outerr(&up));
    beat(0.02);

    // Clone the fork. GIT_TERMINAL_PROMPT=0 is not optional: without it a
    // missing credential blocks forever on an interactive username prompt,
    // which looks exactly like a slow clone.
    let clone = os.exec(name, &clone_script(cfg, branch), short).await?;
    anyhow::ensure!(clone.ok(), "clone failed: {}", outerr(&clone));
    let branch_state = branch_state_of(&clone.stdout);
    beat(0.03);

    // ---- the agent -------------------------------------------------------

    let briefing = briefing(grant, branch_state, branch, &cfg.repo.upstream, &cfg.repo.base);
    let start = os.exec(name, &start_script(cfg, &briefing), short).await?;
    anyhow::ensure!(start.ok(), "agent did not start: {}", outerr(&start));
    beat(0.04);

    // From the top of the log with nothing spent: this run is ours from its
    // first line. An adopted run joins here instead, further in.
    let (run, spent) =
        watch_agent(board, os, cfg, agent_id, id, name, 1, 0, lease_secs).await?;
    finish(board, os, cfg, agent_id, id, name, branch, &run, spent).await
}

/// Watch a detached agent to completion: heartbeat on every line it writes,
/// charge cost as it arrives, and hand back its exit status.
///
/// `from_line` and `seen_cents` are the whole reason this is a separate step.
/// They make watching *resumable*: a supervisor that starts halfway through a
/// run skips the lines a previous one already streamed, and starts its cost
/// arithmetic from what that one already charged.
#[allow(clippy::too_many_arguments)]
async fn watch_agent(
    board: &SharedBoard,
    os: &OpenShell,
    cfg: &AgentConfig,
    agent_id: &str,
    id: ItemId,
    name: &str,
    from_line: u64,
    seen_cents: u64,
    lease_secs: i64,
) -> anyhow::Result<(Output, u64)> {
    let spent = Arc::new(AtomicU64::new(seen_cents));
    let started = std::time::Instant::now();
    // The agent carries its own deadline inside the sandbox; this one only has
    // to outlast it, so a hung *follower* still fails rather than waiting.
    let timeout = Duration::from_secs(cfg.agent_timeout_secs) + Duration::from_secs(120);
    // An adopted run is already part-way along, and a card whose progress bar
    // walks backwards after a restart is the board lying about the run.
    let floor = board.get(id).map(|i| i.progress).unwrap_or(0.0);

    let (board2, spent2) = (board.clone(), spent.clone());
    let agent_owned = agent_id.to_string();

    let run = os
        .exec_streaming(name, &follow_script(from_line), timeout, move |line| {
            // Progress is not knowable from the stream, so it is reported as
            // elapsed-against-deadline and capped below 1.0 — honest about
            // being an estimate, and monotonic, which is what the card face
            // needs. Only `report` sets 1.0.
            let frac = started.elapsed().as_secs_f32() / timeout.as_secs_f32();
            let progress = frac.max(floor).clamp(0.0, 0.95);

            let cents = parse_cost_cents(line);
            let delta = cents
                .map(|c| {
                    let prev = spent2.swap(c, Ordering::Relaxed);
                    c.saturating_sub(prev)
                })
                .unwrap_or(0);
            if delta > 0 {
                SPENT_TODAY.fetch_add(delta, Ordering::Relaxed);
            }

            // Every line is an activity ping. A stream-format change therefore
            // degrades to "still alive" rather than crashing the supervisor.
            let _ = board2.heartbeat(id, &agent_owned, progress, delta, lease_secs);
        })
        .await?;

    Ok((run, spent.load(Ordering::Relaxed)))
}

/// Settle a finished run: check what it cost, check it succeeded, and put the
/// PR on the board.
#[allow(clippy::too_many_arguments)]
async fn finish(
    board: &SharedBoard,
    os: &OpenShell,
    cfg: &AgentConfig,
    agent_id: &str,
    id: ItemId,
    name: &str,
    branch: &str,
    run: &Output,
    spent: u64,
) -> anyhow::Result<()> {
    let short = Duration::from_secs(180);
    let budget = cfg.per_card_budget_cents;
    if spent > budget {
        anyhow::bail!("per-card budget breached: {spent}c > {budget}c");
    }
    anyhow::ensure!(run.ok(), "agent exited {}: {}", run.code, outerr(run));

    // ---- verify what the agent published ---------------------------------
    //
    // The agent pushes and opens the PR; the supervisor only asks GitHub what
    // happened. That split is deliberate, and it is a reversal.
    //
    // The supervisor used to script the publish itself, justified as
    // "deterministic, and it keeps gh out of the agent's hands". The second
    // half was never true — gh is in the image and GITHUB_TOKEN is in the
    // environment, so the agent always had this capability. And the
    // determinism bought nothing: every one of upload-dest-is-a-directory,
    // non-idempotent `gh pr create`, URL-scraped-from-stdout and
    // --force-with-lease-against-a-URL was a failure in *our* shell, not in
    // the agent. Meanwhile the agent resolved a seven-commit rebase conflict
    // unaided.
    //
    // What is left here is a *query*, not a script that has to keep working.
    // Containment comes from GitHub: the bot has no write access to upstream,
    // so the worst it can do is make a mess of a disposable fork.
    let pr = os.exec(name, &pr_lookup_script(cfg, branch), short).await?;
    anyhow::ensure!(pr.ok(), "could not ask GitHub about the PR: {}", outerr(&pr));
    let url = pr
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix(PR_URL_MARK))
        .map(str::to_string)
        // A Review card with no PR is a card you cannot action, so this is a
        // failure rather than a quietly empty field.
        .ok_or_else(|| {
            anyhow::anyhow!("agent finished but opened no PR for {branch}")
        })?;
    board.set_pr_url(id, Some(url.clone()));

    // Gates are the agent's own claim for now; a clean-checkout verifier the
    // agent cannot influence is the next hardening step.
    //
    // `report` hands the card to the verifier, and with the simulated verifier
    // deleted there is nothing else to settle it — a card would sit in Verify
    // forever. Settling here keeps the lifecycle closed, and names the gate
    // honestly so the board never implies more assurance than we have.
    board.report(id, agent_id, 0, 0, vec!["agent-reported".into()])?;
    board
        .settle_gates(id, true, "agent-reported; supervisor-run gates not implemented yet")
        .map_err(|e| anyhow::anyhow!("settle_gates: {e}"))?;
    tracing::info!("#{id} reported; pr={url}");
    Ok(())
}

// --------------------------------------------------------------- scripts

/// Start the agent **detached**, so it outlives the exec that launched it —
/// and therefore outlives honr.
///
/// This is what makes re-adoption possible at all. As a child of the exec
/// session the agent died whenever the process watching it died, so every
/// `cargo run` threw away a live run; the supervisor had no honest option but
/// to delete the sandbox. Detached, the supervisor is a *reader of a log*
/// rather than the owner of a process, and a reader can be replaced.
///
/// Two consequences are deliberate:
///
/// - `timeout` runs inside the sandbox. Nothing out here can bound a process it
///   does not own, and an agent nobody is watching still spends money.
///   `--foreground` is load-bearing: without it `timeout` puts the command in a
///   process group of its own, so signalling the wrapper's group leaves
///   `claude` orphaned and still billing. Observed, not assumed.
/// - The briefing travels in an exported variable rather than inline. It is
///   already single-quoted for the outer shell, and quoting it a second time
///   for the inner `bash -c` is exactly the sort of thing that works until a
///   card description contains an apostrophe.
fn start_script(cfg: &AgentConfig, briefing: &str) -> String {
    let secs = cfg.agent_timeout_secs;
    format!(
        r#"set -e
rm -f {AGENT_PID} {AGENT_STATUS}
: > {AGENT_LOG}
export HONR_BRIEFING={brief}
setsid nohup bash -c 'echo $$ > {AGENT_PID}; cd {WORKDIR} && timeout --foreground {secs} claude -p "$HONR_BRIEFING" --output-format stream-json --verbose --permission-mode bypassPermissions >> {AGENT_LOG} 2>&1; echo $? > {AGENT_STATUS}' </dev/null >/dev/null 2>&1 &
for i in $(seq 1 40); do
  if [ -s {AGENT_PID} ]; then exit 0; fi
  sleep 0.25
done
echo agent-did-not-start >&2; exit 1"#,
        brief = shell_quote(briefing)
    )
}

/// Follow the agent's output from `from_line`, then exit with the agent's own
/// status.
///
/// A pure reader: running it twice, or from a different honr process, does not
/// disturb the run. The pid it waits on is the wrapper's, and the wrapper
/// writes the status file before exiting, so by the time `tail` notices the
/// process is gone the exit code is already on disk.
fn follow_script(from_line: u64) -> String {
    format!(
        r#"if [ -f {AGENT_STATUS} ]; then
  tail -n +{from_line} {AGENT_LOG} 2>/dev/null || true
  exit "$(cat {AGENT_STATUS})"
fi
tail -n +{from_line} -f --pid="$(cat {AGENT_PID})" {AGENT_LOG}
for i in $(seq 1 40); do
  if [ -f {AGENT_STATUS} ]; then break; fi
  sleep 0.25
done
exit "$(cat {AGENT_STATUS} 2>/dev/null || echo 1)""#
    )
}

pub const MARK_ALIVE: &str = "HONR-AGENT-ALIVE";
pub const MARK_EXITED: &str = "HONR-AGENT-EXITED";
pub const MARK_GONE: &str = "HONR-AGENT-GONE";
pub const MARK_LINES: &str = "HONR-LOG-LINES=";
pub const MARK_COST: &str = "HONR-LOG-COST=";

/// Ask a sandbox whether its agent is still going, and how far its log got.
///
/// The line count is what a new supervisor resumes from, and the last cost line
/// is what its arithmetic starts from — both because the previous supervisor
/// already streamed, and charged for, everything before them.
fn probe_script() -> String {
    format!(
        r#"if [ -f {AGENT_STATUS} ]; then echo {MARK_EXITED}
elif [ -s {AGENT_PID} ] && kill -0 "$(cat {AGENT_PID})" 2>/dev/null; then echo {MARK_ALIVE}
else echo {MARK_GONE}
fi
printf '%s%s\n' '{MARK_LINES}' "$(wc -l < {AGENT_LOG} 2>/dev/null || echo 0)"
printf '%s%s\n' '{MARK_COST}' "$(grep -h total_cost_usd {AGENT_LOG} 2>/dev/null | tail -1)""#
    )
}

/// Stop a detached agent, best effort.
///
/// Only the failure path needs this. The sandbox is kept for inspection, and
/// the agent is no longer a child of anything we hold — so without this a run
/// we have already given up on keeps burning Vertex spend until its own
/// timeout. `setsid` made the wrapper a process-group leader, so negating the
/// pid takes `claude` with it.
async fn stop_agent(os: &OpenShell, name: &str) {
    let script = format!(
        r#"if [ -s {AGENT_PID} ]; then kill -TERM -"$(cat {AGENT_PID})" 2>/dev/null || true; fi"#
    );
    let _ = os.exec(name, &script, Duration::from_secs(30)).await;
}

fn agent_env(cfg: &AgentConfig) -> Vec<(String, String)> {
    vec![
        ("CLAUDE_CODE_USE_VERTEX".into(), "1".into()),
        ("ANTHROPIC_VERTEX_PROJECT_ID".into(), cfg.vertex.project.clone()),
        ("CLOUD_ML_REGION".into(), cfg.vertex.location.clone()),
        ("ANTHROPIC_MODEL".into(), cfg.vertex.model.clone()),
        // Point google-auth at the shim instead of the blocked metadata server.
        ("GCE_METADATA_HOST".into(), "127.0.0.1:8127".into()),
        ("DISABLE_TELEMETRY".into(), "1".into()),
        ("DISABLE_ERROR_REPORTING".into(), "1".into()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
        // The image's own ENV does NOT reach `sandbox exec` — PATH arrives as
        // the base image's default and CARGO_HOME arrives empty, so cargo is
        // invisible and rustup cannot pick a toolchain. Baking ENV into the
        // Containerfile is not sufficient; it has to be passed explicitly.
        ("RUSTUP_HOME".into(), "/opt/rust".into()),
        ("CARGO_HOME".into(), "/opt/cargo".into()),
        ("NPM_CONFIG_CACHE".into(), "/opt/npm-cache".into()),
        ("PATH".into(), "/opt/cargo/bin:/sandbox/.venv/bin:/usr/local/bin:/usr/bin:/bin".into()),
    ]
}

/// A credential helper that echoes the injected token. The value is OpenShell's
/// opaque placeholder; the egress proxy substitutes the real one.
const GIT_CRED: &str =
    r#"credential.helper=!f(){ echo username=x-access-token; echo password=$GITHUB_TOKEN; };f"#;

/// Marker lines the supervisor reads back out of the clone step, so the
/// briefing can tell the agent what it is walking into.
pub const MARK_FRESH: &str = "HONR-BRANCH-FRESH";
pub const MARK_REBASED: &str = "HONR-BRANCH-REBASED";
pub const MARK_CONFLICT: &str = "HONR-BRANCH-CONFLICT";

/// Clone, and **resume the card's branch if it already exists.**
///
/// Always branching from base was wrong the moment a card could be re-run:
/// the agent would start over from scratch and its push would be rejected as
/// non-fast-forward against its own earlier work. That is precisely the
/// "changes requested, go fix it" path.
///
/// Not shallow. A rebase against base needs real history, and honr is small.
fn clone_script(cfg: &AgentConfig, branch: &str) -> String {
    let fork = &cfg.repo.fork;
    let upstream = &cfg.repo.upstream;
    let base = &cfg.repo.base;
    format!(
        r#"set -e
export GIT_TERMINAL_PROMPT=0
rm -rf {WORKDIR}
git -c '{GIT_CRED}' clone -q --branch {base} https://github.com/{fork}.git {WORKDIR}
cd {WORKDIR}
git config user.email "agent@honr.local"
git config user.name "honr agent"
# The fork's own base drifts the moment upstream moves, and nothing syncs it.
# The PR targets upstream, so upstream is the only base worth rebasing onto.
git remote add upstream https://github.com/{upstream}.git
git -c '{GIT_CRED}' fetch -q upstream {base}
if git -c '{GIT_CRED}' ls-remote --exit-code --heads origin {branch} >/dev/null 2>&1; then
  git -c '{GIT_CRED}' fetch -q origin {branch}
  git checkout -q -B {branch} origin/{branch}
else
  git checkout -q -B {branch} upstream/{base}
  echo {MARK_FRESH}
  exit 0
fi
# Rebase so the branch is reviewable against what it will actually merge into.
# A conflict is not a supervisor failure — resolving it needs the semantics of
# the change, so leave the branch alone and say so in the briefing.
if git rebase -q upstream/{base} >/dev/null 2>&1; then
  echo {MARK_REBASED}
else
  git rebase --abort >/dev/null 2>&1 || true
  echo {MARK_CONFLICT}
fi"#
    )
}

/// Ask GitHub whether the agent actually opened a PR.
///
/// Not "create a PR" — the agent does that. This is the supervisor checking a
/// fact it is going to put on the board, which is the one thing it must not
/// take on trust. A query keeps working when tool output changes; a script
/// that creates things has to be right about flags, idempotency and failure
/// modes, and ours repeatedly was not.
fn pr_lookup_script(cfg: &AgentConfig, branch: &str) -> String {
    let upstream = &cfg.repo.upstream;
    format!(
        r#"set -e
export GH_TOKEN=$GITHUB_TOKEN
url=$(gh pr list --repo {upstream} --head {branch} --state open --json url --jq '.[0].url // empty')
if [ -n "$url" ]; then echo "{PR_URL_MARK}$url"; fi"#
    )
}

/// Prefix so the URL is read from a line we chose, not guessed at.
pub const PR_URL_MARK: &str = "HONR-PR-URL=";

/// What the agent is walking into, read back from the clone step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchState {
    /// New branch off base — nothing done on this card yet.
    Fresh,
    /// The card's branch already existed and rebased cleanly onto base.
    Rebased,
    /// The branch exists but conflicts with base. Resolving that is the agent's
    /// job, not the supervisor's — it needs the semantics to do it safely.
    Conflicted,
}

fn branch_state_of(stdout: &str) -> BranchState {
    if stdout.contains(MARK_CONFLICT) {
        BranchState::Conflicted
    } else if stdout.contains(MARK_REBASED) {
        BranchState::Rebased
    } else {
        BranchState::Fresh
    }
}

/// The intent chain is the highest-leverage payload in the system, and a fresh
/// `claude -p` in an empty container has none of it.
fn briefing(
    grant: &ClaimGrant,
    branch: BranchState,
    branch_name: &str,
    upstream: &str,
    base: &str,
) -> String {
    let mut b = String::new();
    b.push_str("You are working on one card in a larger plan. Do exactly this card.\n\n");

    if !grant.ancestry.is_empty() {
        b.push_str("Why this exists, from the top down:\n");
        for a in &grant.ancestry {
            b.push_str(&format!("  {}: {} — {}\n", a.level, a.title, a.intent));
        }
        b.push('\n');
    }

    b.push_str(&format!("Your card: {}\n", grant.title));
    if let Some(dod) = &grant.definition_of_done {
        b.push_str(&format!("Definition of done: {dod}\n"));
    }

    if !grant.constraints.is_empty() {
        b.push_str("\nStanding constraints. These bind everything below them:\n");
        for c in &grant.constraints {
            b.push_str(&format!("  - {c}\n"));
        }
    }
    if !grant.notes.is_empty() {
        b.push_str("\nNotes from the human steering this:\n");
        for n in &grant.notes {
            b.push_str(&format!("  - {n}\n"));
        }
    }

    b.push_str(match branch {
        BranchState::Fresh => {
            "\nYou are on a new branch off the base. Nothing has been done on this card yet.\n"
        }
        BranchState::Rebased => {
            "\nThis card has been worked before. You are on its existing branch, already rebased \
             onto the current base — review what is there and address the notes above rather \
             than starting over.\n"
        }
        // The supervisor deliberately does not resolve this: only something
        // that understands the change can decide what the merged result means.
        BranchState::Conflicted => {
            "\nThis card has been worked before and its branch CONFLICTS with the base. The \
             rebase was left un-applied, so you are on the branch as it was. Rebase onto \
             `origin/<base>` yourself and resolve the conflicts, keeping the intent of both \
             sides. Do this before any other work.\n"
        }
    });

    b.push_str(&format!(
        "\nRun the project's own checks before you finish — `cargo test --offline --locked` and \
         `cargo clippy --offline -- -D warnings`. Both work with no network; if either needs to \
         reach the network, something is wrong and you should say so rather than work around it.\n\
         \nWhen the work is done, publish it yourself:\n\
         \n  1. Commit on `{branch}`. Do not commit to any other branch.\n\
           2. Push to `origin` (the fork). Force-push is fine on your own branch.\n\
           3. Open a pull request against `{upstream}` base `{base}`, or update the existing \
              one if a PR for this branch is already open.\n\
         \nThe PR is how a human reviews this, so it is part of the work, not an afterthought. \
         Leave `{base}` alone.\n",
        branch = branch_name,
        upstream = upstream,
        base = base,
    ));
    b
}

// ----------------------------------------------------------------- helpers

/// Single-quote for `bash -lc`. A briefing is untrusted text as far as the
/// shell is concerned — it contains human prose, quotes and newlines.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Cost, in cents, if this stream line carries one.
///
/// Deliberately tolerant: Claude Code reports a running `total_cost_usd`, but
/// the exact shape is not a stable contract. An unrecognised line is liveness
/// and nothing more — never an error, because a stream-format change must not
/// take the supervisor down.
fn parse_cost_cents(line: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let usd = v
        .get("total_cost_usd")
        .or_else(|| v.get("cost_usd"))
        .or_else(|| v.pointer("/result/total_cost_usd"))?
        .as_f64()?;
    Some((usd * 100.0).round().max(0.0) as u64)
}

/// Both streams. git writes its actual error to stderr, so reporting only
/// stdout produced `push failed:` with nothing after the colon — a failure
/// message that says less than no message at all.
fn outerr(o: &crate::openshell::Output) -> String {
    let mut s = o.stderr.trim().to_string();
    if !o.stdout.trim().is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(o.stdout.trim());
    }
    tail(&s, 500)
}

fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.len() <= n {
        return t.to_string();
    }
    t[t.len() - n..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_is_parsed_from_the_stream() {
        assert_eq!(parse_cost_cents(r#"{"total_cost_usd":0.42}"#), Some(42));
        assert_eq!(parse_cost_cents(r#"{"type":"result","result":{"total_cost_usd":1.23}}"#), Some(123));
    }

    /// Half-cent values land wherever binary float puts them — `1.005 * 100.0`
    /// is `100.4999…`, so this is 100 rather than 101. Fine here: this figure
    /// is a budget backstop, not a ledger, and a cent of slack against a
    /// two-dollar cap changes nothing. It would not be fine if it were billing.
    #[test]
    fn sub_cent_precision_is_not_promised() {
        assert_eq!(parse_cost_cents(r#"{"total_cost_usd":1.005}"#), Some(100));
    }

    /// A line we don't understand is liveness, not a crash. The stream format
    /// is not a contract we control.
    #[test]
    fn unknown_lines_are_not_errors() {
        assert_eq!(parse_cost_cents("not json at all"), None);
        assert_eq!(parse_cost_cents(r#"{"type":"assistant","message":{}}"#), None);
        assert_eq!(parse_cost_cents(""), None);
    }

    #[test]
    fn briefings_quote_safely_for_bash() {
        let nasty = "it's \"quoted\"; rm -rf /";
        let q = shell_quote(nasty);
        assert!(q.starts_with('\'') && q.ends_with('\''));
        assert!(q.contains(r"'\''"), "single quotes must be escaped: {q}");
    }

    /// Re-running a card must resume its branch, not start over. Always
    /// branching from base meant the push was rejected as non-fast-forward
    /// against the card's own earlier work — which is exactly the
    /// "changes requested, go fix it" path.
    #[test]
    fn clone_resumes_an_existing_branch() {
        let cfg = repo_cfg();
        let s = clone_script(&cfg, "honr/card-8");
        assert!(s.contains("ls-remote --exit-code --heads origin honr/card-8"), "{s}");
        assert!(s.contains("checkout -q -B honr/card-8 origin/honr/card-8"), "{s}");
        assert!(s.contains("rebase -q upstream/main"), "{s}");
        // A conflict is the agent's problem to resolve, so the supervisor must
        // back out rather than leave a half-applied rebase behind.
        assert!(s.contains("rebase --abort"), "{s}");
        // Shallow history cannot be rebased reliably.
        assert!(!s.contains("--depth"), "clone must not be shallow: {s}");
    }

    /// Nothing syncs the fork, so its own base freezes at the moment it was
    /// created while upstream moves on — 6 commits, within a day, in practice.
    /// Rebasing onto the fork's base would tell the agent it was current when
    /// it was not, and produce a PR that conflicts with what it targets.
    #[test]
    fn base_comes_from_upstream_not_the_fork() {
        let s = clone_script(&repo_cfg(), "honr/card-8");
        assert!(s.contains("git remote add upstream https://github.com/shanemcd/honr.git"), "{s}");
        assert!(s.contains("fetch -q upstream main"), "{s}");
        assert!(s.contains("rebase -q upstream/main"), "{s}");
        // A brand-new card must also start from upstream, not the stale fork.
        assert!(s.contains("checkout -q -B honr/card-8 upstream/main"), "{s}");
        assert!(!s.contains("rebase -q origin/main"), "must not rebase onto the fork: {s}");
    }

    /// The supervisor asks GitHub a question; it does not create anything.
    /// Every publish failure today came from our shell being wrong about a
    /// tool the agent already knows how to drive.
    #[test]
    fn the_supervisor_only_looks_up_the_pr() {
        let s = pr_lookup_script(&repo_cfg(), "honr/card-8");
        assert!(s.contains("gh pr list"), "{s}");
        assert!(s.contains("--head honr/card-8"), "pr list wants a bare branch: {s}");
        assert!(s.contains(PR_URL_MARK), "url must come from a marked line: {s}");
        assert!(!s.contains("gh pr create"), "creating is the agent's job now: {s}");
        assert!(!s.contains("push"), "pushing is the agent's job now: {s}");
    }

    /// If the agent did not open a PR, the card must not reach Review looking
    /// finished — a Review card you cannot open is not a review.
    #[test]
    fn no_pr_means_no_url_to_report() {
        let s = pr_lookup_script(&repo_cfg(), "honr/card-8");
        assert!(s.contains("// empty"), "must yield nothing rather than error: {s}");
    }

    /// Publishing moved into the agent's job, so the briefing is now the only
    /// place that says how. If it stops saying it, nothing pushes at all.
    #[test]
    fn the_briefing_tells_the_agent_to_publish() {
        let b = briefing(&grant(), BranchState::Fresh, "honr/card-7", "shanemcd/honr", "main");
        assert!(b.contains("honr/card-7"), "must name the branch: {b}");
        assert!(b.contains("shanemcd/honr"), "must name the PR target: {b}");
        assert!(b.to_lowercase().contains("push"), "{b}");
        assert!(b.to_lowercase().contains("pull request"), "{b}");
        assert!(b.contains("cargo test --offline --locked"), "gates must be named: {b}");
    }

    #[test]
    fn branch_state_is_read_from_the_clone_output() {
        assert_eq!(branch_state_of("HONR-BRANCH-FRESH\n"), BranchState::Fresh);
        assert_eq!(branch_state_of("noise\nHONR-BRANCH-REBASED\n"), BranchState::Rebased);
        assert_eq!(branch_state_of("HONR-BRANCH-CONFLICT\n"), BranchState::Conflicted);
        // Unrecognised output must not silently claim a clean rebase.
        assert_eq!(branch_state_of("something else"), BranchState::Fresh);
    }

    /// An agent resuming a conflicted branch has to be told, or it will build
    /// on top of a branch that cannot merge.
    #[test]
    fn the_briefing_tells_the_agent_about_a_conflict() {
        let conflicted = briefing(&grant(), BranchState::Conflicted, "honr/card-7", "shanemcd/honr", "main");
        assert!(conflicted.contains("CONFLICTS"), "{conflicted}");
        assert!(conflicted.to_lowercase().contains("resolve"), "{conflicted}");

        let fresh = briefing(&grant(), BranchState::Fresh, "honr/card-7", "shanemcd/honr", "main");
        assert!(!fresh.contains("CONFLICTS"));
        assert!(fresh.contains("new branch"));
    }

    /// Changes-requested notes are the whole steering mechanism: they reach the
    /// next run only by way of the briefing.
    #[test]
    fn steering_notes_reach_the_briefing() {
        let mut g = grant();
        g.notes = vec!["Changes requested: rebase onto latest, api.rs only.".into()];
        let b = briefing(&g, BranchState::Rebased, "honr/card-7", "shanemcd/honr", "main");
        assert!(b.contains("rebase onto latest, api.rs only."), "{b}");
    }

    // ---- surviving a restart ------------------------------------------

    /// The agent must not be a child of the exec that starts it. As a child it
    /// died whenever honr did, which made every rebuild throw away a live run
    /// and left deleting the sandbox as the only honest option.
    #[test]
    fn the_agent_outlives_the_exec_that_starts_it() {
        let s = start_script(&repo_cfg(), "do the thing");
        assert!(s.contains("setsid nohup"), "must be detached: {s}");
        assert!(s.trim_end().contains("&\n") || s.contains("2>&1 &"), "must background it: {s}");
        // The three files are the whole contract with whoever watches next.
        assert!(s.contains(AGENT_LOG) && s.contains(AGENT_PID) && s.contains(AGENT_STATUS), "{s}");
        // Starting must return once the run is up, not hold the exec open.
        assert!(s.contains("exit 0"), "must return as soon as the pid lands: {s}");
    }

    /// The deadline has to live inside the sandbox. Once the agent is detached
    /// nothing on this side owns the process, and an agent nobody is watching
    /// still spends money.
    ///
    /// `--foreground` is not cosmetic: without it `timeout` moves the command
    /// into its own process group, and `stop_agent` then signals a group the
    /// agent is not in.
    #[test]
    fn the_agent_carries_its_own_deadline() {
        let mut cfg = repo_cfg();
        cfg.agent_timeout_secs = 900;
        let s = start_script(&cfg, "b");
        assert!(s.contains("timeout --foreground 900 claude"), "{s}");
    }

    /// The briefing is quoted once, for the outer shell, and reaches the inner
    /// shell as an environment variable. Interpolating it into a second layer
    /// of single quotes breaks on the first card description with an
    /// apostrophe in it — which is most of them.
    #[test]
    fn the_briefing_crosses_the_inner_shell_intact() {
        let s = start_script(&repo_cfg(), "it's a card; rm -rf /");
        assert!(s.contains(r"it'\''s a card; rm -rf /"), "must be escaped once: {s}");
        assert!(s.contains(r#"claude -p "$HONR_BRIEFING""#), "inner shell reads the var: {s}");
    }

    /// Following is a *reader*. It can start part-way through, which is what
    /// lets a restarted honr take over a run instead of killing it.
    #[test]
    fn following_can_start_part_way_through() {
        let s = follow_script(118);
        assert!(s.contains("tail -n +118"), "{s}");
        assert!(s.contains("--pid="), "must stop when the agent does: {s}");
        assert!(s.contains(AGENT_STATUS), "must exit with the agent's own code: {s}");
        assert!(!s.contains("claude"), "following must not start anything: {s}");
    }

    /// A run can finish while honr is down. Waiting on a pid that is already
    /// gone would hang, so the finished case is handled before the wait.
    #[test]
    fn a_finished_run_is_not_waited_on() {
        let s = follow_script(1);
        let wait = s.find("--pid=").expect("waits somewhere");
        let done = s.find(&format!("if [ -f {AGENT_STATUS} ]")).expect("checks for the status");
        assert!(done < wait, "the already-finished case must come first: {s}");
    }

    /// The card decides what happens to a sandbox, not the sandbox.
    #[test]
    fn only_the_cards_own_live_sandbox_is_adopted() {
        let mut item = WorkItem::new(9, "t", "i");
        item.state = State::Running;
        item.environment = Some("honr-card-9-a2".into());
        assert!(adoptable(Some(&item), "honr-card-9-a2").is_some());

        // The previous attempt's sandbox is kept for inspection and carries the
        // same `honr.item` label. Adopting it would attach to a dead log while
        // the real run went unwatched.
        assert!(adoptable(Some(&item), "honr-card-9-a1").is_none(), "reap the old attempt");

        // Not running: whatever is out there is debris.
        item.state = State::Review;
        assert!(adoptable(Some(&item), "honr-card-9-a2").is_none(), "reap a finished card's box");

        // A sandbox for a card that no longer exists.
        assert!(adoptable(None, "honr-card-9-a2").is_none());
    }

    /// Where to resume, and what has already been paid for.
    ///
    /// The stream reports a *cumulative* total and the supervisor charges the
    /// difference, so a fresh process that started its arithmetic at zero would
    /// bill the whole run again on the very next cost line.
    #[test]
    fn a_probe_says_where_to_resume_and_what_was_already_charged() {
        let out = format!(
            "{MARK_ALIVE}\n{MARK_LINES}117\n{MARK_COST}{{\"total_cost_usd\":0.88}}\n"
        );
        assert_eq!(probe_of(&out), Some((118, 88)));

        // A run that finished while honr was down still has a PR to record.
        let done = format!("{MARK_EXITED}\n{MARK_LINES}4\n{MARK_COST}\n");
        assert_eq!(probe_of(&done), Some((5, 0)));

        // Nothing running means there is nothing to adopt — the card goes back
        // in the queue rather than being watched forever.
        let gone = format!("{MARK_GONE}\n{MARK_LINES}0\n{MARK_COST}\n");
        assert_eq!(probe_of(&gone), None);
    }

    /// The other way to lose a live run to a restart.
    ///
    /// Reconciliation used to no-op when the gateway could not answer, and the
    /// sweeper started regardless — so a podman machine that was merely slow to
    /// come up got a still-running card requeued and a second agent dispatched
    /// onto its branch. `false` stands in for a gateway that is not there.
    #[tokio::test]
    async fn startup_waits_for_a_gateway_that_is_not_up_yet() {
        let os = OpenShell::new("false", Duration::from_secs(5));
        let board = test_board();
        let grace = Duration::from_millis(300);

        let began = std::time::Instant::now();
        let adopted = reconcile_once_reachable(&os, &board, 600, grace).await;

        assert!(adopted.is_empty(), "nothing can be adopted through a dead gateway");
        assert!(began.elapsed() >= grace, "must wait for the gateway, not skip past it");
    }

    /// The wait is bounded on purpose. A gateway that is never coming back must
    /// not hold the sweeper — and therefore every Running card — forever.
    #[tokio::test]
    async fn a_gateway_that_never_answers_does_not_freeze_the_board() {
        let os = OpenShell::new("false", Duration::from_secs(5));
        let began = std::time::Instant::now();
        reconcile_once_reachable(&os, &test_board(), 600, Duration::from_millis(50)).await;
        assert!(began.elapsed() < Duration::from_secs(30), "gave up in bounded time");
    }

    /// Never flushed, so the path is only ever a name.
    fn test_board() -> SharedBoard {
        Arc::new(crate::store::Board::new(
            crate::schema::Schema::default(),
            std::env::temp_dir().join("honr-test-reconcile.json"),
        ))
    }

    /// Cross-fork PRs need `owner:branch` as the head, or gh silently looks for
    /// the branch on upstream and fails.
    fn repo_cfg() -> AgentConfig {
        let mut cfg = AgentConfig::default();
        cfg.repo.upstream = "shanemcd/honr".into();
        cfg.repo.fork = "clankrshq/honr".into();
        cfg.repo.base = "main".into();
        cfg
    }

    fn grant() -> ClaimGrant {
        ClaimGrant {
            item_id: 7,
            title: "t".into(),
            definition_of_done: None,
            ancestry: vec![],
            constraints: vec![],
            notes: vec![],
            lease_expires_at: chrono::Utc::now(),
            budget_remaining_cents: None,
        }
    }
}
