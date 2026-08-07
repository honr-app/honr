# Configuration

Where each knob lives, and which ones are read once at startup.

Three layers:

| Layer | Role |
|---|---|
| **`honr.yaml`** | Boot essentials only: board database URL, level schema, sweep timing, and the `agents.enabled` process gate. Read once at startup. |
| **Compiled defaults** | Empty sandbox-spec catalogs and unset Agent runtime seed from built-in constants (`AgentConfig::default`, `src/seed_policies.rs`). |
| **Board DB + Settings / API** | Live source of truth for sandbox specs, Agent runtime, OpenShell gateway/providers, Forge, and GitHub App. Edit in the UI or via REST. |

Changing `honr.yaml` after a board already has the corresponding rows does not rewrite those rows. Live create knobs (image, policy, cpu, memory, engine) and Agent runtime process settings are board-owned after seed.

## Board database

Board rows live in a SQLx store. **SQLite is the default**; Postgres is
optional, for a shared server.

| Source | Example |
|---|---|
| `honr.yaml` → `board.database.url` | `sqlite:honr.db` (default) |
| Environment override | `HONR_DATABASE_URL=postgres://honr:honr@127.0.0.1:5432/honr` |

Accepted forms:

- SQLite — `sqlite:honr.db`, `sqlite://…`, `sqlite::memory:` (tests)
- Postgres — `postgres://…` or `postgresql://…`

On boot honr opens the URL, applies versioned migrations from `migrations/`, and
restores the board from rows.

**One-shot JSON import:** if the database is empty and `honr.json` exists in the
working directory, honr imports it once and leaves the JSON alone — archive or
delete it yourself. Later boots use the database only.

Offline `cargo test` always uses SQLite. To exercise Postgres migrations
locally, point `HONR_TEST_DATABASE_URL` at a reachable Postgres URL.

## Environment

| Variable | Effect |
|---|---|
| `HONR_PORT` | Listen port (default 8080) |
| `HONR_DATABASE_URL` | Overrides `board.database.url` |
| `HONR_MCP_URL` | Resource URL minted into cockpit MCP tokens. Defaults to `http://host.docker.internal:8080/mcp` |
| `HONR_TEST_DATABASE_URL` | Postgres URL for migration tests |

One host secret file: `~/.config/honr/master.key`, which seals credentials
stored on the board.

## `honr.yaml`

Boot-only knobs. A typical file:

```yaml
board:
  database:
    url: sqlite:honr.db

levels:
  - name: Project
    horizon: 2q
    owner: human
    elaborate: on_commit
  - name: Task
    horizon: 1d
    owner: agent
    claimable: true
    requires: [definition_of_done]

execution:
  sweep_interval_ms: 2000
  agents:
    enabled: false          # see: Your first agent
```

| Field | Why it is set that way |
|---|---|
| `board.database` | Where the board persists. Overridable with `HONR_DATABASE_URL`. |
| `levels` | Project + Task schema for this install. |
| `sweep_interval_ms` | How often the supervisor checks overdue run deadlines. |
| `agents.enabled` | **Off by default.** Process boot gate — read once at startup. Turning it on spends real money. On a fresh board it also seeds Settings → Agent runtime; after that, Settings owns the durable toggle (restart still required when enabling a process that started disabled). |

Optional `execution.agents` fields that older files may still carry (`engine`,
`image`, `policy`, `cpu`, `memory`, concurrency, timeouts, `repo`,
`branch_prefix`) parse for compatibility. Empty-catalog seed and last-resort
create knobs use compiled defaults instead; live edits stay on the board.

## Sandbox specs

A sandbox spec is the recipe for a sandbox: image, network policy, CPU, memory,
engine, attached providers. They live on the board and are edited in
**Settings → OpenShell → Sandbox specs** (REST: `/api/sandbox-profiles`).

Policy is **inline YAML text stored on the board**. At create time the
supervisor writes it to a temp file for OpenShell's `--policy` flag.

### Which spec a card gets

Resolved in this order:

1. **Project override** — `sandbox_profile_id` on the containing Project, if set
   and present in the catalog
2. **Global default** — `default_sandbox_profile_id` on durable board state
3. **Compiled-default fallback** — `AgentConfig::default()` image / policy / cpu /
   memory / engine (same constants that seed an empty catalog)

An empty catalog is seeded with two specs:

| Spec | For | Egress |
|---|---|---|
| `default` | Card workers | Inference, GitHub, package registries. **No** honr MCP. |
| `cockpit` | The cockpit seat | honr MCP, inference, GitHub. **No** package registries. |

The global default stays the worker spec. Boards that already had a worker
catalog get `cockpit` added at boot if it is missing. After seed, edit the
board specs — changing the compiled seed text in `src/seed_policies.rs` updates
the next empty catalog, not an existing board.

## Engines

Which agent CLI runs in a seat. It is a field on the sandbox spec; when a spec
omits it, claim falls back to **Settings → Agent runtime**. Per-card overrides
are ignored.

| Id | Launch | Resume |
|---|---|---|
| `cursor` | `agent … --output-format stream-json` | `--resume` |
| `agy` | `agy … --output-format stream-json` | `--conversation` |
| `claude` | `claude --bare -p … --output-format stream-json` | (none) |
| `opencode` | `opencode run --format json --auto` | `--session` |

The registry in `src/engine.rs` is explicit: an unknown id fails loudly rather
than silently falling through to Claude.

Claude and OpenCode use OpenShell `inference.local` via `ANTHROPIC_BASE_URL`.
`agy` uses the `antigravity` provider type. Both are covered in
[Sandbox](sandbox.md).

## Providers

**Settings → OpenShell → Providers** holds the desired list with credentials
sealed. That list is the source of truth; **Sync** applies it to the gateway
(`POST /api/openshell/providers/sync`), and Save also applies when the gateway is
reachable. Providers marked **attach** are passed on sandbox create.

Provider `github` is owned by **Settings → GitHub App** — installation tokens
sync in as `GH_TOKEN`. Do not hand-edit that provider's credentials.

## Forge and webhooks

**Settings → Forge** configures the forge provider and an optional **webhook
polling fallback**: honr polls GitHub on an interval *in addition to* webhooks
(default 60s, minimum 15s) using the App installation token. Same board effects
either way — merge → Done, main-advanced, and submitted PR review feedback
(`CHANGES_REQUESTED` / `COMMENT` → pointer steer + Backlog). Needs a configured
GitHub App and installation id.

Webhook ingress is `POST /api/webhooks/github`. See
[Workflow](workflow.md#when-main-moves) for what a push does, and
[PR review feedback](workflow.md#pr-review-feedback) for submitted reviews.
