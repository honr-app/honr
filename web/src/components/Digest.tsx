import { useEffect, useState } from "react";
import { api, money } from "../api";

interface NeedsYou {
  id: number;
  title: string;
  question: string;
  options: string[];
  recommended: number;
  blocked_secs: number;
}

interface GoalDigest {
  goal_id: number;
  goal: string;
  merged: number;
  spend_cents: number;
  budget_cents: number | null;
  needs_you: NeedsYou[];
  running: number;
  running_stalled: number;
  ready: number;
  in_review: number;
  latest_story: string | null;
}

/**
 * The primary interface for most sessions. Two taps to resolve both blockers
 * is the actual product; the board is where you go when something has gone
 * wrong.
 */
export function Digest({ onOpen, onChanged }: { onOpen: (id: number) => void; onChanged: () => void }) {
  const [goals, setGoals] = useState<GoalDigest[] | null>(null);

  const load = () => api.digest().then((d) => setGoals(d.goals)).catch(() => setGoals([]));
  useEffect(() => {
    load();
    const t = setInterval(load, 4000);
    return () => clearInterval(t);
  }, []);

  if (!goals) return <div className="digest dim">loading…</div>;

  return (
    <div className="digest">
      {goals.map((g) => (
        <div className="dgoal" key={g.goal_id}>
          <h3>{g.goal} — since this morning</h3>
          <div className="dline">
            <span className="ok">✓ {g.merged} items merged</span>
            <span className="dim">
              {money(g.spend_cents)}
              {g.budget_cents != null && ` of ${money(g.budget_cents)}`}
            </span>
          </div>

          {g.needs_you.length > 0 ? (
            <div className="dline">
              <span className="alarm">⚠ {g.needs_you.length} need you</span>
              <span className="chips">
                {g.needs_you.map((n) => (
                  <button
                    key={n.id}
                    className="chip"
                    onClick={() => onOpen(n.id)}
                    title={n.question}
                  >
                    {n.title}?
                  </button>
                ))}
              </span>
            </div>
          ) : (
            <div className="dline dim">⚠ nothing needs you</div>
          )}

          <div className="dline">
            <span>⟳ {g.running} running</span>
            <span className={g.running_stalled ? "warn" : "dim"}>
              {g.running_stalled ? `${g.running_stalled} stalled` : "all healthy"}
            </span>
          </div>
          <div className="dline dim">
            ◷ {g.ready} ready · {g.in_review} awaiting review
          </div>
          {g.latest_story && <div className="dstory">{g.latest_story}</div>}
        </div>
      ))}

      {/* Two taps from here should resolve a blocker. */}
      <QuickResolve goals={goals} onChanged={onChanged} />
    </div>
  );
}

function QuickResolve({ goals, onChanged }: { goals: GoalDigest[]; onChanged: () => void }) {
  const pending = goals.flatMap((g) => g.needs_you);
  const [busy, setBusy] = useState<number | null>(null);
  if (pending.length === 0) return null;

  return (
    <div className="quick">
      <div className="section-title">Resolve without opening the board</div>
      {pending.map((n) => (
        <div className="qrow" key={n.id}>
          <div className="qq">
            <b>#{n.id} {n.title}</b>
            <div className="dim">{n.question}</div>
          </div>
          <div className="btns">
            {n.options.map((o, i) => (
              <button
                key={o}
                className={i === n.recommended ? "primary" : ""}
                disabled={busy === n.id}
                onClick={() => {
                  setBusy(n.id);
                  api.answer(n.id, o).then(onChanged).finally(() => setBusy(null));
                }}
              >
                {o}
                {i === n.recommended && " ★"}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
