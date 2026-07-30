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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetConfig {
    #[serde(default = "d_size")]
    pub size: usize,
    #[serde(default = "d_tick")]
    pub tick_ms: u64,
    #[serde(default = "d_lease")]
    pub lease_secs: i64,
    #[serde(default)]
    pub escalate_p: f64,
    #[serde(default)]
    pub split_p: f64,
    #[serde(default)]
    pub die_p: f64,
    #[serde(default)]
    pub gate_fail_p: f64,
    #[serde(default = "d_hb")]
    pub heartbeat_expect_secs: i64,
}

fn d_size() -> usize { 7 }
fn d_tick() -> u64 { 2000 }
fn d_lease() -> i64 { 45 }
fn d_hb() -> i64 { 6 }

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            size: d_size(),
            tick_ms: d_tick(),
            lease_secs: d_lease(),
            escalate_p: 0.04,
            split_p: 0.03,
            die_p: 0.01,
            gate_fail_p: 0.20,
            heartbeat_expect_secs: d_hb(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub levels: Vec<Level>,
    #[serde(default)]
    pub fleet: FleetConfig,
}

impl Default for Schema {
    fn default() -> Self {
        Self { levels: Vec::new(), fleet: FleetConfig::default() }
    }
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
