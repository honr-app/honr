import { useMemo, useState } from "react";
import { api, money, since } from "../api.js";
import type { GoalView, State, StoryLine, WorkItem } from "../types";

const PAGE_SIZE = 10;

type StatusFilter =
  | "all"
  | "active"
  | "needs_you"
  | "review"
  | "ready"
  | "done"
  | "shaping";

/**
 * Home — the front door.
 *
 * Friendly status, decisions that need you (one-tap), recent story across
 * Projects, compact Project cards, then a filterable/paginated issue list.
 */
export function Home({
  items,
  goals,
  now,
  onOpen,
  onOpenBoard,
  onChanged,
}: {
  items: Map<number, WorkItem>;
  goals: GoalView[];
  now: number;
  onOpen: (id: number) => void;
  onOpenBoard: () => void;
  onChanged: () => void;
}) {
  const all = [...items.values()].filter((i) => i.state !== "retired");
  const projects = all.filter((i) => i.parent == null);
  const tasks = all.filter((i) => i.parent != null);

  const needsYou = tasks.filter((t) => t.state === "needs_human" && t.escalation);
  const running = tasks.filter((t) =>
    ["claimed", "running", "splitting"].includes(t.state),
  ).length;
  const inReview = tasks.filter((t) => t.state === "review").length;
  const ready = tasks.filter((t) => t.state === "ready").length;
  const done = tasks.filter((t) => t.state === "done").length;
  const spend = tasks.reduce((s, t) => s + t.cost_cents, 0);

  const activeGoals = goals.filter((g) => items.get(g.id)?.state !== "retired");
  const recent = activeGoals
    .flatMap((g) =>
      (g.story ?? []).map((s) => ({
        ...s,
        project: g.title,
        projectId: g.id,
      })),
    )
    .sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime())
    .slice(0, 8);

  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [projectFilter, setProjectFilter] = useState<number | "all">("all");
  const [page, setPage] = useState(0);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return tasks
      .filter((t) => (projectFilter === "all" ? true : t.parent === projectFilter))
      .filter((t) => matchesStatus(t, status))
      .filter((t) => {
        if (!q) return true;
        const bid = (t.beads_id ?? "").toLowerCase();
        return (
          t.title.toLowerCase().includes(q) ||
          t.intent.toLowerCase().includes(q) ||
          `#${t.id}`.includes(q) ||
          bid.includes(q)
        );
      })
      .sort((a, b) => statusRank(a.state) - statusRank(b.state) || b.id - a.id);
  }, [tasks, query, status, projectFilter]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, pageCount - 1);
  const pageItems = filtered.slice(safePage * PAGE_SIZE, safePage * PAGE_SIZE + PAGE_SIZE);

  const setStatusFilter = (s: StatusFilter) => {
    setStatus(s);
    setPage(0);
  };
  const setProject = (id: number | "all") => {
    setProjectFilter(id);
    setPage(0);
  };

  if (!projects.length) {
    return (
      <div className="home">
        <header className="home-hero">
          <h1>Welcome to honr</h1>
          <p>
            This is the control plane for agent work. Create a Project — it
            seeds an Initial plan Task — approve the Plan artifact, and Tasks
            show up on the Board.
          </p>
        </header>
        <div className="home-empty-card">
          <h2>No projects yet</h2>
          <p className="dim">
            From Claude Code (with the honr MCP connected): create a Project,
            propose_breakdown to write the Plan, then Approve Plan. Agents stay
            idle until you approve.
          </p>
        </div>
      </div>
    );
  }

  const headline =
    needsYou.length > 0
      ? `${needsYou.length} decision${needsYou.length === 1 ? "" : "s"} waiting on you`
      : running > 0
        ? `${running} task${running === 1 ? "" : "s"} running`
        : inReview > 0
          ? `${inReview} ready for review`
          : ready > 0
            ? `${ready} ready to claim`
            : done === tasks.length && tasks.length > 0
              ? "All tasks done — nice work"
              : "Quiet for now";

  return (
    <div className="home">
      <header className="home-hero">
        <p className="home-kicker">Home</p>
        <h1>{headline}</h1>
        <p className="home-lede">
          {needsYou.length > 0
            ? "Answer below and work unblocks. Everything else can wait."
            : "A glance at your Projects — open the Board when you want the columns."}
        </p>
        <div className="home-stats">
          <Stat label="working" value={String(running)} accent={running > 0} />
          <Stat
            label="need you"
            value={String(needsYou.length)}
            accent={needsYou.length > 0}
            alarm={needsYou.length > 0}
          />
          <Stat label="in review" value={String(inReview)} />
          <Stat label="done" value={`${done}/${tasks.length}`} />
          <Stat label="spent" value={money(spend)} />
        </div>
        <button type="button" className="home-board-link" onClick={onOpenBoard}>
          Open Board →
        </button>
      </header>

      {needsYou.length > 0 && (
        <section className="home-section home-needs" aria-labelledby="home-needs-title">
          <div className="home-section-head">
            <h2 id="home-needs-title">Needs you</h2>
            <span className="dim">One tap answers — no need to open the Board</span>
          </div>
          <NeedsYouList items={needsYou} now={now} onOpen={onOpen} onChanged={onChanged} />
        </section>
      )}

      {recent.length > 0 && (
        <section className="home-section" aria-labelledby="home-recent-title">
          <div className="home-section-head">
            <h2 id="home-recent-title">Recent</h2>
            <span className="dim">what just happened</span>
          </div>
          <ol className="home-recent">
            {recent.map((s, i) => (
              <li key={`${s.at}-${i}`}>
                <span className="home-recent-when">{since(s.at, now)}</span>
                <span className="home-recent-mark" aria-hidden>
                  {storyMark(s)}
                </span>
                <div className="home-recent-body">
                  {activeGoals.length > 1 && (
                    <button
                      type="button"
                      className="home-recent-project"
                      onClick={() => onOpen(s.projectId)}
                    >
                      {s.project}
                    </button>
                  )}
                  <p>{clipStory(s.text)}</p>
                </div>
              </li>
            ))}
          </ol>
        </section>
      )}

      <section className="home-section" aria-labelledby="home-projects-title">
        <div className="home-section-head">
          <h2 id="home-projects-title">Projects</h2>
          <span className="dim">{projects.length} active</span>
        </div>
        <div className="home-projects">
          {projects.map((p) => {
            const kids = all.filter((i) => i.parent === p.id);
            const roll = rollup(kids);
            const selected = projectFilter === p.id;
            const g = goals.find((x) => x.id === p.id);
            const planStatus = g?.plan_status ?? p.plan?.status ?? "no_plan";
            const planLabel =
              planStatus === "awaiting_approval"
                ? "awaiting approval"
                : planStatus === "no_plan" || planStatus === "empty"
                  ? "no plan"
                  : planStatus.startsWith("approved")
                    ? planStatus.replace("approved_", "")
                    : planStatus;
            return (
              <article
                key={p.id}
                className={`home-project ${selected ? "selected" : ""}`}
              >
                <header className="home-project-head">
                  <button
                    type="button"
                    className="home-project-title"
                    onClick={() => setProject(selected ? "all" : p.id)}
                    title={selected ? "Clear project filter" : "Filter issues to this project"}
                  >
                    <span className="home-project-name">{p.title}</span>
                    <span
                      className={`home-pill ${
                        planStatus === "awaiting_approval" ? "alarm" : ""
                      }`}
                    >
                      {planLabel}
                    </span>
                    {roll.needsYou > 0 && (
                      <span className="home-pill alarm">{roll.needsYou} need you</span>
                    )}
                    {roll.running > 0 && (
                      <span className="home-pill live">{roll.running} working</span>
                    )}
                  </button>
                  <div className="home-project-meta">
                    <div className="home-progress">
                      <div className="bar wide">
                        <div
                          className="fill"
                          style={{
                            width: `${Math.round(
                              (roll.leaves ? roll.done / roll.leaves : 0) * 100,
                            )}%`,
                          }}
                        />
                      </div>
                      <span className="dim">
                        {roll.done}/{roll.leaves}
                      </span>
                    </div>
                    <span className="ospend">{roll.spend ? money(roll.spend) : "—"}</span>
                    <button
                      type="button"
                      className="home-project-open dim"
                      onClick={() => onOpen(p.id)}
                    >
                      details
                    </button>
                  </div>
                </header>
                <p className="home-intent">{p.intent}</p>
              </article>
            );
          })}
        </div>
      </section>

      <section className="home-section" aria-labelledby="home-issues-title">
        <div className="home-section-head">
          <h2 id="home-issues-title">Issues</h2>
          <span className="dim">
            {filtered.length} match{filtered.length === 1 ? "" : "es"}
            {filtered.length !== tasks.length ? ` · ${tasks.length} total` : ""}
          </span>
        </div>

        <div className="home-issue-controls">
          <input
            type="search"
            className="home-issue-search"
            placeholder="Filter by title, id, beads…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setPage(0);
            }}
          />
          {projects.length > 1 && (
            <select
              className="home-issue-select"
              value={projectFilter === "all" ? "all" : String(projectFilter)}
              onChange={(e) => {
                const v = e.target.value;
                setProject(v === "all" ? "all" : Number(v));
              }}
            >
              <option value="all">All projects</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.title}
                </option>
              ))}
            </select>
          )}
          <div className="home-issue-chips">
            {(
              [
                ["all", "All"],
                ["active", "Active"],
                ["needs_you", "Needs you"],
                ["review", "Review"],
                ["ready", "Ready"],
                ["shaping", "Shaping"],
                ["done", "Done"],
              ] as [StatusFilter, string][]
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                className={`filter-btn ${status === key ? "active" : ""}`}
                onClick={() => setStatusFilter(key)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {pageItems.length === 0 ? (
          <div className="home-issues-empty dim">No issues match these filters.</div>
        ) : (
          <ul className="home-tasks home-issues">
            {pageItems.map((t) => {
              const project = t.parent != null ? items.get(t.parent) : undefined;
              const blockersList = (
                t.blockers && t.blockers.length > 0
                  ? t.blockers
                  : t.blocked_by.map((id) => {
                      const found = items.get(id);
                      return {
                        id,
                        title: found?.title ?? `Task #${id}`,
                        state: found?.state ?? ("ready" as const),
                      };
                    })
              ).filter((b) => b.state !== "done" && b.state !== "retired");

              return (
                <li key={t.id}>
                  <button type="button" onClick={() => onOpen(t.id)}>
                    <span className={`ostate s-${t.state}`}>
                      {t.state.replace("_", " ")}
                    </span>
                    <span className="home-task-title">{t.title}</span>
                    {projects.length > 1 && project && (
                      <span className="home-task-project dim">{project.title}</span>
                    )}
                    {t.beads_id && !t.beads_id.startsWith("bd-honr-") && (
                      <span className="obeads">{t.beads_id}</span>
                    )}
                    {t.cost_cents > 0 && (
                      <span className="dim">{money(t.cost_cents)}</span>
                    )}
                  </button>
                  {blockersList.length > 0 && (
                    <div
                      className="owaiting blocker-chips"
                      data-testid="blocker-chips"
                      style={{ paddingLeft: 16 }}
                    >
                      <span className="blocker-label">⊘ waiting on</span>
                      {blockersList.map((b) => (
                        <span
                          key={b.id}
                          className={`blocker-chip state-${b.state}`}
                          title={`#${b.id}: ${b.title} (${b.state.replace("_", " ")})`}
                        >
                          <span className="blocker-id">#{b.id}</span>
                          <span className="blocker-title">{b.title}</span>
                          <span className="state-cue">{b.state.replace("_", " ")}</span>
                        </span>
                      ))}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        {filtered.length > PAGE_SIZE && (
          <div className="home-pager">
            <button
              type="button"
              disabled={safePage <= 0}
              onClick={() => setPage((p) => Math.max(0, p - 1))}
            >
              ← Prev
            </button>
            <span className="dim">
              {safePage * PAGE_SIZE + 1}–{Math.min(filtered.length, (safePage + 1) * PAGE_SIZE)} of{" "}
              {filtered.length}
            </span>
            <button
              type="button"
              disabled={safePage >= pageCount - 1}
              onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
            >
              Next →
            </button>
          </div>
        )}
      </section>
    </div>
  );
}

function Stat({
  label,
  value,
  accent,
  alarm,
}: {
  label: string;
  value: string;
  accent?: boolean;
  alarm?: boolean;
}) {
  return (
    <div className={`home-stat ${accent ? "on" : ""} ${alarm ? "alarm" : ""}`}>
      <span className="home-stat-value">{value}</span>
      <span className="home-stat-label">{label}</span>
    </div>
  );
}

function storyMark(s: StoryLine): string {
  const t = s.text;
  if (t.includes("approved") || t.includes("merged")) return "✓";
  if (t.includes("blocked") || t.includes("failed")) return "!";
  if (t.includes("Constraint") || t.includes("pinned")) return "·";
  if (t.includes("Scope cut")) return "×";
  return "·";
}

/** Keep the feed scannable — long agent dumps belong in the drawer, not Home. */
function clipStory(text: string, max = 160): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length <= max ? one : `${one.slice(0, max - 1)}…`;
}

function rollup(tasks: WorkItem[]) {
  return {
    leaves: tasks.length,
    done: tasks.filter((t) => t.state === "done").length,
    running: tasks.filter((t) =>
      ["claimed", "running", "splitting"].includes(t.state),
    ).length,
    needsYou: tasks.filter((t) => t.state === "needs_human").length,
    spend: tasks.reduce((s, t) => s + t.cost_cents, 0),
  };
}

function matchesStatus(t: WorkItem, status: StatusFilter): boolean {
  switch (status) {
    case "all":
      return true;
    case "active":
      return ["claimed", "running", "splitting", "needs_human", "verifying", "ready"].includes(
        t.state,
      );
    case "needs_you":
      return t.state === "needs_human";
    case "review":
      return t.state === "review";
    case "ready":
      return t.state === "ready";
    case "done":
      return t.state === "done";
    case "shaping":
      return t.state === "shaping" || t.state === "draft";
  }
}

/** Surface blockers and live work before the long tail of done. */
function statusRank(s: State): number {
  switch (s) {
    case "needs_human":
      return 0;
    case "running":
    case "claimed":
    case "splitting":
      return 1;
    case "verifying":
      return 2;
    case "review":
      return 3;
    case "ready":
      return 4;
    case "shaping":
    case "draft":
      return 5;
    case "done":
      return 6;
    case "retired":
      return 7;
  }
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

  return (
    <div className="home-needs-list">
      {items.map((item) => {
        const esc = item.escalation!;
        const blocked = since(esc.blocked_since, now);
        return (
          <div className="home-need" key={item.id}>
            <div className="home-need-main">
              <button type="button" className="home-need-title" onClick={() => onOpen(item.id)}>
                {item.title}
              </button>
              <p className="home-need-q">{esc.question}</p>
              <span className="dim">blocked {blocked}</span>
            </div>
            <div className="home-need-opts">
              {esc.options.map((o, i) => (
                <button
                  key={o.label}
                  type="button"
                  className={i === esc.recommended ? "primary" : ""}
                  disabled={busy === item.id}
                  onClick={() => {
                    setBusy(item.id);
                    api
                      .answer(item.id, o.label)
                      .then(onChanged)
                      .finally(() => setBusy(null));
                  }}
                >
                  {o.label}
                  {i === esc.recommended ? " ★" : ""}
                </button>
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}
