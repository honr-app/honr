# Sandbox

How a sandboxed agent run works, and the operator-relevant gotchas. Assets live
under [`sandbox/`](../sandbox/); this page is the prose companion.

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
sudo. For honr itself, [`sandbox/Containerfile`](../sandbox/Containerfile) bakes
`cargo` / `clippy`, Cursor Agent (`agent`), OpenCode (`opencode`), and pre-warms
cargo and npm caches so crates.io never needs to be reachable from an agent
sandbox.

```bash
# from the repo root; Cargo.lock and web/package-lock.json must be in context
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
# or: make sandbox
```

The image flag is `--from`, not `--image`. Rebuild when `Cargo.lock` changes
materially, or when you need a newer Cursor/OpenCode CLI. Matching `/opt`
entries (including `/opt/opencode`) belong in the worker **board** sandbox
spec policy (Settings → OpenShell → Sandbox specs → `default`; seed text in
`src/seed_policies.rs`). Live specs do not auto-pick up seed edits — paste
policy updates (or recreate the spec) after changing the seed.

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

**`/sandbox/repo` starts empty.** The agent clones from the Remotes briefing
(`origin` / `upstream`) into that path. `/sandbox/.honr` is always present at
start with at least `report.schema.json`.

**Rebase onto upstream `main`.** Re-running a card resumes its existing branch
and rebases onto the PR-target tip. If the rebase conflicts, the supervisor
backs out and tells the agent to resolve it.

**Policy is immutable on a live sandbox** for filesystem and process sections.
Set it at create time; `policy set --wait` is expensive.

**Binary paths are matched literally** in the policy. Lists are deliberately
generous (git's real remote helper is `/usr/lib/git-core/git-remote-http`).

## Cockpit vs worker containment

| Catalog id | Seed source | Egress |
|---|---|---|
| `default` (worker) | Built-in `src/seed_policies.rs` when catalog empty; then **board profile only** | Inference + GitHub (+ package registries). **No** honr MCP — workers stay air-gapped from the board. |
| `cockpit` | [`cockpit-policy.yaml`](../sandbox/cockpit-policy.yaml) at seed; then **board profile only** | Host honr MCP (`host.docker.internal` / `127.0.0.1` / `localhost`:8080) + inference + GitHub (`GH_TOKEN`). **No** package-registry allow-list. |

Settings → OpenShell → Sandbox specs is the live source of truth for both. The
cockpit spec seeds with `github` + `vertex` + `antigravity` attach names so
Claude/OpenCode can use workspace `inference.local`, `gh` gets an App token,
and agy gets a Bearer placeholder (never a host OAuth file).

## Antigravity / agy

Bare OpenShell `generic` providers do **not** resolve
`openshell:resolve:…` placeholders — the egress proxy only substitutes on
endpoints declared by a **provider type**. Honr ships
[`sandbox/openshell/antigravity.yaml`](../sandbox/openshell/antigravity.yaml)
(`auth_style: bearer`, Cloud Code / Google API hosts) and imports it on
provider Sync when missing.

| Layer | Holds |
|---|---|
| Host keychain (`gemini` / `antigravity`) | Live OAuth access token (refresh stays on host) |
| Board provider `antigravity` | Sealed `ANTIGRAVITY_ACCESS_TOKEN` (refreshed from keychain on Sync) |
| Gateway | Real credential; injects placeholder env into attached sandboxes |
| Seat token file | Placeholder only + far-future expiry; **no** `refresh_token` |
| Seat `settings.json` | `enableTelemetry: false`, `gcp.project` / `gcp.location` from Board provider config `ANTIGRAVITY_GCP_PROJECT` / `ANTIGRAVITY_GCP_LOCATION` (Settings → Providers — not host files, not Vertex/`GOOGLE_CLOUD_PROJECT`) |

Existing cockpit specs created before this attach name need the checkbox (or
board ensure on startup appends `antigravity` to the cockpit create-spec).
