# Plan: Harden Agent Split Protocol

## Context & Objectives

Currently in `honr`, agents self-orchestrate card decomposition by writing `.honr/split.json`. The supervisor processes this file in `process_verdict` and calls `Board::split` to materialize sibling tasks under the parent Project.

However, two major failure modes have been observed in practice:
1. **Unrelated/External Work Invention**: Agents split cards into unrelated sibling tasks or invent scope belonging to other projects, rather than carving the card's DoD into smaller slices of the same outcome.
2. **PR and Split Collisions (Split-Publish Race)**: Agents open/publish a PR on their card's branch and subsequently write a `split.json` file. Because publishing a PR and splitting the card into new tasks are mutually exclusive workflows for a single run, accepting a split after a PR is opened leaves orphan PRs or inconsistent board states.
3. **Unvalidated Splits Reaching Ready**: When an agent produces malformed or invalid split children (e.g. empty titles, missing intents, or invalid states), the supervisor and board currently risk materializing bad tasks directly into `Ready`.

The objective of **«Harden agent split protocol»** is to:
- Tighten the prompt briefing delivered to agents so the rules around splitting and PR mutual exclusivity are explicit and binding.
- Enforce PR/split mutual exclusivity in both `Board::split` and `process_verdict`, rejecting split attempts when a PR already exists and escalating the card to `NeedsHuman`.
- Validate split child payloads in the supervisor and store before materialization, ensuring invalid splits are stopped and escalated before any child tasks reach `Ready`.

---

## Architectural Changes & Protocol Rules

### 1. Briefing Rule Reinforcement (`src/supervisor.rs`)
- Update `briefing(...)` in `src/supervisor.rs` to include binding instructions:
  - Splits must ONLY carve the current card's DoD into smaller slices of the same outcome. Agents must never invent work belonging to another project (escalate instead).
  - Splitting and opening/publishing a PR are mutually exclusive for a single run. If a PR has already been opened or pushed, the agent must not split; it must finish via `report.json` or request human guidance via `escalate.json`.

### 2. Store & Supervisor PR-Split Collision Guard (`src/store.rs`, `src/supervisor.rs`)
- In `Board::split`: Check if `card.pr_url.is_some()`. Return an error if a PR is already recorded for the card.
- In `process_verdict`: Before processing a `"split"` verdict, check if `card.pr_url.is_some()` (or if a PR exists on the branch). If a PR exists, reject the split and escalate the card to `NeedsHuman` with an explanation ("Agent attempted to split after opening a PR; split and PR publish are mutually exclusive").

### 3. Split Child Validation & Safe Escalation (`src/supervisor.rs`, `src/store.rs`)
- Validate split child payload fields in `process_verdict` and `Board::split`:
  - Enforce non-empty `title` and non-empty `intent`.
  - Validate `definition_of_done`.
- If any child fails validation or if `Board::split` returns an error, ensure no child tasks are published to `Ready` and the parent card is cleanly escalated to `NeedsHuman`.

---

## Tasks & Dependencies

```
[Task 1: Briefing Rules] ───► [Task 2: Reject Split with PR] ───► [Task 3: Validate & Safe Escalation]
```

### Task 1: Supervisor: Tighten agent briefing rules for splits and PR collisions
- **Description**: Update `briefing(...)` in `src/supervisor.rs` to explicitly instruct agents that splits may only carve the current card's DoD into smaller slices of the same outcome, and that splitting after opening a PR is prohibited (split and publish are mutually exclusive).
- **Dependencies**: None.
- **Definition of Done**: `cargo test --offline --locked` passes; supervisor unit tests in `src/supervisor.rs` verify that the generated briefing contains explicit instructions regarding slice-only splits, prohibiting inventing external work, and PR/split mutual exclusivity.

### Task 2: Store & Supervisor: Reject split attempts on cards with existing PRs
- **Description**: Update `Board::split` in `src/store.rs` and `process_verdict` in `src/supervisor.rs` to check whether a PR already exists for the card (`pr_url` set or PR detected). If so, refuse the split and escalate the card to NeedsHuman rather than creating sibling tasks.
- **Dependencies**: Task 1.
- **Definition of Done**: `cargo test --offline --locked` passes; unit tests in `src/store.rs` and `src/supervisor.rs` verify that calling split on a card with a PR returns an error and escalates to NeedsHuman.

### Task 3: Supervisor & Store: Validate split child payloads and prevent bad splits from reaching Ready
- **Description**: Update `process_verdict` in `src/supervisor.rs` and `Board::split` in `src/store.rs` to strictly validate split child fields (non-empty title, non-empty intent, valid DoD) and ensure that if any child fails validation or governor limits, no sibling tasks are published to Ready and the parent card is escalated.
- **Dependencies**: Task 1, Task 2.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass; unit tests verify that invalid split payloads (e.g., empty title/intent) are rejected and escalated before reaching Ready.
