# Plan: Keep Sandboxes Until Done

## Context & Objectives

Currently in `honr`, when an agent completes a run successfully (transitioning a card from `Running` to `Review`), `finalize` in `src/supervisor.rs` immediately deletes the OpenShell sandbox (`os.delete`).
If a human reviewer requests changes on the card, or if the card needs to be re-run, a brand new sandbox must be created from scratch. This requires downloading packages, cloning the repository, and building all Rust/Web dependencies again from zero (`cargo build`, `npm install`, etc.).

The objective of **«Keep sandboxes until Done»** is to park each card's OpenShell sandbox from its first claim until the card reaches `Done` (or `Retired`/Cut). By preserving live sandboxes across `Review` and `Request changes` iterations, `WORKDIR` build caches (such as Cargo `target/` directories and Node `node_modules/`) survive between runs. This dramatically speeds up subsequent iteration loops.

---

## Standing Constraints & Architectural Changes

### 1. Preserve Sandboxes on Run Completion to Review
- **Current Behavior**: `finalize` in `src/supervisor.rs` calls `os.delete(name)` on successful runs (`Ok(_)`).
- **Target Behavior**: On successful run completion (`Running` → `Review`), stop the agent process inside the sandbox (`stop_agent`) to halt active billing, but KEEP the sandbox in OpenShell (`os.delete` is skipped).

### 2. Preserve Parked Sandboxes During Reconcile
- **Current Behavior**: `adoptable` filters only for `State::Claimed | State::Running`. `reconcile` reaps any sandbox whose associated item is not currently claimed or running.
- **Target Behavior**: Update `adoptable` and `reconcile` so sandboxes associated with non-terminal cards (`Review`, `Ready` with a named `environment`, `Claimed`, `Running`) are kept intact. Only reap sandboxes when the associated card is deleted/absent or in terminal state (`Done`, `Retired`).

### 3. In-Place Sandbox Reuse on Card Reclaim
- **Current Behavior**: `run_inside` always calls `os.delete` and `os.create` before running a full `git clone`.
- **Target Behavior**: When claiming a card, if `card.environment` names a live sandbox (`os.exec` probe succeeds), reuse the existing sandbox. Perform in-place code refresh (`git fetch`, `git rebase`/`checkout`) and briefing upload without re-creating the container. Fall back to container creation only if the sandbox is dead or missing.

### 4. Explicit Sandbox Cleanup on Done, Retired, or Item Deletion
- **Target Behavior**: When a card reaches terminal state (`Done`, `Retired`/Cut) or is deleted (`delete_item`), delete the named OpenShell sandbox (`os.delete`) and clear `environment`.

---

## Tasks & Dependencies

```
[Task 1: Keep on Review] ──┬──► [Task 3: Reclaim & In-Place Reuse] ──┐
                           │                                         ├──► [Task 4: Cleanup on Done/Retire]
[Task 2: Preserve Reconcile]┴────────────────────────────────────────┘
```

### Task 1: Supervisor: Preserve sandboxes on successful run completion to Review
- **Description**: Modify `finalize` in `src/supervisor.rs` so that when an agent run finishes successfully (`Ok(_)`) and transitions to `Review`, the OpenShell sandbox is preserved rather than deleted. Stop the agent process inside the sandbox (`stop_agent`) to prevent lingering resource usage or billing.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; supervisor unit tests verify that successful runs preserve the sandbox and stop the agent process.

### Task 2: Supervisor: Preserve parked sandboxes in Review and Ready during reconcile
- **Description**: Update `adoptable` and `reconcile` in `src/supervisor.rs` so that sandboxes for non-terminal cards (`Review` and `Ready` with `environment` set) are preserved across restart/reconciliation loops instead of being reaped as orphans.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; reconciliation unit tests verify that sandboxes for `Review` and `Ready` cards are kept during reconcile.

### Task 3: Supervisor: In-place sandbox reuse and branch refresh on card reclaim
- **Description**: Update `run_inside` in `src/supervisor.rs` so that when claiming a card with an existing `environment` sandbox, `honr` probes whether the box is live. If live, skip `os.delete`/`os.create` and refresh the workspace in place using `git fetch` and branch update/rebase, preserving `WORKDIR` build caches (`target/`, etc.). Recreate only if the sandbox is dead or missing.
- **Dependencies**: Task 1, Task 2.
- **Definition of Done**: `cargo test --offline --locked` passes; supervisor tests verify live sandbox reuse and in-place workspace refresh on card reclaim.

### Task 4: Store & Supervisor: Delete sandboxes on Done, Retired, and item deletion
- **Description**: Ensure that when a card transitions to terminal states (`Done` or `Retired`/Cut) or is deleted via `delete_item`, any associated sandbox named in `item.environment` is deleted via `os.delete` and `environment` is cleared.
- **Dependencies**: Task 1, Task 2, Task 3.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; unit tests verify sandbox deletion on `Done`, `Retired`, and item deletion.
