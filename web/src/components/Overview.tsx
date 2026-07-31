import { useState } from "react";
import { money } from "../api";
import type { WorkItem } from "../types";

/**
 * The front door: what is this system doing?
 *
 * Not a triage inbox and not an operational board — this answers the question
 * someone has *before* they have either of those. You read the why-chain from
 * the top down and see how much of each branch is real, so you can hold an
 * informed conversation about the work without opening anything else.
 *
 * Every row drills into the drawer, which carries the full intent chain.
 */
export function Overview({
  items,
  onOpen,
}: {
  items: Map<number, WorkItem>;
  onOpen: (id: number) => void;
}) {
  const all = [...items.values()].filter((i) => i.state !== "retired");
  const roots = all.filter((i) => i.parent == null);
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());

  const toggle = (id: number) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  if (!roots.length) {
    return (
      <div className="overview empty">
        <h2>Nothing here yet</h2>
        <p>
          Work arrives by asking the agent for it — describe a goal, approve the
          breakdown it proposes, and the cards show up here.
        </p>
      </div>
    );
  }

  return (
    <div className="overview">
      {roots.map((r) => (
        <Node
          key={r.id}
          item={r}
          all={all}
          depth={0}
          collapsed={collapsed}
          toggle={toggle}
          onOpen={onOpen}
        />
      ))}
    </div>
  );
}

/** Everything a branch is worth knowing at a glance, computed over its subtree. */
interface Roll {
  leaves: number;
  done: number;
  running: number;
  needsYou: number;
  spend: number;
}

function rollup(item: WorkItem, all: WorkItem[]): Roll {
  const kids = all.filter((i) => i.parent === item.id);
  if (!kids.length) {
    return {
      leaves: 1,
      done: item.state === "done" ? 1 : 0,
      running: ["claimed", "running", "splitting"].includes(item.state) ? 1 : 0,
      needsYou: item.state === "needs_human" ? 1 : 0,
      spend: item.cost_cents,
    };
  }
  return kids
    .map((k) => rollup(k, all))
    .reduce((a, b) => ({
      leaves: a.leaves + b.leaves,
      done: a.done + b.done,
      running: a.running + b.running,
      needsYou: a.needsYou + b.needsYou,
      spend: a.spend + b.spend,
    }));
}

function Node({
  item,
  all,
  depth,
  collapsed,
  toggle,
  onOpen,
}: {
  item: WorkItem;
  all: WorkItem[];
  depth: number;
  collapsed: Set<number>;
  toggle: (id: number) => void;
  onOpen: (id: number) => void;
}) {
  const kids = all.filter((i) => i.parent === item.id);
  const isLeaf = kids.length === 0;
  const shut = collapsed.has(item.id);
  const roll = rollup(item, all);
  const machine = item.origin.kind !== "human";

  // Committed to and deliberately unelaborated is a *correct* state for work
  // months out, not an omission. Say so, or it reads as a gap.
  const named = item.above_line && isLeaf;

  return (
    <>
      <div
        className={`onode ${isLeaf ? "leaf" : "branch"} ${machine ? "machine" : ""}`}
        style={{ "--depth": depth } as React.CSSProperties}
        onClick={() => onOpen(item.id)}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => e.key === "Enter" && onOpen(item.id)}
      >
        <button
          className={`twist ${isLeaf ? "hidden" : ""}`}
          onClick={(e) => {
            e.stopPropagation();
            toggle(item.id);
          }}
          aria-label={shut ? "expand" : "collapse"}
        >
          {shut ? "▸" : "▾"}
        </button>

        <span className={`olevel lvl-${item.level ?? "story"}`}>{item.level ?? "·"}</span>
        <span className="otitle">{item.title}</span>

        {named && <span className="onote">named only</span>}
        {roll.needsYou > 0 && <span className="oalarm">⚠ {roll.needsYou}</span>}

        {isLeaf ? (
          <span className={`ostate s-${item.state}`}>{item.state.replace("_", " ")}</span>
        ) : (
          <>
            <Bar done={roll.done} total={roll.leaves} running={roll.running} />
            <span className="ocount">
              {roll.done}/{roll.leaves}
            </span>
          </>
        )}
        <span className="ospend">{roll.spend ? money(roll.spend) : ""}</span>
      </div>

      {/* The intent chain is the highest-leverage payload in the system, so a
          container shows its contract inline rather than hiding it a click
          away. Leaves don't — at that density it would drown the tree. */}
      {!isLeaf && !shut && (
        <div className="ointent" style={{ "--depth": depth } as React.CSSProperties}>
          {item.intent}
        </div>
      )}

      {!shut &&
        kids.map((k) => (
          <Node
            key={k.id}
            item={k}
            all={all}
            depth={depth + 1}
            collapsed={collapsed}
            toggle={toggle}
            onOpen={onOpen}
          />
        ))}
    </>
  );
}

function Bar({ done, total, running }: { done: number; total: number; running: number }) {
  const pct = total ? (done / total) * 100 : 0;
  const live = total ? (running / total) * 100 : 0;
  return (
    <span className="obar" title={`${done} done, ${running} running, of ${total}`}>
      <span className="obar-done" style={{ width: `${pct}%` }} />
      <span className="obar-live" style={{ width: `${live}%` }} />
    </span>
  );
}
