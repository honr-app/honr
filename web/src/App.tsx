import { useMemo, useState } from "react";
import { Board } from "./components/Board";
import { DetailDrawer } from "./components/Detail";
import { Home } from "./components/Home";
import { STALE_AFTER_MS, useBoard, useNow } from "./useBoard";
import { api, money } from "./api";
import type { WorkItem } from "./types";

type Tab = "home" | "board";

export default function App() {
  const b = useBoard();
  const now = useNow();
  const [tab, setTab] = useState<Tab>("home");
  const [open, setOpen] = useState<number | null>(null);
  const [pauseBusy, setPauseBusy] = useState(false);

  const { goalOf, breadcrumbOf } = useMemo(() => buildLookups(b.items), [b.items]);

  const totalNeedsYou = b.goals.reduce((n, g) => n + g.needs_you, 0);
  const totalSpend = b.goals.reduce((n, g) => n + g.spend_cents, 0);
  const totalBudget = b.goals.reduce((n, g) => n + (g.budget_cents ?? 0), 0);
  const live = b.goals.reduce((n, g) => n + g.agents_live, 0);

  const age = b.lastLoadedAt === null ? null : now - b.lastLoadedAt;
  const staleFor = age !== null && age > STALE_AFTER_MS ? age : null;

  const toggleDispatch = async () => {
    if (pauseBusy) return;
    setPauseBusy(true);
    try {
      if (b.dispatchPaused) await api.resumeDispatch();
      else await api.pauseDispatch();
      await b.refresh();
    } catch (e) {
      console.error(e);
    } finally {
      setPauseBusy(false);
    }
  };

  return (
    <div className="app">
      <header className="top">
        <div className="brand">honr</div>
        <nav>
          {(
            [
              ["home", "Home"],
              ["board", "Board"],
            ] as [Tab, string][]
          ).map(([t, label]) => (
            <button key={t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
              {label}
              {t === "home" && totalNeedsYou > 0 && <span className="pip">{totalNeedsYou}</span>}
            </button>
          ))}
        </nav>
        <div className="stats">
          <span className="dim">spent</span>
          <span>
            {money(totalSpend)}
            {totalBudget > 0 && <span className="dim"> of {money(totalBudget)}</span>}
          </span>
          <span className="sep">·</span>
          <span className="live">{live} working</span>
          <span className={b.connected ? "conn ok" : "conn off"}>
            {b.connected ? "live" : "reconnecting…"}
          </span>
          <button
            type="button"
            className={b.dispatchPaused ? "dispatch-toggle paused" : "dispatch-toggle"}
            disabled={pauseBusy || !b.loaded}
            onClick={toggleDispatch}
            title={
              b.dispatchPaused
                ? "Resume all projects — clear every project pause"
                : "Pause all projects — then Resume individual ones as exceptions"
            }
          >
            {b.dispatchPaused ? "Resume all" : "Pause all"}
          </button>
        </div>
      </header>

      {b.dispatchPaused && (
        <div className="info banner">
          All projects paused — running cards continue. Resume a project below to
          let only that subtree claim again, or Resume all in the header.
        </div>
      )}
      {staleFor !== null && (
        <div className="err banner">
          ⚠ NOT LIVE — showing state from {Math.round(staleFor / 1000)}s ago.
          honr is unreachable; nothing here is current.
          <button className="link" onClick={b.refresh}>retry now</button>
        </div>
      )}
      {b.error && staleFor === null && <div className="err banner">{b.error}</div>}

      <main className={open ? "with-drawer" : ""}>
        {!b.loaded ? (
          <div className="dim pad">loading…</div>
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
            onChanged={b.refresh}
          />
        ) : (
          <Home
            items={b.items}
            goals={b.goals}
            now={now}
            onOpen={setOpen}
            onOpenBoard={() => setTab("board")}
            onChanged={b.refresh}
          />
        )}

        {open != null && (
          <DetailDrawer id={open} now={now} onClose={() => setOpen(null)} onChanged={b.refresh} />
        )}
      </main>
    </div>
  );
}

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
    return c[0] ?? id;
  };

  const breadcrumbOf = (id: number) => {
    const c = chainOf(id);
    const parent = c[c.length - 2];
    return parent != null ? (items.get(parent)?.title ?? "") : "";
  };

  return { goalOf, breadcrumbOf };
}
