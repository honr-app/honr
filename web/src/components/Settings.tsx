import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  AgentRuntimeConfig,
  AuthSettings,
  GitHubAppSettings,
  WebhookPollConfig,
  WorkspaceBinding,
} from "../types.js";
import { OpenShellPanel } from "./OpenShellSettings.js";

export { OpenShellPanelView } from "./OpenShellSettings.js";
export { OpenShellProvidersPanelView } from "./OpenShellProviders.js";
export {
  ProjectSandboxPicker,
  SandboxesPanelView,
} from "./OpenShellProfiles.js";

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

const emptyWorkspace = (): WorkspaceBinding => ({
  forge: "github",
});

const emptyWebhookPoll = (): WebhookPollConfig => ({
  enabled: false,
  interval_secs: 60,
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
          Control-plane preferences. Forge holds provider and webhook poll. Each
          card’s <code>pull_request</code> (after report) holds work remotes.
          OpenShell holds Connectivity, Providers, and Profiles (providers
          attach per profile). GitHub App holds sealed App credentials for
          installation tokens. Agent runtime holds concurrency, timeouts, and
          the fallback engine.
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
            Installation that mints sandbox <code>GH_TOKEN</code> into the
            OpenShell <code>github</code> provider.
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
        put(
          body,
          "Saved. Sealed credentials and synced installation token to OpenShell when ready.",
        );
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
  poll,
  busy,
  error,
  savedHint,
  onDraftChange,
  onPollChange,
  onSave,
}: {
  draft: WorkspaceBinding;
  poll: WebhookPollConfig;
  busy?: boolean;
  error?: string | null;
  savedHint?: string | null;
  onDraftChange: (next: WorkspaceBinding) => void;
  onPollChange: (next: WebhookPollConfig) => void;
  onSave: () => void;
}) {
  return (
    <section aria-labelledby="workspace-title" data-testid="workspace-panel">
      <h2 id="workspace-title">Forge</h2>
      <p className="dim">
        Forge provider and webhook poll. Work remotes live on each card’s{" "}
        <code>pull_request</code> (url / base / head) after the agent reports.
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
        <fieldset className="workspace-poll-fieldset" data-testid="workspace-poll">
          <legend>Webhook polling fallback</legend>
          <label className="workspace-poll-enabled">
            <input
              type="checkbox"
              checked={poll.enabled}
              disabled={busy}
              onChange={(e) =>
                onPollChange({ ...poll, enabled: e.target.checked })
              }
              data-testid="workspace-poll-enabled"
            />
            Poll GitHub on an interval (in addition to webhooks)
          </label>
          <label>
            Interval (seconds)
            <input
              className="search-input"
              type="number"
              min={15}
              step={1}
              value={poll.interval_secs}
              disabled={busy || !poll.enabled}
              onChange={(e) =>
                onPollChange({
                  ...poll,
                  interval_secs: Number(e.target.value) || 60,
                })
              }
              data-testid="workspace-poll-interval"
            />
            <span className="dim sandbox-field-hint">
              Minimum 15s. Uses the GitHub App installation token. Completes
              merged Review cards and advances main when the tip moves.
            </span>
          </label>
        </fieldset>

        <div className="btns">
          <button type="submit" className="primary" disabled={busy} data-testid="workspace-save">
            Save
          </button>
        </div>
      </form>
    </section>
  );
}

function WorkspacePanel() {
  const [draft, setDraft] = useState<WorkspaceBinding>(emptyWorkspace);
  const [poll, setPoll] = useState<WebhookPollConfig>(emptyWebhookPoll);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedHint, setSavedHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    return Promise.all([api.getWorkspace(), api.getWebhookPoll()])
      .then(([ws, wp]) => {
        setDraft({
          forge: ws.forge || "github",
        });
        setPoll({
          enabled: !!wp.enabled,
          interval_secs: wp.interval_secs || 60,
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
      poll={poll}
      busy={busy}
      error={error}
      savedHint={savedHint}
      onDraftChange={(next) => {
        setSavedHint(null);
        setDraft(next);
      }}
      onPollChange={(next) => {
        setSavedHint(null);
        setPoll(next);
      }}
      onSave={() => {
        setBusy(true);
        setError(null);
        setSavedHint(null);
        const body: WorkspaceBinding = {
          forge: draft.forge.trim() || "github",
        };
        const pollBody: WebhookPollConfig = {
          enabled: poll.enabled,
          interval_secs: Math.max(15, Number(poll.interval_secs) || 60),
        };
        Promise.all([api.putWorkspace(body), api.putWebhookPoll(pollBody)])
          .then(([saved, savedPoll]) => {
            setDraft({
              forge: saved.forge,
            });
            setPoll({
              enabled: !!savedPoll.enabled,
              interval_secs: savedPoll.interval_secs || 60,
            });
            setSavedHint("Saved. Forge and poll settings update board state.");
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
