# Plan: Surface Infra Bounce Reasons on the Card

## Context & Objectives
When an infrastructure failure occurs during agent dispatch (e.g., `openshell` sandbox creation error, `podman.sock` connection error, gateway timeout), honr's supervisor currently calls `board.release(id, &agent_id)`.
Currently, `release` hardcodes the transition reason as `"released by agent"` and does not record the specific human-readable infrastructure error message on the `WorkItem`.
As a result, when a card returns to `Ready` due to an infrastructure outage, human operators looking at the board, drawer, or API cannot tell why the card was bounced without digging through honr's background process logs.

The objective of this project is to capture human-readable infrastructure errors, pass them to `release` (or `record_infra_bounce`), store the bounce reason on the card's history and item state (`last_bounce_reason`), and display this bounce reason in the Web UI card drawer and overview.

---

## Architectural Changes

### 1. Data Model & Store (`src/model.rs`, `src/store.rs`)
- Add `pub last_bounce_reason: Option<String>` to `WorkItem` struct in `src/model.rs`.
- Update `Board::release` (or add `release_with_reason`) in `src/store.rs` to accept an explicit reason string parameter (`reason: Option<&str>`).
- When releasing an item due to an infrastructure or execution bounce:
  - Record the exact failure message as the transition reason in `HistoryEntry`.
  - Set `it.last_bounce_reason = Some(reason.to_string())` on `WorkItem`.
  - Add a narrative story line (`board.story(...)`) detailing the bounce event.

### 2. Supervisor (`src/supervisor.rs`)
- In `Fleet::supervise`: when `is_infrastructure(&msg)` is true (or sandbox creation fails), forward the exact error string `msg` to `board.release` (e.g., `f.board.release(id, &agent_id, Some(&format!("infra failure: {msg}")))`).
- Ensure no infrastructure error is masked by generic hardcoded messages like `"released by agent"`.

### 3. API & Web UI (`src/api.rs`, `web/src/components/Detail.tsx`, `web/src/components/Overview.tsx`)
- Ensure API serialization of `WorkItem` exposes `last_bounce_reason`.
- In `web/src/components/Detail.tsx` (Card Drawer): render a prominent notice/banner when `last_bounce_reason` is set, displaying the human-readable bounce reason.
- In `web/src/components/Overview.tsx`: render an indicator icon/pill on cards in `Ready` that have a recent bounce reason.

---

## Tasks & Dependencies

```
[Task 1: Store & Model] ───► [Task 2: Supervisor] ───► [Task 3: API & Web UI]
```

### Task 1: Store & Model: Record infra bounce reason on WorkItem and transition history
- **Description**: Add `last_bounce_reason: Option<String>` to `WorkItem` in `src/model.rs`. Update `Board::release` (or add `release_with_reason`) in `src/store.rs` to take an explicit reason string and record it in `HistoryEntry`, `WorkItem`, and board story.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests in `src/store.rs` verify that calling `release` with an infra reason stores `last_bounce_reason` on `WorkItem` and preserves the exact reason in the item's transition history.

### Task 2: Supervisor: Capture and forward human-readable error messages on infrastructure failure
- **Description**: Update `Fleet::supervise` in `src/supervisor.rs` so that when `is_infrastructure(&msg)` is true or sandbox creation fails, the human-readable error message `msg` is passed directly to `board.release`.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` passes; unit test in `src/supervisor.rs` (or `store.rs`) verifies that infrastructure error strings are passed to `board.release` and saved on the item.

### Task 3: API & Web UI: Expose bounce reasons in API and render in card drawer
- **Description**: Ensure `last_bounce_reason` is included in API item responses (`src/api.rs`). Update Web UI (`web/src/components/Detail.tsx` and `web/src/components/Overview.tsx`) to render the bounce reason in the card drawer and card list views.
- **Dependencies**: Task 1, Task 2.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; card detail drawer renders the human-readable bounce reason banner when `last_bounce_reason` is present.
