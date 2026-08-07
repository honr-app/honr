# Glossary

Every term the rest of the docs assume, in one place.

## The board

| Term | Meaning |
|---|---|
| **Card** | Any item on the board. A card is either a Project or a Task. |
| **Project** | Groups Tasks, the Plan, standing instructions, and an optional sandbox override. Agents claim Tasks, not Projects. |
| **Task** | The claimable leaf. Initial plan, implementation cards, and follow-ups are all Tasks under a Project. |
| **Initial plan** | The Task honr creates with every Project. An agent claims it, reads the repo, and proposes the sibling Tasks. |
| **Proposal** | The breakdown an Initial plan (or a split) hands back. Editable until you Approve; Approve turns it into real cards. |
| **Blocked by** | A dependency edge between Tasks. Blocked cards sort last in Backlog and render a `⊘ waiting on` chip. |
| **Swimlane** | One Project's row on the board, with its own columns and auto-dispatch toggle. |

## Columns and states

Several internal states collapse into one column, because the question you ask
of them is the same.

| Column | States | The question |
|---|---|---|
| **Backlog** | `backlog` | What could start? |
| **Running** | `claimed`, `running`, `splitting` | What is an agent on right now? |
| **Needs You** | `needs_human` | What is stopped, waiting on a person? |
| **Review** | `review` | What finished and wants judgement? |
| **Done** | `done` | What landed? |
| **Retired** | `retired` | Archived or cut. Kept for history, not deleted. |

`draft` and `shaping` exist for cards still being formed.

## Actions

| Term | What it does |
|---|---|
| **Dispatch** / **Start** | Marks a Backlog card as wanting to run. The supervisor picks it up on the next tick. |
| **Auto mode** | Per-Project. Queues every claimable Backlog leaf automatically. Does not approve, answer Needs You, or unpark. |
| **Approve** | On a proposal, creates the sibling Tasks. On a Review card, surfaces the PR. Does not merge. A GitHub PR `APPROVED` review does not Approve in honr. |
| **Request changes** | Sends a Review card back with a note that reaches the next run's briefing. Does not restart it. Submitted GitHub `CHANGES_REQUESTED` / `COMMENT` reviews take the same Board path (pointer steer → Backlog); see [Workflow](workflow.md#pr-review-feedback). |
| **Steer** | A soft note stored for the next claim. Does not interrupt a running turn. |
| **Park** | Stops the agent but keeps the sandbox and conversation. Resume continues the same thread. |
| **Halt** | Stops the agent, clears the conversation, deletes the sandbox. The next dispatch starts clean. |
| **Archive** | Hides a Project (and its cards) from the active board via `cut_scope`. Not delete — toggle **Show archived** to browse. |
| **Unarchive** | Restore an archived Project/subtree. In-flight priors come back as Backlog (or Shaping), not Claimed/Running. |
| **Escalate** | What an agent does instead of guessing: writes a question with options and stops. |
| **Split** | What an agent does when its card is bigger than one card. Proposes siblings, same as Approve. |

Prefer **park** over **halt** when you want the same conversation to continue,
and **steer** over both when the note can wait.

## Roles

| Term | Meaning |
|---|---|
| **Operator** | You, plus any chat agent you drive honr from. Reaches honr over MCP at `/mcp` with operator tools only. |
| **Worker** | The agent inside a sandbox working a card. Has **no** network path to honr. |
| **Supervisor** | The part of honr that dispatches cards, runs sandboxes, and speaks for workers. |
| **Cockpit** | A durable privileged sandbox you can attach a terminal to, which reaches honr's operator tools. Distinct from the operator MCP surface itself. |

## Execution

| Term | Meaning |
|---|---|
| **OpenShell** | The sandbox gateway honr talks to over gRPC. Owns containers, network policy, and provider credentials. |
| **Policy** | A named OpenShell YAML allow-list (filesystem / network) on the board. Edited in Settings → OpenShell → Policies. Applied at sandbox create and fixed for that sandbox's life. |
| **Sandbox spec** | The named recipe for a sandbox: image, CPU, memory, engine, attached providers, and a reference to a Policy by id. Managed in Settings → OpenShell → Sandbox specs. |
| **Engine** | Which agent CLI runs in the sandbox: `cursor`, `claude`, `opencode`, or `agy`. |
| **Provider** | A credential OpenShell holds and injects on egress — inference, GitHub. Secrets never enter the sandbox. |
| **Briefing** | What the supervisor assembles for an agent at claim time: card intent, DoD, standing Project instructions, remotes, protocol paths. |
| **Lease** | The claim an agent holds on a card. Expires if output stops, so another run can take the card. |
| **Compute driver** | Whatever provides the Docker-compatible API OpenShell needs: podman, Colima, Docker. Your choice, outside honr. |

## Protocol files

An agent has no API to honr, so it finishes by writing a file into its sandbox.

| File | Means |
|---|---|
| `plan.json` | Here is the proposed breakdown. |
| `report.json` | I am done; here is the PR and the diffstat. |
| `escalate.json` | I need a decision; here is the question and the options. |
| `split.json` | This card is too big; here are the siblings it should become. |
