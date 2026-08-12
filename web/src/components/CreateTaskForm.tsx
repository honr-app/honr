import { useMemo, useState, type FormEvent } from "react";
import { api } from "../api.js";
import type { WorkItem } from "../types.js";

/** Match server `clone_repo_from_prose` — first `Clone repository: owner/name` token. */
export function cloneRepoFromProse(text: string): string | null {
  for (const line of text.split("\n")) {
    const t = line.trim();
    const lower = t.toLowerCase();
    if (!lower.startsWith("clone repository:")) continue;
    const rest = t.slice("clone repository:".length).trim();
    const token = rest.split(/\s+/)[0];
    if (!token) continue;
    const cleaned = token.replace(/[.,;:]+$/, "");
    if (/^[^/\s]+\/[^/\s]+$/.test(cleaned)) return cleaned;
  }
  return null;
}

export function proseHasCloneRepo(intent: string, dod: string): boolean {
  return (
    cloneRepoFromProse(intent) != null || cloneRepoFromProse(dod) != null
  );
}

/** Same stamp shape the board applies when Why/DoD omit a clone line. */
export function stampCloneIntoIntent(
  intent: string,
  clone: string,
): string {
  const stamp = `Clone repository: ${clone}.`;
  const trimmed = intent.trim();
  return trimmed ? `${stamp} ${trimmed}` : stamp;
}

export interface SiblingTaskOption {
  id: number;
  title: string;
}

export interface CreateTaskFormProps {
  parentId: number;
  /** Project intent — used to surface the Project default clone in the lede. */
  projectIntent: string;
  /** Sibling Tasks under the same Project (optional blockers). */
  siblings?: SiblingTaskOption[];
  onCreated: (item: WorkItem) => void;
  /** Compact trigger; form expands in place. Default true. */
  collapsible?: boolean;
  /**
   * Controlled open state. When set with `onOpenChange`, the parent owns
   * expand/collapse (e.g. Board puts the trigger in the lane header).
   */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Omit the closed-state trigger — parent renders `create-task-open` itself. */
  hideTrigger?: boolean;
  /** Classes for the built-in open trigger. Default `primary`. */
  triggerClassName?: string;
}

/**
 * Create Task under an existing Project — title, intent, DoD, optional blockers.
 * Posts via `api.createTask` (same fields as MCP `create_task`). Clone target is
 * named in Why/DoD when needed; otherwise the board stamps the Project default.
 */
export function CreateTaskForm({
  parentId,
  projectIntent,
  siblings = [],
  onCreated,
  collapsible = true,
  open: openProp,
  onOpenChange,
  hideTrigger = false,
  triggerClassName = "primary",
}: CreateTaskFormProps) {
  const projectDefault = useMemo(
    () => cloneRepoFromProse(projectIntent),
    [projectIntent],
  );
  const controlled = openProp !== undefined;
  const [internalOpen, setInternalOpen] = useState(!collapsible);
  const open = controlled ? openProp : internalOpen;
  const setOpen = (next: boolean) => {
    if (!controlled) setInternalOpen(next);
    onOpenChange?.(next);
  };
  const [title, setTitle] = useState("");
  const [intent, setIntent] = useState("");
  const [dod, setDod] = useState("");
  const [blockedBy, setBlockedBy] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reset = () => {
    setTitle("");
    setIntent("");
    setDod("");
    setBlockedBy([]);
    setError(null);
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    const why = intent.trim();
    const done = dod.trim();
    if (!t || !why || !done) {
      setError("Title, intent, and definition of done are required.");
      return;
    }

    setBusy(true);
    setError(null);
    api
      .createTask({
        parent: parentId,
        title: t,
        intent: why,
        definition_of_done: done,
        blocked_by: blockedBy.length ? blockedBy : undefined,
      })
      .then((item) => {
        reset();
        if (collapsible || controlled) setOpen(false);
        onCreated(item);
      })
      .catch((err) => setError(String(err?.message ?? err)))
      .finally(() => setBusy(false));
  };

  const blockerChoices = siblings.filter((s) => !blockedBy.includes(s.id));
  const canCollapse = collapsible || controlled;

  return (
    <div className="create-task" data-testid="create-task">
      {canCollapse && !open && !hideTrigger && (
        <button
          type="button"
          className={triggerClassName}
          onClick={() => setOpen(true)}
          data-testid="create-task-open"
        >
          Create Task
        </button>
      )}

      {open && (
        <form
          className="sandbox-profile-form workspace-form create-task-form"
          onSubmit={submit}
          data-testid="create-task-form"
        >
          <h3>Create Task</h3>
          <p className="dim create-task-lede" data-testid="create-task-clone-hint">
            Adds a Backlog card under this Project. Same fields as MCP{" "}
            <code>create_task</code>
            {projectDefault ? (
              <>
                {" "}
                — Project default clone <code>{projectDefault}</code> is stamped
                when Why/DoD omit it
              </>
            ) : (
              <>
                {" "}
                — this Project has no default; name{" "}
                <code>Clone repository: owner/name</code> in Why or DoD
              </>
            )}
            .
          </p>
          {error && (
            <div className="err" data-testid="create-task-error">
              {error}
            </div>
          )}
          <label>
            Title
            <input
              className="search-input"
              value={title}
              disabled={busy}
              required
              onChange={(e) => setTitle(e.target.value)}
              data-testid="create-task-title"
            />
          </label>
          <label>
            Why / intent
            <textarea
              className="search-input"
              value={intent}
              disabled={busy}
              required
              rows={3}
              onChange={(e) => setIntent(e.target.value)}
              data-testid="create-task-intent"
            />
          </label>
          <label>
            Definition of done
            <textarea
              className="search-input"
              value={dod}
              disabled={busy}
              required
              rows={3}
              onChange={(e) => setDod(e.target.value)}
              data-testid="create-task-dod"
            />
          </label>
          {siblings.length > 0 && (
            <div data-testid="create-task-blockers">
              <div className="create-task-blockers-label">
                Optional blockers (sibling Tasks)
              </div>
              <div className="create-task-blocker-chips">
                {blockedBy.length === 0 ? (
                  <span className="dim" style={{ fontSize: 11 }}>
                    None
                  </span>
                ) : (
                  blockedBy.map((id) => {
                    const s = siblings.find((x) => x.id === id);
                    return (
                      <span key={id} className="blocker-chip">
                        <span className="blocker-id">#{id}</span>
                        {s?.title && (
                          <span className="blocker-title">{s.title}</span>
                        )}
                        <button
                          type="button"
                          disabled={busy}
                          title={`Remove #${id} blocker`}
                          data-testid={`create-task-blocker-remove-${id}`}
                          onClick={() =>
                            setBlockedBy((prev) => prev.filter((x) => x !== id))
                          }
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
                        >
                          ✕
                        </button>
                      </span>
                    );
                  })
                )}
                {blockerChoices.length > 0 && (
                  <select
                    className="search-input"
                    style={{ fontSize: 11, padding: "2px 6px", height: 26 }}
                    value=""
                    disabled={busy}
                    data-testid="create-task-blocker-add"
                    onChange={(e) => {
                      const val = Number(e.target.value);
                      if (!val) return;
                      setBlockedBy((prev) =>
                        prev.includes(val) ? prev : [...prev, val],
                      );
                    }}
                  >
                    <option value="">Add blocker…</option>
                    {blockerChoices.map((s) => (
                      <option key={s.id} value={s.id}>
                        #{s.id} {s.title}
                      </option>
                    ))}
                  </select>
                )}
              </div>
            </div>
          )}
          <div className="btns create-task-actions">
            <button
              type="submit"
              className="primary"
              disabled={
                busy || !title.trim() || !intent.trim() || !dod.trim()
              }
              data-testid="create-task-submit"
            >
              {busy ? "Creating…" : "Create Task"}
            </button>
            {canCollapse && (
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setOpen(false);
                  setError(null);
                }}
                data-testid="create-task-cancel"
              >
                Cancel
              </button>
            )}
          </div>
        </form>
      )}
    </div>
  );
}
