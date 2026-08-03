//! Project + Task — the only two rungs.
//!
//! Project is the container; Tasks are flat claimable leaves under it. The
//! engine still reads the tree (parent edges) for containment; task↔task
//! ordering lives in dependency edges (blocks / relates-to), usually via beads.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Level {
    pub name: String,
    #[serde(default)]
    pub horizon: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub elaborate: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub claimable: bool,
}

/// How work actually gets executed. The run budget is
/// `agents.agent_timeout_secs`; `lease_secs` / `heartbeat_expect_secs` are
/// ignored leftovers kept so older `honr.yaml` files still parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Deprecated — ignored. Run deadline is `agents.agent_timeout_secs`.
    #[serde(default = "d_lease")]
    pub lease_secs: i64,
    /// Deprecated — ignored. UI shows countdown to `run_deadline_at`.
    #[serde(default = "d_hb")]
    pub heartbeat_expect_secs: i64,
    /// How often to check for overdue run deadlines.
    #[serde(default = "d_sweep")]
    pub sweep_interval_ms: u64,
    /// Real agents in real sandboxes. Off by default: the board must still run
    /// on a machine with no podman, no gateway and no credentials.
    #[serde(default)]
    pub agents: AgentConfig,
}

fn d_lease() -> i64 { 600 }
fn d_hb() -> i64 { 6 }
fn d_sweep() -> u64 { 2000 }

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            lease_secs: d_lease(),
            heartbeat_expect_secs: d_hb(),
            sweep_interval_ms: d_sweep(),
            agents: AgentConfig::default(),
        }
    }
}

/// Where code comes from and where it goes back.
///
/// The agent clones the **fork** and opens a PR against **upstream**. The bot
/// account has no write access to upstream, so a cross-fork PR is its only
/// route in — the trust boundary is GitHub's, not ours to enforce.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    /// `owner/name` that PRs target.
    pub upstream: String,
    /// `owner/name` the agent clones and pushes to.
    pub fork: String,
    #[serde(default = "d_base")]
    pub base: String,
}

fn d_base() -> String { "main".into() }

impl Default for RepoConfig {
    fn default() -> Self {
        Self { upstream: String::new(), fork: String::new(), base: d_base() }
    }
}

/// Vertex settings passed into the sandbox. Values that are wrong here fail as
/// a hang, so they are configuration rather than constants — see
/// `docs/sandbox-stack.md` for which combination actually works.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexConfig {
    pub project: String,
    #[serde(default = "d_location")]
    pub location: String,
    #[serde(default = "d_model")]
    pub model: String,
}

fn d_location() -> String { "global".into() }
fn d_model() -> String { "claude-opus-5".into() }

impl Default for VertexConfig {
    fn default() -> Self {
        Self { project: String::new(), location: d_location(), model: d_model() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Passed to `--from`. Needs a Rust toolchain to build honr; see
    /// `sandbox/Containerfile`.
    #[serde(default = "d_image")]
    pub image: String,
    #[serde(default = "d_policy")]
    pub policy: String,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub repo: RepoConfig,
    #[serde(default)]
    pub vertex: VertexConfig,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub memory: Option<String>,
    /// Sandboxes are heavy and this is alpha software. Do not start at seven.
    /// Primary agent CLI engine (`cursor`, `agy`, or `claude`).
    #[serde(default = "d_engine")]
    pub engine: String,
    #[serde(default = "d_concurrent")]
    pub max_concurrent: usize,
    /// Real money. Breaching this stops the sandbox rather than truncating
    /// the work silently. Optional — no budget limit by default.
    #[serde(default)]
    pub per_card_budget_cents: Option<u64>,
    #[serde(default)]
    pub daily_budget_cents: Option<u64>,
    /// Hard ceiling on one agent run. Everything here fails as a hang.
    #[serde(default = "d_agent_timeout")]
    pub agent_timeout_secs: u64,
    /// Runs that die without producing work before the card becomes a human's
    /// problem. The money caps do not cover this: an early failure spends
    /// nothing, so without a count it requeues forever.
    #[serde(default = "d_max_attempts")]
    pub max_attempts: u32,
}

fn d_image() -> String { "honr-sandbox:latest".into() }
fn d_policy() -> String { "sandbox/policy.yaml".into() }
fn d_engine() -> String { "cursor".into() }
fn d_concurrent() -> usize { 2 }
fn d_agent_timeout() -> u64 { 1800 }
fn d_max_attempts() -> u32 { 3 }

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            image: d_image(),
            policy: d_policy(),
            providers: Vec::new(),
            repo: RepoConfig::default(),
            vertex: VertexConfig::default(),
            cpu: None,
            memory: None,
            engine: d_engine(),
            max_concurrent: d_concurrent(),
            per_card_budget_cents: None,
            daily_budget_cents: None,
            agent_timeout_secs: d_agent_timeout(),
            max_attempts: d_max_attempts(),
        }
    }
}

impl AgentConfig {
    /// Refuse to run rather than half-run. Every one of these presents as a
    /// hang if it's wrong at exec time, so check it at startup instead.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.repo.upstream.is_empty() || self.repo.fork.is_empty() {
            return Err("execution.agents.repo needs both `upstream` and `fork`".into());
        }
        if self.vertex.project.is_empty() {
            return Err("execution.agents.vertex.project is required".into());
        }
        if self.providers.is_empty() {
            return Err("execution.agents.providers is empty; need at least a Vertex and a GitHub provider".into());
        }
        if !std::path::Path::new(&self.policy).exists() {
            return Err(format!("execution.agents.policy {:?} does not exist", self.policy));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub levels: Vec<Level>,
    #[serde(default)]
    pub execution: ExecutionConfig,
}

impl Schema {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())?;
        let schema: Schema = serde_yaml::from_str(&raw)?;
        schema.validate();
        Ok(schema)
    }



    /// Depth 0 → Project, depth ≥1 → Task (flat under Project). Extra depth
    /// collapses to Task so a mistaken nest still labels correctly.
    pub fn level_for_depth(&self, depth: usize) -> Option<&Level> {
        if self.levels.is_empty() {
            return None;
        }
        if depth == 0 {
            self.levels.first()
        } else {
            self.levels.iter().find(|l| l.claimable).or_else(|| self.levels.last())
        }
    }

    pub fn project_level(&self) -> Option<&Level> {
        self.levels.iter().find(|l| !l.claimable).or_else(|| self.levels.first())
    }

    pub fn task_level(&self) -> Option<&Level> {
        self.levels.iter().find(|l| l.claimable).or_else(|| self.levels.last())
    }

    /// A schema that declares a rung a human can't occupy should say so at
    /// configuration time, not at 2am. We can't know child counts here, so this
    /// only catches the structural mistakes.
    fn validate(&self) {
        if self.levels.is_empty() {
            tracing::warn!("level schema declares no levels; the tree will render unlabelled");
        }
        if self.levels.len() > 2 {
            tracing::warn!(
                count = self.levels.len(),
                "more than 2 levels: Plan A is Project + Task only; deeper ladders are retired"
            );
        }
        if !self.levels.iter().any(|l| l.claimable) {
            tracing::warn!("no level is marked claimable; agents will find nothing to pick up");
        }
        if !self.levels.iter().any(|l| !l.claimable) {
            tracing::warn!("no non-claimable Project level; every node would look claimable");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workable() -> AgentConfig {
        AgentConfig {
            enabled: true,
            providers: vec!["vertex".into(), "gh-clankr".into()],
            repo: RepoConfig {
                upstream: "shanemcd/honr".into(),
                fork: "clankrshq/honr".into(),
                base: "main".into(),
            },
            vertex: VertexConfig { project: "shanemcd-rh".into(), ..Default::default() },
            ..Default::default()
        }
    }

    /// Off by default. The board has to run on a machine with no podman, no
    /// gateway and no credentials.
    #[test]
    fn agents_are_off_unless_asked_for() {
        assert!(!AgentConfig::default().enabled);
        assert!(AgentConfig::default().validate().is_ok());
    }

    /// Every one of these presents as a hang if it's wrong at exec time, so
    /// they are checked at startup instead.
    #[test]
    fn enabling_agents_requires_a_complete_config() {
        assert!(workable().validate().is_ok(), "the reference config must pass");

        let mut no_fork = workable();
        no_fork.repo.fork = String::new();
        assert!(no_fork.validate().is_err());

        let mut no_project = workable();
        no_project.vertex.project = String::new();
        assert!(no_project.validate().is_err());

        let mut no_providers = workable();
        no_providers.providers.clear();
        assert!(no_providers.validate().is_err());

        let mut bad_policy = workable();
        bad_policy.policy = "sandbox/does-not-exist.yaml".into();
        assert!(bad_policy.validate().is_err());
    }

    /// The location and model defaults are load-bearing: us-east5 is
    /// quota-exhausted and us-central1 does not serve the model.
    #[test]
    fn vertex_defaults_match_what_actually_works() {
        let v = VertexConfig::default();
        assert_eq!(v.location, "global");
        assert_eq!(v.model, "claude-opus-5");
    }

    /// honr.yaml must parse into the config the code expects — the file is the
    /// real contract, not the struct.
    #[test]
    fn shipped_honr_yaml_parses() {
        let s = Schema::load("honr.yaml").expect("honr.yaml parses");
        assert!(!s.levels.is_empty(), "levels should be declared");
        assert!(s.levels.iter().any(|l| l.claimable), "something must be claimable");
        s.execution.agents.validate().expect("shipped agent config is valid");
    }
}
