-- Denormalized columns for hot list / dispatch / lease / leaf filters (plan t3).
-- Maintained on write (Board flush / upsert). Query paths use these instead of
-- full-table scans for "is leaf" and "has unresolved blockers".

ALTER TABLE items ADD COLUMN non_retired_child_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN open_blocker_count INTEGER NOT NULL DEFAULT 0;

-- Backlog / ready / awaiting_dispatch: leaf + unblocked + dispatch flags.
CREATE INDEX idx_items_backlog_ready
    ON items(state, level, non_retired_child_count, open_blocker_count, capability);

-- Lease sweep: Claimed/Running by deadline.
CREATE INDEX idx_items_lease_sweep
    ON items(state, run_deadline_at);
