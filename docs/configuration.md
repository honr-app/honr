# Configuration

Where each knob lives, and which ones are read once at startup.

Two sources, and the split matters: **`honr.yaml`** is read at boot and seeds
empty state; **Settings** (stored on the board) is the live source of truth once
anything exists. Editing the YAML does not change a board that already has the
corresponding rows.

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
    engine: cursor
    image: honr-sandbox:latest
    policy: embedded
    repo:
      upstream: honr-app/honr
      fork: honr-app/honr
      base: main
    cpu: "2"
    memory: 4Gi
    max_concurrent: 1
    agent_timeout_secs: 1800
    max_attempts: 3
```

The ones worth understanding:

| Field | Why it is set that way |
|---|---|
| `agents.enabled` | **Off by default.** Read once at startup — there is no runtime toggle. Turning it on spends real money. |
| `max_concurrent` | Sandboxes are heavy and OpenShell is alpha. Do not start at seven. |
| `agent_timeout_secs` | The one run clock. `lease_secs` / `heartbeat_expect_secs` are ignored if present, kept only so older YAML still loads. |
| `max_attempts` | Runs that may die without producing work before the card becomes a human's problem. Without a cap, early failures requeue every lease period forever. |
| `policy: embedded` | Seeds the worker spec from the built-in default **when the catalog is empty**. There is no host `policy.yaml` to edit. |
| `repo` | Default remotes for a card that already has `pull_request` facts. The first clone still follows the card's prose. |
| `branch_prefix` | Stem for branch and sandbox names. Defaults to `honr` → `honr/card-N`, `honr-card-N-a1`. |

## Sandbox specs

A sandbox spec is the recipe for a sandbox: image, network policy, CPU, memory,
engine, attached providers. They live on the board and are edited in
**Settings → OpenShell → Sandbox specs** (REST: `/api/sandbox-profiles`).

Policy is **inline YAML text stored on the board**, not a file path. At create
time the supervisor writes it to a temp file for OpenShell's `--policy` flag.

### Which spec a card gets

Resolved in this order:

1. **Project override** — `sandbox_profile_id` on the containing Project, if set
   and present in the catalog
2. **Global default** — `default_sandbox_profile_id` on durable board state
3. **YAML fallback** — `execution.agents` `image` / `policy` / `cpu` / `memory`
   / `engine`

An empty catalog is seeded with two specs:

| Spec | For | Egress |
|---|---|---|
| `default` | Card workers | Inference, GitHub, package registries. **No** honr MCP. |
| `cockpit` | The cockpit seat | honr MCP, inference, GitHub. **No** package registries. |

The global default stays the worker spec. Boards that already had a worker
catalog get `cockpit` added at boot if it is missing.

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
