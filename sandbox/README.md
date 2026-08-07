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

## Cockpit network policy (board profile)

The **cockpit** control-plane seat policy lives on the board sandbox profile
(`cockpit`): Settings → OpenShell → Profiles. Egress: host honr MCP
(`host.docker.internal` / `127.0.0.1` / `localhost` on port 8080), inference,
and GitHub (App `GH_TOKEN` via the `github` provider). Package registries stay
on the worker identity.

Empty boards seed `cockpit` from `DEFAULT_COCKPIT_SANDBOX_POLICY` in
`src/seed_policies.rs` (lighter cpu/memory than the worker default). After
seed, the board profile is authoritative — edit there, not a host YAML path.

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
the worker **board** profile policy (and the embedded seed in
`src/seed_policies.rs`). Live profiles do not auto-pick up seed edits — add
`/opt/cargo-target` to `read_write` if the catalog already exists.

Claude / OpenCode auth goes through OpenShell `inference.local` (see
[`docs/sandbox.md`](../docs/sandbox.md)) — no in-sandbox metadata shim.
