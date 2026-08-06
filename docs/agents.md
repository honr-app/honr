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
the **gateway process**, not to honr Settings. Honr reaches the gateway over
in-process gRPC using endpoint + mTLS from Settings → OpenShell.

The driver can stop on its own. The supervisor health-checks before claiming
and pauses after an infrastructure failure rather than burning a card’s retry
budget, but it cannot prevent the outage.

## 2. OpenShell gateway

Start the OpenShell gateway on the host (Homebrew service, systemd, or your
install’s equivalent). Honr does **not** spawn an `openshell` CLI for board
traffic: `src/openshell.rs` talks to the gateway in-process over gRPC with
client certificates.

**Settings → OpenShell** is the source of truth for connectivity:

- **Gateway endpoint** (often `https://127.0.0.1:17670` — deliberately not
  honr’s `8080`; your install may differ)
- **mTLS PEMs** (CA, client cert, client key) sealed into the board DB with
  `~/.config/honr/master.key`. Paste them or use **Import from local config**
  (`~/.config/openshell/gateways/<name>/mtls/`). The API never returns private
  key material.
- **Health** via Refresh status (`GET /api/openshell/status`) — Healthy /
  Unhealthy / not configured. Host Docker / Colima stay outside honr.

```bash
# Optional host check of the gateway process itself (not how honr connects):
#   brew services start openshell
openshell status       # expect Connected + Authenticated (mTLS transport)
```

Settings does not offer an OpenShell CLI binary path override.

## 3. Providers

**Settings → OpenShell → Providers** holds the desired provider list on the
board (credentials sealed). That list is the source of truth; Sync applies it
to the gateway (`POST /api/openshell/providers/sync`). Save also applies when
the gateway is reachable. Providers marked **attach** are passed on sandbox
create.

Provider **`github`** is owned by **Settings → GitHub App**: installation
tokens sync into the OpenShell `github` provider as `GH_TOKEN`. Do not manage
that provider’s credentials by hand in the OpenShell providers band.

REST: `/api/openshell/providers` (list/create/update/delete) and
`/api/openshell/providers/sync`.

## 4. Sandbox image

Build (or pull) the image your sandbox profile’s `--from` / `image` field
references. For honr’s own Rust toolchain image:

```bash
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

From the repo root, not `sandbox/`: `Cargo.lock` and `web/package-lock.json`
must be in context. Other product repos may use a different image via
Settings → OpenShell → Sandbox specs.

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

Empty-catalog seed inserts two profiles: **`default`** (card worker — image/cpu/memory
from `execution.agents`, policy from the built-in seed when
`execution.agents.policy` is `embedded`) and **`cockpit`** (control-plane seat, from
[`sandbox/cockpit-policy.yaml`](../sandbox/cockpit-policy.yaml) with lighter cpu/memory).
Boards that already have a worker catalog still get `cockpit` via
`ensure_cockpit_sandbox_profile` at boot when it is missing. The global default
stays the worker profile; pick `cockpit` in Settings when starting a cockpit.
The supervisor materializes that seat from the Board cockpit-session record
(create/reuse sandbox, detached agent, park-like keep across restart, stop)
without the card claim/heartbeat/report path. Start, TTY attach/reconnect, and
stop: [Cockpit](cockpit.md).

Profile `policy` is **inline YAML text** stored on the board (edited in Settings
as a textarea). That is the source of truth. At create, the supervisor writes a
temp file for OpenShell's `--policy` flag. `execution.agents.policy: embedded`
only seeds an empty catalog — there is no host `sandbox/policy.yaml` to edit.

**Engine** is a field on the sandbox spec (Settings → OpenShell → Sandbox specs).
When a spec omits it, claim/run falls back to Settings → Agent runtime
`engine`. Per-card engine overrides are ignored.

Registered engines (explicit registry in `src/engine.rs` — unknown ids fail
loud, no silent Claude fallthrough):

| Id | Binary / launch | Resume |
|---|---|---|
| `cursor` | `agent … --output-format stream-json` | `--resume` |
| `agy` | `agy … --output-format stream-json` | `--conversation` |
| `claude` | `claude --bare -p … --output-format stream-json` | (none) |
| `opencode` | `opencode run --format json --auto` | `--session` |

Claude and OpenCode use OpenShell `inference.local` (`ANTHROPIC_BASE_URL`);
configure a workspace Vertex route with `openshell inference set` (see
[`docs/sandbox.md`](sandbox.md)). OpenCode is baked into `honr-sandbox`
(`/usr/local/bin/opencode`) with models.dev / opencode.ai egress in the seed +
cockpit policies. Do not bake API keys into the image.

**agy** uses OpenShell provider type `antigravity` (shipped YAML under
`sandbox/openshell/antigravity.yaml`). Sync imports the type, seals the host
access token into Board provider `antigravity`, and attaches it so the seat
only sees `ANTIGRAVITY_ACCESS_TOKEN=openshell:resolve:…`. Pre-start / cockpit
attach write that placeholder into
`/sandbox/.gemini/antigravity-cli/antigravity-oauth-token` — never a host
OAuth file. Details: [`docs/sandbox.md`](sandbox.md#antigravity-agy).

Sandbox specs are managed under Settings → OpenShell → Sandbox specs (REST:
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
