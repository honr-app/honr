# Agents

This spends real money and opens pull requests. Four **roles** must be
satisfied on the host: the concrete tools below are examples, not the only
stack. For sandbox mechanics and gotchas, see [Sandbox](sandbox.md).

## 1. Compute driver

OpenShell’s gateway needs a working Docker-compatible API (`docker info`
succeeds). How you provide that is a **host choice**:

| Driver | Typical setup |
|---|---|
| **podman** (machine) | `podman machine start`; CLI often via `docker` talking to podman's socket |
| **Colima** | `colima start`; point the gateway at Colima’s socket (e.g. `DOCKER_HOST=unix://$HOME/.colima/default/docker.sock` in the gateway’s env) |
| **Docker Desktop / engine** | Ensure the daemon is up and the gateway process can reach its socket |

`DOCKER_HOST`, `~/.config/openshell/gateway.env`, and similar knobs belong to
the **gateway process**, not to honr Settings. Honr only needs the gateway to
answer `openshell status`.

The driver can stop on its own. The supervisor health-checks before claiming
and pauses after an infrastructure failure rather than burning a card’s retry
budget, but it cannot prevent the outage.

## 2. OpenShell gateway

```bash
# Example (Homebrew service). Use whatever starts *your* gateway:
#   brew services start openshell
openshell status       # expect Connected + Authenticated (mTLS transport)
```

Confirm in **Settings → OpenShell** (healthy / unhealthy, or an explicit error
if the CLI is missing). Optional binary path override lives there when
`openshell` is not on `PATH`.

Default local gateway port is often `17670`: deliberately not honr’s `8080`.
Your install may differ; trust `openshell status`, not a hardcoded URL.

## 3. Providers (temporarily out of honr)

Honr no longer stores OpenShell provider names or Vertex/GitHub secrets in
yaml or Settings. Sandbox create passes an empty provider list until providers
can be created and attached entirely through the honr UI/API.

Until that lands, wipe and recreate gateway providers yourself if you need a
manual smoke test — but do not expect honr to wire them.

## 4. Sandbox image

Build (or pull) the image your sandbox profile’s `--from` / `image` field
references. For honr’s own Rust toolchain image:

```bash
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

From the repo root, not `sandbox/`: `Cargo.lock` and `web/package-lock.json`
must be in context. Other product repos may use a different image via
Settings → OpenShell → Profiles.

Then flip `execution.agents.enabled: true` in `honr.yaml` and **restart** -
config is read once at startup; there is no hot reload and no runtime toggle.

> Put it back to `false` before committing. It has been committed as `true`
> once already, swept in by `git add -A`, which would make a fresh clone spend
> money on startup.

## Sandbox profile resolution

When the supervisor creates an OpenShell sandbox for a card, create knobs
(`--from` image, policy YAML, cpu, memory) and the agent engine resolve in this
order:

1. **Project override**: `sandbox_profile_id` on the containing Project, if set
   and present in the board profile catalog
2. **Global default**: `default_sandbox_profile_id` on durable board state
3. **YAML fallback**: `execution.agents` `image` / `policy` / `cpu` / `memory`
   / `engine` in `honr.yaml` (also used to seed the catalog when it is empty at
   load)

Profile `policy` is **inline YAML text** stored on the board (edited in Settings
as a textarea). At create, the supervisor writes a temp file for OpenShell's
`--policy` flag. The host path in `execution.agents.policy` is seed/fallback
only: not the catalog source of truth.

**Engine** is a field on the sandbox profile (Settings → OpenShell → Profiles).
When a profile omits it, claim/run falls back to Settings → Agent runtime
`engine`. Per-card engine overrides are ignored.

Profiles are managed under Settings → OpenShell → Profiles (REST:
`/api/sandbox-profiles`). Concurrency, timeouts, and the fallback engine live
under Agent runtime.

## When something breaks

**Everything in this stack fails as a hang, not an error.** A denied egress, a
missing credential, a wedged relay: all present as silence. If something is
taking too long, it has already failed.

```bash
openshell logs <sandbox> -n 60          # grep DENIED, ALLOWED, ssrf, HTTP:
openshell sandbox list                  # phases; Deleting still appears here
```

A sandbox is **kept, not deleted, when a card fails**: `openshell logs` is the
tool that answers questions and a deleted sandbox answers none. Names are
attempt-scoped (`honr-card-8-a2`), so retries don't collide with the one kept
for inspection. `reconcile` clears them on the next startup.

## Restarting honr while a card is running

Safe. The agent runs **detached** inside its sandbox. On startup `reconcile`
lists the sandboxes honr labelled, matches each against its card's
`environment`, and for a card still Claimed or Running probes the sandbox and
picks the run back up. The card stays Running; no second sandbox is created.

Startup waits up to 3 minutes for the gateway before reconciling. If the wait
runs out you get a loud `gateway unreachable after 180s; starting without
reconciling`: treat any Running card as suspect.

If the sandbox is up but nothing is running in it, the card returns to Backlog
without spending a retry: that was the restart's fault, not the card's.
