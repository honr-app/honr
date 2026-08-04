-- Board persistence schema (SQLite default; Postgres-compatible types).
-- Indexed columns match the hot query paths (column filters, parent, status).
-- Nested WorkItem fields that are not filter keys live in JSON text blobs.

CREATE TABLE meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE items (
    id INTEGER PRIMARY KEY NOT NULL,
    parent_id INTEGER,
    level TEXT,
    title TEXT NOT NULL,
    intent TEXT NOT NULL,
    definition_of_done TEXT,
    state TEXT NOT NULL,
    above_line INTEGER NOT NULL DEFAULT 0,
    capability TEXT,
    run_deadline_at TEXT,
    parked INTEGER NOT NULL DEFAULT 0,
    awaiting_dispatch INTEGER NOT NULL DEFAULT 0,
    rebase_requested INTEGER NOT NULL DEFAULT 0,
    entered_state_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- Nested / infrequently filtered fields as JSON text (portable across backends).
    origin_json TEXT NOT NULL,
    lease_json TEXT,
    escalation_json TEXT,
    gates_json TEXT NOT NULL DEFAULT '[]',
    notes_json TEXT NOT NULL DEFAULT '[]',
    history_json TEXT NOT NULL DEFAULT '[]',
    plan_json TEXT,
    proposal_json TEXT,
    extras_json TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY (parent_id) REFERENCES items(id)
);

CREATE INDEX idx_items_parent_id ON items(parent_id);
CREATE INDEX idx_items_state ON items(state);
CREATE INDEX idx_items_level ON items(level);
CREATE INDEX idx_items_capability ON items(capability);
CREATE INDEX idx_items_awaiting_dispatch ON items(awaiting_dispatch);
CREATE INDEX idx_items_parked ON items(parked);
CREATE INDEX idx_items_rebase_requested ON items(rebase_requested);
CREATE INDEX idx_items_run_deadline_at ON items(run_deadline_at);
CREATE INDEX idx_items_entered_state_at ON items(entered_state_at);
-- Hot dispatch/backlog filter: Backlog + awaiting_dispatch + not parked.
CREATE INDEX idx_items_dispatch_queue
    ON items(state, awaiting_dispatch, parked, entered_state_at);

CREATE TABLE item_blockers (
    item_id INTEGER NOT NULL,
    blocker_id INTEGER NOT NULL,
    PRIMARY KEY (item_id, blocker_id),
    FOREIGN KEY (item_id) REFERENCES items(id) ON DELETE CASCADE,
    FOREIGN KEY (blocker_id) REFERENCES items(id)
);

CREATE INDEX idx_item_blockers_blocker ON item_blockers(blocker_id);

CREATE TABLE stories (
    goal_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    at TEXT NOT NULL,
    text TEXT NOT NULL,
    PRIMARY KEY (goal_id, position),
    FOREIGN KEY (goal_id) REFERENCES items(id) ON DELETE CASCADE
);

CREATE INDEX idx_stories_goal_id ON stories(goal_id);
