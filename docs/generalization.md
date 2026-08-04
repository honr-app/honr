# Investigation: generalize honr beyond this stack

**Status:** Report-driven forge binding: each card stores `pull_request`
(`url` + GitHub-shaped `base`/`head`) from `report.json` (schema at
`docs/schemas/report.schema.json`). First clone is prompt-only — supervisor
does not pre-clone without card remotes. Settings → Forge is **provider +
beads sync only**. Settings → **OpenShell** shows gateway health (`openshell
status`) and an optional CLI binary override. Settings → **Agent runtime**
holds engine / Vertex / providers / budgets / **branch_prefix** /
**quality_gates** (seeded from yaml; Board SoT). Yaml `execution.agents.repo`
is legacy/optional. **Second-repo proof:** done — see
[second-repo-proof.md](./second-repo-proof.md) (Board card #180 →
`clankrshq/honr-sandbox-probe` PR #2).

**Goal:** a fresh install can drive **any GitHub-hosted repo** (and eventually
**many** on one board) with configurable OpenShell/runtime, without hardcoding
`shanemcd/honr`, Colima/gateway paths, or other machine-specific defaults.

**Out of scope for this Project’s implementation cards:** GitLab (future forge
seam only), rewriting the board state machine, and shipping generalization
beyond what each Task’s DoD says.

---

## Verdict

Most **logic** is already parameterized (`RepoConfig`, `VertexConfig`, sandbox
profiles, webhook payload matching). What blocks a second operator — and a
second *concurrent* repo — is:

1. **Shipped values and silent fallbacks** that assume Shane’s machine
   (`shanemcd/honr`, `clankrshq`, `shanemcd-rh`, provider names) — mostly fixed
   for beads (`workspace-binding`).
2. **Process knobs stuck in `honr.yaml`** while Settings only covers sandbox
   create-specs — operators must edit YAML + restart for auth/runtime.
3. **Host OpenShell/Docker** living entirely outside honr (correct), but
   documented only as a Colima/podman/macOS playbook.
4. **Briefing / image assumptions** tuned for building honr itself (`cargo
   test --offline`, `honr-sandbox:latest`).
5. **Install-wide Workspace `upstream`/`fork` as the only work binding** (#169 /
   #170) — too rigid for multi-repo boards (decision in
   [Multi-repo forge model](#multi-repo-forge-model-decision-card-175) below).

Prefer **extending Settings** (narrowed Workspace + Agent runtime + OpenShell
status) and **Project fields** for work remotes — not a parallel config UI.
Keep `honr.yaml` as bootstrap/fallback.

---

## Multi-repo forge model (decision, card #175)

### (1) What is wrong with a single install-wide upstream/fork

| Problem | Why it bites |
|---|---|
| One board, one work target | Rebinding Settings → Workspace to run Project B against `acme/widgets` unbinds Project A’s clone/PR/rebase path. Concurrent multi-repo dispatch is impossible. |
| Couples beads Issues to work remotes | `beads_sync_repo` defaults to Workspace `upstream`. Honr’s Issue mirror often *is* `shanemcd/honr` while a second product repo is the PR target — one field cannot mean both. |
| Contradicts payload-driven merge | `complete_for_merged_pr` / webhook Done already match card `pr_url`, not a global upstream string (`src/store.rs`, `src/api.rs`). Install binding is unused on the path that matters most. |
| Second-repo proof as “rebind install” | Plan Task #174 as written forces thrashing the whole Workspace. That proves single-repo portability, not multi-repo. |
| Settings copy overclaims | Workspace panel today: “supervisor and beads use them after reload” and “Agents stay disabled until upstream and fork are set” — true for today’s code, wrong as a long-term product rule. |

**Recommendation:** drop the assumption that Settings Workspace *is* the work
clone/PR binding. Keep Settings for **forge identity + install defaults + ops**.
Put **work remotes on the Project** (same shape as today’s `RepoConfig`), and
**learn from the card’s `pr_url` + sandbox remotes** once a PR exists.

### (2) How clone / push / rebase / webhook / PR-complete learn the repo

No global work binding required. Resolve **per card** (via its Project +
optional `pr_url`):

```
card.pr_url  →  parse owner/repo as upstream; fork = {Workspace.fork_owner}/{repo}
       ↓ if missing
Workspace / yaml default (upstream, fork, base)
       ↓ if missing
refuse with error naming missing pr_url / Workspace defaults
```

No per-Project `repo` field — the card's reported PR URL is the multi-repo
signal (binding note on #175).

| Path | How it learns the repo | Notes |
|---|---|---|
| **First clone** (no `pr_url`) | Project `fork` / `upstream` / `base` (else install default) | Supervisor `clone_script` / `refresh_script` must take **resolved** `RepoConfig`, not process-global Workspace alone (`src/supervisor.rs`). |
| **Push** | Sandbox `origin` = fork (clone target). Agent briefing: push to `origin` (the fork). | Invariant: bot has no write to upstream. |
| **Rebase (supervisor + agent)** | Always onto **PR base repo** `upstream/{base}`, never `origin/{base}` alone (fork base freezes). Upstream comes from resolution above. | Existing scripts already use `cfg.repo.upstream`; change is *where* `cfg.repo` is filled. |
| **Resume after PR** | Parse card `pr_url`: `https://github.com/{owner}/{repo}/pull/{n}` → upstream `owner/repo`. Fork: prefer Project.fork; else `gh pr view <url> --json headRepository,headRepositoryOwner` / existing sandbox `origin`; else refuse. | Do not require Settings upstream to match the PR. |
| **PR lookup** (`gh pr list --repo …`) | Resolved upstream for that card | Today hardcoded from install `cfg.repo.upstream`. |
| **PR-complete (merge webhook)** | Card `pr_url` match only (`normalize_pr_url` / `complete_for_merged_pr`) | **Already correct** — no global binding. Keep. |
| **MainAdvanced / Review rebase catch-up** | Each Review card’s resolved upstream/base (from `pr_url` or Project) | Must not rebase every Review card onto the install Workspace upstream when Projects differ. |
| **Webhook forward (ops)** | Operator runs one forwarder **per upstream they care about**. Settings shows a **template** (`--repo=<owner/name>`), not a single Shane (or even single Workspace) repo as the only instruction. | Multi-repo installs may need multiple forwarders or a prod webhook app. |

**What remains in Settings (install-wide):**

| Keep | Drop as *required* work SoT |
|---|---|
| Forge provider (`github` now; GitLab disabled/future) | Requiring complete install `upstream`+`fork` before any agent runs |
| Bot / token docs + OpenShell GitHub provider **names** | Treating Workspace upstream as beads *and* every Project’s PR target |
| OpenShell connectivity / health | |
| Agent runtime (engine, Vertex, budgets, providers) | |
| Sandbox profile catalog | |
| **Beads sync repo** (honr Issue mirror) — explicit, not “same as work upstream” | |
| Optional **default** upstream/fork/base for new Projects / yaml migration | |

Prefer extending today’s Settings chrome over a parallel config UI. Narrow the
Workspace panel copy and fail-closed rules in a follow-on Task — **not** in #175.

### (3) Briefing / `project_prompt` language (rebase onto the correct upstream)

Standing rules (product invariants — keep in default `project_prompt` or
briefing template):

1. **Remote names:** `origin` = fork (push); `upstream` = PR base repository
   (fetch/rebase). Never treat the fork’s default branch as the merge base.
2. **Rebase target:** `git fetch upstream <base> && git rebase upstream/<base>`
   (or the conflict-resume wording already in `briefing()`).
3. **When `pr_url` is set:** name the URL; “update that PR”; push to `origin`;
   open/update against the **parsed** upstream `owner/repo`, base from Project
   or PR baseRef.
4. **When a fork is involved:** “The fork’s base freezes at create time; rebase
   onto `upstream/<base>`, not `origin/<base>`.” (Already in supervisor comments
   / conflict briefing — keep, interpolate Project-resolved names.)
5. **Per-Project `project_prompt`:** for a non-default repo, operators should
   state the canonical upstream/fork once (e.g. “This Project opens PRs against
   `acme/widgets` from fork `bot/widgets`”). Briefing still injects the resolved
   values so agents do not rely on memory alone.
6. **Quality gates:** remain Project/prompt territory (`briefing-repo-agnostic`)
   — do not hardcode `cargo` for every repo.

### (4) Plan Tasks #170–#174 — revise / cancel / re-block

Board cards under Project **Generalize honr beyond this stack**:

| Card | Key | Disposition | Updated intent / DoD |
|---|---|---|---|
| **#170** | `settings-workspace` | **Revise (already merged).** No further UI expansion that treats install upstream/fork as the only work binding. Follow-on narrows copy + fail-closed. | **DoD (residual / follow-on `narrow-workspace-settings`):** (1) Workspace panel states work remotes are **per-Project**; install upstream/fork are **optional defaults**. (2) Agents may run with empty install upstream/fork when the **Project** binding is complete (fail-closed names Project fields). (3) Webhook hint is a placeholder template, not “the” configured install upstream as sole ops path. (4) Beads sync field stays install-wide and is labeled as Issue mirror, not PR target. *No new Settings app.* |
| **#171** | `settings-agent-runtime` | **Shipped.** Settings → Agent runtime; Vertex location from durable config; providers from Board. | **DoD:** met (panel + Board REST; `setup_agy_auth` uses configured location; providers at sandbox create from durable config). |
| **#172** | `openshell-ops-surface` | **Shipped.** Settings → OpenShell health + binary; role-based ops docs; Shane values labeled example. | **DoD:** met (panel; operating.md roles; sandbox-stack example table; dispatch still gates on `healthy()`). |
| **#173** | `briefing-repo-agnostic` | **Shipped.** | **DoD:** (1) Empty `quality_gates` omits mandatory cargo from briefing (test). (2) `branch_prefix` config default drives `{prefix}/card-N` (override via Settings/yaml). (3) Verdict paths unchanged. (4) Briefing variants covered by `cargo test --offline`. |
| **#174** | `second-repo-proof` | **Done** — [second-repo-proof.md](./second-repo-proof.md). | **DoD met:** (1) Run record names non-Shane upstream/fork `clankrshq/honr-sandbox-probe`. (2) Board card **#180** → Review with https://github.com/clankrshq/honr-sandbox-probe/pull/2. (3) Install Workspace/beads stayed `shanemcd/honr`; work remotes were the probe. (4) Follow-ups #256 / #257. |

**New sibling Task (create after Approve of this decision):**

| Key | Title | Intent | blocked_by | DoD (mechanical) |
|---|---|---|---|---|
| `project-repo-binding` | Per-Project repo binding + resolve from `pr_url` | Add Project fields `upstream`/`fork`/`base` (seed from install default / yaml). Supervisor resolves per card: `pr_url` → Project → optional Workspace default → refuse. Clone/push/rebase/PR-lookup use resolved config. Agents no longer require install Workspace upstream/fork when Project is complete. | (none; #169 done) | (1) Project with its own upstream/fork dispatches clone against that fork URL (`rg`/unit test on script or resolve helper). (2) Card with `pr_url` on repo A does not use install Workspace upstream B for `gh pr list` / rebase target (test). (3) Empty Project + empty default → actionable error naming Project fields. (4) Migration: existing single-repo installs seed Project from Workspace/yaml so Shane self-host keeps working. (5) `cargo test --offline` + clippy `-D warnings` clean. |

Optional small follow-on: `narrow-workspace-settings` (UI copy + fail-closed) if not folded into `project-repo-binding`.

### (5) GitLab

**Out of scope** for every Task in this Project except as a named Settings /
`forge` enum seam (`github` now; `gitlab` listed disabled). No GitLab API,
webhooks, or clone URLs in DoDs above.

---

## Inventory (with paths)

### GitHub — API, webhooks, `gh`, PRs, fork/upstream

| Assumption | Where | Today |
|---|---|---|
| Upstream / fork / base | Board Workspace + `honr.yaml` `execution.agents.repo`; `RepoConfig` | **Install-wide** SoT after #169/#170 — **to become Project + pr_url** (#175) |
| Clone fork, PR → upstream | `src/supervisor.rs` `clone_script`, `briefing`, `pr_lookup_script` | Uses process `AgentConfig.repo` from Workspace overlay |
| Cross-fork head `owner:branch` | supervisor PR scripts / tests | Design invariant |
| Webhook ingress | `POST /api/webhooks/github` in `src/api.rs` | Payload-driven; **no** repo allowlist; **no** signature verify |
| PR complete on merge | `src/store.rs` `complete_for_merged_pr` / `normalize_pr_url` | Matches card `pr_url` — **already multi-repo-safe** |
| Dev forwarder example | `docs/operating.md` + Settings → Workspace hint | Placeholder / configured upstream |
| PR label UI | `web/src/components/Card.tsx` `prLabel` | Generic `github.com/owner/repo/pull/N` |

### Repo identity

| Assumption | Where | Today |
|---|---|---|
| `shanemcd/honr` + `clankrshq/honr` + `main` | `honr.yaml` | Shipped install values / seed |
| Empty defaults until yaml | `RepoConfig::default` | Safe schema default |
| Test fixtures | `schema.rs` `workable()`, supervisor/store/api tests | Convenient, not runtime |
| Agents `enabled: true` | shipped `honr.yaml` | Footgun for fresh clones (`docs/operating.md`) |
| Per-Project repo | — | **Missing** — required by #175 decision |

### OpenShell — gateway, Docker, image, policy, credentials

| Assumption | Where | Today |
|---|---|---|
| CLI binary `openshell` | `src/openshell.rs` | Hardcoded name; gateway URL **not** in honr |
| Health = `openshell status` | openshell + supervisor dispatch gates | Host CLI config (mTLS under `~/.config/openshell/`) |
| Providers list | `honr.yaml` `providers: [vertex, gh-clankr, cursor-honr]` | YAML names must match **local** gateway registrations |
| Image / policy / cpu / memory | yaml seed → Settings sandbox profiles | **Already** Settings + Project override |
| Policy allowlists | `sandbox/policy.yaml` (Vertex, GitHub, crates, Cursor) | Seed; catalog stores inline YAML |
| Image build | `sandbox/Containerfile` | Honr-toolchain image, not Shane identity |
| Metadata shim | `sandbox/metadata-shim.py`; supervisor `GCE_METADATA_HOST=127.0.0.1:8127` | Hardcoded port/path |
| Colima / `DOCKER_HOST` / `gateway.env` | `docs/sandbox-stack.md`, `docs/operating.md`, beads memory — **not** in source | Host-only |
| Homebrew tap `shanemcd/openshell` | sandbox-stack doc | Docs-only |

### Vertex / agent auth

| Assumption | Where | Today |
|---|---|---|
| Project / location / model | `honr.yaml` `vertex`; `agent_env` in supervisor | YAML |
| Always Vertex env for Claude path | `CLAUDE_CODE_USE_VERTEX=1` in supervisor | Hardcoded posture |
| `setup_agy_auth` location `"global"` | supervisor | **Ignores** `cfg.vertex.location` |
| Host agy token path | `$HOME/.gemini/antigravity-cli/...` | Host-path assumption |
| Default engine | yaml `engine: cursor`; Project override in UI | Partial |
| Provider create recipes | sandbox-stack doc | `shanemcd-rh`, `gh-clankr` |

### Beads ↔ GitHub Issues

| Assumption | Where | Today |
|---|---|---|
| Fallback repo `shanemcd/honr` | ~~`src/beads.rs`~~ **removed** — Workspace / env / refuse | Fixed in `workspace-binding` |
| Env `GITHUB_REPOSITORY` / `OWNER` / `REPO` | beads + Workspace `beads_sync_repo` | Env wins; else Workspace / yaml |
| `bd config github.owner/repo` | live e2e test uses `clankrshq`/`honr` | External to honr config |
| Mirror scheduling | `src/store.rs` | Generic once URL known |

### UI copy & Settings today

| Surface | Status |
|---|---|
| Settings → **Sandboxes** | Real: profile CRUD, default, inline policy |
| Project sandbox picker | Real (`ProjectSandboxPicker`) |
| Project engine select | Real in Detail drawer |
| Settings → **Workspace** | Real (#170) — install upstream/fork/base + beads sync (**narrow after #175**) |
| Settings → **OpenShell** | Real (#172) — gateway health + optional CLI binary; host Docker/Colima stays docs |
| Repo / vertex / providers / budgets | Settings → Agent runtime (#171); yaml seed |
| Production UI strings | No Shane repo names (fixtures in `web/ui-fixture.mjs` / tests only) |

### Briefing & tests

| Assumption | Where | Today |
|---|---|---|
| Branch `{prefix}/card-{id}`, sandbox `{prefix}-card-…` | supervisor via `branch_prefix` | **Configurable** (default `honr`) on Agent runtime / yaml |
| Quality gates (e.g. cargo) | `briefing()` from `quality_gates` + Project prompt | **No hardcoded cargo** — empty gates omit toolchain; yaml/Settings list or Project prompt names them |
| Verdict paths `/sandbox/.honr/*.json` | supervisor constants | Product protocol (keep) |
| Widespread `shanemcd/honr` URLs | supervisor/store/api/beads tests | Fixtures |

### Existing seams worth extending

1. `RepoConfig` / `VertexConfig` already threaded through clone, briefing, env.
2. Sandbox profile catalog + Project override + yaml seed.
3. Per-Project `engine` / `sandbox_profile_id` / `project_prompt` — **same pattern for repo**.
4. Webhooks match `pr_url`, not a hardcoded upstream string.
5. Settings shell already has Workspace + Sandboxes — extend/narrow; do not add a second config app.

---

## Classification

### (A) Must be per-install Settings / config

- Forge provider (**GitHub now**; GitLab = future enum value only).
- Bot / token wiring (OpenShell GitHub provider name + docs for credential key `GITHUB_TOKEN`/`GH_TOKEN`).
- Fork strategy note (cross-fork PR; bot must not write upstream) — configurable *repos*, invariant *model*.
- **Beads GitHub sync target** (Issue mirror for the board — **independent** of work remotes).
- Optional **default** upstream/fork/base for new Projects (yaml seed / migration convenience) — **not** required for agents when Project binding is complete.
- Default agent engine; Vertex project / location / model (or “Cursor-only” installs).
- OpenShell provider name list; agents enabled; concurrency / budgets / timeout.
- Default sandbox profile (already A).
- Optional: `openshell` binary path if not on `PATH`.

### (B) Must be per-Project overrides

- `sandbox_profile_id` (exists).
- `engine` (exists).
- `project_prompt` / Plan (exists) — standing policy including quality gates for *this* codebase.
- **`upstream` / `fork` / `base` work remotes** — derived per card from `pr_url`
  (+ Workspace fork-owner default); not a Project struct field.

### (C) May remain code defaults with clear override

- Base branch default `main`.
- Seed image / policy path / cpu / memory.
- Vertex location/model defaults when Vertex is used.
- Branch/sandbox name prefixes (`honr/card-…`) — **Settings / yaml `branch_prefix`** (default `honr`).
- Install-wide `quality_gates` list — clear for non-Rust; prefer Project `project_prompt` for per-repo gates.
- Metadata shim listen address.
- `openshell` CLI name.
- Test fixture owner/repo strings (not runtime).

### (D) True product invariants (stay hardcoded)

- All mutations through `Board` / `machine` (no transport-local state machine).
- Agent is material: no agent→honr MCP path.
- Liveness and cost observed from agent output (no fake keepalives).
- Human merges; Approve surfaces PR, never merges.
- Bot containment via forge permissions (no write to upstream).
- Verdict protocol: `report.json` / `plan.json` / `split.json` / `escalate.json` under `/sandbox/.honr/`.
- Hangs-as-failure; every OpenShell call has a deadline.
- Rebase onto PR-base upstream remote, not the fork’s frozen default branch.
- GitLab **not** implemented in this Project — only a named seam in Settings IA.

---

## Proposed Settings IA (fresh system)

Extend the existing Settings chrome (`web/src/components/Settings.tsx`):

| Section | Contents | vs Project picker |
|---|---|---|
| **Workspace** (narrowed in #175) | Forge: GitHub; **beads sync repo**; optional default upstream/fork/base; webhook **template**. | Defaults only. **Work remotes from card `pr_url`** (+ fork-owner derive). |
| **Sandboxes** (exists) | Profiles: image, inline policy, cpu, memory; set default. | Project overrides profile. |
| **Agent runtime** (new, #171) | Default engine; Vertex project/location/model; OpenShell provider names; enabled / concurrency / budgets. | Project may override engine only (keep). |
| **OpenShell** (new, thin, #172) | Read-only gateway health (`openshell status` summary); link to ops doc; optional binary path. **No** Colima path editor — host env stays host. | Global. |

**Project Detail** (not Settings): engine / sandbox picker as today. Work remotes
are **not** Project fields — derive from card `pr_url` and Workspace fork-owner
defaults (see §2).

**Not** a parallel “config app.” YAML remains cold bootstrap: seed Workspace
defaults + each new Project’s repo when empty.

---

## Workspace binding field map (as shipped in #169/#170; post-#175 semantics)

Durable state lives on the Board (`BoardState.workspace` / SQLite meta
`workspace_binding`). `honr.yaml` `execution.agents.repo` is bootstrap only.

| Field | Type | Seed / read order | Role after #175 |
|---|---|---|---|
| `forge` | string | default `github` | Settings IA seam (GitLab later) — **keep install-wide** |
| `upstream` | `owner/name` | yaml `execution.agents.repo.upstream` | **Optional default** for new Projects; not sole agent SoT |
| `fork` | `owner/name` | yaml `execution.agents.repo.fork` | **Optional default** for new Projects |
| `base` | branch | yaml `base` or `main` | Default base when Project omits |
| `beads_sync_repo` | optional `owner/name` | `GITHUB_REPOSITORY` env at seed, else was `upstream` | **Issue mirror only** — set explicitly; do not imply work target |

**Resolution for agents (target after `project-repo-binding`):** card `pr_url` →
Project repo → Workspace default → refuse (name missing **Project** fields). No
`shanemcd/honr` default.

**Resolution for beads Issue URLs (unchanged intent):** `GITHUB_REPOSITORY` env →
Workspace `beads_sync_repo` → optional default upstream → refuse.

**Seed once:** `Board::seed_workspace_binding_if_empty` on load (mirrors
sandbox profile seed). After `project-repo-binding`, also seed Project.repo from
Workspace/yaml when empty so existing single-repo installs keep working.

---

## Migration path (today’s self-hosting keeps working)

1. **Read order (today):** board durable workspace if complete → else `honr.yaml`
   → else refuse. **After `project-repo-binding`:** per-card resolve as in §2;
   Workspace/yaml become defaults only.
2. **Seed once:** on load, if workspace empty and yaml has upstream/fork, copy
   into board state. On Project create / migrate, copy Workspace default into
   Project.repo when Project has no binding.
3. **Env bridge:** honor `GITHUB_REPOSITORY` / `HONR_DATABASE_URL` / `HONR_PORT`
   as today; beads sync should be an **explicit** Issue-mirror field (often still
   the honr repo while work Projects point elsewhere).
4. **Host OpenShell unchanged:** `~/.config/openshell/`, `DOCKER_HOST`,
   Colima/podman remain operator setup; rewrite ops docs to be role-based with
   Shane’s stack as *one worked example* (#172).
5. **Shipped yaml:** `enabled: false` in examples; real deploys keep local
   overrides uncommitted.
6. **Tests:** keep example `owner/repo` fixtures; add tests for Project override
   and `pr_url` resolve; missing binding errors instead of Shane fallback.

---

## Phased roadmap (second GitHub repo before any GitLab)

1. ~~**Workspace binding + kill fallbacks** (#169)~~ — landed.
2. ~~**Settings → Workspace** (#170)~~ — landed; **narrow** after #175 (see table).
3. **`project-repo-binding`** (new) — Project remotes + `pr_url` resolve; fail-closed without install-wide requirement.
4. **Settings → Agent runtime** (#171) — providers / Vertex / engine / budgets.
5. **OpenShell ops surface + docs** (#172) — health in Settings; de-Shane ops docs.
6. ~~**Repo-agnostic briefing / gates** (#173)~~ — `branch_prefix` + `quality_gates` on Agent runtime; no hardcoded cargo.
7. ~~**Second-repo proof** (#174)~~ — landed; see [second-repo-proof.md](./second-repo-proof.md).

GitLab: mention only as `forge: github | (future) gitlab` — no Tasks implement it.

---

## Approve checklist (card #175)

A human can Approve this multi-repo decision when:

- [ ] Install-wide upstream/fork is rejected as the long-term work SoT; problems in §1 match reality.
- [ ] Resolve order in §2 (pr_url → Project → default → refuse) is acceptable; Settings retain tokens/forge/OpenShell/engines/beads sync.
- [ ] Briefing / `project_prompt` rebase language in §3 is enough to implement without another research card.
- [ ] Task dispositions for #170–#174 + new `project-repo-binding` look right; GitLab stays a named seam only.
- [ ] No Settings UI rewrite required to accept this doc; implementation is follow-on Tasks.
