# The sandbox stack: how an agent runs, and everything that bit us

**Status: working.** A Claude Code agent runs inside an OpenShell sandbox,
authenticates to Vertex with no API key and no credential in the sandbox, builds
and tests honr offline, and opens a real GitHub PR.

Everything below is verified on a real machine, not inferred. Where something is
a workaround for an upstream bug, that's called out. If you are about to change
anything under `sandbox/` or the supervisor's exec scripts, read this first —
most of it was found by watching something hang.
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

One worked example (Shane’s laptop at the time of the Phase 0 spike) — **not**
the required schema for every install. Substitute your driver, Vertex project,
bot identity, and provider names.

| Thing | Example value | Notes |
|---|---|---|
| OpenShell | 0.0.92 | Example: release Homebrew formula into a local tap `shanemcd/openshell` |
| Gateway | `https://127.0.0.1:17670` | **not** 8080 — no conflict with honr (your port may differ) |
| Compute driver | podman | Example: `brew install docker` supplies only the CLI; podman serves `/var/run/docker.sock`. Colima + `DOCKER_HOST` is another valid driver — see `docs/operating.md`. |
| Vertex project | `shanemcd-rh` | Example project id — use yours |
| Vertex location | **`global`** | Example: `us-east5` was quota-exhausted; `us-central1` did not serve the model |
| Vertex model | `claude-opus-5` | |
| Sandbox image | `ghcr.io/nvidia/openshell-community/sandboxes/base:latest` | Ubuntu 24.04.3, multi-arch (amd64 + arm64). Ships `claude` 2.1.156, `gh` 2.93, git, node 22 + npm, python 3.14. **No Rust toolchain** — see below. |
| Sandbox `HOME` | `/sandbox` | writable; user is `sandbox`, **no sudo** |
| Image `ENTRYPOINT` | `/bin/bash` | so `docker run <image> sh -c '…'` fails with `cannot execute binary file` — bash reads `sh` as a *script*. Use `-c '…'` directly, or `--entrypoint`. |
| GitHub identity | `clankrshq` (bot) | Example bot; `shanemcd` still present but inactive on that machine |

### Providers configured on the gateway (example)

```bash
openshell provider create --name vertex    --type google-vertex-ai --from-gcloud-adc
openshell provider update  vertex --config VERTEX_AI_PROJECT_ID=<your-gcp-project> \
                                  --config VERTEX_AI_LOCATION=global
openshell provider create --name gh-clankr --type github --credential GITHUB_TOKEN
```

Replace project id and provider names with values that match **your** gateway
and `honr.yaml` / Agent runtime Settings. The GitHub credential key **must** be
`GITHUB_TOKEN` or `GH_TOKEN` — the profile matches on the name.

> ⚠️ On the example install, `gh-clankr` held the bot's OAuth token across many
> repos. Prefer a fine-grained PAT scoped to the repos you actually dispatch
> against before agents run unattended.

---

## Why `openshell.rs` shells out to the CLI instead of using the gRPC API

OpenShell is Rust and ships `crates/openshell-sdk`, "the shared async Rust client for OpenShell
gateways", plus 9 `.proto` files. Talking to the gateway directly looks like the obvious choice. It
isn't, for one decisive reason and three supporting ones.

**The SDK does not support mTLS.** Its README says so outright: plaintext, server-authenticated
TLS, OIDC bearer, Cloudflare Access, and insecure-TLS — "mTLS (client certificates) is not
supported." Our gateway is mTLS-only (`openshell status` → `Authenticated (mTLS transport)`, certs
in `~/.config/openshell/gateways/openshell/mtls/`). Using the SDK would mean reconfiguring the
gateway away from mTLS: a weaker posture, and a departure from the setup phase 0 was proven on.

Supporting reasons:

- **Nothing is published to crates.io.** A search returns no OpenShell crates at all, so the SDK
  can only be a git dependency pinned to a rev of a large alpha workspace — which also fights the
  offline sandbox image, since `cargo fetch --locked` would need to clone OpenShell at build time.
- **The curated surface can't stream exec**, and streaming is the whole point: liveness and real
  cost come from parsing `claude`'s `stream-json` line by line. That forces the `raw` tonic clients,
  the protos, `protoc` in our build, and generated code we'd own.
- **Alpha churn argues for looser coupling.** We already route around #2478. CLI flags move more
  slowly than internal proto messages.

The CLI's usual weakness barely applies: `sandbox list -o json` emits clean JSON, `exec` propagates
the remote exit code, and process-spawn overhead is noise against sandbox operations measured in
seconds.

**Revisit when both change:** the gateway moves off mTLS *and* the crates are published. Same shape
as the `inference.local` note above.

---

## The image has no Rust toolchain — which matters for self-hosting

Probed directly (`docker run --rm <image> -c '...'`, arm64):

```
cargo MISSING   rustc MISSING   rustup MISSING
node  v22.22.1  npm ✓   gh ✓   git ✓   claude 2.1.156
Ubuntu 24.04.3 LTS · user=sandbox · no sudo · $HOME=/sandbox (writable) · nproc=7
```

Fine for the probe repo. **Blocking for honr**, which is Rust: an agent cannot run `cargo build` or
`cargo test`, so it cannot satisfy any definition of done on this codebase.

Two constraints shape the fix:

- **No sudo**, so `apt-get install` is not available to the agent. `rustup` is the only in-sandbox
  route, and it works only because it installs to `$HOME/.cargo` and `/sandbox` is already
  `read_write` in `policy.yaml`.
- **Egress is the real cost.** `policy.yaml` allows Vertex and GitHub only. rustup needs
  `static.rust-lang.org`; the build then needs `index.crates.io` and `static.crates.io`; `web/`
  needs `registry.npmjs.org`. All four are default-denied today, and **denial presents as a hang.**
  Binary paths are matched literally, so `/sandbox/.cargo/bin/cargo` must be listed explicitly.

### Prefer a prebuilt image over per-card rustup

Installing rustup and cold-fetching honr's dependency graph (1383-line `Cargo.lock`) on *every card*
costs minutes of wall-clock and real Vertex spend per run, and permanently widens the policy by
three destinations. `sandbox/Containerfile` bakes the toolchain and pre-warms the cargo registry
instead, which drops per-card cost to near zero and means **crates.io never has to be reachable from
an agent sandbox at all**.

Build and point OpenShell at it:

```bash
# from the repo root — Cargo.lock and web/package-lock.json must be in context
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
openshell sandbox create --name <NAME> --from honr-sandbox:latest ...
```

The image flag is `--from`, **not** `--image`. It doubles as a build trigger: given a Dockerfile or
a directory it builds into the local Docker daemon, and a bare name like `ollama` is resolved
against `ghcr.io/nvidia/openshell-community/sandboxes/`. A local tag such as `honr-sandbox:latest`
is taken as an image reference.

It also needs companion entries in `policy.yaml` (`/opt/cargo` + `/opt/npm-cache` read-write,
`/opt/rust` read-only) — the Containerfile documents them inline.

Rebuild it whenever `Cargo.lock` changes materially, or the warm cache just goes stale and cargo
fetches the delta — which still needs crates.io egress, so treat a stale image as a real failure
mode rather than a slow path.

### The image's `ENV` does not reach `sandbox exec`

Baking `ENV RUSTUP_HOME=... CARGO_HOME=... PATH=...` into the Containerfile is **not enough**.
Inside `openshell sandbox exec`, `PATH` arrives as the base image's default and `CARGO_HOME` arrives
empty, so `cargo` is not on the path — and invoking `/opt/cargo/bin/cargo` directly then fails with
*"rustup could not choose a version of cargo to run"*, because `RUSTUP_HOME` is unset too. The
toolchain vars have to be passed explicitly as `--env` at sandbox creation. `supervisor.rs` does
this in `agent_env`.

Related: `/opt` itself is not listable under the policy (`ls /opt` → permission denied) even though
`/opt/cargo` and `/opt/rust` are reachable. Listing the specific subpaths is what matters; the
parent does not need to be granted.

**Verified end to end** in a sandbox created with `sandbox/policy.yaml` as the unprivileged
`sandbox` user: `git clone` of honr, then `cargo build --offline --locked` in **29s**, then
`cargo test --offline --locked` → **24 passed**. That is the whole gate chain running under policy,
with no crates.io access.

**Also verified** on the host, with the source mounted read-only:

| Gate | Result |
|---|---|
| `cargo build --offline --locked` | ✅ 14.2s from cold `target/` |
| `cargo test --offline --locked` | ✅ 8 passed |
| `npm ci --offline` | ✅ 68 packages in 0.7s |
| `npm run build` | ✅ 449ms |

`cargo 1.97.1`, `clippy 0.1.97`, 139 crates pre-cached. The `--offline` flags are the point: both
gates complete with **no network at all**, so the agent's policy never needs crates.io or npm.
Pass `--offline` explicitly in the gate commands so a cache miss fails loudly instead of hanging on
a denied fetch.

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
- **`sandbox upload` takes a destination *directory*, not a destination file.** `upload NAME
  local/metadata-shim.py /tmp/metadata-shim.py` creates a **directory** `/tmp/metadata-shim.py`
  with the file inside it, and python then fails with
  `can't find '__main__' module in '/tmp/metadata-shim.py'`. Upload to `/tmp` instead. The
  destination must also **already exist** — uploading to a fresh path fails with
  `ssh tar extract exited with status exit status: 1`.
- **A lingering background process does *not* hold `sandbox exec` open.** `nohup … &` then exiting
  returns in ~25ms, so the shim can be started in its own exec and outlive it. The supervisor now
  leans on this for the agent itself: `setsid nohup` it, write its pid and exit code to `/tmp`, and
  follow the log with `tail -n +N -f --pid=…`. That makes watching a run something a *different*
  honr process can pick up after a restart, which is the whole of re-adoption.
- **`timeout` gives the command its own process group** unless you pass `--foreground`. So
  `kill -TERM -$pgid` against the wrapper kills `timeout` and leaves `claude` orphaned and still
  billing. Verified by watching the pgids, not inferred.
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
