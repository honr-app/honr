# Plan: Archive / Retire Projects from the UI

## Context & Objectives

Currently in `honr`, human operators viewing a Project (or card) in the drawer UI (`web/src/components/Detail.tsx`) only see a permanent delete action ("🗑 Delete") which triggers `Board::delete_item` (`DELETE /items/{id}`). This permanently removes the Project and its entire subtree from the board and Dolt issue store.

However, in many cases operators want to soft-archive a Project and its subtree without hard-deleting, matching MCP `cut_scope` and `Retired` state semantics (`Board::cut_scope`, `POST /items/{id}/cut`). Soft-archiving marks the Project and its child tasks as `State::Retired` ("we chose not to / scope cut"), preserving historical context and decision rationale while removing the Project from active workflows.

The objectives of **«Archive / retire Projects from the UI»** are:
1. Provide a prominent, safe "Archive Project" / "Cut Scope" action in the drawer UI (`web/src/components/Detail.tsx`) with a soft-archive confirmation modal, allowing operators to enter an optional retirement reason and execute `api.cut(id, reason)`.
2. Update the Web UI (`web/src/components/Home.tsx`, `web/src/components/Overview.tsx`, and `web/src/components/Board.tsx`) so soft-archived/retired Projects and their subtrees are visually distinguished (scope cut indicator, muted state) and properly filtered so archived projects don't clutter active project views while remaining accessible.
3. Verify and enhance backend semantics in `src/store.rs` and `src/api.rs` so that soft-archiving a Project automatically transitions the Project and all descendant tasks to `State::Retired`, records the scope cut story event, and returns clean state updates.

---

## Standing Constraints & Architectural Changes

### 1. API & Store (`src/store.rs`, `src/api.rs`)
- Verify that `Board::cut_scope` in `src/store.rs` recursively transitions all subtree items to `State::Retired`.
- Ensure `cut_scope` records a board story event ("Scope cut: ... retired") and emits SSE broadcast updates.
- Add backend unit tests in `src/store.rs` and `src/api.rs` testing `cut_scope` on a Project root with child tasks.

### 2. Web UI Drawer (`web/src/components/Detail.tsx`)
- Add a "Retire / Archive" action button in the drawer head / action panel for Projects and tasks.
- Show a soft-archive confirmation dialog explaining that the Project and its subtree will be soft-archived to `Retired` state (not hard deleted), allowing an optional reason.
- On confirmation, call `api.cut(id, reason)` (`POST /items/{id}/cut`) to soft-archive the Project and its subtree, triggering item state updates (`onChanged()`).

### 3. Web UI Home & Overview Filtering (`web/src/components/Home.tsx`, `web/src/components/Board.tsx`)
- Update `Home.tsx` to handle `Retired` projects cleanly, ensuring archived projects do not clutter active project lists while remaining accessible.
- Render muted/archived styling and scope cut indicators for `Retired` items across board and drawer views.

---

## Tasks & Dependencies

```
[Task 1: API & Store Soft-Archive] ───► [Task 2: Drawer Retire UI] ───► [Task 3: UI Indicators & Filtering]
```

### Task 1: API & Store: Verify and enhance soft-archive (cut_scope) for Projects and subtrees
- **Description**: Ensure `Board::cut_scope` in `src/store.rs` and `POST /items/:id/cut` in `src/api.rs` cleanly handle soft-archiving a Project and its subtree, transitioning all descendant items to `State::Retired`, recording a story event, and emitting appropriate updates. Add backend unit tests in `src/store.rs` and `src/api.rs` testing `cut_scope` on a Project with child tasks.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests in `src/store.rs` and `src/api.rs` verify that soft-archiving a Project transitions the Project and all descendant tasks to `State::Retired` and records the scope cut story event.

### Task 2: Web UI: Add Soft-Archive / Retire action and confirmation dialog in drawer
- **Description**: Update `web/src/components/Detail.tsx` to add a "Retire / Archive" action button and confirmation dialog for Projects (and Tasks). The confirmation dialog must clearly state that the Project and its subtree will be soft-archived to `Retired` state (not deleted), allow an optional reason, and invoke `api.cut(id, reason)`.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; `Detail.tsx` includes the Retire/Archive button and confirmation flow calling `api.cut`, and typescript typechecks/build succeeds.

### Task 3: Web UI: Visual indicator and filter management for Retired Projects and Tasks
- **Description**: Update `web/src/components/Home.tsx`, `Board.tsx`, and `Detail.tsx` to visually distinguish `Retired` projects/items (greyed/muted state, scope cut indicator) and provide clean filtering in the Projects overview so archived projects don't clutter active project views while remaining accessible.
- **Dependencies**: Task 1, Task 2.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; UI components render `Retired` items with muted/archived styling and handle retired projects correctly in filter selectors.
