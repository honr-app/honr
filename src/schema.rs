//! Your ladder, not mine.
//!
//! Someone else writes Initiative / Outcome / Increment. Someone else deletes
//! three rungs and runs Goal -> Task. The engine never reads these names — it
//! reads the tree. The schema drives labels, default gates, and what the UI
//! asks a human to fill in.

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

/// How work actually gets executed. Timings here are control-plane facts, not
/// simulation knobs: the lease is what makes a dead agent survivable, and the
/// expected heartbeat interval is what the UI decays a card against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default = "d_lease")]
    pub lease_secs: i64,
    /// Expected heartbeat interval; cards decay visibly past this.
    #[serde(default = "d_hb")]
    pub heartbeat_expect_secs: i64,
    /// How often to check for expired leases.
    #[serde(default = "d_sweep")]
    pub sweep_interval_ms: u64,
}

fn d_lease() -> i64 { 45 }
fn d_hb() -> i64 { 6 }
fn d_sweep() -> u64 { 2000 }

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            lease_secs: d_lease(),
            heartbeat_expect_secs: d_hb(),
            sweep_interval_ms: d_sweep(),
        }
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



    /// The rung a node lands on given its depth in the tree. Machine-created
    /// depth below the line never adds a rung — it collapses into the deepest
    /// declared level, so a depth-6 branch and a depth-2 branch render the same
    /// to a human looking at the epic.
    pub fn level_for_depth(&self, depth: usize) -> Option<&Level> {
        if self.levels.is_empty() {
            return None;
        }
        self.levels.get(depth).or_else(|| self.levels.last())
    }

    /// A schema that declares a rung a human can't occupy should say so at
    /// configuration time, not at 2am. We can't know child counts here, so this
    /// only catches the structural mistakes.
    fn validate(&self) {
        if self.levels.is_empty() {
            tracing::warn!("level schema declares no levels; the tree will render unlabelled");
        }
        if self.levels.len() > 7 {
            tracing::warn!(
                count = self.levels.len(),
                "more than 7 rungs: a human occupies one rung at a time, and this many \
                 is likely more than anyone can hold"
            );
        }
        if !self.levels.iter().any(|l| l.claimable) {
            tracing::warn!("no level is marked claimable; agents will find nothing to pick up");
        }
    }
}
