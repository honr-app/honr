# Operating honr

## Just the board

```bash
cargo run           # :8080 — API, SSE, MCP, and web/dist if built
```

No podman, no gateway, no credentials needed. Agents are off by default.
`HONR_PORT` overrides the port. State is `honr.json` in the working directory.

### Local GitHub webhooks (`gh webhook forward`)

Ingress is `POST /api/webhooks/github`. A push to the default branch emits
`MainAdvanced`. That event does three distinct things:

1. **Merged card → Done** — when a Review/NeedsHuman card's `pr_url` matches the
   merged PR, the card moves to Done (beads close + `github_push` closes the
   linked Issue).
2. **Review rebase catch-up** — sibling PRs still in Review that are behind
   `main` get a rebase dispatch. The supervisor rebases those PR branches
   against updated `main`. Clean rebase stays in Review; a true conflict
   returns the card to Backlog with conflict context. This path is
   supervisor-driven git on the PR branch — not an agent turn.
3. **Live runs (Claimed / Running)** — each live card gets a steer note naming
   the advanced ref (and commit sha when present) and instructing the agent to
   fetch upstream main, rebase the card branch onto it, then continue. Because
   steer alone does not inject mid-turn, honr then **parks** (reason: main
   advanced) and **unparks** so the supervisor re-claims with a resume briefing
   that includes the note. Sandbox and `conversation_id` are preserved. Cards
   already parked are steered only (no second park/unpark). The supervisor does
   **not** git-rebase the live worktree itself — the agent does that on resume.

Operators will see Running cards pause briefly and come back with a
fetch/rebase instruction whenever someone pushes to `main`. That is expected,
not a wedged run.

```bash
gh extension install cli/gh-webhook   # once

gh webhook forward \
  --repo=shanemcd/honr \
  --events=pull_request,push \
  --url=http://127.0.0.1:8080/api/webhooks/github
```

Leave that running while you merge a test PR. Only one forwarder per repo at a
time. Dev-only — not for production delivery.

## Running real agents

This spends real money and opens pull requests. Four things must be true.

**1. The compute driver is up.**

```bash
podman machine start
docker info            # the socket is a symlink to podman's
```

It stops on its own — three times in one session, historically. The supervisor
health-checks before claiming and pauses 60s after an infrastructure failure
rather than burning a card's retry budget, but it cannot prevent the outage.

**2. The gateway is up.**

```bash
brew services start openshell
openshell status       # expect Connected + Authenticated (mTLS transport)
```

Port 17670, deliberately not 8080.

**3. Providers exist.**

```bash
openshell provider list        # expect a Vertex provider and a GitHub one
```

See [`sandbox-stack.md`](sandbox-stack.md) for how to create them. The GitHub
credential key **must** be named `GITHUB_TOKEN` or `GH_TOKEN` — the profile
matches on the name.

**4. The image is built.**

```bash
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

From the repo root, not `sandbox/` — `Cargo.lock` and `web/package-lock.json`
must be in context. Rebuild when `Cargo.lock` changes materially. ~3.7GB.

Then flip `execution.agents.enabled: true` in `honr.yaml` and **restart** —
config is read once at startup, there is no hot reload and no runtime toggle.

> Put it back to `false` before committing. It has been committed as `true`
> once already, swept in by `git add -A`, which would have made a fresh clone
> spend money on startup.

## Sandbox profile resolution

When the supervisor creates an OpenShell sandbox for a card, create knobs
(`--from` image, policy, cpu, memory) resolve in this order:

1. **Project override** — `sandbox_profile_id` on the containing Project, if set
   and present in the board profile catalog
2. **Global default** — `default_sandbox_profile_id` on durable board state
3. **YAML fallback** — `execution.agents` `image` / `policy` / `cpu` / `memory`
   in `honr.yaml` (also used to seed the catalog when it is empty at load)

Profiles are managed via Settings (REST: `/api/sandbox-profiles`). Process
knobs (auth, repo, engine, budgets, concurrency) stay in YAML and are not
part of a profile.

## What dispatch decides, and when

Cockpit decides what starts. A Backlog card is inert until someone calls
`dispatch` (MCP tool or UI **Start**), which sets `awaiting_dispatch`.

`dispatch_loop` polls every 3 seconds and passes four gates in order: in-flight
below `max_concurrent`, spend below the daily ceiling, not in an infrastructure
cooldown, gateway healthy. Then it takes the **oldest** Backlog card with
`awaiting_dispatch` that is claimable and not already being run by this process,
and claims it.

A card is eligible for enqueue when it is `Backlog`, not parked, unblocked,
has a definition of done, and is not parked. Lease expiry,
park, halt, release, and request_changes all clear `awaiting_dispatch` — cockpit
must dispatch again. Unpark clears the hold and queues the supervisor (same as Start).

**Approve Plan** materializes Tasks into Backlog; the Project itself never goes
to Backlog. Approve Plan does not auto-dispatch.

## Steering a card

| You want to | Do this |
|---|---|
| Send a reviewed card back with instructions | **Request changes** in the drawer. The note reaches the next run's briefing. |
| Answer a blocked agent | **Needs you** — pick an option. Resets the card's retry budget. |
| Stop a wedged run but keep context | **Park** — stops the agent, keeps sandbox + agy conversation, and **holds** the card until **Resume session** / `unpark`. |
| Resume a parked card | **Resume** / `unpark` — clears the hold and queues the supervisor; next claim uses `--conversation` when an id is still on the card. |
| Throw away the LLM session | **Halt** — stops the agent and clears `conversation_id`; sandbox may still be reused for caches. |
| Anything requiring a reason | Tell the cockpit. Steer, pin, park, halt and cut live there. |

Manual `steer` on a *running* card does not inject mid-turn: the note is stored
and seen on the next claim (or on resume after park). Prefer **park** when the
agent is stuck and you want the same conversation to continue. Exception:
`MainAdvanced` steers every Claimed/Running card and then auto park+unparks so
that fetch/rebase note is acted on promptly — see the webhook section above.

Park + resume (agy only): the supervisor persists `conversation_id` from
stream-json. Park leaves it on the card and sets `parked` so the supervisor will
not reclaim until `unpark`. Unpark queues dispatch; the next claim in a live
sandbox runs `agy --conversation <id>` with a short resume prompt. Halt clears
the id so the next claim starts fresh.

Re-running a card resumes its existing branch and rebases onto **upstream**,
not the fork's base — a fork's base freezes the moment it's created, and drifted
6 commits inside one day. If the rebase conflicts, the supervisor backs out and
tells the agent to resolve it.

## Looking at the UI

```bash
npm --prefix web run shots      # -> web/shots/*.png
```

Runs a scratch honr on :8081 against a fixture board, captures desktop and
phone for each view plus both drawers. Your real state is untouched. The
fixture (`web/ui-fixture.mjs`) writes `honr.json` directly, because the states
worth seeing — lease ages that show the decay gradient, PR links, an escalation
mid-flight — are exactly the ones no public verb produces.

## When something breaks

**Everything in this stack fails as a hang, not an error.** A denied egress, a
missing credential, a wedged relay — all present as silence. If something is
taking too long, it has already failed.

```bash
openshell logs <sandbox> -n 60          # grep DENIED, ALLOWED, ssrf, HTTP:
openshell sandbox list                  # phases; Deleting still appears here
```

A sandbox is **kept, not deleted, when a card fails** — `openshell logs` is the
tool that answers questions and a deleted sandbox answers none. Names are
attempt-scoped (`honr-card-8-a2`), so retries don't collide with the one kept
for inspection. `reconcile` clears them on the next startup.

## Restarting honr while a card is running

Safe. The agent runs **detached** inside its sandbox — `/tmp/agent.log` for its
output, `/tmp/agent.pid` for its process group, `/tmp/agent.status` for its exit
code — so it does not die with the process watching it. On startup `reconcile`
lists the sandboxes honr labelled, matches each against its card's
`environment`, and for a card still Claimed or Running probes the sandbox and
picks the run back up from the line its log had reached. The card stays Running,
the lease is renewed before the sweeper gets a turn, and no second sandbox is
created. Everything else it finds is reaped.

Four things worth knowing:

- The story line `honr restarted; picked <sandbox> back up` is how you tell an
  adopted run from a fresh one.
- **Startup waits up to 3 minutes for the gateway** before reconciling, and
  holds the sweeper for as long as it waits. honr and the podman machine tend to
  start together, and reconciling blind is worse than reconciling late: without
  a sandbox listing honr cannot tell which runs are live, so the sweeper would
  requeue one that is still going and dispatch would race a second agent onto
  its branch. If the wait runs out you get a loud `gateway unreachable after
  180s; starting without reconciling` — treat any Running card as suspect.
- Spend during the downtime is not billed to the card. The supervisor charges
  the *difference* between cost lines and resumes from the last one already in
  the log, so it under-reports rather than double-counting. The per-card budget
  check still sees the run's real total.
- If the sandbox is up but nothing is running in it — honr died during setup,
  say — the card returns to Backlog without spending a retry. That was the
  restart's fault, not the card's.

Failure signatures worth recognising:

| Symptom | Cause |
|---|---|
| `can't find '__main__' module in '/tmp/metadata-shim.py'` | `upload` takes a destination *directory* |
| `timeout: failed to run command 'cargo'` | toolchain not on PATH — the image's `ENV` does not reach `sandbox exec` |
| `push failed:` with nothing after it | git writes errors to stderr; check `outerr`, not stdout |
| `(stale info)` on push | `--force-with-lease` against an ad-hoc URL instead of a named remote |
| `create sandbox failed: connection error` | podman died; classified as infrastructure, does not count against the card |
| Card flips Running → Backlog (needs dispatch again) | `run_deadline_at` exceeded (`agent_timeout_secs`); or Halt / infrastructure bounce |
