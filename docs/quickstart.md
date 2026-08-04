# Quickstart

Run the board with no compute driver, no gateway, and no credentials. Agents
are off by default — that is deliberate so honr works on a laptop that only
needs to show and shape work.

## Start the server

```bash
cargo run                 # :8080 — API, SSE, MCP, and the built UI
```

Serves `web/dist` if it exists. For hot reload:

```bash
cd web && npm install && npm run dev
```

`:5173` proxies to `:8080`. `HONR_PORT` overrides the listen port.

## Board database

Board rows live in a SQLx store. **SQLite is the default** (local and offline
tests). **Postgres is optional** for a shared server.

| Source | Example |
|---|---|
| `honr.yaml` → `board.database.url` | `sqlite:honr.db` (default) |
| Env override | `HONR_DATABASE_URL=postgres://honr:honr@127.0.0.1:5432/honr` |

Accepted URL forms:

- SQLite: `sqlite:honr.db`, `sqlite://…`, `sqlite::memory:` (tests)
- Postgres: `postgres://…` or `postgresql://…`

On boot honr opens the URL, applies versioned migrations from `migrations/`,
and restores the board from rows.

**One-shot JSON import:** if the database is empty and `honr.json` exists in
the working directory, honr loads that file into the DB once and leaves the
JSON untouched (archive or delete it yourself). Later boots use the DB only.

Offline `cargo test` always uses SQLite. To exercise Postgres migrations
locally, point `HONR_TEST_DATABASE_URL` at a reachable Postgres URL.

## Connect the operator MCP

honr must already be listening.

**Cursor** — project config is [`.cursor/mcp.json`](../.cursor/mcp.json):

```json
{ "mcpServers": { "honr": { "type": "http", "url": "http://127.0.0.1:8080/mcp" } } }
```

Then **Cursor Settings → Tools & MCP**, enable **honr**, and reload if needed.
With the operator rule in
[`.cursor/rules/honr-operator.mdc`](../.cursor/rules/honr-operator.mdc), the
chat agent drives Projects / Plans via MCP; sandboxed workers claim Ready Tasks
and open PRs.

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

## Empty board

The board starts empty. Nothing claims cards until you enable agents — see
[Agents](agents.md). You can still create Projects, inspect columns in the UI,
and exercise MCP tools that do not need a sandbox.

Next: [Workflow](workflow.md) for the day-to-day loop.
