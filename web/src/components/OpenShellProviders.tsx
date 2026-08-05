import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  OpenShellProviderView,
  OpenShellProviderWrite,
  ProviderTypeProfile,
} from "../types.js";

/** Fixed OpenShell provider name filled by Settings → GitHub App. */
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
          Settings → GitHub App (installation token).
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

export function OpenShellProvidersPanel({ gatewayHealthy }: { gatewayHealthy: boolean }) {
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
