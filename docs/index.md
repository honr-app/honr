# honr

**honr** is a board you point at a repository. You describe what you want; it
runs coding agents in sandboxes; pull requests come back for you to merge.

The difference from a task tracker is that the board is not a report of work
happening elsewhere. It *is* the work. Moving a card starts an agent. Answering
a question unblocks one. Approving a plan creates the tasks.

![The honr board: Backlog, Running, Needs You, Review, Done](images/desktop-board.png)

## What a turn looks like

1. You create a **Project** pointed at a repo and say what you want.
2. An agent reads the repo and proposes a breakdown. You **Approve** it, and
   those become real cards.
3. Agents claim cards, work in isolated sandboxes, and open pull requests.
4. Cards that need a decision stop and wait in **Needs You** — costing nothing
   while they do.
5. You review the PRs and merge on GitHub. honr never merges anything.

The [Tour](tour.md) walks that loop with screenshots and needs nothing
installed.

## Is this for you?

honr is for someone who wants several agents working a repository at once, and
wants one place to steer them from. It assumes you are comfortable reviewing
pull requests and running a service on your own machine.

It is **not** a hosted product, and it is not an IDE assistant. There is no
autonomy setting that merges code for you — that boundary is deliberate and
[not moving](invariants.md).

You can run the board with agents switched off: no Docker, no gateway, no
credentials, no spend. That is the default, and it is the right way to start.

## Start here

| If you want to | Read |
|---|---|
| See it work without installing anything | [Tour](tour.md) |
| Learn the vocabulary | [Concepts](concepts.md) · [Glossary](glossary.md) |
| Run the board locally | [Quickstart](quickstart.md) |
| Get one real agent opening PRs | Welcome/Help, then [Your first agent](first-agent.md) |
| Operate it day to day | [Workflow](workflow.md) |
| Understand how it is built | [Architecture](architecture.md) |

Machine contracts, not prose: [`schemas/report.schema.json`](schemas/report.schema.json).

honr is under active development and is used to ship changes to itself. Expect
sharp edges.
