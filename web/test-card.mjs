import React from "react";
import { renderToString } from "react-dom/server";
import assert from "node:assert";
import { Card } from "./dist-test/components/Card.js";

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
assert(!unblockedHtml.includes("waiting on"), "Unblocked card should not render waiting on");

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

console.log("\n✅ All Card component assertions passed!");
