import React from "react";
import { renderToString } from "react-dom/server";
import assert from "node:assert";
import { DependencyGraph } from "./dist-test/components/DependencyGraph.js";

const now = Math.floor(Date.now() / 1000);

// Helper to create dummy WorkItem
function makeItem(id, title, blockedBy = [], state = "backlog") {
  return {
    id,
    parent: 2,
    level: "Story",
    title,
    intent: `Intent for ${title}`,
    definition_of_done: "Done",
    state,
    origin: { kind: "human" },
    above_line: false,
    blocked_by: blockedBy,
    blockers: blockedBy.map((bId) => ({ id: bId, title: `Task #${bId}`, state: "backlog" })),
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
}

// Test Case 1: Small Diamond DAG (Task 1 -> Task 2 & Task 3 -> Task 4)
const item1 = makeItem(1, "Root Setup Task", []);
const item2 = makeItem(2, "Left Branch Task", [1]);
const item3 = makeItem(3, "Right Branch Task", [1]);
const item4 = makeItem(4, "Diamond Merge Task", [2, 3]);

const diamondItems = [item1, item2, item3, item4];

const html = renderToString(
  React.createElement(DependencyGraph, {
    items: diamondItems,
    onOpen: () => {},
  })
);

console.log("Diamond DAG Graph HTML Output Length:", html.length);

// Assertions for DependencyGraph
assert(html.includes('data-testid="graph-container"'), "Should render graph container");
assert(html.includes('data-testid="graph-banner"'), "Should render plain-language status banner");
assert(html.includes("Visual Dependency DAG"), "Should render graph header title");
assert(html.includes("Root Setup Task"), "Should render Task 1");
assert(html.includes("Left Branch Task"), "Should render Task 2");
assert(html.includes("Right Branch Task"), "Should render Task 3");
assert(html.includes("Diamond Merge Task"), "Should render Task 4");

// Check topological steps (Step 1, Step 2, Step 3)
assert(/Step\s*(<!-- -->)?\s*1/.test(html), "Should compute Rank 0 (Step 1)");
assert(/Step\s*(<!-- -->)?\s*2/.test(html), "Should compute Rank 1 (Step 2)");
assert(/Step\s*(<!-- -->)?\s*3/.test(html), "Should compute Rank 2 (Step 3)");

// Check plain-language blocker cues
assert(html.includes("blocked by"), "Should contain plain-language blocker cue");
assert(html.includes("ready / unblocked"), "Should contain ready / unblocked cue for root task");

// Test Case 2: UI Fixture Board Diamond DAG
import { execSync } from "node:child_process";
const fixtureJson = JSON.parse(execSync("node ui-fixture.mjs").toString());
const fixtureItems = Object.values(fixtureJson.items);

const fixtureHtml = renderToString(
  React.createElement(DependencyGraph, {
    items: fixtureItems,
    onOpen: () => {},
  })
);

assert(fixtureHtml.includes("Surface PR checks on the Review card"), "Fixture should include Task A");
assert(fixtureHtml.includes("Fail closed when CI is red"), "Fixture should include Task B");
assert(fixtureHtml.includes("Report the real diffstat"), "Fixture should include Task C");
assert(fixtureHtml.includes("Observe cost during the run"), "Fixture should include Task D");

// Verify step ranks in fixture HTML output
assert(/Step\s*(<!-- -->)?\s*1/.test(fixtureHtml), "Fixture should contain Step 1 rank");
assert(/Step\s*(<!-- -->)?\s*2/.test(fixtureHtml), "Fixture should contain Step 2 rank");
assert(/Step\s*(<!-- -->)?\s*3/.test(fixtureHtml), "Fixture should contain Step 3 rank");

console.log("✅ All DependencyGraph component assertions passed!");

