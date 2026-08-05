# Quickstart

Run the board with no compute driver, no gateway, and no credentials. Agents
are off by default: that is deliberate so honr works on a laptop that only
needs to show and shape work.

## Start the server

```bash
cargo run                 # :8080 (API, SSE, MCP, and the built UI)
make dev                  # same, but cargo-watch rebuilds/restarts on Rust changes
```

Serves `web/dist` if it exists. For UI hot reload (pair with `cargo run` or `make dev`):

```bash
make dev-ui               # or: cd web && npm install && npm run dev
```

`:5173` proxies to `:8080`. `HONR_PORT` overrides the listen port. `make dev`
needs [`cargo-watch`](https://crates.io/crates/cargo-watch)
(`cargo install cargo-watch` or `brew install cargo-watch`).

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

honr must already be listening. After local admin bootstrap, `/mcp` requires
MCP OAuth 2.1 (Bearer). Clients discover the authorization server from
`/.well-known/oauth-protected-resource` and open a browser login/consent that
reuses the same admin / GitHub allowlist as the board UI.

**Cursor**: project config is [`.cursor/mcp.json`](../.cursor/mcp.json):

```json
{
  "mcpServers": {
    "honr": {
      "type": "http",
      "url": "http://127.0.0.1:8080/mcp",
      "auth": { "CLIENT_ID": "honr-cursor", "scopes": ["mcp"] }
    }
  }
}
```

`CLIENT_ID` `honr-cursor` is a built-in public client (no secret). Authenticate
from the CLI:

```bash
agent mcp login honr
```

That opens a browser for board login + consent. Or use **Cursor Settings →
Tools & MCP → Authenticate**. Reload if the tools list stays empty. With the
operator rule in
[`.cursor/rules/honr-operator.mdc`](../.cursor/rules/honr-operator.mdc), the
chat agent drives Projects / Plans via MCP; sandboxed workers claim Ready Tasks
and open PRs.

**Claude Code:**

```bash
claude mcp add --transport http honr http://localhost:8080/mcp
```

Complete the OAuth browser flow when Claude prompts for authorization.

## Empty board

The board starts empty. Nothing claims cards until you enable agents. See
[Agents](agents.md). You can still create Projects (`create_project` auto-seeds
Initial plan), dispatch that card, inspect columns in the UI, and exercise MCP
tools that do not need a sandbox. Help in the sidebar documents the same loop.

Next: [Workflow](workflow.md) for the day-to-day loop.
