# Sandbox assets

Inputs to `src/openshell.rs` / `src/supervisor.rs` (not yet written). Both files are
**verified working** — see `docs/phase-0-findings.md` for how they were arrived at.

## `policy.yaml`

The sandbox network policy: Vertex for inference, GitHub for code, default-deny for everything else.
Passed with `--policy` at `sandbox create`.

It must be set **at creation** — the filesystem and process sections are immutable on a live
sandbox, and `policy set --wait` costs ~50s.

Binary paths are matched literally, so the lists are deliberately generous (git's real remote helper
is `/usr/lib/git-core/git-remote-http`, note **not** `-https`).

## `metadata-shim.py`

A minimal GCE metadata server, uploaded to the sandbox and run on `127.0.0.1:8127` for the lifetime
of an agent.

Claude Code's Vertex mode walks google-auth's ADC chain and ends at the GCE metadata server, which
OpenShell blocks permanently as SSRF hardening. Pointing `GCE_METADATA_HOST` at this shim gives
google-auth a token source it *is* allowed to reach.

The token it serves is OpenShell's **opaque placeholder**, not a real credential — the egress proxy
substitutes the real value on the way out. So no secret ever exists inside the sandbox.

Serves the endpoints `gcp-metadata` probes: the root (with the `Metadata-Flavor: Google` header),
`/token`, `/project/project-id`, `/service-accounts/default/{email,scopes}`, and
`/universe/universe-domain`.
