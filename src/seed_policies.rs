//! Built-in sandbox policy text for **empty-catalog seed only**.
//!
//! Live worker and cockpit policy live on the board sandbox profiles
//! (`Settings → Profiles`, `meta.sandbox_profiles`). Do not reintroduce host
//! policy path seeds — editing a host file does not change running profiles.

/// Default card-worker OpenShell policy seeded into the `default` profile when
/// the catalog is empty. After seed, edit the profile on the board.
pub const DEFAULT_WORKER_SANDBOX_POLICY: &str = r#"# honr card-worker sandbox: reach Vertex / Cursor for inference and GitHub for
# code. Default-deny includes honr MCP — workers stay air-gapped from the board.
# The privileged cockpit uses DEFAULT_COCKPIT_SANDBOX_POLICY (cockpit profile) instead.
# Live edits: Settings → OpenShell → Profiles → default (not this string).
version: 1

filesystem_policy:
  include_workdir: true
  # /opt/rust is the toolchain itself; /opt/cargo (registry) and
  # /opt/cargo-target (precompiled debug deps) plus /opt/npm-cache are baked by
  # sandbox/Containerfile. Cargo and npm both write during a build, so those
  # must be read_write — without them a build fails on permissions, which
  # surfaces as a hang. /opt/cursor-agent and /opt/opencode are baked CLIs;
  # read-only.
  read_only: [/usr, /lib, /proc, /app, /etc, /var/log, /opt/rust, /opt/cursor-agent, /opt/opencode]
  read_write: [/sandbox, /tmp, /dev, /opt/cargo, /opt/cargo-target, /opt/npm-cache]

landlock:
  compatibility: best_effort

network_policies:
  vertex_ai:
    name: vertex-ai
    endpoints:
      - { host: aiplatform.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*-aiplatform.googleapis.com', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cloudcode-pa.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: daily-cloudcode-pa.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: oauth2.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: www.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: play.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: lh3.googleusercontent.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }

  github:
    name: github
    endpoints:
      - { host: api.github.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: github.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/bin/git }
      - { path: /usr/local/bin/git }
      - { path: /usr/bin/gh }
      - { path: /usr/local/bin/gh }
      - { path: /usr/bin/git-remote-https }
      - { path: /usr/lib/git-core/git-remote-https }
      - { path: /usr/bin/curl }
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      # rustup resolves `cargo` to the toolchain binary; git deps hit github.com
      # as that path, not the /usr/local/bin or /opt/cargo/bin wrapper.
      - { path: /usr/local/bin/cargo }
      - { path: /opt/cargo/bin/cargo }
      - { path: /opt/rust/toolchains/**/bin/cargo }

  # Cargo/npm registries. Use L4 passthrough (`tls: skip`, no `protocol`) —
  # OpenShell MITM injects a CA via SSL_CERT_FILE/CURL_CA_BUNDLE/etc, but
  # cargo's rustls stack uses Mozilla roots and rejects the proxy cert.
  package_registries:
    name: package-registries
    endpoints:
      - { host: index.crates.io, port: 443, access: full, tls: skip }
      - { host: static.crates.io, port: 443, access: full, tls: skip }
      - { host: crates.io, port: 443, access: full, tls: skip }
      - { host: registry.npmjs.org, port: 443, access: full, tls: skip }
    binaries:
      - { path: /usr/local/bin/cargo }
      - { path: /opt/cargo/bin/cargo }
      - { path: /opt/rust/toolchains/**/bin/cargo }
      - { path: /usr/local/bin/rustc }
      - { path: /opt/cargo/bin/rustc }
      - { path: /opt/rust/toolchains/**/bin/rustc }
      - { path: /bin/sh }
      - { path: /usr/bin/sh }
      - { path: /bin/bash }
      - { path: /usr/bin/bash }
      - { path: /usr/bin/curl }
      - { path: /usr/local/bin/curl }
      - { path: /usr/bin/npm }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/agent }
      - { path: /usr/local/bin/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/node }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }

  # Cursor Agent CLI. Builtin OpenShell `cursor` profile only covers editor
  # bootstrap hosts; the CLI's agent loop also hits api5 / repoNN hosts.
  #
  # api5 must be L4 passthrough (`tls: skip`, no `protocol`). With protocol
  # rest/websocket the egress MITM breaks the Cursor CLI's CONNECT/upgrade to
  # agentn.*.api5.cursor.sh (NET:FAIL / CONNECT 403). See OpenShell docs:
  # endpoints without protocol + tls:skip relay the encrypted stream raw.
  cursor:
    name: cursor
    endpoints:
      - { host: api2.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*.api5.cursor.sh', port: 443, access: full, tls: skip }
      - { host: agentn.us.api5.cursor.sh, port: 443, access: full, tls: skip }
      - { host: '*.api5geo.cursor.sh', port: 443, access: full, tls: skip }
      - { host: '*.api5lat.cursor.sh', port: 443, access: full, tls: skip }
      - { host: agentn.api5geo.cursor.sh, port: 443, access: full, tls: skip }
      - { host: agentn.api5lat.cursor.sh, port: 443, access: full, tls: skip }
      - { host: repo.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: 'repo*.cursor.sh', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: repo42.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cursor.blob.core.windows.net, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: download.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: downloads.cursor.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cursor.download.prss.microsoft.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/agent }
      - { path: /usr/local/bin/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/node }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/bin/bash }
      - { path: /bin/bash }

  # OpenCode CLI — models catalog + product hosts. Provider inference hosts
  # (Anthropic/OpenAI/…) come from attached OpenShell providers or Vertex above.
  opencode:
    name: opencode
    endpoints:
      - { host: models.dev, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: opencode.ai, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*.opencode.ai', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: api.opencode.ai, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/bin/bash }
      - { path: /bin/bash }
"#;

/// Default cockpit OpenShell policy seeded into the `cockpit` profile when
/// the catalog is empty (or when ensure-cockpit inserts it). After seed,
/// edit the profile on the board — not a host YAML path.
pub const DEFAULT_COCKPIT_SANDBOX_POLICY: &str = r#"# honr cockpit sandbox: privileged control-plane seat.
# Egress: host honr MCP, inference, and GitHub (App `GH_TOKEN` via the `github`
# provider). Package registries stay on the card-worker profile (`default`).
# Worker sandboxes stay air-gapped from honr (no honr MCP allow-list there).
# Live edits: Settings → OpenShell → Profiles → cockpit (not this string).
version: 1

filesystem_policy:
  include_workdir: true
  # Same /opt layout as the worker image so one Containerfile serves both
  # profiles; cockpit does not need write to cargo/npm registries for package
  # fetch. /opt/cargo-target is read_write so a cockpit build can update
  # fingerprints against the precompiled debug tree.
  read_only: [/usr, /lib, /proc, /app, /etc, /var/log, /opt/rust, /opt/cursor-agent, /opt/opencode, /opt/cargo, /opt/npm-cache]
  read_write: [/sandbox, /tmp, /dev, /opt/cargo-target]

landlock:
  compatibility: best_effort

network_policies:
  # Host honr MCP (Streamable HTTP). OpenShell L7 must use protocol: mcp —
  # protocol: rest + access: full still 403s POST /mcp (policy_denied).
  # Docker/OpenShell reaches the board as host.docker.internal; localhost /
  # 127.0.0.1 cover the same operator URL on the host.
  honr_mcp:
    name: honr-mcp
    endpoints:
      - host: host.docker.internal
        port: 8080
        protocol: mcp
        enforcement: enforce
        mcp:
          allow_all_known_mcp_methods: true
        rules:
          - allow: {}
      - host: 127.0.0.1
        port: 8080
        protocol: mcp
        enforcement: enforce
        mcp:
          allow_all_known_mcp_methods: true
        rules:
          - allow: {}
      - host: localhost
        port: 8080
        protocol: mcp
        enforcement: enforce
        mcp:
          allow_all_known_mcp_methods: true
        rules:
          - allow: {}
    binaries:
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/local/bin/agent }
      - { path: /usr/local/bin/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/node }
      - { path: /usr/bin/curl }
      - { path: /usr/local/bin/curl }
      - { path: /bin/sh }
      - { path: /usr/bin/sh }
      - { path: /bin/bash }
      - { path: /usr/bin/bash }

  vertex_ai:
    name: vertex-ai
    endpoints:
      - { host: aiplatform.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*-aiplatform.googleapis.com', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cloudcode-pa.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: daily-cloudcode-pa.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: oauth2.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: www.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: play.googleapis.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: lh3.googleusercontent.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }

  github:
    name: github
    endpoints:
      - { host: api.github.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: github.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/bin/git }
      - { path: /usr/local/bin/git }
      - { path: /usr/bin/gh }
      - { path: /usr/local/bin/gh }
      - { path: /usr/bin/git-remote-https }
      - { path: /usr/lib/git-core/git-remote-https }
      - { path: /usr/bin/curl }
      - { path: /usr/local/bin/curl }
      - { path: /usr/local/bin/claude }
      - { path: /usr/local/bin/agy }
      - { path: /usr/bin/agy }
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/local/bin/agent }
      - { path: /usr/local/bin/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/node }
      - { path: /bin/sh }
      - { path: /usr/bin/sh }
      - { path: /bin/bash }
      - { path: /usr/bin/bash }

  # Cursor Agent CLI — same L4 passthrough notes as the worker board profile.
  cursor:
    name: cursor
    endpoints:
      - { host: api2.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*.api5.cursor.sh', port: 443, access: full, tls: skip }
      - { host: agentn.us.api5.cursor.sh, port: 443, access: full, tls: skip }
      - { host: '*.api5geo.cursor.sh', port: 443, access: full, tls: skip }
      - { host: '*.api5lat.cursor.sh', port: 443, access: full, tls: skip }
      - { host: agentn.api5geo.cursor.sh, port: 443, access: full, tls: skip }
      - { host: agentn.api5lat.cursor.sh, port: 443, access: full, tls: skip }
      - { host: repo.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: 'repo*.cursor.sh', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: repo42.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cursor.blob.core.windows.net, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: download.cursor.sh, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: downloads.cursor.com, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: cursor.download.prss.microsoft.com, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/agent }
      - { path: /usr/local/bin/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/cursor-agent }
      - { path: /opt/cursor-agent/versions/**/node }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/bin/bash }
      - { path: /bin/bash }

  opencode:
    name: opencode
    endpoints:
      - { host: models.dev, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: opencode.ai, port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: '*.opencode.ai', port: 443, protocol: rest, enforcement: enforce, access: full }
      - { host: api.opencode.ai, port: 443, protocol: rest, enforcement: enforce, access: full }
    binaries:
      - { path: /usr/local/bin/opencode }
      - { path: /opt/opencode/bin/opencode }
      - { path: /usr/bin/node }
      - { path: /usr/local/bin/node }
      - { path: /usr/bin/bash }
      - { path: /bin/bash }
"#;
