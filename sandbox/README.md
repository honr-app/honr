# Sandbox assets

Inputs to `src/openshell.rs` and `src/supervisor.rs`. How a run works and what
breaks if you change them: [`docs/sandbox.md`](../docs/sandbox.md).

Card context is briefing-only (`/sandbox/.honr` contracts); see
[`docs/sandbox.md`](../docs/sandbox.md).

## Worker network policy (board profile)

The **card-worker** network policy is **not** a file in this directory. It lives
on the board sandbox profile (`default`): Settings → OpenShell → Profiles, or
`GET`/`POST /api/sandbox-profiles`. That YAML is what OpenShell gets at
`sandbox create`.

Empty boards seed `default` from the built-in string in
`src/seed_policies.rs` (`policy: embedded` in `honr.yaml`). After seed, edit the
**profile on the board** — changing source or docs does not change live
sandboxes. Policy filesystem/process sections are immutable on a live sandbox;
set them at create time.

## `cockpit-policy.yaml`

The **cockpit** control-plane seat policy: narrow egress to the host honr MCP
endpoint (`host.docker.internal` / `127.0.0.1` / `localhost` on port 8080) plus
inference. It does not include GitHub or package-registry egress — those stay
on the worker identity. Seeded into the sandbox-profile catalog as id `cockpit`
(Settings → OpenShell → Profiles) with lighter cpu/memory than the worker
default. After seed, the board profile is authoritative for cockpit too.

## `Containerfile`

The base image has no Rust toolchain and the `sandbox` user has no sudo, so an agent cannot build
or test honr out of the box. This adds `cargo`/`clippy` and pre-warms the cargo and npm caches, so
a card doesn't pay a rustup install and a cold crates.io fetch every run — and so crates.io never
needs to be reachable from an agent sandbox at all.

Build from the **repo root**, not this directory:

```bash
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

Rebuild when `Cargo.lock` changes. Matching `/opt` entries belong in the worker
**board** profile policy (and the embedded seed in `src/seed_policies.rs`).

## `metadata-shim.py`

A minimal GCE metadata server, uploaded to the sandbox and run on `127.0.0.1:8127` for the lifetime
of an agent.
