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

## Why the CLI wrapper, not the gRPC SDK

`src/openshell.rs` shells out to the `openshell` CLI. The SDK does not support
mTLS, and the gateway is mTLS-only. Crates are not on crates.io, and the curated
surface cannot stream exec: streaming is how liveness is observed. Revisit when
the gateway moves off mTLS *and* the crates are published.

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
materially. Matching `/opt` entries belong in `policy.yaml` (documented inline
in the Containerfile).

Pass `--offline` in gate commands so a cache miss fails loudly instead of
hanging on a denied fetch.

## Operator-relevant gotchas

**Everything fails as a hang, not an error.** Denied egress, missing
credential, wedged relay: all silence. Every exec needs a deadline; treat
silence as failure.

**The image's `ENV` does not reach `openshell sandbox exec`.** Pass toolchain
vars explicitly in `agent_env` (supervisor does this), or install wrappers on
the default PATH. Baking `ENV PATH=…` into the Containerfile is not enough.

**`sandbox upload` takes a destination directory**, and that directory must
already exist. Wrong shape surfaces as
`can't find '__main__' module in '/tmp/metadata-shim.py'`.

**The compute driver can stop on its own.** Classify that as infrastructure,
not as the card failing: see `is_infrastructure` in the supervisor.

**`/sandbox/repo` may start empty.** When the supervisor does not pre-clone,
the agent clones from the Remotes briefing (`origin` / `upstream`) into that
path. `/sandbox/.honr` is always present at start with at least
`report.schema.json`.

**The fork's base freezes** the moment it is created. Re-running a card resumes
its existing branch and rebases onto **upstream**, not the fork's base. If the
rebase conflicts, the supervisor backs out and tells the agent to resolve it.

**Policy is immutable on a live sandbox** for filesystem and process sections.
Set it at create time; `policy set --wait` is expensive.

**Binary paths are matched literally** in the policy. Lists are deliberately
generous (git's real remote helper is `/usr/lib/git-core/git-remote-http`).

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

See [`sandbox/README.md`](../sandbox/README.md) for `policy.yaml`,
`Containerfile`, and `metadata-shim.py` in short form.
