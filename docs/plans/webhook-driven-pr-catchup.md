# Plan: Webhook-driven PR Catch-Up After Parallel Merges

## Context & Objectives
When parallel agents open PRs touching overlapping code, merging one PR advances `main`. Other open PRs for sibling cards in `Review` are left behind `main`.

To prevent merging stale PRs or encountering late merge failures, `honr` needs to:
1. Receive GitHub webhooks (via `/api/webhooks/github`, forwarded locally with `gh webhook forward`).
2. Detect open sibling PRs in `Review` that are now behind `main`.
3. Request or trigger a git rebase of those behind branches against `main`.
4. Handle rebase outcomes:
   - Clean rebase + green CI: stays quiet in `Review` until a human merges.
   - True rebase conflict: transition card to `Ready` with conflict context (conflicting files and failure reason).
   - Second conflict on overlapping files: detect repeated conflict on the same files, classify as a decomposition failure, and escalate to a human checkpoint (`NeedsHuman` state with escalation choices).

### Standing Invariants & Operational Rules
- **Merging is a human action**: Approving in `honr` surfaces the PR; `honr` never auto-merges PRs.
- **Invariant & Security Preservation**: Do not weaken `machine.rs` state machine invariants, supervisor budget enforcement, or `sandbox/policy.yaml`.
- **Webhook Ingress Only**: Ingress is strictly via GitHub webhooks (`POST /api/webhooks/github`). Do not introduce outbound runner polling.

---

## Architectural Changes & Design

### 1. Webhook Ingress (`src/api.rs`, `src/store.rs`, `src/events.rs`)
- Ingress is GitHub webhooks ONLY. No outbound runner relay polling GitHub.
- Add `POST /api/webhooks/github` endpoint in `src/api.rs`.
- Parse GitHub webhook payloads (`push` events on default branch `main`, `pull_request` merged events).
- Forward webhook notifications into `Board`, emitting a board event (`BoardEvent::MainAdvanced`).

### 2. PR Catch-Up & Rebase Trigger (`src/supervisor.rs`, `src/store.rs`)
- On `MainAdvanced` event, query all active cards in `Review` with associated PR branches (`pr_url` / git branches).
- Check if sibling PR branches are behind `main`.
- Queue rebase tasks for behind sibling PRs to update their branches against updated `main`.

### 3. Rebase Outcome & Conflict Context (`src/store.rs`, `src/model.rs`, `src/machine.rs`)
- Perform mechanical check via CI on the PR; no `Verify` column.
- On clean rebase and passing CI: retain card state in `Review` quietly.
- On git rebase conflict:
  - Extract conflicting file list from git output.
  - Record conflict context (`last_bounce_reason`, `conflict_files`) on `WorkItem`.
  - Transition card from `Review` to `Ready` with the conflict context reason.

### 4. Decomposition Failure & Human Checkpoint Escalation (`src/store.rs`, `src/supervisor.rs`)
- Maintain conflict file history on `WorkItem` / `Board`.
- If a card experiences a second rebase conflict on the same overlapping file set:
  - Classify as a decomposition failure (tasks were partitioned with overlapping boundaries).
  - Escalate to a human checkpoint (`NeedsHuman` state) with explicit `Escalation` options (e.g. "Re-split tasks to isolate overlapping files", "Manually resolve conflict and approve", "Retire card").
  - Block automated re-dispatch until human makes a choice.

---

## Tasks & Dependencies

```
[Task 1: Ingress: Webhook Endpoint] ──► [Task 2: PR Catch-Up & Rebase Trigger]
                                                     │
                                                     ▼
[Task 4: Repeated Conflict Escalation] ◄── [Task 3: Conflict Context & State Machine]
```

### Task 1: Ingress: GitHub Webhook Receiver Endpoint & Event Handler
- **Title**: `Ingress: GitHub Webhook Receiver Endpoint & Event Handler`
- **Intent**: Add HTTP webhook receiver (`/api/webhooks/github`) to ingest GitHub `push` and `pull_request` events, parse payloads, and notify `Board` when `main` advances without polling GitHub.
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; tests verify `/api/webhooks/github` accepts valid webhook payloads, filters for `main` branch updates, and emits `MainAdvanced` board events.

### Task 2: PR Catch-Up: Detect Behind Sibling PRs and Trigger Rebase
- **Title**: `PR Catch-Up: Detect Behind Sibling PRs and Trigger Rebase`
- **Intent**: When `main` advances via webhook event, identify open sibling PRs in `Review` that are behind `main` and queue rebase operations for those branches.
- **Dependencies**: Ingress: GitHub Webhook Receiver Endpoint & Event Handler.
- **Definition of Done**: `cargo test --offline --locked` passes; tests verify that `Board` identifies sibling PRs in `Review` behind `main` and dispatches rebase requests.

### Task 3: Conflict Context & State Machine Transitions on Rebase
- **Title**: `Conflict Context & State Machine Transitions on Rebase`
- **Intent**: Handle rebase execution and state transitions: keep clean rebases in `Review` quietly, and return cards with true rebase conflicts to `Ready` with recorded conflict file context and failure reason.
- **Dependencies**: PR Catch-Up: Detect Behind Sibling PRs and Trigger Rebase.
- **Definition of Done**: `cargo test --offline --locked` passes; tests verify clean rebase keeps card in `Review`, while git rebase conflicts move card to `Ready` with `last_bounce_reason` and conflicting file details set.

### Task 4: Decomposition Failure & Escalation Checkpoint on Repeated Conflict
- **Title**: `Decomposition Failure & Escalation Checkpoint on Repeated Conflict`
- **Intent**: Detect repeated rebase conflicts on the same overlapping files for a card, flag as a decomposition failure, and escalate to a human checkpoint in `NeedsHuman` state with structured resolution choices.
- **Dependencies**: Conflict Context & State Machine Transitions on Rebase.
- **Definition of Done**: `cargo test --offline --locked` passes; tests verify that a second conflict on overlapping files moves card to `NeedsHuman` with escalation options instead of returning to `Ready`.
