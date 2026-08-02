/**
 * A board dense enough to judge the UI against — Project + flat Tasks.
 *
 * Writes a `honr.json` directly rather than driving the API, because the
 * states worth looking at are exactly the ones no public verb can produce.
 *
 *   node ui-fixture.mjs > /tmp/honr-ui/honr.json
 */

const NOW = Date.now();
const iso = (secsAgo) => new Date(NOW - secsAgo * 1000).toISOString();

let next = 1;
const items = {};

function item(o) {
  const id = next++;
  items[id] = {
    id,
    parent: null,
    level: null,
    definition_of_done: null,
    origin: { kind: "human" },
    above_line: false,
    blocked_by: [],
    capability: null,
    lease: null,
    model: null,
    progress: 0,
    cost_cents: 0,
    budget_cents: null,
    escalation: null,
    gates: [],
    gate_failures: 0,
    run_failures: 0,
    diff_added: 0,
    diff_removed: 0,
    notes: [],
    pinned: [],
    release_target: null,
    environment: null,
    pr_url: null,
    beads_id: null,
    created_at: iso(o.age ?? 3600),
    entered_state_at: iso(o.since ?? 600),
    history: [],
    ...o,
  };
  delete items[id].age;
  delete items[id].since;
  return id;
}

const lease = (agent, hbAgo) => ({
  agent_id: agent,
  granted_at: iso(hbAgo + 900),
  last_heartbeat: iso(hbAgo),
  expires_at: iso(hbAgo - 600),
});

// ---- Project root ---------------------------------------------------------

const project = item({
  title: "Phase 2 — real agents",
  intent: "Replace the simulated fleet with agents in OpenShell sandboxes that open real PRs.",
  state: "shaping",
  level: "Project",
  above_line: true,
  budget_cents: 5000,
  beads_id: "honr-proj1",
  pinned: [
    "Merging is a human action. Approving in honr surfaces the PR; it never merges it.",
    "Everything in the sandbox stack fails as a hang, not an error.",
  ],
});

const task = (title, intent, extra = {}) =>
  item({
    parent: project,
    title,
    intent,
    definition_of_done: `${title} is done and covered by a test.`,
    state: "ready",
    level: "Task",
    capability: "any",
    ...extra,
  });

// ---- Ready (Diamond DAG: A -> B, A -> C, B+C -> D) -------------------------

const taskA = task("Surface PR checks on the Review card", "CI is the mechanical gate; the board should show it.", {
  since: 2400,
  beads_id: "honr-t10",
});
const taskB = task("Fail closed when CI is red", "A Review card with failing checks should be obvious.", {
  blocked_by: [taskA],
  since: 2100,
  beads_id: "honr-t11",
});
const taskC = task("Report the real diffstat", "Review sorts by a blast radius it does not actually know.", {
  blocked_by: [taskA],
  since: 1500,
  beads_id: "honr-t17",
});
const taskD = task("Observe cost during the run", "Spend only arrives in the final message, so no cap can interrupt.", {
  blocked_by: [taskB, taskC],
  since: 900,
  beads_id: "honr-t18",
});
task("Re-adopt live sandboxes on restart", "A rebuilt honr should resume watching a run, not kill it.", {
  since: 600,
  beads_id: "honr-t9",
});
task("Split from inside the sandbox", "Work bigger than its card should become sibling tasks, not nest.", {
  since: 300,
  beads_id: "honr-t13",
});
task("Receipt copy for the digest", "Plain-language wording for the phone view.", {
  capability: "writer",
  since: 120,
  beads_id: "honr-t15",
});

// ---- Running --------------------------------------------------------------

item({
  parent: project,
  title: "Verdict file protocol",
  intent: "An agent needs a way to hand a decision back without a network path to honr.",
  definition_of_done: "escalate.json with two options lands the card in Needs You.",
  state: "running",
  level: "Task",
  capability: "any",
  lease: lease("sandbox-12", 2),
  model: "claude-opus-5",
  progress: 0.62,
  cost_cents: 210,
  environment: "honr-card-12-a1",
  beads_id: "honr-t12",
  since: 840,
});

item({
  parent: project,
  title: "Sandbox name on the card",
  intent: "environment is stored but nothing renders it.",
  definition_of_done: "A Running card shows its sandbox name.",
  state: "running",
  level: "Task",
  capability: "any",
  lease: lease("sandbox-13", 14),
  model: "claude-opus-5",
  progress: 0.18,
  cost_cents: 80,
  environment: "honr-card-13-a1",
  beads_id: "honr-t14",
  since: 200,
});

item({
  parent: project,
  title: "Clean-checkout verifier",
  intent: "Gates should run where the agent cannot reach them.",
  definition_of_done: "Gates run in a sandbox created from the pushed branch.",
  state: "running",
  level: "Task",
  capability: "any",
  lease: lease("sandbox-14", 47),
  model: "claude-opus-5",
  progress: 0.91,
  cost_cents: 340,
  environment: "honr-card-14-a2",
  run_failures: 1,
  beads_id: "honr-t11b",
  since: 1500,
});

// ---- Needs You ------------------------------------------------------------

item({
  parent: project,
  title: "Force-push policy on shared branches",
  intent: "Two cards touching one branch need a rule before either lands.",
  definition_of_done: "The rule is documented and enforced in the supervisor.",
  state: "needs_human",
  level: "Task",
  capability: "any",
  lease: lease("sandbox-15", 30),
  model: "claude-opus-5",
  progress: 0.4,
  cost_cents: 155,
  beads_id: "honr-t19",
  since: 1080,
  escalation: {
    question:
      "Two cards want to touch honr/card-8. Force-push would drop whichever landed first. Which wins?",
    options: [
      {
        label: "Serialise on the branch",
        detail: "Second card waits for the first to merge. Simple; costs throughput when cards overlap.",
      },
      {
        label: "Rebase the second card",
        detail: "Second agent rebases onto the first's work. Keeps both moving; can conflict.",
      },
    ],
    recommended: 1,
    blocked_since: iso(1080),
    answer: null,
  },
});

// ---- Review ---------------------------------------------------------------

item({
  parent: project,
  title: "Attempt-scoped sandbox names",
  intent: "A retry must not collide with the sandbox kept for inspection.",
  definition_of_done: "A second attempt creates honr-card-N-a2.",
  state: "review",
  level: "Task",
  capability: "any",
  progress: 1,
  cost_cents: 96,
  diff_added: 34,
  diff_removed: 8,
  pr_url: "https://github.com/example/honr/pull/20",
  beads_id: "honr-t20",
  since: 45,
});

item({
  parent: project,
  title: "First self-hosted card: GET /api/version",
  intent: "A deliberately small change to prove the loop end to end.",
  definition_of_done: "GET /api/version returns the crate version; a test asserts it.",
  state: "review",
  level: "Task",
  capability: "any",
  progress: 1,
  cost_cents: 122,
  diff_added: 25,
  diff_removed: 0,
  pr_url: "https://github.com/shanemcd/honr/pull/1",
  environment: "honr-card-8-a1",
  gates: [{ name: "agent-reported", status: "passed", detail: "3 gates passed" }],
  beads_id: "honr-t8",
  since: 300,
});

item({
  parent: project,
  title: "Rebase onto upstream, not the fork's frozen base",
  intent: "Nothing syncs a fork, so its base freezes while upstream moves.",
  definition_of_done: "A re-run rebases onto upstream/main.",
  state: "review",
  level: "Task",
  capability: "any",
  progress: 1,
  cost_cents: 402,
  diff_added: 318,
  diff_removed: 96,
  gate_failures: 2,
  pr_url: "https://github.com/shanemcd/honr/pull/4",
  environment: "honr-card-17-a3",
  gates: [{ name: "agent-reported", status: "passed", detail: "3 gates passed" }],
  beads_id: "honr-t21",
  since: 5400,
});

// ---- Done -----------------------------------------------------------------

for (const [title, added, removed, bid] of [
  ["Open the sandbox policy for the toolchain", 6, 2, "honr-t7"],
  ["Cap run retries so a failing card stops looping", 148, 12, "honr-t22"],
  ["Let the agent publish; the supervisor verifies", 97, 113, "honr-t23"],
  ["Add openshell.rs: typed CLI wrapper", 402, 0, "honr-t24"],
]) {
  item({
    parent: project,
    title,
    intent: `${title}.`,
    definition_of_done: `${title} verified.`,
    state: "done",
    level: "Task",
    capability: "any",
    progress: 1,
    cost_cents: 90 + added,
    diff_added: added,
    diff_removed: removed,
    gates: [{ name: "agent-reported", status: "passed", detail: "gates passed" }],
    beads_id: bid,
    since: 7200,
  });
}

const stories = {
  [project]: [
    { at: iso(5400), text: "Phase 2 started: supervisor lands, first sandbox boots." },
    { at: iso(1800), text: "honr opened its own first PR (#1) for $1.22." },
    { at: iso(300), text: "Two cards in review; one agent blocked on a branch policy call." },
  ],
};

// Populate blockers array for items with blocked_by
for (const item of Object.values(items)) {
  if (item.blocked_by && item.blocked_by.length > 0 && (!item.blockers || item.blockers.length === 0)) {
    item.blockers = item.blocked_by.map((bid) => ({
      id: bid,
      title: items[bid]?.title ?? `Task #${bid}`,
      state: items[bid]?.state ?? "ready",
    }));
  }
}

process.stdout.write(JSON.stringify({ next_id: next, items, stories }, null, 1));
