//! `BoardStore` — durable board rows behind the in-process `Board` facade.
//!
//! Mutations still go through `Board` / `machine.rs`; transports must not grow
//! SQL. Hot list/snapshot/lease paths use indexed SQL and denormalized columns
//! (`non_retired_child_count`, `open_blocker_count`) plus in-process secondary
//! indexes on `BoardState`.

use crate::model::{ItemId, WorkItem};
use crate::store::StoryLine;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

use super::config::DatabaseBackend;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("connect: {0}")]
    Connect(String),
    #[error("migrate: {0}")]
    Migrate(String),
    #[error("query: {0}")]
    Query(String),
    #[error("database URL: {0}")]
    Url(#[from] super::config::ParseDatabaseUrlError),
    #[error("expected {expected} database URL, got {got}")]
    WrongBackend {
        expected: DatabaseBackend,
        got: DatabaseBackend,
    },
}

/// Persistence API for board items, blockers, stories, and meta.
#[async_trait]
pub trait BoardStore: Send + Sync {
    async fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError>;
    async fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError>;

    async fn get_next_id(&self) -> Result<ItemId, StoreError>;
    async fn set_next_id(&self, next_id: ItemId) -> Result<(), StoreError>;

    /// True when no items and no JSON-import stamp — gate for one-shot import.
    async fn is_empty(&self) -> Result<bool, StoreError>;

    async fn upsert_item(&self, item: &WorkItem) -> Result<(), StoreError>;
    async fn delete_item(&self, id: ItemId) -> Result<(), StoreError>;
    async fn get_item(&self, id: ItemId) -> Result<Option<WorkItem>, StoreError>;
    async fn load_all_items(&self) -> Result<Vec<WorkItem>, StoreError>;

    async fn replace_blockers(&self, item_id: ItemId, blocker_ids: &[ItemId])
        -> Result<(), StoreError>;
    async fn load_blockers(&self, item_id: ItemId) -> Result<Vec<ItemId>, StoreError>;

    async fn replace_stories(
        &self,
        goal_id: ItemId,
        lines: &[StoryLine],
    ) -> Result<(), StoreError>;
    async fn load_stories(&self, goal_id: ItemId) -> Result<Vec<StoryLine>, StoreError>;
    async fn load_all_stories(&self) -> Result<BTreeMap<ItemId, Vec<StoryLine>>, StoreError>;

    /// Indexed backlog leaves matching capabilities (same filters as `Board::list_backlog`).
    async fn query_backlog(&self, capabilities: &[String]) -> Result<Vec<WorkItem>, StoreError>;

    /// Indexed awaiting-dispatch queue, oldest `entered_state_at` first.
    async fn query_awaiting_dispatch(&self) -> Result<Vec<WorkItem>, StoreError>;

    /// Indexed lease sweep: Claimed/Running past `run_deadline_at` (or legacy lease expiry).
    async fn query_expired_leases(&self, now: DateTime<Utc>) -> Result<Vec<ItemId>, StoreError>;

    /// Children of `id` via `parent_id` index.
    async fn query_children_of(&self, id: ItemId) -> Result<Vec<ItemId>, StoreError>;

    /// True when a non-retired child exists (`non_retired_child_count > 0`).
    async fn query_has_non_retired_children(&self, id: ItemId) -> Result<bool, StoreError>;
}
