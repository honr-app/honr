# Ops seat (TTY attach)

The durable control-plane ops chatbot: a privileged OpenShell sandbox on the
`ops` profile, with narrow egress to host honr MCP (operator tools only). Chat
and TTY are faces over the Board **ops session** singleton — they do not own
lifecycle. Mutations go through `Board` in `store.rs` via REST
`/api/ops-session*`. The supervisor materializes sandbox + detached agent from
that record.

Prerequisites: [Agents](agents.md) path is live (`execution.agents.enabled:
true`, OpenShell gateway healthy, `ops` profile seeded). Containment:
[Sandbox](sandbox.md#ops-vs-worker-containment).

## What stays on the Board

| Field | Meaning |
|---|---|
| `environment` | OpenShell sandbox name (default `{branch_prefix}-ops`, usually `honr-ops`) |
| `conversation_id` | Agent conversation for resume after park / reconnect |
| `status` | `Running` or `Parked` (park-like hold: sandbox + conversation kept) |

`POST /api/ops-session` creates Running. Park / resume / `DELETE` are the hold
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

# Create the Board ops session (empty body is fine; supervisor fills environment).
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' \
  -d '{}' \
  http://127.0.0.1:8080/api/ops-session
```

Within a few seconds the supervisor creates or reuses the `ops` sandbox, starts
the ops agent detached, and writes `environment` / `conversation_id` back onto
the session. Confirm:

```bash
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  http://127.0.0.1:8080/api/ops-session
# → { "session": { "environment": "honr-ops", "status": "Running", … } }

openshell sandbox list --selector honr.ops=1
```

Optional thin shim (same Board calls; no local lifecycle file):
[`scripts/ops-seat.sh`](../scripts/ops-seat.sh) `start`.

## Attach / TTY reconnect

Read the Board-named environment, then open an OpenShell SSH/TTY session into
that sandbox. Disconnecting does **not** stop the seat — sandbox + conversation
stay under the Board session (park-like keep). Re-attach with the same command.

```bash
ENV=$(curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  http://127.0.0.1:8080/api/ops-session \
  | jq -r '.session.environment // empty')
test -n "$ENV" || { echo "no ops session / environment yet"; exit 1; }

openshell sandbox connect "$ENV"
# or: openshell sandbox connect "$ENV" --editor cursor
```

After honr restart, the supervisor reconciles: if the Board session is still
`Running` and the sandbox is live, it re-adopts the detached agent and keeps
`conversation_id`. Attach again with `openshell sandbox connect` on the same
`environment`. You do not recreate the session to reconnect.

Shim: `scripts/ops-seat.sh attach`.

Inside the seat the agent talks MCP at `$HONR_MCP_URL` (default
`http://host.docker.internal:8080/mcp`) — operator tools only. Host chat
clients (Cursor / Claude Code on `/mcp`) are the same ops-seat tool surface;
TTY is how you sit with the sandboxed liaison.

## Park, resume, stop

| Intent | Board call | Effect |
|---|---|---|
| Hold without deleting | `POST /api/ops-session/park` | Agent stopped; sandbox + conversation kept; `Parked` |
| Continue after park | `POST /api/ops-session/resume` | `Running` again; supervisor restarts / resumes agent |
| Tear down | `DELETE /api/ops-session` | Agent stopped; sandbox deleted; session cleared |

```bash
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X POST http://127.0.0.1:8080/api/ops-session/park

curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X POST http://127.0.0.1:8080/api/ops-session/resume

curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -X DELETE http://127.0.0.1:8080/api/ops-session
```

Shim: `park` / `resume` / `stop`. Do not `openshell sandbox delete` the ops box
while a Board session still points at it — let `DELETE /api/ops-session` drive
stop so inventory reconcile stays consistent.

## Human owns irreversibles

Chat and the ops agent prepare and surface **Review** / **Needs You**. Approving
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
  http://127.0.0.1:8080/api/ops-session
# wait until .session.environment is set
for i in $(seq 1 30); do
  ENV=$(curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
    http://127.0.0.1:8080/api/ops-session | jq -r '.session.environment // empty')
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
  -X DELETE http://127.0.0.1:8080/api/ops-session
openshell sandbox list --selector honr.ops=1   # expect empty after reconcile
```

Or: `scripts/ops-seat.sh start && scripts/ops-seat.sh attach` then
`scripts/ops-seat.sh stop`.

## Related

- [Concepts](concepts.md) — operator vs ops seat vs worker
- [Agents](agents.md) — enable compute + `ops` profile
- [Architecture](architecture.md) — one state machine; supervisor ops loop
- [Quickstart](quickstart.md) — host `/mcp` OAuth as the same operator tool surface
