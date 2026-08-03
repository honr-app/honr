//! `BoardStore` — durable board rows behind the in-process `Board` facade.
//!
//! Mutations still go through `Board` / `machine.rs`; transports must not grow
//! SQL. Implementations (SQLite, later Postgres) land in subsequent Tasks —
//! this trait is the compile boundary they target.

#![allow(dead_code)]

use crate::model::{ItemId, WorkItem};
use crate::store::StoryLine;
use async_trait::async_trait;
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
    /// Method reserved for a later Task; trait surface is stable now.
    #[error("board store method not implemented yet: {0}")]
    NotYet(&'static str),
}

/// Persistence API for board items, blockers, stories, and meta.
///
/// Hot list/snapshot/lease paths will grow indexed query methods in a later
/// Task; the load/upsert surface here is enough for the SQLite cutover.
#[async_trait]
pub trait BoardStore: Send + Sync {
    async fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError>;
    async fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError>;

    async fn get_next_id(&self) -> Result<ItemId, StoreError>;
    async fn set_next_id(&self, next_id: ItemId) -> Result<(), StoreError>;

    /// True when no items (and no import stamp) — JSON import gate for Task 2.
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
}
