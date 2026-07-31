import { useMemo, useState } from "react";
import { Board } from "./components/Board";
import { DetailDrawer } from "./components/Detail";
import { Digest } from "./components/Digest";
import { Tree } from "./components/Tree";
import { STALE_AFTER_MS, useBoard, useNow } from "./useBoard";
import { money } from "./api";
import type { WorkItem } from "./types";

type Tab = "digest" | "board" | "tree";

export default function App() {
  const b = useBoard();
  const now = useNow();
  // Diff-first: the session opens on what changed, and the board is one click
  // behind it. If the digest works, the tab can stay closed.
  const [tab, setTab] = useState<Tab>("digest");
  const [open, setOpen] = useState<number | null>(null);

  const { goalOf, breadcrumbOf } = useMemo(() => buildLookups(b.items), [b.items]);

  const totalNeedsYou = b.goals.reduce((n, g) => n + g.needs_you, 0);
  const totalSpend = b.goals.reduce((n, g) => n + g.spend_cents, 0);
  const totalBudget = b.goals.reduce((n, g) => n + (g.budget_cents ?? 0), 0);
  const live = b.goals.reduce((n, g) => n + g.agents_live, 0);

  // How long we have been showing a picture we could not refresh.
  const age = b.lastLoadedAt === null ? null : now - b.lastLoadedAt;
  const staleFor = age !== null && age > STALE_AFTER_MS ? age : null;

  return (
    <div className="app">
      <header className="top">
        <div className="brand">honr</div>
        <nav>
          {(["digest", "board", "tree"] as Tab[]).map((t) => (
            <button key={t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
              {t === "digest" ? "Digest" : t === "board" ? "Board" : "Tree"}
            </button>
          ))}
        </nav>
        <div className="stats">
          <span>
            {money(totalSpend)}
            {totalBudget > 0 && <span className="dim"> / {money(totalBudget)}</span>}
          </span>
          <span className="live">● {live} agents live</span>
          {totalNeedsYou > 0 && <span className="alarm">⚠ {totalNeedsYou} need you</span>}
          <span className={b.connected ? "conn ok" : "conn off"}>
            {b.connected ? "live" : "reconnecting…"}
          </span>
        </div>
      </header>

      {/* The board keeps rendering its last good snapshot when a poll fails,
          which is right — blanking it would be worse. But it has to say so.
          Stale state that looks current is how you end up acting on a picture
          that stopped being true fifteen minutes ago. */}
      {staleFor !== null && (
        <div className="err bar">
          ⚠ NOT LIVE — showing state from {Math.round(staleFor / 1000)}s ago.
          honr is unreachable; nothing here is current.
          <button className="link" onClick={b.refresh}>retry now</button>
        </div>
      )}
      {b.error && staleFor === null && <div className="err bar">{b.error}</div>}

      <main className={open ? "with-drawer" : ""}>
        {!b.loaded ? (
          <div className="dim pad">loading board…</div>
        ) : tab === "digest" ? (
          <Digest onOpen={(id) => { setOpen(id); setTab("board"); }} onChanged={b.refresh} />
        ) : tab === "board" ? (
          <Board
            goals={b.goals}
            items={b.items}
            stories={b.stories}
            goalOf={goalOf}
            breadcrumbOf={breadcrumbOf}
            now={now}
            heartbeatExpect={b.heartbeatExpect}
            onOpen={setOpen}
          />
        ) : (
          <Tree items={b.items} onOpen={setOpen} />
        )}

        {open != null && (
          <DetailDrawer id={open} now={now} onClose={() => setOpen(null)} onChanged={b.refresh} />
        )}
      </main>

      <footer className="bottom dim">
        The board should never require watching. If your honest workflow is
        "keep the tab open and glance at it", the design has failed.
      </footer>
    </div>
  );
}

/** Goal lane and breadcrumb for every card, derived from the tree once. */
function buildLookups(items: Map<number, WorkItem>) {
  const chainOf = (id: number): number[] => {
    const out: number[] = [];
    let cur: number | null | undefined = id;
    while (cur != null && out.length < 32) {
      out.push(cur);
      cur = items.get(cur)?.parent ?? null;
    }
    return out.reverse();
  };

  const goalOf = (id: number) => {
    const c = chainOf(id);
    return c[1] ?? c[0] ?? id;
  };

  // The nearest meaningful ancestor, not six levels of concatenated prose.
  const breadcrumbOf = (id: number) => {
    const c = chainOf(id);
    const parent = c[c.length - 2];
    return parent != null ? (items.get(parent)?.title ?? "") : "";
  };

  return { goalOf, breadcrumbOf };
}
