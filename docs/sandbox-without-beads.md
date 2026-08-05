# Sandbox agent surface

**Status:** landed (`t1`–`t3`). Sandbox agents receive card context through
supervisor briefings and `/sandbox/.honr` contract files. They publish via
`report.json` / `plan.json` / `escalate.json` / `split.json`. Host/operator
beads stay on the host.

**Board epic:** [#290](https://github.com/shanemcd/honr/issues/290)
(`honr-tr6`).

**Out of scope (still):** host/operator beads (`src/beads.rs`, dual-write
mirror, Dolt sync, Settings → Forge, `AGENTS.md` beads integration for humans
working in this repo).

---

## Contract

```
Host (operator)                    Sandbox (agent)
─────────────────                  ────────────────
board DB + optional beads    →     briefing + /.honr schemas/verdicts
openshell create/exec        →     no bd binary, no /.beads, no BEADS_DIR
                                   report/plan/escalate/split only
```

| Surface | Must stay absent |
|---|---|
| Image bake | no `bd` install in `sandbox/Containerfile` |
| Policy allowlist | no `/usr/local/bin/bd` in `sandbox/policy.yaml` |
| Snapshot upload | no host→sandbox beads DB tar/`sync_beads_into_sandbox` |
| Runtime env | no `BEADS_DIR` / `BEADS_SANDBOX_DIR` in agent env or start script |
| Briefing | no `bd show` / `bd prime` / “Beads id: … (use `bd show`)” / read-snapshot beads workflow |

Regression coverage: `briefing_injects_intent_without_bd_workflow` and
`sandbox_assets_have_no_beads_surface` in `src/supervisor.rs` tests.

Operator docs: [Architecture](architecture.md) (host beads mirror),
[Sandbox](sandbox.md) and [`sandbox/README.md`](../sandbox/README.md) (no `bd`
in the agent image).

---

## Why

Coupling the agent environment to tracker internals invited durable-state
mistakes (`bd remember`, `bd dep add`) even when docs said not to. Prefer
deleting surface area over a disabled stub.
