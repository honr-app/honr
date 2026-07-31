//! The execution side of the board.
//!
//! An agent is **material, not a participant in the control plane**. It gets no
//! network path to honr; the supervisor calls `claim`/`heartbeat`/`report` on
//! its behalf. An agent that could reach honr's MCP could approve its own
//! review. (OpenShell only forwards host→sandbox anyway, which independently
//! forces this shape.)
//!
//! Two properties are load-bearing:
//!
//! - **Liveness and cost are observed, never self-reported.** Both come from
//!   parsing the agent's `stream-json` as it arrives, so a hung agent cannot
//!   claim to be fine and a chatty one cannot under-report spend.
//! - **Everything fails as a hang.** Every exec carries a deadline, and silence
//!   is treated as failure rather than patience.

use crate::model::{ItemId, State};
use crate::openshell::{OpenShell, SandboxSpec, LABEL_ITEM};
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

pub fn spawn(board: SharedBoard, cfg: ExecutionConfig) {
    tokio::spawn(sweeper_loop(board.clone(), cfg.clone()));

    if !cfg.agents.enabled {
        tracing::info!("execution.agents.enabled = false; board runs with no executor");
        return;
    }
    if let Err(e) = cfg.agents.validate() {
        tracing::error!("agents enabled but misconfigured: {e}");
        return;
    }
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

async fn dispatch_loop(board: SharedBoard, cfg: ExecutionConfig) {
    let os = Arc::new(OpenShell::default());
    let agents = Arc::new(cfg.agents.clone());
    let in_flight = Arc::new(AtomicU64::new(0));

    reap_orphans(&os, &board).await;

    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tick.tick().await;

        if in_flight.load(Ordering::Relaxed) as usize >= agents.max_concurrent {
            continue;
        }
        if SPENT_TODAY.load(Ordering::Relaxed) >= agents.daily_budget_cents {
            continue;
        }
        // The podman machine stops on its own. Claiming a card we can't run
        // would strand it until the lease lapsed.
        if !os.healthy().await {
            tracing::warn!("openshell gateway unhealthy; not claiming");
            continue;
        }

        let ready = board.list_ready(&["any".to_string()]);
        let Some(item) = ready.into_iter().next() else { continue };

        let agent_id = format!("sandbox-{}", item.id);
        let grant = match board.claim(item.id, &agent_id, Some(agents.vertex.model.clone()), cfg.lease_secs) {
            Ok(g) => g,
            Err(e) => {
                tracing::debug!("claim of #{} refused: {e}", item.id);
                continue;
            }
        };

        in_flight.fetch_add(1, Ordering::Relaxed);
        let (board, os, agents, in_flight2) =
            (board.clone(), os.clone(), agents.clone(), in_flight.clone());
        let lease = cfg.lease_secs;
        tokio::spawn(async move {
            let id = grant.item_id;
            match run_card(&board, &os, &agents, &agent_id, grant, lease).await {
                Ok(()) => board.clear_run_failures(id),
                Err(e) => {
                    tracing::error!("#{id} failed: {e}");
                    // Count it. A run that dies early spends nothing, so no
                    // money cap stops the sweeper requeueing it forever —
                    // after `max_attempts` this becomes a human's problem
                    // instead of an overnight loop.
                    if let Err(e2) = board.record_run_failure(id, &e.to_string(), agents.max_attempts)
                    {
                        tracing::error!("#{id}: could not record failure: {e2}");
                    }
                }
            }
            in_flight2.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// A restarted honr must find sandboxes it started before the restart. honr is
/// rebuilt constantly while honr is what's being built, so without this every
/// `cargo run` orphans a live sandbox and leaks real money.
async fn reap_orphans(os: &OpenShell, board: &SharedBoard) {
    let Ok(ours) = os.list_ours().await else {
        tracing::warn!("could not list sandboxes; skipping reap");
        return;
    };
    for sb in ours {
        let Some(id) = sb.item_id() else { continue };
        let still_running =
            board.get(id).map(|i| matches!(i.state, State::Claimed | State::Running)).unwrap_or(false);
        if still_running {
            // The card is mid-flight but the task driving it died with the old
            // process. Nothing is watching the sandbox, so the honest move is
            // to drop it and let the lease requeue the card.
            tracing::warn!("#{id}: sandbox {} outlived its supervisor; deleting", sb.name);
        } else {
            tracing::info!("reaping orphaned sandbox {}", sb.name);
        }
        let _ = os.delete(&sb.name).await;
        board.set_environment(id, None);
    }
}

// ----------------------------------------------------------- the lifecycle

async fn run_card(
    board: &SharedBoard,
    os: &OpenShell,
    cfg: &AgentConfig,
    agent_id: &str,
    grant: ClaimGrant,
    lease_secs: i64,
) -> anyhow::Result<()> {
    let id = grant.item_id;
    // Attempt-scoped, because a failed sandbox is *kept* for inspection and a
    // retry would otherwise collide with its name. The `honr.item` label is
    // what reconciliation matches on, so it stays stable across attempts.
    let attempt = board.get(id).map(|i| i.run_failures).unwrap_or(0) + 1;
    let name = format!("honr-card-{id}-a{attempt}");
    let branch = format!("honr/card-{id}");

    // Recorded before creation so a crash between here and `create` still
    // leaves a name to reconcile against.
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

    let result = run_inside(board, os, cfg, agent_id, &grant, &name, &branch, lease_secs, &spec).await;

    // Keep the sandbox on failure: `openshell logs` is the tool that actually
    // answers questions, and a deleted sandbox answers none.
    match &result {
        Ok(_) => {
            let _ = os.delete(&name).await;
            board.set_environment(id, None);
        }
        Err(e) => tracing::error!("#{id}: keeping sandbox {name} for inspection: {e}"),
    }
    result
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

    os.create(spec).await?;

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
    anyhow::ensure!(up.ok(), "metadata shim never came up: {}", up.stderr.trim());

    // Clone the fork. GIT_TERMINAL_PROMPT=0 is not optional: without it a
    // missing credential blocks forever on an interactive username prompt,
    // which looks exactly like a slow clone.
    let clone = os.exec(name, &clone_script(cfg, branch), short).await?;
    anyhow::ensure!(clone.ok(), "clone failed: {}", clone.stderr.trim());

    // ---- the agent -------------------------------------------------------

    let budget = cfg.per_card_budget_cents;
    let spent = Arc::new(AtomicU64::new(0));
    let briefing = briefing(grant);

    let script = format!(
        "cd {WORKDIR} && claude -p {} --output-format stream-json --verbose --permission-mode \
         bypassPermissions 2>&1",
        shell_quote(&briefing)
    );

    let started = std::time::Instant::now();
    let timeout = Duration::from_secs(cfg.agent_timeout_secs);
    let (board2, spent2) = (board.clone(), spent.clone());
    let agent_owned = agent_id.to_string();

    let run = os
        .exec_streaming(name, &script, timeout, move |line| {
            // Progress is not knowable from the stream, so it is reported as
            // elapsed-against-deadline and capped below 1.0 — honest about
            // being an estimate, and monotonic, which is what the card face
            // needs. Only `report` sets 1.0.
            let frac = started.elapsed().as_secs_f32() / timeout.as_secs_f32();
            let progress = frac.clamp(0.0, 0.95);

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
            let _ = board2.heartbeat(grant.item_id, &agent_owned, progress, delta, lease_secs);
        })
        .await?;

    let final_cents = spent.load(Ordering::Relaxed);
    if final_cents > budget {
        anyhow::bail!("per-card budget breached: {final_cents}c > {budget}c");
    }
    anyhow::ensure!(run.ok(), "agent exited {}: {}", run.code, tail(&run.stdout, 400));

    // ---- publish ---------------------------------------------------------
    //
    // The supervisor pushes and opens the PR, not the agent. Deterministic,
    // and it keeps `gh` out of the agent's hands.

    let push = os.exec(name, &push_script(cfg, branch, grant.item_id), short).await?;
    anyhow::ensure!(push.ok(), "push failed: {}", tail(&push.stdout, 400));

    let pr = os.exec(name, &pr_script(cfg, branch, grant), short).await?;
    anyhow::ensure!(pr.ok(), "pr create failed: {}", tail(&pr.stdout, 400));
    let url = pr.stdout.lines().rev().find(|l| l.starts_with("http")).map(str::to_string);
    board.set_pr_url(id, url.clone());

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
    tracing::info!("#{id} reported; pr={}", url.unwrap_or_else(|| "none".into()));
    Ok(())
}

// --------------------------------------------------------------- scripts

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

fn clone_script(cfg: &AgentConfig, branch: &str) -> String {
    let fork = &cfg.repo.fork;
    let base = &cfg.repo.base;
    format!(
        r#"set -e
export GIT_TERMINAL_PROMPT=0
rm -rf {WORKDIR}
git -c '{GIT_CRED}' clone -q --depth 50 --branch {base} https://github.com/{fork}.git {WORKDIR}
cd {WORKDIR}
git checkout -q -b {branch}
git config user.email "agent@honr.local"
git config user.name "honr agent""#
    )
}

fn push_script(cfg: &AgentConfig, branch: &str, id: ItemId) -> String {
    let fork = &cfg.repo.fork;
    format!(
        r#"set -e
export GIT_TERMINAL_PROMPT=0
cd {WORKDIR}
if [ -n "$(git status --porcelain)" ]; then
  git add -A
  git commit -q -m "honr card #{id}"
fi
git -c '{GIT_CRED}' push -q --set-upstream https://github.com/{fork}.git {branch}"#
    )
}

fn pr_script(cfg: &AgentConfig, branch: &str, grant: &ClaimGrant) -> String {
    let upstream = &cfg.repo.upstream;
    let base = &cfg.repo.base;
    // Cross-fork head is `owner:branch`.
    let fork_owner = cfg.repo.fork.split('/').next().unwrap_or_default();
    let title = format!("{} (honr #{})", grant.title, grant.item_id);
    let body = format!(
        "Opened by a honr agent for card #{}.\n\n**Intent:** {}\n\n**Definition of done:** {}\n",
        grant.item_id,
        grant.ancestry.last().map(|a| a.intent.as_str()).unwrap_or(""),
        grant.definition_of_done.as_deref().unwrap_or("(none)"),
    );
    format!(
        r#"set -e
cd {WORKDIR}
GH_TOKEN=$GITHUB_TOKEN gh pr create --repo {upstream} --base {base} \
  --head {fork_owner}:{branch} --title {} --body {}"#,
        shell_quote(&title),
        shell_quote(&body)
    )
}

/// The intent chain is the highest-leverage payload in the system, and a fresh
/// `claude -p` in an empty container has none of it.
fn briefing(grant: &ClaimGrant) -> String {
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

    b.push_str(
        "\nYou are in a git clone on a branch of your own. Make the change and leave the tree \
         clean; the supervisor commits, pushes and opens the PR. Run the project's own checks \
         before you finish — `cargo test --offline --locked` and `cargo clippy --offline -- -D \
         warnings`, both of which work with no network. Do not push, do not open a PR, and do \
         not touch git remotes.\n",
    );
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

    /// Cross-fork PRs need `owner:branch` as the head, or gh silently looks for
    /// the branch on upstream and fails.
    #[test]
    fn pr_head_is_namespaced_to_the_fork() {
        let mut cfg = AgentConfig::default();
        cfg.repo.upstream = "shanemcd/honr".into();
        cfg.repo.fork = "clankrshq/honr".into();
        let grant = ClaimGrant {
            item_id: 7,
            title: "t".into(),
            definition_of_done: None,
            ancestry: vec![],
            constraints: vec![],
            notes: vec![],
            lease_expires_at: chrono::Utc::now(),
            budget_remaining_cents: None,
        };
        let s = pr_script(&cfg, "honr/card-7", &grant);
        assert!(s.contains("--head clankrshq:honr/card-7"), "{s}");
        assert!(s.contains("--repo shanemcd/honr"), "{s}");
    }
}
