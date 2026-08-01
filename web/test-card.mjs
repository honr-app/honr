import React from "react";
import { renderToString } from "react-dom/server";
import assert from "node:assert";
import { Card } from "./dist-test/components/Card.js";
import { Home } from "./dist-test/components/Home.js";
import { isBlocked, sortFor } from "./dist-test/components/Board.js";
import { Head } from "./dist-test/components/Detail.js";

const now = Math.floor(Date.now() / 1000);

// Test 1: Blocked card renders human-readable blocker chips
const blockedItem = {
  id: 7,
  parent: null,
  level: "Story",
  title: "Verify from a clean checkout",
  intent: "Gates in the agent's sandbox can be influenced by the agent.",
  definition_of_done: "Done",
  state: "ready",
  origin: { kind: "human" },
  above_line: false,
  blocked_by: [6],
  blockers: [
    { id: 6, title: "Supervisor runs the gates", state: "ready" },
  ],
  capability: "any",
  lease: null,
  model: null,
  progress: 0,
  cost_cents: 0,
  budget_cents: null,
  escalation: null,
  gates: [],
  gate_failures: 0,
  diff_added: 0,
  diff_removed: 0,
  notes: [],
  pinned: [],
  release_target: null,
  environment: null,
  pr_url: null,
  created_at: new Date().toISOString(),
  entered_state_at: new Date().toISOString(),
  history: [],
};

const blockedHtml = renderToString(
  React.createElement(Card, {
    item: blockedItem,
    column: "ready",
    now,
    heartbeatExpect: 600,
    onOpen: () => {},
  })
);

console.log("Blocked Card HTML:\n", blockedHtml);

// Assertions for blocked card
assert(blockedHtml.includes('class="blocker-chips"'), "Should contain blocker-chips container");
assert(blockedHtml.includes('class="blocker-chip state-ready"'), "Should contain blocker-chip with state class");
assert(blockedHtml.includes('#6'), "Should contain blocker ID");
assert(blockedHtml.includes('Supervisor runs the gates'), "Should contain human-readable blocker title");
assert(blockedHtml.includes('ready'), "Should contain state cue");

// Test 2: Unblocked card is empty when unblocked (no blocker chips rendered)
const unblockedItem = {
  ...blockedItem,
  id: 8,
  blocked_by: [],
  blockers: [],
};

const unblockedHtml = renderToString(
  React.createElement(Card, {
    item: unblockedItem,
    column: "ready",
    now,
    heartbeatExpect: 600,
    onOpen: () => {},
  })
);

assert(!unblockedHtml.includes("blocker-chips"), "Unblocked card should be empty when unblocked (no blocker chips)");

// Test 3: Running card with engine null and defaultEngine agy shows agy badge
const runningItem = {
  ...unblockedItem,
  id: 9,
  state: "running",
  engine: null,
  model: "claude-opus-5",
  progress: 0.5,
  cost_cents: 100,
  lease: {
    agent_id: "agent-1",
    granted_at: new Date().toISOString(),
    last_heartbeat: new Date().toISOString(),
    expires_at: new Date().toISOString(),
  },
};

const runningAgyHtml = renderToString(
  React.createElement(Card, {
    item: runningItem,
    column: "running",
    now,
    heartbeatExpect: 600,
    defaultEngine: "agy",
    onOpen: () => {},
  })
);

console.log("\nRunning Card HTML (defaultEngine=agy):\n", runningAgyHtml);
assert(runningAgyHtml.includes("agy"), "Running card with engine null and defaultEngine agy should render agy badge");
assert(!runningAgyHtml.includes("◍ claude-opus-5"), "Running card should not render model name as badge when engine resolves to agy");

// Test 4: Running card with engine null and defaultEngine claude shows claude badge
const runningClaudeHtml = renderToString(
  React.createElement(Card, {
    item: runningItem,
    column: "running",
    now,
    heartbeatExpect: 600,
    defaultEngine: "claude",
    onOpen: () => {},
  })
);

console.log("\nRunning Card HTML (defaultEngine=claude):\n", runningClaudeHtml);
assert(runningClaudeHtml.includes("claude"), "Running card with engine null and defaultEngine claude should render claude badge");

// Test 5: isBlocked helper
assert.strictEqual(isBlocked(blockedItem), true, "blockedItem should be blocked");
assert.strictEqual(isBlocked(unblockedItem), false, "unblockedItem should be unblocked");

const resolvedBlockerItem = {
  ...blockedItem,
  id: 10,
  blocked_by: [6],
  blockers: [{ id: 6, title: "Supervisor runs the gates", state: "done" }],
};
assert.strictEqual(isBlocked(resolvedBlockerItem), false, "Item with done blocker should be unblocked");

// Test 6: Ready column sorts claimable cards first, including after claim-release bounce
const oldDate = new Date(Date.now() - 3600 * 1000).toISOString();
const olderDate = new Date(Date.now() - 7200 * 1000).toISOString();

const card1_unblocked = {
  ...unblockedItem,
  id: 1,
  title: "Unblocked Card 1",
  entered_state_at: oldDate,
};

const card2_blocked = {
  ...blockedItem,
  id: 2,
  title: "Blocked Card 2",
  entered_state_at: olderDate, // older timestamp
};

const card3_blocked = {
  ...blockedItem,
  id: 3,
  title: "Blocked Card 3",
  entered_state_at: olderDate,
};

const card4_blocked = {
  ...blockedItem,
  id: 4,
  title: "Blocked Card 4",
  entered_state_at: olderDate,
};

const card5_blocked = {
  ...blockedItem,
  id: 5,
  title: "Blocked Card 5",
  entered_state_at: olderDate,
};

// Ready column sorting: Card 1 (unblocked) must sort before Cards 2..5 (blocked)
let readyCards = [card2_blocked, card3_blocked, card4_blocked, card5_blocked, card1_unblocked];
readyCards.sort(sortFor("ready"));
assert.strictEqual(readyCards[0].id, 1, "Unblocked card #1 must sort first");

// Claim -> Release bounce: Card 1 is claimed and then released back to Ready.
// Its entered_state_at refreshes to NOW (newest timestamp).
card1_unblocked.entered_state_at = new Date().toISOString();

readyCards = [card2_blocked, card3_blocked, card4_blocked, card5_blocked, card1_unblocked];
readyCards.sort(sortFor("ready"));

assert.strictEqual(readyCards[0].id, 1, "After claim-release bounce, unblocked card #1 must STILL sort first");

// Test 7: Home Issues rows show a friendly waiting-on line when blocked_by is non-empty
const projectItem = {
  id: 100,
  parent: null,
  level: "Project",
  title: "Test Project",
  intent: "Test project intent",
  definition_of_done: "Done",
  state: "ready",
  origin: { kind: "human" },
  above_line: true,
  blocked_by: [],
  blockers: [],
  capability: null,
  lease: null,
  model: null,
  progress: 0,
  cost_cents: 0,
  budget_cents: null,
  escalation: null,
  gates: [],
  gate_failures: 0,
  diff_added: 0,
  diff_removed: 0,
  notes: [],
  pinned: [],
  release_target: null,
  environment: null,
  pr_url: null,
  created_at: new Date().toISOString(),
  entered_state_at: new Date().toISOString(),
  history: [],
};

const blockedHomeItem = {
  ...blockedItem,
  parent: 100,
};

const unblockedHomeItem = {
  ...unblockedItem,
  parent: 100,
};

const homeItemsMap = new Map([
  [100, projectItem],
  [7, blockedHomeItem],
  [8, unblockedHomeItem],
]);

const homeHtml = renderToString(
  React.createElement(Home, {
    items: homeItemsMap,
    goals: [],
    now,
    onOpen: () => {},
    onOpenBoard: () => {},
    onChanged: () => {},
  })
);

console.log("\nHome HTML:\n", homeHtml);

assert(homeHtml.includes('class="owaiting blocker-chips"') || homeHtml.includes('blocker-chips'), "Home should contain blocker-chips line");
assert(homeHtml.includes('⊘ waiting on'), "Home should contain waiting on label");
assert(homeHtml.includes('Supervisor runs the gates'), "Home row should display human-readable blocker title");

// Test 8: Home with only unblocked items renders no waiting-on line
const unblockedHomeItemsMap = new Map([
  [100, projectItem],
  [8, unblockedHomeItem],
]);
const cleanHomeHtml = renderToString(
  React.createElement(Home, {
    items: unblockedHomeItemsMap,
    goals: [],
    now,
    onOpen: () => {},
    onOpenBoard: () => {},
    onChanged: () => {},
  })
);

assert(!cleanHomeHtml.includes("owaiting"), "Home with unblocked items should stay clean");
assert(!cleanHomeHtml.includes("waiting on"), "Home with unblocked items should not show waiting on");

// Test 9: Detail Head renders Archive and Delete actions
const headHtml = renderToString(
  React.createElement(Head, {
    title: "#100 Test Project",
    onClose: () => {},
    onArchive: () => {},
    onDelete: () => {},
  })
);

console.log("\nDetail Head HTML:\n", headHtml);
assert(headHtml.includes("📦 Archive"), "Detail Head should offer Archive action button");
assert(headHtml.includes("🗑 Delete"), "Detail Head should offer Delete action button");

console.log("\n✅ All Card, Board, Home, and Detail component assertions passed!");
