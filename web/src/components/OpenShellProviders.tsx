import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  OpenShellProviderTypeEntry,
  OpenShellProviderView,
  OpenShellProviderWrite,
} from "../types.js";

/** Fixed OpenShell provider name filled by Settings → GitHub App. */
const GITHUB_APP_PROVIDER_NAME = "github";

/** Gateway builtin without board YAML — keep project/location fields. */
const VERTEX_FORM_KEYS = ["VERTEX_AI_PROJECT_ID", "VERTEX_AI_LOCATION"];

function isGitHubAppManagedProvider(name: string, type?: string): boolean {
  return name.trim() === GITHUB_APP_PROVIDER_NAME && (type == null || type === "github");
}

function formConfigKeysForType(
  typeId: string,
  profile?: OpenShellProviderTypeEntry,
): string[] {
  if (profile?.form_config_keys?.length) return profile.form_config_keys;
  if (typeId === "google-vertex-ai") return VERTEX_FORM_KEYS;
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
}) {
  const typeOptions = profiles.length
    ? profiles.map((p) => ({ id: p.id, label: p.display_name || p.id }))
    : [
        { id: "github", label: "github" },
        { id: "google-vertex-ai", label: "google-vertex-ai" },
        { id: "cursor-agent", label: "cursor-agent" },
        { id: "antigravity", label: "antigravity" },
      ];
  // Prefer a non-App type for "Add" — github/GH_TOKEN is owned by GitHub App.
  const defaultAddType =
    typeOptions.find((t) => t.id !== "github")?.id ??
    typeOptions[0]?.id ??
    "google-vertex-ai";
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
  const configKeys = draft
    ? formConfigKeysForType(draft.type, selectedProfile)
    : [];
  const knownConfig = new Set(configKeys);
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
          Desired provider catalog on the board (credentials sealed). Save
          applies to the gateway when reachable; Sync all imports missing
          provider types, applies credentials, and attaches listed providers to
          a running cockpit seat. Which providers attach on create is chosen per{" "}
          <strong>Sandbox spec</strong>, not here. Provider <code>github</code>{" "}
          is owned by Settings → GitHub App (<code>GH_TOKEN</code>); do not
          manage those credentials by hand.
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
            Provider type
            <select
              className="search-input"
              value={draft.type}
              disabled={busy || draftManaged}
              onChange={(e) => onDraftChange({ ...draft, type: e.target.value })}
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
                  Passed into the sandbox as <code>{key}</code>. Value comes
                  from the App installation token — mint under Settings → GitHub
                  App.
                </span>
              )}
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
      .listOpenShellProviderTypes()
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
    />
  );
}
