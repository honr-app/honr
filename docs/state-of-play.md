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

**Nothing verifies an agent's work.** Gates are recorded as
`agent-reported`; the supervisor never runs `cargo test`. A card reaches Review
on the agent's own claim, so the real gate is a human reading the diff. This is
the highest-value gap on the board.

**One card, one success.** The loop has run cleanly end to end exactly once.
Treat operational confidence accordingly.

**Nothing has run unattended.** Every run so far was watched.

## The board

`honr.json` is gitignored, so a fresh clone starts empty. The tree below was
built through the cockpit (`create_goal` → `propose_breakdown` → `approve_plan`)
and is reproducible the same way.

```
#1  honr builds honr                                    (vision)
  #2  Phase 2 — real agents                             (project)
    #3  Prove the loop
      #7  done   Open the sandbox policy for the toolchain
      #8  done   First self-hosted card: GET /api/version   ← merged
      #9         Re-adopt live sandboxes on restart
    #4  Verification the agent cannot influence
      #10        Supervisor runs the gates                  ← highest value
      #11        Verify from a clean checkout
    #5  Agent-initiated decisions
      #12        Verdict file protocol
      #13        Split from inside the sandbox
    #6  The board shows the run
      #14        Sandbox name and PR link on the card       ← largely done by hand
    #15 Report the run honestly
      #16        Query the PR URL instead of scraping it    ← done by hand
      #17        Report the real diffstat
      #18        Observe cost during the run, not at the end
      #19        Make gh pr create idempotent across retries ← done by hand
```

Everything except #7 and #8 is in **shaping** — proposed but not approved, so
nothing is claimable and honr is idle by design. `approve_plan` on an epic is
the single call that puts agents back to work.

Constraints pinned on the vision, inherited by every card:

- Merging is a human action. Approving in honr surfaces the PR; it never merges it.
- Agents may not weaken `machine.rs` invariants, supervisor budget enforcement,
  or `sandbox/policy.yaml`. If a card seems to require it, escalate instead.
- Everything in the sandbox stack fails as a hang, not an error. Every exec
  needs a deadline; treat silence as failure.
- Gates run with `--offline`. A cache miss must fail loudly rather than reach
  the network.

## Known gaps, roughly by value

**Gates are self-reported** (#10, #11). Until the supervisor runs the checks
itself from a checkout the agent cannot influence, Review means "an agent says
it's fine."

**Capability routing is dead.** `dispatch_loop` hardcodes `["any"]`, so a card
tagged `writer` is silently never claimed. It just sits in Ready looking
healthy. Also, the `claimable` flag in `honr.yaml` is decorative — `list_ready`
never consults the level schema, so leaves land at whatever depth they're
created and are claimable regardless.

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
3. `cargo test` (46, all hermetic) and `cargo clippy --all-targets -- -D warnings`
   should both be clean before and after any change.
4. `npm --prefix web run shots` renders the UI to `web/shots/` so you can look
   at it rather than reason about it.
5. The highest-value next card is **#10, supervisor-run gates**. It converts
   "I read every diff" into "I read the interesting ones".
