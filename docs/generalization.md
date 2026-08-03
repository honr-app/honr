# Investigation: generalize honr beyond this stack

**Status:** plan only — approve to materialize Tasks. No product code in this PR.

**Goal:** a fresh install can drive **any GitHub-hosted repo** with a configurable
OpenShell/runtime, without hardcoding `shanemcd/honr`, Colima/gateway paths, or
other machine-specific defaults.

**Out of scope here:** GitLab (future forge seam only), rewriting the board state
machine, and shipping the generalization implementation (that is the Task DAG
below / in `plan.json`).

---

## Verdict

Most **logic** is already parameterized (`RepoConfig`, `VertexConfig`, sandbox
profiles, webhook payload matching). What blocks a second operator is:

1. **Shipped values and silent fallbacks** that assume Shane’s machine
   (`shanemcd/honr`, `clankrshq`, `shanemcd-rh`, provider names).
2. **Process knobs stuck in `honr.yaml`** while Settings only covers sandbox
   create-specs — operators must edit YAML + restart for repo/auth.
3. **Host OpenShell/Docker** living entirely outside honr (correct), but
   documented only as a Colima/podman/macOS playbook.
4. **Briefing / image assumptions** tuned for building honr itself (`cargo
   test --offline`, `honr-sandbox:latest`).

Prefer **extending Settings** (Workspace + Agent runtime + OpenShell status)
over a parallel config UI. Keep `honr.yaml` as bootstrap/fallback.

---

## Inventory (with paths)

### GitHub — API, webhooks, `gh`, PRs, fork/upstream

| Assumption | Where | Today |
|---|---|---|
| Upstream / fork / base | `honr.yaml` `execution.agents.repo`; `src/schema.rs` `RepoConfig` | YAML; restart required |
| Clone fork, PR → upstream | `src/supervisor.rs` `clone_script`, `briefing`, `pr_lookup_script` | Uses yaml `RepoConfig` (generic) |
| Cross-fork head `owner:branch` | supervisor PR scripts / tests | Design invariant |
| Webhook ingress | `POST /api/webhooks/github` in `src/api.rs` | Payload-driven; **no** repo allowlist; **no** signature verify |
| PR complete on merge | `src/store.rs` `complete_for_merged_pr` / `normalize_pr_url` | Matches card `pr_url` |
| Dev forwarder example | `docs/operating.md` | Hardcoded `--repo=shanemcd/honr` |
| PR label UI | `web/src/components/Card.tsx` `prLabel` | Generic `github.com/owner/repo/pull/N` |

### Repo identity

| Assumption | Where | Today |
|---|---|---|
| `shanemcd/honr` + `clankrshq/honr` + `main` | `honr.yaml` | Shipped install values |
| Empty defaults until yaml | `RepoConfig::default` | Safe schema default |
| Test fixtures | `schema.rs` `workable()`, supervisor/store/api tests | Convenient, not runtime |
| Agents `enabled: true` | shipped `honr.yaml` | Footgun for fresh clones (`docs/operating.md`) |
| Per-Project repo | — | **Missing** (process-wide only) |

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
| Fallback repo `shanemcd/honr` | `src/beads.rs` (issue URL + push/sync) when `GITHUB_REPOSITORY` unset | **Hardcoded — high severity** |
| Env `GITHUB_REPOSITORY` / `OWNER` / `REPO` | beads | Env-only; no yaml stanza |
| `bd config github.owner/repo` | live e2e test uses `clankrshq`/`honr` | External to honr config |
| Mirror scheduling | `src/store.rs` | Generic once URL known |

### UI copy & Settings today

| Surface | Status |
|---|---|
| Settings → **Sandboxes** | Real: profile CRUD, default, inline policy |
| Project sandbox picker | Real (`ProjectSandboxPicker`) |
| Project engine select | Real in Detail drawer |
| Settings → **General** | **Stub** (“soon”) |
| Repo / vertex / providers / budgets | **Not in Settings** — yaml only |
| Production UI strings | No Shane repo names (fixtures in `web/ui-fixture.mjs` / tests only) |

### Briefing & tests

| Assumption | Where | Today |
|---|---|---|
| Branch `honr/card-{id}`, sandbox `honr-card-…` | supervisor | Hardcoded prefix |
| Quality gates `cargo test/clippy --offline` | `briefing()` | Honr-repo-specific |
| Verdict paths `/sandbox/.honr/*.json` | supervisor constants | Product protocol (keep) |
| Widespread `shanemcd/honr` URLs | supervisor/store/api/beads tests | Fixtures |

### Existing seams worth extending

1. `RepoConfig` / `VertexConfig` already threaded through clone, briefing, env.
2. Sandbox profile catalog + Project override + yaml seed (`docs/operating.md`, prior Settings plan).
3. Per-Project `engine`.
4. Webhooks match `pr_url`, not a hardcoded upstream string.
5. Settings shell already has nav + stub General — fill it; do not add a second config app.

---

## Classification

### (A) Must be per-install Settings / config

- Forge provider (**GitHub now**; GitLab = future enum value only).
- Workspace repo binding: `upstream`, `fork`, `base`.
- Bot / token wiring (OpenShell GitHub provider name + docs for credential key `GITHUB_TOKEN`/`GH_TOKEN`).
- Fork strategy note (cross-fork PR; bot must not write upstream) — configurable *repos*, invariant *model*.
- Beads GitHub sync target (replace `shanemcd/honr` fallback; align with upstream or explicit override).
- Default agent engine; Vertex project / location / model (or “Cursor-only” installs).
- OpenShell provider name list; agents enabled; concurrency / budgets / timeout.
- Default sandbox profile (already A).
- Optional: `openshell` binary path if not on `PATH`.

### (B) Must be per-Project overrides

- `sandbox_profile_id` (exists).
- `engine` (exists).
- `project_prompt` / Plan (exists) — standing policy including quality gates for *this* codebase.
- **Later (not required for “second repo = whole board”):** per-Project repo binding if one board drives multiple upstreams.

### (C) May remain code defaults with clear override

- Base branch default `main`.
- Seed image / policy path / cpu / memory.
- Vertex location/model defaults when Vertex is used.
- Branch/sandbox name prefixes (`honr/card-…`) — override via workspace or Project.
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
- GitLab **not** implemented in this Project — only a named seam in Settings IA.

---

## Proposed Settings IA (fresh system)

Extend the existing Settings chrome (`web/src/components/Settings.tsx`):

| Section | Contents | vs Project picker |
|---|---|---|
| **Workspace** (replace General stub) | Forge: GitHub; upstream / fork / base; beads sync repo; webhook setup hint (repo-agnostic). | Global for v1 (one workspace per process). |
| **Sandboxes** (exists) | Profiles: image, inline policy, cpu, memory; set default. | Project overrides profile. |
| **Agent runtime** (new) | Default engine; Vertex project/location/model; OpenShell provider names; enabled / concurrency / budgets (or “advanced” subsection). | Project may override engine only (keep). |
| **OpenShell** (new, thin) | Read-only gateway health (`openshell status` summary); link to ops doc; optional binary path. **No** Colima path editor — host env stays host. | Global. |

**Not** a parallel “config app.” YAML remains cold bootstrap: empty/placeholder example + migration into board state on first boot (same pattern as sandbox profile seed).

---

## Migration path (today’s self-hosting keeps working)

1. **Read order:** board durable workspace/runtime config if present → else `honr.yaml` `execution.agents.*` → else refuse agents with a clear error (no `shanemcd/honr` fallback).
2. **Seed once:** on load, if workspace binding empty and yaml has upstream/fork, copy into board state (mirror `seed_sandbox_profiles_from`).
3. **Env bridge:** honor `GITHUB_REPOSITORY` / `HONR_DATABASE_URL` / `HONR_PORT` as today; document that beads sync should match Workspace upstream (or explicit override field).
4. **Host OpenShell unchanged:** `~/.config/openshell/`, `DOCKER_HOST`, Colima/podman remain operator setup; rewrite `docs/operating.md` / `docs/sandbox-stack.md` to be role-based (“compute driver”, “gateway”, “providers”) with Shane’s stack as *one worked example*, not the schema.
5. **Shipped yaml:** `enabled: false` in examples; real deploys keep local overrides uncommitted (or gitignored overlay) — stop teaching `enabled: true` as the repo default.
6. **Tests:** keep using example `owner/repo` fixtures; add tests that missing binding errors instead of falling back to Shane.

---

## Phased roadmap (second GitHub repo before any GitLab)

See `plan.json` Tasks. Intent in short:

1. **Workspace binding + kill fallbacks** — config model, migration, no silent `shanemcd/honr`.
2. **Settings → Workspace** — operators bind a repo without hand-editing yaml for the common path.
3. **Settings → Agent runtime** — providers / Vertex / engine / budgets in Settings; fix agy location hardcode.
4. **OpenShell ops surface + docs** — health in Settings; de-Shane the ops docs.
5. **Repo-agnostic briefing / gates** — prefixes + quality gates from Project prompt or workspace, not hardcoded `cargo` for every repo.
6. **Second-repo proof** — configure a non-`shanemcd/honr` upstream/fork and complete one dispatched card to Review with a real PR.

GitLab: mention only as `forge: github | (future) gitlab` on the Workspace model — no Tasks in this Plan implement it.

---

## Approve checklist

A human can Approve this Initial plan when:

- [ ] Inventory + A/B/C/D split look right.
- [ ] Settings IA (extend existing) is preferred over a new config surface.
- [ ] Migration keeps current Shane self-hosting working.
- [ ] Task DoDs are mechanically checkable and ordered to unblock a second GitHub repo before forge expansion.
