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

Connect the cockpit:

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

Then describe a goal, approve the breakdown it proposes, and the cards appear.
State lives in `honr.json` (gitignored); delete it to start over.

**The board starts empty and nothing claims cards until you enable agents** —
see [`docs/operating.md`](docs/operating.md). That's deliberate: honr must run
on a machine with no podman, no gateway and no credentials.

## The three views

| View | The question it answers |
|---|---|
| **Overview** | What is this system doing? The why-chain top-down, with each branch reporting how much of it is real. |
| **Activity** | What is happening right now? Six columns, heartbeat age on the card face, cards visibly decaying as it grows. |
| **Needs you** | What is blocked on me? One-tap answers to agent escalations. |

The UI is for **understanding**; the agent is for **driving**. Steer, pin, halt
and cut scope live in the cockpit, because they all want a reason. What stays
in the UI is what is genuinely one tap: answering an escalation, approving a
review.

## Layout

| Path | What |
|---|---|
| `src/model.rs` | One node type. Project/epic/story/task are labels for altitude. |
| `src/machine.rs` | Legal transitions + the two invariants. |
| `src/store.rs` | The board: state, persistence, event bus, derived reads. |
| `src/api.rs` `src/sse.rs` | The human face. |
| `src/mcp.rs` | The cockpit and worker face. |
| `src/openshell.rs` | Typed async wrapper over the `openshell` CLI. |
| `src/supervisor.rs` | Dispatch, the per-card sandbox lifecycle, lease sweeping. |
| `honr.yaml` | Level schema and execution config. |
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
node type with a level schema and the commitment line (§4), human verbs and the
plan-approval gate (§5), seven worker verbs with `escalate` refusing fewer than
two options (§6), and chunking rather than compression (§8) — overflow reads
`7 ready · 2 blocked on #41 · oldest 40m`, computed server-side so it can't
drift.

Plus the execution path: sandbox per card, briefing built from the intent
chain, liveness and cost observed from the agent's output stream, budget caps,
retry limits, and cross-fork PRs.

## Not implemented

Checkpoints (§5), self-augmentation and proposals (§7), forecasting (§4), the
coherence pass (§8), auth. And **gates are still the agent's own claim** —
nothing independent verifies a card before it reaches Review. That is the
single most valuable thing left; see the state-of-play doc.
