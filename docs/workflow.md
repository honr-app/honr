# Workflow

Day-to-day operation once the board is up. For enabling sandboxed agents, see
[Agents](agents.md). Clone targets live in task prose — see
[Clone targets](task-repo-binding.md).

## Happy path

1. **Create a Project** (`create_project` or UI): auto-seeds a claimable
   Initial plan Task. No product-repo field.
2. **Dispatch** Initial plan (`dispatch` / UI **Start**): agent writes
   `plan.json` only (no docs PR). Each proposed task names its clone target
   in intent/DoD.
3. **Approve** that card: creates sibling Tasks under the Project. Approve
   does not auto-dispatch unless Project auto mode is already on.
4. **Dispatch** each Ready / Backlog Task (or turn on Project auto mode).
5. Worker clones from card text, opens a PR → card lands in **Review**.
6. You merge on GitHub. With webhook forwarding, merged cards move to Done.

`propose_breakdown` is for manual Project replan only. Impl oversize uses the
same split path: `split.json` → Review with proposal → Approve creates siblings.

Standing policy lives in Project `project_prompt` (edit via `update`) — quality
gates and invariants, not the clone target. Name the repo to clone in each
Task's intent/DoD. After `report.json`, card `pull_request` drives resume.
Read `item_detail`'s proposal / Plan before approving. A card that passes its
gates can still be building the wrong thing.

## Triage order

Urgency differs:

1. **Needs You**: an agent is stopped and burning nothing while it waits.
   Resolve these first.
2. **Review**: finished and safe. Sort by blast radius and novelty, not
   arrival time.
3. Everything else waits for a digest (`board_snapshot` / `board_digest`).

Interrupt the human for four things only: irreversible actions, a blocking
ambiguity across several items, and repeated failure on the same card.
Otherwise summarise and let them walk away.

## Dispatch and auto mode

By default the operator decides what starts. A Backlog card is inert until
someone calls `dispatch` (or UI **Start**), which sets `awaiting_dispatch`.

**Project auto mode** (swimlane play/pause, or MCP `set_auto_dispatch`) is the
exception: when on, each supervisor tick queues every claimable Backlog leaf
under that Project. Pause clears `awaiting_dispatch` on still-Backlog cards but
does **not** halt Claimed / Running agents. Auto does not approve Review,
answer Needs You, or unpark.

The supervisor takes the oldest Backlog card with `awaiting_dispatch` that is
claimable and not already being run, subject to concurrency and gateway health
gates. Lease expiry, park, halt, release, and request_changes all clear
`awaiting_dispatch`: with auto off, dispatch again; with auto on, the next
tick re-queues claimable cards. Unpark clears the hold and queues the
supervisor (same as Start).

## Steering a card

| You want to | Do this |
|---|---|
| Send a reviewed card back with instructions | **Request changes**. The note reaches the next run's briefing. Does not auto-start. Dispatch again. |
| Answer a blocked agent | **Needs You**: pick an option. |
| Stop a wedged run but keep context | **Park**: stops the agent, keeps sandbox + conversation, holds until **Resume** / `unpark`. |
| Resume a parked card | **Unpark**: clears the hold and queues the supervisor; next claim can resume the conversation. |
| Throw away the run | **Halt**: stops the agent, clears conversation id, deletes the sandbox. Next dispatch starts clean. |
| Soft note for later | **Steer**: stored and seen on the next claim. Does not inject mid-turn. |
| Auto-start claimable Backlog under a Project | Swimlane **Auto** play/pause (or `set_auto_dispatch`). |

Prefer **park** over **halt** when the agent is stuck and you want the same
conversation to continue. Prefer **steer** for a soft note that can wait.

## MainAdvanced (push to default branch)

Ingress is `POST /api/webhooks/github`. A push to the default branch emits
`MainAdvanced`, which:

1. **Merged card → Done** when a Review / NeedsHuman card's `pr_url` matches
   the merged PR.
2. **Review rebase catch-up** for sibling PRs still in Review that are behind
   `main` (supervisor-driven git on the PR branch).
3. **Live runs**: each Claimed / Running card gets a steer note to fetch /
   rebase onto upstream main. Because steer alone does not inject mid-turn,
   honr then parks and unparks so the agent acts on resume. Sandbox and
   conversation id are preserved.

Dev-only local forwarding:

```bash
gh extension install cli/gh-webhook   # once

gh webhook forward \
  --repo=<owner/name> \
  --events=pull_request,push \
  --url=http://127.0.0.1:8080/api/webhooks/github
```

Only one forwarder per repo at a time. Settings → Forge shows the same
placeholder template.

## Looking at the UI

```bash
npm --prefix web run shots      # -> web/shots/*.png
```

Runs a scratch honr on :8081 against a fixture board and captures desktop /
phone views. Your real state is untouched.
