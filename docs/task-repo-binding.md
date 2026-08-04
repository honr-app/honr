# Task repo binding

Every agent-claimable Task carries durable product remotes so the agent can
clone without first-run guessing. The supervisor does **not** pre-clone — it
leaves `/sandbox/repo` empty. Remotes are **Task-scoped** — not a Project
field, and not Settings → Forge (beads-only).

Related empty-state onboarding: [#279](https://github.com/shanemcd/honr/issues/279)
(Help chrome is separate; this page is the remotes contract).

## Happy path

```text
create_project(title, intent, …)     → Project container (no Task seed)
init_plan(project, repo, …)          → Initial plan Task with Task.repo set
dispatch Initial plan                → empty workdir; agent clones from Task.repo
plan.json + docs PR → Review
Approve                              → sibling Tasks; each gets repo
                                       (explicit in ChildSpec / or default
                                        from Initial plan Task.repo)
```

MCP tools: `create_project`, then `init_plan` with
`{ project, repo: { upstream, fork?, base? } }` (`upstream` required;
`base` defaults to `main`). REST: `POST /api/items/{project_id}/init-plan`
with the same `repo` body. The board UI offers **Start planning** on a Project
that has no Initial plan yet (upstream / optional fork / base).

## Resolution order

```text
card.pull_request (base/head or URL stub)
  → else Task.repo → RepoConfig
  → else Ok(None)  # legacy / misconfig → empty workdir + escalate
```

No `Project.product_repo` step. Settings → Forge / `WorkspaceBinding` stay
beads-only.

When `RepoConfig` is complete, the Remotes briefing names `origin` /
`upstream` / base (same structured path as a card that already reported a PR).
Unbound cards (neither Task repo nor `pull_request`) keep the escalate
contract — never invent an `owner/name`.

## Task repo shape

Same facts as `RepoConfig`:

| Field | Role |
|---|---|
| `upstream` | PR target `owner/name` (required) |
| `fork` | Optional distinct push remote; omit / empty → same-repo |
| `base` | Branch; default `main` |

Stored on claimable Tasks (Initial plan and impl cards). After `report.json`,
card `pull_request` wins for resume / rebase.

## Sibling defaulting

When Approve / split materializes children:

1. Prefer per-child `repo` in `plan.json` / `ChildSpec` when complete (multi-repo).
2. Else copy the **Initial plan Task’s** (or splitting parent Task’s) repo.
3. Refuse materialization if neither yields a complete binding.

Never inherit from a Project product-repo field (there isn’t one). Different
Tasks under one Project may carry different upstreams — multi-repo is
supported on purpose.

## Direct create-task

Any path that creates a claimable Task must set Task repo (MCP / REST create
with parent). Projects remain containers: accidental `repo` / `product_repo`
on Project create/update is ignored or refused.

## Migration notes

| Population | Behavior |
|---|---|
| **New Projects** | `create_project` creates no Initial plan; call `init_plan` when ready. |
| **Existing cards with `pull_request`** | Unchanged — card facts win over Task repo. |
| **Existing Tasks with no repo and no PR** | Legacy: `Ok(None)` → escalate until Task repo is set or a Decision answers. |
| **Empty Settings Forge** | Unchanged — beads sync independent. |
