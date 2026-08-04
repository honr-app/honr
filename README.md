# honr

An agent orchestrator whose board is a **control plane, not a report**. It's
written to at machine speed, read by agents as their source of truth, and
moving a card *is* an action. The scarce resource is human attention.

**honr builds honr.** It takes cards against its own source, runs an agent in a
policy-enforced sandbox, and hands back a pull request you review on GitHub.

```
you ──chat──> operator agent (Cursor / Claude Code)
                    │ MCP (streamable HTTP, /mcp)
                    ▼
            ┌────────────────────┐
            │  honr (Rust/axum)  │◀── REST + SSE ── React UI
            │  one state machine │
            └────────────────────┘
                    ▲
              supervisor ──> worker agent in an OpenShell sandbox
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

Connect the operator MCP (honr must already be listening on :8080):

**Cursor** — project config is [`.cursor/mcp.json`](.cursor/mcp.json):

```json
{ "mcpServers": { "honr": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } } }
```

Then **Cursor Settings → Tools & MCP**, enable **honr**, and reload if needed.
With the operator rule in [`.cursor/rules/honr-operator.mdc`](.cursor/rules/honr-operator.mdc),
the agent drives Projects/Plans via MCP; sandboxed workers claim Ready Tasks and open PRs.

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

The board starts empty and nothing claims cards until you enable agents — see
[`docs/agents.md`](docs/agents.md). That's deliberate: honr must run on a
machine with no compute driver, no gateway and no credentials.

## Docs

Start at **[`docs/index.md`](docs/index.md)**.

| Page | What |
|---|---|
| [Concepts](docs/concepts.md) | Board as control plane, Project + Tasks, operator vs worker |
| [Quickstart](docs/quickstart.md) | Board-only run, UI, MCP connect |
| [Workflow](docs/workflow.md) | Project → plan → Approve → dispatch; triage; park / steer / halt |
| [Agents](docs/agents.md) | Enabling real agents (OpenShell, providers, Settings) |
| [Sandbox](docs/sandbox.md) | How a sandboxed run works and the gotchas that matter |
| [Architecture](docs/architecture.md) | Board / store, supervisor, MCP / REST, beads, persistence |

[`CLAUDE.md`](CLAUDE.md) / [`AGENTS.md`](AGENTS.md) — orientation for an agent
working *on* this repo in operator mode.

## Layout

| Path | What |
|---|---|
| `src/model.rs` | One node type. Project (container) + Task (claimable leaf). |
| `src/machine.rs` | Legal transitions + lifecycle invariants. |
| `src/store.rs` | The board: state, persistence, event bus, derived reads. |
| `src/beads.rs` | `bd` CLI wrapper — identity, Project→Task parent, deps. |
| `src/api.rs` `src/sse.rs` | The human face. |
| `src/mcp.rs` | The operator and worker face. |
| `src/openshell.rs` | Typed async wrapper over the `openshell` CLI. |
| `src/supervisor.rs` | Dispatch, the per-card sandbox lifecycle, lease sweeping. |
| `honr.yaml` | Level schema (Project + Task) and execution config. |
| `sandbox/` | Container image, network policy, metadata shim. |
| `web/` | React UI, plus a Playwright screenshot harness. |
