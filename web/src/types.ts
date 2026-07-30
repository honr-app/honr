export type State =
  | "draft" | "shaping" | "ready" | "claimed" | "running" | "splitting"
  | "needs_human" | "verifying" | "review" | "done" | "retired";

export type ColumnKey =
  | "intake" | "shaping" | "ready" | "running"
  | "needs_you" | "verify" | "review" | "done" | "retired";

export interface Lease {
  agent_id: string;
  granted_at: string;
  last_heartbeat: string;
  expires_at: string;
}

export interface EscalationOption { label: string; detail: string }

export interface Escalation {
  question: string;
  options: EscalationOption[];
  recommended: number;
  blocked_since: string;
  answer: string | null;
}

export interface GateRun {
  name: string;
  status: "pending" | "running" | "passed" | "failed";
  detail: string | null;
}

export interface Note { at: string; author: string; text: string }

export interface Transition {
  at: string; from: State; to: State; by: string; reason: string | null;
}

export type Origin =
  | { kind: "human" }
  | { kind: "planner" }
  | { kind: "split"; from: number }
  | { kind: "reflection" };

export interface WorkItem {
  id: number;
  parent: number | null;
  level: string | null;
  title: string;
  intent: string;
  definition_of_done: string | null;
  state: State;
  origin: Origin;
  above_line: boolean;
  blocked_by: number[];
  capability: string | null;
  lease: Lease | null;
  model: string | null;
  progress: number;
  cost_cents: number;
  budget_cents: number | null;
  escalation: Escalation | null;
  gates: GateRun[];
  gate_failures: number;
  diff_added: number;
  diff_removed: number;
  notes: Note[];
  pinned: string[];
  release_target: string | null;
  environment: string | null;
  created_at: string;
  entered_state_at: string;
  history: Transition[];
}

export interface ChunkSummary { count: number; text: string }
export interface ColumnView { column: ColumnKey; summary: ChunkSummary }
export interface StoryLine { at: string; text: string }

export interface GoalView {
  id: number;
  title: string;
  intent: string;
  progress: number;
  leaves_done: number;
  leaves_total: number;
  spend_cents: number;
  budget_cents: number | null;
  agents_live: number;
  needs_you: number;
  columns: ColumnView[];
  story: StoryLine[];
}

export interface Level {
  name: string;
  horizon: string | null;
  owner: string | null;
  elaborate: string | null;
  requires: string[];
  claimable: boolean;
}

export interface Snapshot {
  items: WorkItem[];
  levels: Level[];
  goals: GoalView[];
  server_time: string;
  heartbeat_expect_secs: number;
  seq: number;
}

export type BoardEvent =
  | { type: "upsert"; seq: number; item: WorkItem }
  | { type: "story"; seq: number; goal: number; at: string; text: string };

/** Which board column a state renders in. Mirrors `State::column` on the server. */
export const COLUMN_OF: Record<State, ColumnKey> = {
  draft: "intake",
  shaping: "shaping",
  ready: "ready",
  claimed: "running",
  running: "running",
  splitting: "running",
  needs_human: "needs_you",
  verifying: "verify",
  review: "review",
  done: "done",
  retired: "retired",
};

/** The six columns the board shows. Intake and Shaping live in the tree view. */
export const BOARD_COLUMNS: { key: ColumnKey; label: string; question: string }[] = [
  { key: "ready", label: "READY", question: "Is this actually ready?" },
  { key: "running", label: "RUNNING", question: "Is it alive, and is it worth it?" },
  { key: "needs_you", label: "⚠ NEEDS YOU", question: "How fast must I act?" },
  { key: "verify", label: "VERIFY", question: "Will it pass?" },
  { key: "review", label: "REVIEW", question: "Can I approve this in 30 seconds?" },
  { key: "done", label: "DONE", question: "" },
];
