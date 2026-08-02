import { useMemo, useState } from "react";
import { Cockpit } from "./components/Cockpit";
import { DetailDrawer } from "./components/Detail";
import { STALE_AFTER_MS, useBoard, useNow } from "./useBoard";
import { api, money } from "./api";
import type { WorkItem } from "./types";

export default function App() {
  const b = useBoard();
  const now = useNow();
  const [open, setOpen] = useState<number | null>(null);
  const [pauseBusy, setPauseBusy] = useState(false);

  const { goalOf, breadcrumbOf } = useMemo(() => buildLookups(b.items), [b.items]);

  const activeGoals = b.goals.filter((g) => b.items.get(g.id)?.state !== "retired");
  const totalNeedsYou = activeGoals.reduce((n, g) => n + g.needs_you, 0);
  const totalSpend = activeGoals.reduce((n, g) => n + g.spend_cents, 0);
  const totalBudget = activeGoals.reduce((n, g) => n + (g.budget_cents ?? 0), 0);
  const live = activeGoals.reduce((n, g) => n + g.agents_live, 0);

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
        <div className="brand">
          honr
          {totalNeedsYou > 0 && <span className="pip">{totalNeedsYou}</span>}
        </div>
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
          {!b.dispatchPaused && (
            <button
              type="button"
              className="dispatch-toggle"
              disabled={pauseBusy || !b.loaded}
              onClick={toggleDispatch}
              title="Pause all projects — then Resume individual ones as exceptions"
            >
              Pause all
            </button>
          )}
        </div>
      </header>

      {b.dispatchPaused && (
        <div className="info banner pause-banner" role="status">
          <p className="banner-text">
            <strong>All projects paused.</strong>{" "}
            Running cards keep going. Resume all, or resume one project in its
            swimlane to let only that subtree claim.
          </p>
          <button
            type="button"
            className="banner-action"
            disabled={pauseBusy || !b.loaded}
            onClick={toggleDispatch}
            title="Resume all projects — clear every project pause"
          >
            Resume all
          </button>
        </div>
      )}
      {staleFor !== null && (
        <div className="err banner">
          ⚠ NOT LIVE — showing state from {Math.round(staleFor / 1000)}s ago.
          honr is unreachable; nothing here is current.
          <button className="link" onClick={b.refresh}>
            retry now
          </button>
        </div>
      )}
      {b.error && staleFor === null && <div className="err banner">{b.error}</div>}

      <main className={open ? "with-drawer" : ""}>
        {!b.loaded ? (
          <div className="dim pad">loading…</div>
        ) : (
          <Cockpit
            goals={b.goals}
            items={b.items}
            stories={b.stories}
            goalOf={goalOf}
            breadcrumbOf={breadcrumbOf}
            now={now}
            heartbeatExpect={b.heartbeatExpect}
            dispatchPaused={b.dispatchPaused}
            defaultEngine={b.defaultEngine}
            defaultModel={b.defaultModel}
            onOpen={setOpen}
            onChanged={b.refresh}
          />
        )}

        {open != null && (
          <DetailDrawer
            id={open}
            now={now}
            defaultEngine={b.defaultEngine}
            defaultModel={b.defaultModel}
            onClose={() => setOpen(null)}
            onChanged={b.refresh}
          />
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
