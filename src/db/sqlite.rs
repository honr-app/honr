//! SQLite `BoardStore` — row-level board persistence and one-shot JSON import.

use super::codec::{
    item_from_row, item_to_row, parent_first, META_DEFAULT_SANDBOX_PROFILE_ID, META_JSON_IMPORTED,
    META_NEXT_ID, META_SANDBOX_PROFILES, META_WORKSPACE_BINDING,
};
use super::config::DatabaseBackend;
use super::store::{BoardStore, StoreError};
use super::{connect_sqlite_migrated, parse_database_url};
use crate::model::{ItemId, SandboxProfile, WorkItem, WorkspaceBinding};
use crate::store::{BoardState, StoryLine};
use async_trait::async_trait;
use chrono::Utc;
use sqlx::sqlite::SqlitePool;
use sqlx::{Sqlite, Transaction};
use std::collections::BTreeMap;
use std::path::Path;

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
        // Schema uses FKs; SQLite leaves them off unless asked.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .map_err(|e| StoreError::Connect(e.to_string()))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Load durable board rows into an in-memory `BoardState` (no agent_logs).
    pub async fn load_board_state(&self) -> Result<BoardState, StoreError> {
        let next_id = self.get_next_id().await?;
        let mut items_list = self.load_all_items().await?;
        // Attach blockers from the edge table.
        for item in &mut items_list {
            item.blocked_by = self.load_blockers(item.id).await?;
        }
        let mut items = BTreeMap::new();
        for item in items_list {
            items.insert(item.id, item);
        }
        let stories = self.load_all_stories().await?;
        let sandbox_profiles = self.load_sandbox_profiles().await?;
        let default_sandbox_profile_id = self.load_default_sandbox_profile_id().await?;
        let workspace = self.load_workspace_binding().await?;
        Ok(BoardState {
            next_id,
            items,
            stories,
            sandbox_profiles,
            default_sandbox_profile_id,
            workspace,
            agent_logs: BTreeMap::new(),
        })
    }

    /// Replace durable rows with the in-memory snapshot (agent_logs stay in-process).
    pub async fn save_board_state(&self, state: &BoardState) -> Result<(), StoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;

        sqlx::query("DELETE FROM item_blockers")
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        sqlx::query("DELETE FROM stories")
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        sqlx::query("DELETE FROM items")
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;

        // Items first, blockers second: `blocked_by` can point at a sibling
        // with a higher id (or any non-ancestor), so writing edges in the same
        // pass as rows trips SQLite FOREIGN KEY (787).
        let items: Vec<WorkItem> = state.items.values().cloned().collect();
        for item in parent_first(&items) {
            upsert_item_tx(&mut tx, item).await?;
        }
        for item in &items {
            replace_blockers_tx(&mut tx, item.id, &item.blocked_by).await?;
        }

        for (&goal_id, lines) in &state.stories {
            // Drop story lines whose Project was deleted — otherwise INSERT
            // trips FOREIGN KEY (787) and the whole board fails to boot.
            if !state.items.contains_key(&goal_id) {
                tracing::warn!(
                    goal_id,
                    lines = lines.len(),
                    "skipping orphan stories (no matching item)"
                );
                continue;
            }
            replace_stories_tx(&mut tx, goal_id, lines).await?;
        }

        set_meta_tx(&mut tx, META_NEXT_ID, &state.next_id.to_string()).await?;
        let profiles_json = serde_json::to_string(&state.sandbox_profiles)
            .map_err(|e| StoreError::Query(format!("serialize sandbox_profiles: {e}")))?;
        set_meta_tx(&mut tx, META_SANDBOX_PROFILES, &profiles_json).await?;
        set_meta_tx(
            &mut tx,
            META_DEFAULT_SANDBOX_PROFILE_ID,
            state.default_sandbox_profile_id.as_deref().unwrap_or(""),
        )
        .await?;
        let workspace_json = match &state.workspace {
            None => String::new(),
            Some(ws) => serde_json::to_string(ws)
                .map_err(|e| StoreError::Query(format!("serialize workspace_binding: {e}")))?,
        };
        set_meta_tx(&mut tx, META_WORKSPACE_BINDING, &workspace_json).await?;

        tx.commit()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    /// When the DB has never been populated, import `honr.json` once and stamp meta.
    /// Returns `true` if an import ran. Leaves the JSON file untouched.
    pub async fn import_json_if_empty(&self, json_path: &Path) -> Result<bool, StoreError> {
        if !self.is_empty().await? {
            return Ok(false);
        }
        let raw = match std::fs::read_to_string(json_path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(StoreError::Query(format!(
                    "read {}: {e}",
                    json_path.display()
                )))
            }
        };
        let state: BoardState = serde_json::from_str(&raw).map_err(|e| {
            StoreError::Query(format!("parse {}: {e}", json_path.display()))
        })?;
        self.save_board_state(&state).await?;
        self.meta_set(META_JSON_IMPORTED, &Utc::now().to_rfc3339())
            .await?;
        Ok(true)
    }

    async fn load_sandbox_profiles(
        &self,
    ) -> Result<BTreeMap<String, SandboxProfile>, StoreError> {
        match self.meta_get(META_SANDBOX_PROFILES).await? {
            None => Ok(BTreeMap::new()),
            Some(raw) if raw.is_empty() || raw == "{}" => Ok(BTreeMap::new()),
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|e| StoreError::Query(format!("decode sandbox_profiles: {e}"))),
        }
    }

    async fn load_default_sandbox_profile_id(&self) -> Result<Option<String>, StoreError> {
        Ok(self
            .meta_get(META_DEFAULT_SANDBOX_PROFILE_ID)
            .await?
            .filter(|s| !s.is_empty()))
    }

    async fn load_workspace_binding(&self) -> Result<Option<WorkspaceBinding>, StoreError> {
        match self.meta_get(META_WORKSPACE_BINDING).await? {
            None => Ok(None),
            Some(raw) if raw.trim().is_empty() || raw == "null" => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|e| StoreError::Query(format!("decode workspace_binding: {e}"))),
        }
    }
}

async fn set_meta_tx(
    tx: &mut Transaction<'_, Sqlite>,
    key: &str,
    value: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        r#"
        INSERT INTO meta (key, value) VALUES (?, ?)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(&mut **tx)
    .await
    .map_err(|e| StoreError::Query(e.to_string()))?;
    Ok(())
}

async fn upsert_item_tx(
    tx: &mut Transaction<'_, Sqlite>,
    item: &WorkItem,
) -> Result<(), StoreError> {
    let row = item_to_row(item)?;
    sqlx::query(
        r#"
        INSERT INTO items (
            id, parent_id, level, title, intent, definition_of_done, state,
            above_line, capability, run_deadline_at, parked, awaiting_dispatch,
            rebase_requested, entered_state_at, created_at,
            origin_json, lease_json, escalation_json, gates_json, notes_json,
            history_json, plan_json, proposal_json, extras_json
        ) VALUES (
            ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?, ?
        )
        ON CONFLICT(id) DO UPDATE SET
            parent_id = excluded.parent_id,
            level = excluded.level,
            title = excluded.title,
            intent = excluded.intent,
            definition_of_done = excluded.definition_of_done,
            state = excluded.state,
            above_line = excluded.above_line,
            capability = excluded.capability,
            run_deadline_at = excluded.run_deadline_at,
            parked = excluded.parked,
            awaiting_dispatch = excluded.awaiting_dispatch,
            rebase_requested = excluded.rebase_requested,
            entered_state_at = excluded.entered_state_at,
            created_at = excluded.created_at,
            origin_json = excluded.origin_json,
            lease_json = excluded.lease_json,
            escalation_json = excluded.escalation_json,
            gates_json = excluded.gates_json,
            notes_json = excluded.notes_json,
            history_json = excluded.history_json,
            plan_json = excluded.plan_json,
            proposal_json = excluded.proposal_json,
            extras_json = excluded.extras_json
        "#,
    )
    .bind(row.id as i64)
    .bind(row.parent_id.map(|p| p as i64))
    .bind(row.level)
    .bind(row.title)
    .bind(row.intent)
    .bind(row.definition_of_done)
    .bind(&row.state)
    .bind(row.above_line as i64)
    .bind(row.capability)
    .bind(row.run_deadline_at.as_deref())
    .bind(row.parked as i64)
    .bind(row.awaiting_dispatch as i64)
    .bind(row.rebase_requested as i64)
    .bind(&row.entered_state_at)
    .bind(&row.created_at)
    .bind(&row.origin_json)
    .bind(row.lease_json.as_deref())
    .bind(row.escalation_json.as_deref())
    .bind(&row.gates_json)
    .bind(&row.notes_json)
    .bind(&row.history_json)
    .bind(row.plan_json.as_deref())
    .bind(row.proposal_json.as_deref())
    .bind(&row.extras_json)
    .execute(&mut **tx)
    .await
    .map_err(|e| StoreError::Query(e.to_string()))?;
    Ok(())
}

async fn replace_blockers_tx(
    tx: &mut Transaction<'_, Sqlite>,
    item_id: ItemId,
    blocker_ids: &[ItemId],
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM item_blockers WHERE item_id = ?")
        .bind(item_id as i64)
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
    for &bid in blocker_ids {
        sqlx::query(
            "INSERT INTO item_blockers (item_id, blocker_id) VALUES (?, ?)",
        )
        .bind(item_id as i64)
        .bind(bid as i64)
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
    }
    Ok(())
}

async fn replace_stories_tx(
    tx: &mut Transaction<'_, Sqlite>,
    goal_id: ItemId,
    lines: &[StoryLine],
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM stories WHERE goal_id = ?")
        .bind(goal_id as i64)
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
    for (pos, line) in lines.iter().enumerate() {
        sqlx::query(
            "INSERT INTO stories (goal_id, position, at, text) VALUES (?, ?, ?, ?)",
        )
        .bind(goal_id as i64)
        .bind(pos as i64)
        .bind(line.at.to_rfc3339())
        .bind(&line.text)
        .execute(&mut **tx)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
    }
    Ok(())
}

#[async_trait]
impl BoardStore for SqliteBoardStore {
    async fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(row.map(|(v,)| v))
    }

    async fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO meta (key, value) VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_next_id(&self) -> Result<ItemId, StoreError> {
        match self.meta_get(META_NEXT_ID).await? {
            Some(v) => v
                .parse::<ItemId>()
                .map_err(|e| StoreError::Query(format!("next_id parse: {e}"))),
            None => Ok(1),
        }
    }

    async fn set_next_id(&self, next_id: ItemId) -> Result<(), StoreError> {
        self.meta_set(META_NEXT_ID, &next_id.to_string()).await
    }

    async fn is_empty(&self) -> Result<bool, StoreError> {
        if self.meta_get(META_JSON_IMPORTED).await?.is_some() {
            return Ok(false);
        }
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM items")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(count == 0)
    }

    async fn upsert_item(&self, item: &WorkItem) -> Result<(), StoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        upsert_item_tx(&mut tx, item).await?;
        replace_blockers_tx(&mut tx, item.id, &item.blocked_by).await?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    async fn delete_item(&self, id: ItemId) -> Result<(), StoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        sqlx::query("DELETE FROM item_blockers WHERE item_id = ? OR blocker_id = ?")
            .bind(id as i64)
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        sqlx::query("DELETE FROM stories WHERE goal_id = ?")
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        sqlx::query("DELETE FROM items WHERE id = ?")
            .bind(id as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_item(&self, id: ItemId) -> Result<Option<WorkItem>, StoreError> {
        let row = sqlx::query("SELECT * FROM items WHERE id = ?")
            .bind(id as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut item = item_from_row(&row)?;
        item.blocked_by = self.load_blockers(id).await?;
        Ok(Some(item))
    }

    async fn load_all_items(&self) -> Result<Vec<WorkItem>, StoreError> {
        let rows = sqlx::query("SELECT * FROM items ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(item_from_row(&row)?);
        }
        Ok(out)
    }

    async fn replace_blockers(
        &self,
        item_id: ItemId,
        blocker_ids: &[ItemId],
    ) -> Result<(), StoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        replace_blockers_tx(&mut tx, item_id, blocker_ids).await?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    async fn load_blockers(&self, item_id: ItemId) -> Result<Vec<ItemId>, StoreError> {
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT blocker_id FROM item_blockers WHERE item_id = ? ORDER BY blocker_id")
                .bind(item_id as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|(id,)| id as ItemId).collect())
    }

    async fn replace_stories(
        &self,
        goal_id: ItemId,
        lines: &[StoryLine],
    ) -> Result<(), StoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        replace_stories_tx(&mut tx, goal_id, lines).await?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Query(e.to_string()))?;
        Ok(())
    }

    async fn load_stories(&self, goal_id: ItemId) -> Result<Vec<StoryLine>, StoreError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT at, text FROM stories WHERE goal_id = ? ORDER BY position",
        )
        .bind(goal_id as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for (at, text) in rows {
            let at = chrono::DateTime::parse_from_rfc3339(&at)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| StoreError::Query(format!("story at: {e}")))?;
            out.push(StoryLine { at, text });
        }
        Ok(out)
    }

    async fn load_all_stories(&self) -> Result<BTreeMap<ItemId, Vec<StoryLine>>, StoreError> {
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT goal_id, position, at, text FROM stories ORDER BY goal_id, position",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Query(e.to_string()))?;
        let mut map: BTreeMap<ItemId, Vec<StoryLine>> = BTreeMap::new();
        for (goal_id, _pos, at, text) in rows {
            let at = chrono::DateTime::parse_from_rfc3339(&at)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| StoreError::Query(format!("story at: {e}")))?;
            map.entry(goal_id as ItemId)
                .or_default()
                .push(StoryLine { at, text });
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BoardStore;
    use crate::model::{Origin, State};
    use chrono::Utc;
    use std::sync::Arc;

    async fn mem_store() -> SqliteBoardStore {
        SqliteBoardStore::connect("sqlite::memory:")
            .await
            .expect("connect")
    }

    #[tokio::test]
    async fn round_trip_item_blockers_and_stories() {
        let store = mem_store().await;
        let mut parent = WorkItem::new(1, "Project", "why");
        parent.level = Some("Project".into());
        parent.state = State::Backlog;
        parent.project_prompt = Some("standing".into());
        parent.beads_id = Some("honr-abc".into());
        parent.sandbox_profile_id = Some("default".into());

        let mut child = WorkItem::new(2, "Task", "do it");
        child.parent = Some(1);
        child.level = Some("Task".into());
        child.state = State::Backlog;
        child.blocked_by = vec![1];
        child.awaiting_dispatch = true;
        child.definition_of_done = Some("shipped".into());

        store.upsert_item(&parent).await.expect("parent");
        store.upsert_item(&child).await.expect("child");
        store.set_next_id(3).await.expect("next_id");

        let line = StoryLine {
            at: Utc::now(),
            text: "kicked off".into(),
        };
        store
            .replace_stories(1, std::slice::from_ref(&line))
            .await
            .expect("stories");

        let loaded = store.get_item(2).await.expect("get").expect("exists");
        assert_eq!(loaded.title, "Task");
        assert_eq!(loaded.parent, Some(1));
        assert_eq!(loaded.blocked_by, vec![1]);
        assert!(loaded.awaiting_dispatch);
        assert_eq!(loaded.definition_of_done.as_deref(), Some("shipped"));

        let p = store.get_item(1).await.expect("get p").expect("p");
        assert_eq!(p.project_prompt.as_deref(), Some("standing"));
        assert_eq!(p.beads_id.as_deref(), Some("honr-abc"));
        assert_eq!(p.sandbox_profile_id.as_deref(), Some("default"));

        let stories = store.load_stories(1).await.expect("stories");
        assert_eq!(stories.len(), 1);
        assert_eq!(stories[0].text, "kicked off");
        assert_eq!(store.get_next_id().await.unwrap(), 3);

        // Full snapshot round-trip including sandbox profile catalog.
        let mut state = store.load_board_state().await.expect("load state");
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.next_id, 3);
        state.sandbox_profiles.insert(
            "default".into(),
            SandboxProfile {
                id: "default".into(),
                name: "Default".into(),
                image: "img:1".into(),
                policy: "version: 1\n# sqlite-roundtrip\n".into(),
                cpu: Some("2".into()),
                memory: None,
            },
        );
        state.default_sandbox_profile_id = Some("default".into());
        state.workspace = Some(crate::model::WorkspaceBinding {
            forge: "github".into(),
            upstream: "acme/widgets".into(),
            fork: "bot/widgets".into(),
            base: "develop".into(),
            beads_sync_repo: Some("acme/beads-mirror".into()),
        });
        store.save_board_state(&state).await.expect("save");
        let again = store.load_board_state().await.expect("reload");
        assert_eq!(again.items.get(&2).unwrap().blocked_by, vec![1]);
        assert_eq!(again.stories.get(&1).unwrap()[0].text, "kicked off");
        assert_eq!(
            again.items.get(&1).unwrap().sandbox_profile_id.as_deref(),
            Some("default")
        );
        assert_eq!(again.default_sandbox_profile_id.as_deref(), Some("default"));
        assert_eq!(again.sandbox_profiles.get("default").unwrap().image, "img:1");
        assert!(
            again
                .sandbox_profiles
                .get("default")
                .unwrap()
                .policy
                .contains("sqlite-roundtrip"),
            "policy YAML must round-trip"
        );
        let ws = again.workspace.expect("workspace round-trip");
        assert_eq!(ws.upstream, "acme/widgets");
        assert_eq!(ws.fork, "bot/widgets");
        assert_eq!(ws.base, "develop");
        assert_eq!(ws.beads_sync_repo.as_deref(), Some("acme/beads-mirror"));
    }

    #[tokio::test]
    async fn save_board_state_allows_sibling_blocker_with_higher_id() {
        let store = mem_store().await;
        let mut parent = WorkItem::new(1, "Project", "why");
        parent.level = Some("Project".into());
        parent.state = State::Backlog;

        let mut early = WorkItem::new(2, "Early", "waits on later sibling");
        early.parent = Some(1);
        early.level = Some("Task".into());
        early.state = State::Backlog;
        early.blocked_by = vec![3];

        let mut later = WorkItem::new(3, "Later", "the blocker");
        later.parent = Some(1);
        later.level = Some("Task".into());
        later.state = State::Backlog;

        let mut items = BTreeMap::new();
        items.insert(1, parent);
        items.insert(2, early);
        items.insert(3, later);
        let state = BoardState {
            next_id: 4,
            items,
            ..Default::default()
        };
        store.save_board_state(&state).await.expect("save");
        let loaded = store.load_board_state().await.expect("load");
        assert_eq!(loaded.items.get(&2).unwrap().blocked_by, vec![3]);
    }

    #[tokio::test]
    async fn one_shot_json_import_and_no_repeat() {
        let dir = std::env::temp_dir().join(format!(
            "honr-import-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("honr.json");

        let mut state = BoardState {
            next_id: 5,
            ..Default::default()
        };
        let mut item = WorkItem::new(4, "Imported", "from json");
        item.origin = Origin::Human;
        item.state = State::Backlog;
        state.items.insert(4, item);
        state.stories.insert(
            4,
            vec![StoryLine {
                at: Utc::now(),
                text: "hello".into(),
            }],
        );
        std::fs::write(&json_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        let store = mem_store().await;
        assert!(store.is_empty().await.unwrap());
        assert!(store
            .import_json_if_empty(&json_path)
            .await
            .expect("import"));
        assert!(!store.is_empty().await.unwrap());

        let loaded = store.load_board_state().await.expect("load");
        assert_eq!(loaded.next_id, 5);
        assert_eq!(loaded.items.get(&4).unwrap().title, "Imported");
        assert_eq!(loaded.stories.get(&4).unwrap()[0].text, "hello");

        // Second boot: stamp present — no re-import even if we wipe items in JSON.
        std::fs::write(&json_path, "{}").unwrap();
        assert!(!store
            .import_json_if_empty(&json_path)
            .await
            .expect("skip"));
        assert_eq!(
            store.get_item(4).await.unwrap().unwrap().title,
            "Imported"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn board_survives_restart_via_db() {
        let dir = std::env::temp_dir().join(format!(
            "honr-board-db-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("honr.db");
        let json_path = dir.join("honr.json");
        let url = format!("sqlite:{}", db_path.display());

        let store = Arc::new(
            SqliteBoardStore::connect(&url)
                .await
                .expect("connect file db"),
        );
        let schema = crate::schema::Schema::default();
        let board = crate::store::Board::load_with_store(
            schema.clone(),
            json_path.clone(),
            store.clone(),
        )
        .await
        .expect("open empty");

        let project = board
            .create(
                None,
                "DB Project",
                "persist me",
                None,
                Origin::Human,
                true,
                None,
            )
            .expect("create project");
        board
            .transition(project.id, State::Backlog, "test", None)
            .ok();
        board.story(project.id, "noted".into());
        board.flush();

        // Drop in-memory board; reopen from the same DB file.
        drop(board);
        let store2 = Arc::new(
            SqliteBoardStore::connect(&url)
                .await
                .expect("reconnect"),
        );
        let board2 = crate::store::Board::load_with_store(
            schema,
            json_path.clone(),
            store2,
        )
        .await
        .expect("reopen");
        let restored = board2.get(project.id).expect("item survives");
        assert_eq!(restored.title, "DB Project");
        let stories = board2.stories_for(project.id);
        assert!(stories.iter().any(|s| s.text == "noted"));

        // Flush with a store attached must not create/rewrite honr.json.
        assert!(!json_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
