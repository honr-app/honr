import React from "react";
import { renderToString } from "react-dom/server";
import assert from "node:assert";
import { Card } from "./dist-test/components/Card.js";
import { Cockpit, isBlocked, sortFor } from "./dist-test/components/Cockpit.js";
import { Head, PlanEditor, planTasksFromArtifact, reduceDetail } from "./dist-test/components/Detail.js";
import { PrimarySidebar } from "./dist-test/components/PrimarySidebar.js";
import { ProjectSandboxPicker, SandboxesPanelView, Settings, WorkspacePanelView, OpenShellPanelView } from "./dist-test/components/Settings.js";
import { initial, reduce, isSequenceGap, subscribeBoardEvents, emitBoardEvent } from "./dist-test/useBoard.js";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const now = Math.floor(Date.now() / 1000);

// Test 1: Blocked card renders human-readable blocker chips
const blockedItem = {
  id: 7,
  parent: null,
  level: "Story",
  title: "Fail closed when CI is red",
  intent: "A Review card with failing checks should be obvious.",
  definition_of_done: "Done",
  state: "backlog",
  origin: { kind: "human" },
  above_line: false,
  blocked_by: [6],
  blockers: [
    { id: 6, title: "Surface PR checks on the Review card", state: "backlog" },
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
  pull_request: null,
  created_at: new Date().toISOString(),
  entered_state_at: new Date().toISOString(),
  history: [],
};

const blockedHtml = renderToString(
  React.createElement(Card, {
    item: blockedItem,
    column: "backlog",
    now,
    agentTimeout: 600,
    onOpen: () => {},
  })
);

console.log("Blocked Card HTML:\n", blockedHtml);

// Assertions for blocked card
assert(blockedHtml.includes('class="blocker-chips"'), "Should contain blocker-chips container");
assert(blockedHtml.includes('class="blocker-chip state-backlog"'), "Should contain blocker-chip with state class");
assert(blockedHtml.includes('#6'), "Should contain blocker ID");
assert(blockedHtml.includes('Surface PR checks on the Review card'), "Should contain human-readable blocker title");
assert(blockedHtml.includes('backlog'), "Should contain state cue");

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
    column: "backlog",
    now,
    agentTimeout: 600,
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
    expires_at: new Date(now + 600_000).toISOString(),
  },
  run_deadline_at: new Date(now + 600_000).toISOString(),
};

const runningAgyHtml = renderToString(
  React.createElement(Card, {
    item: runningItem,
    column: "running",
    now,
    agentTimeout: 600,
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
    agentTimeout: 600,
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

// Test 6: Backlog column sorts claimable cards first, including after claim-release bounce
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

// Backlog column sorting: Card 1 (unblocked) must sort before Cards 2..5 (blocked)
let readyCards = [card2_blocked, card3_blocked, card4_blocked, card5_blocked, card1_unblocked];
readyCards.sort(sortFor("backlog"));
assert.strictEqual(readyCards[0].id, 1, "Unblocked card #1 must sort first");

// Claim -> Release bounce: Card 1 is claimed and then released back to Backlog.
// Its entered_state_at refreshes to NOW (newest timestamp).
card1_unblocked.entered_state_at = new Date().toISOString();

readyCards = [card2_blocked, card3_blocked, card4_blocked, card5_blocked, card1_unblocked];
readyCards.sort(sortFor("backlog"));

assert.strictEqual(readyCards[0].id, 1, "After claim-release bounce, unblocked card #1 must STILL sort first");

// Test 7: Cockpit surfaces Needs you action cards with humanized copy
const projectItem = {
  id: 100,
  parent: null,
  level: "Project",
  title: "Test Project",
  intent: "Test project intent",
  definition_of_done: "Done",
  state: "backlog",
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
  pull_request: null,
  created_at: new Date().toISOString(),
  entered_state_at: new Date().toISOString(),
  history: [],
};

const needsYouItem = {
  ...blockedItem,
  id: 9,
  parent: 100,
  state: "needs_human",
  blocked_by: [],
  blockers: [],
  escalation: {
    question:
      "Task failed to run 3 times. Last failure: clone failed: fatal: unable to access 'https://example.com/x.git/': CONNECT tunnel failed, response 403",
    options: [
      { label: "Investigate the environment", detail: "infra" },
      { label: "Cut scope", detail: "drop" },
    ],
    recommended: 0,
    blocked_since: new Date(Date.now() - 3600_000).toISOString(),
    answer: null,
  },
};

const cockpitHtml = renderToString(
  React.createElement(Cockpit, {
    items: new Map([
      [100, projectItem],
      [9, needsYouItem],
    ]),
    goals: [
      {
        id: 100,
        title: "Test Project",
        intent: "Test",
        progress: 0,
        leaves_done: 0,
        leaves_total: 1,
        spend_cents: 0,
        budget_cents: null,
        agents_live: 0,
        needs_you: 1,
        plan_status: "approved_v1",
        columns: [],
        story: [],
      },
    ],
    stories: new Map(),
    goalOf: () => 100,
    breadcrumbOf: () => "Test Project",
    now,
    agentTimeout: 600,
    onOpen: () => {},
    onChanged: () => {},
  })
);

console.log("\nCockpit HTML:\n", cockpitHtml.slice(0, 800));
assert(cockpitHtml.includes("cockpit-needs"), "Cockpit should show Needs you section");
assert(
  cockpitHtml.includes("Sandbox couldn") && cockpitHtml.includes("clone"),
  "Cockpit Needs you should humanize clone failures",
);
assert(cockpitHtml.includes("Investigate the environment"), "Cockpit should offer answer options");

// Test 8: Detail Head renders Archive and Delete actions

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

// Test 9: Detail Plan editor renders plan task blocker selection UI
const samplePlanTasksSpec = [
  { key: "t1", title: "Setup Database", intent: "Setup DB intent", definition_of_done: "DB ready", blocked_by_keys: [] },
  { key: "t2", title: "Build API", intent: "Build API intent", definition_of_done: "API ready", blocked_by_keys: ["t1"] },
];

const editPlanTasks = planTasksFromArtifact(samplePlanTasksSpec);

const planEditorHtml = renderToString(
  React.createElement(PlanEditor, {
    planTasks: editPlanTasks,
    setPlanTasks: () => {},
  })
);

console.log("\nPlan Editor HTML:\n", planEditorHtml);
assert(planEditorHtml.includes("Blocked by tasks:"), "Plan editor should render 'Blocked by tasks:' label");
assert(planEditorHtml.includes("Setup Database"), "Blocker chip for t1 should display human readable sibling task title");
assert(planEditorHtml.includes("+ Select blocker task..."), "Plan editor should offer '+ Select blocker task...' dropdown to select sibling tasks");

// Test 10: reduce tracks lastSeenSeq from Snapshot and BoardEvent
let s = reduce(initial, {
  type: "snapshot",
  snap: {
    items: [unblockedItem],
    levels: [],
    goals: [],
    server_time: new Date().toISOString(),
    agent_timeout_secs: 1800,
    seq: 10,
  },
});
assert.strictEqual(s.lastSeenSeq, 10, "Snapshot seq 10 should update lastSeenSeq to 10");

const liveItem = { ...unblockedItem, title: "Updated Live by Event" };
s = reduce(s, {
  type: "event",
  ev: {
    type: "upsert",
    seq: 11,
    item: liveItem,
  },
});
assert.strictEqual(s.lastSeenSeq, 11, "BoardEvent seq 11 should update lastSeenSeq to 11");
assert.strictEqual(s.items.get(8).title, "Updated Live by Event", "Upsert event should update item in state");

// Test 11: Stale REST snapshot race protection
const staleSnap = {
  items: [{ ...unblockedItem, title: "Stale REST Snapshot Title" }],
  levels: [],
  goals: [],
  server_time: new Date().toISOString(),
  agent_timeout_secs: 1800,
  seq: 9, // older sequence number than lastSeenSeq=11
};

const sAfterStaleSnap = reduce(s, { type: "snapshot", snap: staleSnap });
assert.strictEqual(sAfterStaleSnap.lastSeenSeq, 11, "Stale snapshot (seq 9 < 11) must not lower lastSeenSeq");
assert.strictEqual(
  sAfterStaleSnap.items.get(8).title,
  "Updated Live by Event",
  "Stale REST snapshot (seq 9 < 11) must not overwrite newer live event state"
);

// Test 12: Stale/duplicate BoardEvent ignored
const staleEvent = {
  type: "upsert",
  seq: 10, // older than lastSeenSeq=11
  item: { ...unblockedItem, title: "Duplicate/Stale Event Title" },
};
const sAfterStaleEvent = reduce(s, { type: "event", ev: staleEvent });
assert.strictEqual(sAfterStaleEvent.lastSeenSeq, 11, "Stale event (seq 10 <= 11) must keep lastSeenSeq at 11");
assert.strictEqual(
  sAfterStaleEvent.items.get(8).title,
  "Updated Live by Event",
  "Stale event (seq 10 <= 11) must not overwrite newer state"
);

// Test 13: Sequence Gap detection helper
assert.strictEqual(isSequenceGap(11, 12), false, "Sequential event (12 after 11) is not a gap");
assert.strictEqual(isSequenceGap(11, 14), true, "Event with gap (14 after 11) is detected as sequence gap");
assert.strictEqual(isSequenceGap(0, 5), false, "Initial event with lastSeenSeq=0 is not a gap");

// Test 14: reduceDetail updates card Detail drawer state live upon receiving Upsert event
const detailInitial = {
  ...unblockedItem,
  id: 7,
  title: "Initial Card Title",
  state: "running",
  notes: [{ author: "human", text: "initial note" }],
  ancestry: [{ level: "Project", title: "Parent Project", intent: "project intent" }],
  children: [10, 11],
};

const upsertEv = {
  type: "upsert",
  seq: 20,
  item: {
    ...unblockedItem,
    id: 7,
    title: "Updated Card Title Live",
    state: "review",
    pull_request: { url: "https://github.com/shanemcd/honr/pull/186" },
    notes: [
      { author: "human", text: "initial note" },
      { author: "agent", text: "PR opened" },
    ],
  },
};

const updatedDetail = reduceDetail(detailInitial, upsertEv, 7);
assert.strictEqual(updatedDetail.title, "Updated Card Title Live", "Upsert event for id 7 must update detail title live");
assert.strictEqual(updatedDetail.state, "review", "Upsert event for id 7 must update detail state live");
assert.strictEqual(updatedDetail.pull_request?.url, "https://github.com/shanemcd/honr/pull/186", "Upsert event for id 7 must update pull_request.url live");
assert.strictEqual(updatedDetail.notes.length, 2, "Upsert event for id 7 must update notes live");
assert.strictEqual(updatedDetail.ancestry.length, 1, "reduceDetail must preserve existing detail ancestry");

// Upsert event for a different card ID does not alter Detail state for card 7
const otherUpsertEv = {
  type: "upsert",
  seq: 21,
  item: { ...unblockedItem, id: 99, title: "Unrelated Card" },
};
const unchangedDetail = reduceDetail(updatedDetail, otherUpsertEv, 7);
assert.strictEqual(unchangedDetail.title, "Updated Card Title Live", "Upsert event for different id 99 must not modify detail for id 7");

// Delete event for card 7 clears detail
const deleteEv = { type: "delete", seq: 22, id: 7 };
const deletedDetail = reduceDetail(updatedDetail, deleteEv, 7);
assert.strictEqual(deletedDetail, null, "Delete event for matching id 7 must clear detail");

// Test 15: subscribeBoardEvents and emitBoardEvent live drawer subscription
let receivedEvent = null;
const unsubscribe = subscribeBoardEvents((ev) => {
  receivedEvent = ev;
});

emitBoardEvent(upsertEv);
assert.deepStrictEqual(receivedEvent, upsertEv, "subscribeBoardEvents listener must receive emitted board event");

// Unsubscribe cleanly removes listener
receivedEvent = null;
unsubscribe();

const nextEv = { type: "delete", seq: 23, id: 88 };
emitBoardEvent(nextEv);
// Test 16: WebSocket subscribe and ping/pong message protocol
let mockSent = [];
class MockWebSocket {
  constructor(url) {
    this.url = url;
    this.readyState = 1;
  }
  send(data) {
    mockSent.push(data);
  }
  close() {
    if (this.onclose) this.onclose();
  }
}

const mockWs = new MockWebSocket("ws://localhost:8080/api/ws");
const subPayload = JSON.stringify({ type: "subscribe", last_seq: 15 });
mockWs.send(subPayload);
assert.strictEqual(mockSent.length, 1, "Mock WebSocket send must record sent message");
assert(mockSent[0].includes('"type":"subscribe"') && mockSent[0].includes('"last_seq":15'), "Subscribe message must match required protocol");

const pingPayload = JSON.stringify({ type: "ping" });
const parsedPing = JSON.parse(pingPayload);
assert.strictEqual(parsedPing.type, "ping", "Ping frame type must be ping");

// Test 17: App chrome — Board | Settings sidebar + Settings Sandboxes panel
const sidebarHtml = renderToString(
  React.createElement(PrimarySidebar, {
    view: "board",
    onNavigate: () => {},
  }),
);
assert(sidebarHtml.includes("data-testid=\"app-sidebar\""), "App should render primary sidebar");
assert(sidebarHtml.includes("Board"), "Sidebar should include Board nav");
assert(sidebarHtml.includes("Settings"), "Sidebar should include Settings nav");
assert(sidebarHtml.includes("data-testid=\"nav-board\""), "Sidebar should expose Board control");
assert(sidebarHtml.includes("data-testid=\"nav-settings\""), "Sidebar should expose Settings control");

const settingsHtml = renderToString(React.createElement(Settings));
assert(settingsHtml.includes("data-testid=\"settings\""), "Settings view should render");
assert(settingsHtml.includes("Sandboxes"), "Settings should include Sandboxes section");
assert(settingsHtml.includes("data-testid=\"sandboxes-panel\""), "Settings should show Sandboxes panel");
assert(settingsHtml.includes("Forge"), "Settings should include Forge section");
assert(settingsHtml.includes("data-testid=\"settings-nav-workspace\""), "Settings should nav to Forge (workspace id)");
assert(settingsHtml.includes("OpenShell"), "Settings should include OpenShell section");
assert(settingsHtml.includes("data-testid=\"settings-nav-openshell\""), "Settings should nav to OpenShell");
assert(!settingsHtml.includes("data-testid=\"general-stub\""), "General stub must be gone");
assert(!settingsHtml.includes("settings-stub-tag"), "Forge must not be a stub section");

const openshellHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    status: {
      healthy: true,
      binary: "openshell",
      summary: "Connected\nAuthenticated (mTLS transport)",
      cli_missing: false,
    },
    binaryPath: "",
    onBinaryPathChange: () => {},
    onRefresh: () => {},
    onSaveBinary: () => {},
  }),
);
assert(openshellHtml.includes("data-testid=\"openshell-panel\""), "OpenShell panel should render");
assert(openshellHtml.includes("data-testid=\"openshell-health\""), "OpenShell health block");
assert(openshellHtml.includes("Healthy"), "OpenShell healthy label");
assert(openshellHtml.includes("data-testid=\"openshell-health-summary\""), "OpenShell status summary");
assert(openshellHtml.includes("data-testid=\"openshell-field-binary\""), "OpenShell binary path field");
assert(openshellHtml.includes("data-testid=\"openshell-ops-hint\""), "OpenShell host setup hint");

const openshellMissingHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    status: {
      healthy: false,
      binary: "/missing/openshell",
      summary: "OpenShell CLI not found",
      cli_missing: true,
      error: "No such file",
    },
    binaryPath: "/missing/openshell",
    onBinaryPathChange: () => {},
    onRefresh: () => {},
    onSaveBinary: () => {},
  }),
);
assert(openshellMissingHtml.includes("CLI missing"), "OpenShell CLI-missing label");
assert(
  openshellMissingHtml.includes("data-cli-missing=\"true\""),
  "OpenShell CLI-missing attribute",
);

const openshellUnhealthyHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    status: {
      healthy: false,
      binary: "openshell",
      summary: "gateway unreachable",
      cli_missing: false,
    },
    binaryPath: "",
    onBinaryPathChange: () => {},
    onRefresh: () => {},
    onSaveBinary: () => {},
  }),
);
assert(openshellUnhealthyHtml.includes("Unhealthy"), "OpenShell unhealthy label");

const workspaceHtml = renderToString(
  React.createElement(WorkspacePanelView, {
    draft: {
      forge: "github",
      beads_sync_repo: "",
    },
    onDraftChange: () => {},
    onSave: () => {},
  }),
);
assert(workspaceHtml.includes("data-testid=\"workspace-panel\""), "Forge panel should render");
assert(workspaceHtml.includes("data-testid=\"workspace-form\""), "Forge form should render");
assert(workspaceHtml.includes("data-testid=\"workspace-field-beads\""), "Beads sync field");
assert(workspaceHtml.includes("data-testid=\"workspace-field-forge\""), "Provider field");
assert(!workspaceHtml.includes("data-testid=\"workspace-first-clone-defaults\""), "no first-clone defaults");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-upstream\""), "no upstream field");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-fork\""), "no fork field");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-base\""), "no base field");
assert(workspaceHtml.includes("GitLab (future)"), "GitLab listed as future/disabled");
assert(workspaceHtml.includes("data-testid=\"workspace-webhook-hint\""), "Webhook hint present");
assert(
  workspaceHtml.includes("--repo=<owner/name>") || workspaceHtml.includes("--repo=&lt;owner/name&gt;"),
  "Webhook hint is a repo placeholder template",
);
assert(!workspaceHtml.includes("shanemcd/honr"), "Webhook hint must not hardcode Shane repo");
assert(
  (workspaceHtml.includes("pull_request") || workspaceHtml.includes("pr_url")) && workspaceHtml.includes("not"),
  "Forge copy must say Settings is not the work repo",
);
assert(workspaceHtml.includes("data-testid=\"workspace-save\""), "Forge save control");

const fixtureProfiles = [
  {
    id: "default",
    name: "Default",
    image: "img:1",
    policy: "version: 1\n# default\n",
    cpu: "2",
    memory: "4Gi",
  },
  {
    id: "heavy",
    name: "Heavy",
    image: "img:heavy",
    policy: "version: 1\n# heavy\n",
    cpu: "8",
    memory: "16Gi",
  },
];

const sandboxesHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    profiles: fixtureProfiles,
    defaultId: "default",
    editingId: null,
    draft: { id: "", name: "", image: "", policy: "", cpu: "", memory: "" },
    onDraftChange: () => {},
    onStartCreate: () => {},
    onStartEdit: () => {},
    onCancelEdit: () => {},
    onSave: () => {},
    onSetDefault: () => {},
  }),
);
assert(sandboxesHtml.includes("data-testid=\"sandboxes-panel\""), "Sandboxes panel should render");
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-list\""), "Sandboxes panel should list profiles");
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-default\""), "Should list default profile");
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-heavy\""), "Should list heavy profile");
assert(sandboxesHtml.includes("data-testid=\"sandbox-default-badge\""), "Default profile should be badged");
assert(sandboxesHtml.includes("data-testid=\"sandbox-set-default-heavy\""), "Non-default should offer Set default");
assert(sandboxesHtml.includes("data-testid=\"sandbox-create\""), "Sandboxes panel should support create");
assert(sandboxesHtml.includes("data-testid=\"sandbox-edit-default\""), "Sandboxes panel should support edit");
assert(!sandboxesHtml.includes("data-testid=\"sandbox-destroy\""),
  "Sandboxes panel must not offer live OpenShell sandbox destroy");
assert(!/destroy sandbox|delete environment|openshell.*delete/i.test(sandboxesHtml),
  "Sandboxes panel must not offer live OpenShell sandbox destroy controls");
// List meta should not dump full YAML or imply a host path field.
assert(!sandboxesHtml.includes("version: 1"), "Profile list should not dump inline policy YAML");

const createFormHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    profiles: fixtureProfiles,
    defaultId: "default",
    editingId: "",
    draft: {
      id: "",
      name: "CI",
      image: "img:ci",
      policy: "version: 1\nfilesystem_policy:\n  include_workdir: true\n",
      cpu: "",
      memory: "",
    },
    onDraftChange: () => {},
    onStartCreate: () => {},
    onStartEdit: () => {},
    onCancelEdit: () => {},
    onSave: () => {},
    onSetDefault: () => {},
  }),
);
assert(createFormHtml.includes("data-testid=\"sandbox-profile-form\""), "Create/edit form should render");
assert(!createFormHtml.includes("data-testid=\"sandbox-field-id\""),
  "Create form must not require an Id field (server slugs from name)");
assert(createFormHtml.includes("data-testid=\"sandbox-field-name\""), "Create form should include name");
assert(createFormHtml.includes("data-testid=\"sandbox-field-policy\""), "Form should include policy field");
assert(createFormHtml.includes("<textarea"), "Policy control should be a textarea for inline YAML");
assert(/not a path on the host/i.test(createFormHtml), "Policy hint must not ask for a host filesystem path");
assert(!/policy path|path to.*policy|host path/i.test(createFormHtml),
  "Settings must not ask for a host filesystem policy path");
assert(createFormHtml.includes("data-testid=\"sandbox-save\""), "Form should include save");

const editFormHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    profiles: fixtureProfiles,
    defaultId: "default",
    editingId: "default",
    draft: {
      id: "default",
      name: "Default",
      image: "img",
      policy: "version: 1\n# default\n",
      cpu: "",
      memory: "",
    },
    onDraftChange: () => {},
    onStartCreate: () => {},
    onStartEdit: () => {},
    onCancelEdit: () => {},
    onSave: () => {},
    onSetDefault: () => {},
  }),
);
assert(editFormHtml.includes("data-testid=\"sandbox-field-id\""),
  "Edit form may show id read-only");
assert(editFormHtml.includes("disabled") || editFormHtml.includes("readonly"),
  "Edit id field should be non-editable");

const pickerHtml = renderToString(
  React.createElement(ProjectSandboxPicker, {
    projectId: 42,
    value: null,
    profiles: fixtureProfiles,
    defaultId: "default",
    onChange: () => {},
  }),
);
assert(pickerHtml.includes("data-testid=\"project-sandbox-picker\""), "Project sandbox picker should render");
assert(pickerHtml.includes("data-testid=\"project-sandbox-select-42\""), "Project sandbox select should render");
assert(pickerHtml.includes("Use global default"), "Unset option should read 'Use global default'");
assert(!pickerHtml.includes("Global default ("), "Unset option must not duplicate name as 'Global default (…)'");
assert(pickerHtml.includes("Default · global default"), "Global default profile marked once by name");
assert(pickerHtml.includes("Heavy"), "Named profiles list by display name");
assert(!pickerHtml.includes("Default (default)"), "Must not show 'Default (default)' duplication");
assert(!pickerHtml.includes("Heavy (heavy)"), "Must not show raw id in every option");

// Board view still mounts Cockpit (regression: chrome must not replace it).
const emptyCockpitHtml = renderToString(
  React.createElement(Cockpit, {
    goals: [],
    items: new Map(),
    stories: new Map(),
    goalOf: (id) => id,
    breadcrumbOf: () => "",
    now: Date.now(),
    agentTimeout: 300,
    onOpen: () => {},
  }),
);
assert(emptyCockpitHtml.includes("cockpit") || emptyCockpitHtml.includes("Welcome to honr"),
  "Board view should still render Cockpit");

const pkg = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), "package.json"), "utf8"),
);
assert(!Object.keys(pkg.dependencies || {}).some((d) => /patternfly/i.test(d)),
  "Must not add a PatternFly dependency");
assert(!Object.keys(pkg.devDependencies || {}).some((d) => /patternfly/i.test(d)),
  "Must not add a PatternFly devDependency");

console.log("\n✅ All Card, Cockpit, Detail, Settings chrome, and useBoard sequence guard assertions passed!");

