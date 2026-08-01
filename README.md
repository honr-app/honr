# honr

An agent orchestrator whose board is a **control plane, not a report**. It's
written to at machine speed, read by agents as their source of truth, and
moving a card *is* an action. The scarce resource is human attention.

**honr builds honr.** It takes cards against its own source, runs an agent in a
policy-enforced sandbox, and hands back a pull request you review on GitHub.
That loop works today — [PR #1](https://github.com/shanemcd/honr/pull/1) was
written by an agent honr dispatched, and is merged.

```
you ──chat──> Claude Code (the cockpit)
                    │ MCP (streamable HTTP, /mcp)
                    ▼
            ┌────────────────────┐
            │  honr (Rust/axum)  │◀── REST + SSE ── React UI
            │  one state machine │
            └────────────────────┘
                    ▲ seven verbs
              supervisor ──> agent in an OpenShell sandbox
                                 └─> cross-fork PR ──> you merge
```

The UI and the agent API are **two renderings of one state machine**. Every
mutation goes through `Board` in `src/store.rs`; no transport holds any
state-machine logic, which is what stops the two drifting apart.

## Run it

```bash
cargo run                 # :8080 — API, SSE, MCP, and the built UI
```

Serves `web/dist` if it exists. For hot reload: `cd web && npm install && npm run dev`
(:5173, proxies to :8080). `HONR_PORT` overrides the port.

Connect the cockpit (honr must already be listening on :8080):

**Cursor** — project config is [`.cursor/mcp.json`](.cursor/mcp.json):

```json
{ "mcpServers": { "honr": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } } }
```

Then **Cursor Settings → Tools & MCP** (or Customize → MCPs), enable **honr**, and reload if needed.
With the cockpit rule in [`.cursor/rules/honr-cockpit.mdc`](.cursor/rules/honr-cockpit.mdc), the agent
drives Projects/Plans via MCP; sandboxed workers claim Ready Tasks and open PRs.

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

Then describe a goal, approve the Plan, and the cards appear.
State lives in `honr.json` (gitignored); delete it to start over.

**The board starts empty and nothing claims cards until you enable agents** —
see [`docs/operating.md`](docs/operating.md). That's deliberate: honr must run
on a machine with no podman, no gateway and no credentials.

## The two views

| View | The question it answers |
|---|---|
| **Home** | What should I know right now? Status, one-tap answers to escalations, and Project progress. |
| **Board** | What is happening in the columns? Ready → Done, with heartbeat decay on running cards. |

The UI is for **understanding**; the agent is for **driving**. Steer, pin, halt
and cut scope live in the cockpit, because they all want a reason. What stays
in the UI is what is genuinely one tap: answering an escalation, approving a
review.

## Layout

| Path | What |
|---|---|
| `src/model.rs` | One node type. Project (container) + Task (claimable leaf). |
| `src/machine.rs` | Legal transitions + the two invariants. |
| `src/store.rs` | The board: state, persistence, event bus, derived reads. |
| `src/beads.rs` | `bd` CLI wrapper — identity, Project→Task parent, deps. |
| `src/api.rs` `src/sse.rs` | The human face. |
| `src/mcp.rs` | The cockpit and worker face. |
| `src/openshell.rs` | Typed async wrapper over the `openshell` CLI. |
| `src/supervisor.rs` | Dispatch, the per-card sandbox lifecycle, lease sweeping. |
| `honr.yaml` | Level schema (Project + Task) and execution config. |
| `sandbox/` | Container image, network policy, metadata shim. |
| `web/` | React UI, plus a Playwright screenshot harness. |

## Docs

- **[`docs/state-of-play.md`](docs/state-of-play.md)** — where this actually
  is, what's proven, what's next. **Start here.**
- [`docs/operating.md`](docs/operating.md) — running it, enabling agents,
  what breaks and how it presents.
- [`docs/sandbox-stack.md`](docs/sandbox-stack.md) — how an agent gets Vertex
  credentials it can't read, why we route around an upstream bug, and every
  gotcha found the hard way.
- [`CLAUDE.md`](CLAUDE.md) — orientation for an agent working *on* this repo.

## What's implemented

Columns and card anatomy (§1–2), the lifecycle enforced server-side (§3), one
node type with a **Project + flat Tasks** level schema (§4) — Vision/Epic/Story
are retired; task↔task links are dependency edges (via beads). Human verbs and
the plan-approval gate (§5), seven worker verbs with `escalate` refusing fewer
than two options (§6), and chunking rather than compression (§8).

Plus the execution path: sandbox per card, briefing built from the Project→Task
chain, beads mirrored into the sandbox (`BEADS_DIR`), liveness and cost from the
agent's output stream, budget caps, retry limits, and cross-fork PRs.

## Not implemented

Checkpoints (§5), self-augmentation and proposals (§7), forecasting (§4), the
coherence pass (§8), auth. And **gates are still the agent's own claim** —
nothing independent verifies a card before it reaches Review. That is the
single most valuable thing left; see the state-of-play doc.
