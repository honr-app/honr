# State of play

Where honr actually is, as of the end of the session that made it self-hosting.
Read this before changing anything.

## What is proven

**honr built honr.** Card #8 (`GET /api/version`) was dispatched by honr's
supervisor to a Claude Code agent in an OpenShell sandbox. The agent wrote the
endpoint and a test, pushed to a fork, and opened
[PR #1](https://github.com/shanemcd/honr/pull/1), which is merged as `bf20f8c`.
The test it wrote now runs in the suite that gates its own future work.

One successful run: 5 minutes, $1.22, zero failures. The endpoint answers:

```
$ curl localhost:8080/api/version
{"version":"0.1.0"}
```

Also proven, and worth more than the endpoint: on an earlier run the agent hit
a **seven-commit rebase conflict** and resolved it unaided, first try. The
supervisor detected the conflict, aborted its own attempt, and handed it over
with instructions — which is the intended division of labour.

## What is not proven

**CI is the mechanical gate, not a board column.** A card reaches Review when
the agent opens a PR; GitHub Actions (or equivalent) is what must pass. The old
Verify column is gone — it never ran real checks.

**One card, one success.** The loop has run cleanly end to end exactly once.
Treat operational confidence accordingly.

**Nothing has run unattended.** Every run so far was watched.

## The board

Work is **Project + Plan artifact + flat Tasks**. Vision / Epic / Story are
retired. A Project is the only container (Board swimlane, never a Backlog card).
Creating a Project seeds an **Initial plan** Task. `propose_breakdown` writes a
**Plan artifact** (task keys, deps, DoDs) on the Project; **Approve Plan**
(`approve_plan`) materializes sibling Tasks onto the Board into **Backlog**.
Nothing auto-starts — cockpit must **dispatch** (MCP or UI Start) each card.
Tasks relate by beads dependency edges (`blocks` / `relates-to`), not nested
hierarchy. Upon Plan approval, tasks with dependency constraints materialize
as a DAG (e.g. A→B, A→C, B+C→D). Friendly plain-language blocker chips appear on cards,
and the Board visual graph (`npm --prefix web run shots` / `web/shots/desktop-graph.png`)
renders the topological dependency DAG step by step.

`honr.json` still holds the rich lifecycle + runtime fields (including the Plan
artifact); `.beads/` holds identity and the graph (Plan A dual-write). A fresh
clone starts empty.

Example shape (historical card numbers; recreate via the cockpit):

```
Phase 2 — real agents                                   (Project + Plan)
  done   Initial plan                                   (Task — seed)
  done   Open the sandbox policy for the toolchain      (Task)
  done   First self-hosted card: GET /api/version       (Task) ← merged
         Re-adopt live sandboxes on restart
         Verdict file protocol
         Split from inside the sandbox                  → sibling Tasks, not nest
         Sandbox name and PR link on the card
         Report the real diffstat
         Observe cost during the run, not at the end
```

Tasks from a Plan stay off Backlog until **Approve Plan** on their Project.
**Dispatch** is the call that puts execution agents to work. The Project itself
never moves to Backlog.

Constraints pinned on the Project, inherited by every Task:

- Merging is a human action. Approving in honr surfaces the PR; it never merges it.
- Agents may not weaken `machine.rs` invariants, supervisor budget enforcement,
  or `sandbox/policy.yaml`. If a card seems to require it, escalate instead.
- Everything in the sandbox stack fails as a hang, not an error. Every exec
  needs a deadline; treat silence as failure.
## Known gaps, roughly by value

**Capability routing is dead.** Enqueue does not filter by capability today;
`dispatch_loop` claims any awaiting card that passes `may_claim`. Also, the
`claimable` flag in `honr.yaml` is decorative — `list_backlog` never consults
the level schema, so leaves land at whatever depth they're created.

**The daily budget resets on restart.** `SPENT_TODAY` is a process static, and
honr restarts constantly while honr is what's being built. It's a runaway
backstop, not a spend cap.

**Cost only arrives at the end.** Claude Code emits `total_cost_usd` in its
final message, so the per-card cap cannot interrupt a run — it only notices
afterwards (#18). Card cost is also cumulative across attempts while the cap is
per-attempt.

**Diffstat is zeros** (#17). The supervisor never diffs the branch, so Review
sorts by a blast radius it doesn't know.

**"What changed" is capped at 3 lines** with no expander. For a goal running
for days that's a keyhole.

**No auth.** Anything that can reach :8080 can drive the board.

## Hard-won lessons

Nine failures produced one successful run. Eight were scaffolding, not the
agent. The pattern is worth internalising before writing more of it:

**Don't script what the agent can drive.** The supervisor originally pushed and
opened PRs itself. Four separate failures came from that shell being wrong
about tools the agent already knew: `upload` takes a destination *directory*;
`gh pr create` errors when a PR exists; the URL was scraped from stdout;
`--force-with-lease` refuses against an ad-hoc URL. Publishing now belongs to
the agent, and the supervisor only *asks GitHub what happened*. A query keeps
working when tool output changes.

**Everything fails as a hang.** Blocked egress, a missing credential helper, a
wedged relay — none of them error. Every exec carries a deadline and silence
counts as failure.

**Liveness must be observed, not asserted.** Heartbeats come from the agent's
output stream, so a hung agent cannot claim to be fine. The corollary bit us:
a heartbeat is a side effect of *output*, not of work, and a silent
`cargo build` runs ~30s. `lease_secs` must exceed the longest legitimate
silence — it's 600 now, was 45, and at 45 the sweeper requeued a live card and
two agents raced on one branch.

**Separate infrastructure failure from card failure.** The podman machine
stopped on its own three times in one session. Those outages consumed a card's
retry budget and it escalated claiming it had "failed to run 3 times without
producing any work" — about a card that never got to run.

**The board lying is worse than the board erroring.** An answered escalation
stayed attached to its card, so a running card reported `WAITING ON YOU,
blocked 15m` against a resolved question. Separately, the UI kept rendering its
last good snapshot when polls failed, with no signal. Both are fixed; the
principle is that a control plane silently showing history is worse than one
showing an error.

## If you are picking this up

1. Read [`docs/operating.md`](operating.md) — how to run it and what breaks.
2. Read [`docs/sandbox-stack.md`](sandbox-stack.md) before touching anything
   under `sandbox/` or the supervisor's exec scripts.
3. `cargo test` (48, all hermetic) and `cargo clippy --all-targets -- -D warnings`
   should both be clean before and after any change.
4. `npm --prefix web run shots` renders the UI to `web/shots/` so you can look
   at it rather than reason about it.
5. The highest-value next card is **#10, supervisor-run gates**. It converts
   "I read every diff" into "I read the interesting ones".
