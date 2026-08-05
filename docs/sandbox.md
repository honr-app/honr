# Sandbox

How a sandboxed agent run works, and the operator-relevant gotchas. Assets live
under [`sandbox/`](../sandbox/); this page is the prose companion.

## How credentials reach the agent

Claude Code on Vertex walks google-auth's ADC chain. In a sandbox that chain
finds nothing and falls through to the GCE metadata server. OpenShell blocks
metadata endpoints permanently as SSRF hardening. No policy can open them.
Symptom: `claude -p` appears to hang, then the exec relay closes.

The fix is a metadata server the sandbox *is* allowed to reach:

```
OpenShell provider (gateway-side, holds gcloud ADC, auto-refreshes)
    │  injects an OPAQUE PLACEHOLDER as GOOGLE_VERTEX_AI_TOKEN
    ▼
sandbox/metadata-shim.py on 127.0.0.1:8127     ← GCE_METADATA_HOST
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

- **No credential is ever in the sandbox.** Reading the env var gains an agent
  nothing: the value is an opaque placeholder; the proxy substitutes on egress.
- **Traffic goes straight to `aiplatform.googleapis.com`**, which the
  `google-vertex-ai` provider already allow-lists. OpenShell's `inference.local`
  router has been blocked by SSRF on this stack
  ([NVIDIA/OpenShell#2478](https://github.com/NVIDIA/OpenShell/issues/2478));
  the shim path sidesteps that.

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
`cargo` / `clippy` and pre-warms cargo and npm caches so crates.io never needs
to be reachable from an agent sandbox.

```bash
# from the repo root; Cargo.lock and web/package-lock.json must be in context
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

The image flag is `--from`, not `--image`. Rebuild when `Cargo.lock` changes
materially. Matching `/opt` entries belong in the worker **board** sandbox
profile policy (Settings → Profiles → `default`; seed text in
`src/seed_policies.rs`).

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
to `/tmp/metadata-shim.py` creates a *directory* of that name with the file
inside it, and python then reports
`can't find '__main__' module in '/tmp/metadata-shim.py'`. Put the shim in
`/tmp` so it lands at `/tmp/metadata-shim.py`.

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
| `cockpit` | [`cockpit-policy.yaml`](../sandbox/cockpit-policy.yaml) at seed; then **board profile only** | Host honr MCP (`host.docker.internal` / `127.0.0.1` / `localhost`:8080) + inference. **No** GitHub or package-registry allow-list. |

Settings → OpenShell → Profiles is the live source of truth for both. The cockpit
profile seeds with distinct cpu/memory (`1` / `2Gi`) from the worker default.
Card dispatch keeps using the worker default unless a Project overrides the
profile id.

## Failure signatures

| Symptom | Cause |
|---|---|
| `can't find '__main__' module in '/tmp/metadata-shim.py'` | `upload` takes a destination *directory* |
| `timeout: failed to run command 'cargo'` | toolchain not on PATH. Image `ENV` does not reach `sandbox exec` |
| `push failed:` with nothing after it | git writes errors to stderr; check `outerr`, not stdout |
| `(stale info)` on push | `--force-with-lease` against an ad-hoc URL instead of a named remote |
| `create sandbox failed: connection error` | compute driver died; infrastructure, not a card retry |
| Card flips Running → Backlog | `run_deadline_at` exceeded, Halt, or infrastructure bounce |

## Assets

See [`sandbox/README.md`](../sandbox/README.md) for profile vs seed policy,
`cockpit-policy.yaml`, `Containerfile`, and `metadata-shim.py` in short form.
