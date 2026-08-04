# Plan: Remove beads from the sandbox

**Status:** plan only — Approve materializes Tasks. No product code in this PR.

**Goal:** sandboxed agents must not see `bd`, a beads DB, or beads CLI workflow.
Issue / card / epic context reaches them only through supervisor briefings (and
other `/sandbox/.honr` contract files). Agent outputs stay the existing
reporting contracts — not beads mutations.

**Board epic:** [#290](https://github.com/shanemcd/honr/issues/290)
(`honr-tr6`).

**Out of scope:** host/operator beads (`src/beads.rs`, dual-write mirror, Dolt
sync, Settings → Forge, `AGENTS.md` beads integration for humans working in
this repo). Host beads may remain; this Project is the **sandbox agent
surface** only.

---

## Verdict

**Delete the sandbox beads surface in one coherent cut** — image binary,
policy allowlist, host→sandbox DB upload, `BEADS_DIR` env, and briefing
instructions that tell the agent to run `bd`. Prefer deletion over a disabled
stub.

**Replace** what agents today get from `bd show` / `bd prime` by injecting
board fields the supervisor already owns (especially card **intent**) into
the briefing (and, if needed, a small file under `/sandbox/.honr`). Do not
leave a window where `bd` is gone but the briefing is still thinner than
today's snapshot.

---

## 1. Current coupling (cite these paths)

| Surface | Path | What it does today |
|---|---|---|
| Image bake | `sandbox/Containerfile` (~L41–44) | `RUN` installs `bd` to `/usr/local/bin/bd` |
| Policy allowlist | `sandbox/policy.yaml` | `/usr/local/bin/bd` on `vertex_ai` and `github` binary lists |
| Unpack target | `src/supervisor.rs` `BEADS_SANDBOX_DIR` | `/sandbox/.beads` |
| Snapshot upload | `sync_beads_into_sandbox` in `src/supervisor.rs` | Tars host beads dir, `openshell upload`, unpacks into sandbox; called on create and reuse (~L989, ~L1030) |
| Runtime env | `agent_env` + start script in `src/supervisor.rs` | `export BEADS_DIR=/sandbox/.beads` every run |
| Briefing | `briefing` / `resume_briefing` in `src/supervisor.rs` | Emits `Beads id: … (use \`bd show …\`)` and a “read snapshot / `bd prime`” paragraph |
| Schemas upload | `ensure_report_schema_in_sandbox` | Already injects `/sandbox/.honr` contracts (pattern to keep; not beads) |

Host dual-write (`src/beads.rs`, `Board::mirror_beads_*` in `src/store.rs`) is
**not** in scope — operators may keep using beads outside the sandbox.

### What the agent actually needs from beads today

`ClaimGrant` (`src/store.rs`) already carries title, DoD, project prompt, plan
rows, notes, and optional `beads_id` — but **not** card `intent`. Agents are
told to run `bd show <id>` for description / graph context. After removal,
that line must disappear and **intent (at minimum)** must appear in the
briefing so the cut does not regress card context.

Reporting stays: `plan.json`, `report.json`, `escalate.json`, `split.json`
under `/sandbox/.honr` — never `bd create` / `bd remember` / `bd dep add`.

---

## 2. Target shape

```
Host (operator)                    Sandbox (agent)
─────────────────                  ────────────────
board DB + optional beads    →     briefing + /.honr schemas/verdicts
openshell create/exec        →     no bd binary, no /.beads, no BEADS_DIR
                                   report/plan/escalate/split only
```

1. **Briefing first (or same PR as the cut):** inject `intent`; drop all
   sandbox `bd` instructions and beads-id “use `bd show`” lines.
2. **Runtime cut:** delete `sync_beads_into_sandbox` and call sites; drop
   `BEADS_DIR` / `BEADS_SANDBOX_DIR`; remove Containerfile install and policy
   `bd` entries. Rebuild `honr-sandbox` so live profiles pick up the image.
3. **Docs:** architecture / sandbox pages must stop claiming a mirrored
   `BEADS_DIR` inside agent sandboxes.

Do **not** leave `bd` installed “but unused,” or sync code behind a flag.

---

## 3. Task DAG (see card `plan.json`)

| Key | Title | Blocked by |
|---|---|---|
| `t1` | Briefing injects card context without `bd` | — |
| `t2` | Delete sandbox beads runtime (image, policy, upload, env) | `t1` |
| `t3` | Docs + tests: sandbox agent surface has no beads | `t2` |

`t1` before `t2` avoids a half-removed state where the binary/DB are gone but
the agent is still told to `bd show`. `t3` locks the contract in prose and
unit tests after the cut lands.

---

## 4. Non-goals / anti-patterns

- Removing host beads mirror or GitHub issue sync.
- Teaching sandbox agents a “read-only bd” or shipping a fake `bd` stub.
- Putting durable tracker state in the sandbox (no `bd remember` substitute
  inside the image).
- Changing report / plan / escalate / split schemas unless a Task DoD
  explicitly requires it.

---

## 5. Acceptance for the Project (after Tasks)

- Fresh sandbox image: `which bd` fails; no `/sandbox/.beads` from supervisor.
- Briefing contains card intent and protocol paths; contains no `bd show` /
  `bd prime` / “read snapshot” beads workflow.
- `docs/architecture.md` no longer says sandboxes get a mirrored `BEADS_DIR`.
- Host beads workflows for operators remain intact.
