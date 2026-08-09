# Configuration

What is set at process start vs in board Settings.

| Layer | Role |
|---|---|
| **Process boot** | Database URL (`HONR_DATABASE_URL` else `sqlite:honr.db`). Hierarchy is compile-time Project + Task. |
| **Board DB + Settings / API** | Live source of truth for Policies, sandbox specs, Agent runtime (engine, concurrency, timeouts, sweep interval), OpenShell gateway/providers (incl. shipped `github-app`), and Forge. |

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
| `HONR_TEST_DATABASE_URL` | Postgres URL for migration tests |

Cockpit's shipped `honr` MCP entry is stdio over a local Unix socket
(`socat`, see [Cockpit](cockpit.md#how-the-mcp-relay-works)) — no URL, no env var.

One host secret file: `~/.config/honr/master.key`, which seals credentials
stored on the board.

## Hierarchy

Project + Task is fixed in code (`schema::default_levels`). There is no
install-time level ladder to configure.

## Agent runtime

**Settings → Agent runtime** (REST: `/api/agent-runtime`): default engine,
concurrency, agent timeout, max attempts, branch prefix, and sweep interval.
Fresh boards use built-in defaults; edits persist on the board. OpenShell
gateway + a sandbox spec are the practical readiness gates before dispatch does
anything useful.

## Policies

A **Policy** is a named OpenShell YAML allow-list (filesystem / network). The
catalog lives on the board and is edited in **Settings → OpenShell → Policies**
(REST: `/api/openshell/policies`). Empty boards seed a minimal row from
`src/seed_policies.rs`; operators add egress there as needed.

Live policy always comes from this board catalog. At sandbox create the
supervisor resolves the selected policy to YAML for OpenShell. Policy is
**fixed for that sandbox's life** for filesystem and process sections —
recreate the sandbox after a change.

## Sandbox specs

A sandbox spec is the recipe for a sandbox: image, CPU, memory, engine,
attached providers, and a **reference to a named Policy** (`policy_id`). Specs
live on the board and are edited in **Settings → OpenShell → Sandbox specs**
(REST: `/api/sandbox-profiles`). Upsert requires a known `policy_id`; you edit
allow-list YAML under Policies, not on the spec.

### Which spec a card gets

Resolution order is documented in [Sandbox](sandbox.md). Create-form defaults
select the seeded minimal policy; attach providers and pick the policy the run
needs.

### Cockpit

Cockpit uses the global default sandbox spec unless you set an explicit Cockpit
profile under Sandbox specs. That spec's `policy_id` is what the sandbox gets at
create.

## OpenShell / Forge / GitHub App provider

Connectivity, providers (including the shipped `github-app` type that mints
`GH_TOKEN`), provider types, Policies, Sandbox specs, and Forge poll are board
Settings — see the Settings UI and [Your first agent](first-agent.md).
