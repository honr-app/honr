# Concepts

The ideas the rest of the docs assume. For terms in isolation, see the
[Glossary](glossary.md); for the loop with pictures, the [Tour](tour.md).

**honr** means **honer**: the one that hones. To *hone* is to refine a skill,
idea, or technique through practice and time. The board is that loop made
concrete — intent in, agents against real repositories, pull requests out, under
human judgement.

## The board is a control plane

Most boards are a *report*: something happens elsewhere, and the board describes
it afterwards. honr's board is the other way round. It is written to at machine
speed, read by agents as their source of truth, and **moving a card is an
action**.

Dispatching a card claims it. Approving a proposal materializes Tasks.
Answering Needs You unblocks an agent that is stopped mid-run. There is no
separate "apply" step, because there is nowhere else for the work to live.

The UI and the agent API are two renderings of **one state machine**. Every
mutation goes through `Board` in `src/store.rs`, whichever face it arrived on.
That is what stops the two drifting apart.

## Project and Task

One node type, two roles:

| Kind | Role |
|---|---|
| **Project** | Container. Holds the Plan, standing instructions (`project_prompt`), an optional sandbox override, and auto-dispatch. Never claimable work itself. |
| **Task** | The claimable leaf. Initial plan, implementation cards, and follow-ups are all Tasks under a Project. |

Tasks are flat siblings related by dependency edges, not a nested hierarchy.
There is no Epic and no Story — a deeper tree buys structure you then have to
maintain, and the dependency edges already say what depends on what.

Every Project is seeded with one claimable **Initial plan** Task. You do not
write the breakdown yourself: an agent reads the repo and proposes it, you edit
and Approve, and the proposal becomes real cards. Same path for a card that
turns out too big — the agent proposes siblings and you approve those.

## Operator and worker

Three seats, and the difference between them is reach:

| Role | Who | Reach |
|---|---|---|
| **Operator** | You, and any chat agent you drive honr from | MCP at `/mcp`: shape Projects, triage, dispatch, park / steer / halt. Operator tools only. |
| **Worker** | The agent working a card, inside a sandbox | GitHub and inference. **No network path to honr.** |
| **Cockpit** | A privileged sandbox seat you attach a terminal to | honr's operator tools, plus inference and GitHub. No package-registry egress. |

The worker's containment is the load-bearing one. **An agent that could reach
honr's MCP could approve its own review.** So it cannot: the supervisor calls
`claim` / `heartbeat` / `report` on its behalf, and the card worker is material
the board acts on, not a participant in it.

The cockpit is a separate sandbox spec precisely so that privileged reach to the
board does not share the worker's network allow-list.

## How an agent finishes

A worker has no API to call, so it finishes by writing a file into its sandbox:
`plan.json`, `report.json`, `escalate.json`, or `split.json`. The supervisor
picks the file up and moves the card.

This is why an agent that hits an ambiguity **stops** rather than guessing. It
writes `escalate.json` with the question and its options, the card lands in
Needs You, and it costs nothing until you answer. A stopped agent waiting on a
decision is the cheapest thing on the board.

## Where the human stays

Two boundaries are not settings:

**Merging is human.** Approving in honr surfaces the pull request. A human
merges on GitHub. honr has no write access to your default branch and no
autonomy dial that changes this.

**Liveness is observed, never self-reported.** The supervisor parses the agent's
output stream to know it is alive. There is no keepalive timer, because a timer
would assert liveness without evidence and throw away the only property that
makes the signal worth having.

The full set, with the reasoning: [Invariants](invariants.md).

## Where to go next

- See the loop → [Tour](tour.md)
- Run the board → [Quickstart](quickstart.md)
- Turn on agents → Welcome/Help OpenShell guide, then [Your first agent](first-agent.md)
- Day-to-day operation → [Workflow](workflow.md)
- How the pieces fit → [Architecture](architecture.md)
