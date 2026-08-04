# Architecture

One page on how the pieces fit. Present tense; code paths, not history.

## One state machine

Every mutation: UI, MCP, supervisor: goes through `Board` in `src/store.rs`.
Legal transitions and lifecycle invariants live in `src/machine.rs`. Transports
(`api.rs`, `mcp.rs`, SSE) render and invoke; they do not own rules.

```
UI / MCP / supervisor
         │
         ▼
      Board (store.rs) ── persistence (SQLx) ── event bus (SSE)
         │
         ├── machine.rs   legal transitions
         ├── model.rs     Project + Task node type
         └── beads.rs     identity / parent / deps mirror
```

## Layout

| Path | What |
|---|---|
| `src/model.rs` | One node type. Project (container) + Task (claimable leaf). |
| `src/machine.rs` | Legal transitions + lifecycle invariants. |
| `src/store.rs` | The board: state, persistence, event bus, derived reads. |
| `src/beads.rs` | `bd` CLI wrapper: identity, Project→Task parent, deps. |
| `src/api.rs` `src/sse.rs` | The human face (REST + SSE). |
| `src/mcp.rs` | Operator tools and worker verbs. |
| `src/openshell.rs` | Typed async wrapper over the `openshell` CLI; every call has a deadline. |
| `src/supervisor.rs` | Dispatch, per-card sandbox lifecycle, briefing, lease sweeping. |
| `honr.yaml` | Level schema (Project + Task) and execution config. |
| `sandbox/` | Container image, network policy, metadata shim. |
| `web/` | React UI + Playwright screenshot harness. |
| `migrations/` | Versioned SQLx migrations for the board store. |

## Supervisor

When agents are enabled, the supervisor:

1. Health-checks the OpenShell gateway.
2. Auto-enqueues claimable Backlog leaves under Projects with auto mode on.
3. Claims the oldest `awaiting_dispatch` card within concurrency limits.
4. Creates (or reuses) a sandbox, uploads the shim, builds a briefing from the
   Project→Task chain, and starts the agent detached.
5. Parses the output stream for liveness; calls `heartbeat` / `report` on the
   board's behalf.
6. Sweeps expired leases; on startup, reconciles live sandboxes so a honr
   restart does not orphan a running agent.

The agent has no network path to honr. The supervisor is the only caller of
worker verbs on the live path.

## MCP and REST

| Face | Transport | Audience |
|---|---|---|
| Operator + worker tools | MCP streamable HTTP at `/mcp` | Chat agents on the host; supervisor for worker verbs |
| Human UI | REST + SSE | React app; one-tap answers and approvals |

Steer, pin, park, halt, and cut scope want a reason. They live in MCP. What
stays one-tap in the UI is answering an escalation and approving a review.

## Beads mirror

Beads (`bd`) holds issue identity and the dependency graph. honr dual-writes
board mutations into beads so Projects / Tasks stay addressable outside the
board DB. Sandboxes get a mirrored `BEADS_DIR`. Beads sync
(`refs/dolt/data`) is separate from git code refs. See the beads docs linked
from `AGENTS.md`.

## Persistence

SQLx board store (SQLite default, Postgres optional). Configured via
`board.database.url` or `HONR_DATABASE_URL`. Mutations flush as row updates.
Optional one-shot import from legacy `honr.json` when the DB is empty. See
[Quickstart](quickstart.md).

## Related

- [Concepts](concepts.md): product model and invariants
- [Agents](agents.md): enabling the execution path
- [Sandbox](sandbox.md): sandbox stack and gotchas
