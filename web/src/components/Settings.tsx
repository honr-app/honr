import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type { SandboxProfile, WorkspaceBinding } from "../types.js";

type SettingsSection = "sandboxes" | "workspace";

const SECTIONS: { id: SettingsSection; label: string; stub?: boolean }[] = [
  { id: "sandboxes", label: "Sandboxes" },
  { id: "workspace", label: "Workspace" },
];

type ProfileDraft = {
  id: string;
  name: string;
  image: string;
  policy: string;
  cpu: string;
  memory: string;
};

const emptyDraft = (): ProfileDraft => ({
  id: "",
  name: "",
  image: "",
  policy: "",
  cpu: "",
  memory: "",
});

function draftFrom(p: SandboxProfile): ProfileDraft {
  return {
    id: p.id,
    name: p.name,
    image: p.image,
    policy: p.policy,
    cpu: p.cpu ?? "",
    memory: p.memory ?? "",
  };
}

const emptyWorkspace = (): WorkspaceBinding => ({
  forge: "github",
  upstream: "",
  fork: "",
  base: "main",
  beads_sync_repo: "",
});

/**
 * Settings shell — Sandboxes + Workspace are real panels; more sections land
 * via the generalization roadmap (Agent runtime, OpenShell).
 */
export function Settings() {
  const [section, setSection] = useState<SettingsSection>("sandboxes");

  return (
    <div className="settings" data-testid="settings">
      <header className="settings-hero">
        <h1>Settings</h1>
        <p className="settings-lede">
          Control-plane preferences. Workspace holds optional forge defaults and
          beads sync; card <code>pr_url</code> drives multi-repo work remotes.
          Sandboxes manages named profiles and the global default.
        </p>
      </header>

      <div className="settings-body">
        <nav className="settings-nav" aria-label="Settings sections">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              className={`settings-nav-btn ${section === s.id ? "active" : ""}`}
              aria-current={section === s.id ? "page" : undefined}
              onClick={() => setSection(s.id)}
              data-testid={`settings-nav-${s.id}`}
            >
              {s.label}
              {s.stub && <span className="dim settings-stub-tag">soon</span>}
            </button>
          ))}
        </nav>

        <div className="settings-panel" data-testid={`settings-panel-${section}`}>
          {section === "sandboxes" ? <SandboxesPanel /> : <WorkspacePanel />}
        </div>
      </div>
    </div>
  );
}

/** Presentational list + form — exported so tests can render without fetch. */
export function SandboxesPanelView({
  profiles,
  defaultId,
  busy,
  error,
  editingId,
  draft,
  onDraftChange,
  onStartCreate,
  onStartEdit,
  onCancelEdit,
  onSave,
  onSetDefault,
}: {
  profiles: SandboxProfile[];
  defaultId: string | null;
  busy?: boolean;
  error?: string | null;
  editingId: string | null;
  draft: ProfileDraft;
  onDraftChange: (next: ProfileDraft) => void;
  onStartCreate: () => void;
  onStartEdit: (p: SandboxProfile) => void;
  onCancelEdit: () => void;
  onSave: () => void;
  onSetDefault: (id: string) => void;
}) {
  const isCreate = editingId === "";
  const isEditing = editingId !== null;

  return (
    <section aria-labelledby="sandboxes-title" data-testid="sandboxes-panel">
      <h2 id="sandboxes-title">Sandboxes</h2>
      <p className="dim">
        Named create-specs for OpenShell. The global default is used when a
        Project has no override. Live card environments are managed on the board,
        not here.
      </p>

      {error && <div className="err">{error}</div>}

      <div className="sandbox-profile-list" data-testid="sandbox-profile-list">
        {profiles.length === 0 ? (
          <div className="settings-placeholder" data-testid="sandboxes-empty">
            <p>No profiles yet.</p>
            <p className="dim">Create one, or wait for the catalog to seed from config.</p>
          </div>
        ) : (
          <ul className="sandbox-profile-ul">
            {profiles.map((p) => {
              const isDefault = defaultId === p.id;
              return (
                <li
                  key={p.id}
                  className="sandbox-profile-row"
                  data-testid={`sandbox-profile-${p.id}`}
                >
                  <div className="sandbox-profile-main">
                    <div className="sandbox-profile-title">
                      <strong>{p.name}</strong>
                      <span className="dim sandbox-profile-id">{p.id}</span>
                      {isDefault && (
                        <span className="sandbox-default-badge" data-testid="sandbox-default-badge">
                          default
                        </span>
                      )}
                    </div>
                    <div className="dim sandbox-profile-meta">
                      {p.image}
                      {(p.cpu || p.memory) && (
                        <>
                          <span className="sep">·</span>
                          {[p.cpu, p.memory].filter(Boolean).join(" / ")}
                        </>
                      )}
                    </div>
                  </div>
                  <div className="sandbox-profile-actions">
                    <button
                      type="button"
                      disabled={busy || isEditing}
                      onClick={() => onStartEdit(p)}
                      data-testid={`sandbox-edit-${p.id}`}
                    >
                      Edit
                    </button>
                    {!isDefault && (
                      <button
                        type="button"
                        className="primary"
                        disabled={busy || isEditing}
                        onClick={() => onSetDefault(p.id)}
                        data-testid={`sandbox-set-default-${p.id}`}
                      >
                        Set default
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {!isEditing && (
        <div className="btns sandbox-profile-toolbar">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={onStartCreate}
            data-testid="sandbox-create"
          >
            New profile
          </button>
        </div>
      )}

      {isEditing && (
        <form
          className="sandbox-profile-form"
          data-testid="sandbox-profile-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSave();
          }}
        >
          <h3>{isCreate ? "Create profile" : `Edit ${editingId}`}</h3>
          {!isCreate && (
            <label>
              Id
              <input
                className="search-input"
                value={draft.id}
                disabled
                readOnly
                data-testid="sandbox-field-id"
              />
            </label>
          )}
          <label>
            Name
            <input
              className="search-input"
              value={draft.name}
              disabled={busy}
              onChange={(e) => onDraftChange({ ...draft, name: e.target.value })}
              required
              data-testid="sandbox-field-name"
            />
          </label>
          <label>
            Image
            <input
              className="search-input"
              value={draft.image}
              disabled={busy}
              onChange={(e) => onDraftChange({ ...draft, image: e.target.value })}
              required
              data-testid="sandbox-field-image"
            />
          </label>
          <label>
            Policy (YAML)
            <textarea
              className="sandbox-policy-textarea"
              value={draft.policy}
              disabled={busy}
              onChange={(e) => onDraftChange({ ...draft, policy: e.target.value })}
              required
              rows={10}
              spellCheck={false}
              placeholder={"version: 1\nfilesystem_policy:\n  include_workdir: true\n"}
              data-testid="sandbox-field-policy"
            />
            <span className="dim sandbox-field-hint">
              Inline OpenShell policy YAML — not a path on the host.
            </span>
          </label>
          <div className="sandbox-profile-form-row">
            <label>
              CPU
              <input
                className="search-input"
                value={draft.cpu}
                disabled={busy}
                placeholder="optional"
                onChange={(e) => onDraftChange({ ...draft, cpu: e.target.value })}
                data-testid="sandbox-field-cpu"
              />
            </label>
            <label>
              Memory
              <input
                className="search-input"
                value={draft.memory}
                disabled={busy}
                placeholder="optional"
                onChange={(e) => onDraftChange({ ...draft, memory: e.target.value })}
                data-testid="sandbox-field-memory"
              />
            </label>
          </div>
          <div className="btns">
            <button type="submit" className="primary" disabled={busy} data-testid="sandbox-save">
              {isCreate ? "Create" : "Save"}
            </button>
            <button type="button" disabled={busy} onClick={onCancelEdit}>
              Cancel
            </button>
          </div>
        </form>
      )}
    </section>
  );
}

function SandboxesPanel() {
  const [profiles, setProfiles] = useState<SandboxProfile[]>([]);
  const [defaultId, setDefaultId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState<ProfileDraft>(emptyDraft);

  const refresh = useCallback(() => {
    setLoading(true);
    return api
      .listSandboxProfiles()
      .then((out) => {
        setProfiles(out.profiles);
        setDefaultId(out.default_sandbox_profile_id);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const run = (p: Promise<unknown>) => {
    setBusy(true);
    setError(null);
    return p
      .then(() => refresh())
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  if (loading && profiles.length === 0 && !error) {
    return (
      <section aria-labelledby="sandboxes-title" data-testid="sandboxes-panel">
        <h2 id="sandboxes-title">Sandboxes</h2>
        <p className="dim">loading…</p>
      </section>
    );
  }

  return (
    <SandboxesPanelView
      profiles={profiles}
      defaultId={defaultId}
      busy={busy}
      error={error}
      editingId={editingId}
      draft={draft}
      onDraftChange={setDraft}
      onStartCreate={() => {
        setEditingId("");
        setDraft(emptyDraft());
      }}
      onStartEdit={(p) => {
        setEditingId(p.id);
        setDraft(draftFrom(p));
      }}
      onCancelEdit={() => {
        setEditingId(null);
        setDraft(emptyDraft());
      }}
      onSave={() => {
        const body = {
          ...(editingId ? { id: draft.id.trim() } : {}),
          name: draft.name.trim(),
          image: draft.image.trim(),
          // Keep YAML as typed (trailing newline is normal); server rejects empty.
          policy: draft.policy,
          cpu: draft.cpu.trim() || null,
          memory: draft.memory.trim() || null,
        };
        run(api.upsertSandboxProfile(body)).then(() => {
          setEditingId(null);
          setDraft(emptyDraft());
        });
      }}
      onSetDefault={(id) => run(api.setDefaultSandboxProfile(id))}
    />
  );
}

/** Project-level sandbox override picker (unset = global default). */
export function ProjectSandboxPicker({
  projectId,
  value,
  profiles,
  defaultId,
  busy,
  error,
  onChange,
}: {
  projectId: number;
  value: string | null | undefined;
  profiles: SandboxProfile[];
  defaultId: string | null;
  busy?: boolean;
  error?: string | null;
  onChange: (next: string | null) => void;
}) {
  const defaultLabel =
    defaultId != null
      ? profiles.find((p) => p.id === defaultId)?.name ?? defaultId
      : "none configured";

  return (
    <div className="project-sandbox-picker" data-testid="project-sandbox-picker">
      <label className="section-title" style={{ display: "block", marginBottom: 2 }}>
        Sandbox profile
      </label>
      <p className="dim" style={{ marginBottom: 4, fontSize: 12 }}>
        Override for this Project. Unset uses the global default
        ({defaultLabel}).
      </p>
      {error && <div className="err">{error}</div>}
      <select
        className="search-input"
        style={{ width: "100%", background: "var(--panel)", color: "var(--ink)", padding: "6px" }}
        value={value ?? ""}
        disabled={busy}
        data-testid={`project-sandbox-select-${projectId}`}
        onChange={(e) => {
          const v = e.target.value;
          onChange(v === "" ? null : v);
        }}
      >
        <option value="">Use global default</option>
        {profiles.map((p) => (
          <option key={p.id} value={p.id}>
            {p.id === defaultId ? `${p.name} · global default` : p.name}
          </option>
        ))}
      </select>
    </div>
  );
}

/** Presentational Workspace form — exported for UI tests without fetch. */
export function WorkspacePanelView({
  draft,
  busy,
  error,
  savedHint,
  onDraftChange,
  onSave,
}: {
  draft: WorkspaceBinding;
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onDraftChange: (next: WorkspaceBinding) => void;
  onSave: () => void;
}) {
  const incomplete = !draft.upstream.trim() || !draft.fork.trim();

  return (
    <section aria-labelledby="workspace-title" data-testid="workspace-panel">
      <h2 id="workspace-title">Workspace</h2>
      <p className="dim">
        Optional install defaults for forge remotes and beads Issue sync.
        Seeded from <code>honr.yaml</code> on first boot. Work remotes for a
        card come from its <code>pr_url</code> when set (multi-repo); these
        fields are a fallback for first clone, not the only work target.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="workspace-saved-hint">
          {savedHint}
        </p>
      )}

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="workspace-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label>
          Forge
          <select
            className="search-input"
            value={draft.forge}
            disabled={busy}
            onChange={(e) => onDraftChange({ ...draft, forge: e.target.value })}
            data-testid="workspace-field-forge"
          >
            <option value="github">GitHub</option>
            <option value="gitlab" disabled>
              GitLab (future)
            </option>
          </select>
        </label>
        <label>
          Upstream
          <input
            className="search-input"
            value={draft.upstream}
            disabled={busy}
            placeholder="owner/name"
            onChange={(e) => onDraftChange({ ...draft, upstream: e.target.value })}
            data-testid="workspace-field-upstream"
          />
          <span className="dim sandbox-field-hint">
            Optional default PR target (<code>owner/name</code>). Cards with a
            <code> pr_url</code> use that upstream instead.
          </span>
        </label>
        <label>
          Fork
          <input
            className="search-input"
            value={draft.fork}
            disabled={busy}
            placeholder="owner/name"
            onChange={(e) => onDraftChange({ ...draft, fork: e.target.value })}
            data-testid="workspace-field-fork"
          />
          <span className="dim sandbox-field-hint">
            Optional default fork (bot <code>owner/name</code>). For other
            upstreams, the bot owner is reused with that repo name.
          </span>
        </label>
        <label>
          Base branch
          <input
            className="search-input"
            value={draft.base}
            disabled={busy}
            placeholder="main"
            onChange={(e) => onDraftChange({ ...draft, base: e.target.value })}
            data-testid="workspace-field-base"
          />
        </label>
        <label>
          Beads sync repo
          <input
            className="search-input"
            value={draft.beads_sync_repo ?? ""}
            disabled={busy}
            placeholder="defaults to upstream"
            onChange={(e) =>
              onDraftChange({ ...draft, beads_sync_repo: e.target.value })
            }
            data-testid="workspace-field-beads"
          />
          <span className="dim sandbox-field-hint">
            Explicit beads ↔ GitHub Issues mirror (<code>owner/name</code>).
            Empty falls back to the Workspace upstream default when set.
          </span>
        </label>

        {incomplete && (
          <p className="dim" data-testid="workspace-incomplete-hint">
            Defaults incomplete — first clone needs upstream and fork here (or
            in yaml), or a card <code>pr_url</code> after the first PR. Agents
            can still start; per-card resolve fails closed when remotes are
            missing.
          </p>
        )}

        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="workspace-save">
            Save
          </button>
        </div>
      </form>

      <aside className="workspace-webhook-hint" data-testid="workspace-webhook-hint">
        <h3>Local webhook forward</h3>
        <p className="dim">
          Point <code>gh webhook forward</code> at each upstream you care about
          (template — not a single hard-coded repo). Cards complete on{" "}
          <code>pr_url</code> match:
        </p>
        <pre data-testid="workspace-webhook-example">{`gh webhook forward \\
  --repo=<owner/name> \\
  --events=pull_request,push \\
  --url=http://127.0.0.1:8080/api/webhooks/github`}</pre>
      </aside>
    </section>
  );
}

function WorkspacePanel() {
  const [draft, setDraft] = useState<WorkspaceBinding>(emptyWorkspace);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    return api
      .getWorkspace()
      .then((ws) => {
        setDraft({
          forge: ws.forge || "github",
          upstream: ws.upstream ?? "",
          fork: ws.fork ?? "",
          base: ws.base || "main",
          beads_sync_repo: ws.beads_sync_repo ?? "",
        });
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  if (loading && !error) {
    return (
      <section aria-labelledby="workspace-title" data-testid="workspace-panel">
        <h2 id="workspace-title">Workspace</h2>
        <p className="dim">loading…</p>
      </section>
    );
  }

  return (
    <WorkspacePanelView
      draft={draft}
      busy={busy}
      error={error}
      savedHint={savedHint}
      onDraftChange={(next) => {
        setSavedHint(null);
        setDraft(next);
      }}
      onSave={() => {
        setBusy(true);
        setError(null);
        setSavedHint(null);
        const body: WorkspaceBinding = {
          forge: draft.forge.trim() || "github",
          upstream: draft.upstream.trim(),
          fork: draft.fork.trim(),
          base: draft.base.trim() || "main",
          beads_sync_repo: (draft.beads_sync_repo ?? "").trim() || null,
        };
        api
          .putWorkspace(body)
          .then((saved) => {
            setDraft({
              forge: saved.forge,
              upstream: saved.upstream,
              fork: saved.fork,
              base: saved.base,
              beads_sync_repo: saved.beads_sync_repo ?? "",
            });
            setSavedHint("Saved. Binding is board state — used after reload.");
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}
