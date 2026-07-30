import type { WorkItem } from "../types";

/**
 * The commitment line rendered literally. Above it: human-approved, stable,
 * what goes in a release plan. Below it: agents create, split and retire
 * freely. Dashed borders mark machine-born nodes so the tree stays honest
 * about what a person actually asked for.
 */
export function Tree({
  items,
  onOpen,
}: {
  items: Map<number, WorkItem>;
  onOpen: (id: number) => void;
}) {
  const all = [...items.values()];
  const roots = all.filter((i) => i.parent == null);

  return (
    <div className="tree">
      <div className="line-label above">
        ABOVE THE LINE · human-approved · quarterly cadence
      </div>
      <div className="tree-body">
        {roots.map((r) => (
          <Node key={r.id} item={r} items={all} depth={0} onOpen={onOpen} />
        ))}
      </div>
    </div>
  );
}

function Node({
  item,
  items,
  depth,
  onOpen,
}: {
  item: WorkItem;
  items: WorkItem[];
  depth: number;
  onOpen: (id: number) => void;
}) {
  const kids = items.filter((i) => i.parent === item.id);
  const machine = item.origin.kind !== "human";
  const unelaborated = item.above_line && kids.length === 0;

  const why =
    item.origin.kind === "split"
      ? `split out of #${item.origin.from}`
      : item.origin.kind === "human"
        ? "asked for by a human"
        : `created by the ${item.origin.kind}`;

  return (
    <>
      {/* The line falls where committed structure stops and churn begins. */}
      {depth > 0 && !item.above_line && isFirstBelow(item, items) && (
        <div className="line-label below">
          BELOW THE LINE · agent-owned · hourly churn
        </div>
      )}

      <div
        className={`tnode ${item.above_line ? "above" : "below"} ${machine ? "machine" : ""} ${
          item.state === "retired" ? "retired" : ""
        }`}
        style={{ marginLeft: depth * 18 }}
        onClick={() => onOpen(item.id)}
        title={`#${item.id} — ${why}`}
      >
        <span className="tlevel">{item.level ?? "·"}</span>
        <span className="ttitle">{item.title}</span>
        {unelaborated && <span className="tnote">named only, not elaborated</span>}
        <span className="tstate">{item.state}</span>
      </div>

      {kids.map((k) => (
        <Node key={k.id} item={k} items={items} depth={depth + 1} onOpen={onOpen} />
      ))}
    </>
  );
}

/** First below-line child under an above-line parent — where the line sits. */
function isFirstBelow(item: WorkItem, items: WorkItem[]): boolean {
  const parent = items.find((i) => i.id === item.parent);
  if (!parent?.above_line) return false;
  const siblings = items.filter((i) => i.parent === parent.id && !i.above_line);
  return siblings[0]?.id === item.id;
}
