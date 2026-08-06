<p align="center">
  <img src="assets/honr-logo.png" alt="honr, the one that hones" width="200" />
</p>

<h1 align="center">honr</h1>

<p align="center">
  <strong>The board that hones the work.</strong><br />
  An agent orchestrator whose board is a control plane, not a status report.
</p>

<p align="center">
  <a href="https://honr-app.github.io/"><img alt="Docs" src="https://img.shields.io/badge/docs-honr--app.github.io-3d7ea6?style=flat-square" /></a>
  <a href="Cargo.toml"><img alt="Rust" src="https://img.shields.io/badge/rust-stable-dea584?style=flat-square&logo=rust&logoColor=white" /></a>
  <a href="#status"><img alt="Status" src="https://img.shields.io/badge/status-active-2ea44f?style=flat-square" /></a>
</p>

---

**honr** is a board you point at a repository. You describe what you want; it
runs coding agents in sandboxes; pull requests come back for you to merge.

The board is not a report of work happening elsewhere. It *is* the work. Moving
a card starts an agent. Answering a question unblocks one. Approving a plan
creates the tasks.

<p align="center">
  <img src="https://honr-app.github.io/images/desktop-board.png" alt="The honr board: Backlog, Running, Needs You, Review, Done" width="900" />
</p>

**📖 [Read the docs →](https://honr-app.github.io/)** — start with the
[Tour](https://honr-app.github.io/tour.html), which walks one card's life with
screenshots and needs nothing installed.

## Name

**honr** means **honer**: the one that hones.

To **hone** *(v.)* is to refine: to make a skill, idea, or technique better and
more effective through practice and time. honr is built for that loop. Intent
in, agents against real repositories in sandboxed compute, pull requests out, so
the work gets sharper every turn under human judgement.

## What makes it different

- **One board, one state machine.** The UI and the MCP API are two faces of the
  same lifecycle. There is no separate "apply" step, because there is nowhere
  else for the work to live.
- **Operator and worker, separated.** You shape Projects and Plans over MCP.
  Workers run inside OpenShell sandboxes with **no network path back to honr**
  — an agent that could reach the board could approve its own review.
- **PRs, not merges.** Approving in honr surfaces the pull request. A human
  merges on GitHub. There is no autonomy dial that changes this.
- **Liveness is observed, never self-reported.** Parsed out of the agent's
  output stream, because a keepalive timer asserts liveness without evidence.

**honr builds honr.** Cards against this repository are claimed by sandboxed
agents and land as PRs you review like any other contribution.

```
you ──chat──> operator agent (Cursor / Claude Code)
                    │ MCP (streamable HTTP, /mcp)
                    ▼
            ┌────────────────────┐
            │  honr (Rust/axum)  │◀── REST + SSE ── React UI
            │  one state machine │
            └────────────────────┘
                    ▲
              supervisor ──> worker agent in an OpenShell sandbox
                                 └─> same-repo PR ──> you merge
```

## Quick start

**Requirements:** a current Rust stable toolchain, and a recent Node.js if you
want to build the UI.

```bash
git clone https://github.com/honr-app/honr.git
cd honr
cargo run                 # http://127.0.0.1:8080  (API, SSE, MCP, UI)
```

The board asks you to create an admin on first open, then starts empty. Nothing
claims cards until you enable agents — deliberately, so the control plane runs
on a machine with no compute driver and no credentials.

Full walkthrough: **[Quickstart](https://honr-app.github.io/quickstart.html)**.
Turning on real sandboxed agents:
**[Your first agent](https://honr-app.github.io/first-agent.html)**.

```bash
make run                  # debug API + web/dist, then serve
make dev                  # watchexec rebuild/restart on Rust changes
make dev-ui               # Vite on :5173 (proxies to :8080)
make docs-serve           # this book, at :3000
```

## Repository layout

| Path | Role |
|---|---|
| `src/store.rs` | The board — the only write path |
| `src/machine.rs` | Legal transitions and lifecycle invariants |
| `src/supervisor.rs` | Dispatch, sandbox lifecycle, lease sweeping |
| `src/mcp.rs` | Operator-seat tools; the host seat keeps worker verbs |
| `src/openshell.rs` | In-process gRPC client to the OpenShell gateway |
| `sandbox/` | Container image, network policy |
| `web/` | React UI + screenshot harness |
| `docs/` | The mdBook published to honr-app.github.io |

## Status

honr is under active development and already used to ship changes to itself.
Expect sharp edges: the happy path (board → plan → sandboxed agent → PR) works;
independent verification gates are still ahead.

The properties we will not break while the surface grows are written down in
**[Invariants](https://honr-app.github.io/invariants.html)**.

## Contributing

The preferred path is the product itself: open or join a Project on a running
board, let a worker open a PR, review and merge on GitHub.

By hand:

1. Keep mutations on the board path — do not encode lifecycle rules in
   transports (`api.rs` / `mcp.rs`).
2. `cargo test` and `cargo clippy --all-targets -- -D warnings` must be clean.
3. Stage deliberately. `git add -A` has committed `enabled: true` here before,
   which makes a fresh clone spend money on startup.

Agent orientation for work *on* this repo: [`AGENTS.md`](AGENTS.md)
(`CLAUDE.md` is a symlink) and
[`.cursor/rules/honr-operator.mdc`](.cursor/rules/honr-operator.mdc).

## License

License terms are not yet published in this repository. Treat the code as
source-available until a `LICENSE` file lands; ask before redistributing.
