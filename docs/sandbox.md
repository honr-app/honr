# Sandbox

How a sandboxed agent run works, and the operator-relevant gotchas. Assets live
under [`sandbox/`](https://github.com/honr-app/honr/tree/main/sandbox); this page is the prose companion.

## How credentials reach the agent

Claude Code and OpenCode talk to OpenShell's local inference router. The
gateway holds Vertex (or other provider) credentials and injects them on
egress — nothing in the sandbox does GCP ADC or metadata discovery.

```
openshell inference set --provider vertex --model claude-sonnet-4-6@default
        │
        ▼
sandbox agent
  ANTHROPIC_BASE_URL=https://inference.local[/v1]
  ANTHROPIC_API_KEY=unused
        │
        ▼
https://inference.local  →  gateway strips placeholder key, injects Vertex token
        │
        ▼
aiplatform.googleapis.com (rawPredict / Messages API)
```

Operator setup (once per gateway):

```bash
openshell provider create --name vertex --type google-vertex-ai --from-gcloud-adc \
  --config VERTEX_AI_PROJECT_ID=<project> --config VERTEX_AI_REGION=global
openshell inference set --provider vertex --model claude-sonnet-4-6@default
```

Inside the sandbox honr exports (engine-specific):

| Engine | `ANTHROPIC_BASE_URL` | Notes |
|---|---|---|
| `claude` | `https://inference.local` | Claude appends `/v1/messages`; `--bare` + `--mcp-config` for seat MCP |
| `opencode` | `https://inference.local/v1` | OpenCode requires the `/v1` suffix |

Do **not** set `CLAUDE_CODE_USE_VERTEX=1` in the sandbox. That forces direct
Vertex + ADC/metadata discovery, which OpenShell blocks (real GCE metadata is
SSRF-hardened). Use `inference.local` instead.

## Gateway client (gRPC + mTLS)

`src/openshell.rs` talks to the gateway in-process over gRPC with client
certificates. Endpoint + sealed PEMs live in Settings (board DB); the only host
secret file is `~/.config/honr/master.key`. Upload/download use exec + tar over
that same channel — no `openshell` CLI spawn. Upstream `openshell-sdk` still
omits mTLS; we build the channel ourselves and use `openshell-core` /
`openshell-policy` for protos and YAML policy.

## Agent surface

Card intent and protocol paths come from the supervisor briefing (and files
under `/sandbox/.honr`). Agents finish via `plan.json` / `report.json` /
`escalate.json` / `split.json`. The board is the only tracker — sandboxes do
not carry a separate issue-store CLI or database.

## Image and offline gates

The community base image has no Rust toolchain and the `sandbox` user has no
sudo. For honr itself, [`sandbox/Containerfile`](https://github.com/honr-app/honr/blob/main/sandbox/Containerfile) bakes
`cargo` / `clippy`, Cursor Agent (`agent`), OpenCode (`opencode`), and pre-compiles
the Rust dependency tree (plus npm `ci`) so crates.io never needs to be reachable
from an agent sandbox. The warm step runs `cargo build --locked` and
`cargo test --no-run --locked` into `CARGO_TARGET_DIR=/opt/cargo-target`; agents
inherit that path and reuse the debug artifacts.

```bash
# from the repo root; Cargo.lock, src/, migrations/, and web/package-lock.json in context
make sandbox
# or: podman build -f sandbox/Containerfile -t honr-sandbox:latest .
# Docker: CONTAINER_ENGINE=docker make sandbox
```

The image flag is `--from`, not `--image`. Rebuild when `Cargo.lock`, `src/`, or
`migrations/` change materially, or when you need a newer Cursor/OpenCode CLI. Matching `/opt`
entries (`/opt/cargo`, `/opt/cargo-target`, `/opt/npm-cache`, `/opt/opencode`, …)
belong in the **board** sandbox spec policy (Settings → OpenShell → Sandbox
specs). Create starts from a minimal policy (`src/seed_policies.rs`); paste
your `/opt` allow-list (including `/opt/cargo-target` on `read_write`) when you
need the honr image layout.

Pass `--offline` in gate commands so a cache miss fails loudly instead of
hanging on a denied fetch.

## Operator-relevant gotchas

**Everything fails as a hang, not an error.** Denied egress, missing
credential, wedged relay: all silence. Every exec needs a deadline; treat
silence as failure.

**The image's `ENV` does not reach `openshell sandbox exec`.** Pass toolchain
vars explicitly in `agent_env` (supervisor does this), or install wrappers on
the default PATH. Baking `ENV PATH=…` into the Containerfile is not enough.

**Upload destination is a directory** (same semantics as the old CLI): uploading
to `/tmp/foo.py` creates a *directory* of that name with the file
inside it. Put the file in `/tmp` so it lands at `/tmp/foo.py`.

**The compute driver can stop on its own.** Classify that as infrastructure,
not as the card failing: see `is_infrastructure` in the supervisor.

**Workdir: cold start empties; reclaim preserves.** Brand-new sandbox create
clears `/sandbox/repo` so the agent clones into an empty tree from the Remotes
briefing (`origin` / `upstream`). Reclaim of a kept sandbox — park resume and
Needs You answer share the same reuse path — does **not** wipe `/sandbox/repo`.
When a checkout exists, the supervisor refreshes in place: fetch the PR-target
tip, prefer the local card branch (not a hard reset to `origin/`), and rebase
only when the tree is clean. Dirty mid-run edits stay put — MainAdvanced steer
asks the agent to rebase. Otherwise it ensures the directory without clearing
prior contents or caches. The supervisor never clones; agent-owns-clone stays.
`/sandbox/.honr` is always present at start with at least `report.schema.json`.
If a clean-tree reuse rebase conflicts, the supervisor backs out and tells the
agent to resolve it.

**Policy is immutable on a live sandbox** for filesystem and process sections.
Set it at create time; `policy set --wait` is expensive.

**Binary paths are matched literally** in the policy. Lists are deliberately
generous (git's real remote helper is `/usr/lib/git-core/git-remote-http`).

## Default vs Cockpit

There is no separate seeded Cockpit profile. Create a sandbox spec under
**Settings → OpenShell → Sandbox specs** (minimal policy prefill). The first
profile becomes the global default; Cockpit uses that default until you create
another profile and click **Use for Cockpit**.

Attach on create starts empty — add providers under **Providers**, then check
them on the spec. Add honr MCP / package-registry / toolchain egress in the
policy YAML when you need it.

## Antigravity / agy

Bare OpenShell `generic` providers do **not** resolve
`openshell:resolve:…` placeholders — the egress proxy only substitutes on
endpoints declared by a **provider type**. Honr ships board provider types
[`sandbox/openshell/antigravity.yaml`](https://github.com/honr-app/honr/blob/main/sandbox/openshell/antigravity.yaml)
(`auth_style: bearer`, Cloud Code / Google API hosts) and
[`sandbox/openshell/cursor-agent.yaml`](https://github.com/honr-app/honr/blob/main/sandbox/openshell/cursor-agent.yaml)
(`CURSOR_API_KEY`, Bearer on Cursor API hosts). Both seed into
**Settings → OpenShell → Provider types** and import on provider Sync when
missing. Builtin OpenShell `cursor` remains egress-only (no credentials).

Create the provider yourself under **Settings → OpenShell → Providers** with an
`ANTIGRAVITY_ACCESS_TOKEN` credential. honr does not read the host keychain —
it makes no assumptions about credentials sitting on the machine it runs on, and
configuration arrives over the API like everything else.

| Layer | Holds |
|---|---|
| Board provider `antigravity` | Sealed `ANTIGRAVITY_ACCESS_TOKEN`, supplied via the API |
| Gateway | Real credential; injects placeholder env into attached sandboxes |
| Seat token file | Placeholder only + far-future expiry; **no** `refresh_token` |
| Seat `settings.json` | `enableTelemetry: false`, `gcp.project` / `gcp.location` from Board provider config `ANTIGRAVITY_GCP_PROJECT` / `ANTIGRAVITY_GCP_LOCATION` (Settings → Providers; not Vertex/`GOOGLE_CLOUD_PROJECT`) |

Attach names that are not in the Providers catalog are pruned on board load.

## Related

- [Your first agent](first-agent.md) — Welcome/Help OpenShell onboarding, then the first run
- [Troubleshooting](troubleshooting.md) — the gotchas above, as symptoms
- [Configuration](configuration.md#sandbox-specs) — spec resolution and engines
