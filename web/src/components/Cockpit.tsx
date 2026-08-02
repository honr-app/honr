import { useEffect, useMemo, useState, type MouseEvent } from "react";
import { Card } from "./Card.js";
import { DependencyGraph } from "./DependencyGraph.js";
import { humanizeEscalation } from "../humanize.js";
import { BOARD_COLUMNS, COLUMN_OF, normState } from "../types.js";
import type { ColumnKey, GoalView, StoryLine, WorkItem } from "../types.js";
import { api, money, since } from "../api.js";

export interface CockpitProps {
  goals: GoalView[];
  items: Map<number, WorkItem>;
  stories: Map<number, StoryLine[]>;
  goalOf: (id: number) => number;
  breadcrumbOf: (id: number) => string;
  now: number;
  heartbeatExpect: number;
  defaultEngine?: string;
  defaultModel?: string;
  onOpen: (id: number) => void;
  onChanged?: () => void;
}

type BoardFilter = "all" | "running" | "needs_you" | "review";

function labelOfItem(items: Map<number, WorkItem>, id: number): string {
  const it = items.get(id);
  if (it?.beads_id && !it.beads_id.startsWith("bd-honr-")) return it.beads_id;
  return `#${id}`;
}

/** How many cards to show before the rest becomes a chunk. */
const VISIBLE = 4;

/**
 * Cockpit — the one surface.
 *
 * Mode sentence + Needs you actions, then project swimlanes. Filtering and
 * expanding a lane is the drill-down; there is no separate Home/Board.
 */
export function Cockpit(props: CockpitProps) {
  const [filterQuery, setFilterQuery] = useState("");
  const [filterState, setFilterState] = useState<BoardFilter>("all");
  const [projectFilter, setProjectFilter] = useState<number | "all">("all");
  const [showArchived, setShowArchived] = useState(false);
  /** Explicit open/closed overrides; missing keys use the lane's default. */
  const [laneOpen, setLaneOpen] = useState<Record<number, boolean>>({});

  const isArchivedGoal = (goal: GoalView) =>
    goal.archived === true || props.items.get(goal.id)?.state === "retired";

  const activeGoals = props.goals.filter((goal) => !isArchivedGoal(goal));
  const archivedGoals = props.goals.filter((goal) => isArchivedGoal(goal));

  const tasks = useMemo(
    () =>
      [...props.items.values()].filter(
        (i) => i.parent != null && i.state !== "retired" && i.level !== "Project",
      ),
    [props.items],
  );

  const needsYouItems = useMemo(() => {
    const q = filterQuery.toLowerCase().trim();
    return tasks.filter((t) => {
      if (t.state !== "needs_human" || !t.escalation) return false;
      if (projectFilter !== "all" && t.parent !== projectFilter) return false;
      if (
        q &&
        !t.title.toLowerCase().includes(q) &&
        !`#${t.id}`.includes(q) &&
        !(t.beads_id ?? "").toLowerCase().includes(q)
      )
        return false;
      return true;
    });
  }, [tasks, filterQuery, projectFilter]);

  const totals = useMemo(() => {
    let needsYou = 0;
    let running = 0;
    let review = 0;
    let backlog = 0;
    let done = 0;
    for (const t of tasks) {
      if (t.state === "needs_human") needsYou += 1;
      if (["claimed", "running", "splitting"].includes(t.state)) running += 1;
      if (t.state === "review") review += 1;
      if (normState(t.state) === "backlog") backlog += 1;
      if (t.state === "done") done += 1;
    }
    return { needsYou, running, review, backlog, done, total: tasks.length };
  }, [tasks]);

  // Stable order from the board (id / creation). Activity shows on badges and
  // lane styling — reordering swimlanes as work moves is disorienting.
  // Archived lanes sort after active when revealed.
  const sortedGoals = useMemo(() => {
    const base = showArchived
      ? [...activeGoals, ...archivedGoals]
      : activeGoals;
    if (projectFilter === "all") return base;
    return base.filter((g) => g.id === projectFilter);
  }, [activeGoals, archivedGoals, projectFilter, showArchived]);

  const mode = modeCopy({
    needsYou: totals.needsYou,
    running: totals.running,
    inReview: totals.review,
    backlog: totals.backlog,
    done: totals.done,
    totalTasks: totals.total,
  });

  if (!activeGoals.length && !(showArchived && archivedGoals.length)) {
    return (
      <div className="cockpit">
        <header className="cockpit-hero">
          <h1>Welcome to honr</h1>
          <p className="cockpit-lede">
            This is the control plane for agent work. Create a Project — it
            seeds an Initial plan Task — approve the Plan, and Tasks show up
            here. Agents stay idle until you approve.
          </p>
        </header>
        <div className="cockpit-empty-card">
          <h2>No projects yet</h2>
          <p className="dim">
            From a cockpit agent with the honr MCP: create_project →
            propose_breakdown → approve_plan. Then dispatch Backlog tasks to start.
          </p>
          {archivedGoals.length > 0 && (
            <p style={{ marginTop: 12 }}>
              <button
                type="button"
                className="filter-btn active"
                onClick={() => setShowArchived(true)}
              >
                Show {archivedGoals.length} archived
              </button>
            </p>
          )}
        </div>
        <HowItWorks />
      </div>
    );
  }

  const showNeedsBlock =
    needsYouItems.length > 0 &&
    (filterState === "all" || filterState === "needs_you");

  return (
    <div className={`cockpit ${totals.needsYou > 0 ? "cockpit-attention" : ""}`}>
      <header className="cockpit-hero">
        <h1>{mode.title}</h1>
        <p className="cockpit-lede">{mode.lede}</p>
      </header>

      <div className="board-filter">
        <input
          type="text"
          className="search-input"
          placeholder="Filter cards…"
          value={filterQuery}
          onChange={(e) => setFilterQuery(e.target.value)}
        />
        {(showArchived ? sortedGoals : activeGoals).length > 1 && (
          <select
            className="cockpit-project-select"
            value={projectFilter === "all" ? "all" : String(projectFilter)}
            onChange={(e) => {
              const v = e.target.value;
              setProjectFilter(v === "all" ? "all" : Number(v));
            }}
          >
            <option value="all">All projects</option>
            {activeGoals.map((g) => (
              <option key={g.id} value={g.id}>
                {g.title}
              </option>
            ))}
            {showArchived &&
              archivedGoals.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.title} (archived)
                </option>
              ))}
          </select>
        )}
        {(
          [
            ["all", "All"],
            ["running", "Running"],
            ["needs_you", `Needs you${totals.needsYou ? ` (${totals.needsYou})` : ""}`],
            ["review", `Review${totals.review ? ` (${totals.review})` : ""}`],
          ] as [BoardFilter, string][]
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            className={`filter-btn ${filterState === key ? "active" : ""} ${
              key === "needs_you" && totals.needsYou > 0 ? "alarmish" : ""
            }`}
            onClick={() => setFilterState(key)}
          >
            {label}
          </button>
        ))}
        {archivedGoals.length > 0 && (
          <button
            type="button"
            className={`filter-btn ${showArchived ? "active" : ""}`}
            onClick={() => setShowArchived((v) => !v)}
            title="Soft-retired projects stay in state; toggle to browse them"
          >
            Archived{showArchived ? "" : ` (${archivedGoals.length})`}
          </button>
        )}
        {sortedGoals.length > 0 && (
          <span className="lane-expand-controls">
            <button
              type="button"
              className="filter-btn"
              onClick={() =>
                setLaneOpen(
                  Object.fromEntries(sortedGoals.map((g) => [g.id, true])),
                )
              }
            >
              Expand all
            </button>
            <button
              type="button"
              className="filter-btn"
              onClick={() =>
                setLaneOpen(
                  Object.fromEntries(sortedGoals.map((g) => [g.id, false])),
                )
              }
            >
              Collapse all
            </button>
          </span>
        )}
      </div>

      {showNeedsBlock && (
        <section className="cockpit-needs" aria-labelledby="cockpit-needs-title">
          <div className="cockpit-section-head">
            <h2 id="cockpit-needs-title">Needs you</h2>
            <span className="dim">Answer here — work unblocks without opening a drawer</span>
          </div>
          <NeedsYouList
            items={needsYouItems}
            now={props.now}
            onOpen={props.onOpen}
            onChanged={props.onChanged ?? (() => {})}
          />
        </section>
      )}

      <div className="cockpit-lanes">
        {sortedGoals.map((goal) => {
          const archived =
            goal.archived === true ||
            props.items.get(goal.id)?.state === "retired";
          const hot = !archived && laneRank(goal, props.items) < 3;
          const defaultOpen = archived
            ? false
            : hot || filterState !== "all";
          const open = laneOpen[goal.id] ?? defaultOpen;
          return (
            <Swimlane
              key={goal.id}
              goal={goal}
              filterQuery={filterQuery}
              filterState={filterState}
              open={open}
              onOpenChange={(next) =>
                setLaneOpen((prev) => ({ ...prev, [goal.id]: next }))
              }
              {...props}
            />
          );
        })}
      </div>

      <HowItWorks />
    </div>
  );
}

function modeCopy({
  needsYou,
  running,
  inReview,
  backlog,
  done,
  totalTasks,
}: {
  needsYou: number;
  running: number;
  inReview: number;
  backlog: number;
  done: number;
  totalTasks: number;
}): { title: string; lede: string } {
  if (needsYou > 0) {
    return {
      title: `${needsYou} decision${needsYou === 1 ? "" : "s"} waiting on you`,
      lede: "Resolve the decision below to unblock that card. Everything else can wait.",
    };
  }
  if (running > 0) {
    return {
      title: `${running} task${running === 1 ? "" : "s"} running`,
      lede: "Agents are working. Expand a project for columns, or wait for the next decision.",
    };
  }
  if (inReview > 0) {
    return {
      title: `${inReview} ready for review`,
      lede: "Finished work is waiting. Review when you have time — nothing is blocked on you right now.",
    };
  }
  if (backlog > 0) {
    return {
      title: `${backlog} in backlog`,
      lede: "Nothing starts until you dispatch. Open a card and press Start, or use the MCP dispatch tool.",
    };
  }
  if (done === totalTasks && totalTasks > 0) {
    return {
      title: "All tasks done — nice work",
      lede: "Nothing in flight. Start another Project when you're ready.",
    };
  }
  return {
    title: "Quiet for now",
    lede: "No decisions and nothing running. Expand a project, or change the filter.",
  };
}

function HowItWorks() {
  return (
    <details className="cockpit-howto">
      <summary>How this works</summary>
      <ol className="cockpit-howto-steps">
        <li>
          <strong>Project</strong> — a goal. An Initial plan task writes the Plan.
        </li>
        <li>
          <strong>Approve Plan</strong> — tasks land in Backlog; dispatch to start a run.
        </li>
        <li>
          <strong>Needs you</strong> — an agent stopped; answer so it can continue.
        </li>
        <li>
          <strong>Review</strong> — finished work with a PR. You merge; honr never does.
        </li>
      </ol>
    </details>
  );
}

function NeedsYouList({
  items,
  now,
  onOpen,
  onChanged,
}: {
  items: WorkItem[];
  now: number;
  onOpen: (id: number) => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<number | null>(null);
  const [showDetail, setShowDetail] = useState<Record<number, boolean>>({});

  return (
    <div className="cockpit-needs-list">
      {items.map((item) => {
        const esc = item.escalation!;
        const waited = since(esc.blocked_since, now);
        const { summary, detail } = humanizeEscalation(esc.question);
        const openDetail = showDetail[item.id];
        return (
          <div className="cockpit-need" key={item.id}>
            <div className="cockpit-need-main">
              <button type="button" className="cockpit-need-title" onClick={() => onOpen(item.id)}>
                {item.title}
              </button>
              <p className="cockpit-need-q">{summary}</p>
              {detail && detail !== summary && (
                <div className="cockpit-need-detail">
                  <button
                    type="button"
                    className="linkish"
                    onClick={() =>
                      setShowDetail((s) => ({ ...s, [item.id]: !s[item.id] }))
                    }
                  >
                    {openDetail ? "Hide technical detail" : "Show technical detail"}
                  </button>
                  {openDetail && <pre className="cockpit-need-raw">{detail}</pre>}
                </div>
              )}
              <span className="dim">Waiting on you · {waited}</span>
            </div>
            <div className="cockpit-need-opts">
              {esc.options.map((o, i) => (
                <button
                  key={o.label}
                  type="button"
                  className={i === esc.recommended ? "primary" : ""}
                  disabled={busy === item.id}
                  title={o.detail || undefined}
                  onClick={() => {
                    setBusy(item.id);
                    api
                      .answer(item.id, o.label)
                      .then(onChanged)
                      .finally(() => setBusy(null));
                  }}
                >
                  {o.label}
                  {i === esc.recommended ? " · suggested" : ""}
                </button>
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/**
 * Swimlanes go by Project, never by agent. You care about "is billing v2 moving",
 * not "what is agent-7 up to".
 */
function Swimlane({
  goal,
  filterQuery,
  filterState,
  open,
  onOpenChange,
  ...p
}: CockpitProps & {
  goal: GoalView;
  filterQuery: string;
  filterState: BoardFilter;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const archived =
    goal.archived === true || p.items.get(goal.id)?.state === "retired";
  const hot = !archived && laneRank(goal, p.items) < 3;
  const urgent = !archived && goal.needs_you > 0;
  const [viewMode, setViewMode] = useState<"columns" | "graph">("columns");
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const story = p.stories.get(goal.id) ?? goal.story;
  const q = filterQuery.toLowerCase().trim();

  const mine = [...p.items.values()].filter((i) => {
    if (i.parent !== goal.id) return false;
    if (i.level === "Project") return false;
    if (
      q &&
      !i.title.toLowerCase().includes(q) &&
      !`#${i.id}`.includes(q) &&
      !(i.beads_id ?? "").toLowerCase().includes(q)
    )
      return false;
    if (!archived && filterState !== "all") {
      const colKey = COLUMN_OF[normState(i.state)];
      if (colKey !== filterState) return false;
    }
    return true;
  });

  useEffect(() => {
    if (filterState !== "all" && mine.length > 0 && !open) onOpenChange(true);
  }, [filterState, mine.length, open, onOpenChange]);

  const archiveProject = async (e: MouseEvent) => {
    e.stopPropagation();
    if (archiveBusy) return;
    setArchiveBusy(true);
    try {
      await api.cut(goal.id, "archived from cockpit");
      setConfirmArchive(false);
      p.onChanged?.();
    } catch (err) {
      console.error(err);
    } finally {
      setArchiveBusy(false);
    }
  };

  const planLabel = planStatusLabel(goal.plan_status, goal);
  const reviewCount =
    goal.columns.find((c) => c.column === "review")?.summary.count ??
    mine.filter((i) => i.state === "review").length;

  return (
    <section
      className={`lane ${archived ? "lane-archived" : urgent ? "lane-hot" : hot ? "" : "lane-quiet"} ${open ? "open" : ""}`}
    >
      <header
        className="lane-head"
        onClick={() => {
          setConfirmArchive(false);
          onOpenChange(!open);
        }}
      >
        <span className="chev">{open ? "▾" : "▸"}</span>
        <h2
          className="lane-title"
          title="Open project"
          onClick={(e) => {
            e.stopPropagation();
            p.onOpen(goal.id);
          }}
        >
          {goal.title}
        </h2>
        {archived && (
          <span className="pill" title="Soft-retired — history only">
            Archived
          </span>
        )}
        {!archived && goal.needs_you > 0 && (
          <span className="alarm">⚠ {goal.needs_you} need you</span>
        )}
        {!archived && goal.agents_live > 0 && (
          <span className="live">● {goal.agents_live} working</span>
        )}
        {!archived && reviewCount > 0 && goal.needs_you === 0 && (
          <span className="pill review-pill">{reviewCount} review</span>
        )}
        {!archived && planLabel && (
          <span className="dim plan-status">{planLabel}</span>
        )}
        <div className="progress">
          <div className="bar wide">
            <div className="fill" style={{ width: `${Math.round(goal.progress * 100)}%` }} />
          </div>
          <span className="dim">
            {archived
              ? `${mine.length} cards`
              : `${goal.leaves_done}/${goal.leaves_total}`}
          </span>
        </div>
        {(goal.spend_cents > 0 || goal.budget_cents != null) && (
          <span className="spend">
            {money(goal.spend_cents)}
            {goal.budget_cents != null && (
              <span className="dim"> / {money(goal.budget_cents)}</span>
            )}
          </span>
        )}

        {!archived && (
          <div className="lane-actions" onClick={(e) => e.stopPropagation()}>
            {!confirmArchive ? (
              <button
                type="button"
                className="dispatch-toggle archive-toggle"
                disabled={archiveBusy}
                onClick={() => setConfirmArchive(true)}
                title="Archive this Project and its subtree (retire, not delete)"
              >
                Archive
              </button>
            ) : (
              <span className="lane-archive-confirm">
                <span className="dim">Retire project?</span>
                <button
                  type="button"
                  className="dispatch-toggle archive-confirm"
                  disabled={archiveBusy}
                  onClick={archiveProject}
                >
                  Confirm
                </button>
                <button
                  type="button"
                  className="dispatch-toggle"
                  disabled={archiveBusy}
                  onClick={() => setConfirmArchive(false)}
                >
                  Cancel
                </button>
              </span>
            )}
            {open && (
              <div className="lane-view-switcher">
                <button
                  type="button"
                  className={`view-btn ${viewMode === "columns" ? "on" : ""}`}
                  onClick={() => setViewMode("columns")}
                  title="Kanban columns"
                >
                  Columns
                </button>
                <button
                  type="button"
                  className={`view-btn ${viewMode === "graph" ? "on" : ""}`}
                  onClick={() => setViewMode("graph")}
                  title="Dependency graph"
                  data-testid="toggle-graph-view"
                >
                  Graph
                </button>
              </div>
            )}
          </div>
        )}
      </header>

      {open && (
        <div className="lane-body">
          {story.length > 0 && (
            <details className="lane-story">
              <summary>
                Recent <span className="dim">· {Math.min(3, story.length)}</span>
              </summary>
              <ol>
                {story
                  .slice(-3)
                  .reverse()
                  .map((s, n) => (
                    <li key={`${s.at}-${n}`}>
                      <span className="dim">{since(s.at, p.now)}</span>
                      <span>{clip(s.text, 140)}</span>
                    </li>
                  ))}
              </ol>
            </details>
          )}

          {archived ? (
            mine.length === 0 ? (
              <div className="lane-empty dim">No cards in this archived project.</div>
            ) : (
              <div className="lane-archived-cards">
                {[...mine]
                  .sort((a, b) => a.id - b.id)
                  .map((item) => (
                    <Card
                      key={item.id}
                      item={item}
                      column="retired"
                      now={p.now}
                      heartbeatExpect={p.heartbeatExpect}
                      breadcrumb={p.breadcrumbOf(item.id)}
                      defaultEngine={p.defaultEngine}
                      defaultModel={p.defaultModel}
                      labelOf={(id) => labelOfItem(p.items, id)}
                      onOpen={p.onOpen}
                    />
                  ))}
              </div>
            )
          ) : viewMode === "graph" ? (
            <DependencyGraph items={mine} onOpen={p.onOpen} />
          ) : (
            <div
              className={`columns cols-${BOARD_COLUMNS.length}`}
              style={{ ["--board-cols" as string]: String(BOARD_COLUMNS.length) }}
            >
              {BOARD_COLUMNS.map((col) => {
                const cards = mine
                  .filter((i) => COLUMN_OF[normState(i.state)] === col.key)
                  .sort(sortFor(col.key));
                const summary = goal.columns.find((c) => c.column === col.key)?.summary;
                return (
                  <ColumnEl
                    key={col.key}
                    label={col.label}
                    question={col.question}
                    colKey={col.key}
                    cards={cards}
                    summary={summary?.text ?? ""}
                    {...p}
                  />
                );
              })}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function laneRank(goal: GoalView, items: Map<number, WorkItem>): number {
  if (goal.needs_you > 0) return 0;
  if (goal.agents_live > 0) return 1;
  const review =
    goal.columns.find((c) => c.column === "review")?.summary.count ??
    [...items.values()].filter((i) => i.parent === goal.id && i.state === "review")
      .length;
  if (review > 0) return 2;
  const backlog =
    goal.columns.find((c) => c.column === "backlog" || c.column === "ready")?.summary.count ??
    [...items.values()].filter((i) => i.parent === goal.id && normState(i.state) === "backlog")
      .length;
  if (backlog > 0) return 3;
  return 4;
}

function planStatusLabel(planStatus: string, goal: GoalView): string | null {
  if (planStatus === "awaiting_approval") return "plan needs approval";
  if (planStatus?.startsWith("approved")) return null;
  if (planStatus === "no_plan" || planStatus === "empty") {
    if (goal.leaves_total > 0) return null;
    return "awaiting plan";
  }
  return null;
}

function clip(text: string, max: number): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length <= max ? one : `${one.slice(0, max - 1)}…`;
}

/** Check whether an item has unresolved blockers. */
export function isBlocked(item: WorkItem): boolean {
  if (item.blockers && item.blockers.length > 0) {
    return item.blockers.some((b) => b.state !== "done" && b.state !== "retired");
  }
  return item.blocked_by ? item.blocked_by.length > 0 : false;
}

/** Sort Review by regret risk, not arrival time: blast radius × novelty.
    Sort Backlog by dispatchable first: unblocked cards sort above blocked cards. */
export function sortFor(key: ColumnKey) {
  if (key === "backlog" || key === "ready") {
    return (a: WorkItem, b: WorkItem) => {
      const aBlocked = isBlocked(a) ? 1 : 0;
      const bBlocked = isBlocked(b) ? 1 : 0;
      if (aBlocked !== bBlocked) {
        return aBlocked - bBlocked;
      }
      return new Date(a.entered_state_at).getTime() - new Date(b.entered_state_at).getTime();
    };
  }
  if (key === "review") {
    return (a: WorkItem, b: WorkItem) => {
      const risk = (i: WorkItem) => (i.diff_added + i.diff_removed) * (i.gate_failures + 1);
      return risk(b) - risk(a);
    };
  }
  if (key === "needs_you") {
    return (a: WorkItem, b: WorkItem) =>
      new Date(a.escalation?.blocked_since ?? a.entered_state_at).getTime() -
      new Date(b.escalation?.blocked_since ?? b.entered_state_at).getTime();
  }
  return (a: WorkItem, b: WorkItem) =>
    new Date(a.entered_state_at).getTime() - new Date(b.entered_state_at).getTime();
}

function ColumnEl({
  label,
  question,
  colKey,
  cards,
  summary,
  now,
  heartbeatExpect,
  defaultEngine,
  defaultModel,
  breadcrumbOf,
  items,
  onOpen,
}: CockpitProps & {
  label: string;
  question: string;
  colKey: ColumnKey;
  cards: WorkItem[];
  summary: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? cards : cards.slice(0, VISIBLE);
  const hidden = cards.length - shown.length;

  return (
    <div className={`column column-${colKey}`}>
      <div className="col-head" title={question}>
        {label} <span className="count">({cards.length})</span>
      </div>

      {shown.map((item) => (
        <Card
          key={item.id}
          item={item}
          column={colKey}
          now={now}
          heartbeatExpect={heartbeatExpect}
          defaultEngine={defaultEngine}
          defaultModel={defaultModel}
          breadcrumb={breadcrumbOf(item.id)}
          labelOf={(id) => labelOfItem(items, id)}
          onOpen={onOpen}
        />
      ))}

      {hidden > 0 && (
        <button type="button" className="chunk" onClick={() => setExpanded(true)}>
          {summary || `${hidden} more`}
        </button>
      )}
      {expanded && cards.length > VISIBLE && (
        <button type="button" className="chunk" onClick={() => setExpanded(false)}>
          collapse
        </button>
      )}
    </div>
  );
}
