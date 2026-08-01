import { useState } from "react";
import { Card } from "./Card";
import { DependencyGraph } from "./DependencyGraph";
import { BOARD_COLUMNS, COLUMN_OF } from "../types";
import type { ColumnKey, GoalView, StoryLine, WorkItem } from "../types";
import { money, since } from "../api";

interface Props {
  goals: GoalView[];
  items: Map<number, WorkItem>;
  stories: Map<number, StoryLine[]>;
  goalOf: (id: number) => number;
  breadcrumbOf: (id: number) => string;
  now: number;
  heartbeatExpect: number;
  onOpen: (id: number) => void;
}

/** How many cards to show before the rest becomes a chunk. */
const VISIBLE = 4;

export function Board(props: Props) {
  return (
    <div className="board">
      {props.goals.map((goal) => (
        <Swimlane key={goal.id} goal={goal} {...props} />
      ))}
    </div>
  );
}

/**
 * Swimlanes go by goal, never by agent. You care about "is billing v2 moving",
 * not "what is agent-7 up to".
 */
function Swimlane({ goal, ...p }: Props & { goal: GoalView }) {
  const [open, setOpen] = useState(true);
  const [viewMode, setViewMode] = useState<"columns" | "graph">("columns");
  const story = p.stories.get(goal.id) ?? goal.story;
  const mine = [...p.items.values()].filter((i) => p.goalOf(i.id) === goal.id);

  return (
    <section className="lane">
      <header className="lane-head" onClick={() => setOpen(!open)}>
        <span className="chev">{open ? "▾" : "▸"}</span>
        <h2>◈ {goal.title}</h2>
        <div className="progress">
          <div className="bar wide">
            <div className="fill" style={{ width: `${Math.round(goal.progress * 100)}%` }} />
          </div>
          <span className="dim">
            {goal.leaves_done}/{goal.leaves_total}
          </span>
        </div>
        <span className="spend">
          {money(goal.spend_cents)}
          {goal.budget_cents != null && <span className="dim"> / {money(goal.budget_cents)}</span>}
        </span>
        <span className="live">● {goal.agents_live} live</span>
        {goal.needs_you > 0 && <span className="alarm">⚠ {goal.needs_you} need you</span>}

        <div className="lane-view-switcher" onClick={(e) => e.stopPropagation()}>
          <button
            className={`view-btn ${viewMode === "columns" ? "on" : ""}`}
            onClick={() => setViewMode("columns")}
            title="Kanban Columns View"
          >
            ▤ Columns
          </button>
          <button
            className={`view-btn ${viewMode === "graph" ? "on" : ""}`}
            onClick={() => setViewMode("graph")}
            title="Visual Dependency Graph View"
            data-testid="toggle-graph-view"
          >
            ☩ Dependency Graph
          </button>
        </div>
      </header>

      {open && (
        <>
          {/* Above the columns, not below them: this is what just happened,
              which is context for reading the board rather than a footnote to
              it. Goal-level, so it stands whether or not a card is selected. */}
          {story.length > 0 && (
            <div className="story">
              <span className="story-label">what changed</span>
              <div className="story-lines">
                {story.slice(-3).reverse().map((s, n) => (
                  <div key={n} className="story-line">
                    {/* A narrative with no time is not a narrative — you could
                        not tell a line from 30s ago from one from last week. */}
                    <span className="story-when">{since(s.at, p.now)}</span>
                    <span>{s.text}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {viewMode === "graph" ? (
            <DependencyGraph items={mine} onOpen={p.onOpen} />
          ) : (
            <div className="columns">
              {BOARD_COLUMNS.map((col) => {
                const cards = mine
                  .filter((i) => COLUMN_OF[i.state] === col.key)
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
        </>
      )}
    </section>
  );
}

/** Sort Review by regret risk, not arrival time: blast radius × novelty. */
function sortFor(key: ColumnKey) {
  if (key === "review") {
    return (a: WorkItem, b: WorkItem) => {
      const risk = (i: WorkItem) => (i.diff_added + i.diff_removed) * (i.gate_failures + 1);
      return risk(b) - risk(a);
    };
  }
  if (key === "needs_you") {
    // Longest blocked first — every minute costs throughput.
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
  breadcrumbOf,
  onOpen,
}: Props & {
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
          breadcrumb={breadcrumbOf(item.id)}
          onOpen={onOpen}
        />
      ))}

      {/* Chunked, not compressed. `+7 more` hides seven items and tells you
          nothing; this is smaller *and* answers the column's question. */}
      {hidden > 0 && (
        <button className="chunk" onClick={() => setExpanded(true)}>
          {summary || `${hidden} more`}
        </button>
      )}
      {expanded && cards.length > VISIBLE && (
        <button className="chunk" onClick={() => setExpanded(false)}>
          collapse
        </button>
      )}
      {cards.length === 0 && <div className="empty">—</div>}
    </div>
  );
}
