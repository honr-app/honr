# honr docs

Operator-first docs for running and understanding honr.

| Page | What |
|---|---|
| [Concepts](concepts.md) | What honr is: board as control plane, Project + Tasks, operator vs worker |
| [Quickstart](quickstart.md) | Board-only `cargo run`, UI, MCP connect, empty board |
| [Workflow](workflow.md) | Day-to-day: Project → plan → Approve → dispatch; Needs You / Review; park / steer / halt |
| [Agents](agents.md) | Enabling real agents: compute driver, OpenShell, `honr.yaml` / Settings |
| [Sandbox](sandbox.md) | How a sandboxed run works and the gotchas that matter |
| [Cockpit](cockpit.md) | Durable Cockpit seat: start, TTY attach/reconnect, park/stop |
| [Architecture](architecture.md) | One page: Board / store, supervisor, MCP / REST, persistence |
| [Clone targets](task-repo-binding.md) | Clone from task prose; `pull_request` after report; Initial plan via `plan.json` |

Machine contracts (not prose): [`schemas/report.schema.json`](schemas/report.schema.json).

## Building this book

```bash
mdbook serve             # http://localhost:3000  (book.toml at repo root)
mdbook build             # writes to target/mdbook/
# or: make docs / make docs-serve
```

CI publishes `target/mdbook` to [`honr-app/honr-app.github.io`](https://github.com/honr-app/honr-app.github.io) → [honr-app.github.io](https://honr-app.github.io/) via a write **deploy key** (`PAGES_DEPLOY_KEY` on `honr-app/honr`). Org setting **Deploy keys** must stay enabled.
