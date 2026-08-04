# honr docs

Operator-first docs for running and understanding honr. Flat pages with stable
slugs: ready to map to a docs site later.

| Page | What |
|---|---|
| [Concepts](concepts.md) | What honr is: board as control plane, Project + Tasks, operator vs worker |
| [Quickstart](quickstart.md) | Board-only `cargo run`, UI, MCP connect, empty board |
| [Workflow](workflow.md) | Day-to-day: Project → plan → Approve → dispatch; Needs You / Review; park / steer / halt |
| [Agents](agents.md) | Enabling real agents: compute driver, OpenShell, providers, `honr.yaml` / Settings |
| [Sandbox](sandbox.md) | How a sandboxed run works and the gotchas that matter |
| [Architecture](architecture.md) | One page: Board / store, supervisor, MCP / REST, beads, persistence |
| [Task repo binding](task-repo-binding.md) | Plan: task-scoped remotes, init_plan, stop unbound Initial plan auto-seed |
| [Sandbox without beads](sandbox-without-beads.md) | Plan: remove bd/image/upload from agent sandboxes; briefing-only context |

Machine contracts (not prose): [`schemas/report.schema.json`](schemas/report.schema.json).
