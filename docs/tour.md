# Tour

One card's life, start to finish. Nothing here needs to be installed — read it
first and decide whether the loop is one you want.

The board below is a real honr running against a fixture. Every screenshot on
this page is captured from the running UI, so what you see is what ships.

## 1. The board

![The honr board](images/desktop-board.png)

Five columns, and each asks a different question:

| Column | The question it asks |
|---|---|
| **Backlog** | What could start? |
| **Running** | What is an agent working on right now? |
| **Needs You** | What is stopped, waiting on a human? |
| **Review** | What finished and is waiting for judgement? |
| **Done** | What landed? |

Two things on this screen are worth pointing at.

The **Needs You** band sits above everything else, with the answer buttons right
there. That placement is a claim about what is scarce: agents are cheap and your
attention is not, so the one thing only you can do is the one thing you should
not have to go looking for.

Down in Backlog, cards carry **`⊘ waiting on`** chips naming what blocks them —
`#3 Fail closed when CI is red` cannot start until `#2 Surface PR checks` lands.
Blocked cards sort to the bottom, so the top of the column is always what could
actually start now.

## 2. A Project proposes its own breakdown

You do not write the task list. You create a **Project**, point it at a repo,
and say what you want. honr seeds one claimable card — the *Initial plan* — and
an agent reads the repo and proposes the breakdown.

That proposal comes back as a card in Review:

![The Initial plan card with its proposed Tasks](images/desktop-drawer-plan.png)

Four proposed Tasks, each with a key, an intent, a definition of done, and its
dependencies. **You can edit any of it before approving.** Approve, and those
four become real cards in Backlog with the dependency edges already wired — the
same `⊘ waiting on` chips you saw in step 1.

This is the one moment where the plan is cheap to change. A card that passes
every gate can still be building the wrong thing, and no amount of review at the
PR stage recovers a breakdown that was wrong here.

## 3. An agent picks up a card

Dispatch a Backlog card (**Start** in the UI) and the supervisor claims it,
creates a sandbox, and runs an agent inside it.

Running cards show what you would want to know mid-flight: which engine, how
much of the run budget is left, and the sandbox name for when you need to go
look at logs.

The agent has **no network path back to honr**. It cannot see the board, cannot
claim its own card, and cannot approve its own review. The supervisor speaks for
it, and liveness is read out of the agent's output stream rather than
self-reported. An agent that could tell you it was alive without evidence would
be telling you nothing.

## 4. When it needs a decision, it stops

![A card waiting in Needs You](images/desktop-drawer-needs-you.png)

An agent that hits a genuine ambiguity does not guess and does not spin. It
writes the question with options and stops. The card lands in **Needs You** and
burns nothing until you answer.

Answering is one tap, from the band at the top of the board. The answer reaches
the agent on its next turn.

## 5. Finished work waits in Review

![A finished card in Review with its pull request](images/desktop-drawer-review.png)

The agent pushes a branch and opens a pull request. The card moves to **Review**
carrying the PR link, the diffstat, and whichever gates it ran.

Review is sorted by blast radius, not arrival time — a 300-line change with a
failed gate sorts above a 6-line one, because that is the order in which you
would regret not looking.

**Approving in honr surfaces the PR. It never merges it.** You merge on GitHub,
like any other contribution. When the merge lands, a webhook moves the card to
Done and tells any sibling still in Review to rebase.

## 6. Seeing the shape of the work

![The dependency graph view](images/desktop-graph.png)

Columns answer "what is happening". The graph answers "what depends on what" —
useful when a plan has grown past the point where the chips on individual cards
tell you the whole story.

## And on a phone

![The board on a phone](images/phone-board.png)

The claim the phone view makes is narrow but real: the thing you need away from
your desk is not the whole board, it is the short list of decisions only you can
make. If that fits on a phone, you can walk away from the rest.

## Next

- Run the board yourself, agents off: [Quickstart](quickstart.md)
- The vocabulary in one place: [Concepts](concepts.md) · [Glossary](glossary.md)
- Turn on real sandboxed agents: empty-board **Welcome** / **Help**, then [Your first agent](first-agent.md)
