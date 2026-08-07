# Concepts

The ideas the rest of the docs assume. For terms in isolation, see the
[Glossary](glossary.md); for the loop with pictures, the [Tour](tour.md).

## The board runs the work

honr’s board is not a status report of work elsewhere. Changing a card *is* an
action: dispatch claims it, Approve creates Tasks from a proposal, answering
Needs You unblocks a stopped agent.

The UI and the MCP API share **one state machine**. Every mutation goes through
`Board` in `src/store.rs`, so UI and MCP cannot drift apart.

## Project and Task

One node type, two roles:

| Kind | Role |
|---|---|
| **Project** | Container for the Plan, standing instructions (`project_prompt`), an optional sandbox override, and auto-dispatch. Not claimable work itself. |
| **Task** | The claimable leaf. Initial plan, implementation cards, and follow-ups are all Tasks under a Project. |

Tasks are flat siblings related by dependency edges, not a nested hierarchy.
There is no Epic or Story layer — dependencies already say what blocks what.

Every Project gets one claimable **Initial plan** Task. An agent reads the repo
and proposes the breakdown; you edit and Approve; the proposal becomes real
cards. Same path when a card turns out too big — the agent proposes siblings
and you approve those.

## Operator and worker

Three roles, different reach:

| Role | Who | Reach |
|---|---|---|
| **Operator** | You, and any chat agent you drive honr from | MCP at `/mcp`: shape Projects, triage, dispatch, park / steer / halt. Operator tools only. |
| **Worker** | The agent working a card, inside a sandbox | GitHub and inference. **No network path to honr.** |
| **Cockpit** | A privileged sandbox you attach a terminal to | honr's operator tools, plus inference and GitHub. No package-registry egress. |

Workers cannot call honr. An agent that could reach the board’s MCP could
approve its own review — so the supervisor calls `claim` / `heartbeat` /
`report` on its behalf.

Cockpit uses a separate sandbox spec (and Policy) so privileged reach to the
board does not share the worker’s network allow-list.

## How an agent finishes

A worker has no API to call, so it finishes by writing a file into its sandbox:
`plan.json`, `report.json`, `escalate.json`, or `split.json`. The supervisor
picks the file up and moves the card.

An agent that hits an ambiguity **stops** rather than guessing. It writes
`escalate.json` with the question and options; the card lands in Needs You and
costs nothing until you answer.

## Where the human stays

**You merge on GitHub.** Approving in honr surfaces the pull request. honr has
no write access to your default branch.

**Liveness is observed.** The supervisor parses the agent’s output stream. There
is no keepalive timer that can claim a wedged agent is alive.

The full set, with the reasoning: [Invariants](invariants.md).

## Where to go next

- See the loop → [Tour](tour.md)
- Run the board → [Quickstart](quickstart.md)
- Turn on agents → Welcome/Help OpenShell guide, then [Your first agent](first-agent.md)
- Day-to-day operation → [Workflow](workflow.md)
- How the pieces fit → [Architecture](architecture.md)
