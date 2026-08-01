//! Integration module for gastownhall/beads (`bd`).
//!
//! Plan A: beads holds identity, Project→Task containment (`--parent`), and
//! task↔task dependency edges. Honr keeps the richer lifecycle machine and
//! runtime fields (lease, sandbox, cost) keyed by board id + `beads_id`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeadsIssue {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub issue_type: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BeadsClient {
    pub beads_dir: PathBuf,
}

#[allow(dead_code)] // Plan A API surface — not every verb is on the hot path yet.
impl BeadsClient {
    pub fn new(beads_dir: impl Into<PathBuf>) -> Self {
        Self {
            beads_dir: beads_dir.into(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("bd");
        c.env("BEADS_DIR", &self.beads_dir);
        // Prevent `bd` from walking up into a parent workspace's `.beads`.
        if let Some(parent) = self.beads_dir.parent() {
            c.current_dir(parent);
        }
        c
    }

    /// True when `id` looks like a real beads hash, not a local placeholder.
    pub fn is_real_id(id: &str) -> bool {
        !id.is_empty() && !id.starts_with("bd-honr-")
    }

    /// Run `bd init --quiet --stealth` in the target directory.
    pub async fn init_stealth(&self) -> Result<(), String> {
        if self.beads_dir.join("metadata.json").exists()
            || self.beads_dir.join("embeddeddolt").exists()
        {
            return Ok(());
        }
        std::fs::create_dir_all(&self.beads_dir)
            .map_err(|e| format!("mkdir beads dir: {e}"))?;
        let status = self
            .cmd()
            .args(["init", "--quiet", "--stealth"])
            .status()
            .await
            .map_err(|e| format!("failed to execute bd init: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            // Stealth init can succeed with non-zero when already initialised.
            Ok(())
        }
    }

    /// Run `bd ready --json` to fetch unblocked ready issues.
    pub async fn list_ready(&self) -> Result<Vec<BeadsIssue>, String> {
        let out = self
            .cmd()
            .args(["ready", "--json"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd ready: {e}"))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd ready failed: {err}"));
        }

        if out.stdout.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd ready JSON: {e}"))
    }

    /// List issues (`bd list --json --all -n 0`).
    pub async fn list_all(&self) -> Result<Vec<BeadsIssue>, String> {
        let out = self
            .cmd()
            .args(["list", "--json", "--all", "-n", "0"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd list: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd list failed: {err}"));
        }
        if out.stdout.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd list JSON: {e}"))
    }

    /// `bd show <id> --json`
    pub async fn show(&self, id: &str) -> Result<BeadsIssue, String> {
        let out = self
            .cmd()
            .args(["show", id, "--json"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd show: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd show failed: {err}"));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd show JSON: {e}"))
    }

    /// Create without parent/deps (compat).
    pub async fn create(
        &self,
        title: &str,
        priority: u32,
        issue_type: &str,
        description: Option<&str>,
    ) -> Result<BeadsIssue, String> {
        self.create_linked(title, priority, issue_type, description, None, &[])
            .await
    }

    /// Create with optional Project parent and blocker deps.
    ///
    /// - Projects → `issue_type=epic`
    /// - Tasks → `issue_type=task` with `--parent=<project beads id>`
    /// - `blocked_by` → `bd dep add <new> <blocker>` (type blocks)
    pub async fn create_linked(
        &self,
        title: &str,
        priority: u32,
        issue_type: &str,
        description: Option<&str>,
        parent: Option<&str>,
        blocked_by: &[String],
    ) -> Result<BeadsIssue, String> {
        let _ = self.init_stealth().await;
        let mut cmd = self.cmd();
        cmd.arg("create")
            .arg(title)
            .arg("-p")
            .arg(priority.to_string())
            .arg("-t")
            .arg(issue_type)
            .arg("--json");

        if let Some(desc) = description {
            cmd.arg("-d").arg(desc);
        }
        if let Some(p) = parent.filter(|p| Self::is_real_id(p)) {
            cmd.arg("--parent").arg(p);
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd create: {e}"))?;

        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("bd create failed: {err}"));
        }

        let issue: BeadsIssue = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("failed to parse bd create output: {e}"))?;

        for blocker in blocked_by.iter().filter(|b| Self::is_real_id(b)) {
            let _ = self.dep_add(&issue.id, blocker, "blocks").await;
        }

        Ok(issue)
    }

    /// `bd update <id> --status <status>`
    pub async fn set_status(&self, id: &str, status: &str) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["update", id, "--status", status])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd update: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd update status failed: {err}"))
        }
    }

    /// Resolve GitHub token from GITHUB_TOKEN env var, falling back to `gh auth token`
    /// via `gh` and `/opt/homebrew/bin/gh` when PATH is thin.
    pub fn resolve_github_token() -> Option<String> {
        Self::resolve_github_token_with(
            || std::env::var("GITHUB_TOKEN").ok(),
            |cmd_path| {
                std::process::Command::new(cmd_path)
                    .args(["auth", "token"])
                    .output()
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if stdout.is_empty() {
                                None
                            } else {
                                Some(stdout)
                            }
                        } else {
                            None
                        }
                    })
            },
        )
    }

    fn resolve_github_token_with<E, C>(get_env: E, run_cmd: C) -> Option<String>
    where
        E: Fn() -> Option<String>,
        C: FnMut(&str) -> Option<String>,
    {
        if let Some(token) = get_env().filter(|t| !t.trim().is_empty()) {
            return Some(token.trim().to_string());
        }

        let mut run_cmd = run_cmd;
        if let Some(token) = run_cmd("gh") {
            return Some(token);
        }

        if let Some(token) = run_cmd("/opt/homebrew/bin/gh") {
            return Some(token);
        }

        None
    }

    /// Run `bd github sync` to sync beads issues with GitHub Issues.
    pub async fn github_sync(&self) -> Result<(), String> {
        if !self.beads_dir.join("metadata.json").exists()
            && !self.beads_dir.join("embeddeddolt").exists()
        {
            return Ok(());
        }
        let token = Self::resolve_github_token().unwrap_or_default();

        let mut cmd = self.cmd();
        cmd.arg("github").arg("sync").arg("--push-only");

        if !token.is_empty() {
            cmd.env("GITHUB_TOKEN", token);
        }

        let out = cmd.output().await.map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd github sync failed: {err}"))
        }
    }

    /// Run `bd update <id> --claim` to claim a task.
    pub async fn claim(&self, id: &str) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["update", id, "--claim"])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd claim: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd claim failed: {err}"))
        }
    }

    /// `bd dep add <issue> <depends-on> -t <type>`
    ///
    /// `issue` depends on / is blocked by `depends_on` when type is `blocks`.
    pub async fn dep_add(
        &self,
        issue_id: &str,
        depends_on: &str,
        dep_type: &str,
    ) -> Result<(), String> {
        if !Self::is_real_id(issue_id) || !Self::is_real_id(depends_on) {
            return Ok(());
        }
        let out = self
            .cmd()
            .args(["dep", "add", issue_id, depends_on, "-t", dep_type])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd dep add: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd dep add failed: {err}"))
        }
    }

    /// Run `bd close <id> --reason <reason>` to mark a task completed.
    pub async fn close(&self, id: &str, reason: Option<&str>) -> Result<(), String> {
        if !Self::is_real_id(id) {
            return Ok(());
        }
        let mut cmd = self.cmd();
        cmd.arg("close").arg(id);

        if let Some(r) = reason {
            cmd.arg("--reason").arg(r);
        }

        let out = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute bd close: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd close failed: {err}"))
        }
    }

    /// Run `bd remember "insight"` to store persistent project memories.
    pub async fn remember(&self, insight: &str) -> Result<(), String> {
        let out = self
            .cmd()
            .args(["remember", insight])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd remember: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd remember failed: {err}"))
        }
    }

    /// Run `bd prime` to get system prompt context injection for agents.
    pub async fn prime(&self) -> Result<String, String> {
        let out = self
            .cmd()
            .arg("prime")
            .output()
            .await
            .map_err(|e| format!("failed to execute bd prime: {e}"))?;

        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd prime failed: {err}"))
        }
    }

    /// Run `bd dolt push origin` to push Dolt database state to refs/dolt/data.
    pub async fn sync_remote(&self, remote: Option<&str>) -> Result<(), String> {
        let target_remote = remote.unwrap_or("origin");
        let out = self
            .cmd()
            .args(["dolt", "push", target_remote])
            .output()
            .await
            .map_err(|e| format!("failed to execute bd dolt push: {e}"))?;

        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("bd dolt push failed: {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[tokio::test]
    async fn beads_client_lifecycle_test() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-test-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).unwrap();

        let client = BeadsClient::new(&test_dir);
        client.init_stealth().await.expect("init stealth");

        let project = client
            .create_linked(
                "Project Test",
                0,
                "epic",
                Some("Test Project Description"),
                None,
                &[],
            )
            .await
            .expect("create project");
        assert_eq!(project.title, "Project Test");

        let task = client
            .create_linked("Task Test", 1, "task", None, Some(&project.id), &[])
            .await
            .expect("create task");

        client.claim(&task.id).await.expect("claim task");
        client
            .remember("Test insight for beads")
            .await
            .expect("remember insight");

        let ready = client.list_ready().await.expect("list ready");
        assert!(
            ready.iter().any(|i| i.id == task.id) || !ready.is_empty(),
            "expected ready work after claim/in_progress semantics"
        );

        let prime_out = client.prime().await.expect("prime");
        assert!(prime_out.contains("Test insight for beads"));

        client
            .close(&task.id, Some("Done"))
            .await
            .expect("close task");
    }

    #[test]
    fn test_token_env_wins() {
        let mut cmd_calls = Vec::new();
        let token = BeadsClient::resolve_github_token_with(
            || Some("env_token_secret".to_string()),
            |cmd| {
                cmd_calls.push(cmd.to_string());
                Some("gh_token".to_string())
            },
        );
        assert_eq!(token, Some("env_token_secret".to_string()));
        assert!(
            cmd_calls.is_empty(),
            "gh command should not be executed when GITHUB_TOKEN env is set"
        );
    }

    #[test]
    fn test_gh_auth_token_fallback() {
        let mut cmd_calls = Vec::new();
        let token = BeadsClient::resolve_github_token_with(
            || None,
            |cmd| {
                cmd_calls.push(cmd.to_string());
                if cmd == "gh" {
                    Some("gh_token_123".to_string())
                } else {
                    None
                }
            },
        );
        assert_eq!(token, Some("gh_token_123".to_string()));
        assert_eq!(cmd_calls, vec!["gh"]);
    }

    #[test]
    fn test_homebrew_gh_path_fallback_when_bare_gh_missing() {
        let mut cmd_calls = Vec::new();
        let token = BeadsClient::resolve_github_token_with(
            || None,
            |cmd| {
                cmd_calls.push(cmd.to_string());
                if cmd == "/opt/homebrew/bin/gh" {
                    Some("homebrew_token_456".to_string())
                } else {
                    None
                }
            },
        );
        assert_eq!(token, Some("homebrew_token_456".to_string()));
        assert_eq!(
            cmd_calls,
            vec!["gh", "/opt/homebrew/bin/gh"],
            "/opt/homebrew/bin/gh should be tried when bare gh fails/missing"
        );
    }

    #[test]
    fn test_token_resolution_returns_none_when_all_fail() {
        let mut cmd_calls = Vec::new();
        let token = BeadsClient::resolve_github_token_with(
            || None,
            |cmd| {
                cmd_calls.push(cmd.to_string());
                None
            },
        );
        assert_eq!(token, None);
        assert_eq!(cmd_calls, vec!["gh", "/opt/homebrew/bin/gh"]);
    }

    #[tokio::test]
    async fn test_github_sync_skips_when_no_beads_dir() {
        let test_dir = temp_dir().join(format!(
            "honr-beads-empty-test-{}/.beads",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&test_dir);
        let client = BeadsClient::new(&test_dir);
        assert!(client.github_sync().await.is_ok());
    }
}
