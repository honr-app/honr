export type State =
  | "draft" | "shaping" | "backlog" | "claimed" | "running" | "splitting"
  | "needs_human" | "review" | "done" | "retired"
  /** Legacy wire value — treat as backlog. */
  | "ready";

export type ColumnKey =
  | "intake" | "shaping" | "backlog" | "running"
  | "needs_you" | "review" | "done" | "retired"
  | "ready";

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

export interface BlockerSummary {
  id: number;
  title: string;
  state: State;
}

export type PlanStatus = "empty" | "awaiting_approval" | "approved";

export interface PlanTaskSpec {
  key: string;
  title: string;
  intent: string;
  definition_of_done: string;
  blocked_by_keys: string[];
  capability: string | null;
  item_id: number | null;
}

export interface PlanArtifact {
  revision: number;
  summary: string;
  status: PlanStatus;
  tasks: PlanTaskSpec[];
  cancel_keys: string[];
  cancel_item_ids: number[];
  approved_revision: number | null;
}

/** Proposed sibling Tasks on a card (Initial plan or impl split) awaiting Approve. */
export interface TaskProposal {
  summary: string;
  tasks: PlanTaskSpec[];
}

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
  blockers?: BlockerSummary[];
  capability: string | null;
  lease: Lease | null;
  /** Hard end of this run (claim + agent_timeout). Not renewed. */
  run_deadline_at?: string | null;
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
  project_prompt?: string | null;
  last_bounce_reason?: string | null;
  release_target: string | null;
  environment: string | null;
  /** agy conversation id; park keeps it for resume, halt clears it. */
  conversation_id?: string | null;
  /** Park hold: Backlog but not dispatchable until unpark. */
  parked?: boolean;
  /** Cockpit asked supervisor to start this Backlog card. */
  awaiting_dispatch?: boolean;
  engine?: string | null;
  beads_id?: string | null;
  github_issue_url?: string | null;
  pr_url: string | null;
  plan?: PlanArtifact | null;
  proposal?: TaskProposal | null;
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
  /** `no_plan` | `awaiting_approval` | `approved_vN` */
  plan_status: string;
  /** Soft-retired Project — cockpit hides unless "Show archived". */
  archived?: boolean;
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
  /** Wall-clock cap for a run (`agents.agent_timeout_secs`). */
  agent_timeout_secs: number;
  seq: number;
  default_engine?: string;
  default_model?: string;
}

export type BoardEvent =
  | { type: "upsert"; seq: number; item: WorkItem }
  | { type: "story"; seq: number; goal: number; at: string; text: string }
  | { type: "delete"; seq: number; id: number }
  | { type: "main_advanced"; seq: number; ref_name: string; commit_sha?: string | null }
  | { type: "reset"; seq: number };

/** Normalize legacy `ready` wire values to `backlog`. */
export function normState(s: State): Exclude<State, "ready"> {
  return s === "ready" ? "backlog" : s;
}

export function normColumn(c: ColumnKey): Exclude<ColumnKey, "ready"> {
  return c === "ready" ? "backlog" : c;
}

/** Which board column a state renders in. Mirrors `State::column` on the server. */
export const COLUMN_OF: Record<Exclude<State, "ready">, Exclude<ColumnKey, "ready">> = {
  draft: "intake",
  shaping: "shaping",
  backlog: "backlog",
  claimed: "running",
  running: "running",
  splitting: "running",
  needs_human: "needs_you",
  review: "review",
  done: "done",
  retired: "retired",
};

/** Board columns. Intake and Shaping live off the kanban strip. */
export const BOARD_COLUMNS: { key: Exclude<ColumnKey, "ready">; label: string; question: string }[] = [
  { key: "backlog", label: "BACKLOG", question: "What should start next?" },
  { key: "running", label: "RUNNING", question: "Is it alive, and is it worth it?" },
  { key: "needs_you", label: "⚠ NEEDS YOU", question: "How fast must I act?" },
  { key: "review", label: "REVIEW", question: "Can I approve this in 30 seconds?" },
  { key: "done", label: "DONE", question: "" },
];
