import { useEffect, useRef, useState } from "react";
import { api, money, since } from "../api";
import type { WorkItem } from "../types";

interface Detail extends WorkItem {
  ancestry: { level: string; title: string; intent: string }[];
  constraints: string[];
  children: number[];
}

function formatToolTarget(name: string, input: any): string {
  if (!input) return "";
  if (typeof input === "string") return input;

  if (name === "TaskCreate" || input.subject) {
    const subj = input.subject || "";
    const desc = input.description ? ` — ${input.description}` : "";
    return `"${subj}"${desc}`;
  }

  if (name === "TaskUpdate" || input.taskId) {
    const status = input.status ? ` → ${input.status}` : "";
    const notes = input.notes ? ` (${input.notes})` : "";
    return `Task #${input.taskId || "?"}${status}${notes}`;
  }

  if (input.command) return input.command;
  if (input.file_path) return input.file_path;
  if (input.path) return input.path;
  if (input.file) return input.file;
  if (input.pattern) return `"${input.pattern}" in ${input.path || input.directory || "."}`;

  return JSON.stringify(input);
}

function parseClaudeLogLine(
  line: string
): { text: string; type: "text" | "tool" | "result" | "system" | "error" } | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  if (!trimmed.startsWith("{")) {
    return { text: line, type: "text" };
  }

  try {
    const obj = JSON.parse(trimmed);

    if (["message_start", "message_delta", "message_stop", "content_block_stop", "ping"].includes(obj.type)) {
      return null;
    }

    if (obj.type === "content_block_start" && obj.content_block) {
      const cb = obj.content_block;
      if (cb.type === "tool_use" || cb.name) {
        return { text: `🔨 [${cb.name || "tool"}] ${formatToolTarget(cb.name, cb.input)}`, type: "tool" };
      }
      if (cb.type === "text" && cb.text) {
        return { text: cb.text, type: "text" };
      }
      return null;
    }

    if (obj.type === "content_block_delta" && obj.delta) {
      if (obj.delta.text) {
        return { text: obj.delta.text, type: "text" };
      }
      if (obj.delta.type === "text_delta" && obj.delta.text) {
        return { text: obj.delta.text, type: "text" };
      }
      return null;
    }

    if (obj.type === "tool_use" || obj.name) {
      const name = obj.name || "tool";
      return { text: `🔨 [${name}] ${formatToolTarget(name, obj.input)}`, type: "tool" };
    }

    if (obj.type === "tool_result") {
      const content = typeof obj.content === "string" ? obj.content : JSON.stringify(obj.content ?? "");
      return { text: `⚙️ [result] ${content.slice(0, 180)}`, type: "result" };
    }

    if (obj.type === "error" || obj.error) {
      const msg = typeof obj.error === "string" ? obj.error : obj.error?.message || JSON.stringify(obj.error);
      return { text: `❌ ${msg}`, type: "error" };
    }

    if (obj.message?.content || obj.content) {
      const content = obj.message?.content || obj.content;
      if (Array.isArray(content)) {
        const parts: string[] = [];
        let isTool = false;
        let isResult = false;
        for (const item of content) {
          if (item.type === "text" && item.text) {
            parts.push(item.text);
          } else if (item.type === "tool_use") {
            isTool = true;
            parts.push(`🔨 [${item.name || "tool"}] ${formatToolTarget(item.name, item.input)}`);
          } else if (item.type === "tool_result") {
            isResult = true;
            const res = typeof item.content === "string" ? item.content : JSON.stringify(item.content ?? "");
            parts.push(`⚙️ [result] ${res.slice(0, 180)}`);
          }
        }
        if (parts.length > 0) {
          return { text: parts.join("\n"), type: isTool ? "tool" : isResult ? "result" : "text" };
        }
      } else if (typeof content === "string" && content.trim()) {
        return { text: content, type: "text" };
      }
    }

    return null;
  } catch {
    return { text: line, type: "text" };
  }
}

/** Layer 3: is this right? Transcript, diff, cost — and why it exists at all. */
export function DetailDrawer({
  id,
  now,
  onClose,
  onChanged,
}: {
  id: number;
  now: number;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [d, setD] = useState<Detail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [editTitle, setEditTitle] = useState("");
  const [editIntent, setEditIntent] = useState("");
  const [editDod, setEditDod] = useState("");
  const [editEngine, setEditEngine] = useState("claude");
  const [constraintText, setConstraintText] = useState("");
  const [logs, setLogs] = useState<{ claude: string[]; openshell: string[] }>({
    claude: [],
    openshell: [],
  });
  const [logTab, setLogTab] = useState<"claude" | "openshell">("claude");
  const [userScrolledUp, setUserScrolledUp] = useState(false);
  const logContainerRef = useRef<HTMLDivElement | null>(null);

  const handleScroll = () => {
    const el = logContainerRef.current;
    if (!el) return;
    const isAtBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setUserScrolledUp(!isAtBottom);
  };

  useEffect(() => {
    if (!d || (!d.environment && !["running", "verifying", "claimed"].includes(d.state))) return;
    const fetchLogs = () => {
      const el = logContainerRef.current;
      const isAtBottom = el ? el.scrollHeight - el.scrollTop - el.clientHeight < 40 : true;

      api
        .logs(d.id)
        .then((res) => {
          setLogs(res);
          if (isAtBottom && logContainerRef.current) {
            requestAnimationFrame(() => {
              if (logContainerRef.current) {
                logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
              }
            });
          }
        })
        .catch(() => {});
    };
    fetchLogs();
    const interval = setInterval(fetchLogs, 2500);
    return () => clearInterval(interval);
  }, [d?.id, d?.environment, d?.state]);

  const load = () =>
    api
      .detail(id)
      .then((x) => {
        const item = x as Detail;
        setD(item);
        setEditTitle(item.title);
        setEditIntent(item.intent);
        setEditDod(item.definition_of_done ?? "");
        setEditEngine(item.engine ?? "claude");
      })
      .catch((e) => setErr(String(e)));

  useEffect(() => {
    setD(null);
    setErr(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const act = (p: Promise<unknown>) =>
    p.then(() => { load(); onChanged(); }).catch((e) => setErr(String(e)));

  if (err && !d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="err">{err}</div></aside>;
  if (!d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="dim">loading…</div></aside>;

  return (
    <aside className="drawer">
      <Head onClose={onClose} title={`#${d.id} ${d.title}`} />
      {err && <div className="err">{err}</div>}

      <div className="pill-row" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <span className="pill">{d.state}</span>
          {d.level && <span className="pill">{d.level}</span>}
          <span className="pill">{d.engine || "claude"}</span>
          <span className="pill">{money(d.cost_cents)}</span>
          {d.origin.kind !== "human" && <span className="pill machine">{d.origin.kind}-born</span>}
        </div>

        {d.state !== "done" && d.state !== "retired" && (
          <button
            style={{
              fontSize: "11px",
              padding: "3px 10px",
              background: "#15803d",
              color: "#ffffff",
              border: "1px solid #166534",
              borderRadius: "4px",
              cursor: "pointer",
              fontWeight: 600,
            }}
            onClick={() => act(api.transition(d.id, "done", "marked done by human"))}
          >
            ✔ Move to Done
          </button>
        )}
      </div>

      {/* The highest-leverage payload in the system: sixty words that tell an
          agent why it is writing this code. */}
      <Section title="Intent chain">
        <div className="chain">
          {d.ancestry.map((a, n) => (
            <div key={n} className="chain-line">
              <span className="chain-level">{a.level.toUpperCase()}</span>
              <span>{a.intent}</span>
            </div>
          ))}
          {d.definition_of_done && (
            <div className="chain-line dod">
              <span className="chain-level">DoD</span>
              <span>{d.definition_of_done}</span>
            </div>
          )}
        </div>
      </Section>

      {d.constraints.length > 0 && (
        <Section title="Inherited constraints">
          <ul className="plain">
            {d.constraints.map((c, n) => <li key={n}>📌 {c}</li>)}
          </ul>
        </Section>
      )}

      {((d.blockers && d.blockers.length > 0) || d.blocked_by.length > 0) && (
        <Section title="Blockers">
          <div className="blocker-chips">
            {d.blockers && d.blockers.length > 0 ? (
              d.blockers.map((b) => (
                <span key={b.id} className={`blocker-chip state-${b.state}`}>
                  <span className="blocker-id">#{b.id}</span>
                  <span className="blocker-title">{b.title}</span>
                  <span className="state-cue">{b.state.replace("_", " ")}</span>
                </span>
              ))
            ) : (
              d.blocked_by.map((bid) => (
                <span key={bid} className="blocker-chip">
                  <span className="blocker-id">#{bid}</span>
                </span>
              ))
            )}
          </div>
        </Section>
      )}

      {(d.pr_url || d.environment) && (
        <Section title="This run">
          {d.pr_url && (
            <p>
              <a className="pr-link" href={d.pr_url} target="_blank" rel="noreferrer">
                ↗ {d.pr_url}
              </a>
            </p>
          )}
          {d.environment && <p className="dim">sandbox {d.environment}</p>}
        </Section>
      )}

      {(d.environment || ["running", "verifying", "claimed"].includes(d.state)) && (() => {
        const parsedClaudeLogs = logs.claude
          .map((l) => parseClaudeLogLine(l))
          .filter((p): p is { text: string; type: "text" | "tool" | "result" | "system" | "error" } => p !== null);

        return (
          <Section title="Live Logs">
            <div style={{ position: "relative" }}>
              <div style={{ display: "flex", gap: 6, marginBottom: 8 }}>
                <button
                  className={logTab === "claude" ? "primary" : ""}
                  style={{ fontSize: "11px", padding: "3px 10px" }}
                  onClick={() => setLogTab("claude")}
                >
                  Claude Agent ({parsedClaudeLogs.length})
                </button>
                <button
                  className={logTab === "openshell" ? "primary" : ""}
                  style={{ fontSize: "11px", padding: "3px 10px" }}
                  onClick={() => setLogTab("openshell")}
                >
                  OpenShell Sandbox ({logs.openshell.length})
                </button>
              </div>

              <div
                ref={logContainerRef}
                onScroll={handleScroll}
                className="terminal-pane"
                style={{
                  background: "#080c14",
                  border: "1px solid #1e293b",
                  borderRadius: "6px",
                  padding: "10px",
                  fontFamily: "'JetBrains Mono', monospace, monospace",
                  fontSize: "11px",
                  lineHeight: "1.4",
                  color: logTab === "claude" ? "#a7f3d0" : "#38bdf8",
                  maxHeight: "220px",
                  overflowY: "auto",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-all",
                }}
              >
                {logTab === "claude" ? (
                  parsedClaudeLogs.length > 0 ? (
                    <>
                      {parsedClaudeLogs.map((parsed, i) => (
                        <div
                          key={i}
                          style={{
                            color:
                              parsed.type === "tool"
                                ? "#38bdf8"
                                : parsed.type === "result"
                                ? "#a7f3d0"
                                : parsed.type === "error"
                                ? "#f87171"
                                : "#f8fafc",
                            fontWeight: parsed.type === "tool" ? "600" : "normal",
                          }}
                        >
                          {parsed.text}
                        </div>
                      ))}
                      {(() => {
                        const last = parsedClaudeLogs[parsedClaudeLogs.length - 1];
                        let statusText = "Claude is thinking / evaluating model response...";
                        if (last.type === "tool") statusText = `Executing ${last.text.replace("🔨 ", "")}...`;
                        if (last.type === "result") statusText = "Tool output received, processing next action...";
                        return (
                          <div
                            style={{
                              marginTop: "8px",
                              paddingTop: "6px",
                              borderTop: "1px dashed #334155",
                              color: "#fbbf24",
                              fontStyle: "italic",
                              display: "flex",
                              alignItems: "center",
                              gap: "6px",
                            }}
                          >
                            <span
                              style={{
                                display: "inline-block",
                                width: "6px",
                                height: "6px",
                                borderRadius: "50%",
                                background: "#fbbf24",
                              }}
                            />
                            ⚡ {statusText}
                          </div>
                        );
                      })()}
                    </>
                  ) : (
                    <span className="dim">Waiting for Claude agent stdout stream…</span>
                  )
                ) : logs.openshell.length > 0 ? (
                  logs.openshell.map((l, i) => (
                    <div
                      key={i}
                      style={{
                        color: l.includes("ALLOWED")
                          ? "#4ade80"
                          : l.includes("HTTP:")
                          ? "#38bdf8"
                          : l.includes("ERR") || l.includes("WARN")
                          ? "#fbbf24"
                          : "#94a3b8",
                      }}
                    >
                      {l}
                    </div>
                  ))
                ) : (
                  <span className="dim">Connecting to OpenShell sandbox log stream…</span>
                )}
              </div>

              {userScrolledUp && (
                <button
                  style={{
                    position: "absolute",
                    bottom: "10px",
                    right: "12px",
                    fontSize: "10px",
                    padding: "3px 8px",
                    background: "#0284c7",
                    color: "#ffffff",
                    border: "none",
                    borderRadius: "4px",
                    cursor: "pointer",
                    boxShadow: "0 2px 6px rgba(0,0,0,0.5)",
                    zIndex: 10,
                  }}
                  onClick={() => {
                    if (logContainerRef.current) {
                      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
                      setUserScrolledUp(false);
                    }
                  }}
                >
                  ↓ Jump to bottom
                </button>
              )}
            </div>
          </Section>
        );
      })()}

      {d.escalation && (
        <Section title="Waiting on you">
          <p className="question">{d.escalation.question}</p>
          <p className="dim">blocked {since(d.escalation.blocked_since, now)}</p>
          {d.escalation.options.map((o, n) => (
            <div key={n} className="option">
              <button
                className={n === d.escalation!.recommended ? "primary" : ""}
                onClick={() => act(api.answer(d.id, o.label))}
              >
                {o.label}
                {n === d.escalation!.recommended && " ★"}
              </button>
              <span className="dim">{o.detail}</span>
            </div>
          ))}
        </Section>
      )}

      {d.state === "shaping" && (
        <Section title="Refine & Approve Plan">
          <p className="dim" style={{ marginBottom: 8 }}>
            Tweak card details before approving into the Ready queue.
          </p>

          <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 12 }}>
            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Title</label>
              <input
                type="text"
                className="search-input"
                style={{ width: "100%" }}
                value={editTitle}
                onChange={(e) => setEditTitle(e.target.value)}
              />
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Intent (Why this exists)</label>
              <textarea
                rows={2}
                value={editIntent}
                onChange={(e) => setEditIntent(e.target.value)}
              />
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Definition of Done</label>
              <textarea
                rows={2}
                placeholder="How to mechanically verify success..."
                value={editDod}
                onChange={(e) => setEditDod(e.target.value)}
              />
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Agent Engine</label>
              <select
                className="search-input"
                style={{ width: "100%", background: "#0f172a", color: "#f8fafc", padding: "6px" }}
                value={editEngine}
                onChange={(e) => setEditEngine(e.target.value)}
              >
                <option value="claude">Claude Code (Anthropic)</option>
                <option value="agy">Antigravity CLI (agy)</option>
              </select>
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Add Inherited Constraint</label>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  type="text"
                  className="search-input"
                  style={{ flex: 1 }}
                  placeholder="e.g. Must run --offline"
                  value={constraintText}
                  onChange={(e) => setConstraintText(e.target.value)}
                />
                <button
                  disabled={!constraintText.trim()}
                  onClick={() => {
                    act(api.pin(d.id, constraintText.trim()));
                    setConstraintText("");
                  }}
                >
                  Pin
                </button>
              </div>
            </div>
          </div>

          <div className="btns">
            <button
              onClick={() => {
                act(
                  api.update(d.id, {
                    title: editTitle,
                    intent: editIntent,
                    definition_of_done: editDod,
                    engine: editEngine,
                  })
                );
              }}
            >
              Save Changes
            </button>
            <button
              className="primary"
              onClick={() => {
                const saveP = api.update(d.id, {
                  title: editTitle,
                  intent: editIntent,
                  definition_of_done: editDod,
                  engine: editEngine,
                });
                const publishP = saveP.then(() => {
                  if (d.children.length > 0) {
                    return Promise.all(
                      d.children.map((cid) => api.transition(cid, "ready", "plan approved"))
                    ).then(() => api.transition(d.id, "ready", "plan approved"));
                  } else {
                    return api.transition(d.id, "ready", "plan approved");
                  }
                });
                act(publishP);
              }}
            >
              Approve & Publish to Ready
            </button>
          </div>
        </Section>
      )}

      {d.state === "review" && (
        <Section title="Review">
          <p className="dim">
            +{d.diff_added} −{d.diff_removed} · gates {d.gates.map((g) => g.name).join(", ")}
          </p>
          {d.pr_url && (
            <p>
              <a className="pr-link" href={d.pr_url} target="_blank" rel="noreferrer">
                ↗ review the pull request
              </a>
            </p>
          )}
          {/* The note lives with the button that sends it. Previously it was a
              shared textarea in a different section, so it was not obvious
              that Request changes needed one — or that it took the note at all. */}
          <textarea
            rows={2}
            value={text}
            placeholder="What needs to change? This reaches the next agent that picks it up."
            onChange={(e) => setText(e.target.value)}
          />
          <div className="btns">
            <button
              className="primary"
              style={{ background: "#15803d", borderColor: "#166534", fontWeight: 600 }}
              onClick={() => act(api.approve(d.id))}
            >
              ✅ Approve & Move to Done
            </button>
            <button
              disabled={!text.trim()}
              title={text.trim() ? "" : "Say what needs changing first"}
              onClick={() => { act(api.requestChanges(d.id, text)); setText(""); }}
            >
              Request changes
            </button>
          </div>
        </Section>
      )}

      {["running", "claimed", "verifying"].includes(d.state) && (
        <Section title="Interrupt Active Run">
          <p className="dim" style={{ marginBottom: 8 }}>
            Halt the container immediately and return this card to Ready.
          </p>
          <div className="btns">
            <button
              style={{ background: "#7f1d1d", color: "#fca5a5", borderColor: "#991b1b" }}
              onClick={() => act(api.halt(d.id, "halted by human"))}
            >
              🛑 Halt & Cancel Sandbox Run
            </button>
          </div>
        </Section>
      )}

      <Section title="Steer / Resume Instructions">
        <p className="dim" style={{ marginBottom: 6 }}>
          Tell the agent where to pick up or what to change. Notes are automatically included in the next agent's briefing.
        </p>
        <textarea
          rows={2}
          value={text}
          placeholder="e.g. Pick up where PR #1 left off and fix tests..."
          onChange={(e) => setText(e.target.value)}
        />
        <div className="btns" style={{ marginTop: 6 }}>
          <button
            disabled={!text.trim()}
            className="primary"
            onClick={() => {
              const steerP = api.steer(d.id, text.trim());
              const p =
                d.state !== "ready"
                  ? steerP.then(() => api.transition(d.id, "ready", "steered by human"))
                  : steerP;
              act(p);
              setText("");
            }}
          >
            Send Instruction & Queue for Agent
          </button>
        </div>
      </Section>

      {d.notes.length > 0 && (
        <Section title="Notes">
          <ul className="plain">
            {d.notes.map((n, i) => (
              <li key={i}><span className="dim">{n.author}</span> {n.text}</li>
            ))}
          </ul>
        </Section>
      )}

      <Section title="History">
        <ul className="plain hist">
          {d.history.slice(-12).reverse().map((h, i) => (
            <li key={i}>
              <span className="dim">{since(h.at, now)} ago</span> {h.from} → <b>{h.to}</b>
              <span className="dim"> by {h.by}</span>
              {h.reason && <div className="dim reason">{h.reason}</div>}
            </li>
          ))}
        </ul>
      </Section>
    </aside>
  );
}

const Head = ({ title, onClose }: { title: string; onClose: () => void }) => (
  <div className="drawer-head">
    <h3>{title}</h3>
    <button className="close" onClick={onClose}>✕</button>
  </div>
);

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <div className="section">
    <div className="section-title">{title}</div>
    {children}
  </div>
);
