# Plan: Restore beads ↔ GitHub Issues Auto-Sync

## Context & Objectives

In `honr`, beads (`bd`) is used to mirror projects, tasks, dependencies, and synchronize state with GitHub Issues via `bd github sync`.

Currently:
1. **Board Creation & Split Dual-Write**: When cards are created via `Board::create` or `Board::split`, temporary placeholder IDs (`bd-honr-{id}`) are initially assigned, and `schedule_beads_mirror` is scheduled to asynchronously create the corresponding epic/task in beads. However, `beads.github_sync()` is not automatically invoked after real beads IDs are set.
2. **Card Closure Auto-Sync**: When a card transitions to `Done` or `Retired`, `store.rs` triggers `beads.close`, but does not automatically execute `beads.github_sync()` to push the closed state to GitHub Issues remote.
3. **Runtime Auth & CLI Path Fallback**: `BeadsClient::github_sync` currently relies on `GITHUB_TOKEN` environment variable or calling `gh auth token`. When `PATH` is restricted or thin, `gh` execution can fail unless standard fallback paths (such as `/opt/homebrew/bin/gh`) are checked.

The objective of **«Restore beads ↔ GitHub Issues auto-sync»** is to:
- Restore automatic push synchronization to GitHub Issues whenever board items are created, split, or closed/completed.
- Dual-write real beads IDs on creation and split, ensuring board state and GitHub Issues mirror seamlessly without requiring manual `bd github sync` commands.
- Ensure robust runtime `GITHUB_TOKEN` resolution with fallback binary paths for `gh`.

---

## Architectural Changes & Implementation Details

### 1. Robust Auth Resolution & Path Fallback in `BeadsClient::github_sync` (`src/beads.rs`)
- Update `github_sync` in `src/beads.rs` to:
  - Read `GITHUB_TOKEN` from the environment if present.
  - If `GITHUB_TOKEN` is unset or empty, attempt running `gh auth token`, checking `gh` in `PATH` first and falling back to `/opt/homebrew/bin/gh`.
  - Execute `bd github sync --push-only` with the resolved `GITHUB_TOKEN` environment variable.
  - Handle errors gracefully with warning logs so transient network or auth errors do not crash or block internal board state transitions.

### 2. Dual-Write Real Beads IDs & Push Sync on Creation / Split (`src/store.rs`, `src/beads.rs`)
- In `schedule_beads_mirror` (`src/store.rs`), after `beads.create_linked` succeeds and `board.set_beads_id` records the canonical beads hash ID:
  - Call `beads.github_sync()` asynchronously to push the newly created epic or task to GitHub Issues immediately.
- Ensure all card creation entry points (`Board::create`, `Board::split`, MCP, API) that invoke `schedule_beads_mirror` automatically mirror new cards to GitHub Issues.

### 3. Auto-Sync Card Close & Done Transitions (`src/store.rs`, `src/beads.rs`)
- In `src/store.rs` status transition handling (`State::Done` and `State::Retired`):
  - After `beads.close(&bid, ...)` finishes, trigger `beads.github_sync()` asynchronously to push the closed state to GitHub Issues.

---

## Tasks & Dependencies

```
[Task 1: Auth & Sync Helper]
        │
        ├───► [Task 2: Creation Dual-Write & Sync]
        │            │
        └────────────┴───► [Task 3: Close/Done Transition Sync]
```

### Task 1: Beads: Enhance `BeadsClient::github_sync` with `gh` binary path fallback and robust token resolution
- **Intent**: Update `github_sync` in `src/beads.rs` to attempt `GITHUB_TOKEN` env var, falling back to `gh auth token` using `gh` or `/opt/homebrew/bin/gh` when `PATH` is thin. Ensure non-blocking warning logs on failure.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; unit tests in `src/beads.rs` verify `github_sync` token resolution and execution behavior.

### Task 2: Store & Beads: Dual-write real beads IDs and push to GitHub Issues on card create/split
- **Intent**: Update `schedule_beads_mirror` in `src/store.rs` to invoke `beads.github_sync()` upon assigning real beads IDs during card creation and card split, ensuring new items mirror to GitHub Issues without manual sync.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests in `src/store.rs` verify `schedule_beads_mirror` sets real `beads_id` and triggers `github_sync`.

### Task 3: Store & Beads: Auto-sync card close/done transitions to beads and GitHub Issues
- **Intent**: Update card status transition handling in `src/store.rs` (`State::Done`, `State::Retired`) to invoke `beads.github_sync()` after `beads.close` completes, pushing closed state to GitHub Issues.
- **Dependencies**: Task 1, Task 2.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests in `src/store.rs` verify transitioning a card to `Done` or `Retired` triggers `beads.close` followed by `github_sync`.
