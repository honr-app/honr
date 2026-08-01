/**
 * A board dense enough to judge the UI against.
 *
 * Writes a `honr.json` directly rather than driving the API, because the
 * states worth looking at are exactly the ones no public verb can produce:
 * lease timestamps at chosen ages (so the heartbeat decay gradient is
 * visible), `pr_url` on Review cards, gate history, escalations mid-flight.
 *
 * This is a *fixture*, not a seed. It never runs in the product — point a
 * scratch honr at it (see screenshots.mjs) and throw it away afterwards.
 *
 *   node ui-fixture.mjs > /tmp/honr-ui/honr.json
 */

// Relative to *now*, not a fixed instant: leases have to still be live when
// honr boots, or the sweeper requeues every Running card before the first
// screenshot and the board you capture is not the board you described.
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

// ---- above the line -------------------------------------------------------

const vision = item({
  title: "honr builds honr",
  intent: "honr takes cards against its own source and hands back reviewable pull requests.",
  state: "shaping",
  level: "Vision",
  above_line: true,
  pinned: [
    "Merging is a human action. Approving in honr surfaces the PR; it never merges it.",
    "Everything in the sandbox stack fails as a hang, not an error.",
  ],
});

const project = item({
  parent: vision,
  title: "Phase 2 — real agents",
  intent: "Replace the simulated fleet with agents in OpenShell sandboxes that open real PRs.",
  state: "shaping",
  level: "Project",
  above_line: true,
  budget_cents: 5000,
});

const mk = (title, intent) =>
  item({ parent: project, title, intent, state: "shaping", level: "Epic", above_line: true });

const loop = mk("Prove the loop", "One real card goes from Ready to an open PR with no hand-holding.");
const verify = mk("Verification the agent cannot influence", "Gates run by the supervisor from a clean checkout.");
const decide = mk("Agent-initiated decisions", "An agent that hits a real ambiguity stops and asks.");

// ---- Ready: deep enough that the column has to chunk ----------------------

const leaf = (parent, title, intent, extra = {}) =>
  item({
    parent,
    title,
    intent,
    definition_of_done: `${title} is done and covered by a test.`,
    state: "ready",
    level: "Story",
    capability: "any",
    ...extra,
  });

const first = leaf(verify, "Supervisor runs the gates", "The agent's own claim of success is what reaches the board.", { since: 2400 });
leaf(verify, "Verify from a clean checkout", "Gates in the agent's sandbox can be influenced by the agent.", { blocked_by: [first], since: 2100 });
leaf(verify, "Report the real diffstat", "Review sorts by a blast radius it does not actually know.", { since: 1500 });
leaf(verify, "Observe cost during the run", "Spend only arrives in the final message, so no cap can interrupt.", { since: 900 });
leaf(loop, "Re-adopt live sandboxes on restart", "A rebuilt honr should resume watching a run, not kill it.", { since: 600 });
leaf(decide, "Split from inside the sandbox", "Work bigger than its card should decompose, not overrun.", { since: 300 });
leaf(project, "Receipt copy for the digest", "Plain-language wording for the phone view.", { capability: "writer", since: 120 });

// ---- Running: three heartbeat ages, to show the decay gradient ------------

item({
  parent: decide,
  title: "Verdict file protocol",
  intent: "An agent needs a way to hand a decision back without a network path to honr.",
  definition_of_done: "escalate.json with two options lands the card in Needs You.",
  state: "running",
  level: "Story",
  capability: "any",
  lease: lease("sandbox-12", 2),
  model: "claude-opus-5",
  progress: 0.62,
  cost_cents: 210,
  environment: "honr-card-12-a1",
  since: 840,
});

item({
  parent: loop,
  title: "Sandbox name on the card",
  intent: "environment is stored but nothing renders it.",
  definition_of_done: "A Running card shows its sandbox name.",
  state: "running",
  level: "Story",
  capability: "any",
  lease: lease("sandbox-13", 14), // past heartbeat_expect: visibly decaying
  model: "claude-opus-5",
  progress: 0.18,
  cost_cents: 80,
  environment: "honr-card-13-a1",
  since: 200,
});

item({
  parent: verify,
  title: "Clean-checkout verifier",
  intent: "Gates should run where the agent cannot reach them.",
  definition_of_done: "Gates run in a sandbox created from the pushed branch.",
  state: "running",
  level: "Story",
  capability: "any",
  lease: lease("sandbox-14", 47), // deep decay — is this one hung?
  model: "claude-opus-5",
  progress: 0.91,
  cost_cents: 340,
  environment: "honr-card-14-a2",
  run_failures: 1,
  since: 1500,
});

// ---- Needs You ------------------------------------------------------------

item({
  parent: decide,
  title: "Force-push policy on shared branches",
  intent: "Two cards touching one branch need a rule before either lands.",
  definition_of_done: "The rule is documented and enforced in the supervisor.",
  state: "needs_human",
  level: "Story",
  capability: "any",
  lease: lease("sandbox-15", 30),
  model: "claude-opus-5",
  progress: 0.4,
  cost_cents: 155,
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

// ---- Verify ---------------------------------------------------------------

item({
  parent: loop,
  title: "Attempt-scoped sandbox names",
  intent: "A retry must not collide with the sandbox kept for inspection.",
  definition_of_done: "A second attempt creates honr-card-N-a2.",
  state: "verifying",
  level: "Story",
  capability: "any",
  progress: 1,
  cost_cents: 96,
  diff_added: 34,
  diff_removed: 8,
  gates: [{ name: "cargo test", status: "running", detail: null }],
  since: 45,
});

// ---- Review: the column whose whole point is the PR -----------------------

item({
  parent: loop,
  title: "First self-hosted card: GET /api/version",
  intent: "A deliberately small change to prove the loop end to end.",
  definition_of_done: "GET /api/version returns the crate version; a test asserts it.",
  state: "review",
  level: "Story",
  capability: "any",
  progress: 1,
  cost_cents: 122,
  diff_added: 25,
  diff_removed: 0,
  pr_url: "https://github.com/shanemcd/honr/pull/1",
  environment: "honr-card-8-a1",
  gates: [{ name: "agent-reported", status: "passed", detail: "3 gates passed" }],
  since: 300,
});

item({
  parent: verify,
  title: "Rebase onto upstream, not the fork's frozen base",
  intent: "Nothing syncs a fork, so its base freezes while upstream moves.",
  definition_of_done: "A re-run rebases onto upstream/main.",
  state: "review",
  level: "Story",
  capability: "any",
  progress: 1,
  cost_cents: 402,
  diff_added: 318,
  diff_removed: 96,
  gate_failures: 2,
  pr_url: "https://github.com/shanemcd/honr/pull/4",
  environment: "honr-card-17-a3",
  gates: [{ name: "agent-reported", status: "passed", detail: "3 gates passed" }],
  since: 5400,
});

// ---- Done -----------------------------------------------------------------

for (const [title, added, removed] of [
  ["Open the sandbox policy for the toolchain", 6, 2],
  ["Cap run retries so a failing card stops looping", 148, 12],
  ["Let the agent publish; the supervisor verifies", 97, 113],
  ["Add openshell.rs: typed CLI wrapper", 402, 0],
]) {
  item({
    parent: loop,
    title,
    intent: `${title}.`,
    definition_of_done: `${title} verified.`,
    state: "done",
    level: "Story",
    capability: "any",
    progress: 1,
    cost_cents: 90 + added,
    diff_added: added,
    diff_removed: removed,
    gates: [{ name: "agent-reported", status: "passed", detail: "gates passed" }],
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
