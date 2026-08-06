# Workflow

Day-to-day operation once the board is up. To enable sandboxed agents first, see
[Your first agent](first-agent.md).

## The happy path

1. **Create a Project** with a `clone_repo` (`owner/name`). That repo is what
   the planning agent clones. honr auto-seeds a claimable **Initial plan** Task
   with the target stamped in.
2. **Start** the Initial plan. The agent clones, reads, and writes `plan.json`
   proposing sibling Tasks. The card lands in **Review**.
3. **Approve** it. The proposal becomes real Tasks under the Project. Approving
   does not auto-start them unless the Project's auto mode is already on.
4. **Start** each Task (or turn auto mode on).
5. The worker clones, works, and opens a PR. The card lands in **Review**.
6. **You merge on GitHub.** A webhook (or Forge polling) moves the card to Done.

Read the proposal before approving. A card that passes every gate can still be
building the wrong thing, and coherence is not a property any single card has.

`propose_breakdown` is for manually replanning a Project. A card that turns out
too big uses the same path in reverse: the agent writes `split.json`, the card
goes to Review with a proposal, and Approve creates the siblings.

## Which repo an agent clones

Agents clone the repository named in the card's **intent**, **definition of
done**, or **notes**. The supervisor leaves `/sandbox/repo` empty and lets the
agent do it.

This is why `clone_repo` is required when you create a Project: it gets stamped
into the Project intent and into the seeded Initial plan, so the first agent
never has to invent a name. Proposed Tasks should each name their clone target
the same way — the fixture proposals do, and so should yours.

Resolution at claim time is short:

```text
card.pull_request (base/head, once a PR exists)
  → else: clone from the card's prose
  → else: escalate
```

An unbound card escalates rather than guessing an `owner/name`. Once
`report.json` sets `pull_request`, that becomes the durable handle every later
claim uses for resume, rebase, and request-changes.

Standing policy — quality gates, invariants, house style — belongs in the
Project's `project_prompt`, not repeated on every card.

## Triage order

Urgency genuinely differs between columns:

1. **Needs You** — an agent is stopped and burning nothing while it waits. Every
   minute is throughput you are not getting. Resolve these first.
2. **Review** — finished and safe. It can wait until this evening. Sort by blast
   radius and novelty, not arrival time.
3. Everything else waits for a digest (`board_snapshot` / `board_digest`).

If you are driving honr through a chat agent, interrupt the human for three
things only: irreversible actions, an ambiguity blocking several items, and
repeated failure on the same card. Otherwise summarise and let them walk away.

## Dispatch and auto mode

By default the operator decides what starts. A Backlog card is inert until
someone calls `dispatch` (**Start** in the UI), which sets `awaiting_dispatch`.

**Project auto mode** — the swimlane play/pause, or `set_auto_dispatch` — is the
exception. With it on, each supervisor tick queues every claimable Backlog leaf
under that Project. Pause clears `awaiting_dispatch` on cards still in Backlog
but does **not** halt Claimed or Running agents. Auto mode never approves a
Review, answers a Needs You, or unparks anything.

The supervisor takes the oldest claimable Backlog card with `awaiting_dispatch`
that is not already running, subject to concurrency and gateway health.

Lease expiry, park, halt, release, and request_changes all clear
`awaiting_dispatch`. With auto off, dispatch again; with auto on, the next tick
re-queues it. Unpark clears the hold and queues the supervisor, same as Start.

## Steering a card

| You want to | Do this |
|---|---|
| Send a reviewed card back with instructions | **Request changes** — the note reaches the next run's briefing. Does not auto-start; dispatch again. |
| Answer a blocked agent | **Needs You** — pick an option. |
| Stop a wedged run but keep its context | **Park** — stops the agent, keeps sandbox and conversation, holds until Resume. |
| Resume a parked card | **Unpark** — clears the hold; the next claim can resume the conversation. |
| Throw the run away | **Halt** — stops the agent, clears the conversation id, deletes the sandbox. Next dispatch starts clean. |
| Leave a note for later | **Steer** — stored, seen on the next claim. Does not inject mid-turn. |
| Auto-start claimable Backlog under a Project | Swimlane **Auto** play/pause. |

Prefer **park** over **halt** when the agent is stuck and you want the same
conversation to continue. Prefer **steer** when the note can wait.

## When main moves

Ingress is `POST /api/webhooks/github`. A push to the default branch emits
`MainAdvanced`, which does three things:

**1. Merged card → Done.** When a Review or Needs You card's `pr_url` matches
the merged PR, it completes. Same-parent Review siblings with open PRs get
`rebase_requested`. Webhook and polling both go through the same Board
completion helper, so sibling catch-up behaves identically either way.

**2. Review conflict observation.** Every Review card with an open PR targeting
the advanced base is queued for a host-side GitHub API `mergeable` check — an
App installation token, not a `git rebase` in a sandbox. `MERGEABLE` clears the
queue and the card stays in Review. `CONFLICTING` bounces it to Backlog with a
note so a worker can reclaim and rebase. `UNKNOWN` stays queued and retries next
sweep, because GitHub computes mergeability asynchronously.

**3. Live runs get steered.** Each Claimed or Running card gets a note to fetch
and rebase onto upstream main. Because steer alone does not inject mid-turn,
honr then parks and unparks so the agent acts on resume — sandbox and
conversation id are preserved.

Review cards are deliberately *not* parked to reuse that path; they stay in
Review until a conflict or a human bounce. And steering Running does not replace
Review catch-up: both fire on the same `MainAdvanced`, so a Review PR is not left
behind when only Running moved.

### Local webhook forwarding

```bash
gh extension install cli/gh-webhook   # once

gh webhook forward \
  --repo=<owner/name> \
  --events=pull_request,push \
  --url=http://127.0.0.1:8080/api/webhooks/github
```

One forwarder per repo at a time. For a polling fallback instead, see
[Configuration](configuration.md#forge-and-webhooks).

## Next

- [Troubleshooting](troubleshooting.md) — when a card stops moving
- [Cockpit](cockpit.md) — a durable terminal seat with operator reach
- [Configuration](configuration.md) — timeouts, concurrency, sandbox specs
