import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  AgentRuntimeConfig,
  OpenShellSettings,
  OpenShellStatus,
  SandboxProfile,
  WorkspaceBinding,
} from "../types.js";

type SettingsSection = "sandboxes" | "workspace" | "agent-runtime" | "openshell";

const SECTIONS: { id: SettingsSection; label: string; stub?: boolean }[] = [
  { id: "sandboxes", label: "Sandboxes" },
  // Nav label is Forge — "Workspace" implied a single work repo (upstream/fork).
  { id: "workspace", label: "Forge" },
  { id: "agent-runtime", label: "Agent runtime" },
  { id: "openshell", label: "OpenShell" },
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
  beads_sync_repo: "",
});

const emptyAgentRuntime = (): AgentRuntimeConfig => ({
  enabled: false,
  engine: "cursor",
  providers: [],
  vertex: { project: "", location: "global", model: "claude-opus-5" },
  max_concurrent: 2,
  per_card_budget_cents: null,
  daily_budget_cents: null,
  agent_timeout_secs: 1800,
  max_attempts: 3,
});

/**
 * Settings shell — Sandboxes, Forge, Agent runtime, and OpenShell connectivity.
 */
export function Settings() {
  const [section, setSection] = useState<SettingsSection>("sandboxes");

  return (
    <div className="settings" data-testid="settings">
      <header className="settings-hero">
        <h1>Settings</h1>
        <p className="settings-lede">
          Control-plane preferences. Forge holds Issue sync — not a work repo.
          Each card’s <code>pull_request</code> (after report) holds remotes.
          Sandboxes manages named profiles and the global default. Agent runtime
          holds engine, Vertex, providers, and budgets. OpenShell shows gateway
          health on this host.
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
          {section === "sandboxes" ? (
            <SandboxesPanel />
          ) : section === "workspace" ? (
            <WorkspacePanel />
          ) : section === "agent-runtime" ? (
            <AgentRuntimePanel />
          ) : (
            <OpenShellPanel />
          )}
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

/** Presentational Forge form — exported for UI tests without fetch. */
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
  return (
    <section aria-labelledby="workspace-title" data-testid="workspace-panel">
      <h2 id="workspace-title">Forge</h2>
      <p className="dim">
        Where beads mirrors Issues, and which forge provider you use. This is
        not the product repo agents open PRs against — that comes from each
        card’s <code>pull_request</code> (url / base / head) after the agent
        reports.
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
          Provider
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
          Beads sync repo
          <input
            className="search-input"
            value={draft.beads_sync_repo ?? ""}
            disabled={busy}
            placeholder="owner/name — where Issues are mirrored"
            onChange={(e) =>
              onDraftChange({ ...draft, beads_sync_repo: e.target.value })
            }
            data-testid="workspace-field-beads"
          />
          <span className="dim sandbox-field-hint">
            Explicit beads ↔ GitHub Issues mirror. Independent of which product
            repos agents open PRs against.
          </span>
        </label>

        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="workspace-save">
            Save
          </button>
        </div>
      </form>

      <aside className="workspace-webhook-hint" data-testid="workspace-webhook-hint">
        <h3>Local webhook forward</h3>
        <p className="dim">
          Run one forwarder per product upstream you care about. Cards complete
          on <code>pull_request.url</code> match, not on these Settings fields:
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
        <h2 id="workspace-title">Forge</h2>
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
          beads_sync_repo: (draft.beads_sync_repo ?? "").trim() || null,
        };
        api
          .putWorkspace(body)
          .then((saved) => {
            setDraft({
              forge: saved.forge,
              beads_sync_repo: saved.beads_sync_repo ?? "",
            });
            setSavedHint("Saved. Forge + beads sync update board state.");
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}

/** Presentational Agent runtime form — exported for UI tests without fetch. */
export function AgentRuntimePanelView({
  draft,
  busy,
  error,
  savedHint,
  onDraftChange,
  onSave,
}: {
  draft: AgentRuntimeConfig;
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onDraftChange: (next: AgentRuntimeConfig) => void;
  onSave: () => void;
}) {
  const providersText = draft.providers.join(", ");
  return (
    <section aria-labelledby="agent-runtime-title" data-testid="agent-runtime-panel">
      <h2 id="agent-runtime-title">Agent runtime</h2>
      <p className="dim">
        Process knobs for OpenShell sandboxes: default engine, provider names,
        Vertex project/location/model, concurrency and budgets. Seeded from{" "}
        <code>honr.yaml</code>; edits persist on the Board and apply to the next
        sandbox create. Image/policy live under Sandboxes. Host credential paths
        stay documented overrides — not silent home assumptions.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="agent-runtime-saved-hint">
          {savedHint}
        </p>
      )}

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="agent-runtime-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label className="agent-runtime-check">
          <input
            type="checkbox"
            checked={draft.enabled}
            disabled={busy}
            onChange={(e) => onDraftChange({ ...draft, enabled: e.target.checked })}
            data-testid="agent-runtime-field-enabled"
          />
          Agents enabled
          <span className="dim sandbox-field-hint">
            Turning agents on when the process started disabled still needs a
            honr restart so the dispatch loop starts.
          </span>
        </label>

        <label>
          Default engine
          <select
            className="search-input"
            value={draft.engine}
            disabled={busy}
            onChange={(e) => onDraftChange({ ...draft, engine: e.target.value })}
            data-testid="agent-runtime-field-engine"
          >
            <option value="cursor">cursor</option>
            <option value="agy">agy</option>
            <option value="claude">claude</option>
          </select>
        </label>

        <label>
          OpenShell providers
          <input
            className="search-input"
            value={providersText}
            disabled={busy}
            placeholder="vertex, gh-bot, cursor-honr — comma-separated"
            onChange={(e) =>
              onDraftChange({
                ...draft,
                providers: e.target.value
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean),
              })
            }
            data-testid="agent-runtime-field-providers"
          />
          <span className="dim sandbox-field-hint">
            Names must match local OpenShell gateway registrations.
          </span>
        </label>

        <div className="sandbox-profile-form-row">
          <label>
            Vertex project
            <input
              className="search-input"
              value={draft.vertex.project}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  vertex: { ...draft.vertex, project: e.target.value },
                })
              }
              data-testid="agent-runtime-field-vertex-project"
            />
          </label>
          <label>
            Vertex location
            <input
              className="search-input"
              value={draft.vertex.location}
              disabled={busy}
              placeholder="global"
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  vertex: { ...draft.vertex, location: e.target.value },
                })
              }
              data-testid="agent-runtime-field-vertex-location"
            />
          </label>
        </div>

        <label>
          Vertex model
          <input
            className="search-input"
            value={draft.vertex.model}
            disabled={busy}
            onChange={(e) =>
              onDraftChange({
                ...draft,
                vertex: { ...draft.vertex, model: e.target.value },
              })
            }
            data-testid="agent-runtime-field-vertex-model"
          />
        </label>

        <div className="sandbox-profile-form-row">
          <label>
            Max concurrent
            <input
              className="search-input"
              type="number"
              min={1}
              value={draft.max_concurrent}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  max_concurrent: Math.max(1, Number(e.target.value) || 1),
                })
              }
              data-testid="agent-runtime-field-max-concurrent"
            />
          </label>
          <label>
            Agent timeout (secs)
            <input
              className="search-input"
              type="number"
              min={1}
              value={draft.agent_timeout_secs}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  agent_timeout_secs: Math.max(1, Number(e.target.value) || 1),
                })
              }
              data-testid="agent-runtime-field-timeout"
            />
          </label>
        </div>

        <div className="sandbox-profile-form-row">
          <label>
            Per-card budget (cents)
            <input
              className="search-input"
              type="number"
              min={0}
              value={draft.per_card_budget_cents ?? ""}
              disabled={busy}
              placeholder="none"
              onChange={(e) => {
                const v = e.target.value.trim();
                onDraftChange({
                  ...draft,
                  per_card_budget_cents: v === "" ? null : Math.max(0, Number(v) || 0),
                });
              }}
              data-testid="agent-runtime-field-per-card-budget"
            />
          </label>
          <label>
            Daily budget (cents)
            <input
              className="search-input"
              type="number"
              min={0}
              value={draft.daily_budget_cents ?? ""}
              disabled={busy}
              placeholder="none"
              onChange={(e) => {
                const v = e.target.value.trim();
                onDraftChange({
                  ...draft,
                  daily_budget_cents: v === "" ? null : Math.max(0, Number(v) || 0),
                });
              }}
              data-testid="agent-runtime-field-daily-budget"
            />
          </label>
        </div>

        <label>
          Max attempts
          <input
            className="search-input"
            type="number"
            min={1}
            value={draft.max_attempts}
            disabled={busy}
            onChange={(e) =>
              onDraftChange({
                ...draft,
                max_attempts: Math.max(1, Number(e.target.value) || 1),
              })
            }
            data-testid="agent-runtime-field-max-attempts"
          />
        </label>

        <div className="btns">
          <button
            type="submit"
            className="primary"
            disabled={busy}
            data-testid="agent-runtime-save"
          >
            Save
          </button>
        </div>
      </form>
    </section>
  );
}

function AgentRuntimePanel() {
  const [draft, setDraft] = useState<AgentRuntimeConfig>(emptyAgentRuntime);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    return api
      .getAgentRuntime()
      .then((rt) => {
        setDraft({
          ...emptyAgentRuntime(),
          ...rt,
          vertex: { ...emptyAgentRuntime().vertex, ...(rt.vertex ?? {}) },
          providers: rt.providers ?? [],
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
      <section aria-labelledby="agent-runtime-title" data-testid="agent-runtime-panel">
        <h2 id="agent-runtime-title">Agent runtime</h2>
        <p className="dim">loading…</p>
      </section>
    );
  }

  return (
    <AgentRuntimePanelView
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
        api
          .putAgentRuntime(draft)
          .then((saved) => {
            setDraft({
              ...emptyAgentRuntime(),
              ...saved,
              vertex: { ...emptyAgentRuntime().vertex, ...(saved.vertex ?? {}) },
              providers: saved.providers ?? [],
            });
            setSavedHint(
              "Saved. Next sandbox create / agent_env use these providers, Vertex, and budgets.",
            );
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}

/** Presentational OpenShell panel — exported for UI tests without fetch. */
export function OpenShellPanelView({
  status,
  binaryPath,
  busy,
  error,
  savedHint,
  onBinaryPathChange,
  onRefresh,
  onSaveBinary,
}: {
  status: OpenShellStatus | null;
  binaryPath: string;
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onBinaryPathChange: (next: string) => void;
  onRefresh: () => void;
  onSaveBinary: () => void;
}) {
  const healthLabel = !status
    ? "…"
    : status.cli_missing
      ? "CLI missing"
      : status.healthy
        ? "Healthy"
        : "Unhealthy";
  const healthClass = !status
    ? "dim"
    : status.cli_missing
      ? "openshell-health-missing"
      : status.healthy
        ? "openshell-health-ok"
        : "openshell-health-bad";

  return (
    <section aria-labelledby="openshell-title" data-testid="openshell-panel">
      <h2 id="openshell-title">OpenShell</h2>
      <p className="dim">
        Gateway connectivity on this host. Dispatch will not claim cards while
        the gateway is unhealthy. Compute driver, providers, and image stay
        host/ops setup — see the operating docs.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="openshell-saved-hint">
          {savedHint}
        </p>
      )}

      <div className="openshell-health" data-testid="openshell-health">
        <div className="openshell-health-row">
          <span className="dim">Gateway</span>
          <strong
            className={healthClass}
            data-testid="openshell-health-label"
            data-healthy={status?.healthy ? "true" : "false"}
            data-cli-missing={status?.cli_missing ? "true" : "false"}
          >
            {healthLabel}
          </strong>
        </div>
        {status && (
          <>
            <p className="dim openshell-health-bin" data-testid="openshell-health-binary">
              Binary: <code>{status.binary}</code>
            </p>
            <pre className="openshell-health-summary" data-testid="openshell-health-summary">
              {status.summary}
            </pre>
          </>
        )}
        <div className="btns">
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={onRefresh}
            data-testid="openshell-refresh"
          >
            Refresh status
          </button>
        </div>
      </div>

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="openshell-binary-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSaveBinary();
        }}
      >
        <label>
          Binary path (optional)
          <input
            className="search-input"
            value={binaryPath}
            disabled={busy}
            placeholder="openshell — leave empty to use PATH"
            onChange={(e) => onBinaryPathChange(e.target.value)}
            data-testid="openshell-field-binary"
          />
          <span className="dim sandbox-field-hint">
            Override only when the CLI is not on PATH. Host Docker / Colima /
            podman and <code>DOCKER_HOST</code> stay outside honr — configure
            them for the OpenShell gateway process, not here.
          </span>
        </label>
        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="openshell-save-binary">
            Save binary path
          </button>
        </div>
      </form>

      <aside className="workspace-webhook-hint" data-testid="openshell-ops-hint">
        <h3>Host setup</h3>
        <p className="dim">
          Role checklist: compute driver → OpenShell gateway → providers →
          sandbox image. Details in <code>docs/operating.md</code> (Running real
          agents) and <code>docs/sandbox-stack.md</code>.
        </p>
      </aside>
    </section>
  );
}

function OpenShellPanel() {
  const [status, setStatus] = useState<OpenShellStatus | null>(null);
  const [binaryPath, setBinaryPath] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setBusy(true);
    return Promise.all([api.getOpenShellStatus(), api.getOpenShell()])
      .then(([st, cfg]: [OpenShellStatus, OpenShellSettings]) => {
        setStatus(st);
        setBinaryPath(cfg.binary_path ?? st.binary_path ?? "");
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  if (loading && !error && !status) {
    return (
      <section aria-labelledby="openshell-title" data-testid="openshell-panel">
        <h2 id="openshell-title">OpenShell</h2>
        <p className="dim">loading…</p>
      </section>
    );
  }

  return (
    <OpenShellPanelView
      status={status}
      binaryPath={binaryPath}
      busy={busy}
      error={error}
      savedHint={savedHint}
      onBinaryPathChange={(next) => {
        setSavedHint(null);
        setBinaryPath(next);
      }}
      onRefresh={() => {
        setSavedHint(null);
        refresh();
      }}
      onSaveBinary={() => {
        setBusy(true);
        setError(null);
        setSavedHint(null);
        const body: OpenShellSettings = {
          binary_path: binaryPath.trim() || null,
        };
        api
          .putOpenShell(body)
          .then((saved) => {
            setBinaryPath(saved.binary_path ?? "");
            setSavedHint("Saved. Status and dispatch health checks use this binary.");
            return api.getOpenShellStatus();
          })
          .then((st) => setStatus(st))
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}
