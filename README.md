# honr

A POC of the agent-orchestrator design in `agent-orchestrator-ux.md`.

The board is a **control plane**, not a report. It's written to at machine
speed, read by agents as their source of truth, and moving a card *is* an
action. The scarce resource is human attention, so the UI's job is triage.

## Shape

```
you ──chat──> Claude Code (the cockpit)
                    │ MCP (streamable HTTP, /mcp)
                    ▼
            ┌────────────────────┐
            │  honr (Rust/axum)  │◀── REST + SSE ── React board
            │  one state machine │
            └────────────────────┘
                    ▲ seven verbs
              supervisor ──> agents in OpenShell sandboxes
```

The UI and the agent API are **two renderings of one state machine**. Every
mutation from either side goes through `Board` in `src/store.rs`; no transport
holds any state-machine logic, which is what stops the two drifting apart.

## Run it

```bash
cargo run                       # :8080 — API, SSE, MCP, and the built UI
cd web && npm install && npm run dev   # :5173 — hot-reloading board
```

`cargo run` serves `web/dist` if you've run `npm run build`; otherwise use the
Vite dev server, which proxies `/api` and `/mcp` to :8080.

Connect the cockpit:

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

Then create a goal, propose a breakdown, approve the plan — and watch it land in
the browser.

**The board starts empty and nothing claims cards yet.** The simulated fleet
that used to populate it has been removed; the supervisor that replaces it is
still being built (see Phase 2 below). Cards you create will sit in Ready until
it lands — that's correct, not a hang.

State lives in `honr.json` (gitignored). Delete it to start over.

## Layout

| Path | What |
|---|---|
| `src/model.rs` | One node type. Project/epic/story/task are labels for altitude. |
| `src/machine.rs` | Legal transitions + the two invariants. The only tests. |
| `src/store.rs` | The board: state, persistence, event bus, derived reads. |
| `src/api.rs` `src/sse.rs` | The human face. |
| `src/mcp.rs` | The cockpit and worker face. |
| `src/supervisor.rs` | Lease sweeping now; the per-card sandbox lifecycle next. |
| `honr.yaml` | Level schema and execution timings. |
| `web/` | React board, tree and digest. |

## What's implemented

- **Columns** (§1–2) with Needs You split out red from Review yellow. Card
  anatomy differs per column because the question differs; heartbeat age is on
  the card face and cards visibly decay as it grows.
- **Lifecycle** (§3) enforced server-side, including `Running → Splitting`
  self-orchestration and `Running → Ready` on lease expiry.
- **One node type + level schema + commitment line** (§4), with the intent
  chain returned by `claim`.
- **Human verbs** (§5): steer, pin, halt, cut scope, approve, request changes —
  and the plan-approval gate.
- **Seven worker verbs** (§6). `escalate` refuses fewer than two options;
  `split` refuses to exceed the depth/fan-out governor.
- **Chunking, not compression** (§8): overflow reads
  `7 ready · 2 blocked on #41 · oldest 40m`, computed server-side so it can't
  drift. Plus a per-goal narrative and a digest.

## Not implemented

Checkpoints (§5), self-augmentation/proposals (§7), forecasting (§4), the
coherence pass (§8), auth. And **execution environments** — see below.

## Phase 2 — real agents (in progress)

The design doc never said *where agents actually run*. The answer is
[NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell): honr points at a gateway and creates one
policy-enforced sandbox per work item.

**The risky part is already proven.** A Claude Code agent runs in a sandbox, authenticates to Vertex
with no API key and no credential in the sandbox, and opens a real GitHub PR
([probe PR](https://github.com/clankrshq/honr-sandbox-probe/pull/1)).

Start here: **[`docs/phase-0-findings.md`](docs/phase-0-findings.md)** — the working setup, the
Vertex credential trick, the upstream bug we route around, and every gotcha. Sandbox assets live in
[`sandbox/`](sandbox/).

Prerequisites, if you're setting this up fresh:

```bash
podman machine start                 # the driver; it does stop on its own sometimes
brew services start openshell        # gateway on :17670 (not 8080 — no conflict with honr)
openshell status                     # expect Connected + Authenticated
```

The remaining honr-side work — `openshell.rs`, the per-card lifecycle in `supervisor.rs`, the
execution config — is listed at the end of the findings doc.

## The goal: honr builds honr

The point of phase 2 is to point honr at its own remaining work: cards describing honr's source,
agents that clone the repo in a sandbox and open a real PR, review that happens on GitHub, merging
that stays a human action.

Two consequences worth knowing before you read the code:

- **The first pieces can't be built this way.** `openshell.rs` and `supervisor.rs` *are* the
  execution path, so they're hand-written. Only once they work can honr take a card against itself.
- **An agent editing honr's source cannot affect the running honr.** The live binary was built
  before the edit; self-modification only takes effect on a deliberate rebuild.
