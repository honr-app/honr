//! Minimal OpenShell policy used as create-form / last-resort defaults.
//!
//! Live policy lives on board sandbox specs (Settings → Sandbox specs).

/// Bare-bones policy for a new sandbox spec. No honr MCP, no package registries,
/// no language-toolchain paths — operators add egress as needed.
pub const MINIMAL_SANDBOX_POLICY: &str = r#"# Minimal OpenShell sandbox policy.
# Edit under Settings → OpenShell → Sandbox specs for your egress needs.
version: 1

filesystem_policy:
  include_workdir: true
  read_only: [/usr, /lib, /proc, /etc, /var/log]
  read_write: [/sandbox, /tmp, /dev]

landlock:
  compatibility: best_effort

network_policies: {}
"#;

