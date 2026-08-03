# Second-repo dispatch proof (card #174)

**Date:** 2026-08-03  
**Plan key:** `second-repo-proof`  
**Board card (this docs Task):** #174  
**Beads:** `honr-l7s.6` / https://github.com/shanemcd/honr/issues/243

## Verdict

A host Board dispatch against a **non-Shane** GitHub repo reached **Review**
with a real `pr_url`, while install Workspace / beads sync stayed on
`shanemcd/honr`. Work remotes for the probe came from the card’s
`pull_request` / Project prompt — not from editing `shanemcd/honr` into
config for that run.

## Probe run (host Board)

| Field | Value |
|---|---|
| Board card | **#180** |
| Upstream | `clankrshq/honr-sandbox-probe` |
| Fork | `clankrshq/honr-sandbox-probe` (same-repo) |
| `pr_url` | https://github.com/clankrshq/honr-sandbox-probe/pull/2 |
| PR title | Trivial README probe PR (card-180) |
| Head branch | `honr/card-180` |
| Install Workspace / beads | Remained `shanemcd/honr` (Issue mirror) |
| Work remotes for probe | Probe upstream/fork above — **not** rebinding install Workspace |

Related GitHub Issues (beads / Project scaffolding on the honr mirror):
[#253](https://github.com/shanemcd/honr/issues/253),
[#254](https://github.com/shanemcd/honr/issues/254),
[#255](https://github.com/shanemcd/honr/issues/255).

PR #2 body cites `honr-zgl.1` / card-180 and adds `PROBE.md` only (no Rust
gates required).

## DoD checklist

1. **Run record** — this document; names non-Shane upstream/fork above.
2. **Review + `pr_url`** — Board card **#180** →
   https://github.com/clankrshq/honr-sandbox-probe/pull/2 on that upstream.
3. **No Shane work-remote edit** — host left install Workspace/beads on
   `shanemcd/honr`; probe work remotes were the probe repo (same-repo).
4. **Residual hardcodes** — follow-ups filed below.

Same-repo on the probe is enough for this Task (multi-repo / `pr_url`
resolution path). A dedicated **cross-fork** probe (distinct upstream vs
fork owners) remains optional follow-up, not a blocker.

## Residual hardcodes / follow-ups

Production `src/` no longer defaults missing workspace upstream to the
literal `shanemcd/honr` (covered by earlier `workspace-binding` /
multi-repo cards). Remaining Shane-shaped values are **bootstrap / examples
/ fixtures**:

| Residual | Where | Follow-up |
|---|---|---|
| Shipped `honr.yaml` seeds Shane repo + Vertex project + `enabled: true` + cargo `quality_gates` | `honr.yaml` | [#256](https://github.com/shanemcd/honr/issues/256) — ship-safe example defaults |
| OpenShell provider name list assumes local gateway registrations (`gh-clankr`, `cursor-honr`, …) | `honr.yaml` / Agent runtime | Fold into #256 (document placeholders; do not invent names) |
| Widespread `shanemcd/honr` strings in unit tests / UI fixtures | `src/*` tests, `web/ui-fixture.mjs` | **Keep** — classification (C); not runtime |
| Docs Colima / home paths as worked example | `docs/sandbox-stack.md`, `docs/operating.md` | **Keep** — already labeled example / role-based |
| Optional true cross-fork second-repo probe | ops | [#257](https://github.com/shanemcd/honr/issues/257) |

## How to re-run

1. Leave Settings → Forge beads sync on the honr Issue mirror.
2. Create a short Project / Task whose clone target / prompt names
   `clankrshq/honr-sandbox-probe` (or another non-Shane repo).
3. Use empty install-wide `quality_gates` (or a Project prompt without
   cargo) for non-Rust repos; pick an appropriate sandbox image/profile.
4. Dispatch; confirm the card reaches Review with `pull_request.url` on
   that upstream.
5. Append a line to this doc (or open a PR comment) with card id + `pr_url`.

See also: [generalization.md](./generalization.md).
