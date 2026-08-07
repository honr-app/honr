# Configuration

Where each knob lives, and which ones are process-boot vs board-owned.

Two layers:

| Layer | Role |
|---|---|
| **Process boot** | Database URL (`HONR_DATABASE_URL` else `sqlite:honr.db`). Hierarchy is compile-time Project + Task. |
| **Board DB + Settings / API** | Live source of truth for sandbox specs, Agent runtime (engine, concurrency, timeouts, sweep interval), OpenShell gateway/providers (incl. shipped `github-app`), and Forge. |

## Board database

Board rows live in a SQLx store. **SQLite is the default**; Postgres is
optional, for a shared server.

| Source | Example |
|---|---|
| Compiled default | `sqlite:honr.db` |
| Environment override | `HONR_DATABASE_URL=postgres://honr:honr@127.0.0.1:5432/honr` |

Accepted forms:

- SQLite — `sqlite:honr.db`, `sqlite://…`, `sqlite::memory:` (tests)
- Postgres — `postgres://…` or `postgresql://…`

On boot honr opens the URL, applies versioned migrations from `migrations/`, and
restores the board from rows.

The database URL cannot live in board Settings — Settings persist *inside* the
database.

**One-shot JSON import:** if the database is empty and `honr.json` exists in the
working directory, honr imports it once and leaves the JSON alone — archive or
delete it yourself. Later boots use the database only.

Offline `cargo test` always uses SQLite. To exercise Postgres migrations
locally, point `HONR_TEST_DATABASE_URL` at a reachable Postgres URL.

## Environment

| Variable | Effect |
|---|---|
| `HONR_PORT` | Listen port (default 8080) |
| `HONR_DATABASE_URL` | Board database URL (default `sqlite:honr.db`) |
| `HONR_MCP_URL` | Resource URL minted into cockpit MCP tokens. Defaults to `http://host.docker.internal:8080/mcp` |
| `HONR_TEST_DATABASE_URL` | Postgres URL for migration tests |

One host secret file: `~/.config/honr/master.key`, which seals credentials
stored on the board.

## Hierarchy

Project + Task is fixed in code (`schema::default_levels`). There is no
install-time level ladder to configure.

## Agent runtime

**Settings → Agent runtime** (REST: `/api/agent-runtime`): default engine,
concurrency, agent timeout, max attempts, branch prefix, and sweep interval.
Empty boards seed from compiled defaults; edits persist on the board. The
supervisor always starts dispatch and cockpit; OpenShell gateway + a sandbox
spec are the practical readiness gates.

## Sandbox specs

A sandbox spec is the recipe for a sandbox: image, network policy, CPU, memory,
engine, attached providers. They live on the board and are edited in
**Settings → OpenShell → Sandbox specs** (REST: `/api/sandbox-profiles`).

Policy is **inline YAML text stored on the board**. At create time the
supervisor writes it to a temp file for OpenShell's `--policy` flag.

### Which spec a card gets

Resolution order is documented in [Sandbox](sandbox.md). Create-form defaults
use a minimal policy (`src/seed_policies.rs`); operators add egress as needed.

### Cockpit

Cockpit uses the global default sandbox spec unless you set an explicit Cockpit
profile under Sandbox specs.

## OpenShell / Forge / GitHub App provider

Connectivity, providers (including the shipped `github-app` type that mints
`GH_TOKEN`), provider types, and Forge poll are board Settings — see the
Settings UI and [Your first agent](first-agent.md).
