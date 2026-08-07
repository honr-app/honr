# Sandbox assets

Inputs to `src/openshell.rs` and `src/supervisor.rs`. How a run works and what
breaks if you change them: [`docs/sandbox.md`](../docs/sandbox.md).

Card context is briefing-only (`/sandbox/.honr` contracts); see
[`docs/sandbox.md`](../docs/sandbox.md).

## Worker network policy (board Policies)

The **card-worker** allow-list is a named row in the board **Policies** catalog
(Settings → OpenShell → Policies, or `GET`/`POST` `/api/openshell/policies`).
A Sandbox spec references it by `policy_id`; that YAML is what OpenShell gets
at `sandbox create`.

Empty boards seed a minimal Policy from `src/seed_policies.rs`. After seed,
edit the **Policy on the board**, and keep the worker sandbox spec pointed at
it. Policy filesystem/process sections are immutable on a live sandbox; set
them at create time.

## Cockpit network policy (board Policies)

The **cockpit** control-plane seat uses the same catalog: a Policy for egress
(host honr MCP on `host.docker.internal` / `127.0.0.1` / `localhost` port 8080,
inference, and GitHub App `GH_TOKEN`) and a Sandbox spec that selects that
policy. Package registries typically stay on the worker Policy.

`sandbox/cockpit-policy.yaml` is a checked-in starting point for a full seat;
paste or adapt it under Settings → OpenShell → Policies when that matches the
install, then attach it from the cockpit Sandbox spec.

## `Containerfile`

The base image has no Rust toolchain and the `sandbox` user has no sudo, so an agent cannot build
or test honr out of the box. This adds `cargo`/`clippy`, pre-warms the npm cache, and
**pre-compiles** the Rust dependency tree into `/opt/cargo-target` (with the crates.io
registry under `/opt/cargo`), so a card doesn't pay a rustup install, a cold fetch, or a
cold compile every run — and so crates.io never needs to be reachable from an agent sandbox.

Build from the **repo root**, not this directory:

```bash
docker build -f sandbox/Containerfile -t honr-sandbox:latest .
```

Rebuild when `Cargo.lock`, `src/`, or `migrations/` change materially. Matching
`/opt` entries (`/opt/cargo`, `/opt/cargo-target`, `/opt/npm-cache`, …) belong in
the worker **board Policy** (and the embedded seed in `src/seed_policies.rs`).
When the catalog already exists, add `/opt/cargo-target` to `read_write` under
Policies after an image layout change.

Claude / OpenCode auth goes through OpenShell `inference.local` (see
[`docs/sandbox.md`](../docs/sandbox.md)) — no in-sandbox metadata shim.
