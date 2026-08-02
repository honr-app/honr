//! Deltas pushed to every connected board. The UI holds a map keyed by id and
//! merges upserts, so one event shape covers create, transition and heartbeat.

use crate::model::{ItemId, WorkItem};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BoardEvent {
    /// A work item was created or changed. Merge by `item.id`.
    Upsert { seq: u64, item: Box<WorkItem> },
    /// Narrative line appended to a goal's running story (§8). Humans chunk
    /// time into stories, not state transitions.
    Story { seq: u64, goal: ItemId, at: String, text: String },
    /// A work item was deleted. Remove by `id`.
    Delete { seq: u64, id: ItemId },
}

