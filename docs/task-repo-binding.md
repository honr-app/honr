# Plan: Task repo binding and Initial plan seeding

**Status:** plan only — Approve materializes Tasks. No product code in this PR.

**Goal:** every agent-claimable Task carries an unambiguous product repository
(clone target) so the supervisor can pre-clone without brittle first-run
escalations — without binding remotes on the Project container.

**Board epic:** [#280](https://github.com/shanemcd/honr/issues/280). Related
empty-state onboarding: [#279](https://github.com/shanemcd/honr/issues/279).
(GitHub Issue #182 is unrelated — closing linked Issues on Done.)

**Out of scope here:** GitLab, rewriting the board state machine, shipping the
binding implementation (that is the Task DAG in `plan.json`).

**Supersedes:** the Project-scoped product-repo recommendation in an earlier
revision of this plan (PR feedback: remotes are task-scoped).

---

## Verdict

**Remotes are task-scoped.** `create_project` stays a container only (no repo).
Stop auto-seeding an unbound Initial plan. Add MCP `init_plan` (or equivalent)
that **requires** a repo and seeds the Initial plan Task with that binding.
Every claimable Task carries a repo at creation; siblings may default from the
Initial plan Task’s repo. `resolve_card_repo`: card `pull_request` → else Task
repo binding → else `Ok(None)` / escalate. No `Project.product_repo`. Settings
→ Forge stays beads-only. Multi-repo under one Project remains possible.

---

## 1. Current model (with code paths)

### Settings → Forge ≠ work remotes

| Piece | Path | Role today |
|---|---|---|
| `WorkspaceBinding` | `src/model.rs` | `forge` + `beads_sync_repo` only |
| Board SoT | `BoardState.workspace` in `src/store.rs` | Seeded from `GITHUB_REPOSITORY` or yaml upstream |
| REST | `GET`/`PUT /api/workspace` | Settings → Forge |
| UI copy | `web/src/components/Settings.tsx` | Product remotes live on card `pull_request` |

Work remotes are **not** on WorkspaceBinding — they live on each card’s
`PullRequest` after the agent reports. That separation stays.

### Project / Task fields

`WorkItem` has no structured Task repo field. Soft guidance lives in
`project_prompt` (`DEFAULT_PROJECT_PROMPT` in `src/model.rs`): name an exact
`owner/name` in prose or escalate.

Hard remotes appear only as per-card `pull_request` (`url` + `base`/`head`)
after `report.json`.

### Inheritance today

| Inherited into claim briefing | Not inherited |
|---|---|
| `project_prompt`, Plan rows (`ClaimGrant`) | Remotes |

`Board::resolve_card_repo` (`src/store.rs`) inspects **only the claimed card**:

1. `pull_request` with usable base/head → `RepoConfig`
2. Else parseable GitHub PR URL → same-repo stub
3. Else `Ok(None)` — first run

Sibling Tasks after Approve do **not** copy the Initial plan card’s
`pull_request`. Yaml `execution.agents.repo` is legacy; `run_card` clears it
when `resolve_card_repo` returns `None`.

### What the sandbox briefing receives

| Case | Supervisor | Briefing (`remotes_briefing_lines`) |
|---|---|---|
| Card has remotes | Pre-clone via `clone_script` | Named `origin` / `upstream` / base |
| Unbound + Decision `Clone owner/name` | Empty workdir | Clone that target; do not re-escalate |
| Unbound + silent prompt | Empty workdir | Escalate — never invent `owner/name` |
| Unbound + prompt names clone | Empty workdir; **agent** clones | Success depends on prose |

Initial plan cards must write `plan.json` + one docs PR + `report.json` (not
`split.json`).

### create_project + Initial plan auto-seed

`Board::create` (Project root) always calls `seed_initial_plan_task`: titles
`Initial Plan for <Project>`, lands in Backlog. MCP `create_project` accepts
`title`, `intent`, optional `project_prompt` — **no repo arg**. Seeding is
unconditional and unbound.

---

## 2. Failure mode

Missing or ambiguous clone target on first claim → `escalate.json` →
Needs You. Intentional containment (do not invent this product’s repo), but it
fires whenever Initial plan (or any Task) is claimable before a durable repo
exists. Auto-seeding Initial plan on `create_project` makes that the default
path. Answering with `Decision: Clone …` unbinds **that** card only; siblings
still rediscover.

---

## 3. Options (trade-offs)

### A. Install-wide default work remotes before any dispatch

- **Pros:** One knob for single-product installs.
- **Cons:** Conflates with Forge/beads; fights multi-repo under one board;
  reintroduces silent product-repo defaults.

### B. Project-level repo binding at `create_project` — **rejected**

- **Pros:** Binding before Initial plan; simple mental model.
- **Cons:** Encodes “one product repo per Project”; blocks multi-repo under
  one Project; remotes belong on claimable work, not the container.
  **Human steer (this revision): do not ship this.**

### C. Task-level repo at every materialization, with Initial plan as source

- **Pros:** Multi-repo Projects stay natural; supervisor pre-clones from Task
  binding; siblings can default from Initial plan Task for convenience.
- **Cons:** Needs an explicit “start planning” tool so Initial plan is not
  created unbound; every create path must carry or default a repo.

### D. Keep auto-seed; rely on prompt / escalate forever

- **Pros:** No schema change.
- **Cons:** Needs You on first claim remains the happy path — the pain that
  escalated this epic.

### E. Hybrid (recommended)

`create_project` = container only (no repo, **no** auto-seed). MCP `init_plan`
requires repo and seeds Initial plan **with** that Task binding. All other
claimable Task creates require a repo (or default from Initial plan Task).
Resolution: `pull_request` → Task repo → escalate. Forge unchanged.

---

## 4. Invariants to protect

| Invariant | Implication |
|---|---|
| **One state machine** | Task repo requiredness lives in `Board` create / Approve / split paths — not only UI or briefing prose. |
| **Agent is not a participant** | Worker never writes Task binding via honr MCP; operator `init_plan` / Approve set it on the host. |
| **Merging is human** | Binding does not auto-merge. |
| **Bot has no upstream write** | Task stores upstream (PR target) + optional fork (push); containment stays in GitHub permissions. |

Do not add `Project.product_repo`. Do not teach agents to guess `owner/name`
from the honr install.

---

## 5. Migration

| Population | Behavior |
|---|---|
| **New Projects** | `create_project` creates container only (no Initial plan seed). Operator calls `init_plan` with repo when ready to plan. |
| **Existing auto-seeded Initial plans** | Heal or one-shot: require `init_plan`-equivalent bind (UI/MCP) before dispatch; or keep escalate / Decision escape until bound. |
| **Existing cards with `pull_request`** | Unchanged — card facts win over Task repo field. |
| **Existing Tasks with no repo and no PR** | Legacy: `Ok(None)` → escalate until operator sets Task repo or answers Decision. |
| **Empty Settings Forge** | Unchanged — beads sync independent. |
| **Multi-repo under one Project** | Supported: different Tasks may carry different upstream/fork; no “one repo per Project” rule. |

Yaml `execution.agents.repo` stays bootstrap/seed convenience — not the
unbound fallback once Task binding exists.

---

## 6. Recommended path + follow-on DoDs

### Decision

Ship **E**: task-scoped remotes + `init_plan` + stop unbound auto-seed.

### Target happy path

```text
create_project(title, intent, …)     → Project container (no Task seed)
init_plan(project, repo, …)          → Initial plan Task with Task.repo set
dispatch Initial plan                → supervisor pre-clones from Task.repo
plan.json + docs PR → Review
Approve                              → sibling Tasks; each gets repo
                                       (explicit in ChildSpec / or default
                                        from Initial plan Task.repo)
```

### Resolution order (target)

```text
card.pull_request (base/head or URL stub)
  → else card.task_repo → RepoConfig
  → else Ok(None)  # legacy / misconfig → empty workdir + escalate
```

No Project product-repo step.

### Task repo shape

Same facts as today’s `RepoConfig`: `upstream` (`owner/name`, required),
optional `fork` (default same-repo), `base` (default `main`). Stored on the
claimable Task (Initial plan and impl cards). After report, `pull_request`
overrides for resume/rebase as today.

### Sibling defaulting

When Approve / split materializes children:

1. Prefer per-child repo in `plan.json` / `ChildSpec` when present (multi-repo).
2. Else copy the **Initial plan Task’s** (or splitting parent Task’s) repo.
3. Refuse materialization if neither yields a complete binding.

Never read a Project-level product-repo field (there isn’t one).

### Sibling Tasks (see `plan.json`)

| Key | Title | Depends on |
|---|---|---|
| `t1` | Task repo field on WorkItem + API surface | — |
| `t2` | Stop create_project auto-seed; add MCP `init_plan` | `t1` |
| `t3` | `resolve_card_repo` Task fallback + pre-clone | `t1` |
| `t4` | Require repo on ChildSpec / Approve / split / create-task | `t1`, `t2` |
| `t5` | Briefing, prompts, workflow docs + UI for `init_plan` | `t2`, `t3`, `t4` |

Approve on this card creates those Tasks. Implementation PRs land separately.
