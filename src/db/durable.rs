//! Backend-agnostic handle used by `Board` for SQLite (default) or Postgres.

use super::config::{parse_database_url, DatabaseBackend};
use super::postgres::PostgresBoardStore;
use super::sqlite::SqliteBoardStore;
use super::store::StoreError;
use crate::store::BoardState;
use std::path::Path;

/// Durable board row store — SQLite by default, Postgres when configured.
pub enum DurableBoardStore {
    Sqlite(SqliteBoardStore),
    Postgres(PostgresBoardStore),
}

impl DurableBoardStore {
    /// Open and migrate the store selected by a `sqlite:…` or `postgres://…` URL.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let parsed = parse_database_url(url)?;
        match parsed.backend() {
            DatabaseBackend::Sqlite => Ok(Self::Sqlite(SqliteBoardStore::connect(url).await?)),
            DatabaseBackend::Postgres => {
                Ok(Self::Postgres(PostgresBoardStore::connect(url).await?))
            }
        }
    }

    pub fn backend(&self) -> DatabaseBackend {
        match self {
            Self::Sqlite(_) => DatabaseBackend::Sqlite,
            Self::Postgres(_) => DatabaseBackend::Postgres,
        }
    }

    pub async fn load_board_state(&self) -> Result<BoardState, StoreError> {
        match self {
            Self::Sqlite(s) => s.load_board_state().await,
            Self::Postgres(s) => s.load_board_state().await,
        }
    }

    pub async fn save_board_state(&self, state: &BoardState) -> Result<(), StoreError> {
        match self {
            Self::Sqlite(s) => s.save_board_state(state).await,
            Self::Postgres(s) => s.save_board_state(state).await,
        }
    }

    pub async fn import_json_if_empty(&self, json_path: &Path) -> Result<bool, StoreError> {
        match self {
            Self::Sqlite(s) => s.import_json_if_empty(json_path).await,
            Self::Postgres(s) => s.import_json_if_empty(json_path).await,
        }
    }
}
