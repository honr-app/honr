# Cockpit (TTY attach)

The durable control-plane cockpit: a privileged OpenShell sandbox with narrow
egress to host honr MCP (operator tools only). Create knobs (image, policy,
CPU, memory, engine) come from the Board **Cockpit sandbox profile** — pick it
in Settings → Sandboxes (`POST /api/sandbox-profiles/{id}/cockpit`). Seeded
catalog includes a `cockpit` profile and sets it as Cockpit’s preference; you can
point Cockpit at any other profile. The live seat name stays
`{branch_prefix}-cockpit` (label `honr.cockpit`), independent of which profile built it.

Attach faces (Cockpit, host CLI connect) sit over the Board **cockpit session**
singleton — they do not own lifecycle. Mutations go through `Board` in
`store.rs` via REST `/api/cockpit-session*`. The supervisor materializes the
sandbox from that record; **Cockpit attach** launches interactive `agent`
(and resumes `conversation_id` when set).

Prerequisites: [Agents](agents.md) path is live (`execution.agents.enabled:
true`, OpenShell gateway healthy, a Cockpit sandbox profile in the catalog).
Containment: [Sandbox](sandbox.md#cockpit-vs-worker-containment).

## What stays on the Board

| Field | Meaning |
|---|---|
| `environment` | OpenShell sandbox name (default `{branch_prefix}-cockpit`, usually `honr-cockpit`) |
| `conversation_id` | Cursor chat id — Cockpit attach `--resume`s it (minted via `agent create-chat` if missing) |
| `status` | `Running` or `Parked` (park-like hold: sandbox + conversation kept) |

`POST /api/cockpit-session` creates Running. Park / resume / `DELETE` are the hold
and clear path. Creating the sandbox yourself, or storing conversation ids in a
wrapper, is a second state machine — do not.

## Start

With honr listening and agents enabled:

```bash
# Session cookie (same login as the UI). Cookie jar is auth only — not lifecycle.
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"…"}' \
  http://127.0.0.1:8080/auth/login

# Create the Board cockpit session (empty body is fine; supervisor fills environment).
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' \
  -d '{}' \
  http://127.0.0.1:8080/api/cockpit-session
```

Within a few seconds the supervisor creates or reuses the cockpit environment
(from the Cockpit sandbox profile) and writes `environment` onto the session.
Confirm:

```bash
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  http://127.0.0.1:8080/api/cockpit-session
# → { "session": { "environment": "honr-cockpit", "status": "Running", … } }

openshell sandbox list --selector honr.cockpit=1
```

Optional thin shim (same Board calls; no local lifecycle file):
[`scripts/cockpit.sh`](../scripts/cockpit.sh) `start`.

## How `openshell sandbox connect` talks gRPC

There is no `ConnectSandbox` RPC. The CLI path is:

1. `GetSandbox(name)` → `sandbox_id`
2. `CreateSshSession(sandbox_id)` → short-lived `token` + gateway host/port/scheme
3. Local `ssh -tt sandbox` with `ProxyCommand=openshell ssh-proxy … --token …`
4. `ssh-proxy` tunnels via `ForwardTcp` (`SshRelayTarget` + token)

honr wraps (1)–(2) as `OpenShell::create_ssh_session` / `revoke_ssh_session` in
[`src/openshell.rs`](../src/openshell.rs). Browsers cannot complete the OpenSSH
ProxyCommand chain, so Cockpit attach uses a different RPC (below).

Optional CLI: `openshell sandbox connect <env> --editor cursor` installs
managed SSH config and launches native Cursor on the host. honr does **not**
shell out for that — run it yourself if you want Remote SSH.

## Cockpit attach (in-browser)

Primary UI: open **Cockpit** from the centered top-bar chevron grip, Start the
Board cockpit session, then use the xterm.js surface over authenticated WebSocket
**`/api/cockpit-attach`**.

That endpoint opens OpenShell **`ExecSandboxInteractive`** into the
Board-named environment and runs interactive Cursor **`agent`** (`--trust
--approve-mcps --sandbox disabled`, no `--force` — tool calls prompt for
approval). If the session has a `conversation_id`, attach passes **`--resume
<id>`**; otherwise it runs `agent create-chat`, stores the id on the Board,
then resumes that chat. Stdin/stdout/resize are relayed over the WebSocket
(no local SSH).

| Face | Mechanism |
|---|---|
| Cockpit terminal | `GET` WebSocket `/api/cockpit-attach` → interactive `agent` |
| Host TTY (manual) | `openshell sandbox connect <env>` (CreateSshSession + ssh) |

Disconnecting the WebSocket does **not** stop the Board session — sandbox +
conversation stay under Start/Stop. Re-attach resumes the same chat id.
Cockpit does not surface Park/Resume (API still exists).

Legacy `POST /api/cockpit-chat` (detached agent stream-json bridge) remains for
compatibility; Cockpit no longer uses it.

## MCP auth inside the seat (special for Cockpit)

Host Cursor uses browser OAuth (`honr-cursor` client) against `/mcp`. That
dance does not work cleanly inside the sandbox.

Instead, honr **mints** short-lived MCP JWTs for the static public client
`honr-cockpit` and **injects** them into the Board-named sandbox:

| Path | Contents |
|---|---|
| `/sandbox/.honr/mcp/token.json` | access + refresh (mode 0600) |
| `/sandbox/.honr/mcp/mcp.json` | HTTP MCP entry with `Authorization: Bearer …` |
| `/sandbox/.honr/mcp/env.sh` | exports `HONR_MCP_URL` + `HONR_MCP_ACCESS_TOKEN` |

Triggers:

- Supervisor when the cockpit sandbox becomes Ready (subject `cockpit` fallback)
- `POST /api/cockpit-session/mcp-cred` from Cockpit after Start (subject =
  logged-in user; silent — no status chrome in the UI)
- Cockpit attach WebSocket open (same cookie mint, best-effort)

Tokens are **not** returned to browser JS. Do not run `agent mcp login`
inside the seat unless you intend a separate host-style OAuth flow.

Resource URL (JWT `aud`) defaults to
`http://host.docker.internal:8080/mcp` (`HONR_MCP_URL` override).

## Attach / TTY reconnect (CLI)

```bash
ENV=$(curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  http://127.0.0.1:8080/api/cockpit-session \
  | jq -r '.session.environment // empty')
test -n "$ENV" || { echo "no cockpit session / environment yet"; exit 1; }

openshell sandbox connect "$ENV"
```

After honr restart, the supervisor reconciles: if the Board session is still
`Running` and the sandbox is live, it keeps the sandbox and
`conversation_id`. Open Cockpit again to attach (interactive `agent
--resume`). You do not recreate the session to reconnect.

Shim: `scripts/cockpit.sh attach`.

Inside the seat the agent talks MCP at `$HONR_MCP_URL` (default
`http://host.docker.internal:8080/mcp`) — operator tools only. Host chat
clients (Cursor / Claude Code on `/mcp`) are the same cockpit tool surface;
TTY / Cockpit attach is how you sit with the sandboxed liaison.

## Park, resume, stop

| Intent | Board call | Effect |
|---|---|---|
| Hold without deleting | `POST /api/cockpit-session/park` | Agent stopped; sandbox + conversation kept; `Parked` |
| Continue after park | `POST /api/cockpit-session/resume` | `Running` again; supervisor restarts / resumes agent |
| Tear down | `DELETE /api/cockpit-session` | Agent stopped; sandbox deleted; session cleared |

```bash
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X POST http://127.0.0.1:8080/api/cockpit-session/park

curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X POST http://127.0.0.1:8080/api/cockpit-session/resume

curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X DELETE http://127.0.0.1:8080/api/cockpit-session
```

Shim: `park` / `resume` / `stop`. Do not `openshell sandbox delete` the cockpit box
while a Board session still points at it — let `DELETE /api/cockpit-session` drive
stop so inventory reconcile stays consistent.

## Human owns irreversibles

Attach and the cockpit agent prepare and surface **Review** / **Needs You**. Approving
merges stays human (honr never merges). Prefer escalating ambiguous
irreversibles rather than widening `approve_review` / `approve_plan` semantics.
Same rule as host MCP: see [Workflow](workflow.md) triage order.

## Smoke path (running board)

Against a board already on `:8080` with agents + OpenShell up:

```bash
# 1. Auth
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"…"}' \
  http://127.0.0.1:8080/auth/login >/dev/null

# 2. Start seat
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:8080/api/cockpit-session
# wait until .session.environment is set
for i in $(seq 1 30); do
  ENV=$(curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
    http://127.0.0.1:8080/api/cockpit-session | jq -r '.session.environment // empty')
  [ -n "$ENV" ] && break
  sleep 2
done
echo "environment=$ENV"

# 3. Attach (TTY) — Ctrl-D / exit leaves the seat Running
openshell sandbox connect "$ENV"

# 4. Reconnect proves durability (optional second attach)
openshell sandbox connect "$ENV"

# 5. Stop
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X DELETE http://127.0.0.1:8080/api/cockpit-session
openshell sandbox list --selector honr.cockpit=1   # expect empty after reconcile
```

Or: `scripts/cockpit.sh start && scripts/cockpit.sh attach` then
`scripts/cockpit.sh stop`. In the UI: open Cockpit beside the Board → Start →
attach in the terminal.

## Related

- [Concepts](concepts.md) — operator vs cockpit vs worker
- [Agents](agents.md) — enable compute + `cockpit` profile
- [Architecture](architecture.md) — one state machine; supervisor cockpit loop
- [Quickstart](quickstart.md) — host `/mcp` OAuth as the same operator tool surface
