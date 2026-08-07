import React from "react";
import { renderToString } from "react-dom/server";
import assert from "node:assert";
import { Card } from "./dist-test/components/Card.js";
import { Board, isBlocked, sortFor } from "./dist-test/components/Board.js";
import { Head, PlanEditor, planTasksFromArtifact, reduceDetail } from "./dist-test/components/Detail.js";
import { PrimarySidebar } from "./dist-test/components/PrimarySidebar.js";
import { AccountMenu } from "./dist-test/components/AccountMenu.js";
import {
  Cockpit,
  CockpitAttachView,
  CockpitDrop,
  CockpitSessionView,
  CockpitToggle,
  cockpitAttachGate,
  cockpitAttachRetryDelayMs,
  cockpitChatGate,
} from "./dist-test/components/Cockpit.js";
import { Help } from "./dist-test/components/Help.js";
import { OperatorGuide } from "./dist-test/components/OperatorGuide.js";
import { ProjectSandboxPicker, SandboxesPanelView, Settings, WorkspacePanelView, OpenShellPanelView, OpenShellProvidersPanelView, OpenShellProviderTypesPanelView, AgentRuntimePanelView, OpenShellReadinessStripView, gatewayMtlsReady, sandboxSpecReady } from "./dist-test/components/Settings.js";
import { initial, reduce, isSequenceGap, subscribeBoardEvents, emitBoardEvent } from "./dist-test/useBoard.js";
import {
  chromeLocationsEqual,
  formatChromePath,
  parseChromeLocation,
  writeChromeLocation,
} from "./dist-test/location.js";
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

// Test 7: Board surfaces Needs you action cards with humanized copy
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

const boardHtml = renderToString(
  React.createElement(Board, {
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

console.log("\nBoard HTML:\n", boardHtml.slice(0, 800));
assert(boardHtml.includes("board-needs"), "Board should show Needs you section");
assert(
  boardHtml.includes("Sandbox couldn") && boardHtml.includes("clone"),
  "Board Needs you should humanize clone failures",
);
assert(boardHtml.includes("Investigate the environment"), "Board should offer answer options");

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

// Test 11: Stale REST snapshot race protection (only while connected + small gap)
s = reduce(s, { type: "connected", ok: true });
const beforeStaleLoadAt = s.lastLoadedAt;
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
assert.ok(
  sAfterStaleSnap.lastLoadedAt != null &&
    (beforeStaleLoadAt == null || sAfterStaleSnap.lastLoadedAt >= beforeStaleLoadAt),
  "Successful REST during a tiny race must still refresh lastLoadedAt (retry / NOT LIVE)"
);

// Test 11b: After disconnect (or honr restart seq rewind), REST snapshot wins
const sDisconnected = reduce(sAfterStaleSnap, { type: "connected", ok: false });
const rewoundSnap = {
  items: [{ ...unblockedItem, title: "Post-restart Snapshot" }],
  levels: [],
  goals: [],
  server_time: new Date().toISOString(),
  agent_timeout_secs: 1800,
  seq: 2,
};
const sAfterRewind = reduce(sDisconnected, { type: "snapshot", snap: rewoundSnap });
assert.strictEqual(sAfterRewind.lastSeenSeq, 2, "Disconnected retry must accept rewound server seq");
assert.strictEqual(
  sAfterRewind.items.get(8).title,
  "Post-restart Snapshot",
  "Disconnected retry must apply REST after honr restart"
);

// Test 11c: reset event rewinds high-water mark so later events apply
const sAfterReset = reduce(sAfterStaleSnap, { type: "event", ev: { type: "reset", seq: 3 } });
assert.strictEqual(sAfterReset.lastSeenSeq, 3, "reset event must rewind lastSeenSeq");

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
    pull_request: { url: "https://github.com/honr-app/honr/pull/186" },
    notes: [
      { author: "human", text: "initial note" },
      { author: "agent", text: "PR opened" },
    ],
  },
};

const updatedDetail = reduceDetail(detailInitial, upsertEv, 7);
assert.strictEqual(updatedDetail.title, "Updated Card Title Live", "Upsert event for id 7 must update detail title live");
assert.strictEqual(updatedDetail.state, "review", "Upsert event for id 7 must update detail state live");
assert.strictEqual(updatedDetail.pull_request?.url, "https://github.com/honr-app/honr/pull/186", "Upsert event for id 7 must update pull_request.url live");
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

// Test 17: App chrome — Board | Help in sidebar; Settings in account menu
const sidebarHtml = renderToString(
  React.createElement(PrimarySidebar, {
    view: "board",
    onNavigate: () => {},
  }),
);
assert(sidebarHtml.includes("data-testid=\"app-sidebar\""), "App should render primary sidebar");
assert(sidebarHtml.includes("Board"), "Sidebar should include Board nav");
assert(sidebarHtml.includes("Help"), "Sidebar should include Help nav");
assert(!sidebarHtml.includes("Settings"), "Settings lives in the account menu, not the sidebar");
assert(sidebarHtml.includes("data-testid=\"nav-board\""), "Sidebar should expose Board control");
assert(sidebarHtml.includes("data-testid=\"nav-help\""), "Sidebar should expose Help control");
assert(
  !sidebarHtml.includes("data-testid=\"nav-settings\""),
  "Settings control must not live in the sidebar",
);
assert(
  !sidebarHtml.includes("data-testid=\"nav-cockpit\""),
  "Cockpit must not live in primary nav",
);
assert(!sidebarHtml.includes("Cockpit"), "Sidebar must not list Cockpit");

const accountHtml = renderToString(
  React.createElement(AccountMenu, {
    login: "shanemcd",
    themePref: "dark",
    onThemeChange: () => {},
    onOpenSettings: () => {},
    onLogout: () => {},
    defaultOpen: true,
  }),
);
assert(accountHtml.includes("data-testid=\"auth-user\""), "Account menu trigger shows user");
assert(accountHtml.includes("shanemcd"), "Account menu shows login");
assert(accountHtml.includes("data-testid=\"account-menu\""), "Account menu panel opens");
assert(accountHtml.includes("data-testid=\"nav-settings\""), "Settings lives in the account menu");
assert(accountHtml.includes("data-testid=\"auth-logout\""), "Sign out lives in the account menu");
assert(accountHtml.includes("Theme"), "Account menu includes theme switcher");
assert(accountHtml.includes("Dark"), "Account menu theme select includes Dark");

const toggleClosedHtml = renderToString(
  React.createElement(CockpitToggle, { open: false, onToggle: () => {} }),
);
assert(toggleClosedHtml.includes("data-testid=\"cockpit-toggle\""), "Top bar exposes Cockpit toggle");
assert(toggleClosedHtml.includes("cockpit-bar-btn"), "Toggle uses top-bar grip chrome");
assert(toggleClosedHtml.includes("cockpit-bar-icon"), "Grip uses chevron SVG icon");
assert(toggleClosedHtml.includes("<svg"), "Grip renders an SVG chevron");
assert(
  toggleClosedHtml.includes('aria-expanded="false"') ||
    !toggleClosedHtml.includes('aria-expanded="true"'),
  "Closed toggle is not expanded",
);

const toggleOpenHtml = renderToString(
  React.createElement(CockpitToggle, { open: true, onToggle: () => {} }),
);
assert(toggleOpenHtml.includes("cockpit-bar-btn open"), "Open toggle marks open class");
assert(toggleOpenHtml.includes("cockpit-bar-icon"), "Open grip keeps chevron icon");
assert(
  toggleOpenHtml.includes('aria-expanded="true"') ||
    toggleOpenHtml.includes('aria-expanded=""'),
  "Open toggle is expanded",
);

const dropClosedHtml = renderToString(React.createElement(CockpitDrop, { open: false }));
assert.equal(
  dropClosedHtml,
  "",
  "Drop stays unmounted until first open (collapse later keeps it mounted client-side)",
);

const dropOpenHtml = renderToString(React.createElement(CockpitDrop, { open: true }));
assert(dropOpenHtml.includes("data-testid=\"cockpit-drop\""), "Open drop mounts under the top bar");
assert(dropOpenHtml.includes("data-testid=\"cockpit-pane\""), "Open drop mounts Cockpit pane");
// `open` class is applied after rAF so the slide can run — not present in SSR.

const cockpitHtml = renderToString(React.createElement(Cockpit));
assert(cockpitHtml.includes("data-testid=\"cockpit-pane\""), "Cockpit renders as drop pane");
assert(!cockpitHtml.includes("data-testid=\"cockpit-page\""), "Cockpit is not a separate page");
assert(cockpitHtml.includes("data-testid=\"cockpit-attach\""), "Cockpit should show cockpit attach surface");
assert(cockpitHtml.includes("data-testid=\"cockpit-term-window\""), "Cockpit should show terminal chrome");
assert(cockpitHtml.includes("data-testid=\"cockpit-xterm\""), "Cockpit should mount xterm host");
assert(cockpitHtml.includes("data-testid=\"cockpit-session\""), "Cockpit should show Start/Stop strip");
assert(cockpitHtml.includes("data-testid=\"cockpit-session-start\""), "Cockpit should expose Start");
assert(cockpitHtml.includes("data-testid=\"cockpit-session-stop\""), "Cockpit should expose Stop");
assert(!cockpitHtml.includes("data-testid=\"cockpit-open-cursor\""), "Cockpit should not shell out Open in Cursor");
assert(!cockpitHtml.includes("data-testid=\"cockpit-mcp-provision\""), "Cockpit should not expose Refresh MCP");
assert(!cockpitHtml.includes("data-testid=\"cockpit-mcp-status\""), "Cockpit should not dump MCP status");
assert(!cockpitHtml.includes("data-testid=\"cockpit-session-status\""), "Cockpit should not dump session status");
assert(!cockpitHtml.includes("data-testid=\"cockpit-session-park\""), "Cockpit should not expose Park");
assert(!cockpitHtml.includes("data-testid=\"cockpit-session-resume\""), "Cockpit should not expose Resume");
assert(!cockpitHtml.includes("/api/cockpit-attach"), "Cockpit should not show attach API lede");
assert(
  cockpitHtml.indexOf("data-testid=\"cockpit-session\"") <
    cockpitHtml.indexOf("data-testid=\"cockpit-attach\""),
  "Start/Stop precede the attach window in the drop",
);

// CockpitSessionView — Start/Stop only; no status dump
const noop = () => {};
const buttonTag = (html, testId) => {
  // React SSR may place attrs in any order — match the whole opening tag.
  const all = html.match(/<button\b[^>]*>/g) || [];
  const tag = all.find((t) => t.includes(`data-testid="${testId}"`));
  assert(tag, `missing button ${testId}`);
  return tag;
};
const isDisabled = (html, testId) => /\bdisabled\b/.test(buttonTag(html, testId));

const absentHtml = renderToString(
  React.createElement(CockpitSessionView, {
    session: null,
    onStart: noop,
    onStop: noop,
  }),
);
assert(!absentHtml.includes("data-testid=\"cockpit-session-status\""), "No status dump when absent");
assert(!isDisabled(absentHtml, "cockpit-session-start"), "Start enabled when no session");
assert(isDisabled(absentHtml, "cockpit-session-stop"), "Stop disabled when no session");

const runningSession = {
  environment: "honr-cockpit",
  conversation_id: "conv-cockpit-1",
  status: "running",
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
};
const runningHtml = renderToString(
  React.createElement(CockpitSessionView, {
    session: runningSession,
    onStart: noop,
    onStop: noop,
  }),
);
assert(!runningHtml.includes("honr-cockpit"), "Session strip does not dump environment");
assert(!runningHtml.includes("conv-cockpit-1"), "Session strip does not dump conversation_id");
assert(isDisabled(runningHtml, "cockpit-session-start"), "Start disabled when Running");
assert(!isDisabled(runningHtml, "cockpit-session-stop"), "Stop enabled when Running");
assert(!runningHtml.includes("data-testid=\"cockpit-open-cursor\""), "No Open in Cursor button");
assert(!runningHtml.includes("openshell sandbox connect"), "No host TTY hint in Cockpit");

const parkedSession = {
  ...runningSession,
  status: "parked",
  conversation_id: null,
};
const parkedHtml = renderToString(
  React.createElement(CockpitSessionView, {
    session: parkedSession,
    onStart: noop,
    onStop: noop,
  }),
);
assert(isDisabled(parkedHtml, "cockpit-session-start"), "Start disabled while a session exists");
assert(!isDisabled(parkedHtml, "cockpit-session-stop"), "Stop enabled when Parked");
assert(!parkedHtml.includes("data-testid=\"cockpit-session-park\""), "No Park control");
assert(!parkedHtml.includes("data-testid=\"cockpit-session-resume\""), "No Resume control");

// Cockpit attach — gated by Board session; xterm host present when attachable
const attachAbsent = renderToString(
  React.createElement(CockpitAttachView, {
    canAttach: false,
    disabledReason: "Start a cockpit session to open the seat.",
  }),
);
assert(attachAbsent.includes("data-testid=\"cockpit-attach\""), "Attach root");
assert(attachAbsent.includes("data-testid=\"cockpit-term-window\""), "Terminal chrome");
assert(attachAbsent.includes("data-testid=\"cockpit-attach-gate\""), "Gate copy when disabled");
assert(attachAbsent.includes("Start a cockpit session"), "Absent gate explains Start");

const attachParked = renderToString(
  React.createElement(CockpitAttachView, {
    canAttach: false,
    disabledReason: "Cockpit session is parked. Stop it, then Start again.",
  }),
);
assert(attachParked.includes("Stop it, then Start again"), "Parked gate explains Stop+Start");

const attachRunning = renderToString(
  React.createElement(CockpitAttachView, {
    canAttach: true,
    disabledReason: null,
    environment: "honr-cockpit",
    sessionStatus: "running",
  }),
);
assert(attachRunning.includes("data-testid=\"cockpit-xterm\""), "xterm host present");
assert(attachRunning.includes("honr-cockpit"), "Title bar shows environment");
assert(!attachRunning.includes("data-testid=\"cockpit-attach-gate\""), "No gate when attachable");

// Attach reconnect backoff (honr restart must not stick on a dead socket)
assert.equal(cockpitAttachRetryDelayMs(0), 1000);
assert.equal(cockpitAttachRetryDelayMs(1), 2000);
assert.equal(cockpitAttachRetryDelayMs(5), 15_000);
assert.equal(cockpitAttachRetryDelayMs(9), 15_000, "backoff caps at 15s");

// Gate helpers
assert.deepEqual(cockpitAttachGate(null), {
  canAttach: false,
  reason: "Start a cockpit session to open the seat.",
});
assert.equal(cockpitAttachGate(parkedSession).canAttach, false);
assert.match(cockpitAttachGate(parkedSession).reason, /Stop it, then Start again/);
assert.equal(cockpitAttachGate(runningSession).canAttach, true);
assert.equal(cockpitAttachGate(runningSession).reason, null);
assert.equal(
  cockpitAttachGate({ ...runningSession, environment: null }).canAttach,
  false,
);
// Legacy alias
assert.equal(cockpitChatGate(runningSession).canSend, true);

const helpHtml = renderToString(React.createElement(Help));
assert(helpHtml.includes("data-testid=\"help-page\""), "Help view should render");
assert(helpHtml.includes("data-testid=\"operator-guide\""), "Help embeds OperatorGuide");
assert(helpHtml.includes("data-testid=\"operator-guide-quickstart\""), "Help shows OperatorGuide Quickstart");
assert(helpHtml.includes("data-testid=\"operator-guide-mcp\""), "Help shows OperatorGuide MCP section");
assert(helpHtml.includes("data-testid=\"operator-guide-openshell\""), "Help shows OperatorGuide OpenShell section");
assert(helpHtml.includes("create_project"), "Help should document create_project");
assert(helpHtml.includes("clone_repo"), "Help should document clone_repo");
assert(helpHtml.includes("plan.json"), "Help should document plan.json");
assert(helpHtml.includes("Approve"), "Help should document Approve");
assert(helpHtml.includes("dispatch"), "Help should document dispatch");
assert(helpHtml.includes("http://127.0.0.1:8080/mcp"), "Help should show MCP URL");
assert(helpHtml.includes("Streamable HTTP"), "Help should name Streamable HTTP transport");
assert(helpHtml.includes("Quickstart"), "Help hero names Quickstart as a Help job");
assert(helpHtml.includes("Connect MCP"), "Help hero names Connect MCP as a Help job");
// Help surface order: Quickstart pillar before MCP pillar.
{
  const helpQuickstartIdx = helpHtml.indexOf("data-testid=\"operator-guide-quickstart\"");
  const helpMcpIdx = helpHtml.indexOf("data-testid=\"operator-guide-mcp\"");
  assert(
    helpQuickstartIdx >= 0 && helpMcpIdx > helpQuickstartIdx,
    "Help orders Quickstart before MCP",
  );
}

// OperatorGuide — Quickstart → MCP → OpenShell/sandbox (Board empty / Help)
const guideHtml = renderToString(React.createElement(OperatorGuide));
assert(guideHtml.includes("data-testid=\"operator-guide\""), "OperatorGuide root testid");
assert(guideHtml.includes("data-testid=\"operator-guide-quickstart\""), "OperatorGuide Quickstart section");
assert(guideHtml.includes("data-testid=\"operator-guide-quickstart-steps\""), "OperatorGuide Quickstart steps");
assert(guideHtml.includes("data-testid=\"operator-guide-mcp\""), "OperatorGuide MCP section");
assert(guideHtml.includes("data-testid=\"operator-guide-openshell\""), "OperatorGuide OpenShell/sandbox section");
assert(guideHtml.includes("data-testid=\"operator-guide-client-examples\""), "OperatorGuide client examples are secondary");
assert(guideHtml.includes("data-testid=\"operator-guide-mcp-url\""), "OperatorGuide copyable MCP URL");
assert(guideHtml.includes("data-testid=\"operator-guide-cursor-snippet\""), "OperatorGuide Cursor snippet");
assert(guideHtml.includes("data-testid=\"operator-guide-claude-snippet\""), "OperatorGuide Claude snippet");
assert(guideHtml.includes("http://127.0.0.1:8080/mcp"), "OperatorGuide shows MCP endpoint");
assert(guideHtml.includes("Streamable HTTP"), "OperatorGuide names Streamable HTTP transport");
assert(guideHtml.includes("create_project"), "OperatorGuide documents create_project");
assert(guideHtml.includes("clone_repo"), "OperatorGuide documents clone_repo");
assert(guideHtml.includes("plan.json"), "OperatorGuide documents plan.json");
assert(guideHtml.includes("Approve"), "OperatorGuide documents Approve");
assert(guideHtml.includes("idle"), "OperatorGuide notes agents stay idle until enable+dispatch");
assert(guideHtml.includes("claude mcp add"), "OperatorGuide has Claude mcp add example");
assert(guideHtml.includes("mcp.json"), "OperatorGuide has Cursor mcp.json example");
assert(guideHtml.includes("OpenShell + sandbox"), "OperatorGuide OpenShell section title");
assert(guideHtml.includes("/settings/openshell/connectivity"), "OpenShell deep link: Connectivity");
assert(guideHtml.includes("/settings/openshell/providers"), "OpenShell deep link: Providers");
assert(guideHtml.includes("/settings/openshell/profiles"), "OpenShell deep link: Sandbox specs");
assert(guideHtml.includes("/settings/agent-runtime"), "OpenShell deep link: Agent runtime");
assert(guideHtml.includes("/settings/github-app"), "OpenShell deep link: GitHub App for GH_TOKEN");
assert(guideHtml.includes("GH_TOKEN"), "OperatorGuide mentions GH_TOKEN via GitHub App");
assert(guideHtml.includes("Sandbox specs"), "OperatorGuide names Sandbox specs tab");
assert(guideHtml.includes("mTLS"), "OperatorGuide mentions mTLS on Connectivity");
// Order: Quickstart → MCP (with examples) → OpenShell/sandbox.
const quickstartIdx = guideHtml.indexOf("data-testid=\"operator-guide-quickstart\"");
const mcpIdx = guideHtml.indexOf("data-testid=\"operator-guide-mcp\"");
const openshellIdx = guideHtml.indexOf("data-testid=\"operator-guide-openshell\"");
const examplesIdx = guideHtml.indexOf("data-testid=\"operator-guide-client-examples\"");
assert(
  quickstartIdx >= 0 && mcpIdx > quickstartIdx,
  "OperatorGuide leads with Quickstart before MCP",
);
assert(
  openshellIdx > mcpIdx,
  "OpenShell/sandbox follows MCP (after the two Help pillars)",
);
assert(examplesIdx > mcpIdx && examplesIdx < openshellIdx, "Client examples sit under MCP, before OpenShell");

const settingsHtml = renderToString(React.createElement(Settings));
assert(settingsHtml.includes("data-testid=\"settings\""), "Settings view should render");
assert(!settingsHtml.includes("data-testid=\"settings-nav-sandboxes\""), "Sandboxes nav item removed");
assert(settingsHtml.includes("data-testid=\"settings-nav-openshell\""), "Settings should nav to OpenShell");
assert(settingsHtml.includes("data-testid=\"openshell-panel\""), "Default section is OpenShell");
assert(settingsHtml.includes("data-testid=\"openshell-subnav\""), "OpenShell has section subnav");
assert(settingsHtml.includes("data-testid=\"openshell-tab-profiles\""), "OpenShell tab for Profiles");
assert(settingsHtml.includes("data-testid=\"openshell-connectivity\""), "Default OpenShell tab is Connectivity");
assert(settingsHtml.includes("Connectivity"), "Settings OpenShell names Connectivity");
assert(settingsHtml.includes("Forge"), "Settings should include Forge section");
assert(settingsHtml.includes("data-testid=\"settings-nav-workspace\""), "Settings should nav to Forge (workspace id)");
assert(settingsHtml.includes("OpenShell"), "Settings should include OpenShell section");
assert(settingsHtml.includes("Agent runtime"), "Settings should include Agent runtime section");
assert(settingsHtml.includes("data-testid=\"settings-nav-agent-runtime\""), "Settings should nav to Agent runtime");
assert(!settingsHtml.includes("data-testid=\"general-stub\""), "General stub must be gone");
assert(!settingsHtml.includes("settings-stub-tag"), "Forge must not be a stub section");

const agentRuntimeHtml = renderToString(
  React.createElement(AgentRuntimePanelView, {
    draft: {
      engine: "agy",
      max_concurrent: 1,
      agent_timeout_secs: 1800,
      max_attempts: 3,
      branch_prefix: "honr",
      sweep_interval_ms: 2000,
    },
    onDraftChange: () => {},
    onSave: () => {},
  }),
);
assert(agentRuntimeHtml.includes("data-testid=\"agent-runtime-panel\""), "Agent runtime panel should render");
assert(agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-engine\""), "Agent runtime engine field");
assert(!agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-enabled\""), "Agents enabled checkbox removed");
assert(!agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-providers\""), "Providers field removed");
assert(!agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-vertex-location\""), "Vertex fields removed");
assert(!agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-quality-gates\""), "Quality gates removed");
assert(agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-branch-prefix\""), "Agent runtime branch prefix");
assert(agentRuntimeHtml.includes("data-testid=\"agent-runtime-field-sweep\""), "Agent runtime sweep interval");
assert(agentRuntimeHtml.includes("data-testid=\"agent-runtime-save\""), "Agent runtime save control");

const openshellPanelProps = {
  gatewayEndpoint: "https://127.0.0.1:17670",
  caPem: "",
  clientCertPem: "",
  clientKeyPem: "",
  mtls: { ca: false, client_cert: false, client_key: false, complete: false },
  onGatewayEndpointChange: () => {},
  onCaPemChange: () => {},
  onClientCertPemChange: () => {},
  onClientKeyPemChange: () => {},
  onRefresh: () => {},
  onSave: () => {},
  onImportCliMtls: () => {},
  onClearMtls: () => {},
};

const openshellHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    ...openshellPanelProps,
    status: {
      healthy: true,
      summary: "Connected\nAuthenticated (mTLS transport)",
      not_configured: false,
    },
  }),
);
assert(openshellHtml.includes("data-testid=\"openshell-panel\""), "OpenShell panel should render");
assert(openshellHtml.includes("data-testid=\"openshell-connectivity\""), "Connectivity band wrapper");
assert(
  openshellHtml.includes(">Connectivity<") || openshellHtml.includes("Connectivity</h3>"),
  "Connectivity heading",
);
assert(openshellHtml.includes("data-testid=\"openshell-health\""), "OpenShell health block");
assert(openshellHtml.includes("Healthy"), "OpenShell healthy label");
assert(openshellHtml.includes("data-testid=\"openshell-health-summary\""), "OpenShell status summary");
assert(openshellHtml.includes("data-testid=\"openshell-field-endpoint\""), "OpenShell gateway endpoint field");
assert(openshellHtml.includes("data-testid=\"openshell-field-ca\""), "OpenShell CA PEM field");
assert(!openshellHtml.includes("data-testid=\"openshell-field-binary\""), "OpenShell must not expose CLI binary path");
assert(!openshellHtml.includes("openshell-health-bin"), "Legacy binary health CSS class removed");
assert(openshellHtml.includes("data-testid=\"openshell-subnav\""), "OpenShell subnav for sections");
assert(
  openshellHtml.includes("data-testid=\"openshell-tab-connectivity\"") &&
    openshellHtml.includes("data-testid=\"openshell-tab-providers\"") &&
    openshellHtml.includes("data-testid=\"openshell-tab-provider-types\"") &&
    openshellHtml.includes("data-testid=\"openshell-tab-profiles\""),
  "OpenShell tabs: Connectivity / Providers / Provider types / Sandbox specs",
);

const openshellUnhealthyHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    ...openshellPanelProps,
    status: {
      healthy: false,
      summary: "gateway unreachable",
      not_configured: false,
    },
  }),
);
assert(openshellUnhealthyHtml.includes("Unhealthy"), "OpenShell unhealthy label");

const openshellProvidersHtml = renderToString(
  React.createElement(OpenShellProvidersPanelView, {
    providers: [
      {
        name: "gh-clankr",
        type: "github",
        config: {},
        credential_keys: ["GH_TOKEN"],
        has_credentials: true,
        has_refresh: false,
        gateway_synced: true,
      },
    ],
    gatewayReachable: true,
    profiles: [
      {
        id: "github",
        display_name: "GitHub",
        description: "",
        category: "scm",
        credential_env_vars: ["GH_TOKEN"],
        config_keys: [],
      },
    ],
    draft: null,
    onDraftChange: () => {},
    onSave: () => {},
    onCancelEdit: () => {},
    onEdit: () => {},
    onDelete: () => {},
    onSync: () => {},
  }),
);
assert(openshellProvidersHtml.includes("data-testid=\"openshell-providers\""), "Providers band renders");
assert(
  openshellProvidersHtml.includes(">Providers<") || openshellProvidersHtml.includes("Providers</h3>"),
  "Providers heading",
);
assert(openshellProvidersHtml.includes("data-testid=\"openshell-provider-gh-clankr\""), "Provider row renders");
assert(openshellProvidersHtml.includes("data-testid=\"openshell-providers-sync\""), "Sync all control");
assert(!openshellProvidersHtml.includes("openshell-providers-import-adc"), "Import ADC control removed");
assert(openshellProvidersHtml.includes("on gateway"), "Gateway sync badge");
assert(!openshellProvidersHtml.includes("sk-"), "Providers view must not echo secrets");
assert(
  openshellProvidersHtml.includes("Settings → GitHub App"),
  "Providers intro points at GitHub App for tokens",
);
assert(
  !openshellProvidersHtml.includes("openshell-provider-attach-"),
  "Attach toggles live on Sandbox specs, not Providers",
);
assert(
  openshellProvidersHtml.includes("Sandbox spec"),
  "Providers copy points attach to Sandbox specs",
);

const openshellManagedGithubHtml = renderToString(
  React.createElement(OpenShellProvidersPanelView, {
    providers: [
      {
        name: "github",
        type: "github",
        config: {},
        credential_keys: ["GH_TOKEN"],
        has_credentials: true,
        has_refresh: false,
        gateway_synced: true,
      },
    ],
    gatewayReachable: true,
    profiles: [],
    draft: {
      name: "github",
      type: "github",
      config: {},
      credentials: {},
    },
    onDraftChange: () => {},
    onSave: () => {},
    onCancelEdit: () => {},
    onEdit: () => {},
    onDelete: () => {},
    onSync: () => {},
  }),
);
assert(
  openshellManagedGithubHtml.includes("data-testid=\"openshell-provider-managed-github\""),
  "Managed github row marks App source",
);
assert(
  openshellManagedGithubHtml.includes("secrets: GH_TOKEN · GitHub App"),
  "Managed github row lists attached env vars like other providers",
);
assert(
  openshellManagedGithubHtml.includes("data-testid=\"openshell-provider-cred-GH_TOKEN\""),
  "Managed github form still shows GH_TOKEN field",
);
assert(
  openshellManagedGithubHtml.includes("data-testid=\"openshell-provider-app-managed-note\""),
  "Managed github field hint points at GitHub App mint",
);
assert(
  openshellManagedGithubHtml.includes("Edit"),
  "Managed github keeps Edit control like other providers",
);

const openshellProvidersEmptyHtml = renderToString(
  React.createElement(OpenShellProvidersPanelView, {
    providers: [],
    gatewayReachable: false,
    profiles: [],
    draft: null,
    onDraftChange: () => {},
    onSave: () => {},
    onCancelEdit: () => {},
    onEdit: () => {},
    onDelete: () => {},
    onSync: () => {},
  }),
);
assert(openshellProvidersEmptyHtml.includes("data-testid=\"openshell-providers-empty\""), "Empty providers state");
assert(openshellProvidersEmptyHtml.includes("gateway offline"), "Offline gateway badge");

const openshellCursorAgentHtml = renderToString(
  React.createElement(OpenShellProvidersPanelView, {
    providers: [],
    gatewayReachable: true,
    profiles: [
      {
        id: "cursor-agent",
        display_name: "Cursor Agent",
        description: "CURSOR_API_KEY",
        source: "board",
        credential_env_vars: ["CURSOR_API_KEY"],
        form_config_keys: [],
      },
    ],
    draft: {
      name: "cursor-cli",
      type: "cursor-agent",
      config: {},
      credentials: {},
    },
    onDraftChange: () => {},
    onSave: () => {},
    onCancelEdit: () => {},
    onEdit: () => {},
    onDelete: () => {},
    onSync: () => {},
  }),
);
assert(
  openshellCursorAgentHtml.includes("data-testid=\"openshell-provider-cred-CURSOR_API_KEY\""),
  "cursor-agent type renders CURSOR_API_KEY credential field",
);

const openshellProviderTypesHtml = renderToString(
  React.createElement(OpenShellProviderTypesPanelView, {
    types: [
      {
        id: "cursor-agent",
        display_name: "Cursor Agent",
        description: "",
        source: "board",
        credential_env_vars: ["CURSOR_API_KEY"],
        form_config_keys: [],
        yaml: "id: cursor-agent\n",
        shipped: true,
      },
    ],
    draft: null,
    editingId: null,
    onDraftChange: () => {},
    onSave: () => {},
    onCancelEdit: () => {},
    onEdit: () => {},
    onDelete: () => {},
    onAdd: () => {},
  }),
);
assert(
  openshellProviderTypesHtml.includes("data-testid=\"openshell-provider-types\""),
  "Provider types band renders",
);
assert(
  openshellProviderTypesHtml.includes("data-testid=\"openshell-provider-type-cursor-agent\""),
  "Shipped cursor-agent type row renders",
);
assert(
  openshellProviderTypesHtml.includes("shipped"),
  "Shipped badge on provider type row",
);

const openshellWithBandsHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    ...openshellPanelProps,
    activeTab: "providers",
    status: {
      healthy: true,
      summary: "ok",
      not_configured: false,
    },
    providers: React.createElement("div", { "data-testid": "openshell-providers-slot" }, "providers"),
    profiles: React.createElement("div", { "data-testid": "openshell-profiles-slot" }, "profiles"),
  }),
);
assert(openshellWithBandsHtml.includes("data-testid=\"openshell-providers-slot\""), "Providers tab hosts providers slot");
assert(!openshellWithBandsHtml.includes("data-testid=\"openshell-profiles-slot\""), "Profiles slot hidden off-tab");
assert(!openshellWithBandsHtml.includes("data-testid=\"openshell-connectivity\""), "Connectivity pane hidden off-tab");

const openshellProfilesTabHtml = renderToString(
  React.createElement(OpenShellPanelView, {
    ...openshellPanelProps,
    activeTab: "profiles",
    status: { healthy: true, summary: "ok", not_configured: false },
    providers: React.createElement("div", { "data-testid": "openshell-providers-slot" }, "providers"),
    profiles: React.createElement("div", { "data-testid": "openshell-profiles-slot" }, "profiles"),
  }),
);
assert(openshellProfilesTabHtml.includes("data-testid=\"openshell-profiles-slot\""), "Profiles tab hosts profiles slot");

const workspaceHtml = renderToString(
  React.createElement(WorkspacePanelView, {
    draft: {
      forge: "github",
    },
    poll: {
      enabled: false,
      interval_secs: 60,
    },
    onDraftChange: () => {},
    onPollChange: () => {},
    onSave: () => {},
  }),
);
assert(workspaceHtml.includes("data-testid=\"workspace-panel\""), "Forge panel should render");
assert(workspaceHtml.includes("data-testid=\"workspace-form\""), "Forge form should render");
assert(workspaceHtml.includes("data-testid=\"workspace-field-forge\""), "Provider field");
assert(workspaceHtml.includes("data-testid=\"workspace-poll\""), "Poll fallback controls");
assert(workspaceHtml.includes("data-testid=\"workspace-poll-enabled\""), "Poll enabled checkbox");
assert(workspaceHtml.includes("data-testid=\"workspace-poll-interval\""), "Poll interval field");
assert(!workspaceHtml.includes("data-testid=\"workspace-first-clone-defaults\""), "no first-clone defaults");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-upstream\""), "no upstream field");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-fork\""), "no fork field");
assert(!workspaceHtml.includes("data-testid=\"workspace-field-base\""), "no base field");
assert(workspaceHtml.includes("GitLab (future)"), "GitLab listed as future/disabled");
assert(!workspaceHtml.includes("data-testid=\"workspace-webhook-hint\""), "no gh webhook forward hint");
assert(!workspaceHtml.includes("gh webhook forward"), "no gh webhook forward copy");
assert(!workspaceHtml.includes("honr-app/honr"), "Forge panel must not hardcode Shane repo");
assert(
  workspaceHtml.includes("pull_request") || workspaceHtml.includes("Work remotes"),
  "Forge copy must mention card pull_request / work remotes",
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
    engine: "cursor",
  },
  {
    id: "heavy",
    name: "Heavy",
    image: "img:heavy",
    policy: "version: 1\n# heavy\n",
    cpu: "8",
    memory: "16Gi",
    engine: "agy",
  },
];

const sandboxPanelBase = {
  profiles: fixtureProfiles,
  defaultId: "default",
  cockpitId: "default",
  availableProviders: [
    {
      name: "vertex",
      type: "google-vertex-ai",
      config: {},
      credential_keys: [],
      has_credentials: true,
      has_refresh: false,
      gateway_synced: true,
    },
  ],
  selectedId: "default",
  editingId: null,
  draft: {
    id: "",
    name: "",
    image: "",
    policy: "",
    cpu: "",
    memory: "",
    engine: "cursor",
    provider_names: [],
  },
  onSelect: () => {},
  onDraftChange: () => {},
  onStartCreate: () => {},
  onStartEdit: () => {},
  onCancelEdit: () => {},
  onSave: () => {},
  onDelete: () => {},
  onSetDefault: () => {},
  onSetCockpit: () => {},
};

const sandboxesHtml = renderToString(
  React.createElement(SandboxesPanelView, sandboxPanelBase),
);
assert(sandboxesHtml.includes("data-testid=\"openshell-profiles\""), "Sandbox specs band wrapper");
assert(sandboxesHtml.includes("data-testid=\"sandboxes-panel\""), "Sandbox specs panel should render");
assert(
  sandboxesHtml.includes(">Sandbox specs<") || sandboxesHtml.includes("Sandbox specs</h3>"),
  "Sandbox specs heading",
);
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-list\""), "Sandbox specs panel should list specs");
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-default\""), "Should list default profile");
assert(sandboxesHtml.includes("data-testid=\"sandbox-profile-heavy\""), "Should list heavy profile");
assert(sandboxesHtml.includes("data-testid=\"sandbox-default-badge\""), "Default profile should be badged");
assert(sandboxesHtml.includes("data-testid=\"sandbox-cockpit-badge\""), "Cockpit profile should be badged");
assert(sandboxesHtml.includes("data-testid=\"sandbox-create\""), "Sandbox specs panel should support create");
assert(sandboxesHtml.includes("data-testid=\"sandbox-edit-default\""), "Selected profile offers Edit");
assert(sandboxesHtml.includes("cursor"), "Selected profile shows engine");
assert(sandboxesHtml.includes("data-testid=\"sandbox-delete-default\""), "Default profile can be deleted");
assert(!sandboxesHtml.includes("data-testid=\"sandbox-destroy\""),
  "Sandbox specs panel must not offer live OpenShell sandbox destroy");
assert(!/destroy sandbox|delete environment/i.test(sandboxesHtml),
  "Sandbox specs panel must not offer live OpenShell sandbox destroy controls");

const sandboxesHeavyHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    ...sandboxPanelBase,
    cockpitId: "default",
    selectedId: "heavy",
  }),
);
assert(sandboxesHeavyHtml.includes("data-testid=\"sandbox-set-default-heavy\""), "Non-default offers Set default");
assert(sandboxesHeavyHtml.includes("data-testid=\"sandbox-set-cockpit-heavy\""),
  "Non-Cockpit offers Use for Cockpit");
assert(sandboxesHeavyHtml.includes("data-testid=\"sandbox-delete-heavy\""), "Deletable profile offers Delete");

const createFormHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    ...sandboxPanelBase,
    cockpitId: "cockpit",
    editingId: "",
    draft: {
      id: "",
      name: "CI",
      image: "img:ci",
      policy: "version: 1\nfilesystem_policy:\n  include_workdir: true\n",
      cpu: "",
      memory: "",
      engine: "cursor",
      provider_names: ["vertex"],
    },
  }),
);
assert(createFormHtml.includes("data-testid=\"sandbox-profile-form\""), "Create/edit form should render");
assert(!createFormHtml.includes("data-testid=\"sandbox-field-id\""),
  "Create form must not require an Id field (server slugs from name)");
assert(createFormHtml.includes("data-testid=\"sandbox-field-name\""), "Create form should include name");
assert(createFormHtml.includes("data-testid=\"sandbox-field-engine\""), "Form should include engine field");
assert(createFormHtml.includes("data-testid=\"sandbox-field-policy\""), "Form should include policy field");
assert(createFormHtml.includes("data-testid=\"sandbox-field-providers\""), "Form includes per-profile providers");
assert(createFormHtml.includes("data-testid=\"sandbox-provider-vertex\""), "Form lists available providers");
assert(createFormHtml.includes("<textarea"), "Policy control should be a textarea for inline YAML");
assert(!/policy path|path to.*policy|host path/i.test(createFormHtml),
  "Settings must not ask for a host filesystem policy path");
assert(createFormHtml.includes("data-testid=\"sandbox-save\""), "Form should include save");

const editFormHtml = renderToString(
  React.createElement(SandboxesPanelView, {
    ...sandboxPanelBase,
    cockpitId: null,
    editingId: "default",
    draft: {
      id: "default",
      name: "Default",
      image: "img",
      policy: "version: 1\n# default\n",
      cpu: "",
      memory: "",
      engine: "cursor",
      provider_names: ["vertex"],
    },
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

// Board view still mounts Board (regression: chrome must not replace it).
const emptyBoardHtml = renderToString(
  React.createElement(Board, {
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
assert(emptyBoardHtml.includes("board-page") || emptyBoardHtml.includes("Welcome to honr"),
  "Board view should still render Board");
assert(emptyBoardHtml.includes("Welcome to honr"), "Board empty keeps Welcome hero");
assert(emptyBoardHtml.includes("data-testid=\"board-empty\""), "Board empty shell testid");
assert(emptyBoardHtml.includes("data-testid=\"operator-guide\""), "Board empty embeds OperatorGuide");
assert(emptyBoardHtml.includes("data-testid=\"operator-guide-quickstart\""), "Board empty shows Quickstart section");
assert(emptyBoardHtml.includes("data-testid=\"operator-guide-mcp\""), "Board empty shows MCP section");
assert(emptyBoardHtml.includes("data-testid=\"operator-guide-openshell\""), "Board empty shows OpenShell section");
assert(emptyBoardHtml.includes("clone_repo"), "Board empty documents clone_repo");
assert(emptyBoardHtml.includes("plan.json"), "Board empty documents plan.json");
assert(emptyBoardHtml.includes("Approve"), "Board empty documents Approve");
assert(emptyBoardHtml.includes("http://127.0.0.1:8080/mcp"), "Board empty shows MCP URL");
assert(emptyBoardHtml.includes("Streamable HTTP"), "Board empty names Streamable HTTP transport");
assert(emptyBoardHtml.includes("OpenShell and sandbox"), "Board Welcome lede mentions OpenShell/sandbox");
assert(emptyBoardHtml.includes("/settings/openshell/connectivity"), "Board empty deep-links Connectivity");
assert(emptyBoardHtml.includes("/settings/agent-runtime"), "Board empty deep-links Agent runtime");
assert(emptyBoardHtml.includes("data-testid=\"openshell-readiness\""), "Board empty shows OpenShell readiness strip");
assert(emptyBoardHtml.includes("data-testid=\"openshell-readiness-gateway\""), "Board empty readiness: gateway row");
assert(emptyBoardHtml.includes("data-testid=\"openshell-readiness-sandbox\""), "Board empty readiness: sandbox row");
assert(!emptyBoardHtml.includes("data-testid=\"openshell-readiness-agents\""), "Board empty readiness: no agents-enabled row");
// Board empty shares OperatorGuide order: Quickstart → MCP.
{
  const boardQuickstartIdx = emptyBoardHtml.indexOf("data-testid=\"operator-guide-quickstart\"");
  const boardMcpIdx = emptyBoardHtml.indexOf("data-testid=\"operator-guide-mcp\"");
  assert(
    boardQuickstartIdx >= 0 && boardMcpIdx > boardQuickstartIdx,
    "Board empty orders Quickstart before MCP",
  );
}

// OpenShell readiness strip — presentational ready / not-ready fixtures
assert.strictEqual(
  gatewayMtlsReady({
    healthy: true,
    summary: "Connected",
    not_configured: false,
    mtls: { ca: true, client_cert: true, client_key: true, complete: true },
  }),
  true,
  "gatewayMtlsReady when healthy + complete mTLS",
);
assert.strictEqual(
  gatewayMtlsReady({
    healthy: true,
    summary: "Connected",
    not_configured: false,
    mtls: { ca: true, client_cert: false, client_key: false, complete: false },
  }),
  false,
  "gatewayMtlsReady fails closed on incomplete mTLS",
);
assert.strictEqual(
  gatewayMtlsReady({
    healthy: false,
    summary: "unreachable",
    not_configured: false,
    mtls: { ca: true, client_cert: true, client_key: true, complete: true },
  }),
  false,
  "gatewayMtlsReady fails closed when unhealthy",
);
assert.strictEqual(
  gatewayMtlsReady({
    healthy: false,
    summary: "not configured",
    not_configured: true,
    mtls: { ca: false, client_cert: false, client_key: false, complete: false },
  }),
  false,
  "gatewayMtlsReady fails closed when not_configured",
);
assert.strictEqual(gatewayMtlsReady(null), false, "gatewayMtlsReady fails closed on null");
assert.strictEqual(
  sandboxSpecReady({
    profiles: [{ id: "default", name: "Default", image: "honr-sandbox:latest", policy: "" }],
    default_sandbox_profile_id: "default",
    cockpit_sandbox_profile_id: null,
  }),
  true,
  "sandboxSpecReady when default profile set",
);
assert.strictEqual(
  sandboxSpecReady({
    profiles: [],
    default_sandbox_profile_id: null,
    cockpit_sandbox_profile_id: null,
  }),
  false,
  "sandboxSpecReady fails closed without default",
);
assert.strictEqual(sandboxSpecReady(null), false, "sandboxSpecReady fails closed on null");
const readinessReadyHtml = renderToString(
  React.createElement(OpenShellReadinessStripView, {
    gateway: { ready: true, detail: "Connected" },
    sandbox: { ready: true, detail: "Default: Default" },
  }),
);
assert(readinessReadyHtml.includes("data-testid=\"openshell-readiness\""), "Readiness strip root");
assert(readinessReadyHtml.includes("data-ready=\"true\""), "Ready strip marks rows ready");
assert(readinessReadyHtml.includes("data-testid=\"openshell-readiness-gateway-status\""), "Gateway status testid");
assert(readinessReadyHtml.includes(">Ready<"), "Ready strip shows Ready labels");
assert(readinessReadyHtml.includes("href=\"/settings/openshell/connectivity\""), "Ready strip CTA: Connectivity");
assert(readinessReadyHtml.includes("href=\"/settings/openshell/profiles\""), "Ready strip CTA: Sandbox specs");
assert(!readinessReadyHtml.includes("href=\"/settings/agent-runtime\""), "Ready strip no longer gates on agents enabled");
assert(readinessReadyHtml.includes("Settings → Connectivity"), "Ready strip Connectivity CTA copy");
assert(readinessReadyHtml.includes("Settings → Sandbox specs"), "Ready strip Sandbox specs CTA copy");

const readinessNotReadyHtml = renderToString(
  React.createElement(OpenShellReadinessStripView, {
    gateway: { ready: false, detail: "gateway unreachable" },
    sandbox: { ready: false, detail: "No default sandbox profile" },
  }),
);
assert(readinessNotReadyHtml.includes("data-ready=\"false\""), "Not-ready strip marks rows not ready");
assert(readinessNotReadyHtml.includes(">Not ready<"), "Not-ready strip shows Not ready labels");
assert(!readinessNotReadyHtml.includes(">Ready<"), "Not-ready strip has no Ready label");
assert(readinessNotReadyHtml.includes("gateway unreachable"), "Not-ready strip shows gateway detail");
assert(readinessNotReadyHtml.includes("No default sandbox profile"), "Not-ready strip shows sandbox detail");
assert(readinessNotReadyHtml.includes("href=\"/settings/openshell/connectivity\""), "Not-ready CTA: Connectivity");
assert(readinessNotReadyHtml.includes("href=\"/settings/openshell/profiles\""), "Not-ready CTA: Sandbox specs");
assert(!readinessNotReadyHtml.includes("href=\"/settings/agent-runtime\""), "Not-ready strip no agents CTA");

const readinessCheckingHtml = renderToString(
  React.createElement(OpenShellReadinessStripView, {
    gateway: { ready: false, checking: true },
    sandbox: { ready: false, checking: true },
  }),
);
assert(readinessCheckingHtml.includes("data-ready=\"false\""), "Checking state fails closed (not ready)");
assert(readinessCheckingHtml.includes("Checking…"), "Checking state shows Checking label");

// Archived toggle on empty board when only retired projects exist.
const archivedEmptyBoardHtml = renderToString(
  React.createElement(Board, {
    goals: [
      {
        id: 7,
        title: "Old project",
        intent: "done",
        progress: 1,
        leaves_done: 1,
        leaves_total: 1,
        agents_live: 0,
        needs_you: 0,
        plan_status: "approved_v1",
        archived: true,
        columns: [],
        story: [],
      },
    ],
    items: new Map(),
    stories: new Map(),
    goalOf: (id) => id,
    breadcrumbOf: () => "",
    now: Date.now(),
    agentTimeout: 300,
    onOpen: () => {},
  }),
);
assert(archivedEmptyBoardHtml.includes("data-testid=\"board-empty\""),
  "Archived-only board still shows empty shell");
assert(archivedEmptyBoardHtml.includes("data-testid=\"operator-guide\""),
  "Archived-only empty embeds OperatorGuide");
assert(archivedEmptyBoardHtml.includes("data-testid=\"board-empty-show-archived\""),
  "Archived toggle present on empty board");
assert(
  /Show\s*(?:<!-- -->)?1(?:<!-- -->)?\s*archived/.test(archivedEmptyBoardHtml),
  "Archived toggle labels count",
);
const pkg = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), "package.json"), "utf8"),
);
assert(!Object.keys(pkg.dependencies || {}).some((d) => /patternfly/i.test(d)),
  "Must not add a PatternFly dependency");
assert(!Object.keys(pkg.devDependencies || {}).some((d) => /patternfly/i.test(d)),
  "Must not add a PatternFly devDependency");

// Chrome URL location contract (History API — no router dependency)
assert.deepStrictEqual(parseChromeLocation("/"), {
  view: "board",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/help"), {
  view: "help",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/help/"), {
  view: "help",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/openshell"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/openshell/providers"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "providers",
});
assert.deepStrictEqual(parseChromeLocation("/settings/openshell/provider-types"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "provider-types",
});
assert.deepStrictEqual(parseChromeLocation("/settings/openshell/profiles"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "profiles",
});
assert.deepStrictEqual(parseChromeLocation("/settings/openshell/connectivity"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/github-app"), {
  view: "settings",
  cardId: null,
  settingsSection: "github-app",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/access"), {
  view: "settings",
  cardId: null,
  settingsSection: "access",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/workspace"), {
  view: "settings",
  cardId: null,
  settingsSection: "workspace",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/agent-runtime"), {
  view: "settings",
  cardId: null,
  settingsSection: "agent-runtime",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/settings/nope"), {
  view: "settings",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/card/42"), {
  view: "board",
  cardId: 42,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/card/0"), {
  view: "board",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/card/nope"), {
  view: "board",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.deepStrictEqual(parseChromeLocation("/unknown"), {
  view: "board",
  cardId: null,
  settingsSection: "openshell",
  openShellTab: "connectivity",
});
assert.strictEqual(
  formatChromePath({
    view: "board",
    cardId: null,
    settingsSection: "openshell",
    openShellTab: "connectivity",
  }),
  "/",
);
assert.strictEqual(
  formatChromePath({
    view: "help",
    cardId: null,
    settingsSection: "openshell",
    openShellTab: "connectivity",
  }),
  "/help",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: 99,
    settingsSection: "openshell",
    openShellTab: "connectivity",
  }),
  "/settings",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "openshell",
    openShellTab: "providers",
  }),
  "/settings/openshell/providers",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "openshell",
    openShellTab: "provider-types",
  }),
  "/settings/openshell/provider-types",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "openshell",
    openShellTab: "profiles",
  }),
  "/settings/openshell/profiles",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "github-app",
    openShellTab: "providers",
  }),
  "/settings/github-app",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "access",
    openShellTab: "connectivity",
  }),
  "/settings/access",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "workspace",
    openShellTab: "connectivity",
  }),
  "/settings/workspace",
);
assert.strictEqual(
  formatChromePath({
    view: "settings",
    cardId: null,
    settingsSection: "agent-runtime",
    openShellTab: "connectivity",
  }),
  "/settings/agent-runtime",
);
assert.strictEqual(
  formatChromePath({
    view: "board",
    cardId: 7,
    settingsSection: "openshell",
    openShellTab: "connectivity",
  }),
  "/card/7",
);
assert(
  chromeLocationsEqual(
    {
      view: "board",
      cardId: 1,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
    {
      view: "board",
      cardId: 1,
      settingsSection: "github-app",
      openShellTab: "providers",
    },
  ),
  "equal chrome locations (board ignores settings axes)",
);
assert(
  chromeLocationsEqual(
    {
      view: "settings",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "providers",
    },
    {
      view: "settings",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "providers",
    },
  ),
  "equal settings+openshell tab locations",
);
assert(
  !chromeLocationsEqual(
    {
      view: "settings",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "providers",
    },
    {
      view: "settings",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "profiles",
    },
  ),
  "distinct openshell tabs",
);
assert(
  !chromeLocationsEqual(
    {
      view: "settings",
      cardId: null,
      settingsSection: "github-app",
      openShellTab: "connectivity",
    },
    {
      view: "settings",
      cardId: null,
      settingsSection: "access",
      openShellTab: "connectivity",
    },
  ),
  "distinct settings sections",
);
assert(
  !chromeLocationsEqual(
    {
      view: "board",
      cardId: 1,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
    {
      view: "help",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
  ),
  "distinct chrome locations",
);
{
  const pushes = [];
  const replaces = [];
  const hist = {
    pushState: (_s, _t, url) => pushes.push(url),
    replaceState: (_s, _t, url) => replaces.push(url),
  };
  writeChromeLocation(
    {
      view: "help",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
    "push",
    hist,
    { pathname: "/" },
  );
  writeChromeLocation(
    {
      view: "help",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
    "push",
    hist,
    { pathname: "/help" },
  );
  writeChromeLocation(
    {
      view: "board",
      cardId: 3,
      settingsSection: "openshell",
      openShellTab: "connectivity",
    },
    "replace",
    hist,
    { pathname: "/help" },
  );
  writeChromeLocation(
    {
      view: "settings",
      cardId: null,
      settingsSection: "openshell",
      openShellTab: "providers",
    },
    "push",
    hist,
    { pathname: "/settings" },
  );
  writeChromeLocation(
    {
      view: "settings",
      cardId: null,
      settingsSection: "agent-runtime",
      openShellTab: "connectivity",
    },
    "push",
    hist,
    { pathname: "/settings/openshell/providers" },
  );
  assert.deepStrictEqual(
    pushes,
    ["/help", "/settings/openshell/providers", "/settings/agent-runtime"],
    "pushState for help + settings section/tab changes",
  );
  assert.deepStrictEqual(replaces, ["/card/3"], "replaceState for card deep link");
}
// Round-trip: settings section + OpenShell tab
for (const path of [
  "/settings",
  "/settings/openshell/providers",
  "/settings/openshell/profiles",
  "/settings/github-app",
  "/settings/access",
  "/settings/workspace",
  "/settings/agent-runtime",
]) {
  const parsed = parseChromeLocation(path);
  assert.strictEqual(
    formatChromePath(parsed),
    path === "/settings/openshell/connectivity" ? "/settings" : path,
    `round-trip ${path}`,
  );
}
assert.strictEqual(
  formatChromePath(parseChromeLocation("/settings/openshell")),
  "/settings",
  "openshell default tab canonicalizes to /settings",
);
assert.strictEqual(
  formatChromePath(parseChromeLocation("/settings/openshell/connectivity")),
  "/settings",
  "explicit connectivity tab canonicalizes to /settings",
);
{
  // Controlled Settings deep-link: section + OpenShell tab from URL contract
  const settingsDeepHtml = renderToString(
    React.createElement(Settings, {
      section: "openshell",
      openShellTab: "providers",
    }),
  );
  assert(
    settingsDeepHtml.includes("data-testid=\"settings-panel-openshell\""),
    "Settings deep link opens OpenShell section",
  );
  assert(
    settingsDeepHtml.includes("data-testid=\"openshell-providers-slot\"") ||
      settingsDeepHtml.includes("data-testid=\"openshell-tab-providers\""),
    "Settings deep link selects OpenShell Providers tab",
  );
  assert(
    /aria-current="page"[^>]*data-testid="openshell-tab-providers"|data-testid="openshell-tab-providers"[^>]*aria-current="page"/.test(
      settingsDeepHtml,
    ),
    "Providers tab marked current for deep link",
  );
  const forgeDeepHtml = renderToString(
    React.createElement(Settings, { section: "workspace" }),
  );
  assert(
    forgeDeepHtml.includes("data-testid=\"settings-panel-workspace\""),
    "Settings deep link opens Forge (workspace) section",
  );
}
assert(
  !Object.keys(pkg.dependencies || {}).some((d) => /react-router|@tanstack\/react-router|wouter/i.test(d)),
  "Must not add a client router dependency for chrome URL sync",
);

console.log("\n✅ All Card, Board, Detail, Settings chrome, and useBoard sequence guard assertions passed!");

