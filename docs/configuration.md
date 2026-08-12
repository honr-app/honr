# Configuration

honr stacks configuration in layers. Lower layers are operator concerns; upper
layers are what agents read at claim time. See also
[Workflow](workflow.md#standing-instructions-and-quality-gates) and
[Concepts](concepts.md#configuration-layers).

| Layer | Who sets it | Role |
|---|---|---|
| **Process boot** | Host / deploy | Database URL (`HONR_DATABASE_URL` else `sqlite:honr.db`). Hierarchy is compile-time Project + Task. |
| **Board Settings** | Operator | Policies, sandbox specs, Agent runtime (engine, concurrency, timeouts, sweep interval), OpenShell gateway/providers (incl. shipped `github-app`), and Forge. |
| **Project fields** | Operator | Default clone repo (`clone_repo`), optional sandbox spec override (`sandbox_profile_id`). Seeded into Project intent and the Initial plan. |
| **`project_prompt`** | Operator | Standing agent policy for the Project — escalation rules, clone-target protocol, plan/split/report paths, and where to name **quality gates**. Seeded from [`DEFAULT_PROJECT_PROMPT`](../src/model.rs) on create; editable per Project. |
| **Per-card intent / DoD** | Operator (per Task) | Card-specific work: clone target (`owner/name`), card-local gates, and the operational proof. Notes can override at claim time. |

**Boot, Settings, and Project fields are operator concerns — not `project_prompt`.**
Do not put database URLs, Policy YAML, or sandbox spec ids in `project_prompt`.
Agents inherit `project_prompt` on every claim; the supervisor assembles it into
the briefing ahead of the Plan and the card's own intent/DoD.

**Quality gates** — test/lint commands agents should run before publish — belong
in `project_prompt` when they apply Project-wide. Name the commands explicitly
(`cargo test`, `npm test`, …). honr does **not** assume `cargo` or any other
toolchain unless `project_prompt` (or a card's definition of done) names it.
Card-specific gates can live in that card's DoD instead.

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
| `HONR_BIND_ADDR` | Bind host (default `127.0.0.1`; containers use `0.0.0.0`) |
| `HONR_DATABASE_URL` | Board database URL (default `sqlite:honr.db`) |
| `HONR_TEST_DATABASE_URL` | Postgres URL for migration tests |

Cockpit's shipped `honr` MCP entry is stdio over a local Unix socket
(`socat`, see [Cockpit](cockpit.md#how-the-mcp-relay-works)) — no URL, no env var.

One host secret file: `~/.config/honr/master.key`, which seals credentials
stored on the board.

## Hierarchy

Project + Task is fixed in code (`schema::default_levels`). There is no
install-time level ladder to configure.

## Project fields and `project_prompt`

When you create a Project (board UI, REST `POST /api/items`, or MCP
`create_project`):

| Field | Stored on | Purpose |
|---|---|---|
| `clone_repo` | Project intent | Default `owner/name` for the Initial plan and for Tasks that omit an explicit clone line. Required on create. |
| `sandbox_profile_id` | Project row | Optional override of the board default sandbox spec. Unset means inherit Settings. |
| `project_prompt` | Project row | Standing instructions every worker sees. Defaults to compiled `DEFAULT_PROJECT_PROMPT` when omitted. |

`project_prompt` is **not** a substitute for Settings or Project fields. Keep
boot-time config, OpenShell Policies, sandbox specs, and `clone_repo` where
they belong. Use `project_prompt` for rules agents need on every card —
invariants, escalation, naming clone targets in Task prose, the
`plan.json` / `split.json` / `report.json` protocol, and Project-wide quality
gates.

Per-card **intent** and **definition of done** carry the card's clone target
and any gates that apply to that card only. The supervisor never invents gates;
it points agents at `project_prompt` and the card DoD.

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

Four specs come seeded — `sandbox-cursor`, `sandbox-agy`, `sandbox-claude`,
`sandbox-opencode` — one per split `quay.io/honr-app/sandbox-<engine>` image
([Sandbox](sandbox.md#image-and-offline-gates)), each already wired to a
matching minimal Cockpit policy with honr MCP attached. Editing a seeded row
sticks; the seed only inserts what's missing.

### Which spec a card gets

Resolution order is documented in [Sandbox](sandbox.md). Create-form defaults
select the seeded minimal policy; attach providers and pick the policy the run
needs.

### Cockpit

Cockpit uses the global default sandbox spec unless you set an explicit Cockpit
profile under Sandbox specs. A fresh board seeds all four specs but picks none
of them as default — that choice is an onboarding step (Welcome flags it red
until you set one). Pick a seeded spec (or one you made) and click **Set
default**, or **Use for Cockpit** to give Cockpit its own engine. That spec's
`policy_id` is what the sandbox gets at create.

## OpenShell / Forge / GitHub App provider

Connectivity, providers (including the shipped `github-app` type that mints
`GH_TOKEN`), provider types, Policies, Sandbox specs, and Forge poll are board
Settings — see the Settings UI and [Your first agent](first-agent.md).
