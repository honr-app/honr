# Clone targets (task prose)

Agents clone from the repository named in the card's **intent**, **definition
of done**, and/or **notes**. The supervisor leaves `/sandbox/repo` empty so the
agent clones. Settings → Forge configures the forge provider and webhook poll.

Related empty-state onboarding: [#279](https://github.com/honr-app/honr/issues/279)
(Help chrome is separate; this page is the remotes contract).

## Happy path

```text
create_project(title, intent, clone_repo, …)
  → Project + Initial plan (clone_repo stamped in prose)
dispatch Initial plan → clone clone_repo; write plan.json (proposed Tasks
                        name clone targets in intent/DoD) → Review
Approve               → sibling Tasks (prose carries clone targets)
dispatch impl Task    → agent clones from card text; opens PR
report.json           → card.pull_request set → resume remotes
```

`clone_repo` is required (`owner/name`). It is stamped into Project intent and
the seeded Initial plan so Remotes can clone without inventing a name. MCP
`create_project` and REST Project create both require it. `init_plan` re-seeds
if a Project somehow has none. REST: `POST /api/items/{project_id}/init-plan`
with `{}` is the same.

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
