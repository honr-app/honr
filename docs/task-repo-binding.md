# Clone targets (task prose)

Agents clone from the repository named in the card's **intent**, **definition
of done**, and/or **notes**. The supervisor leaves `/sandbox/repo` empty so the
agent clones. Settings → Forge configures beads Issue sync.

Related empty-state onboarding: [#279](https://github.com/shanemcd/honr/issues/279)
(Help chrome is separate; this page is the remotes contract).

## Happy path

```text
create_project(title, intent, …)  → Project + auto-seeded Initial plan Task
dispatch Initial plan             → write plan.json (proposed Tasks name
                                    clone targets in intent/DoD) → Review
Approve                           → sibling Tasks (prose carries clone targets)
dispatch impl Task                → agent clones from card text; opens PR
report.json                       → card.pull_request set → resume remotes
```

MCP: `create_project` seeds Initial plan. `init_plan` re-seeds if a Project
somehow has none. REST: `POST /api/items/{project_id}/init-plan` with `{}` is
the same.

## Resolution order

```text
card.pull_request (base/head or URL stub)
  → else Ok(None)  # unbound → briefing: clone from card prose or escalate
```

When `RepoConfig` is complete (after a PR exists), the Remotes briefing names
`origin` / `upstream` / base for resume and rebase. Unbound cards escalate
rather than inventing an `owner/name`.

## After report

`pull_request` on the card is the durable remotes handle for later claims
(rebase, request-changes, park/unpark). Approving in honr surfaces the PR; it
never merges.

## Sibling Tasks

Approve / split materialize children from `plan.json` / `ChildSpec` titles and
prose. Each child's intent/DoD should name the clone target.

## Direct create-task

Creating a claimable Task accepts intent/DoD (and optional notes). Name the
clone target there. Projects remain containers (plus the auto-seeded Initial
plan).
