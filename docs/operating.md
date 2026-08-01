# Operating honr

## Just the board

```bash
cargo run           # :8080 — API, SSE, MCP, and web/dist if built
```

No podman, no gateway, no credentials needed. Agents are off by default.
`HONR_PORT` overrides the port. State is `honr.json` in the working directory.

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

## What dispatch decides, and when

`dispatch_loop` polls every 3 seconds and passes four gates in order: in-flight
below `max_concurrent`, spend below the daily ceiling, not in an infrastructure
cooldown, gateway healthy. Then it takes the **lowest-id** Ready card not
already being run by this process, and claims it.

A card is eligible if it is `Ready`, has no children, has no unresolved
blockers, and its capability matches — which today is hardcoded to `["any"]`,
so a card tagged anything else is silently never claimed.

Nothing pushes work. A card becoming Ready is the entire trigger, which means
the real human decision point is **Approve Plan** (`approve_plan` — materialize
the Project's Plan artifact into Ready Tasks; the Project itself never goes
Ready), not dispatch.

## Steering a card

| You want to | Do this |
|---|---|
| Send a reviewed card back with instructions | **Request changes** in the drawer. The note reaches the next run's briefing. |
| Answer a blocked agent | **Home** — Needs you section, pick an option. Resets the card's retry budget. |
| Anything requiring a reason | Tell the cockpit. Steer, pin, halt and cut live there. |

`steer` on a *running* card does nothing today: the briefing is built once at
claim time and `claude -p` has no injection channel. The note is stored and
seen by the next run.

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
  say — the card returns to Ready without spending a retry. That was the
  restart's fault, not the card's.

Failure signatures worth recognising:

| Symptom | Cause |
|---|---|
| `can't find '__main__' module in '/tmp/metadata-shim.py'` | `upload` takes a destination *directory* |
| `timeout: failed to run command 'cargo'` | toolchain not on PATH — the image's `ENV` does not reach `sandbox exec` |
| `push failed:` with nothing after it | git writes errors to stderr; check `outerr`, not stdout |
| `(stale info)` on push | `--force-with-lease` against an ad-hoc URL instead of a named remote |
| `create sandbox failed: connection error` | podman died; classified as infrastructure, does not count against the card |
| Card flips Running → Ready → Running | lease expired during a silent build; `lease_secs` too low |
