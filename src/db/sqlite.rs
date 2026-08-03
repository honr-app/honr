//! SQLite `BoardStore` shell.
//!
//! Opens a migrated pool so later Tasks can fill in row I/O without reshaping
//! the module boundary. Methods return [`StoreError::NotYet`] until Task 2.

#![allow(dead_code)]

use super::config::DatabaseBackend;
use super::store::{BoardStore, StoreError};
use super::{connect_sqlite_migrated, parse_database_url};
use crate::model::{ItemId, WorkItem};
use crate::store::StoryLine;
use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;
use std::collections::BTreeMap;

pub struct SqliteBoardStore {
    pool: SqlitePool,
}

impl SqliteBoardStore {
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let parsed = parse_database_url(url)?;
        if parsed.backend() != DatabaseBackend::Sqlite {
            return Err(StoreError::WrongBackend {
                expected: DatabaseBackend::Sqlite,
                got: parsed.backend(),
            });
        }
        let pool = connect_sqlite_migrated(parsed.as_str()).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

macro_rules! not_yet {
    ($name:literal) => {
        Err(StoreError::NotYet($name))
    };
}

#[async_trait]
impl BoardStore for SqliteBoardStore {
    async fn meta_get(&self, _key: &str) -> Result<Option<String>, StoreError> {
        not_yet!("meta_get")
    }
    async fn meta_set(&self, _key: &str, _value: &str) -> Result<(), StoreError> {
        not_yet!("meta_set")
    }
    async fn get_next_id(&self) -> Result<ItemId, StoreError> {
        not_yet!("get_next_id")
    }
    async fn set_next_id(&self, _next_id: ItemId) -> Result<(), StoreError> {
        not_yet!("set_next_id")
    }
    async fn is_empty(&self) -> Result<bool, StoreError> {
        not_yet!("is_empty")
    }
    async fn upsert_item(&self, _item: &WorkItem) -> Result<(), StoreError> {
        not_yet!("upsert_item")
    }
    async fn delete_item(&self, _id: ItemId) -> Result<(), StoreError> {
        not_yet!("delete_item")
    }
    async fn get_item(&self, _id: ItemId) -> Result<Option<WorkItem>, StoreError> {
        not_yet!("get_item")
    }
    async fn load_all_items(&self) -> Result<Vec<WorkItem>, StoreError> {
        not_yet!("load_all_items")
    }
    async fn replace_blockers(
        &self,
        _item_id: ItemId,
        _blocker_ids: &[ItemId],
    ) -> Result<(), StoreError> {
        not_yet!("replace_blockers")
    }
    async fn load_blockers(&self, _item_id: ItemId) -> Result<Vec<ItemId>, StoreError> {
        not_yet!("load_blockers")
    }
    async fn replace_stories(
        &self,
        _goal_id: ItemId,
        _lines: &[StoryLine],
    ) -> Result<(), StoreError> {
        not_yet!("replace_stories")
    }
    async fn load_stories(&self, _goal_id: ItemId) -> Result<Vec<StoryLine>, StoreError> {
        not_yet!("load_stories")
    }
    async fn load_all_stories(&self) -> Result<BTreeMap<ItemId, Vec<StoryLine>>, StoreError> {
        not_yet!("load_all_stories")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_store_connects_and_exposes_trait() {
        let store = SqliteBoardStore::connect("sqlite::memory:")
            .await
            .expect("connect");
        // Pool is live (migrations applied); row methods wait for Task 2.
        assert!(matches!(
            store.is_empty().await,
            Err(StoreError::NotYet("is_empty"))
        ));
        let _trait: &dyn BoardStore = &store;
        let _ = _trait;
    }
}
