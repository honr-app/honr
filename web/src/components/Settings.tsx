import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api } from "../api.js";
import type {
  AgentRuntimeConfig,
  AuthSettings,
  GitHubAppSettings,
  OpenShellProviderView,
  OpenShellProviderWrite,
  OpenShellSettings,
  OpenShellStatus,
  ProviderTypeProfile,
  SandboxProfile,
  WorkspaceBinding,
} from "../types.js";

type SettingsSection =
  | "workspace"
  | "agent-runtime"
  | "openshell"
  | "github-app"
  | "access";

const SECTIONS: { id: SettingsSection; label: string; stub?: boolean }[] = [
  { id: "openshell", label: "OpenShell" },
  { id: "github-app", label: "GitHub App" },
  { id: "access", label: "Access" },
  // Nav label is Forge — "Workspace" implied a single work repo (upstream/fork).
  { id: "workspace", label: "Forge" },
  { id: "agent-runtime", label: "Agent runtime" },
];

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

const emptyWorkspace = (): WorkspaceBinding => ({
  forge: "github",
  beads_sync_repo: "",
});

const emptyAgentRuntime = (): AgentRuntimeConfig => ({
  enabled: false,
  engine: "cursor",
  max_concurrent: 2,
  agent_timeout_secs: 1800,
  max_attempts: 3,
  branch_prefix: "honr",
});

/**
 * Settings shell — OpenShell, GitHub App, Forge, Agent runtime.
 */
export function Settings() {
  const [section, setSection] = useState<SettingsSection>("openshell");

  return (
    <div className="settings" data-testid="settings">
      <header className="settings-hero">
        <h1>Settings</h1>
        <p className="settings-lede">
          Control-plane preferences. Forge holds Issue sync — not a work repo.
          Each card’s <code>pull_request</code> (after report) holds remotes.
          OpenShell holds gateway connectivity, providers, and sandbox profiles
          (including which agent engine a profile runs). GitHub App holds the
          sealed App credentials for installation tokens. Agent runtime holds
          concurrency, timeouts, and the fallback engine.
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
          {section === "openshell" ? (
            <OpenShellPanel />
          ) : section === "github-app" ? (
            <GitHubAppPanel />
          ) : section === "access" ? (
            <AccessPanel />
          ) : section === "workspace" ? (
            <WorkspacePanel />
          ) : (
            <AgentRuntimePanel />
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
    <div className="openshell-profiles" data-testid="openshell-profiles">
    <section aria-labelledby="openshell-profiles-title" data-testid="sandboxes-panel">
      <h3 id="openshell-profiles-title">Profiles</h3>
      <p className="dim">
        Named create-specs (image, policy, CPU, memory). The global default is
        used when a Project has no override. Live card environments are managed
        on the board, not here.
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
    </div>
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

/** Presentational Access form — local admin allowlists + password. */
export function AccessPanelView({
  adminUsername,
  allowedUsers,
  allowedTeams,
  newPassword,
  githubLoginEnabled,
  hasClientSecret,
  busy,
  error,
  savedHint,
  onAllowedUsersChange,
  onAllowedTeamsChange,
  onNewPasswordChange,
  onSave,
}: {
  adminUsername: string;
  allowedUsers: string;
  allowedTeams: string;
  newPassword: string;
  githubLoginEnabled: boolean;
  hasClientSecret: boolean;
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onAllowedUsersChange: (next: string) => void;
  onAllowedTeamsChange: (next: string) => void;
  onNewPasswordChange: (next: string) => void;
  onSave: () => void;
}) {
  return (
    <section aria-labelledby="access-title" data-testid="access-panel">
      <h2 id="access-title">Access</h2>
      <p className="dim">
        Local admin <strong>{adminUsername || "…"}</strong> can always sign in.
        GitHub sign-in is limited to the users and org teams below. Any signed-in
        operator can edit this for now.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="access-saved-hint">
          {savedHint}
        </p>
      )}

      <div className="openshell-health" data-testid="access-github-status">
        <div className="openshell-health-row">
          <span className="dim">GitHub login</span>
          <strong>
            {githubLoginEnabled
              ? "Enabled"
              : hasClientSecret
                ? "Incomplete App config"
                : "Needs Client secret (GitHub App)"}
          </strong>
        </div>
      </div>

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="access-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label>
          Allowed GitHub users
          <textarea
            className="search-input"
            rows={3}
            value={allowedUsers}
            disabled={busy}
            placeholder="one login per line, e.g. shanemcd"
            onChange={(e) => onAllowedUsersChange(e.target.value)}
            data-testid="access-field-users"
          />
        </label>
        <label>
          Allowed org teams
          <textarea
            className="search-input"
            rows={3}
            value={allowedTeams}
            disabled={busy}
            placeholder="one org/team_slug per line"
            onChange={(e) => onAllowedTeamsChange(e.target.value)}
            data-testid="access-field-teams"
          />
        </label>
        <label>
          New admin password
          <input
            className="search-input"
            type="password"
            autoComplete="new-password"
            value={newPassword}
            disabled={busy}
            placeholder="leave blank to keep current"
            onChange={(e) => onNewPasswordChange(e.target.value)}
            data-testid="access-field-password"
          />
        </label>
        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="access-save">
            Save
          </button>
        </div>
      </form>
    </section>
  );
}

function AccessPanel() {
  const [adminUsername, setAdminUsername] = useState("");
  const [allowedUsers, setAllowedUsers] = useState("");
  const [allowedTeams, setAllowedTeams] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [githubLoginEnabled, setGithubLoginEnabled] = useState(false);
  const [hasClientSecret, setHasClientSecret] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const apply = useCallback((cfg: AuthSettings) => {
    setAdminUsername(cfg.admin_username);
    setAllowedUsers(cfg.allowed_users.join("\n"));
    setAllowedTeams(cfg.allowed_teams.join("\n"));
    setGithubLoginEnabled(cfg.github_login_enabled);
    setHasClientSecret(cfg.has_client_secret);
    setNewPassword("");
  }, []);

  const refresh = useCallback(() => {
    setBusy(true);
    return api
      .getAuthSettings()
      .then((cfg) => {
        apply(cfg);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setLoading(false);
      });
  }, [apply]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <AccessPanelView
      adminUsername={adminUsername}
      allowedUsers={allowedUsers}
      allowedTeams={allowedTeams}
      newPassword={newPassword}
      githubLoginEnabled={githubLoginEnabled}
      hasClientSecret={hasClientSecret}
      busy={busy || loading}
      error={error}
      savedHint={savedHint}
      onAllowedUsersChange={(next) => {
        setSavedHint(null);
        setAllowedUsers(next);
      }}
      onAllowedTeamsChange={(next) => {
        setSavedHint(null);
        setAllowedTeams(next);
      }}
      onNewPasswordChange={(next) => {
        setSavedHint(null);
        setNewPassword(next);
      }}
      onSave={() => {
        setBusy(true);
        setError(null);
        setSavedHint(null);
        const users = allowedUsers
          .split(/[\n,]+/)
          .map((s) => s.trim())
          .filter(Boolean);
        const teams = allowedTeams
          .split(/[\n,]+/)
          .map((s) => s.trim())
          .filter(Boolean);
        const body: {
          allowed_users: string[];
          allowed_teams: string[];
          new_password?: string;
        } = { allowed_users: users, allowed_teams: teams };
        if (newPassword.trim()) body.new_password = newPassword.trim();
        api
          .putAuthSettings(body)
          .then((cfg) => {
            apply(cfg);
            setSavedHint("Saved access settings.");
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}

/** Presentational GitHub App form — exported for UI tests without fetch. */
export function GitHubAppPanelView({
  appId,
  clientId,
  privateKeyPem,
  webhookSecret,
  clientSecret,
  installationId,
  installations,
  tokenStatus,
  status,
  busy,
  error,
  savedHint,
  onAppIdChange,
  onClientIdChange,
  onPrivateKeyPemChange,
  onWebhookSecretChange,
  onClientSecretChange,
  onInstallationIdChange,
  onSave,
  onClear,
  onSyncToken,
}: {
  appId: string;
  clientId: string;
  privateKeyPem: string;
  webhookSecret: string;
  clientSecret: string;
  installationId: string;
  installations: NonNullable<GitHubAppSettings["installations"]>;
  tokenStatus?: GitHubAppSettings["token_status"];
  status?: GitHubAppSettings["status"];
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onAppIdChange: (next: string) => void;
  onClientIdChange: (next: string) => void;
  onPrivateKeyPemChange: (next: string) => void;
  onWebhookSecretChange: (next: string) => void;
  onClientSecretChange: (next: string) => void;
  onInstallationIdChange: (next: string) => void;
  onSave: () => void;
  onClear: () => void;
  onSyncToken: () => void;
}) {
  const statusLabel = status?.complete
    ? "Configured (encrypted in board DB)"
    : status?.app_id || status?.private_key
      ? "Incomplete"
      : "Not configured";
  const tokenLabel = !tokenStatus?.configured
    ? "Pick an installation, then Mint / sync"
    : tokenStatus.error
      ? `Error: ${tokenStatus.error}`
      : tokenStatus.expires_at
        ? `OK until ${tokenStatus.expires_at}`
        : tokenStatus.provider_attached
          ? "Provider attached"
          : "Configured — sync to gateway";

  return (
    <section aria-labelledby="github-app-title" data-testid="github-app-panel">
      <h2 id="github-app-title">GitHub App</h2>
      <p className="dim">
        Sealed App credentials mint short-lived{" "}
        <strong>installation tokens</strong> into the OpenShell{" "}
        <code>github</code> provider (no PAT paste). Also used for Sign in with
        GitHub. Private key / secrets stay in the board DB under{" "}
        <code>~/.config/honr/master.key</code>.
      </p>

      {error && <div className="err">{error}</div>}
      {savedHint && (
        <p className="dim" data-testid="github-app-saved-hint">
          {savedHint}
        </p>
      )}

      <div className="openshell-health" data-testid="github-app-status">
        <div className="openshell-health-row">
          <span className="dim">Credentials</span>
          <strong data-testid="github-app-status-label">{statusLabel}</strong>
        </div>
        <div className="openshell-health-row">
          <span className="dim">Sandbox token</span>
          <strong data-testid="github-app-token-label">{tokenLabel}</strong>
        </div>
        {status?.complete && (
          <div className="openshell-health-row">
            <span className="dim">Webhook secret</span>
            <strong data-testid="github-app-webhook-flag">
              {status.webhook_secret ? "Set" : "Not set"}
            </strong>
          </div>
        )}
      </div>

      <form
        className="sandbox-profile-form workspace-form"
        data-testid="github-app-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label>
          App ID
          <input
            className="search-input"
            value={appId}
            disabled={busy}
            placeholder="123456"
            onChange={(e) => onAppIdChange(e.target.value)}
            data-testid="github-app-field-app-id"
          />
        </label>
        <label>
          Private key (PEM)
          <textarea
            className="search-input"
            rows={6}
            value={privateKeyPem}
            disabled={busy}
            placeholder={
              status?.private_key
                ? "Configured — paste to replace"
                : "-----BEGIN RSA PRIVATE KEY-----"
            }
            onChange={(e) => onPrivateKeyPemChange(e.target.value)}
            data-testid="github-app-field-private-key"
          />
        </label>
        <label>
          Installation (sandbox git)
          <select
            className="search-input"
            value={installationId}
            disabled={busy}
            onChange={(e) => onInstallationIdChange(e.target.value)}
            data-testid="github-app-field-installation"
          >
            <option value="">Select installation…</option>
            {installationId &&
              !installations.some((i) => String(i.id) === installationId) && (
                <option value={installationId}>
                  Installation {installationId}
                </option>
              )}
            {installations.map((inst) => (
              <option key={inst.id} value={String(inst.id)}>
                {inst.account_login || "unknown"} ({inst.id})
              </option>
            ))}
          </select>
          <span className="dim sandbox-field-hint">
            Prefer the fork account install (e.g. clankrshq). Honr mints tokens
            into OpenShell provider <code>github</code> automatically.
          </span>
        </label>
        <label>
          Webhook secret
          <input
            className="search-input"
            type="password"
            autoComplete="off"
            value={webhookSecret}
            disabled={busy}
            placeholder={
              status?.webhook_secret ? "Configured — paste to replace" : "optional"
            }
            onChange={(e) => onWebhookSecretChange(e.target.value)}
            data-testid="github-app-field-webhook-secret"
          />
        </label>
        <label>
          Client ID
          <input
            className="search-input"
            value={clientId}
            disabled={busy}
            placeholder="Iv1.… (optional, for user OAuth)"
            onChange={(e) => onClientIdChange(e.target.value)}
            data-testid="github-app-field-client-id"
          />
        </label>
        <label>
          Client secret
          <input
            className="search-input"
            type="password"
            autoComplete="off"
            value={clientSecret}
            disabled={busy}
            placeholder={
              status?.client_secret
                ? "Configured — paste to replace"
                : "optional"
            }
            onChange={(e) => onClientSecretChange(e.target.value)}
            data-testid="github-app-field-client-secret"
          />
        </label>
        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="github-app-save">
            Save
          </button>
          <button
            type="button"
            disabled={busy || !status?.complete || !installationId}
            onClick={onSyncToken}
            data-testid="github-app-sync-token"
          >
            Mint / sync token
          </button>
          <button
            type="button"
            disabled={busy || !status?.complete}
            onClick={onClear}
            data-testid="github-app-clear"
          >
            Clear
          </button>
        </div>
      </form>
    </section>
  );
}

function GitHubAppPanel() {
  const [appId, setAppId] = useState("");
  const [clientId, setClientId] = useState("");
  const [privateKeyPem, setPrivateKeyPem] = useState("");
  const [webhookSecret, setWebhookSecret] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [installationId, setInstallationId] = useState("");
  const [installations, setInstallations] = useState<
    NonNullable<GitHubAppSettings["installations"]>
  >([]);
  const [tokenStatus, setTokenStatus] =
    useState<GitHubAppSettings["token_status"]>();
  const [status, setStatus] = useState<GitHubAppSettings["status"]>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const applySaved = useCallback((cfg: GitHubAppSettings) => {
    setAppId(cfg.app_id ?? "");
    setClientId(cfg.client_id ?? "");
    setStatus(cfg.status);
    setInstallationId(
      cfg.installation_id != null ? String(cfg.installation_id) : "",
    );
    setInstallations(cfg.installations ?? []);
    setTokenStatus(cfg.token_status);
    setPrivateKeyPem("");
    setWebhookSecret("");
    setClientSecret("");
  }, []);

  const refresh = useCallback(() => {
    setBusy(true);
    return api
      .getGitHubApp()
      .then((cfg) => {
        applySaved(cfg);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setLoading(false);
      });
  }, [applySaved]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const put = (body: GitHubAppSettings, hint: string) => {
    setBusy(true);
    setError(null);
    setSavedHint(null);
    api
      .putGitHubApp(body)
      .then((saved) => {
        applySaved(saved);
        setSavedHint(hint);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <GitHubAppPanelView
      appId={appId}
      clientId={clientId}
      privateKeyPem={privateKeyPem}
      webhookSecret={webhookSecret}
      clientSecret={clientSecret}
      installationId={installationId}
      installations={installations}
      tokenStatus={tokenStatus}
      status={status}
      busy={busy || loading}
      error={error}
      savedHint={savedHint}
      onAppIdChange={(next) => {
        setSavedHint(null);
        setAppId(next);
      }}
      onClientIdChange={(next) => {
        setSavedHint(null);
        setClientId(next);
      }}
      onPrivateKeyPemChange={(next) => {
        setSavedHint(null);
        setPrivateKeyPem(next);
      }}
      onWebhookSecretChange={(next) => {
        setSavedHint(null);
        setWebhookSecret(next);
      }}
      onClientSecretChange={(next) => {
        setSavedHint(null);
        setClientSecret(next);
      }}
      onInstallationIdChange={(next) => {
        setSavedHint(null);
        setInstallationId(next);
      }}
      onSave={() => {
        const body: GitHubAppSettings = {
          app_id: appId.trim() || null,
          // Empty string clears; omit would leave the sealed value unchanged.
          client_id: clientId.trim(),
        };
        if (privateKeyPem.trim()) body.private_key_pem = privateKeyPem;
        if (webhookSecret.trim()) body.webhook_secret = webhookSecret;
        if (clientSecret.trim()) body.client_secret = clientSecret;
        if (installationId.trim()) {
          body.installation_id = Number(installationId);
        } else {
          body.clear_installation_id = true;
        }
        put(body, "Saved. GitHub App credentials are sealed in the board database.");
      }}
      onSyncToken={() => {
        setBusy(true);
        setError(null);
        setSavedHint(null);
        const saveFirst: GitHubAppSettings = {};
        if (installationId.trim()) {
          saveFirst.installation_id = Number(installationId);
        }
        const chain = installationId.trim()
          ? api.putGitHubApp(saveFirst).then(() => api.syncGitHubAppToken())
          : api.syncGitHubAppToken();
        chain
          .then((saved) => {
            applySaved(saved);
            setSavedHint("Installation token minted and synced to OpenShell provider github.");
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onClear={() => {
        put({ clear: true }, "Cleared sealed GitHub App credentials.");
      }}
    />
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
  return (
    <section aria-labelledby="agent-runtime-title" data-testid="agent-runtime-panel">
      <h2 id="agent-runtime-title">Agent runtime</h2>
      <p className="dim">
        Process knobs for OpenShell sandboxes: branch prefix, concurrency,
        timeouts, and the fallback agent engine when a profile omits one.
        Seeded from <code>honr.yaml</code>; edits persist on the Board. Per-run
        engine lives on OpenShell → Profiles.
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

        <label>
          Branch prefix
          <input
            className="search-input"
            value={draft.branch_prefix}
            disabled={busy}
            placeholder="honr"
            onChange={(e) => onDraftChange({ ...draft, branch_prefix: e.target.value })}
            data-testid="agent-runtime-field-branch-prefix"
          />
          <span className="dim sandbox-field-hint">
            Branches are <code>{"{prefix}/card-{id}"}</code>; sandboxes{" "}
            <code>{"{prefix}-card-{id}-a{n}"}</code>. Default <code>honr</code>.
          </span>
        </label>

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
          branch_prefix: rt.branch_prefix || "honr",
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
              branch_prefix: saved.branch_prefix || "honr",
            });
            setSavedHint("Saved. Next runs use this engine, prefix, and timeouts.");
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
  gatewayEndpoint,
  caPem,
  clientCertPem,
  clientKeyPem,
  mtls,
  busy,
  error,
  savedHint,
  onGatewayEndpointChange,
  onCaPemChange,
  onClientCertPemChange,
  onClientKeyPemChange,
  onRefresh,
  onSave,
  onImportCliMtls,
  onClearMtls,
  providers,
  profiles,
}: {
  status: OpenShellStatus | null;
  gatewayEndpoint: string;
  caPem: string;
  clientCertPem: string;
  clientKeyPem: string;
  mtls?: OpenShellSettings["mtls"];
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onGatewayEndpointChange: (next: string) => void;
  onCaPemChange: (next: string) => void;
  onClientCertPemChange: (next: string) => void;
  onClientKeyPemChange: (next: string) => void;
  onRefresh: () => void;
  onSave: () => void;
  onImportCliMtls: () => void;
  onClearMtls: () => void;
  /** Optional providers band (live panel passes a mounted subview). */
  providers?: ReactNode;
  /** Optional profiles band (live panel passes SandboxesPanel). */
  profiles?: ReactNode;
}) {
  const healthLabel = !status
    ? "…"
    : status.healthy
      ? "Healthy"
      : "Unhealthy";
  const healthClass = !status
    ? "dim"
    : status.healthy
      ? "openshell-health-ok"
      : "openshell-health-bad";
  const mtlsLabel = mtls?.complete
    ? "Configured (encrypted in board DB)"
    : mtls?.ca || mtls?.client_cert || mtls?.client_key
      ? "Incomplete"
      : "Not configured";

  return (
    <section aria-labelledby="openshell-title" data-testid="openshell-panel">
      <h2 id="openshell-title">OpenShell</h2>
      <p className="dim">
        Gateway connectivity, providers, and sandbox profiles. Paste endpoint +
        certs (or import from the local OpenShell config dir). PEMs are sealed
        into the board database with a host master key (
        <code>~/.config/honr/master.key</code>); the API never returns private
        key material. Host Docker / Colima stay outside honr.
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
          >
            {healthLabel}
          </strong>
        </div>
        <div className="openshell-health-row">
          <span className="dim">mTLS material</span>
          <strong data-testid="openshell-mtls-label">{mtlsLabel}</strong>
        </div>
        {status?.summary && (
          <pre className="openshell-health-summary" data-testid="openshell-health-summary">
            {status.summary}
          </pre>
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
        data-testid="openshell-gateway-form"
        onSubmit={(e) => {
          e.preventDefault();
          onSave();
        }}
      >
        <label>
          Gateway endpoint
          <input
            className="search-input"
            value={gatewayEndpoint}
            disabled={busy}
            placeholder="https://127.0.0.1:17670"
            onChange={(e) => onGatewayEndpointChange(e.target.value)}
            data-testid="openshell-field-endpoint"
          />
        </label>
        <label>
          CA certificate (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={caPem}
            disabled={busy}
            placeholder={
              mtls?.ca
                ? "Configured — paste to replace"
                : "-----BEGIN CERTIFICATE-----"
            }
            onChange={(e) => onCaPemChange(e.target.value)}
            data-testid="openshell-field-ca"
          />
        </label>
        <label>
          Client certificate (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={clientCertPem}
            disabled={busy}
            placeholder={
              mtls?.client_cert
                ? "Configured — paste to replace"
                : "-----BEGIN CERTIFICATE-----"
            }
            onChange={(e) => onClientCertPemChange(e.target.value)}
            data-testid="openshell-field-client-cert"
          />
        </label>
        <label>
          Client private key (PEM)
          <textarea
            className="search-input"
            rows={4}
            value={clientKeyPem}
            disabled={busy}
            placeholder={
              mtls?.client_key
                ? "Configured — paste to replace"
                : "-----BEGIN PRIVATE KEY-----"
            }
            onChange={(e) => onClientKeyPemChange(e.target.value)}
            data-testid="openshell-field-client-key"
          />
        </label>
        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="openshell-save">
            Save
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={onImportCliMtls}
            data-testid="openshell-import-cli-mtls"
          >
            Import from local config
          </button>
          <button
            type="button"
            disabled={busy || !mtls?.complete}
            onClick={onClearMtls}
            data-testid="openshell-clear-mtls"
          >
            Clear mTLS
          </button>
        </div>
      </form>

      {providers}

      {profiles}

      <aside className="workspace-webhook-hint" data-testid="openshell-ops-hint">
        <h3>Host setup</h3>
        <p className="dim">
          Role checklist: compute driver → gateway (mTLS) → providers → profiles
          (image/policy). Details in <code>docs/agents.md</code> and{" "}
          <code>docs/sandbox.md</code>.
        </p>
      </aside>
    </section>
  );
}

/** Fixed OpenShell provider name filled by Settings → GitHub App (not a PAT). */
const GITHUB_APP_PROVIDER_NAME = "github";

function isGitHubAppManagedProvider(name: string, type?: string): boolean {
  return name.trim() === GITHUB_APP_PROVIDER_NAME && (type == null || type === "github");
}

/** Presentational providers band — exported for UI tests without fetch. */
export function OpenShellProvidersPanelView({
  providers,
  gatewayReachable,
  profiles,
  busy,
  error,
  hint,
  draft,
  onDraftChange,
  onSave,
  onCancelEdit,
  onEdit,
  onDelete,
  onSync,
  onToggleAttach,
}: {
  providers: OpenShellProviderView[];
  gatewayReachable: boolean;
  profiles: ProviderTypeProfile[];
  busy?: boolean;
  error?: string | null;
  hint?: string | null;
  draft: OpenShellProviderWrite | null;
  onDraftChange: (next: OpenShellProviderWrite | null) => void;
  onSave: () => void;
  onCancelEdit: () => void;
  onEdit: (p: OpenShellProviderView) => void;
  onDelete: (name: string) => void;
  onSync: () => void;
  onToggleAttach: (p: OpenShellProviderView, attach: boolean) => void;
}) {
  const typeOptions = profiles.length
    ? profiles.map((p) => p.id)
    : ["github", "google-vertex-ai", "cursor", "claude"];
  // Prefer a non-App type for "Add" — github/GH_TOKEN is owned by GitHub App.
  const defaultAddType =
    typeOptions.find((t) => t !== "github") ?? typeOptions[0] ?? "google-vertex-ai";
  const selectedProfile = draft
    ? profiles.find((p) => p.id === draft.type)
    : undefined;
  const draftManaged = draft
    ? isGitHubAppManagedProvider(draft.name, draft.type)
    : false;
  // App-managed github is always GH_TOKEN only (never GITHUB_TOKEN).
  const credKeys = draftManaged
    ? ["GH_TOKEN"]
    : selectedProfile?.credential_env_vars?.length
      ? selectedProfile.credential_env_vars
      : draft?.type === "github"
        ? ["GH_TOKEN"]
        : [];

  return (
    <div className="openshell-providers" data-testid="openshell-providers">
      <div className="openshell-providers-head">
        <h3 id="openshell-providers-title">Providers</h3>
        <p className="dim">
          Desired providers live on the board (credentials sealed). Save applies
          to the gateway when it is reachable; Sync recreates after a wipe.
          Provider <code>github</code> attaches <code>GH_TOKEN</code> from
          Settings → GitHub App (installation token), not a pasted PAT.
        </p>
      </div>

      {error && <div className="err">{error}</div>}
      {hint && (
        <p className="dim" data-testid="openshell-providers-hint">
          {hint}
        </p>
      )}

      <div className="btns" style={{ marginBottom: 12 }}>
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={() =>
            onDraftChange({
              name: "",
              type: defaultAddType,
              config: {},
              credentials: {},
              attach_to_sandboxes: true,
            })
          }
          data-testid="openshell-providers-add"
        >
          Add provider
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={onSync}
          data-testid="openshell-providers-sync"
        >
          Sync all to gateway
        </button>
        <span className="dim" data-testid="openshell-providers-gateway-badge">
          {gatewayReachable ? "gateway reachable" : "gateway offline — local only"}
        </span>
      </div>

      {providers.length === 0 && !draft ? (
        <p className="dim" data-testid="openshell-providers-empty">
          No providers yet. Add one to attach credentials to sandboxes.
        </p>
      ) : (
        <ul className="openshell-provider-list" data-testid="openshell-provider-list">
          {providers.map((p) => {
            const managed = isGitHubAppManagedProvider(p.name, p.type);
            const secretKeys =
              (p.credential_keys ?? []).length > 0
                ? (p.credential_keys ?? [])
                : managed
                  ? ["GH_TOKEN"]
                  : [];
            return (
            <li key={p.name} className="openshell-provider-row" data-testid={`openshell-provider-${p.name}`}>
              <div className="openshell-provider-main">
                <strong>{p.name}</strong>
                <span className="dim">{p.type}</span>
                <span
                  className={
                    p.gateway_synced === true
                      ? "openshell-health-ok"
                      : p.gateway_synced === false
                        ? "openshell-health-bad"
                        : "dim"
                  }
                  data-testid={`openshell-provider-sync-${p.name}`}
                >
                  {p.gateway_synced === true
                    ? "on gateway"
                    : p.gateway_synced === false
                      ? "not on gateway"
                      : "sync unknown"}
                </span>
              </div>
              <div
                className="openshell-provider-meta dim"
                data-testid={
                  managed ? `openshell-provider-managed-${p.name}` : undefined
                }
              >
                {p.has_credentials || p.has_refresh || managed
                  ? `secrets: ${secretKeys.join(", ") || "refresh"}${
                      managed ? " · GitHub App" : ""
                    }`
                  : "no secrets"}
              </div>
              <label className="agent-runtime-check">
                <input
                  type="checkbox"
                  checked={p.attach_to_sandboxes}
                  disabled={busy}
                  onChange={(e) => onToggleAttach(p, e.target.checked)}
                  data-testid={`openshell-provider-attach-${p.name}`}
                />
                Attach to sandboxes
              </label>
              <div className="btns">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => onEdit(p)}
                  data-testid={`openshell-provider-edit-${p.name}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => onDelete(p.name)}
                  data-testid={`openshell-provider-delete-${p.name}`}
                >
                  Delete
                </button>
              </div>
            </li>
            );
          })}
        </ul>
      )}

      {draft && (
        <form
          className="sandbox-profile-form workspace-form openshell-provider-form"
          data-testid="openshell-provider-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSave();
          }}
        >
          <label>
            Name
            <input
              className="search-input"
              value={draft.name}
              disabled={busy || draftManaged}
              onChange={(e) => onDraftChange({ ...draft, name: e.target.value })}
              data-testid="openshell-provider-field-name"
            />
          </label>
          <label>
            Type
            <select
              className="search-input"
              value={draft.type}
              disabled={busy || draftManaged}
              onChange={(e) => onDraftChange({ ...draft, type: e.target.value })}
              data-testid="openshell-provider-field-type"
            >
              {typeOptions.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </label>
          {credKeys.map((key) => (
            <label key={key}>
              {key}
              <input
                className="search-input"
                type="password"
                autoComplete="off"
                placeholder={
                  draftManaged
                    ? "set by Settings → GitHub App (Mint / sync)"
                    : "write-only — leave blank to keep existing"
                }
                value={draft.credentials?.[key] ?? ""}
                disabled={busy || draftManaged}
                readOnly={draftManaged}
                onChange={(e) =>
                  onDraftChange({
                    ...draft,
                    credentials: { ...(draft.credentials ?? {}), [key]: e.target.value },
                  })
                }
                data-testid={`openshell-provider-cred-${key}`}
              />
              {draftManaged && (
                <span
                  className="dim sandbox-field-hint"
                  data-testid="openshell-provider-app-managed-note"
                >
                  Attached into sandboxes as <code>{key}</code>. Value comes from
                  the App installation token — mint under Settings → GitHub App.
                </span>
              )}
            </label>
          ))}
          {draft.type === "google-vertex-ai" && (
            <>
              <label>
                VERTEX_AI_PROJECT_ID
                <input
                  className="search-input"
                  value={draft.config?.VERTEX_AI_PROJECT_ID ?? ""}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({
                      ...draft,
                      config: { ...(draft.config ?? {}), VERTEX_AI_PROJECT_ID: e.target.value },
                    })
                  }
                  data-testid="openshell-provider-config-project"
                />
              </label>
              <label>
                VERTEX_AI_LOCATION
                <input
                  className="search-input"
                  value={draft.config?.VERTEX_AI_LOCATION ?? "global"}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({
                      ...draft,
                      config: { ...(draft.config ?? {}), VERTEX_AI_LOCATION: e.target.value },
                    })
                  }
                  data-testid="openshell-provider-config-location"
                />
              </label>
            </>
          )}
          <label className="agent-runtime-check">
            <input
              type="checkbox"
              checked={draft.attach_to_sandboxes ?? true}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({ ...draft, attach_to_sandboxes: e.target.checked })
              }
              data-testid="openshell-provider-field-attach"
            />
            Attach to sandboxes
          </label>
          <div className="btns">
            <button type="submit" className="primary" disabled={busy} data-testid="openshell-provider-save">
              Save provider
            </button>
            <button type="button" disabled={busy} onClick={onCancelEdit}>
              Cancel
            </button>
          </div>
        </form>
      )}
    </div>
  );
}

function OpenShellProvidersPanel({ gatewayHealthy }: { gatewayHealthy: boolean }) {
  const [providers, setProviders] = useState<OpenShellProviderView[]>([]);
  const [gatewayReachable, setGatewayReachable] = useState(gatewayHealthy);
  const [profiles, setProfiles] = useState<ProviderTypeProfile[]>([]);
  const [draft, setDraft] = useState<OpenShellProviderWrite | null>(null);
  const [editingName, setEditingName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    return api
      .listOpenShellProviders()
      .then((out) => {
        setProviders(out.providers);
        setGatewayReachable(out.gateway_reachable);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    api
      .listOpenShellProviderProfiles()
      .then(setProfiles)
      .catch(() => setProfiles([]));
  }, [refresh]);

  const stripEmptyCreds = (body: OpenShellProviderWrite): OpenShellProviderWrite => {
    const credentials = Object.fromEntries(
      Object.entries(body.credentials ?? {}).filter(([, v]) => v.trim().length > 0),
    );
    return {
      ...body,
      name: body.name.trim(),
      type: body.type.trim(),
      credentials: Object.keys(credentials).length ? credentials : null,
    };
  };

  return (
    <OpenShellProvidersPanelView
      providers={providers}
      gatewayReachable={gatewayHealthy || gatewayReachable}
      profiles={profiles}
      busy={busy}
      error={error}
      hint={hint}
      draft={draft}
      onDraftChange={setDraft}
      onCancelEdit={() => {
        setDraft(null);
        setEditingName(null);
      }}
      onEdit={(p) => {
        setEditingName(p.name);
        setDraft({
          name: p.name,
          type: p.type,
          config: { ...p.config },
          credentials: {},
          attach_to_sandboxes: p.attach_to_sandboxes,
        });
        setHint(null);
        setError(null);
      }}
      onSave={() => {
        if (!draft) return;
        const body = stripEmptyCreds(draft);
        if (!body.name) {
          setError("name is required");
          return;
        }
        setBusy(true);
        setError(null);
        setHint(null);
        const req = editingName
          ? api.updateOpenShellProvider(editingName, body)
          : api.createOpenShellProvider(body);
        req
          .then(() => {
            setDraft(null);
            setEditingName(null);
            setHint(
              gatewayReachable
                ? "Provider saved and applied to the gateway when possible."
                : "Provider saved locally. Sync when the gateway is up.",
            );
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onDelete={(name) => {
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .deleteOpenShellProvider(name)
          .then(() => {
            if (editingName === name) {
              setDraft(null);
              setEditingName(null);
            }
            setHint(`Deleted ${name}.`);
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onSync={() => {
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .syncOpenShellProviders()
          .then((out) => {
            const errBits = out.errors.map((e) => `${e.name}: ${e.error}`).join("; ");
            setHint(
              errBits
                ? `Synced ${out.applied.length}; errors: ${errBits}`
                : `Synced ${out.applied.length} provider(s) to the gateway.`,
            );
            if (out.errors.length) setError(errBits);
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onToggleAttach={(p, attach) => {
        setBusy(true);
        setError(null);
        api
          .updateOpenShellProvider(p.name, {
            name: p.name,
            type: p.type,
            config: p.config,
            credentials: null,
            attach_to_sandboxes: attach,
          })
          .then(() => refresh())
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}

function OpenShellPanel() {
  const [status, setStatus] = useState<OpenShellStatus | null>(null);
  const [gatewayEndpoint, setGatewayEndpoint] = useState("");
  const [caPem, setCaPem] = useState("");
  const [clientCertPem, setClientCertPem] = useState("");
  const [clientKeyPem, setClientKeyPem] = useState("");
  const [mtls, setMtls] = useState<OpenShellSettings["mtls"]>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const applySaved = useCallback((cfg: OpenShellSettings, st?: OpenShellStatus) => {
    setGatewayEndpoint(cfg.gateway_endpoint ?? st?.gateway_endpoint ?? "");
    setMtls(cfg.mtls ?? st?.mtls);
    setCaPem("");
    setClientCertPem("");
    setClientKeyPem("");
  }, []);

  const refresh = useCallback(() => {
    setBusy(true);
    return Promise.all([api.getOpenShellStatus(), api.getOpenShell()])
      .then(([st, cfg]: [OpenShellStatus, OpenShellSettings]) => {
        setStatus(st);
        applySaved(cfg, st);
        setError(null);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        setBusy(false);
        setLoading(false);
      });
  }, [applySaved]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const put = (body: OpenShellSettings, hint: string) => {
    setBusy(true);
    setError(null);
    setSavedHint(null);
    api
      .putOpenShell(body)
      .then((saved) => {
        applySaved(saved);
        setSavedHint(hint);
        return api.getOpenShellStatus();
      })
      .then((st) => setStatus(st))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <OpenShellPanelView
      status={status}
      gatewayEndpoint={gatewayEndpoint}
      caPem={caPem}
      clientCertPem={clientCertPem}
      clientKeyPem={clientKeyPem}
      mtls={mtls}
      busy={busy || loading}
      error={error}
      savedHint={savedHint}
      onGatewayEndpointChange={(next) => {
        setSavedHint(null);
        setGatewayEndpoint(next);
      }}
      onCaPemChange={(next) => {
        setSavedHint(null);
        setCaPem(next);
      }}
      onClientCertPemChange={(next) => {
        setSavedHint(null);
        setClientCertPem(next);
      }}
      onClientKeyPemChange={(next) => {
        setSavedHint(null);
        setClientKeyPem(next);
      }}
      onRefresh={() => {
        setSavedHint(null);
        refresh();
      }}
      onSave={() => {
        const body: OpenShellSettings = {
          gateway_endpoint: gatewayEndpoint.trim() || null,
        };
        if (caPem.trim()) body.ca_pem = caPem;
        if (clientCertPem.trim()) body.client_cert_pem = clientCertPem;
        if (clientKeyPem.trim()) body.client_key_pem = clientKeyPem;
        put(body, "Saved. mTLS PEMs are sealed in the board database.");
      }}
      onImportCliMtls={() => {
        put(
          {
            gateway_endpoint: gatewayEndpoint.trim() || null,
            import_openshell_cli_mtls: true,
          },
          "Imported mTLS from local OpenShell config and sealed it.",
        );
      }}
      onClearMtls={() => {
        put(
          {
            gateway_endpoint: gatewayEndpoint.trim() || null,
            clear_mtls: true,
          },
          "Cleared sealed mTLS material.",
        );
      }}
      providers={<OpenShellProvidersPanel gatewayHealthy={!!status?.healthy} />}
      profiles={<SandboxesPanel />}
    />
  );
}
