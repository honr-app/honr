# Phase 0 — real agents in OpenShell sandboxes: what we proved

**Status: both gates PASSED.** A Claude Code agent runs inside an OpenShell sandbox,
authenticates to Vertex with no API key and no credential in the sandbox, and opens a real
GitHub PR. Proof: [clankrshq/honr-sandbox-probe#1](https://github.com/clankrshq/honr-sandbox-probe/pull/1).

Everything below is verified on this machine, not inferred. Where something is a workaround for an
upstream bug, that's called out.

---

## The one hard problem, and its answer

Claude Code on Vertex uses google-auth's ADC chain. In a sandbox that chain finds nothing and falls
through to the **GCE metadata server** (`169.254.169.254` / `metadata.google.internal`). OpenShell
blocks metadata endpoints **permanently** as SSRF hardening — the logs say
`Skipped proposal for always-blocked destination`. No policy can open it. Symptom: `claude -p`
appears to hang, then `exec relay closed before the command reported an exit status` after ~78s.

The fix is to give google-auth a metadata server it *is* allowed to reach:

```
OpenShell provider (gateway-side, holds gcloud ADC, auto-refreshes)
    │  injects an OPAQUE PLACEHOLDER as GOOGLE_VERTEX_AI_TOKEN
    ▼
sandbox/metadata-shim.py on 127.0.0.1:8127     ← GCE_METADATA_HOST points google-auth here
    │  serves that placeholder as a normal GCE token response
    ▼
claude (Vertex mode) → aiplatform.googleapis.com
    │  Authorization: Bearer <placeholder>
    ▼
OpenShell egress proxy SUBSTITUTES THE REAL TOKEN
    ▼
Vertex
```

Two properties worth preserving:

- **No credential is ever in the sandbox.** `GOOGLE_VERTEX_AI_TOKEN` inside the sandbox literally
  reads `openshell:resolve:env:v14809318921281687850_GOOGLE_VERTEX_AI_TOKEN`. The real secret is
  injected at egress by the proxy. Reading the env var gains an agent nothing.
- **We talk straight to `aiplatform.googleapis.com`,** which the `google-vertex-ai` provider profile
  already allow-lists. This is why we do *not* use OpenShell's `inference.local` router — see the
  upstream bug below.

### Why not the inference router (the "designed" path)

`openshell inference set --provider vertex --model claude-opus-5` configures a route, and the
sandbox loads it (`Inference routing enabled with local execution`). But requests fail:

```
{"detail":"POST inference.local:80 blocked: declared endpoint check failed","error":"ssrf_denied"}
```

`inference.local` has **no DNS entry** inside the sandbox, and `/etc/hosts` is a read-only bind mount
so it can't be patched. This is upstream
[NVIDIA/OpenShell#2478](https://github.com/NVIDIA/OpenShell/issues/2478) — "SSRF engine blocks
host.openshell.internal in 0.0.91 (regression from 0.0.86)", reported on the same stack we're on
(podman, macOS arm64, `--provider vertex`), labelled `state:pr-opened` with fix PR
[#2479](https://github.com/NVIDIA/OpenShell/pull/2479).

**When that lands, re-evaluate.** The router is the cleaner design; the shim is our workaround.
Until then, going direct sidesteps the bug entirely.

---

## Verified environment facts

| Thing | Value | Notes |
|---|---|---|
| OpenShell | 0.0.92 | installed via the release's own Homebrew formula into a local tap `shanemcd/openshell` |
| Gateway | `https://127.0.0.1:17670` | **not** 8080 — no conflict with honr |
| Compute driver | podman | `brew install docker` supplies only the CLI; podman serves `/var/run/docker.sock` |
| Vertex project | `shanemcd-rh` | |
| Vertex location | **`global`** | `us-east5` is quota-exhausted; `us-central1` does not serve the model |
| Vertex model | `claude-opus-5` | |
| Sandbox image | `ghcr.io/nvidia/openshell-community/sandboxes/base:latest` | ships `claude` 2.1.156, `gh` 2.93, git, node 22, python 3.14 |
| Sandbox `HOME` | `/sandbox` | writable |
| GitHub identity | `clankrshq` (bot) | active `gh` account; `shanemcd` still present but inactive |

### Providers configured on the gateway

```bash
openshell provider create --name vertex    --type google-vertex-ai --from-gcloud-adc
openshell provider update  vertex --config VERTEX_AI_PROJECT_ID=shanemcd-rh \
                                  --config VERTEX_AI_LOCATION=global
openshell provider create --name gh-clankr --type github --credential GITHUB_TOKEN
```

The GitHub credential key **must** be `GITHUB_TOKEN` or `GH_TOKEN` — the profile matches on the name.

> ⚠️ `gh-clankr` currently holds the bot's OAuth token, which reaches **all 9** of clankrshq's repos
> (including forks of OpenShell and NemoClaw). Swapping it for a fine-grained PAT scoped to one repo
> is a one-line provider recreate, and is the right move before agents run unattended.

---

## The working incantation

Policy is `sandbox/policy.yaml`, shim is `sandbox/metadata-shim.py`.

```bash
openshell sandbox create --name <NAME> --no-tty \
  --provider vertex --provider gh-clankr \
  --policy sandbox/policy.yaml \
  --env CLAUDE_CODE_USE_VERTEX=1 \
  --env ANTHROPIC_VERTEX_PROJECT_ID=shanemcd-rh \
  --env CLOUD_ML_REGION=global \
  --env ANTHROPIC_MODEL=claude-opus-5 \
  --env GCE_METADATA_HOST=127.0.0.1:8127 \
  --env DISABLE_TELEMETRY=1 --env DISABLE_ERROR_REPORTING=1 --env DISABLE_AUTOUPDATER=1 \
  -- echo up

openshell sandbox upload <NAME> sandbox/metadata-shim.py /tmp/metadata-shim.py
```

Then every agent run needs this preamble (the shim must be alive for the agent's whole lifetime):

```sh
python3 /tmp/metadata-shim.py >/tmp/shim.log 2>&1 &
# poll http://127.0.0.1:8127/ until it answers, then run claude
```

Git must be driven non-interactively — see Failure Modes:

```sh
export GIT_TERMINAL_PROMPT=0
GC="credential.helper=!f(){ echo username=x-access-token; echo password=$GITHUB_TOKEN; };f"
git -c "$GC" clone -q https://github.com/clankrshq/honr-sandbox-probe.git probe
git -c "$GC" push -q -u origin honr/card-N
GH_TOKEN=$GITHUB_TOKEN gh pr create --repo ... --head honr/card-N --base main --title ... --body ...
```

---

## Failure modes: everything fails as a HANG, not an error

This is the single most important operational lesson, and it should shape `supervisor.rs`.

| What actually went wrong | How it presented |
|---|---|
| Blocked metadata server (credentials) | `claude -p` silent ~78s, then `exec relay closed` |
| `git clone` with no credential helper | blocked forever on an interactive username prompt |
| Denied egress generally | command hangs until something upstream times out |

`NET:OPEN ALLOWED` in the log followed by silence means the connection was fine and the *process* is
stuck — usually waiting on input or retrying.

**Therefore the supervisor must:** set `GIT_TERMINAL_PROMPT=0`, pass a hard timeout on every
`sandbox exec`, and treat "no output for N seconds" as a failure rather than waiting. A trivial
`openshell sandbox exec -- echo hi` returns in **0s**, so the relay itself is fast — slowness is
always the payload, never the transport.

### Debugging

`openshell logs <name> -n 60` is the tool that actually answers questions. Grep for `DENIED`,
`ALLOWED`, `ssrf`, `HTTP:`. The OCSF lines name the binary making each request, which is how the
metadata-server problem was found.

---

## Other gotchas

- **`policy set --wait` takes ~50s** and reloads the sandbox supervisor. Filesystem and process
  policy sections are **immutable after creation** anyway (`read_only path '/etc' cannot be removed
  on a live sandbox`). So: always pass `--policy` at `sandbox create`, never mutate mid-flight.
- **The placeholder token is injected at creation** and is good for ~1h. Fine for per-card
  sandboxes; it bounds a single card's runtime. Treat as a budget-style limit.
- **The podman machine dies.** It stopped once mid-session with no obvious cause; upstream
  [#2179](https://github.com/NVIDIA/OpenShell/issues/2179) covers sandboxes stuck in Error after a
  podman restart with no recovery path. The supervisor should health-check
  (`docker info` / `openshell status`) before claiming a card.
- **A stale `~/.config/openshell/gateway.toml`** with Kubernetes paths (`/etc/openshell-tls/...`,
  `/home/shanemcd/...`) will silently break the brew-installed gateway. Ours is backed up at
  `gateway.toml.k8s-stale.bak`. If the gateway won't start, check this first.
- **Binary paths in policy are matched literally** and symlink resolution warns constantly. Git's
  real binary is `/usr/lib/git-core/git-remote-http` (note: **not** `-https`).
- **OpenShell is alpha.** Sandbox startup is seconds, not milliseconds. Don't plan for a 2s tick or
  seven concurrent sandboxes; start at `max_concurrent: 2`.

---

## Where honr itself stands

Phase 1 is complete and verified (board, state machine, MCP, simulated fleet, React UI). Phase 2 has
only had its Phase 0 spike done. Remaining, in order:

1. `git init` honr + push to GitHub — **not done**, and it blocks the self-hosting goal
2. `src/openshell.rs` — typed async wrapper over the CLI (`tokio::process`, `--output json`)
3. `ExecutionConfig` in `src/schema.rs` + `honr.yaml` (`mode: simulated | openshell`, default stays
   `simulated` so the board runs with no infrastructure)
4. `src/supervisor.rs` — per-card lifecycle; see the plan file for the eight steps
5. `OpenShellExecutor` wired into the `fleet.rs` harness
6. Sandbox name → `WorkItem.environment`; add `pr_url`; surface both in the UI
7. End-to-end verification against the probe repo

The full design (supervisor-mediated trust boundary, briefing construction, verdict file protocol,
budget enforcement) is in the plan file:
`~/.claude/plans/modular-roaming-seal.md`.

### Design decisions already locked

- **Supervisor-mediated.** The agent gets **no** network path to honr. The supervisor calls
  `claim`/`heartbeat`/`report`/`escalate` on its behalf. An agent that could reach honr's MCP could
  approve its own review. All OpenShell port-forwarding is host→sandbox anyway, which independently
  confirms this.
- **Liveness is observed, not self-reported** — parsed from the agent's `stream-json` output and
  real cost, so a hung agent cannot claim to be fine.
- **GitHub PRs are the review artifact.** Approve in honr surfaces the PR; merging stays a human
  action for now.
- **Verdict file protocol:** the agent writes `/work/.honr/{report,escalate,split}.json`; the
  supervisor reads them via `sandbox download` and routes to the board. The ≥2-options rule for
  escalation is enforced in `Board`, so neither transport can bypass it.
