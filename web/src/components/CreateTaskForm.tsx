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

/** Stamp Project default (or explicit field) into intent when prose omits a clone line. */
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
  /** Project intent — used to prefill / stamp the default clone target. */
  projectIntent: string;
  /** Sibling Tasks under the same Project (optional blockers). */
  siblings?: SiblingTaskOption[];
  onCreated: (item: WorkItem) => void;
  /** Compact trigger; form expands in place. Default true. */
  collapsible?: boolean;
}

/**
 * Create Task under an existing Project — title, intent, DoD, optional blockers.
 * Posts via `api.createTask` (POST /api/items with parent). Clone target is
 * required in intent/DoD prose; when omitted, stamps the Project default if known.
 */
export function CreateTaskForm({
  parentId,
  projectIntent,
  siblings = [],
  onCreated,
  collapsible = true,
}: CreateTaskFormProps) {
  const projectDefault = useMemo(
    () => cloneRepoFromProse(projectIntent),
    [projectIntent],
  );
  const [open, setOpen] = useState(!collapsible);
  const [title, setTitle] = useState("");
  const [intent, setIntent] = useState("");
  const [dod, setDod] = useState("");
  const [cloneRepo, setCloneRepo] = useState(projectDefault ?? "");
  const [blockedBy, setBlockedBy] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reset = () => {
    setTitle("");
    setIntent("");
    setDod("");
    setCloneRepo(projectDefault ?? "");
    setBlockedBy([]);
    setError(null);
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    const why = intent.trim();
    const done = dod.trim();
    const clone = cloneRepo.trim() || projectDefault || "";
    if (!t || !why || !done) {
      setError("Title, intent, and definition of done are required.");
      return;
    }
    if (!proseHasCloneRepo(why, done) && !clone) {
      setError(
        "clone_repo (owner/name) is required — name it in intent/DoD or the Project default.",
      );
      return;
    }
    const stampedIntent =
      proseHasCloneRepo(why, done) || !clone
        ? why
        : stampCloneIntoIntent(why, clone);

    setBusy(true);
    setError(null);
    api
      .createTask({
        parent: parentId,
        title: t,
        intent: stampedIntent,
        definition_of_done: done,
        blocked_by: blockedBy.length ? blockedBy : undefined,
      })
      .then((item) => {
        reset();
        if (collapsible) setOpen(false);
        onCreated(item);
      })
      .catch((err) => setError(String(err?.message ?? err)))
      .finally(() => setBusy(false));
  };

  const blockerChoices = siblings.filter((s) => !blockedBy.includes(s.id));

  return (
    <div className="create-task" data-testid="create-task">
      {collapsible && !open && (
        <button
          type="button"
          className="primary"
          onClick={() => {
            setCloneRepo((prev) => prev || projectDefault || "");
            setOpen(true);
          }}
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
          <p className="dim create-task-lede">
            Adds a Backlog card under this Project. Each Task must name its
            clone target (<code>owner/name</code>) in intent or definition of
            done
            {projectDefault ? (
              <>
                {" "}
                — default from Project: <code>{projectDefault}</code>
              </>
            ) : (
              <> when the Project has no default</>
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
          <label>
            <span>
              clone_repo (<code>owner/name</code>)
            </span>
            <input
              className="search-input"
              value={cloneRepo}
              disabled={busy}
              required={!projectDefault}
              placeholder={projectDefault ?? "owner/name"}
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setCloneRepo(e.target.value)}
              data-testid="create-task-clone-repo"
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
                busy ||
                !title.trim() ||
                !intent.trim() ||
                !dod.trim() ||
                (!projectDefault && !cloneRepo.trim())
              }
              data-testid="create-task-submit"
            >
              {busy ? "Creating…" : "Create Task"}
            </button>
            {collapsible && (
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
