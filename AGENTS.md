# AGENTS.md

Orientation for the **operator** agent sitting with the human. If you are an
agent honr dispatched to work on a card, you already have a briefing — ignore
this file.

**Operator mode:** drive honr via MCP (`http://127.0.0.1:8080/mcp`). Do not
implement product work by editing this tree; shape Projects/Plans, triage
Needs You / Review, let sandboxed workers open PRs. See
[`.cursor/rules/honr-operator.mdc`](.cursor/rules/honr-operator.mdc).

Start with [`docs/index.md`](docs/index.md) and
[`docs/concepts.md`](docs/concepts.md).

## What this is

An agent orchestrator that dispatches work against **its own source**. The
board is a control plane: moving a card *is* an action. honr claims a card,
runs a Claude Code agent in an OpenShell sandbox, and the agent opens a
same-repo PR (`honr/card-*` → `main`) that a human merges.

## The invariants worth protecting

**One state machine.** Every mutation — UI, MCP, supervisor — goes through
`Board` in `src/store.rs`. No transport holds state-machine logic. If you find
yourself encoding a rule in `api.rs` or `mcp.rs`, it belongs in `machine.rs` or
`store.rs` instead.

**The agent is material, not a participant.** It gets no network path to honr.
The supervisor calls `claim`/`heartbeat`/`report` on its behalf. An agent that
could reach honr's MCP could approve its own review.

**Liveness is observed, never self-reported.** It is parsed from the agent's
output stream. Do not add a timer-based keepalive — it would assert liveness
without evidence and throw away the property that makes the signal trustworthy.

**Merging is human.** Approving in honr surfaces the PR. It never merges.

**The bot may push feature branches, not main.** Containment is the GitHub
ruleset on the default branch (owner-only) plus human merge — not “no write on
the repo.” Use the App installation on `shanemcd`, same-repo fork=upstream.

## Conventions

Comments explain **why**, not what. The existing code reads like prose and
argues with itself where a decision was close; match that. A comment that
restates the line below it is noise.

Tests live next to what they test. `machine.rs` holds the lifecycle
invariants; other modules test the things that break silently — argv shape,
shell quoting, config validation. Prefer a test that names the
failure it prevents over one that names the function it calls.

Before you finish: `cargo test` and `cargo clippy --all-targets -- -D warnings`
must both be clean. Both run `--offline` inside a sandbox.

`git add -A` has twice committed something unintended here — `enabled: true` in
`honr.yaml` most notably. Stage deliberately.

## Things that will waste your time if you don't know them

**Everything in the sandbox stack fails as a hang, not an error.** Denied
egress, a missing credential, a wedged relay — all silence. Every exec needs a
deadline; treat silence as failure. This shaped `openshell.rs` entirely.

**Don't script what the agent can drive.** The supervisor used to push and open
PRs itself; four separate failures came from that shell being wrong about tools
the agent already knew. It now only *asks GitHub what happened*. Before adding
a shell script to the supervisor, ask whether the briefing could say it instead.

**The image's `ENV` does not reach `openshell sandbox exec`.** Pass what the
agent needs explicitly in `agent_env`, or install wrappers on the default PATH.

**`sandbox upload` takes a destination directory**, and the destination must
already exist.

**The podman machine stops on its own.** Classify that as infrastructure, not
as the card failing — see `is_infrastructure`.

**Rebase onto upstream `main`.** Same-repo still fetches/rebases onto the
default branch tip; do not treat a stale local main as truth.

## Where things are

| Path | What |
|---|---|
| `src/machine.rs` | Legal transitions and the two invariants |
| `src/store.rs` | The board — the only write path |
| `src/supervisor.rs` | Dispatch, per-card lifecycle, briefing, lease sweeping |
| `src/openshell.rs` | Typed wrapper over the CLI; every call has a deadline |
| `src/mcp.rs` | Operator tools and worker verbs |
| `sandbox/` | Containerfile, network policy, metadata shim |
| `web/` | React UI + `npm run shots` screenshot harness |
| `docs/sandbox.md` | How a sandboxed run works and the gotchas that matter. Read before touching `sandbox/`. |

## Environment

Agent model / Vertex-style credentials are no longer configured in
`honr.yaml` or Settings → Agent runtime. They will come back through OpenShell
configuration managed in honr. Until then, sandboxed Claude/agy runs that
depended on Vertex env injection are expected to fail.

GitHub work uses a bot account, configured as `execution.agents.repo.fork`.
Its token currently carries broad `repo` scope across that account — a
fine-grained PAT scoped to the fork alone is the right hardening step and has
not been done.

## Working with the human here

They will tell you when the board looks wrong, and they have been right every
time it mattered. Check the evidence before defending the code — twice in one
session a confident "it's fine" was based on a log that was not capturing
anything.

State corrections plainly and move on. Do not narrate the mistake at length.


<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:6cd5cc61 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready --exclude-type=epic  # Find available task work (excludes non-claimable epics; add --parent=<epic> to scope)
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->
