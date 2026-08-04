<p align="center">
  <img src="assets/honr-logo.png" alt="honr, the one that hones" width="180" />
</p>

<h1 align="center">honr</h1>

<p align="center">
  <strong>The board that hones the work.</strong><br />
  An agent orchestrator whose board is a control plane, not a status report.
</p>

<p align="center">
  <a href="docs/index.md"><img alt="Docs" src="https://img.shields.io/badge/docs-operator%20guide-3d7ea6?style=flat-square" /></a>
  <a href="Cargo.toml"><img alt="Rust" src="https://img.shields.io/badge/rust-1.88+-dea584?style=flat-square&logo=rust&logoColor=white" /></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-active-2ea44f?style=flat-square" /></a>
</p>

---

## Name

**honr** means **honer**: the one that hones.

To **hone** *(v.)* is to refine: to make a skill, idea, or technique better and
more effective through practice and time. honr is built for that loop. Intent
in, agents against real repositories in sandboxed compute, pull requests out,
so the work gets sharper every turn under human judgment.

## What it is

honr is a self-hostable control plane for agent-driven development:

- **One board, one state machine.** The UI and the MCP API are two faces of
  the same lifecycle. Moving a card *is* an action.
- **Operator and worker, separated.** You (and a chat agent on the host) shape
  Projects and Plans over MCP. Workers run inside OpenShell sandboxes with no
  network path back to honr. They cannot approve their own review.
- **PRs, not merges.** Approving in honr surfaces the pull request. A human
  merges on GitHub. The bot has no write access to upstream.

**honr builds honr.** Cards against this repository are claimed by sandboxed
agents and land as cross-fork PRs you review like any other contribution.

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

## Features

| | |
|---|---|
| **Project + Task model** | Containers hold the Plan; claimable Tasks are the leaves. |
| **MCP-native operator** | Create, dispatch, park, steer, halt, and triage from Cursor or Claude Code. |
| **Sandboxed execution** | OpenShell sandboxes, policy-enforced egress, Vertex (or your stack) for inference. |
| **Board-only mode** | Run with agents off. No Docker, no gateway, no credentials required. |
| **Beads identity** | Optional dual-write into `bd` for durable issue graphs. |
| **Webhook catch-up** | Push to `main` advances Done and steers live runs to rebase. |

## Quick start

**Requirements:** Rust 1.88+, a recent Node.js for the UI build (optional if
you only need the API).

```bash
git clone https://github.com/shanemcd/honr.git
cd honr
cargo run                 # http://127.0.0.1:8080  (API, SSE, MCP, UI)
```

Or with the Makefile:

```bash
make run                  # release build of API + web/dist, then serve
make dev-ui               # Vite on :5173 (proxies to :8080)
```

Connect an operator MCP client (honr must already be listening):

**Cursor** ([`.cursor/mcp.json`](.cursor/mcp.json)):

```json
{
  "mcpServers": {
    "honr": { "type": "http", "url": "http://127.0.0.1:8080/mcp" }
  }
}
```

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

The board starts empty. Nothing claims cards until you enable agents. See
[`docs/agents.md`](docs/agents.md). That is deliberate: the control plane
should run on a machine with no compute driver and no credentials.

Full walkthrough: [`docs/quickstart.md`](docs/quickstart.md).

## Documentation

Start at **[`docs/index.md`](docs/index.md)**.

| Guide | |
|---|---|
| [Concepts](docs/concepts.md) | Board as control plane, operator vs worker, invariants |
| [Quickstart](docs/quickstart.md) | Board-only run, database, MCP connect |
| [Workflow](docs/workflow.md) | Project → plan → Approve → dispatch; triage; park / steer / halt |
| [Agents](docs/agents.md) | OpenShell, providers, Settings / `honr.yaml` |
| [Sandbox](docs/sandbox.md) | How a sandboxed run works and the gotchas that matter |
| [Architecture](docs/architecture.md) | Board / store, supervisor, MCP / REST, beads, persistence |

Operator orientation for agents working *on* this repo:
[`CLAUDE.md`](CLAUDE.md), [`AGENTS.md`](AGENTS.md),
[`.cursor/rules/honr-operator.mdc`](.cursor/rules/honr-operator.mdc).

## Repository layout

| Path | Role |
|---|---|
| `src/store.rs` | The board: the only write path |
| `src/machine.rs` | Legal transitions and lifecycle invariants |
| `src/supervisor.rs` | Dispatch, sandbox lifecycle, lease sweeping |
| `src/mcp.rs` | Operator tools and worker verbs |
| `src/openshell.rs` | Typed wrapper over the OpenShell CLI |
| `sandbox/` | Container image, network policy, metadata shim |
| `web/` | React UI |
| `docs/` | Operator docs |
| `assets/` | Branding |

## Status

honr is under active development and already used to ship changes to itself.
Expect sharp edges: the happy path (board → plan → sandboxed agent → PR) works;
auth, independent verification gates, and a published docs site are still ahead.

See [`docs/concepts.md`](docs/concepts.md) for the invariants we will not break
while the surface grows.

## Contributing

The preferred path is the product itself: open or join a Project on a running
board, let a worker open a PR, review and merge on GitHub.

If you are contributing by hand:

1. Keep mutations on the board path: do not encode lifecycle rules in
   transports (`api.rs` / `mcp.rs`).
2. Run `cargo test` and `cargo clippy --all-targets -- -D warnings` before you
   finish.
3. Stage deliberately (`git add -A` has committed `enabled: true` here before).

## License

License terms are not yet published in this repository. Treat the code as
source-available until a `LICENSE` file lands; ask before redistributing.
