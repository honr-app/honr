# Concepts

**honr** means **honer**: the one that hones. To *hone* is to refine a skill,
idea, or technique through practice and time. The board is that loop made
concrete.

honr is an agent orchestrator whose board is a **control plane, not a report**.
It is written to at machine speed, read by agents as their source of truth, and
moving a card *is* an action. The scarce resource is human attention.

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

## Board as control plane

The UI and the agent API are two renderings of **one state machine**. Every
mutation goes through `Board` in `src/store.rs`. No transport holds
state-machine logic: that is what stops the two drifting apart.

Columns (Ready / Backlog through Done) are not status labels for a dashboard;
transitions are the work. Approving a Plan materializes Tasks. Dispatching a
card claims it. Answering Needs You unblocks a waiting agent.

## Project + Tasks

One node type, two roles:

| Kind | Role |
|---|---|
| **Project** | Container. Holds the Plan, optional `project_prompt`, sandbox profile override, auto-dispatch. Never sits in Backlog as claimable work. |
| **Task** | Claimable leaf. Initial plan, implementation cards, and follow-ups are Tasks under a Project. |

Task↔task links are board `blocked_by` edges. The product schema is Project +
Task.

## Operator vs worker

| Role | Who | Reach |
|---|---|---|
<<<<<<< HEAD
| **Operator / ops seat** | Human + chat agent on the host | MCP `/mcp`: shape Projects, triage Needs You / Review, dispatch, park / steer / halt. No worker verbs. |
| **Worker** | Agent inside an OpenShell sandbox | No network path to honr. Supervisor calls `claim` / `heartbeat` / `report` on its behalf |
=======
| **Operator (host)** | Human + chat agent on the host | MCP into honr: shape Projects, triage Needs You / Review, dispatch, park / steer / halt |
| **Ops seat** | Privileged agent in an OpenShell sandbox on the `ops` profile | Narrow egress to host honr MCP (+ inference). No GitHub / package-registry identity. Selectable in Settings → OpenShell → Profiles |
| **Worker** | Agent inside an OpenShell sandbox on the default / project profile | GitHub + inference egress. No network path to honr. Supervisor calls `claim` / `heartbeat` / `report` on its behalf |
>>>>>>> 43d29d2 (Add ops sandbox policy and seedable catalog profile.)

An agent that could reach honr's MCP could approve its own review. Worker
containment is intentional: the card worker is material, not a participant. The
ops seat is a separate profile and policy (`sandbox/ops-policy.yaml`) so that
privileged MCP reach does not share the worker network allow-list.

The durable **ops session** (sandbox environment name, conversation id, running
or parked-like hold) lives on the Board as a singleton record — not a WorkItem
and not card claim/heartbeat/report lifecycle. Chat and TTY reconnect read and
mutate that Board record so they do not grow a second state machine.

## Invariants worth protecting

**One state machine.** If a rule belongs in the lifecycle, it lives in
`machine.rs` / `store.rs`, not in `api.rs` or `mcp.rs`.

**Liveness is observed, never self-reported.** The supervisor parses the
agent's output stream. A timer-based keepalive would assert liveness without
evidence.

**Merging is human.** Approving in honr surfaces the PR. It never merges.

**Feature branches are writable; `main` is human-gated.** The repository
ruleset keeps the default branch owner-only. Agents push `honr/card-*` on
`shanemcd/honr` (App installation on that account) and open PRs into `main`;
humans merge.

## Where to go next

- New to the board → [Quickstart](quickstart.md)
- Day-to-day operation → [Workflow](workflow.md)
- Turn on sandboxed agents → [Agents](agents.md)
- How the pieces fit → [Architecture](architecture.md)
