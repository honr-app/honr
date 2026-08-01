import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { WorkItem } from "../types";

interface Props {
  items: WorkItem[];
  onOpen: (id: number) => void;
}

interface Edge {
  from: number;
  to: number;
  fromItem?: WorkItem;
  toItem?: WorkItem;
  isUnresolved: boolean;
}

interface NodePort {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

/** Pixel coords of `el` in `ancestor`'s scrolled content space (SVG local). */
function contentOffset(el: Element, ancestor: HTMLElement): { left: number; top: number; width: number; height: number } {
  const er = el.getBoundingClientRect();
  const ar = ancestor.getBoundingClientRect();
  return {
    left: er.left - ar.left + ancestor.scrollLeft,
    top: er.top - ar.top + ancestor.scrollTop,
    width: er.width,
    height: er.height,
  };
}

export function DependencyGraph({ items, onOpen }: Props) {
  const uid = useId().replace(/:/g, "");
  const markerUnresolved = `arrow-unresolved-${uid}`;
  const markerResolved = `arrow-resolved-${uid}`;
  const markerHover = `arrow-hover-${uid}`;

  const [hoveredNode, setHoveredNode] = useState<number | null>(null);
  const [hoveredEdge, setHoveredEdge] = useState<{ from: number; to: number } | null>(null);
  const [simplifyDone, setSimplifyDone] = useState<boolean>(false);
  const [search, setSearch] = useState<string>("");

  // SVG lives inside the scrollable canvas — measure against this, not the outer shell.
  const canvasRef = useRef<HTMLDivElement>(null);
  const ranksRef = useRef<HTMLDivElement>(null);
  const [nodeCoords, setNodeCoords] = useState<Map<number, NodePort>>(new Map());
  const [svgSize, setSvgSize] = useState({ w: 0, h: 0 });

  // Filter items if simplifyDone or search is active
  const filteredItems = useMemo(() => {
    let list = items;
    if (search.trim()) {
      const q = search.toLowerCase();
      list = list.filter((i) => i.id.toString().includes(q) || i.title.toLowerCase().includes(q));
    }
    if (simplifyDone) {
      // Hide done items that have no active outgoing edges to non-done items
      const activeBlockerIds = new Set<number>();
      for (const i of items) {
        if (i.state !== "done" && i.blocked_by) {
          for (const b of i.blocked_by) activeBlockerIds.add(b);
        }
      }
      list = list.filter((i) => i.state !== "done" || activeBlockerIds.has(i.id));
    }
    return list;
  }, [items, simplifyDone, search]);

  const { ranks, maxRank, edges, itemMap } = useMemo(() => {
    const map = new Map<number, WorkItem>();
    for (const item of filteredItems) {
      map.set(item.id, item);
    }

    // Topological depth/rank computation
    const rMap = new Map<number, number>();
    const getDepth = (id: number, visited = new Set<number>()): number => {
      if (rMap.has(id)) return rMap.get(id)!;
      if (visited.has(id)) return 0;
      visited.add(id);

      const item = map.get(id);
      if (!item || !item.blocked_by || item.blocked_by.length === 0) {
        rMap.set(id, 0);
        return 0;
      }

      let maxB = 0;
      for (const bId of item.blocked_by) {
        if (map.has(bId)) {
          maxB = Math.max(maxB, getDepth(bId, visited) + 1);
        }
      }
      rMap.set(id, maxB);
      return maxB;
    };

    for (const item of filteredItems) {
      getDepth(item.id);
    }

    // Group items by rank
    const groups = new Map<number, WorkItem[]>();
    let maxR = 0;
    for (const item of filteredItems) {
      const r = rMap.get(item.id) ?? 0;
      maxR = Math.max(maxR, r);
      if (!groups.has(r)) groups.set(r, []);
      groups.get(r)!.push(item);
    }

    // Edges
    const edgesList: Edge[] = [];
    for (const item of filteredItems) {
      if (item.blocked_by) {
        for (const bId of item.blocked_by) {
          if (map.has(bId)) {
            const blocker = map.get(bId)!;
            edgesList.push({
              from: bId,
              to: item.id,
              fromItem: blocker,
              toItem: item,
              isUnresolved: blocker.state !== "done",
            });
          }
        }
      }
    }

    return { ranks: groups, maxRank: maxR, edges: edgesList, itemMap: map };
  }, [filteredItems]);

  // Compute node ports in the SVG's content coordinate space.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const updateCoords = () => {
      const ranksEl = ranksRef.current;
      if (!ranksEl) return;

      const newCoords = new Map<number, NodePort>();
      for (const item of filteredItems) {
        const el = document.getElementById(`graph-node-${item.id}`);
        if (!el) continue;
        const box = contentOffset(el, canvas);
        newCoords.set(item.id, {
          x1: box.left,
          y1: box.top + box.height / 2,
          x2: box.left + box.width,
          y2: box.top + box.height / 2,
        });
      }
      setNodeCoords(newCoords);
      setSvgSize({
        w: Math.max(ranksEl.scrollWidth, canvas.clientWidth),
        h: Math.max(ranksEl.scrollHeight, canvas.clientHeight),
      });
    };

    updateCoords();
    // Second pass after fonts/layout settle (card heights can shift).
    const raf = requestAnimationFrame(updateCoords);

    const ro = new ResizeObserver(updateCoords);
    ro.observe(canvas);
    if (ranksRef.current) ro.observe(ranksRef.current);
    window.addEventListener("resize", updateCoords);
    canvas.addEventListener("scroll", updateCoords);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener("resize", updateCoords);
      canvas.removeEventListener("scroll", updateCoords);
    };
  }, [filteredItems, ranks]);

  // Sets for highlighting hovered connections
  const highlightedNodes = useMemo(() => {
    const set = new Set<number>();
    if (hoveredNode !== null) {
      set.add(hoveredNode);
      // Add direct blockers and direct dependents
      for (const e of edges) {
        if (e.to === hoveredNode) set.add(e.from);
        if (e.from === hoveredNode) set.add(e.to);
      }
    } else if (hoveredEdge) {
      set.add(hoveredEdge.from);
      set.add(hoveredEdge.to);
    }
    return set;
  }, [hoveredNode, hoveredEdge, edges]);

  // Plain-language status banner text
  const statusBanner = useMemo(() => {
    if (hoveredEdge) {
      const from = itemMap.get(hoveredEdge.from);
      const to = itemMap.get(hoveredEdge.to);
      if (from && to) {
        return `⊘ Task #${from.id} "${from.title}" (${from.state.replace("_", " ")}) BLOCKS Task #${to.id} "${to.title}" (${to.state.replace("_", " ")})`;
      }
    }
    if (hoveredNode !== null) {
      const item = itemMap.get(hoveredNode);
      if (item) {
        const blockers = (item.blocked_by || [])
          .map((id) => {
            const b = itemMap.get(id);
            return b ? `#${b.id} "${b.title}" (${b.state})` : `#${id}`;
          })
          .join(", ");
        const dependents = edges
          .filter((e) => e.from === item.id)
          .map((e) => `#${e.toItem?.id ?? e.to} "${e.toItem?.title ?? ""}"`)
          .join(", ");

        if (blockers && dependents) {
          return `⚡ Task #${item.id} "${item.title}": BLOCKED BY [${blockers}] — BLOCKS [${dependents}]`;
        } else if (blockers) {
          return `⊘ Task #${item.id} "${item.title}" is WAITING ON: ${blockers}`;
        } else if (dependents) {
          return `⚡ Task #${item.id} "${item.title}" BLOCKS: ${dependents}`;
        } else {
          return `✓ Task #${item.id} "${item.title}" (${item.state}) has no active dependency constraints`;
        }
      }
    }
    return `◈ Task Dependency Graph (${filteredItems.length} tasks, ${edges.length} edges). Hover any task or edge to highlight dependencies.`;
  }, [hoveredNode, hoveredEdge, itemMap, edges, filteredItems.length]);

  return (
    <div className="graph-container" data-testid="graph-container">
      {/* Graph Toolbar */}
      <div className="graph-toolbar">
        <div className="graph-title">
          <span>☩ Visual Dependency DAG</span>
          <span className="dim">({edges.length} dependency edges)</span>
        </div>
        <div className="graph-controls">
          <label className="toggle-label">
            <input
              type="checkbox"
              checked={simplifyDone}
              onChange={(e) => setSimplifyDone(e.target.checked)}
              data-testid="toggle-simplify"
            />
            Collapse done tasks
          </label>
          <input
            type="text"
            className="graph-search"
            placeholder="Filter tasks..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
      </div>

      {/* Plain-language explanation banner */}
      <div className="graph-banner" data-testid="graph-banner">
        {statusBanner}
      </div>

      {/* SVG Edges Canvas */}
      <div className="graph-canvas" ref={canvasRef}>
        <svg
          className="graph-svg"
          width={svgSize.w || "100%"}
          height={svgSize.h || "100%"}
          style={{ width: svgSize.w || "100%", height: svgSize.h || "100%", position: "absolute", top: 0, left: 0 }}
        >
          <defs>
            <marker
              id={markerUnresolved}
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto"
              markerUnits="strokeWidth"
            >
              <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--warn)" />
            </marker>
            <marker
              id={markerResolved}
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto"
              markerUnits="strokeWidth"
            >
              <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--ok)" />
            </marker>
            <marker
              id={markerHover}
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="6"
              markerHeight="6"
              orient="auto"
              markerUnits="strokeWidth"
            >
              <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--accent)" />
            </marker>
          </defs>

          {edges.map((e) => {
            const p1 = nodeCoords.get(e.from);
            const p2 = nodeCoords.get(e.to);
            if (!p1 || !p2) return null;

            const isHovered =
              (hoveredEdge?.from === e.from && hoveredEdge?.to === e.to) ||
              (hoveredNode !== null && (hoveredNode === e.from || hoveredNode === e.to));

            const x1 = p1.x2;
            const y1 = p1.y1;
            const x2 = p2.x1;
            const y2 = p2.y2;
            const dx = Math.max(30, Math.abs(x2 - x1) * 0.45);

            const pathD = `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
            const markerId = isHovered
              ? `url(#${markerHover})`
              : e.isUnresolved
                ? `url(#${markerUnresolved})`
                : `url(#${markerResolved})`;

            return (
              <g key={`edge-${e.from}-${e.to}`}>
                <path
                  d={pathD}
                  className={`graph-edge-path ${e.isUnresolved ? "unresolved" : "resolved"} ${
                    isHovered ? "hovered" : ""
                  }`}
                  markerEnd={markerId}
                  onMouseEnter={() => setHoveredEdge({ from: e.from, to: e.to })}
                  onMouseLeave={() => setHoveredEdge(null)}
                />
              </g>
            );
          })}
        </svg>

        {/* Nodes Grid organized by Topological Rank Columns */}
        <div className="graph-ranks" ref={ranksRef}>
          {Array.from({ length: maxRank + 1 }).map((_, rankIdx) => {
            const rankItems = ranks.get(rankIdx) || [];
            if (rankItems.length === 0) return null;

            return (
              <div key={`rank-${rankIdx}`} className="graph-rank-col">
                <div className="rank-head">
                  <span className="rank-badge">Step {rankIdx + 1}</span>
                  <span className="dim">({rankItems.length})</span>
                </div>

                <div className="rank-nodes">
                  {rankItems.map((item) => {
                    const isHovered = hoveredNode === item.id || highlightedNodes.has(item.id);
                    const blockersList = (item.blocked_by || [])
                      .map((id) => itemMap.get(id))
                      .filter(Boolean) as WorkItem[];

                    return (
                      <div
                        key={item.id}
                        id={`graph-node-${item.id}`}
                        className={`graph-node-card state-${item.state} ${isHovered ? "highlighted" : ""}`}
                        onClick={() => onOpen(item.id)}
                        onMouseEnter={() => setHoveredNode(item.id)}
                        onMouseLeave={() => setHoveredNode(null)}
                        data-testid={`graph-node-${item.id}`}
                      >
                        <div className="node-head">
                          <span className="node-id">#{item.id}</span>
                          <span className={`node-state-badge state-${item.state}`}>
                            {item.state.replace("_", " ")}
                          </span>
                        </div>

                        <div className="node-title">{item.title}</div>

                        {blockersList.length > 0 ? (
                          <div className="node-blockers">
                            <span className="blocker-cue">⊘ blocked by</span>
                            {blockersList.map((b) => (
                              <span
                                key={b.id}
                                className={`node-blocker-tag state-${b.state}`}
                                title={`#${b.id} ${b.title} (${b.state})`}
                              >
                                #{b.id} {b.title}
                              </span>
                            ))}
                          </div>
                        ) : (
                          <div className="node-unblocked">✓ ready / unblocked</div>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
