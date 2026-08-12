import { useState, type FormEvent } from "react";
import { api } from "../api.js";
import type { WorkItem } from "../types.js";

export interface CreateProjectFormProps {
  /** Called after create succeeds — typically refresh then open. */
  onCreated: (item: WorkItem) => void;
  /** When true, start with the form open (empty Welcome board). */
  initiallyOpen?: boolean;
  /** Compact trigger for non-empty boards; form expands in place. */
  collapsible?: boolean;
}

/**
 * Create Project — title, why/intent, required clone_repo (`owner/name`).
 * Posts via `api.createProject` (existing POST /api/items create path).
 */
export function CreateProjectForm({
  onCreated,
  initiallyOpen = false,
  collapsible = false,
}: CreateProjectFormProps) {
  const [open, setOpen] = useState(initiallyOpen || !collapsible);
  const [title, setTitle] = useState("");
  const [intent, setIntent] = useState("");
  const [cloneRepo, setCloneRepo] = useState("");
  const [projectPrompt, setProjectPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    const why = intent.trim();
    const clone = cloneRepo.trim();
    if (!t || !why || !clone) {
      setError("Title, intent, and clone_repo (owner/name) are required.");
      return;
    }
    setBusy(true);
    setError(null);
    const prompt = projectPrompt.trim();
    api
      .createProject({
        title: t,
        intent: why,
        clone_repo: clone,
        ...(prompt ? { project_prompt: prompt } : {}),
      })
      .then((item) => {
        setTitle("");
        setIntent("");
        setCloneRepo("");
        setProjectPrompt("");
        if (collapsible) setOpen(false);
        onCreated(item);
      })
      .catch((err) => setError(String(err?.message ?? err)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="create-project" data-testid="create-project">
      {collapsible && !open && (
        <button
          type="button"
          className="primary"
          onClick={() => setOpen(true)}
          data-testid="create-project-open"
        >
          Create Project
        </button>
      )}

      {open && (
        <form
          className="sandbox-profile-form workspace-form create-project-form"
          onSubmit={submit}
          data-testid="create-project-form"
        >
          <h3>Create Project</h3>
          <p className="dim create-project-lede">
            New Projects need a <code>clone_repo</code> as{" "}
            <code>owner/name</code> — the Initial plan clones that repo.
          </p>
          {error && (
            <div className="err" data-testid="create-project-error">
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
              data-testid="create-project-title"
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
              data-testid="create-project-intent"
            />
          </label>
          <label>
            {/* Span keeps caption one flex item — bare text+code stacks in column labels. */}
            <span>
              clone_repo (<code>owner/name</code>)
            </span>
            <input
              className="search-input"
              value={cloneRepo}
              disabled={busy}
              required
              placeholder="owner/name"
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setCloneRepo(e.target.value)}
              data-testid="create-project-clone-repo"
            />
          </label>
          <label>
            Project prompt (optional)
            <textarea
              className="search-input"
              value={projectPrompt}
              disabled={busy}
              rows={4}
              placeholder="Standing instructions for agents on this Project…"
              onChange={(e) => setProjectPrompt(e.target.value)}
              data-testid="create-project-prompt"
            />
          </label>
          <div className="btns create-project-actions">
            <button
              type="submit"
              className="primary"
              disabled={busy || !title.trim() || !intent.trim() || !cloneRepo.trim()}
              data-testid="create-project-submit"
            >
              {busy ? "Creating…" : "Create Project"}
            </button>
            {collapsible && (
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  setOpen(false);
                  setError(null);
                }}
                data-testid="create-project-cancel"
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
