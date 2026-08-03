# Plan: Settings — sandboxes and sidebar

## Context & Objectives

Today the cockpit is a single Board surface (`web/src/App.tsx` → `Cockpit`).
Sandbox create knobs live only in `honr.yaml` (`execution.agents.image` /
`policy` / `cpu` / `memory`), and the supervisor always builds one
`SandboxSpec` from that process config (`src/supervisor.rs` → `run_card`).
There is no way to keep more than one create profile, pick a global default,
or override the profile per Project without editing YAML and restarting.

This Project adds:

1. **App chrome** — a sidebar (or equivalent nav) so Board and Settings are
   separate surfaces; the board stays a board.
2. **Settings → Sandboxes** — the first real settings panel: list / create /
   manage named sandbox **profiles**, set a global default, and assign a
   profile on a Project (unset = use the global default).
3. **Persistence + supervisor** — Project override and global default must be
   readable where dispatch creates OpenShell sandboxes.

Non-goals for v1: multi-tenant auth, per-Task sandbox override, destroying
live OpenShell sandboxes from the UI without a confirm path.

---

## What a “sandbox” means here

A **sandbox profile** is a named create-spec used when the supervisor calls
`openshell sandbox create` for a card. It is **not** the per-card live
instance named `honr-card-{id}-a{n}` (that remains runtime state on
`WorkItem.environment`).

Minimum profile fields for v1 (mirrors `SandboxSpec` / `AgentConfig`):

| Field | Role |
|---|---|
| `id` | Stable key (string) |
| `name` | Human label |
| `image` | `--from` |
| `policy` | Policy path at create |
| `cpu` / `memory` | Optional resource caps |

Process-level knobs stay in `honr.yaml` (`enabled`, `repo`, `vertex`,
`engine`, budgets, `max_concurrent`, providers). Profiles do not replace those.

---

## Source of truth

**Prefer durable board state** for:

- the profile catalog
- the global default profile id
- each Project’s optional `sandbox_profile_id`

Rationale: Settings must mutate without rewriting `honr.yaml` and without a
restart. One clear source of truth beats splitting catalog across YAML + board.

`execution.agents` in `honr.yaml` remains:

- process config (auth, repo, engine, budgets, concurrency)
- **bootstrap / fallback**: if the catalog is empty at load, seed one profile
  from `image` / `policy` / `cpu` / `memory` and mark it default

Resolution at sandbox create (Project Task → containing Project):

```
Project.sandbox_profile_id
  → else BoardState.default_sandbox_profile_id
  → else yaml AgentConfig image/policy/cpu/memory
```

All mutations go through `Board` in `src/store.rs` (same invariant as every
other control-plane write). REST (and MCP if natural for cockpit) are thin.

---

## UI shape

- **Sidebar / nav**: Board | Settings. Do not turn the board header into a
  settings dashboard.
- **Settings page**: thin scaffolding; first real section is **Sandboxes**.
  Other tabs may be stubs (“coming soon”).
- **Project assignment**: picker on Project create/update or Project detail —
  optional profile; empty means global default. Show which default would apply.
- **Visual language**: match existing `web/` CSS (dense, terminal-adjacent).
  There is no PatternFly package in `web/` today — do not add one for v1;
  stay consistent with the current surface.

Destroying a **profile** that is the global default or referenced by a Project
must be refused or require an explicit reassignment path. Destroying **live**
OpenShell sandboxes from Settings is out of scope for v1 (non-goal).

---

## Tasks & Dependencies

```
[sidebar] ─────────────────────────────┐
                                       ├──► [sandboxes-ui]
[profiles-model] ──► [api-supervisor] ─┘
```

### Task `sidebar` — App chrome: Board vs Settings sidebar

- **Intent**: Introduce navigation so operators can switch between the Board
  cockpit and a Settings surface without stuffing settings into board chrome.
- **Dependencies**: none.
- **Definition of Done**: `npm --prefix web test` (or the repo’s existing web
  test script) passes; App renders a persistent Board | Settings nav; Board
  view still shows the existing Cockpit; Settings route/view renders with a
  Sandboxes section placeholder and at least one stub section; no PatternFly
  dependency added.

### Task `profiles-model` — Board-state sandbox profiles + Project override

- **Intent**: Persist a sandbox profile catalog, global default id, and
  optional Project `sandbox_profile_id` in durable board state, with YAML
  seed/fallback when the catalog is empty.
- **Dependencies**: none.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests
  cover seed-from-yaml, set default, set/clear Project override, and refuse
  deleting the default (or in-use) profile without reassignment; fields round-
  trip through board flush/load (JSON and/or SQLite path already used by Board).

### Task `api-supervisor` — API + supervisor resolve profile at create

- **Intent**: Expose profile CRUD / default / Project assignment through the
  control plane, and make `run_card` / sandbox create build `SandboxSpec` from
  the resolved profile.
- **Dependencies**: `profiles-model`.
- **Definition of Done**: `cargo test --offline --locked` and
  `cargo clippy --offline --all-targets -- -D warnings` pass; REST (and MCP if
  added) covered by tests for list/create/update/default/project assign;
  supervisor unit test proves Project override wins over global default, and
  unset Project uses default, when constructing create knobs; `docs/operating.md`
  documents the resolution order in one short subsection.

### Task `sandboxes-ui` — Settings Sandboxes panel + Project sandbox picker

- **Intent**: Wire the Sandboxes settings panel and Project-level assignment
  UI to the API so operators can manage profiles and defaults without YAML.
- **Dependencies**: `sidebar`, `api-supervisor`.
- **Definition of Done**: `npm --prefix web test` passes; Settings → Sandboxes
  lists profiles, supports create/edit, and sets the global default; Project
  UI can set/clear a sandbox profile (unset labeled as using the global
  default); deleting a live OpenShell sandbox is not offered in this panel.

---

## Out of scope (v1)

- Per-Task sandbox override
- Multi-tenant / authz for Settings
- UI to delete live OpenShell sandboxes (card environments) without a later
  confirm-path Project
- Hot-reload of `honr.yaml` process knobs (repo, vertex, engine)
