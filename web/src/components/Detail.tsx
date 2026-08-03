import { useEffect, useRef, useState } from "react";
import { api, money, since } from "../api.js";
import type { BoardEvent, PlanTaskSpec, SandboxProfile, WorkItem } from "../types.js";
import { subscribeBoardEvents } from "../useBoard.js";
import { ProjectSandboxPicker } from "./Settings.js";

interface Detail extends WorkItem {
  ancestry: { level: string; title: string; intent: string }[];
  children: number[];
  default_engine?: string;
  default_model?: string;
}

type EditPlanTask = {
  key: string;
  title: string;
  intent: string;
  definition_of_done: string;
  blocked_by_keys: string[];
};

export function planTasksFromArtifact(tasks: PlanTaskSpec[] | undefined): EditPlanTask[] {
  if (!tasks?.length) return [];
  return tasks.map((t) => ({
    key: t.key,
    title: t.title,
    intent: t.intent,
    definition_of_done: t.definition_of_done,
    blocked_by_keys: Array.isArray(t.blocked_by_keys) ? [...t.blocked_by_keys] : [],
  }));
}

export function emptyPlanTask(n: number): EditPlanTask {
  return {
    key: `t${n}`,
    title: "",
    intent: "",
    definition_of_done: "",
    blocked_by_keys: [],
  };
}

export function PlanEditor({
  planTasks,
  setPlanTasks,
}: {
  planTasks: EditPlanTask[];
  setPlanTasks: (tasks: EditPlanTask[]) => void;
}) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10, marginBottom: 12 }}>
      {planTasks.map((t, idx) => (
        <div
          key={idx}
          style={{
            border: "1px solid var(--line-strong)",
            borderRadius: 6,
            padding: 8,
            display: "flex",
            flexDirection: "column",
            gap: 6,
          }}
        >
          <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <input
              className="search-input"
              style={{ width: 72 }}
              placeholder="key"
              value={t.key}
              onChange={(e) => {
                const oldKey = t.key;
                const newKey = e.target.value;
                const next = planTasks.map((pt, i) => {
                  if (i === idx) {
                    return { ...pt, key: newKey };
                  }
                  if (oldKey && oldKey !== newKey && pt.blocked_by_keys.includes(oldKey)) {
                    return {
                      ...pt,
                      blocked_by_keys: pt.blocked_by_keys.map((k) => (k === oldKey ? newKey : k)),
                    };
                  }
                  return pt;
                });
                setPlanTasks(next);
              }}
            />
            <input
              className="search-input"
              style={{ flex: 1 }}
              placeholder="Title"
              value={t.title}
              onChange={(e) => {
                const next = [...planTasks];
                next[idx] = { ...t, title: e.target.value };
                setPlanTasks(next);
              }}
            />
            <button
              type="button"
              className="dim"
              style={{ fontSize: 11, padding: "2px 8px" }}
              onClick={() => {
                const removedKey = planTasks[idx].key;
                setPlanTasks(
                  planTasks
                    .filter((_, i) => i !== idx)
                    .map((pt) => ({
                      ...pt,
                      blocked_by_keys: pt.blocked_by_keys.filter((k) => k !== removedKey),
                    }))
                );
              }}
            >
              Remove
            </button>
          </div>
          <textarea
            rows={2}
            placeholder="Why this task exists"
            value={t.intent}
            onChange={(e) => {
              const next = [...planTasks];
              next[idx] = { ...t, intent: e.target.value };
              setPlanTasks(next);
            }}
          />
          <textarea
            rows={2}
            placeholder="Definition of done (mechanically checkable)"
            value={t.definition_of_done}
            onChange={(e) => {
              const next = [...planTasks];
              next[idx] = { ...t, definition_of_done: e.target.value };
              setPlanTasks(next);
            }}
          />
          <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 2 }}>
            <div style={{ fontSize: 11, fontWeight: 600, color: "var(--dim)" }}>
              Blocked by tasks:
            </div>
            <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 6 }}>
              {t.blocked_by_keys.length === 0 ? (
                <span className="dim" style={{ fontSize: 11 }}>None</span>
              ) : (
                t.blocked_by_keys.map((bKey) => {
                  const target = planTasks.find((other) => other.key === bKey);
                  const title = target?.title?.trim() ? target.title.trim() : undefined;
                  return (
                    <span
                      key={bKey}
                      className="blocker-chip"
                      style={{ fontSize: 11, cursor: "default" }}
                    >
                      <span className="blocker-id">{bKey}</span>
                      {title && <span className="blocker-title">{title}</span>}
                      <button
                        type="button"
                        style={{
                          background: "none",
                          border: "none",
                          color: "var(--dim)",
                          cursor: "pointer",
                          padding: "0 2px",
                          marginLeft: 2,
                          fontSize: 12,
                          lineHeight: 1,
                        }}
                        title={`Remove ${bKey} blocker`}
                        onClick={() => {
                          const next = [...planTasks];
                          next[idx] = {
                            ...t,
                            blocked_by_keys: t.blocked_by_keys.filter((k) => k !== bKey),
                          };
                          setPlanTasks(next);
                        }}
                      >
                        ✕
                      </button>
                    </span>
                  );
                })
              )}

              {(() => {
                const siblingOptions = planTasks.filter(
                  (other, i) => i !== idx && other.key.trim() !== ""
                );
                return (
                  <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
                    <select
                      className="search-input"
                      style={{ fontSize: 11, padding: "2px 6px", height: 26 }}
                      value=""
                      onChange={(e) => {
                        const val = e.target.value;
                        if (!val) return;
                        if (!t.blocked_by_keys.includes(val)) {
                          const next = [...planTasks];
                          next[idx] = {
                            ...t,
                            blocked_by_keys: [...t.blocked_by_keys, val],
                          };
                          setPlanTasks(next);
                        }
                      }}
                    >
                      <option value="">+ Select blocker task...</option>
                      {siblingOptions.length === 0 ? (
                        <option value="" disabled>No sibling tasks available</option>
                      ) : (
                        siblingOptions.map((sibling) => {
                          const isAdded = t.blocked_by_keys.includes(sibling.key);
                          const label = sibling.title?.trim()
                            ? `${sibling.key} — ${sibling.title.trim()}`
                            : sibling.key;
                          return (
                            <option
                              key={sibling.key}
                              value={sibling.key}
                              disabled={isAdded}
                            >
                              {label} {isAdded ? "(already added)" : ""}
                            </option>
                          );
                        })
                      )}
                    </select>
                  </div>
                );
              })()}
            </div>
          </div>
        </div>
      ))}
      {planTasks.length === 0 && (
        <p className="dim">No tasks in the Plan yet. Add one, or wait for Initial plan.</p>
      )}
    </div>
  );
}

type LogLineType = "text" | "tool" | "result" | "system" | "error" | "thinking";
type ParsedLogLine = { text: string; type: LogLineType };

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
  if (input.targetFile || input.target_file) return input.targetFile || input.target_file;
  if (input.path) return input.path;
  if (input.file) return input.file;
  if (input.pattern) return `"${input.pattern}" in ${input.path || input.directory || "."}`;

  return JSON.stringify(input);
}

/** Cursor Agent CLI: `{ tool_call: { shellToolCall: { args, result } } }`. */
function cursorToolCallBody(
  obj: any
): { name: string; args: any; result: any; description?: string } | null {
  const tc = obj?.tool_call;
  if (!tc || typeof tc !== "object") return null;
  const key = Object.keys(tc).find((k) => k.endsWith("ToolCall"));
  if (!key) return null;
  const body = tc[key] ?? {};
  const name = key.replace(/ToolCall$/, "") || "tool";
  return {
    name,
    args: body.args ?? {},
    result: body.result,
    description: typeof body.description === "string" ? body.description : undefined,
  };
}

function cursorToolResultText(_name: string, result: any): string {
  if (!result) return "";
  const success = result.success ?? result;
  if (typeof success === "string") return success;
  if (success?.stdout != null) {
    const err = success.stderr ? `\nstderr: ${success.stderr}` : "";
    return `${success.stdout}${err}`;
  }
  if (success?.interleavedOutput != null) return String(success.interleavedOutput);
  if (success?.content != null) return String(success.content);
  if (result.failure || result.error) {
    return JSON.stringify(result.failure ?? result.error);
  }
  return JSON.stringify(success);
}

function parseClaudeLogLine(line: string): ParsedLogLine | null {
  const trimmed = line.trim();
  if (!trimmed) return null;
  if (!trimmed.startsWith("{")) {
    return { text: line, type: "text" };
  }

  try {
    const obj = JSON.parse(trimmed);

    // Cursor Agent CLI stream-json (not Claude Code's content_block_* shape).
    if (obj.type === "thinking") {
      if (obj.subtype === "delta" && typeof obj.text === "string" && obj.text) {
        return { text: obj.text, type: "thinking" };
      }
      return null;
    }
    if (obj.type === "tool_call") {
      const tool = cursorToolCallBody(obj);
      if (!tool) return null;
      const label = tool.description || formatToolTarget(tool.name, tool.args);
      if (obj.subtype === "started") {
        return { text: `🔨 [${tool.name}] ${label}`, type: "tool" };
      }
      if (obj.subtype === "completed") {
        const out = cursorToolResultText(tool.name, tool.result).slice(0, 240);
        return {
          text: out ? `⚙️ [${tool.name}] ${out}` : `⚙️ [${tool.name}] done`,
          type: "result",
        };
      }
      return null;
    }
    if (obj.event === "step_update" && obj.step_update) {
      const su = obj.step_update;
      if (su.step_type === "tool" && su.tool_info) {
        const name = su.tool_name || su.tool_info.name || "tool";
        const params = su.tool_info.parameters;
        if (su.state === "ACTIVE") {
          return { text: `🔨 [${name}] ${formatToolTarget(name, params)}`, type: "tool" };
        }
        if (su.state === "DONE" && su.tool_info.output) {
          const out = typeof su.tool_info.output === "string" ? su.tool_info.output : JSON.stringify(su.tool_info.output ?? "");
          return { text: `⚙️ [${name}] ${out.slice(0, 180)}`, type: "result" };
        }
        return null;
      }
      return null;
    }

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

    if (obj.type === "tool_use" || (obj.name && obj.type !== "assistant")) {
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

/** Merge Cursor thinking deltas so the drawer doesn't render one word per line. */
function coalesceAgentLogLines(lines: string[]): ParsedLogLine[] {
  const out: ParsedLogLine[] = [];
  let thinking = "";
  const flushThinking = () => {
    const t = thinking.trim();
    if (t) out.push({ text: t, type: "thinking" });
    thinking = "";
  };
  for (const line of lines) {
    const parsed = parseClaudeLogLine(line);
    if (!parsed) continue;
    if (parsed.type === "thinking") {
      thinking += parsed.text;
      continue;
    }
    flushThinking();
    out.push(parsed);
  }
  flushThinking();
  return out;
}

/**
 * Pure reducer function to update card Detail state live when board events arrive.
 */
export function reduceDetail<T extends Detail = Detail>(
  prev: T | null,
  ev: BoardEvent,
  id: number
): T | null {
  if (ev.type === "upsert" && ev.item && ev.item.id === id) {
    if (!prev) {
      return {
        ancestry: [],
        children: [],
        ...ev.item,
      } as unknown as T;
    }
    return {
      ...prev,
      ...ev.item,
    };
  }
  if (ev.type === "delete" && ev.id === id) {
    return null;
  }
  return prev;
}

/** Layer 3: is this right? Transcript, diff, cost — and why it exists at all. */
export function DetailDrawer({
  id,
  now,
  defaultEngine,
  defaultModel,
  onClose,
  onChanged,
}: {
  id: number;
  now: number;
  defaultEngine?: string;
  defaultModel?: string;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [d, setD] = useState<Detail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [editTitle, setEditTitle] = useState("");
  const [editIntent, setEditIntent] = useState("");
  const [editDod, setEditDod] = useState("");
  const [editEngine, setEditEngine] = useState("");
  const [editPrompt, setEditPrompt] = useState("");
  const [sandboxProfiles, setSandboxProfiles] = useState<SandboxProfile[]>([]);
  const [defaultSandboxProfileId, setDefaultSandboxProfileId] = useState<string | null>(null);
  const [sandboxPickerBusy, setSandboxPickerBusy] = useState(false);
  const [sandboxPickerErr, setSandboxPickerErr] = useState<string | null>(null);
  const [planTasks, setPlanTasks] = useState<EditPlanTask[]>([]);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [confirmHalt, setConfirmHalt] = useState(false);
  const [logs, setLogs] = useState<{ claude: string[]; openshell: string[] }>({
    claude: [],
    openshell: [],
  });
  const [logTab, setLogTab] = useState<"claude" | "openshell">("claude");
  const [userScrolledUp, setUserScrolledUp] = useState(false);
  const logContainerRef = useRef<HTMLDivElement | null>(null);

  const handleScroll = () => {
    if (!logContainerRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = logContainerRef.current;
    const isAtBottom = scrollHeight - scrollTop - clientHeight < 20;
    setUserScrolledUp(!isAtBottom);
  };

  const loadLogs = () => {
    if (!id) return;
    api
      .logs(id)
      .then((res) => {
        setLogs(res);
      })
      .catch(() => {});
  };

  useEffect(() => {
    loadLogs();
    const interval = setInterval(loadLogs, 2000);
    return () => clearInterval(interval);
  }, [id]);

  useEffect(() => {
    if (!userScrolledUp && logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs, userScrolledUp, logTab]);

  const load = () =>
    api
      .detail(id)
      .then((x) => {
        const item = x as Detail;
        setD(item);
        setEditTitle(item.title);
        setEditIntent(item.intent);
        setEditDod(item.definition_of_done ?? "");
        setEditEngine(item.engine ?? item.default_engine ?? defaultEngine ?? "");
        setEditPrompt(item.project_prompt ?? "");
        setPlanTasks(
          planTasksFromArtifact(item.proposal?.tasks ?? item.plan?.tasks),
        );
      })
      .catch((e) => setErr(String(e)));

  useEffect(() => {
    setD(null);
    setErr(null);
    setConfirmDelete(false);
    setConfirmArchive(false);
    setConfirmHalt(false);
    setPlanTasks([]);
    setSandboxPickerErr(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  useEffect(() => {
    if (!d || d.level !== "Project") return;
    api
      .listSandboxProfiles()
      .then((out) => {
        setSandboxProfiles(out.profiles);
        setDefaultSandboxProfileId(out.default_sandbox_profile_id);
        setSandboxPickerErr(null);
      })
      .catch((e) => setSandboxPickerErr(String(e)));
  }, [d?.id, d?.level]);

  useEffect(() => {
    if (!id) return;
    const unsubscribe = subscribeBoardEvents((ev) => {
      setD((prev) => reduceDetail(prev, ev, id));
    });
    return () => {
      unsubscribe();
    };
  }, [id]);

  const act = (p: Promise<unknown>) =>
    p.then(() => { load(); onChanged(); }).catch((e) => setErr(String(e)));

  if (err && !d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="err">{err}</div></aside>;
  if (!d) return <aside className="drawer"><Head onClose={onClose} title={`#${id}`} /><div className="dim">loading…</div></aside>;

  const resolvedEngine = d.engine ?? d.default_engine ?? defaultEngine ?? "";
  const resolvedModel = d.model ?? d.default_model ?? defaultModel ?? "";

  return (
    <aside className="drawer">
      <Head
        onClose={onClose}
        onArchive={
          d.state !== "retired"
            ? () => {
                setConfirmArchive(!confirmArchive);
                setConfirmDelete(false);
              }
            : undefined
        }
        onDelete={() => {
          setConfirmDelete(!confirmDelete);
          setConfirmArchive(false);
        }}
        title={`#${d.id} ${d.title}`}
      />
      {err && <div className="err">{err}</div>}

      {confirmArchive && (
        <div
          style={{
            background: "var(--accent-fill)",
            border: "1px solid var(--accent)",
            borderRadius: "6px",
            padding: "10px 12px",
            marginBottom: "12px",
            display: "flex",
            flexDirection: "column",
            gap: "8px",
          }}
        >
          <div style={{ color: "var(--accent-fill)", fontSize: "12px", fontWeight: 600 }}>
            📦 Archive #{d.id} "{d.title}"?
          </div>
          <div style={{ color: "var(--accent)", fontSize: "11px" }}>
            This item and its subtree will be retired, not deleted. Archived Projects leave the cockpit; history stays in state.
          </div>
          <div style={{ display: "flex", gap: "8px", marginTop: "4px" }}>
            <button
              style={{
                fontSize: "11px",
                padding: "4px 12px",
                background: "var(--accent)",
                color: "#fff",
                border: "none",
                borderRadius: "4px",
                cursor: "pointer",
                fontWeight: 600,
              }}
              onClick={() => {
                api
                  .cut(d.id, "archived from drawer")
                  .then(() => {
                    onChanged();
                    onClose();
                  })
                  .catch((e) => setErr(String(e)));
              }}
            >
              Confirm Archive
            </button>
            <button
              style={{
                fontSize: "11px",
                padding: "4px 12px",
                background: "var(--panel-2)",
                color: "var(--dim)",
                border: "none",
                borderRadius: "4px",
                cursor: "pointer",
              }}
              onClick={() => setConfirmArchive(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {confirmDelete && (
        <div
          style={{
            background: "var(--needs-you-bg)",
            border: "1px solid var(--needs-human)",
            borderRadius: "6px",
            padding: "10px 12px",
            marginBottom: "12px",
            display: "flex",
            flexDirection: "column",
            gap: "8px",
          }}
        >
          <div style={{ color: "var(--needs-human)", fontSize: "12px", fontWeight: 600 }}>
            ⚠️ Permanently delete #{d.id} "{d.title}"?
          </div>
          <div style={{ color: "var(--needs-human)", fontSize: "11px" }}>
            This will remove the item and any child tasks permanently. This action cannot be undone.
          </div>
          <div style={{ display: "flex", gap: "8px", marginTop: "4px" }}>
            <button
              style={{
                fontSize: "11px",
                padding: "4px 12px",
                background: "var(--needs-human)",
                color: "#fff",
                border: "none",
                borderRadius: "4px",
                cursor: "pointer",
                fontWeight: 600,
              }}
              onClick={() => {
                api
                  .deleteItem(d.id)
                  .then(() => {
                    onChanged();
                    onClose();
                  })
                  .catch((e) => setErr(String(e)));
              }}
            >
              Confirm Delete
            </button>
            <button
              style={{
                fontSize: "11px",
                padding: "4px 12px",
                background: "var(--panel-2)",
                color: "var(--dim)",
                border: "none",
                borderRadius: "4px",
                cursor: "pointer",
              }}
              onClick={() => setConfirmDelete(false)}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      <div className="pill-row" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
          <span className="pill">{d.state}</span>
          {d.level && <span className="pill">{d.level}</span>}
          <span className="pill">{resolvedEngine}</span>
          {resolvedModel && <span className="pill">{resolvedModel}</span>}
          <span className="pill">{money(d.cost_cents)}</span>
          {d.beads_id && (
            <span
              className="pill beads"
              style={{ color: "var(--accent)", background: "var(--accent-fill)", border: "1px solid var(--accent)" }}
              title={`Beads Task ID: ${d.beads_id} (Dolt version-controlled issue store on refs/dolt/data)`}
            >
              🔗 {d.beads_id}
            </span>
          )}
          {d.github_issue_url && (
            <a
              className="pill beads"
              href={d.github_issue_url}
              target="_blank"
              rel="noreferrer"
              style={{ textDecoration: "none", color: "var(--accent)", background: "var(--accent-fill)", border: "1px solid var(--accent)" }}
              title={`View on GitHub Issues: ${d.github_issue_url}`}
            >
              🐙 GitHub Issue
            </a>
          )}
          {d.origin.kind !== "human" && <span className="pill machine">{d.origin.kind}-born</span>}
        </div>

        {d.state !== "done" && d.state !== "retired" && (
          <button
            style={{
              fontSize: "11px",
              padding: "3px 10px",
              background: "var(--ok)",
              color: "#fff",
              border: "1px solid var(--ok)",
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

      {/* Task ancestry chain — Projects edit Why below instead. */}
      {d.level !== "Project" && (
        <Section title="Why this exists">
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
          {d.environment && <p className="dim">sandbox {d.environment} ({resolvedEngine})</p>}
        </Section>
      )}

      {(d.environment || ["running", "claimed"].includes(d.state)) && (() => {
        const parsedClaudeLogs = coalesceAgentLogLines(logs.claude);

        const agentTabLabel =
          resolvedEngine === "agy"
            ? "Antigravity Agent"
            : resolvedEngine === "claude"
              ? "Claude Agent"
              : resolvedEngine === "cursor"
                ? "Cursor Agent"
                : `${resolvedEngine} Agent`;

        const engineDisplayName =
          resolvedEngine === "agy"
            ? "Antigravity (agy)"
            : resolvedEngine === "claude"
              ? "Claude"
              : resolvedEngine === "cursor"
                ? "Cursor"
                : resolvedEngine;

        return (
          <Section title="Live Logs">
            <div style={{ position: "relative" }}>
              <div style={{ display: "flex", gap: 6, marginBottom: 8 }}>
                <button
                  className={logTab === "claude" ? "primary" : ""}
                  style={{ fontSize: "11px", padding: "3px 10px" }}
                  onClick={() => setLogTab("claude")}
                >
                  {agentTabLabel} ({parsedClaudeLogs.length})
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
                  background: "var(--panel-inset)",
                  border: "1px solid var(--line-strong)",
                  borderRadius: "6px",
                  padding: "10px",
                  fontFamily: "'JetBrains Mono', monospace, monospace",
                  fontSize: "11px",
                  lineHeight: "1.4",
                  color: logTab === "claude" ? "var(--ok)" : "var(--accent)",
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
                                ? "var(--accent)"
                                : parsed.type === "result"
                                ? "var(--ok)"
                                : parsed.type === "error"
                                ? "var(--needs-human)"
                                : parsed.type === "thinking"
                                ? "var(--dim)"
                                : "var(--ink)",
                            fontWeight: parsed.type === "tool" ? "600" : "normal",
                            fontStyle: parsed.type === "thinking" ? "italic" : "normal",
                          }}
                        >
                          {parsed.type === "thinking" ? `💭 ${parsed.text}` : parsed.text}
                        </div>
                      ))}
                      {(() => {
                        const last = parsedClaudeLogs[parsedClaudeLogs.length - 1];
                        let statusText = `${engineDisplayName} is thinking / evaluating response...`;
                        if (last.type === "thinking") statusText = `${engineDisplayName} is thinking…`;
                        if (last.type === "tool") statusText = `Executing ${last.text.replace("🔨 ", "")}...`;
                        if (last.type === "result") statusText = "Tool output received, processing next action...";
                        return (
                          <div
                            style={{
                              marginTop: "8px",
                              paddingTop: "6px",
                              borderTop: "1px dashed var(--line)",
                              color: "var(--review)",
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
                                background: "var(--review)",
                              }}
                            />
                            ⚡ {statusText}
                          </div>
                        );
                      })()}
                    </>
                  ) : (
                    <span className="dim">
                      Waiting for {engineDisplayName} agent stdout stream…
                    </span>
                  )
                ) : logs.openshell.length > 0 ? (
                  logs.openshell.map((l, i) => (
                    <div
                      key={i}
                      style={{
                        color: l.includes("ALLOWED")
                          ? "var(--ok)"
                          : l.includes("HTTP:")
                          ? "var(--accent)"
                          : l.includes("ERR") || l.includes("WARN")
                          ? "var(--review)"
                          : "var(--dim)",
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
                    background: "var(--accent)",
                    color: "#fff",
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

      {d.level === "Project" && (
        <>
          <Section title="Project">
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <div>
                <label className="section-title" style={{ display: "block", marginBottom: 2 }}>
                  Title
                </label>
                <input
                  type="text"
                  className="search-input"
                  style={{ width: "100%" }}
                  value={editTitle}
                  onChange={(e) => setEditTitle(e.target.value)}
                />
              </div>
              <div>
                <label className="section-title" style={{ display: "block", marginBottom: 2 }}>
                  Why
                </label>
                <p className="dim" style={{ marginBottom: 4, fontSize: 12 }}>
                  One sentence — the outcome contract. Not the Task breakdown.
                </p>
                <textarea
                  rows={2}
                  value={editIntent}
                  onChange={(e) => setEditIntent(e.target.value)}
                />
              </div>
              <div>
                <label className="section-title" style={{ display: "block", marginBottom: 2 }}>
                  Agent Engine
                </label>
                <select
                  className="search-input"
                  style={{ width: "100%", background: "var(--panel)", color: "var(--ink)", padding: "6px" }}
                  value={editEngine}
                  onChange={(e) => setEditEngine(e.target.value)}
                >
                  <option value="claude">Claude Code (Anthropic)</option>
                  <option value="agy">Antigravity CLI (agy)</option>
                  <option value="cursor">Cursor Agent (cursor)</option>
                </select>
              </div>
              <ProjectSandboxPicker
                projectId={d.id}
                value={d.sandbox_profile_id}
                profiles={sandboxProfiles}
                defaultId={defaultSandboxProfileId}
                busy={sandboxPickerBusy}
                error={sandboxPickerErr}
                onChange={(next) => {
                  setSandboxPickerBusy(true);
                  setSandboxPickerErr(null);
                  api
                    .setProjectSandboxProfile(d.id, next)
                    .then(() => {
                      load();
                      onChanged();
                    })
                    .catch((e) => setSandboxPickerErr(String(e)))
                    .finally(() => setSandboxPickerBusy(false));
                }}
              />
              <div className="btns">
                <button
                  onClick={() =>
                    act(
                      api.update(d.id, {
                        title: editTitle,
                        intent: editIntent,
                        engine: editEngine,
                        project_prompt: editPrompt,
                      }),
                    )
                  }
                >
                  Save Project
                </button>
              </div>
            </div>
          </Section>

          <Section title="Project prompt">
            <p className="dim" style={{ marginBottom: 8 }}>
              Standing instructions every Task agent sees (with the Plan from
              Initial plan). Replaces pins.
            </p>
            <textarea
              className="search-input"
              style={{ width: "100%", minHeight: 120, marginBottom: 8 }}
              value={editPrompt}
              onChange={(e) => setEditPrompt(e.target.value)}
              placeholder="Standing agent instructions for this Project…"
            />
          </Section>
        </>
      )}

      {/* Plan lives on Initial plan — editable until Approve freezes it. */}
      {(d.title === "Initial plan" || d.title.startsWith("Initial Plan for ")) && (
        <Section title="Proposed Tasks">
          <p className="dim" style={{ marginBottom: 8 }}>
            {d.state === "done"
              ? "Accepted — frozen on this card. Task agents see this breakdown in their briefing."
              : "Task breakdown (keys, deps, DoDs). Edit until you Approve; then sibling Tasks are created and this list freezes."}
          </p>
          {d.state === "done" ? (
            d.proposal && d.proposal.tasks.length > 0 ? (
              <ol style={{ margin: 0, paddingLeft: 18 }}>
                {d.proposal.tasks.map((t) => (
                  <li key={t.key} style={{ marginBottom: 8 }}>
                    <strong>{t.key}</strong> {t.title}
                    {t.blocked_by_keys?.length > 0 && (
                      <span className="dim"> (after {t.blocked_by_keys.join(", ")})</span>
                    )}
                    <div className="dim" style={{ fontSize: "0.9em" }}>
                      {t.intent}
                    </div>
                    <div className="dim" style={{ fontSize: "0.85em" }}>
                      DoD: {t.definition_of_done}
                    </div>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="dim">No frozen proposal on this card.</p>
            )
          ) : (
            <>
              <PlanEditor planTasks={planTasks} setPlanTasks={setPlanTasks} />
              <div className="btns" style={{ marginBottom: 8 }}>
                <button
                  type="button"
                  onClick={() => setPlanTasks([...planTasks, emptyPlanTask(planTasks.length + 1)])}
                >
                  Add Task
                </button>
                <button
                  type="button"
                  disabled={
                    planTasks.length === 0 ||
                    planTasks.some(
                      (t) =>
                        !t.key.trim() ||
                        !t.title.trim() ||
                        !t.definition_of_done.trim(),
                    )
                  }
                  onClick={() => {
                    const body = {
                      summary: editIntent.trim() || undefined,
                      tasks: planTasks.map((t) => ({
                        key: t.key.trim(),
                        title: t.title.trim(),
                        intent: t.intent.trim() || t.title.trim(),
                        definition_of_done: t.definition_of_done.trim(),
                        blocked_by_keys: t.blocked_by_keys
                          .map((s) => s.trim())
                          .filter(Boolean),
                      })),
                    };
                    act(api.savePlan(d.id, body));
                  }}
                >
                  Save Plan
                </button>
                <button
                  className="primary"
                  disabled={
                    planTasks.length === 0 ||
                    planTasks.some(
                      (t) =>
                        !t.key.trim() ||
                        !t.title.trim() ||
                        !t.definition_of_done.trim(),
                    )
                  }
                  title="Create Backlog Tasks from this proposal and finish Initial plan"
                  onClick={() => {
                    const body = {
                      summary: editIntent.trim() || undefined,
                      tasks: planTasks.map((t) => ({
                        key: t.key.trim(),
                        title: t.title.trim(),
                        intent: t.intent.trim() || t.title.trim(),
                        definition_of_done: t.definition_of_done.trim(),
                        blocked_by_keys: t.blocked_by_keys
                          .map((s) => s.trim())
                          .filter(Boolean),
                      })),
                    };
                    act(
                      api
                        .savePlan(d.id, body)
                        .then(() => api.approvePlan(d.id)),
                    );
                  }}
                >
                  Approve — create Tasks
                </button>
              </div>
            </>
          )}
        </Section>
      )}

      {/* Title / Why / DoD are the contract the next agent is graded on. Editable
          in Shaping, Backlog, and Review — not only before first Backlog — because
          Request changes notes lose to a stale DoD if you cannot rewrite it. */}
      {d.level !== "Project" &&
        (d.state === "shaping" || d.state === "backlog" || d.state === "ready" || d.state === "review") && (
        <Section title={d.state === "review" ? "Card contract" : "Refine"}>
          <p className="dim" style={{ marginBottom: 8 }}>
            {d.state === "shaping"
              ? "Tweak this Task before moving it into the Backlog."
              : d.state === "backlog" || d.state === "ready"
                ? "Still editable until you Start a run. DoD is what the next run must satisfy."
                : "If the PR missed the point, fix DoD / Why here — Request changes saves these with your note."}
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
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Why</label>
              <textarea
                rows={2}
                value={editIntent}
                onChange={(e) => setEditIntent(e.target.value)}
              />
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Definition of Done</label>
              <textarea
                rows={3}
                placeholder="How to mechanically verify success..."
                value={editDod}
                onChange={(e) => setEditDod(e.target.value)}
              />
            </div>

            <div>
              <label className="section-title" style={{ display: "block", marginBottom: 2 }}>Agent Engine</label>
              <select
                className="search-input"
                style={{ width: "100%", background: "var(--panel)", color: "var(--ink)", padding: "6px" }}
                value={editEngine}
                onChange={(e) => setEditEngine(e.target.value)}
              >
                <option value="claude">Claude Code (Anthropic)</option>
                <option value="agy">Antigravity CLI (agy)</option>
                <option value="cursor">Cursor Agent (cursor)</option>
              </select>
            </div>
          </div>

          {d.state !== "review" && (
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
              {d.state === "shaping" && (
                <button
                  className="primary"
                  onClick={() => {
                    const saveP = api.update(d.id, {
                      title: editTitle,
                      intent: editIntent,
                      definition_of_done: editDod,
                      engine: editEngine,
                    });
                    act(saveP.then(() => api.transition(d.id, "backlog", "human approved")));
                  }}
                >
                  Move to Backlog
                </button>
              )}
            </div>
          )}
        </Section>
      )}

      {d.state === "review" && (
        <Section title="Review">
          <p className="dim">
            +{d.diff_added} −{d.diff_removed}
            {d.gates.length > 0 && (
              <> · notes {d.gates.map((g) => g.name).join(", ")}</>
            )}
          </p>
          {d.proposal && d.proposal.tasks.length > 0 && (
            <div style={{ marginBottom: 12 }}>
              <p className="dim" style={{ marginBottom: 6 }}>
                {d.proposal.summary
                  ? d.proposal.summary
                  : "Proposed Tasks — Approve creates these under the Project."}
              </p>
              <ol style={{ margin: 0, paddingLeft: 18 }}>
                {d.proposal.tasks.map((t) => (
                  <li key={t.key} style={{ marginBottom: 8 }}>
                    <strong>{t.key}</strong> {t.title}
                    {t.blocked_by_keys?.length > 0 && (
                      <span className="dim"> (after {t.blocked_by_keys.join(", ")})</span>
                    )}
                    <div className="dim" style={{ fontSize: "0.9em" }}>
                      {t.intent}
                    </div>
                    <div className="dim" style={{ fontSize: "0.85em" }}>
                      DoD: {t.definition_of_done}
                    </div>
                  </li>
                ))}
              </ol>
            </div>
          )}
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
            placeholder="What needs to change? Prefer fixing DoD above when acceptance criteria are wrong — your note still binds over a stale DoD."
            onChange={(e) => setText(e.target.value)}
          />
          <div className="btns">
            <button
              className="primary"
              style={{ background: "var(--ok)", borderColor: "var(--ok)", fontWeight: 600 }}
              onClick={() => act(api.approve(d.id))}
            >
              {d.proposal && d.proposal.tasks.length > 0
                ? "✅ Approve — create Tasks"
                : "✅ Approve & Move to Done"}
            </button>
            <button
              disabled={!text.trim()}
              title={text.trim() ? "" : "Say what needs changing first"}
              onClick={() => {
                const note = text;
                setText("");
                act(
                  api
                    .update(d.id, {
                      title: editTitle,
                      intent: editIntent,
                      definition_of_done: editDod,
                      engine: editEngine,
                    })
                    .then(() => api.requestChanges(d.id, note)),
                );
              }}
            >
              Request changes
            </button>
          </div>
        </Section>
      )}

      {["running", "claimed"].includes(d.state) && (
        <Section title="Interrupt Active Run">
          <p className="dim" style={{ marginBottom: 8 }}>
            Park stops the agent, keeps the sandbox and conversation, and holds the
            card until you Resume. Halt discards the LLM session.
          </p>
          {!confirmHalt ? (
            <div className="btns">
              <button
                className="primary"
                onClick={() => act(api.park(d.id, "parked by human"))}
              >
                Park (keep session)
              </button>
              <button
                style={{ background: "var(--needs-you-bg)", color: "var(--needs-human)", borderColor: "var(--needs-human)" }}
                onClick={() => setConfirmHalt(true)}
              >
                Halt (discard session)
              </button>
            </div>
          ) : (
            <div
              style={{
                background: "var(--needs-you-bg)",
                border: "1px solid var(--needs-human)",
                borderRadius: "6px",
                padding: "10px 12px",
                display: "flex",
                flexDirection: "column",
                gap: "8px",
              }}
            >
              <div style={{ color: "var(--needs-human)", fontSize: "12px", fontWeight: 600 }}>
                Halt #{d.id} and discard the session?
              </div>
              <div style={{ color: "var(--needs-human)", fontSize: "11px" }}>
                The agent stops and the conversation is thrown away. The sandbox may
                still be kept for caches. Prefer Park if you want to resume later.
              </div>
              <div className="btns">
                <button
                  style={{ background: "var(--needs-you-bg)", color: "var(--needs-human)", borderColor: "var(--needs-human)" }}
                  onClick={() => {
                    setConfirmHalt(false);
                    act(api.halt(d.id, "halted by human"));
                  }}
                >
                  Confirm Halt
                </button>
                <button onClick={() => setConfirmHalt(false)}>Cancel</button>
              </div>
            </div>
          )}
        </Section>
      )}

      {(d.state === "backlog" || d.state === "ready") && d.parked && (
        <Section title="Parked session">
          <p className="dim" style={{ marginBottom: 8 }}>
            Agent is stopped. Sandbox
            {d.environment ? ` ${d.environment}` : ""} and conversation are kept.
            Resume queues the supervisor to continue
            {d.conversation_id ? " the same conversation" : ""}.
          </p>
          <div className="btns">
            <button className="primary" onClick={() => act(api.unpark(d.id))}>
              Resume
            </button>
          </div>
        </Section>
      )}

      {(d.state === "backlog" || d.state === "ready") && !d.parked && (
        <Section title="Dispatch">
          <p className="dim" style={{ marginBottom: 8 }}>
            {d.awaiting_dispatch
              ? "Queued — the supervisor will claim this when a sandbox slot opens."
              : "Nothing auto-starts from Backlog. Start when you want a run."}
          </p>
          <div className="btns">
            <button
              className="primary"
              disabled={!!d.awaiting_dispatch}
              onClick={() => act(api.dispatch(d.id))}
            >
              {d.awaiting_dispatch ? "Queued…" : "Start"}
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
              const inBacklog = d.state === "backlog" || d.state === "ready";
              const p = !inBacklog
                ? steerP.then(() => api.transition(d.id, "backlog", "steered by human"))
                : steerP;
              act(p);
              setText("");
            }}
          >
            Send Instruction
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

export const Head = ({
  title,
  onClose,
  onArchive,
  onDelete,
}: {
  title: string;
  onClose: () => void;
  onArchive?: () => void;
  onDelete?: () => void;
}) => (
  <div className="drawer-head" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
    <h3 style={{ margin: 0, flex: 1 }}>{title}</h3>
    <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
      {onArchive && (
        <button
          style={{
            fontSize: "11px",
            padding: "3px 8px",
            background: "var(--accent-fill)",
            color: "var(--accent)",
            border: "1px solid var(--accent)",
            borderRadius: "4px",
            cursor: "pointer",
            fontWeight: 600,
          }}
          onClick={onArchive}
          title="Archive (soft retire) item and its subtree"
        >
          📦 Archive
        </button>
      )}
      {onDelete && (
        <button
          style={{
            fontSize: "11px",
            padding: "3px 8px",
            background: "var(--needs-you-bg)",
            color: "var(--needs-human)",
            border: "1px solid var(--needs-human)",
            borderRadius: "4px",
            cursor: "pointer",
            fontWeight: 600,
          }}
          onClick={onDelete}
          title="Delete item permanently"
        >
          🗑 Delete
        </button>
      )}
      <button className="close" onClick={onClose}>✕</button>
    </div>
  </div>
);

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <div className="section">
    <div className="section-title">{title}</div>
    {children}
  </div>
);
