# Plan: GitHub App sandbox token smoke

**Status:** plan only — Approve materializes Tasks. No product code in this PR.

**Goal:** prove a sandboxed agent can clone/push/open a PR using the OpenShell
github provider’s `GITHUB_TOKEN` when that credential is an **App installation
token** (not a pasted user PAT). Standing Project rule: **SMOKE ONLY** — do
not invent mint-wiring or other product scope; escalate instead.

**Board epic:** [#303](https://github.com/shanemcd/honr/issues/303).

**Out of scope:** implementing Settings → mint installation token → inject into
the OpenShell github provider inside honr; changing auth/OAuth; PAT rotation
UX; multi-install selection. If mint-from-Settings is required before smoke can
mean “App-derived,” escalate rather than expanding this Project.

---

## Verdict

Assume the operator (or an already-landed path outside this Project) has put an
**installation** `GITHUB_TOKEN` on the OpenShell `github` provider and attached
that provider to sandboxes. This Project only **checks the preflight** and
**proves push** with the token the sandbox already receives.

Settings → GitHub App already seals App ID + private key (and related fields)
for installation-token material. Wiring automatic mint/refresh into the
provider is **not** a Task here.

This plan/docs PR itself is opened from the fork with the sandbox’s attached
`GITHUB_TOKEN` — first evidence that git push + `gh` work under that credential.

---

## 1. Current surface (cite these)

| Surface | Path | Role for this smoke |
|---|---|---|
| Sealed App credentials | Settings → GitHub App (`/api/github-app`, `github_app_sealed`) | Operator stores App ID + private key (and optional OAuth fields) |
| OpenShell github provider | Settings → OpenShell → Providers (`GITHUB_TOKEN` credential key) | Gateway injects token into the sandbox |
| Git non-interactive push | `src/supervisor.rs` credential helper + `GH_TOKEN=$GITHUB_TOKEN` | Agent clone/push/`gh pr create` without prompts |
| Remotes briefing | Task-scoped `origin` / `upstream` | Empty `/sandbox/repo` → agent clones; PR targets `upstream` base |

---

## 2. Task DAG (see card `plan.json`)

| Key | Title | Blocked by |
|---|---|---|
| `t1` | Preflight: App + github provider carry installation `GITHUB_TOKEN` | — |
| `t2` | Smoke: sandbox opens a fork PR with attached `GITHUB_TOKEN` | `t1` |

`t1` before `t2` so a missing App config or PAT-only provider fails as
**Needs You / escalate**, not as a mysterious hang mid-push.

---

## 3. Non-goals / anti-patterns

- Building installation-token mint or provider refresh inside this Project.
- Pasting a user PAT into the card briefing or into provider credentials “just
  to make smoke green.”
- Expanding into OAuth login, webhook delivery, or Forge/beads work.
- Treating `origin/main` alone as the merge base when head and base repos differ.

---

## 4. Acceptance for the Project (after Tasks)

- Preflight recorded: GitHub App status complete enough for installation tokens;
  OpenShell `github` provider has `GITHUB_TOKEN`, attach-to-sandboxes on, synced
  when the gateway is up — or an escalate with ≥2 options if not.
- Smoke Task `report.json` has `url` + GitHub-shaped `base`/`head`; PR is from
  the fork against `shanemcd/honr` `main`.
- No product mint-wiring diff required for Project Done.
