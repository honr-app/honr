import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type { OpenShellPolicy } from "../types.js";
import { YamlEditor } from "./YamlEditor.js";

type PolicyDraft = {
  id: string;
  name: string;
  yaml: string;
};

const emptyDraft = (): PolicyDraft => ({
  id: "",
  name: "",
  yaml: "",
});

function draftFrom(p: OpenShellPolicy): PolicyDraft {
  return {
    id: p.id,
    name: p.name,
    yaml: p.yaml,
  };
}

/** Presentational Policies band — exported for UI tests without fetch. */
export function OpenShellPoliciesPanelView({
  policies,
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
  onStartCreate,
}: {
  policies: OpenShellPolicy[];
  busy?: boolean;
  error?: string | null;
  hint?: string | null;
  draft: PolicyDraft | null;
  /** `null` = not editing; `""` = create; otherwise editing that id. */
  editingId: string | null;
  onDraftChange: (next: PolicyDraft | null) => void;
  onSave: () => void;
  onCancelEdit: () => void;
  onEdit: (p: OpenShellPolicy) => void;
  onDelete: (id: string) => void;
  onStartCreate: () => void;
}) {
  const isCreate = editingId === "";
  const isEditing = editingId !== null && draft != null;

  return (
    <div
      className="openshell-band openshell-policies"
      data-testid="openshell-policies"
      aria-labelledby="openshell-policies-title"
    >
      <div className="openshell-band-head">
        <h3 id="openshell-policies-title">Policies</h3>
        <p className="dim">
          Named OpenShell allow-list YAML stored on the board. Edit egress and
          filesystem rules here; each{" "}
          <strong>Sandbox spec</strong> picks one policy by id. Live policy on a
          running sandbox still comes from the board at create and is immutable
          for that seat.
        </p>
      </div>

      {error && <div className="err">{error}</div>}
      {hint && (
        <p className="dim" data-testid="openshell-policies-hint">
          {hint}
        </p>
      )}

      {!isEditing && (
        <div className="btns" style={{ marginBottom: 12 }}>
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={onStartCreate}
            data-testid="openshell-policies-add"
          >
            Add policy
          </button>
        </div>
      )}

      {policies.length === 0 && !isEditing ? (
        <p className="dim" data-testid="openshell-policies-empty">
          No policies yet. Add one here, then attach it on a Sandbox spec.
        </p>
      ) : (
        <ul className="openshell-provider-list" data-testid="openshell-policy-list">
          {policies.map((p) => (
            <li
              key={p.id}
              className="openshell-provider-row"
              data-testid={`openshell-policy-${p.id}`}
            >
              <div className="openshell-provider-main">
                <strong>{p.name}</strong>
                <span className="dim">{p.id}</span>
              </div>
              <div className="openshell-provider-meta dim">
                {p.yaml.split("\n").length} lines
              </div>
              <div className="btns">
                <button
                  type="button"
                  disabled={busy || isEditing}
                  onClick={() => onEdit(p)}
                  data-testid={`openshell-policy-edit-${p.id}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  disabled={busy || isEditing}
                  onClick={() => onDelete(p.id)}
                  data-testid={`openshell-policy-delete-${p.id}`}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {isEditing && draft && (
        <form
          className="sandbox-profile-form workspace-form openshell-provider-form"
          data-testid="openshell-policy-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSave();
          }}
        >
          <h3>{isCreate ? "Create policy" : `Edit ${editingId}`}</h3>
          {!isCreate && (
            <label>
              Id
              <input
                className="search-input"
                value={draft.id}
                disabled
                readOnly
                data-testid="openshell-policy-field-id"
              />
            </label>
          )}
          <label>
            Name
            <input
              className="search-input"
              value={draft.name}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({ ...draft, name: e.target.value })
              }
              required
              data-testid="openshell-policy-field-name"
            />
          </label>
          <label>
            Policy YAML
            <YamlEditor
              className="sandbox-policy-textarea"
              value={draft.yaml}
              disabled={busy}
              onChange={(yaml) => onDraftChange({ ...draft, yaml })}
              required
              rows={18}
              placeholder={
                "version: 1\nfilesystem_policy:\n  include_workdir: true\n"
              }
              data-testid="openshell-policy-field-yaml"
            />
          </label>
          <div className="btns">
            <button
              type="submit"
              className="primary"
              disabled={busy}
              data-testid="openshell-policy-save"
            >
              {isCreate ? "Create" : "Save"}
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

export function OpenShellPoliciesPanel() {
  const [policies, setPolicies] = useState<OpenShellPolicy[]>([]);
  const [draft, setDraft] = useState<PolicyDraft | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    return api
      .listOpenShellPolicies()
      .then((out) => {
        setPolicies(out.policies);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <OpenShellPoliciesPanelView
      policies={policies}
      busy={busy}
      error={error}
      hint={hint}
      draft={draft}
      editingId={editingId}
      onDraftChange={setDraft}
      onCancelEdit={() => {
        setDraft(null);
        setEditingId(null);
      }}
      onStartCreate={() => {
        setEditingId("");
        setDraft(emptyDraft());
        setHint(null);
        setError(null);
      }}
      onEdit={(p) => {
        setEditingId(p.id);
        setDraft(draftFrom(p));
        setHint(null);
        setError(null);
      }}
      onSave={() => {
        if (!draft) return;
        const name = draft.name.trim();
        if (!name) {
          setError("name is required");
          return;
        }
        if (!draft.yaml.trim()) {
          setError("yaml is required");
          return;
        }
        setBusy(true);
        setError(null);
        setHint(null);
        const body = {
          ...(editingId ? { id: draft.id.trim() } : {}),
          name,
          yaml: draft.yaml,
        };
        api
          .upsertOpenShellPolicy(body)
          .then(() => {
            setDraft(null);
            setEditingId(null);
            setHint("Policy saved.");
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onDelete={(id) => {
        if (!window.confirm(`Delete policy ${id}? Specs still using it will refuse.`)) {
          return;
        }
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .deleteOpenShellPolicy(id)
          .then(() => {
            if (editingId === id) {
              setDraft(null);
              setEditingId(null);
            }
            setHint(`Deleted ${id}.`);
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
    />
  );
}
