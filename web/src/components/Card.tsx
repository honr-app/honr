import { money, secsSince, since } from "../api";
import type { ColumnKey, WorkItem } from "../types";

interface Props {
  item: WorkItem;
  column: ColumnKey;
  now: number;
  heartbeatExpect: number;
  breadcrumb?: string;
  onOpen: (id: number) => void;
}

/**
 * Card anatomy differs by column, because the question you're asking differs.
 * Everything on the face is here to answer that column's one question.
 */
export function Card({ item, column, now, heartbeatExpect, breadcrumb, onOpen }: Props) {
  const machine = item.origin.kind !== "human";

  // The one number that tells you an agent is thinking versus hung. It belongs
  // on the card face, not behind a click — and the card decays as it ages.
  const hbAge = item.lease ? secsSince(item.lease.last_heartbeat, now) : null;
  const stale = hbAge !== null && hbAge > heartbeatExpect;
  const decay = hbAge === null ? 0 : Math.min(1, Math.max(0, (hbAge - heartbeatExpect) / (heartbeatExpect * 4)));

  return (
    <div
      className={`card col-${column} ${machine ? "machine" : ""} ${stale ? "stale" : ""}`}
      style={decay ? { opacity: 1 - decay * 0.45, filter: `saturate(${1 - decay * 0.8})` } : undefined}
      onClick={() => onOpen(item.id)}
      title={
        machine
          ? item.origin.kind === "split"
            ? `Created by a splitting sibling (#${item.origin.from})`
            : `Created by the ${item.origin.kind}`
          : "Created by a human"
      }
    >
      <div className="card-title">
        <span className="id">#{item.id}</span> {item.title}
      </div>

      {column === "ready" && (
        <>
          <div className="row">
            <span className="tag">⊙ {item.capability ?? "any"}</span>
            <span className="dim">{since(item.entered_state_at, now)}</span>
          </div>
          {((item.blockers && item.blockers.length > 0) || item.blocked_by.length > 0) && (
            <div className="row blocked">
              ⊘ blocked by{" "}
              {item.blockers && item.blockers.length > 0
                ? item.blockers
                    .map((b) => `#${b.id} "${b.title}" (${b.state.replace("_", " ")})`)
                    .join(", ")
                : item.blocked_by.map((b) => `#${b}`).join(", ")}
            </div>
          )}
          {breadcrumb && <div className="crumb">↑ {breadcrumb}</div>}
        </>
      )}

      {column === "running" && (
        <>
          <div className="row">
            <span className="tag">◍ {item.model ?? "?"}</span>
            <span className={stale ? "hb stale" : "hb"}>♥ {hbAge ?? "—"}s</span>
            <span className="dim">{money(item.cost_cents)}</span>
          </div>
          <div className="bar">
            <div className="fill" style={{ width: `${Math.round(item.progress * 100)}%` }} />
          </div>
          <div className="row dim">
            <span>{Math.round(item.progress * 100)}%</span>
            <span>{since(item.entered_state_at, now)}</span>
          </div>
        </>
      )}

      {column === "needs_you" && item.escalation && (
        <>
          <div className="question">{item.escalation.question}</div>
          <div className="row">
            <span className="tag">{item.escalation.options.length} options</span>
            <span className="blocked-for">
              ⏱ blocked {since(item.escalation.blocked_since, now)}
            </span>
          </div>
        </>
      )}

      {column === "verify" && (
        <>
          <div className="row">
            <span className="tag">
              ⚙ {item.gates.find((g) => g.status !== "passed")?.name ?? "gates"}
            </span>
            <span className="dim">{since(item.entered_state_at, now)}</span>
          </div>
          {item.gate_failures > 0 && (
            <div className="row warn">✗ failed {item.gate_failures}× before</div>
          )}
        </>
      )}

      {column === "review" && (
        <>
          <div className="row">
            <span className="tag ok">✓ gates</span>
            <span className="diff">
              +{item.diff_added} −{item.diff_removed}
            </span>
          </div>
          {/* Review *is* the PR. Without a way to reach it the column asks a
              question you cannot answer from the board. */}
          {item.pr_url && (
            <a
              className="pr-link"
              href={item.pr_url}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => e.stopPropagation()}
            >
              ↗ {prLabel(item.pr_url)}
            </a>
          )}
          {/* Where it ran. The sandbox is gone by now, but the name is what
              the logs and any post-mortem are filed under. */}
          {item.environment && <div className="sandbox">⬚ {item.environment}</div>}
          {breadcrumb && <div className="crumb">↑ {breadcrumb}</div>}
        </>
      )}

      {column === "done" && (
        <div className="row dim">
          <span>
            +{item.diff_added} −{item.diff_removed}
          </span>
          <span>{money(item.cost_cents)}</span>
        </div>
      )}
    </div>
  );
}

/** `https://github.com/owner/repo/pull/1` -> `owner/repo#1`. */
function prLabel(url: string): string {
  const m = url.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/);
  return m ? `${m[1]}/${m[2]}#${m[3]}` : "pull request";
}
