import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type { SandboxProfile } from "../types.js";

type ProfileDraft = {
  id: string;
  name: string;
  image: string;
  policy: string;
  cpu: string;
  memory: string;
  engine: string;
};

const emptyDraft = (): ProfileDraft => ({
  id: "",
  name: "",
  image: "",
  policy: "",
  cpu: "",
  memory: "",
  engine: "cursor",
});

function draftFrom(p: SandboxProfile): ProfileDraft {
  return {
    id: p.id,
    name: p.name,
    image: p.image,
    policy: p.policy,
    cpu: p.cpu ?? "",
    memory: p.memory ?? "",
    engine: p.engine?.trim() || "cursor",
  };
}

export function SandboxesPanelView({
  profiles,
  defaultId,
  cockpitId,
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
  onSetCockpit,
}: {
  profiles: SandboxProfile[];
  defaultId: string | null;
  cockpitId: string | null;
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
  onSetCockpit: (id: string) => void;
}) {
  const isCreate = editingId === "";
  const isEditing = editingId !== null;

  return (
    <div className="openshell-profiles" data-testid="openshell-profiles">
    <section aria-labelledby="openshell-profiles-title" data-testid="sandboxes-panel">
      <h3 id="openshell-profiles-title">Profiles</h3>
      <p className="dim">
        Named create-specs (image, policy, CPU, memory). The global default is
        used when a Project has no override. Cockpit Start builds its seat from
        the Cockpit profile. Live card environments are managed on the board,
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
              const isCockpit = cockpitId === p.id;
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
                      {isCockpit && (
                        <span
                          className="sandbox-default-badge"
                          data-testid="sandbox-cockpit-badge"
                        >
                          Cockpit
                        </span>
                      )}
                    </div>
                    <div className="dim sandbox-profile-meta">
                      {p.engine?.trim() || "engine: default"}
                      <span className="sep">·</span>
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
                    {!isCockpit && (
                      <button
                        type="button"
                        disabled={busy || isEditing}
                        onClick={() => onSetCockpit(p.id)}
                        data-testid={`sandbox-set-cockpit-${p.id}`}
                      >
                        Use for Cockpit
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
            Agent engine
            <select
              className="search-input"
              value={draft.engine}
              disabled={busy}
              onChange={(e) => onDraftChange({ ...draft, engine: e.target.value })}
              data-testid="sandbox-field-engine"
            >
              <option value="cursor">Cursor Agent (cursor)</option>
              <option value="agy">Antigravity CLI (agy)</option>
              <option value="claude">Claude Code (Anthropic)</option>
            </select>
            <span className="dim sandbox-field-hint">
              Cards using this profile run this CLI. Agent runtime supplies the
              fallback when unset on older profiles.
            </span>
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
              Inline OpenShell policy YAML pasted into the profile.
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
    </div>
  );
}

export function SandboxesPanel() {
  const [profiles, setProfiles] = useState<SandboxProfile[]>([]);
  const [defaultId, setDefaultId] = useState<string | null>(null);
  const [cockpitId, setCockpitId] = useState<string | null>(null);
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
        setCockpitId(out.cockpit_sandbox_profile_id);
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
      <div className="openshell-profiles" data-testid="openshell-profiles">
        <section aria-labelledby="openshell-profiles-title" data-testid="sandboxes-panel">
          <h3 id="openshell-profiles-title">Profiles</h3>
          <p className="dim">loading…</p>
        </section>
      </div>
    );
  }

  return (
    <SandboxesPanelView
      profiles={profiles}
      defaultId={defaultId}
      cockpitId={cockpitId}
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
          engine: draft.engine.trim() || null,
        };
        run(api.upsertSandboxProfile(body)).then(() => {
          setEditingId(null);
          setDraft(emptyDraft());
        });
      }}
      onSetDefault={(id) => run(api.setDefaultSandboxProfile(id))}
      onSetCockpit={(id) => run(api.setCockpitSandboxProfile(id))}
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
