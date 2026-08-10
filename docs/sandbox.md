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
| `claude` | `https://inference.local` | Claude appends `/v1/messages`; `--bare` + `--mcp-config` for MCP |
| `opencode` | `https://inference.local/v1` | OpenCode requires the `/v1` suffix |

Do **not** set `CLAUDE_CODE_USE_VERTEX=1` in the sandbox. That forces direct
Vertex + ADC/metadata discovery, which OpenShell blocks (real GCE metadata is
SSRF-hardened). Use `inference.local` instead.

## Gateway client (gRPC + mTLS or OIDC)

`src/openshell.rs` talks to the gateway in-process over gRPC. Settings require
an explicit auth mode:

- **mTLS** — HTTPS with sealed client PEMs (board DB).
- **OIDC** — HTTPS with `authorization: Bearer` (via
  `openshell_core::auth::EdgeAuthInterceptor`); browser PKCE login seals tokens
  in the board DB; refresh uses `openshell-sdk` OIDC helpers.

Endpoint must be `https://`. The only host secret file is
`~/.config/honr/master.key`. Upload/download use exec + tar over that same
channel — no `openshell` CLI spawn. We build the tonic channel ourselves and use
`openshell-core` / `openshell-policy` for protos and YAML policy.

## Agent surface

Card intent and protocol paths come from the supervisor briefing (and files
under `/sandbox/.honr`). Agents finish via `plan.json` / `report.json` /
`escalate.json` / `split.json`. The board is the only tracker — sandboxes do
not carry a separate issue-store CLI or database.

## Image

[`sandbox/Containerfile`](https://github.com/honr-app/honr/blob/main/sandbox/Containerfile) builds honr's own base — a
minimal Red Hat UBI9 image, not the OpenShell community image — plus a Rust
toolchain, split into one build target per agent engine. A `shared` stage
installs OS packages (`git`, `nodejs`/`npm`, `gh`, `gcc`/`make`, `iproute`,
`nftables`, `socat`) and bakes `cargo`/`clippy`, then one leaf stage per agent
engine (`cursor`, `agy`, `claude`, `opencode`) installs only that engine's CLI
on top — a sandbox only ever carries the one binary it will actually run.

The toolchain is baked in, but honr's own source and dependency tree are not.
A card's own `cargo build`/`npm ci` populate `$CARGO_HOME` (`/opt/cargo`),
`$CARGO_TARGET_DIR` (`/opt/cargo-target`), and `$NPM_CONFIG_CACHE`
(`/opt/npm-cache`) at runtime by fetching crates.io/npm live — there is no
pre-baked cache to go stale every time `Cargo.lock` or `src/` changes. This
means the matching Policy has to allow that egress; see
[Default vs Cockpit](#default-vs-cockpit) for the seeded per-engine policies.

Why UBI9 instead of the OpenShell community image: that image bakes in every
supported agent CLI, a Python/uv/cloudpickle skills venv, and Ubuntu
convenience tooling honr never touches, regardless of which engine-specific
target you build. OpenShell's own documented minimum for a custom sandbox
image is just `iproute2` (required) and `nftables` (optional) — see
`examples/bring-your-own-container/Dockerfile` in the OpenShell source.
Building from `ubi9/ubi` plus exactly what honr needs cuts each image from
~15GB (the community-base version) to under 2GB.

```bash
# from the repo root
make sandbox        # builds all four quay.io/honr-app/sandbox-<engine>:latest
make sandbox-push   # builds, then pushes all four
# or: podman build -f sandbox/Containerfile --target cursor -t quay.io/honr-app/sandbox-cursor:latest .
# Docker: CONTAINER_ENGINE=docker make sandbox
# Different registry: REGISTRY=ghcr.io/you make sandbox
```

The image flag is `--from`, not `--image`. Rebuild when you need a newer
engine CLI, OS package, or Rust toolchain version — not when honr's own source
changes, since none of it is baked in. Matching `/opt` entries belong in the
**board Policies** catalog (Settings → OpenShell → Policies):
`/opt/cargo`, `/opt/cargo-target`, `/opt/npm-cache` need **read-write** (a
card's build populates them, not the image), while `/opt/rust` (+ that
engine's own `/opt/cursor-agent` or `/opt/opencode`) stays **read-only**.
`src/seed_policies.rs` seeds one minimal Cockpit policy per engine matching
each split image's contents, including the crates.io/npm/GitHub egress a
build needs — see [Default vs Cockpit](#default-vs-cockpit).

**Binary identity gotcha, verified live:** `/opt/cargo/bin/cargo` is rustup's
proxy binary — it re-execs the real `cargo` under
`/opt/rust/toolchains/<version>/bin/cargo` at runtime. That's a process exec,
not a filesystem symlink, so OpenShell's literal binary-path matching needs
its own entry for the toolchain path (a glob, since the version is baked into
the directory name) or a card's first `cargo build` gets a 403 on crates.io
even with the proxy path allowed.

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
prior contents or caches. The supervisor never clones; the agent does.
`/sandbox/.honr` is always present at start with at least `report.schema.json`.
If a clean-tree reuse rebase conflicts, the supervisor backs out and tells the
agent to resolve it.

**Policy is fixed for a live sandbox** for filesystem and process sections.
Live policy comes from the board Policies catalog and is applied at create time;
`policy set --wait` is expensive.

**Binary paths are matched literally** in the policy. Lists include the real git
helper paths (e.g. `/usr/lib/git-core/git-remote-http`).

## Default vs Cockpit

Four sandbox specs come seeded — `sandbox-cursor`, `sandbox-agy`,
`sandbox-claude`, `sandbox-opencode` — one per split image, each pointed at a
matching minimal Cockpit policy (`cockpit-cursor`, …) with honr MCP already
attached. Seeding never sets a default — which engine to run is your call, and
a fresh board's Welcome page flags "Sandbox spec" as not ready until you make
it. Pick one under **Settings → OpenShell → Sandbox specs** and click **Set
default**; Cockpit inherits that default until you pick a different seeded row
(or a profile you made) and click **Use for Cockpit**. These rows are inserts,
not overwrites — editing one sticks; a re-seed on the next boot leaves your
edit alone.

Attach on create starts empty for a profile you make yourself — add providers
under **Providers**, then check them on the spec. Add honr MCP /
package-registry / toolchain egress under **Policies** when you need it.

Cockpit MCP does **not** use `host.docker.internal` and does not cross the
network at all. When the seat is Ready, honr keeps a board-owned
`ExecSandboxInteractive` relay running one-shot `socat UNIX-LISTEN:… STDIO`
on a local Unix socket inside the sandbox, and wires its gRPC-piped
stdin/stdout straight into the same `Operator` MCP handler that serves host
`/mcp` (`rmcp::serve_server` over the pipe). Disconnect ends the listen so
the board can re-spawn; the agent's MCP client is stdio (`socat -
UNIX-CONNECT:<socket>`) — same path on local Docker/Podman and remote
Kubernetes, since it never leaves the sandbox's own netns. OpenShell SSH has
no RemoteForward either way, so this was never a `ssh -R` option. Not `nc`:
see [Cockpit](cockpit.md#how-the-mcp-relay-works) for why.

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

Under **Settings → OpenShell → Providers**, use **Log in with Google** on the
`antigravity` provider (host-mediated PKCE against Google’s Antigravity
installed-app client). That seals the access token plus refresh material on the
board; the gateway’s `oauth2_refresh_token` strategy keeps `ya29` fresh. honr
does not read the host keychain — it makes no assumptions about credentials
sitting on the machine it runs on.

| Layer | Holds |
|---|---|
| Board provider `antigravity` | Sealed `ANTIGRAVITY_ACCESS_TOKEN` + refresh material (`client_id` / `client_secret` / `refresh_token`) from Log in with Google |
| Gateway | Live credential + refresh; injects placeholder env into attached sandboxes |
| Seat token file | Placeholder only + far-future expiry; **no** seat-side `refresh_token` |
| Seat `settings.json` | `enableTelemetry: false`, `gcp.project` / `gcp.location` from Board provider config `ANTIGRAVITY_GCP_PROJECT` / `ANTIGRAVITY_GCP_LOCATION` (Settings → Providers) |
| Seat env (agy launch) | `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_QUOTA_PROJECT` set from the same Board project so they win over Vertex’s injected project — agy otherwise leaves `quotaProject` empty |
| Seat default model | `gemini-3.6-flash-high` (`DEFAULT_SEAT_MODEL`) — requires the consumer Antigravity OAuth client from Settings → Log in with Google (the Business Cloud Code client returns Flash rows without `vertexModelId`) |

Provider type YAML must list `aiplatform.googleapis.com` (and `*-aiplatform.googleapis.com`) so `streamGenerateContent` / OpenAI-compat chat is allowed under `_provider_antigravity`, not only the cockpit `vertex_ai` policy group. Put `--model` **before** `-p` when invoking agy — `-p` takes the next argv as the prompt.

Attach names that are not in the Providers catalog are pruned on board load.

## Related

- [Your first agent](first-agent.md) — Welcome/Help OpenShell onboarding, then the first run
- [Troubleshooting](troubleshooting.md) — the gotchas above, as symptoms
- [Configuration](configuration.md#policies) — Policies catalog vs Sandbox specs
- [Configuration](configuration.md#sandbox-specs) — spec resolution and engines
