# Plan: Archive / Retire Projects from the UI

## Context & Objectives

In `honr`, operators can soft-archive (retire) items using `cut_scope` via the backend store method `Board::cut_scope`, the REST endpoint `POST /api/items/{id}/cut`, or the MCP `cut_scope` tool. When an item (such as a Project) is soft-archived via `cut_scope`, its entire subtree of child tasks is transitioned to `State::Retired`.

However, the Web UI currently lacks a soft-archive / retire control in the detail drawer (`web/src/components/Detail.tsx`). While hard-deleting an item permanently removes it from the board, soft-archiving preserves historical facts and decisions ("we chose not to") in a greyed/retired state.

The objective of **«Archive / retire Projects from the UI»** is to:
1. Provide a soft-archive action button and confirmation prompt in the Web UI detail drawer for Projects (and Tasks), calling `api.cut(id, reason)`.
2. Apply consistent, scannable visual styling (`Retired` badge, greyed-out cards, story event log) across the Web UI for archived Projects and their subtrees.
3. Ensure backend `cut_scope` handling in `Board::cut_scope` and `supervisor` cleanly releases leases, stops active agent dispatches, and cleans up sandboxes for all items in the retired subtree.

---

## Standing Constraints & Architectural Changes

### 1. Web UI Drawer Action (`web/src/components/Detail.tsx`, `web/src/api.ts`)
- In `web/src/components/Detail.tsx` (Card Drawer): add a soft-archive action button ("Archive Project" / "Soft-archive") available for non-terminal Projects (and Tasks).
- Prompt the operator for an optional reason note ("Why is this project being retired?").
- On confirmation, call `api.cut(id, reason)` (`POST /api/items/{id}/cut`), updating the board snapshot dynamically upon completion.

### 2. Visual Styling & Scannability (`web/src/components/Board.tsx`, `web/src/components/Home.tsx`, `web/src/components/Detail.tsx`)
- Render `Retired` Projects and child tasks with muted/greyed-out visual treatment.
- Display an "Archived" / "Retired" badge or icon indicator on soft-archived cards in project lists and overview views.
- Render the narrative event line in the item's history log ("Scope cut: <Title> retired (<N> items)").

### 3. Store & Supervisor Subtree Lifecycle (`src/store.rs`, `src/supervisor.rs`)
- In `Board::cut_scope`: verify that transitioning a Project to `State::Retired` recursively transitions all non-terminal descendants to `State::Retired`.
- In `supervisor`: ensure active leases and sandbox environments associated with items in the retired subtree are stopped and cleaned up (`os.delete`).

---

## Tasks & Dependencies

```
[Task 1: Drawer Soft-Archive Action] ──┬──► [Task 2: UI Visual Styling & Badges]
                                      └──► [Task 3: Backend & Subtree Lifecycle Tests]
```

### Task 1: Web UI Drawer: Add Soft-Archive / Retire action to DetailDrawer
- **Description**: Add a soft-archive / retire button and confirmation prompt in `web/src/components/Detail.tsx` for non-terminal Projects (and Tasks). Wire the action to call `api.cut(id, reason)` from `web/src/api.ts` and refresh the board snapshot.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; Web UI builds cleanly (`npm run build` or `vite build` in `web/`); component/unit tests in `web/` verify that clicking the archive button triggers `api.cut` and updates the board item state.

### Task 2: Web UI Board & Drawer: Visual styling, badges, and filtering for soft-archived Projects
- **Description**: Update `web/src/components/Home.tsx`, `web/src/components/Board.tsx`, and `web/src/components/Detail.tsx` to render `Retired` projects and child tasks with muted/greyed-out styling, an "Archived" / "Retired" badge, and history log entries.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; Web UI builds cleanly; UI rendering tests verify that `Retired` items display greyed-out styling and "Archived" badges.

### Task 3: Backend & Integration: Verify cut_scope lease release, sandbox cleanup, and subtree retirement
- **Description**: Ensure `Board::cut_scope` in `src/store.rs` and `supervisor` in `src/supervisor.rs` release active leases, stop agent processes, and delete sandboxes for all items in a soft-archived Project subtree. Add unit tests in `src/store.rs` and `src/api.rs` verifying subtree retirement and cleanup.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; unit tests in `src/store.rs` verify that soft-archiving a Project transitions all non-terminal descendants to `Retired` and clears active leases and environments.
