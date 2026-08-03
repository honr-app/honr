//! Typed async wrapper over the `openshell` CLI.
//!
//! One place that knows the CLI's shape, so the supervisor never builds an
//! argv. We shell out rather than use `openshell-sdk` because the SDK does not
//! support mTLS and our gateway is mTLS-only — see `docs/sandbox-stack.md`
//! for the full reasoning and the condition to revisit it.
//!
//! **Everything here takes a timeout, and that is not defensive style.** Every
//! failure mode observed in phase 0 — blocked metadata server, denied egress,
//! git waiting on a credential prompt — presented as a *hang*, not an error. A
//! call without a deadline is a supervisor that stops making progress and
//! never says why.

use serde::Deserialize;
use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("`openshell {argv}` timed out after {secs}s")]
    Timeout { argv: String, secs: u64 },
    #[error("`openshell {argv}` exited {code}: {stderr}")]
    Failed { argv: String, code: i32, stderr: String },
    #[error("could not spawn `openshell`: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("could not parse `openshell {argv}` output: {source}")]
    Parse { argv: String, #[source] source: serde_json::Error },
    #[error("could not write sandbox policy temp file: {0}")]
    PolicyTemp(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Default CLI name when Settings has no binary-path override.
pub const DEFAULT_BIN: &str = "openshell";

/// Outcome of `openshell status` for Settings → OpenShell and ops surfaces.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct GatewayStatus {
    pub healthy: bool,
    /// Binary that was invoked (`openshell` or a Settings override).
    pub binary: String,
    /// Short human summary (stdout/stderr trim, or an actionable error).
    pub summary: String,
    /// True when the CLI binary could not be spawned (missing from PATH / bad path).
    pub cli_missing: bool,
    /// Optional detail when unhealthy or CLI missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn status_summary(stdout: &str, stderr: &str) -> String {
    let out = stdout.trim();
    if !out.is_empty() {
        return out.chars().take(2000).collect();
    }
    let err = stderr.trim();
    if !err.is_empty() {
        return err.chars().take(2000).collect();
    }
    String::new()
}

/// One sandbox, as `sandbox list -o json` reports it. Deliberately partial:
/// unknown fields are ignored so a CLI that grows a field doesn't break us.
#[derive(Debug, Clone, Deserialize)]
pub struct Sandbox {
    pub name: String,
    /// Unused by the supervisor, but kept because `sandbox list` is also how a
    /// human debugs a stuck run and these are what they grep for.
    #[allow(dead_code)]
    #[serde(default)]
    pub id: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

impl Sandbox {
    /// The work item this sandbox belongs to, from the `honr.item` label.
    pub fn item_id(&self) -> Option<u64> {
        self.labels.get(LABEL_ITEM)?.parse().ok()
    }
}

/// How a sandbox is created. Mirrors the flags proven in phase 0.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub name: String,
    /// `--from`. **Not** `--image`: a bare name resolves against the community
    /// registry, a path builds a Dockerfile, a tag is an image reference.
    pub from: String,
    pub providers: Vec<String>,
    /// Inline OpenShell policy YAML. Materialized to a temp file for `--policy`
    /// at create — the board never treats a host path as source of truth.
    pub policy: Option<String>,
    pub env: Vec<(String, String)>,
    pub labels: Vec<(String, String)>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

/// Temp file holding inline policy YAML for `--policy`. Deleted on drop so
/// create does not leave host paths in the board or require a pre-existing file.
struct PolicyTempFile {
    path: std::path::PathBuf,
}

impl PolicyTempFile {
    fn write(yaml: &str) -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "honr-policy-{}-{nanos}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, yaml)?;
        Ok(Self { path })
    }

    fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for PolicyTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write inline policy YAML to a temp path for `openshell sandbox create --policy`.
/// Returns `None` when no policy is set.
fn materialize_policy(policy_yaml: Option<&str>) -> Result<Option<PolicyTempFile>> {
    match policy_yaml {
        None => Ok(None),
        Some(yaml) => Ok(Some(PolicyTempFile::write(yaml).map_err(Error::PolicyTemp)?)),
    }
}

/// What a finished command produced.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

pub const LABEL_ITEM: &str = "honr.item";

/// Argv for `sandbox create`, split out from the call so it can be asserted
/// without a gateway. Worth testing on its own: the flags are exactly the kind
/// of thing that breaks silently — the image flag is `--from`, not `--image`,
/// and getting it wrong yields a confusing registry lookup rather than an error.
fn create_args(spec: &SandboxSpec) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "sandbox".into(),
        "create".into(),
        "--name".into(),
        spec.name.clone(),
        "--from".into(),
        spec.from.clone(),
        "--no-tty".into(),
    ];

    for p in &spec.providers {
        args.push("--provider".into());
        args.push(p.clone());
    }
    if let Some(policy) = &spec.policy {
        // Caller passes a host path here — OpenShell::create materializes
        // inline YAML into a temp file before invoking create_args.
        args.push("--policy".into());
        args.push(policy.clone());
    }
    for (k, v) in &spec.env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    for (k, v) in &spec.labels {
        args.push("--label".into());
        args.push(format!("{k}={v}"));
    }
    if let Some(cpu) = &spec.cpu {
        args.push("--cpu".into());
        args.push(cpu.clone());
    }
    if let Some(mem) = &spec.memory {
        args.push("--memory".into());
        args.push(mem.clone());
    }

    // The policy must be passed at creation: filesystem and process sections
    // are immutable on a live sandbox, and `policy set --wait` costs ~50s.
    args.push("--".into());
    args.push("echo".into());
    args.push("up".into());
    args
}

#[cfg(test)]
type MockHandler = std::sync::Arc<dyn Fn(&[String]) -> Output + Send + Sync>;

#[derive(Clone)]
pub struct OpenShell {
    bin: String,
    /// Applies to control-plane calls (create, list, delete). Exec carries its
    /// own, because an agent legitimately runs for minutes.
    default_timeout: Duration,
    /// In-process stand-in for unit tests. Spawning a bash mock under nextest
    /// contention was the remaining ~1–2s per process_verdict case.
    #[cfg(test)]
    mock: Option<MockHandler>,
}

impl std::fmt::Debug for OpenShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenShell")
            .field("bin", &self.bin)
            .field("default_timeout", &self.default_timeout)
            .finish()
    }
}

impl Default for OpenShell {
    fn default() -> Self {
        Self {
            bin: DEFAULT_BIN.into(),
            default_timeout: Duration::from_secs(120),
            #[cfg(test)]
            mock: None,
        }
    }
}

impl OpenShell {
    /// Used by tests to point at a stand-in binary.
    #[allow(dead_code)]
    pub fn new(bin: impl Into<String>, default_timeout: Duration) -> Self {
        Self {
            bin: bin.into(),
            default_timeout,
            #[cfg(test)]
            mock: None,
        }
    }

    /// In-process CLI stand-in — no process spawn. Prefer this over shell
    /// scripts in unit tests; argv shape stays the same as the real CLI.
    #[cfg(test)]
    pub fn mock(
        handler: impl Fn(&[String]) -> Output + Send + Sync + 'static,
        default_timeout: Duration,
    ) -> Self {
        Self {
            bin: "openshell-mock".into(),
            default_timeout,
            mock: Some(std::sync::Arc::new(handler)),
        }
    }

    // ------------------------------------------------------------- plumbing

    /// Run to completion under a deadline. On timeout the child is killed —
    /// otherwise a hung CLI outlives the supervisor that gave up on it.
    async fn run<I, S>(&self, args: I, timeout: Duration) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<String> =
            args.into_iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
        let argv = args.join(" ");

        #[cfg(test)]
        if let Some(mock) = &self.mock {
            let _ = (&timeout, &argv);
            return Ok(mock(&args));
        }

        let child = Command::new(&self.bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::Spawn)?;

        let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(r) => r.map_err(Error::Spawn)?,
            Err(_) => {
                return Err(Error::Timeout { argv, secs: timeout.as_secs() });
            }
        };

        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run, and treat a non-zero exit as an error.
    async fn run_ok<I, S>(&self, args: I, timeout: Duration) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<String> =
            args.into_iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
        let argv = args.join(" ");
        let out = self.run(args, timeout).await?;
        if !out.ok() {
            return Err(Error::Failed {
                argv,
                code: out.code,
                stderr: out.stderr.trim().to_string(),
            });
        }
        Ok(out)
    }

    // -------------------------------------------------------- the verbs

    /// Is the gateway reachable? Cheap enough to call before claiming a card,
    /// and worth it: the podman machine stops on its own.
    pub async fn healthy(&self) -> bool {
        self.gateway_status().await.healthy
    }

    /// Run `openshell status` and classify the result for Settings / ops.
    /// Distinguishes a missing CLI (`cli_missing`) from an unhealthy gateway.
    pub async fn gateway_status(&self) -> GatewayStatus {
        match self.run(["status"], Duration::from_secs(15)).await {
            Ok(o) if o.ok() => {
                let summary = status_summary(&o.stdout, &o.stderr);
                GatewayStatus {
                    healthy: true,
                    binary: self.bin.clone(),
                    summary: if summary.is_empty() {
                        "Connected".into()
                    } else {
                        summary
                    },
                    cli_missing: false,
                    error: None,
                }
            }
            Ok(o) => {
                let summary = status_summary(&o.stdout, &o.stderr);
                GatewayStatus {
                    healthy: false,
                    binary: self.bin.clone(),
                    summary: if summary.is_empty() {
                        format!("openshell status exited {}", o.code)
                    } else {
                        summary
                    },
                    cli_missing: false,
                    error: Some(format!("openshell status exited {}", o.code)),
                }
            }
            Err(Error::Spawn(e)) => {
                let missing = e.kind() == std::io::ErrorKind::NotFound;
                GatewayStatus {
                    healthy: false,
                    binary: self.bin.clone(),
                    summary: if missing {
                        format!(
                            "OpenShell CLI not found at `{}` — install it or set a binary path in Settings → OpenShell",
                            self.bin
                        )
                    } else {
                        format!("could not spawn `{}`: {e}", self.bin)
                    },
                    cli_missing: missing,
                    error: Some(e.to_string()),
                }
            }
            Err(e) => GatewayStatus {
                healthy: false,
                binary: self.bin.clone(),
                summary: e.to_string(),
                cli_missing: false,
                error: Some(e.to_string()),
            },
        }
    }

    pub async fn list(&self) -> Result<Vec<Sandbox>> {
        let out = self.run_ok(["sandbox", "list", "-o", "json"], self.default_timeout).await?;
        serde_json::from_str(&out.stdout)
            .map_err(|source| Error::Parse { argv: "sandbox list".into(), source })
    }

    /// Sandboxes this honr created, keyed by work item.
    pub async fn list_ours(&self) -> Result<Vec<Sandbox>> {
        Ok(self.list().await?.into_iter().filter(|s| s.item_id().is_some()).collect())
    }

    /// Create and keep alive; we exec into it afterwards. The trailing command
    /// is a no-op that just proves the sandbox came up.
    ///
    /// `spec.policy` is inline YAML content. We write a temp file for the CLI's
    /// `--policy` flag and delete it when create returns — the board must not
    /// store host paths as the policy source of truth.
    pub async fn create(&self, spec: &SandboxSpec) -> Result<()> {
        // Sandbox startup is seconds, not milliseconds, and this is alpha
        // software — give creation more room than a control-plane call.
        let policy_file = materialize_policy(spec.policy.as_deref())?;
        let mut path_spec = spec.clone();
        if let Some(ref f) = policy_file {
            path_spec.policy = Some(f.path_string());
        }
        self.run_ok(create_args(&path_spec), Duration::from_secs(300))
            .await?;
        Ok(())
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        self.run_ok(["sandbox", "delete", name], self.default_timeout).await?;
        Ok(())
    }

    pub async fn upload(&self, name: &str, local: &str, dest: &str) -> Result<()> {
        self.run_ok(["sandbox", "upload", name, local, dest], self.default_timeout).await?;
        Ok(())
    }

    /// Download a file from a sandbox to the host (verdict file protocol).
    pub async fn download(&self, name: &str, remote: &str, dest: &str) -> Result<()> {
        self.run_ok(["sandbox", "download", name, remote, dest], self.default_timeout).await?;
        Ok(())
    }

    /// Unused by the supervisor; `openshell logs` is currently a human's tool.
    #[allow(dead_code)]
    pub async fn logs(&self, name: &str, tail: u32) -> Result<String> {
        let out =
            self.run(["logs", name, "-n", &tail.to_string()], self.default_timeout).await?;
        Ok(out.stdout)
    }

    /// Run a command in a sandbox and wait for it.
    ///
    /// Note both timeouts. `--timeout` is the CLI's own, which the gateway
    /// enforces remotely; the outer one covers the CLI process itself wedging.
    /// A remote timeout leaves us a diagnosable exit code, so it is set
    /// slightly tighter.
    pub async fn exec(&self, name: &str, script: &str, timeout: Duration) -> Result<Output> {
        let remote = timeout.as_secs().saturating_sub(5).max(1);
        self.run(
            [
                "sandbox",
                "exec",
                "-n",
                name,
                "--timeout",
                &remote.to_string(),
                "--",
                "bash",
                "-lc",
                script,
            ],
            timeout,
        )
        .await
    }

    /// Run a command and hand every stdout line to `on_line` as it arrives.
    ///
    /// This is how liveness and cost stay *observed rather than self-reported*:
    /// the supervisor watches `claude --output-format stream-json` go by and
    /// heartbeats on real activity, so a hung agent cannot claim to be fine.
    ///
    /// `on_line` is called from the read loop, so it must not block.
    pub async fn exec_streaming<F>(
        &self,
        name: &str,
        script: &str,
        timeout: Duration,
        mut on_line: F,
    ) -> Result<Output>
    where
        F: FnMut(&str) + Send,
    {
        let remote = timeout.as_secs().saturating_sub(5).max(1);
        let args = [
            "sandbox",
            "exec",
            "-n",
            name,
            "--timeout",
            &remote.to_string(),
            "--",
            "bash",
            "-lc",
            script,
        ];
        let argv = format!("sandbox exec -n {name}");

        let mut child = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::Spawn)?;

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let collect_err = tokio::spawn(async move {
            let mut buf = String::new();
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                buf.push_str(&l);
                buf.push('\n');
            }
            buf
        });

        let pump = async {
            let mut out = String::new();
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await.map_err(Error::Spawn)? {
                on_line(&line);
                out.push_str(&line);
                out.push('\n');
            }
            let status = child.wait().await.map_err(Error::Spawn)?;
            Ok::<_, Error>((status, out))
        };

        match tokio::time::timeout(timeout, pump).await {
            Ok(res) => {
                let (status, stdout) = res?;
                Ok(Output {
                    code: status.code().unwrap_or(-1),
                    stdout,
                    stderr: collect_err.await.unwrap_or_default(),
                })
            }
            Err(_) => Err(Error::Timeout { argv, secs: timeout.as_secs() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            name: "honr-card-7".into(),
            from: "honr-sandbox:latest".into(),
            providers: vec!["vertex".into(), "gh-clankr".into()],
            // create_args takes a path; OpenShell::create materializes YAML first.
            policy: Some("/tmp/honr-policy-test.yaml".into()),
            env: vec![("CLAUDE_CODE_USE_VERTEX".into(), "1".into())],
            labels: vec![(LABEL_ITEM.into(), "7".into())],
            cpu: Some("2".into()),
            memory: Some("4Gi".into()),
        }
    }

    #[tokio::test]
    async fn gateway_status_healthy_when_status_exits_zero() {
        let os = OpenShell::mock(
            |args| {
                assert_eq!(args, &["status".to_string()]);
                Output {
                    code: 0,
                    stdout: "Connected\nAuthenticated (mTLS transport)\n".into(),
                    stderr: String::new(),
                }
            },
            Duration::from_secs(5),
        );
        let st = os.gateway_status().await;
        assert!(st.healthy);
        assert!(!st.cli_missing);
        assert!(st.summary.contains("Connected"));
        assert!(os.healthy().await);
    }

    #[tokio::test]
    async fn gateway_status_unhealthy_when_status_exits_nonzero() {
        let os = OpenShell::mock(
            |_| Output {
                code: 1,
                stdout: String::new(),
                stderr: "gateway unreachable".into(),
            },
            Duration::from_secs(5),
        );
        let st = os.gateway_status().await;
        assert!(!st.healthy);
        assert!(!st.cli_missing);
        assert!(st.summary.contains("gateway unreachable"));
        assert!(!os.healthy().await);
    }

    #[tokio::test]
    async fn gateway_status_marks_cli_missing_on_spawn_not_found() {
        let os = OpenShell::new(
            "/nonexistent/honr-openshell-bin-should-not-exist",
            Duration::from_secs(5),
        );
        let st = os.gateway_status().await;
        assert!(!st.healthy);
        assert!(st.cli_missing, "summary={}", st.summary);
        assert!(st.summary.contains("not found") || st.summary.contains("CLI"));
        assert!(!os.healthy().await);
    }

    /// The image flag is `--from`. `--image` does not exist, and passing it
    /// fails in a way that reads like a registry problem.
    #[test]
    fn image_is_passed_as_from() {
        let args = create_args(&spec());
        let i = args.iter().position(|a| a == "--from").expect("--from present");
        assert_eq!(args[i + 1], "honr-sandbox:latest");
        assert!(!args.iter().any(|a| a == "--image"));
    }

    /// Repeated flags, not comma-joined lists — one `--provider` each.
    #[test]
    fn each_provider_gets_its_own_flag() {
        let args = create_args(&spec());
        let providers: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "--provider")
            .map(|(i, _)| args[i + 1].clone())
            .collect();
        assert_eq!(providers, vec!["vertex", "gh-clankr"]);
    }

    /// Env and labels are `KEY=VALUE`; the label is what reconciliation reads
    /// after a restart to match a live sandbox back to its card.
    #[test]
    fn env_and_labels_are_key_equals_value() {
        let args = create_args(&spec());
        assert!(args.contains(&"CLAUDE_CODE_USE_VERTEX=1".to_string()));
        assert!(args.contains(&"honr.item=7".to_string()));
    }

    /// The policy has to be there at creation — it cannot be added later.
    #[test]
    fn policy_is_passed_at_creation() {
        let args = create_args(&spec());
        let i = args.iter().position(|a| a == "--policy").expect("--policy present");
        assert_eq!(args[i + 1], "/tmp/honr-policy-test.yaml");
    }

    /// Inline YAML is written to a temp file whose contents match; the path is
    /// what `--policy` receives — never a board-stored host path.
    #[test]
    fn inline_policy_materializes_to_temp_file() {
        let yaml = "version: 1\nfilesystem_policy:\n  include_workdir: true\n";
        let file = materialize_policy(Some(yaml))
            .expect("materialize")
            .expect("some file");
        let path = file.path_string();
        assert!(path.ends_with(".yaml"), "path={path}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), yaml);
        drop(file);
        assert!(
            !std::path::Path::new(&path).exists(),
            "temp policy file should be removed on drop"
        );
    }

    #[tokio::test]
    async fn create_passes_temp_path_whose_contents_are_inline_yaml() {
        let yaml = "version: 1\n# inline create test\n";
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(None::<(String, String)>));
        let seen_c = seen.clone();
        let os = OpenShell::mock(
            move |args| {
                let i = args.iter().position(|a| a == "--policy").expect("--policy");
                let path = args[i + 1].clone();
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                *seen_c.lock() = Some((path, content));
                Output {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                }
            },
            Duration::from_secs(5),
        );
        let mut s = spec();
        s.policy = Some(yaml.into());
        os.create(&s).await.expect("create");
        let (path, content) = seen.lock().clone().expect("policy seen");
        assert!(path.contains("honr-policy-"), "temp path={path}");
        assert_eq!(content, yaml);
    }

    #[test]
    fn item_id_round_trips_through_the_label() {
        let json = r#"[{"name":"honr-card-7","phase":"Running","labels":{"honr.item":"7"}}]"#;
        let boxes: Vec<Sandbox> = serde_json::from_str(json).unwrap();
        assert_eq!(boxes[0].item_id(), Some(7));
    }

    /// Unknown fields must not break parsing: the CLI is alpha and will grow
    /// keys, and a sandbox list that fails to parse strands live sandboxes.
    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"[{"name":"x","brand_new_field":42,"labels":{}}]"#;
        let boxes: Vec<Sandbox> = serde_json::from_str(json).unwrap();
        assert_eq!(boxes[0].name, "x");
        assert_eq!(boxes[0].item_id(), None);
    }

    /// The failure mode that matters: everything in this stack fails as a
    /// *hang*, so a deadline must produce `Timeout` rather than a supervisor
    /// that quietly stops making progress. `sleep` stands in for a wedged CLI.
    #[tokio::test]
    async fn a_hang_becomes_a_timeout() {
        let os = OpenShell::default();
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            OpenShell::new("sleep", Duration::from_secs(30)).run(["30"], Duration::from_millis(250)),
        )
        .await
        .expect("the deadline itself must not hang")
        .expect_err("a 30s sleep under a 250ms deadline must fail");

        assert!(matches!(err, Error::Timeout { .. }), "got {err:?}");
        let _ = os;
    }

    // ---- gateway-backed. `cargo test -- --ignored` with podman + gateway up.
    // Ignored by default so the suite stays hermetic: these assert against the
    // real CLI, which is the only way to catch a flag or output-shape change.

    #[tokio::test]
    #[ignore = "needs a running OpenShell gateway"]
    async fn gateway_is_reachable() {
        assert!(OpenShell::default().healthy().await);
    }

    #[tokio::test]
    #[ignore = "needs a running OpenShell gateway"]
    async fn list_parses_real_cli_output() {
        // Asserting it parses, not what it contains — an empty gateway is fine.
        OpenShell::default().list().await.expect("sandbox list -o json parses");
    }

}
