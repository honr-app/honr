# Plan: Task repo binding and Initial plan seeding

**Status:** plan only — Approve materializes Tasks. No product code in this PR.

**Goal:** every agent-claimable Task gets an unambiguous product repository
(clone target) without brittle first-run escalations, while keeping
`create_project` → Initial plan → Approve as the default loop.

**Board epic:** [#280](https://github.com/shanemcd/honr/issues/280). Related
empty-state onboarding: [#279](https://github.com/shanemcd/honr/issues/279).
(GitHub Issue #182 is unrelated — closing linked Issues on Done.)

**Out of scope here:** GitLab, multi-repo Tasks under one Project, rewriting
the state machine, shipping the binding implementation (that is the Task DAG
in `plan.json`).

---

## Verdict

**Require a Project-level product-repo binding at `create_project`. Keep
auto-seeding Initial plan. Resolve remotes as card `pull_request` → else
Project binding. Tasks inherit and cannot omit.**

Settings → Forge stays beads sync only. Prefer one coherent default (one
product repo per Project) over install-wide work remotes or prompt-parsed
clones as the happy path.

---

## 1. Current model (with code paths)

### Settings → Forge ≠ work remotes

| Piece | Path | Role today |
|---|---|---|
| `WorkspaceBinding` | `src/model.rs` | `forge` + `beads_sync_repo` only |
| Board SoT | `BoardState.workspace` in `src/store.rs` | Seeded from `GITHUB_REPOSITORY` or yaml upstream |
| REST | `GET`/`PUT /api/workspace` | Settings → Forge |
| UI copy | `web/src/components/Settings.tsx` | Explicitly: product remotes live on card `pull_request` |

Model comment: work remotes are **not** stored on WorkspaceBinding — they live
on each card's `PullRequest` after the agent reports.

### Project / Task fields

`WorkItem` has no structured `repo_url` / `upstream` / `fork`. Soft guidance
lives in `project_prompt` (seeded from `DEFAULT_PROJECT_PROMPT` in
`src/model.rs`), which tells operators to name an exact `owner/name` in prose.

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

Sibling Tasks after Approve do **not** copy the Initial plan card's
`pull_request`. Yaml `execution.agents.repo` is legacy/optional and is **not**
applied when `resolve_card_repo` returns `None` (`run_card` sets
`agents.repo = Default`).

### What the sandbox briefing receives

| Case | Supervisor behavior | Briefing (`remotes_briefing_lines`) |
|---|---|---|
| Card has remotes | Pre-clone via `clone_script` | Named `origin` / `upstream` / base |
| Unbound + Decision note `Clone owner/name` | Empty workdir | Clone that target; do not re-escalate |
| Unbound + silent prompt | Empty workdir | Escalate — never invent `owner/name` |
| Unbound + prompt names clone | Empty workdir; **agent** clones | Same Remotes text; success depends on prose |

Cold briefing also pastes Project prompt + Plan (`briefing` in
`src/supervisor.rs`). Initial plan cards must write `plan.json` + one docs PR
+ `report.json` (not `split.json`).

### create_project + Initial plan auto-seed

`Board::create` (Project root) always calls `seed_initial_plan_task`: titles
`Initial Plan for <Project>`, intent requires plan.json + docs PR, lands in
Backlog. MCP `create_project` accepts `title`, `intent`, optional
`project_prompt` — **no repo arg**.

Happy path in `docs/workflow.md`: create → dispatch Initial plan → Approve →
dispatch Tasks.

---

## 2. Failure mode

Missing or ambiguous clone target on first claim → agent writes
`escalate.json` → `NeedsHuman` ("Needs You"). That is intentional containment
(agents must not invent this product's own repo from ambient context), but it
fires on the **first** card of a new Project whenever the operator has not yet
stuffed `owner/name` into `project_prompt`.

Auto-seeding Initial plan makes the awkwardness acute: the planning card is
claimable before any human has named remotes. Answering Needs You with
`Decision: Clone …` unblocks **that** card only; siblings still rediscover.

---

## 3. Options (trade-offs)

### A. Install-wide default workspace required before any dispatch

Force Settings (or yaml) work remotes before claim.

- **Pros:** One knob; first clone always works for single-product installs.
- **Cons:** Conflates with Forge/beads (already separated); fights multi-repo
  and [Generalize](generalization.md); reintroduces silent
  `shanemcd/honr`-shaped defaults.

### B. Project-level repo binding required at `create_project`

MCP/UI require `owner/name` (+ optional fork/base) when creating a Project.

- **Pros:** Binding exists before Initial plan can run; clear ownership;
  multi-repo = multiple Projects.
- **Cons:** Slightly heavier create; empty-state onboarding must collect the
  field (#279).

### C. Task-level repo required at materialization (Approve / split) with Project inheritance

- **Pros:** Explicit per-Task; inheritance reduces typing.
- **Cons:** Alone, Initial plan still has nowhere to inherit from unless
  Project also binds — chicken/egg.

### D. Stop auto-seeding Initial plan; explicit "start planning" that carries repo

- **Pros:** No claimable card until the operator is ready with a repo.
- **Cons:** Breaks the documented happy path; "skip planning by default" feels
  wrong for a Plan-first board; still need a durable binding somewhere.

### E. Hybrid (recommended shape)

`create_project` requires product-repo binding (B); keep Initial plan
auto-seed; Tasks inherit and cannot omit (C); escalate remains last resort.

---

## 4. Invariants to protect

| Invariant | Implication for binding |
|---|---|
| **One state machine** | Binding rules live in `Board` / `machine` paths — not only UI validation or briefing prose. |
| **Agent is not a participant** | Supervisor still claims/reports; agent never writes Project binding via MCP. |
| **Merging is human** | Binding does not auto-merge; Approve still surfaces PRs. |
| **Bot has no upstream write** | Cross-fork: Project stores upstream (PR target) + optional fork (push); containment stays in GitHub permissions. |

Do not collapse Settings → Forge into product remotes. Do not teach agents to
guess `owner/name` from the honr install itself.

---

## 5. Migration

| Population | Behavior |
|---|---|
| **New Projects** | `create_project` refuses without product repo (`upstream` required; `fork` optional, default same-repo; `base` default `main`). |
| **Existing Projects unbound** | Dispatch gate or one-shot operator bind (UI/MCP `update`); until bound, keep today's escalate / Decision-note path so in-flight cards do not wedge. |
| **Existing cards with `pull_request`** | Unchanged — card facts win. |
| **Empty Settings Forge** | Unchanged — beads sync independent of product repo. |
| **Multi-repo future (Generalize)** | Coherent default remains **one product repo per Project**. Task-level override is a later opt-in, not the v1 menu. |

Yaml `execution.agents.repo` stays bootstrap/seed for beads or local
self-host convenience — not the supervisor's unbound fallback.

---

## 6. Recommended path + follow-on DoDs

### Decision

Ship **E**: Project product-repo binding at create; keep Initial plan
auto-seed; `resolve_card_repo` = card `pull_request` else Project binding;
materialized Tasks inherit; unbound Project cannot enter the happy-path
dispatch without an explicit legacy escape.

### Resolution order (target)

```text
card.pull_request (base/head or URL stub)
  → else Project.product_repo → RepoConfig
  → else Ok(None)  # legacy / misconfig → empty workdir + escalate
```

Supervisor pre-clones whenever the resolved `RepoConfig` is complete — Initial
plan included — so first claim does not depend on prompt archaeology.

### Sibling Tasks (see `plan.json`)

| Key | Title | Depends on |
|---|---|---|
| `t1` | Project product-repo model + create gate | — |
| `t2` | resolve_card_repo Project fallback + pre-clone | `t1` |
| `t3` | Approve / split inheritance + dispatch guards | `t1` |
| `t4` | Briefing + DEFAULT_PROJECT_PROMPT for bound Projects | `t2` |
| `t5` | UI create Project + docs (workflow / concepts / #279 hook) | `t1`, `t4` |

Approve on this card creates those Tasks. Implementation PRs land separately.
