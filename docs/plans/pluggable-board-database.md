# Plan: Pluggable board database

## Context & Objectives

Today the board is an in-memory `BoardState` (`BTreeMap` of items + stories)
loaded from and flushed to a single `honr.json` file. Every index the control
plane needs — backlog, dispatch queue, lease sweep, parent/child, blockers,
snapshot/digest — is a linear scan. Every durable mutation eventually rewrites
the whole blob.

This Project replaces that persistence with a **pluggable SQL store** (SQLx:
SQLite default, Postgres optional) behind `Board`, without moving state-machine
rules out of `machine.rs` and without absorbing beads/Dolt.

### Project definition of done

- Config can select SQLite or Postgres.
- Board boots from DB; mutations are durable without whole-file JSON rewrite.
- `list_*` / snapshot / lease sweep use indexed queries (or denormalized columns).
- Existing `honr.json` is importable once.
- Claude / agy / cursor agent paths and beads dual-write stay unchanged.
- Tests green offline on SQLite.

### Explicit non-goals

| Stay in-process | Out of scope |
|---|---|
| `machine.rs` transitions | Beads / Dolt as board storage |
| SSE / WebSocket event fan-out | Multi-process board writers |
| Supervisor run maps | Replacing integer `ItemId` |
| `agent_logs` ring buffers (`#[serde(skip)]` today) | Capability routing / claimable schema fixes |

---

## Inventory: hot Board scans (today)

All of these walk `BoardState.items` (and sometimes `stories`) under the
`RwLock`. Persistence is orthogonal but every path below becomes a SQL query
target.

| Call site | What it scans | Why it hurts |
|---|---|---|
| `list_backlog` / `list_ready` | All items: `state == Backlog`, not Project, no children, unresolved blockers, capability | Dispatch operator + MCP; O(n) per poll |
| `list_awaiting_dispatch` | Backlog + `awaiting_dispatch` + not parked + leaf + unblocked; sort by `entered_state_at` | Supervisor drain every tick |
| `sweep_leases` | Claimed/Running where `run_deadline_at` (or legacy lease) expired | Every `sweep_interval_ms` |
| `children_of` / `has_children` | All items with `parent == id` | Called inside backlog/dispatch filters and snapshot |
| `goal_of` / `depth` / `chain` | Walk `parent` pointers (per item in snapshot/digest) | Snapshot clones every item then re-derives goals |
| `populate_blockers` / `unresolved_blockers` | Resolve `blocked_by` ids + filter unresolved | Emit path + list filters |
| `snapshot` | Clone all items + blockers; build every Project `GoalView` via member scans | REST `/api/board` + UI |
| `digest` | Per-goal member scans for NeedsYou / running / backlog / review counts | Operator digest |
| `list_awaiting_rebase` | Review cards with rebase flags | Webhook catch-up sibling |
| `flush` | `serde_json::to_string_pretty` of entire `BoardState` → rename | Interval + shutdown; write amplification |

`Board::emit` marks `dirty`; a 500ms timer calls `flush`. Heartbeats that only
touch progress still force a full rewrite once dirty.

---

## Architectural shape

```
honr.yaml / env
    │  board.database.url = "sqlite:honr.db" | "postgres://…"
    ▼
Board  (facade: machine verbs, emit, beads hooks)
    │
    ├── in-process: seq, event ring, agent_logs, openshell, beads client
    │
    └── dyn BoardStore   ◄── SQLx
            ├── SqliteStore   (default; offline tests)
            └── PostgresStore (ops / multi-host later)
```

**One write path.** Mutations still go through `Board` → `machine` → store.
Transports (`api`, `mcp`, supervisor) must not grow SQL.

**Schema sketch** (exact columns land in the migrations Task):

- `meta` — `next_id`, schema version / import stamp
- `items` — scalar + JSON blobs for nested structs (`lease`, `escalation`,
  `plan`, `proposal`, `notes`, `history`, `gates`, …) where indexing is not
  needed; **indexed columns** at minimum: `id`, `parent_id`, `state`,
  `level`, `awaiting_dispatch`, `parked`, `run_deadline_at`,
  `entered_state_at`, `capability`, `rebase_requested`
- `item_blockers` — `(item_id, blocker_id)` for graph queries
- `stories` — `(goal_id, at, text)` ordered append

Denormalize only when a filter is hot and JSON extraction is awkward
(e.g. “has unresolved blockers” → maintain a `blocker_open_count` or query the
edge table with a status join). Prefer indexed columns over a second cache map
in Rust.

**JSON import (once):** on boot, if the configured DB is empty and `honr.json`
exists, load `BoardState`, insert rows, write an import marker, leave the JSON
file untouched (operator may archive). No ongoing dual-write to JSON.

**Tests:** keep using SQLite (`sqlite::memory:` or temp file) under
`cargo test --offline`. Postgres is documented and compile/feature-tested; CI
stays offline.

---

## Tasks & Dependencies

```
[t1: SQLx migrations + config] ──► [t2: SQLite BoardStore + JSON import]
                                              │
                                              ▼
                                   [t3: Indexed query cutover]
                                              │
                                              ▼
                                   [t4: Postgres URL + docs]
```

### Task 1: SQLx migrations and board database config
- **Key**: `t1`
- **Intent**: Add SQLx (SQLite + Postgres features), versioned migrations for
  items/stories/blockers/meta, and `honr.yaml` / env config that selects a
  database URL. Introduce a `BoardStore` trait (or module boundary) without
  cutting `Board` over yet.
- **Definition of Done**: Migrations apply cleanly to SQLite offline; config
  parses `sqlite:…` and `postgres://…`; trait/API sketched and wired enough
  that later Tasks compile against it; `cargo test --offline --locked` and
  `cargo clippy --offline -- -D warnings` pass; no change to agent engines or
  beads dual-write.

### Task 2: SQLite BoardStore and one-shot JSON import
- **Key**: `t2`
- **Blocked By**: `t1`
- **Intent**: Implement the SQLite store so `Board` boots from the DB and
  mutations persist as row updates (no whole-file `honr.json` rewrite). Support
  one-shot import when the DB is empty and `honr.json` is present. Keep
  `agent_logs`, event seq/buffer, SSE/WS, and `machine.rs` in-process.
- **Definition of Done**: Fresh boot creates/opens SQLite; import populates
  from existing JSON once; create/transition/steer/report survive process
  restart via DB; flush no longer rewrites the full JSON blob as the primary
  store; offline tests cover import + round-trip; beads hooks and
  claude/agy/cursor paths unchanged.

### Task 3: Indexed query cutover for list/snapshot/lease paths
- **Key**: `t3`
- **Blocked By**: `t2`
- **Intent**: Replace linear scans in `list_backlog` / `list_ready` /
  `list_awaiting_dispatch`, `sweep_leases`, parent/child helpers used on those
  paths, blocker resolution, and snapshot/digest aggregation with indexed SQL
  (or denormalized columns maintained on write).
- **Definition of Done**: Each listed hot path has a query plan that uses
  indexes (or documented denormalized fields) rather than loading all items
  into a `BTreeMap` scan; unit tests assert filter semantics match today’s
  behavior; `cargo test --offline --locked` green on SQLite.

### Task 4: Postgres backend URL and operator docs
- **Key**: `t4`
- **Blocked By**: `t3`
- **Intent**: Run the same migrations and store semantics against Postgres via
  configured URL; document selection, import, and “SQLite default / Postgres
  optional” in `docs/operating.md` (and point at this plan). Default remains
  SQLite for local and offline tests.
- **Definition of Done**: Postgres URL accepted in config; migrations apply
  against Postgres when available; docs describe URL forms and import behavior;
  offline CI/tests still use SQLite only; agent and beads paths unchanged.

---

## Standing invariants

- **One state machine.** Legal transitions stay in `machine.rs` / `Board`
  verbs — the store is material, not a second rule engine.
- **Agent is not a participant.** No network path from sandbox agents to the
  board DB; supervisor still claims/heartbeats/reports.
- **Merging is human.** Persistence changes do not auto-merge PRs.
- **Beads dual-write unchanged.** Identity/graph remains `.beads` / Dolt;
  board SQL does not replace it.
