import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type { OpenShellProviderTypeEntry } from "../types.js";
import { YamlEditor } from "./YamlEditor.js";

/** Presentational Provider types band — exported for UI tests without fetch. */
export function OpenShellProviderTypesPanelView({
  types,
  busy,
  error,
  hint,
  draft,
  editingId,
  onDraftChange,
  onSave,
  onCancelEdit,
  onEdit,
  onDelete,
  onAdd,
}: {
  types: OpenShellProviderTypeEntry[];
  busy?: boolean;
  error?: string | null;
  hint?: string | null;
  draft: { id: string; yaml: string; form_config_keys: string[] } | null;
  editingId: string | null;
  onDraftChange: (
    next: { id: string; yaml: string; form_config_keys: string[] } | null,
  ) => void;
  onSave: () => void;
  onCancelEdit: () => void;
  onEdit: (t: OpenShellProviderTypeEntry) => void;
  onDelete: (id: string) => void;
  onAdd: () => void;
}) {
  const boardTypes = types.filter(
    (t) => t.source === "board" || t.source === "both" || t.yaml,
  );

  return (
    <div
      className="openshell-band openshell-provider-types"
      data-testid="openshell-provider-types"
      aria-labelledby="openshell-provider-types-title"
    >
      <div className="openshell-band-head">
        <h3 id="openshell-provider-types-title">Provider types</h3>
        <p className="dim">
          Custom provider type definitions (YAML). Sync uploads them to the
          gateway before applying provider credentials. Built-in types stay on
          the gateway and are not editable here.
        </p>
      </div>

      {error && <div className="err">{error}</div>}
      {hint && (
        <p className="dim" data-testid="openshell-provider-types-hint">
          {hint}
        </p>
      )}

      <div className="btns" style={{ marginBottom: 12 }}>
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={onAdd}
          data-testid="openshell-provider-types-add"
        >
          Add type
        </button>
      </div>

      {boardTypes.length === 0 && !draft ? (
        <p className="dim" data-testid="openshell-provider-types-empty">
          No board provider types yet. Shipped profiles seed on startup; add a
          custom type or Sync after restore.
        </p>
      ) : (
        <ul
          className="openshell-provider-list"
          data-testid="openshell-provider-type-list"
        >
          {boardTypes.map((t) => (
            <li
              key={t.id}
              className="openshell-provider-row"
              data-testid={`openshell-provider-type-${t.id}`}
            >
              <div className="openshell-provider-main">
                <strong>{t.id}</strong>
                <span className="dim">{t.display_name}</span>
                <span
                  className="dim"
                  data-testid={`openshell-provider-type-badge-${t.id}`}
                >
                  {t.shipped ? "shipped" : "custom"}
                </span>
              </div>
              <div className="openshell-provider-meta dim">
                secrets:{" "}
                {t.credential_env_vars.length
                  ? t.credential_env_vars.join(", ")
                  : "none"}
                {t.form_config_keys.length
                  ? ` · config: ${t.form_config_keys.join(", ")}`
                  : ""}
              </div>
              <div className="btns">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => onEdit(t)}
                  data-testid={`openshell-provider-type-edit-${t.id}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => onDelete(t.id)}
                  data-testid={`openshell-provider-type-delete-${t.id}`}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {draft && (
        <form
          className="sandbox-profile-form workspace-form openshell-provider-form"
          data-testid="openshell-provider-type-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSave();
          }}
        >
          <label>
            Id
            <input
              className="search-input"
              value={draft.id}
              disabled={busy || editingId != null}
              onChange={(e) => onDraftChange({ ...draft, id: e.target.value })}
              data-testid="openshell-provider-type-field-id"
              placeholder="my-provider-type"
            />
          </label>
          <label>
            YAML
            <YamlEditor
              className="sandbox-policy-textarea"
              rows={16}
              value={draft.yaml}
              disabled={busy}
              onChange={(yaml) => onDraftChange({ ...draft, yaml })}
              data-testid="openshell-provider-type-field-yaml"
              placeholder={"id: my-type\ndisplay_name: …\ncredentials:\n  - name: api_key\n    env_vars:\n      - MY_API_KEY\n"}
            />
          </label>
          <label>
            Form config keys (comma-separated, non-secret)
            <input
              className="search-input"
              value={draft.form_config_keys.join(", ")}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  form_config_keys: e.target.value
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean),
                })
              }
              data-testid="openshell-provider-type-field-form-keys"
              placeholder="MY_PROJECT, MY_LOCATION"
            />
          </label>
          <div className="btns">
            <button
              type="submit"
              className="primary"
              disabled={busy}
              data-testid="openshell-provider-type-save"
            >
              Save type
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

export function OpenShellProviderTypesPanel() {
  const [types, setTypes] = useState<OpenShellProviderTypeEntry[]>([]);
  const [draft, setDraft] = useState<{
    id: string;
    yaml: string;
    form_config_keys: string[];
  } | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    return api
      .listOpenShellProviderTypes()
      .then((list) => {
        setTypes(list);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <OpenShellProviderTypesPanelView
      types={types}
      busy={busy}
      error={error}
      hint={hint}
      draft={draft}
      editingId={editingId}
      onDraftChange={setDraft}
      onAdd={() => {
        setHint(null);
        setEditingId(null);
        setDraft({ id: "", yaml: "", form_config_keys: [] });
      }}
      onEdit={(t) => {
        setHint(null);
        setEditingId(t.id);
        setDraft({
          id: t.id,
          yaml: t.yaml ?? "",
          form_config_keys: t.form_config_keys ?? [],
        });
      }}
      onCancelEdit={() => {
        setDraft(null);
        setEditingId(null);
      }}
      onDelete={(id) => {
        if (!window.confirm(`Delete provider type ${id}?`)) return;
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .deleteOpenShellProviderType(id)
          .then(() => {
            setHint(`Deleted ${id}. Shipped types stay deleted until you re-add them.`);
            if (editingId === id) {
              setDraft(null);
              setEditingId(null);
            }
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onSave={() => {
        if (!draft) return;
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .putOpenShellProviderType({
            id: draft.id.trim(),
            yaml: draft.yaml,
            form_config_keys: draft.form_config_keys,
          })
          .then(() => {
            setHint("Saved. Sync all on Providers imports this YAML to the gateway.");
            setDraft(null);
            setEditingId(null);
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}
