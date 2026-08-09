# Cockpit

A durable terminal inside a sandbox that can reach honr's operator tools. Use
it when you want an agent that can triage the board with you, rather than one
working a card.

It is the third role in [Concepts](concepts.md#operator-and-worker): narrower
than you (no merges), wider than a card worker (which cannot see the board).

**Prerequisites:** [Your first agent](first-agent.md) setup is live — a healthy
OpenShell gateway and a cockpit sandbox spec in the catalog.

## Start one

In the UI:

1. Open **Cockpit** from the centred chevron grip in the top bar.
2. Click **Start**.
3. Wait a few seconds for the supervisor to provision the sandbox.
4. Type in the terminal.

That is the whole path. Everything below is for automating it or understanding
what it did.

Disconnecting the terminal does **not** stop the session — the sandbox and the
conversation stay up under Start/Stop, and re-attaching resumes the same chat.
Restarting honr does not stop it either: the supervisor reconciles, and you just
open Cockpit again.

## What is actually durable

The session is a singleton record on the Board, not a file or a wrapper script:

| Field | Meaning |
|---|---|
| `environment` | Sandbox name — defaults to `{branch_prefix}-cockpit`, usually `honr-cockpit` |
| `conversation_id` | Chat id the session resumes; minted if missing |
| `status` | `Running`, or `Parked` — a hold that keeps sandbox and conversation |

The terminal and any CLI attach are **faces over that record**. They read and
mutate it through `Board`; they do not own lifecycle. Create sandboxes through
Board APIs so inventory reconcile stays consistent.

Which image, CPU, memory, engine, and **Policy** (by id) Cockpit gets comes
from the **cockpit sandbox spec** (Settings → OpenShell → Sandbox specs). Edit
allow-list YAML under **Policies**; the spec only references it. Live policy is
set at create and fixed for that sandbox. The sandbox name stays
`{branch_prefix}-cockpit` regardless of which spec built it, so you can point
Cockpit at any spec you like.

## Driving it from the CLI

Same Board calls the UI makes. Log in first — the cookie jar is auth, not
lifecycle.

```bash
# HONR_URL = the origin you open the board on (Host / window.location.origin)
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"…"}' \
  "$HONR_URL/auth/login"

# Start (empty body; the supervisor fills in `environment`)
curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  -H 'Content-Type: application/json' -d '{}' \
  "$HONR_URL/api/cockpit-session"
```

| Intent | Call |
|---|---|
| Start | `POST /api/cockpit-session` |
| Inspect | `GET /api/cockpit-session` |
| Hold without deleting | `POST /api/cockpit-session/park` |
| Continue after a park | `POST /api/cockpit-session/resume` |
| Tear down | `DELETE /api/cockpit-session` |

Attach a host terminal once `environment` is set:

```bash
ENV=$(curl -sS -c /tmp/honr.cookies -b /tmp/honr.cookies \
  "$HONR_URL/api/cockpit-session" | jq -r '.session.environment // empty')
openshell sandbox connect "$ENV"
```

[`scripts/cockpit.sh`](https://github.com/honr-app/honr/blob/main/scripts/cockpit.sh) is a thin shim over exactly these
calls: `start` / `attach` / `park` / `resume` / `stop`.

Do not `openshell sandbox delete` the cockpit box while a Board session still
points at it. Let `DELETE /api/cockpit-session` drive teardown so inventory
reconcile stays consistent.

## How the browser terminal works

The in-browser terminal is xterm.js over an authenticated WebSocket at
`/api/cockpit-attach`, which opens OpenShell `ExecSandboxInteractive` into the
Board-named environment and runs the cockpit spec's engine. Stdin, stdout, and
resize are relayed over that socket — no local SSH, because a browser cannot
complete the OpenSSH ProxyCommand chain that `openshell sandbox connect` uses.
(That chain is described in [Architecture](architecture.md#how-the-cli-attaches).)

Cursor launches interactive `agent` with `--trust --approve-mcps --sandbox
disabled` — no `--force`, so tool calls still prompt for approval. OpenCode,
Claude, and agy launch their own TUIs.

## Credentials inside the sandbox

Model auth comes from OpenShell providers and `inference.local`, never from host
secrets copied into the sandbox.

MCP auth from inside the sandbox works differently from the host. Host Cursor
uses browser OAuth against `/mcp`, and that dance does not work cleanly from
inside a sandbox. So the shipped `honr` MCP entry is **stdio**, not HTTP: no
login, no Bearer, no OAuth dance to skip.

| Path | Contents |
|---|---|
| `/sandbox/.honr/mcp/mcp.json` | `honr` → `socat - UNIX-CONNECT:/sandbox/.honr/mcp/agent.sock` (Cursor) |
| `/sandbox/.honr/mcp/claude_mcp.json` | same shape; Claude loads it via `--mcp-config` |
| `/sandbox/.gemini/config/mcp_config.json` | same, for Antigravity |
| `/sandbox/.config/opencode/opencode.jsonc` | OpenCode `mcp.honr`, `type: local` |

Injection happens when the sandbox becomes Ready, on
`POST /api/cockpit-session/mcp-cred`, and on terminal attach. Do not run
`agent mcp login` inside the sandbox unless you specifically want a separate
host-style OAuth flow.

### How the MCP relay works

honr keeps a board-owned `ExecSandboxInteractive` relay running `socat
UNIX-LISTEN:/sandbox/.honr/mcp/agent.sock STDIO` inside the sandbox — its
gRPC-piped stdin/stdout are wired straight into the same `Operator` MCP
handler that serves the HTTP `/mcp` endpoint (`rmcp::serve_server` over the
pipe). No port, no network policy entry, no Bearer to mint — same path on
local Docker/Podman and remote Kubernetes, since it never leaves the
sandbox's own netns.

```mermaid
flowchart TB
  subgraph sandbox ["Sandbox (honr-cockpit)"]
    agent["Agent MCP client<br/>(reads mcp.json)"]
    socatClient["socat - UNIX-CONNECT:agent.sock"]
    sock[["agent.sock"]]
    socatServer["socat UNIX-LISTEN:agent.sock STDIO"]

    agent <--> socatClient
    socatClient <-->|"Unix domain socket"| sock
    sock <--> socatServer
  end

  subgraph host ["honr host process"]
    grpcClient["exec_interactive_raw()"]
    pumpLoop["pump_loop()"]
    duplexPair[["tokio::io::duplex()"]]
    serveServer["rmcp::serve_server"]
    operator["Operator"]
    board["Board"]

    grpcClient <--> pumpLoop
    pumpLoop <--> duplexPair
    duplexPair <-->|"newline-delimited<br/>JSON-RPC"| serveServer
    serveServer <--> operator
    operator <--> board
  end

  socatServer <-->|"exec's own stdin/stdout<br/>= gRPC stream"| grpcClient
```

The one-shot listen means agent disconnect is visible on the socket, not
just inferred: `socat` exits, and the board re-spawns for the next connect.
(Not `nc` — the sandbox image's OpenBSD-netcat build accepts the connection
but never forwards bytes written to its stdin *after* accept out to the
socket, which is exactly the `serve_server`-response direction.) See
[Sandbox](sandbox.md).

For agy the attached `antigravity` provider injects only an
`openshell:resolve:…` placeholder, and attach writes that into the sandbox's
token file — never a host OAuth file. Connect once via Settings → Providers →
**Log in with Google** so the gateway can refresh access tokens. See
[Sandbox → Antigravity](sandbox.md#antigravity--agy).

## Cockpit cannot merge either

The cockpit agent prepares and surfaces Review and Needs You. Approving a merge
stays human, same as on the host MCP surface. Prefer escalating an ambiguous
irreversible over widening what `approve_review` / `approve_plan` mean.

## Related

- [Concepts](concepts.md#operator-and-worker) — how the three roles differ
- [Sandbox](sandbox.md#default-vs-cockpit) — default vs Cockpit specs
- [Configuration](configuration.md#policies) — Policies catalog
- [Configuration](configuration.md#sandbox-specs) — picking the spec
