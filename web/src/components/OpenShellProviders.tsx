import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  GitHubAppInstallation,
  OpenShellProviderTypeEntry,
  OpenShellProviderView,
  OpenShellProviderWrite,
} from "../types.js";

/** Shipped App-minted provider instance / type id. */
export const GITHUB_APP_PROVIDER_NAME = "github-app";
export const GITHUB_APP_PROVIDER_TYPE = "github-app";
const CRED_PRIVATE_KEY = "GITHUB_APP_PRIVATE_KEY";
const CONFIG_INSTALLATION_ID = "GITHUB_INSTALLATION_ID";

/** Gateway builtin without board YAML — keep project/location fields. */
const VERTEX_FORM_KEYS = ["VERTEX_AI_PROJECT_ID", "VERTEX_AI_LOCATION"];

function isGitHubAppType(type?: string): boolean {
  return (type ?? "").trim() === GITHUB_APP_PROVIDER_TYPE;
}

function formConfigKeysForType(
  typeId: string,
  profile?: OpenShellProviderTypeEntry,
): string[] {
  if (profile?.form_config_keys?.length) return profile.form_config_keys;
  if (typeId === "google-vertex-ai") return VERTEX_FORM_KEYS;
  return [];
}

/** Credential fields shown in the form (mint-managed GH_TOKEN omitted for github-app). */
function formCredentialKeys(
  typeId: string,
  profile?: OpenShellProviderTypeEntry,
): string[] {
  if (isGitHubAppType(typeId)) {
    return [CRED_PRIVATE_KEY];
  }
  if (profile?.credential_env_vars?.length) {
    return profile.credential_env_vars;
  }
  if (typeId === "github") {
    return ["GH_TOKEN"];
  }
  return [];
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
  installations = [],
  onRefreshInstallations,
}: {
  providers: OpenShellProviderView[];
  gatewayReachable: boolean;
  profiles: OpenShellProviderTypeEntry[];
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
  installations?: GitHubAppInstallation[];
  onRefreshInstallations?: () => void;
}) {
  const typeOptions = profiles.length
    ? profiles.map((p) => ({ id: p.id, label: p.display_name || p.id }))
    : [
        { id: "github-app", label: "GitHub Application Access Token" },
        { id: "github", label: "github" },
        { id: "google-vertex-ai", label: "google-vertex-ai" },
        { id: "cursor-agent", label: "cursor-agent" },
        { id: "antigravity", label: "antigravity" },
      ];
  const defaultAddType =
    typeOptions.find((t) => t.id === "google-vertex-ai")?.id ??
    typeOptions.find((t) => t.id !== GITHUB_APP_PROVIDER_TYPE)?.id ??
    typeOptions[0]?.id ??
    "google-vertex-ai";
  const selectedProfile = draft
    ? profiles.find((p) => p.id === draft.type)
    : undefined;
  const draftIsGitHubApp = draft ? isGitHubAppType(draft.type) : false;
  const credKeys = draft
    ? formCredentialKeys(draft.type, selectedProfile)
    : [];
  const configKeys = draft
    ? formConfigKeysForType(draft.type, selectedProfile).filter(
        (k) => !(draftIsGitHubApp && k === CONFIG_INSTALLATION_ID && installations.length > 0),
      )
    : [];
  const knownConfig = new Set([
    ...configKeys,
    ...(draftIsGitHubApp ? [CONFIG_INSTALLATION_ID] : []),
  ]);
  const extraConfigEntries = Object.entries(draft?.config ?? {}).filter(
    ([k]) => !knownConfig.has(k),
  );

  return (
    <div
      className="openshell-band openshell-providers"
      data-testid="openshell-providers"
      aria-labelledby="openshell-providers-title"
    >
      <div className="openshell-band-head openshell-providers-head">
        <h3 id="openshell-providers-title">Providers</h3>
        <p className="dim">
          Credentials for sandboxes (model APIs, GitHub, and so on). Save pushes
          them to the gateway; Sync refreshes provider types and credentials.
          Each <strong>Sandbox spec</strong> chooses which providers attach on
          create. Type <code>{GITHUB_APP_PROVIDER_TYPE}</code> mints sandbox{" "}
          <code>GH_TOKEN</code> from a GitHub App.
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
          No providers yet. Add one here, then attach it on a Sandbox spec for
          create.
        </p>
      ) : (
        <ul className="openshell-provider-list" data-testid="openshell-provider-list">
          {providers.map((p) => {
            const secretKeys = (p.credential_keys ?? []).filter(
              (k) => k !== "GH_TOKEN" || p.type !== GITHUB_APP_PROVIDER_TYPE,
            );
            const displaySecrets =
              p.type === GITHUB_APP_PROVIDER_TYPE
                ? [
                    ...secretKeys.filter((k) => k !== "GH_TOKEN"),
                    ...(p.credential_keys?.includes("GH_TOKEN")
                      ? ["GH_TOKEN (minted)"]
                      : []),
                  ]
                : secretKeys;
            return (
              <li
                key={p.name}
                className="openshell-provider-row"
                data-testid={`openshell-provider-${p.name}`}
              >
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
                <div className="openshell-provider-meta dim">
                  {p.has_credentials || p.has_refresh
                    ? `secrets: ${displaySecrets.join(", ") || "refresh"}`
                    : "no secrets"}
                </div>
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
              disabled={busy}
              onChange={(e) => onDraftChange({ ...draft, name: e.target.value })}
              data-testid="openshell-provider-field-name"
            />
          </label>
          <label>
            Provider type
            <select
              className="search-input"
              value={draft.type}
              disabled={busy}
              onChange={(e) => {
                const type = e.target.value;
                onDraftChange({
                  ...draft,
                  type,
                  name:
                    isGitHubAppType(type) && !draft.name.trim()
                      ? GITHUB_APP_PROVIDER_NAME
                      : draft.name,
                });
              }}
              data-testid="openshell-provider-field-type"
            >
              {typeOptions.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.label}
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
                placeholder="write-only — leave blank to keep existing"
                value={draft.credentials?.[key] ?? ""}
                disabled={busy}
                onChange={(e) =>
                  onDraftChange({
                    ...draft,
                    credentials: { ...(draft.credentials ?? {}), [key]: e.target.value },
                  })
                }
                data-testid={`openshell-provider-cred-${key}`}
              />
            </label>
          ))}
          {configKeys.map((key) => (
            <label key={key}>
              {key}
              <input
                className="search-input"
                value={draft.config?.[key] ?? (key.endsWith("_LOCATION") ? "global" : "")}
                disabled={busy}
                onChange={(e) =>
                  onDraftChange({
                    ...draft,
                    config: { ...(draft.config ?? {}), [key]: e.target.value },
                  })
                }
                data-testid={`openshell-provider-config-${key}`}
              />
            </label>
          ))}
          {draftIsGitHubApp && (
            <>
              <label>
                {CONFIG_INSTALLATION_ID}
                {installations.length > 0 ? (
                  <select
                    className="search-input"
                    value={draft.config?.[CONFIG_INSTALLATION_ID] ?? ""}
                    disabled={busy}
                    onChange={(e) =>
                      onDraftChange({
                        ...draft,
                        config: {
                          ...(draft.config ?? {}),
                          [CONFIG_INSTALLATION_ID]: e.target.value,
                        },
                      })
                    }
                    data-testid="openshell-provider-config-GITHUB_INSTALLATION_ID"
                  >
                    <option value="">Select installation…</option>
                    {installations.map((inst) => (
                      <option key={inst.id} value={String(inst.id)}>
                        {inst.account_login} ({inst.account_type || "account"}) #
                        {inst.id}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    className="search-input"
                    value={draft.config?.[CONFIG_INSTALLATION_ID] ?? ""}
                    disabled={busy}
                    placeholder="numeric installation id"
                    onChange={(e) =>
                      onDraftChange({
                        ...draft,
                        config: {
                          ...(draft.config ?? {}),
                          [CONFIG_INSTALLATION_ID]: e.target.value,
                        },
                      })
                    }
                    data-testid="openshell-provider-config-GITHUB_INSTALLATION_ID"
                  />
                )}
                <span className="dim sandbox-field-hint">
                  Install the App on GitHub, then Refresh installations and pick
                  one. Save/Sync mints <code>GH_TOKEN</code>.
                </span>
              </label>
              <div className="btns" style={{ marginTop: 0 }}>
                <a
                  className="button-link"
                  href="https://github.com/settings/installations"
                  target="_blank"
                  rel="noreferrer"
                  data-testid="github-app-install-link"
                >
                  Install / manage on GitHub
                </a>
                {onRefreshInstallations && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={onRefreshInstallations}
                    data-testid="github-app-refresh-installations"
                  >
                    Refresh installations
                  </button>
                )}
              </div>
            </>
          )}
          {extraConfigEntries.map(([key, value]) => (
            <label key={`extra-${key}`}>
              {key} (extra)
              <input
                className="search-input"
                value={value}
                disabled={busy}
                onChange={(e) =>
                  onDraftChange({
                    ...draft,
                    config: { ...(draft.config ?? {}), [key]: e.target.value },
                  })
                }
                data-testid={`openshell-provider-config-extra-${key}`}
              />
            </label>
          ))}
          <div className="btns">
            <button
              type="button"
              disabled={busy}
              data-testid="openshell-provider-config-add-extra"
              onClick={() => {
                const key = window.prompt("Extra config key");
                if (!key?.trim()) return;
                onDraftChange({
                  ...draft,
                  config: { ...(draft.config ?? {}), [key.trim()]: "" },
                });
              }}
            >
              Add config key
            </button>
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

export function OpenShellProvidersPanel({ gatewayHealthy }: { gatewayHealthy: boolean }) {
  const [providers, setProviders] = useState<OpenShellProviderView[]>([]);
  const [gatewayReachable, setGatewayReachable] = useState(gatewayHealthy);
  const [profiles, setProfiles] = useState<OpenShellProviderTypeEntry[]>([]);
  const [draft, setDraft] = useState<OpenShellProviderWrite | null>(null);
  const [editingName, setEditingName] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [installations, setInstallations] = useState<GitHubAppInstallation[]>([]);

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

  const refreshInstallations = useCallback(() => {
    return api
      .getGitHubApp()
      .then((cfg) => {
        setInstallations(cfg.installations ?? []);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
    api
      .listOpenShellProviderTypes()
      .then(setProfiles)
      .catch(() => setProfiles([]));
  }, [refresh]);

  useEffect(() => {
    if (draft && isGitHubAppType(draft.type)) {
      void refreshInstallations();
    }
  }, [draft?.type, refreshInstallations]);

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
      installations={installations}
      onRefreshInstallations={() => {
        setBusy(true);
        refreshInstallations().finally(() => setBusy(false));
      }}
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
        if (!window.confirm(`Delete provider ${name}?`)) return;
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
            const errBits = out.errors
              .map((e) => `${e.name}: ${e.error}`)
              .join("; ");
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
    />
  );
}
