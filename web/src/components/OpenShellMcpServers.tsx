import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type {
  McpAudience,
  McpServerDesired,
  McpTransport,
} from "../types.js";
import { YamlEditor } from "./YamlEditor.js";

type Kind = "http" | "stdio";

type ServerDraft = {
  id: string;
  name: string;
  kind: Kind;
  url: string;
  auth: "none" | "cockpit_bearer" | "bearer_env";
  bearerEnv: string;
  command: string;
  argsText: string;
  cwd: string;
  policy_fragment_yaml: string;
  provider_names_text: string;
  env_text: string;
  audience: McpAudience;
  shipped: boolean;
};

const emptyDraft = (): ServerDraft => ({
  id: "",
  name: "",
  kind: "stdio",
  url: "",
  auth: "none",
  bearerEnv: "",
  command: "uv",
  argsText:
    "tool run --from context-server@latest context-server serve --db /tmp/kb.db",
  cwd: "",
  policy_fragment_yaml: "",
  provider_names_text: "",
  env_text: "",
  audience: "both",
  shipped: false,
});

function draftFrom(s: McpServerDesired): ServerDraft {
  const t = s.transport;
  if (t.kind === "http") {
    const authKind = t.auth?.kind ?? "none";
    return {
      id: s.id,
      name: s.name,
      kind: "http",
      url: t.url ?? "",
      auth:
        authKind === "cockpit_bearer"
          ? "cockpit_bearer"
          : authKind === "bearer_env"
            ? "bearer_env"
            : "none",
      bearerEnv: t.auth?.kind === "bearer_env" ? t.auth.env : "",
      command: "",
      argsText: "",
      cwd: "",
      policy_fragment_yaml: s.policy_fragment_yaml ?? "",
      provider_names_text: (s.provider_names ?? []).join(", "),
      env_text: Object.entries(s.env ?? {})
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
      audience: s.audience ?? "cockpit",
      shipped: !!s.shipped,
    };
  }
  return {
    id: s.id,
    name: s.name,
    kind: "stdio",
    url: "",
    auth: "none",
    bearerEnv: "",
    command: t.command,
    argsText: (t.args ?? []).join(" "),
    cwd: t.cwd ?? "",
    policy_fragment_yaml: s.policy_fragment_yaml ?? "",
    provider_names_text: (s.provider_names ?? []).join(", "),
    env_text: Object.entries(s.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
    audience: s.audience ?? "both",
    shipped: !!s.shipped,
  };
}

function parseEnv(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t || t.startsWith("#")) continue;
    const eq = t.indexOf("=");
    if (eq <= 0) continue;
    out[t.slice(0, eq).trim()] = t.slice(eq + 1);
  }
  return out;
}

function transportFrom(d: ServerDraft): McpTransport {
  if (d.kind === "http") {
    const auth =
      d.auth === "cockpit_bearer"
        ? { kind: "cockpit_bearer" as const }
        : d.auth === "bearer_env"
          ? { kind: "bearer_env" as const, env: d.bearerEnv.trim() }
          : { kind: "none" as const };
    return { kind: "http", url: d.url.trim(), auth };
  }
  const args = d.argsText
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  return {
    kind: "stdio",
    command: d.command.trim(),
    args,
    cwd: d.cwd.trim() || null,
  };
}

function transportLabel(s: McpServerDesired): string {
  if (s.transport.kind === "http") {
    return `http ${s.transport.url || "(cockpit bearer)"}`;
  }
  return `stdio ${s.transport.command}`;
}

export function OpenShellMcpServersPanelView({
  servers,
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
  servers: McpServerDesired[];
  busy?: boolean;
  error?: string | null;
  hint?: string | null;
  draft: ServerDraft | null;
  editingId: string | null;
  onDraftChange: (next: ServerDraft | null) => void;
  onSave: () => void;
  onCancelEdit: () => void;
  onEdit: (s: McpServerDesired) => void;
  onDelete: (id: string) => void;
  onStartCreate: () => void;
}) {
  const isCreate = editingId === "";
  const isEditing = editingId !== null && draft != null;

  return (
    <div
      className="openshell-band openshell-mcp-servers"
      data-testid="openshell-mcp-servers"
      aria-labelledby="openshell-mcp-servers-title"
    >
      <div className="openshell-band-head">
        <h3 id="openshell-mcp-servers-title">MCP servers</h3>
        <p className="dim">
          HTTP or stdio servers injected into sandboxes. Attach them on a{" "}
          <strong>Sandbox spec</strong>. Policy fragments and providers merge at
          create; engines get Cursor/Claude/agy/OpenCode config without pasting
          JSON.
        </p>
      </div>

      {error && <div className="err">{error}</div>}
      {hint && (
        <p className="dim" data-testid="openshell-mcp-servers-hint">
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
            data-testid="openshell-mcp-servers-add"
          >
            Add MCP server
          </button>
        </div>
      )}

      {servers.length === 0 && !isEditing ? (
        <p className="dim" data-testid="openshell-mcp-servers-empty">
          No MCP servers yet.
        </p>
      ) : (
        <ul
          className="openshell-provider-list"
          data-testid="openshell-mcp-server-list"
        >
          {servers.map((s) => (
            <li
              key={s.id}
              className="openshell-provider-row"
              data-testid={`openshell-mcp-server-${s.id}`}
            >
              <div className="openshell-provider-main">
                <strong>{s.name}</strong>
                <span className="dim">
                  {s.id}
                  {s.shipped ? " · shipped" : ""}
                </span>
              </div>
              <div className="openshell-provider-meta dim">
                {transportLabel(s)} · {s.audience ?? "cockpit"}
              </div>
              <div className="btns">
                <button
                  type="button"
                  disabled={busy || isEditing}
                  onClick={() => onEdit(s)}
                  data-testid={`openshell-mcp-server-edit-${s.id}`}
                >
                  Edit
                </button>
                <button
                  type="button"
                  disabled={busy || isEditing || !!s.shipped}
                  onClick={() => onDelete(s.id)}
                  data-testid={`openshell-mcp-server-delete-${s.id}`}
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
          data-testid="openshell-mcp-server-form"
          onSubmit={(e) => {
            e.preventDefault();
            onSave();
          }}
        >
          <h3>{isCreate ? "Create MCP server" : `Edit ${editingId}`}</h3>
          {!isCreate && (
            <label>
              Id
              <input
                className="search-input"
                value={draft.id}
                disabled
                readOnly
                data-testid="openshell-mcp-field-id"
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
              data-testid="openshell-mcp-field-name"
            />
          </label>
          <label>
            Audience
            <select
              className="search-input"
              value={draft.audience}
              disabled={busy || draft.shipped}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  audience: e.target.value as McpAudience,
                })
              }
              data-testid="openshell-mcp-field-audience"
            >
              <option value="cockpit">Cockpit only</option>
              <option value="worker">Workers only</option>
              <option value="both">Cockpit + workers</option>
            </select>
          </label>
          <label>
            Transport
            <select
              className="search-input"
              value={draft.kind}
              disabled={busy || draft.shipped}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  kind: e.target.value as Kind,
                })
              }
              data-testid="openshell-mcp-field-kind"
            >
              <option value="stdio">Stdio (command + args)</option>
              <option value="http">HTTP (Streamable HTTP URL)</option>
            </select>
          </label>

          {draft.kind === "http" ? (
            <>
              <label>
                URL
                <input
                  className="search-input"
                  value={draft.url}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({ ...draft, url: e.target.value })
                  }
                  placeholder="https://… or empty for cockpit bearer"
                  data-testid="openshell-mcp-field-url"
                />
              </label>
              <label>
                Auth
                <select
                  className="search-input"
                  value={draft.auth}
                  disabled={busy || draft.shipped}
                  onChange={(e) =>
                    onDraftChange({
                      ...draft,
                      auth: e.target.value as ServerDraft["auth"],
                    })
                  }
                  data-testid="openshell-mcp-field-auth"
                >
                  <option value="none">None</option>
                  <option value="cockpit_bearer">Cockpit Bearer (honr /mcp)</option>
                  <option value="bearer_env">Bearer from env</option>
                </select>
              </label>
              {draft.auth === "bearer_env" && (
                <label>
                  Env key
                  <input
                    className="search-input"
                    value={draft.bearerEnv}
                    disabled={busy}
                    onChange={(e) =>
                      onDraftChange({ ...draft, bearerEnv: e.target.value })
                    }
                    required
                    data-testid="openshell-mcp-field-bearer-env"
                  />
                </label>
              )}
            </>
          ) : (
            <>
              <label>
                Command
                <input
                  className="search-input"
                  value={draft.command}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({ ...draft, command: e.target.value })
                  }
                  required
                  data-testid="openshell-mcp-field-command"
                />
              </label>
              <label>
                Args (whitespace-separated)
                <input
                  className="search-input"
                  value={draft.argsText}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({ ...draft, argsText: e.target.value })
                  }
                  data-testid="openshell-mcp-field-args"
                />
              </label>
              <label>
                cwd (optional)
                <input
                  className="search-input"
                  value={draft.cwd}
                  disabled={busy}
                  onChange={(e) =>
                    onDraftChange({ ...draft, cwd: e.target.value })
                  }
                  data-testid="openshell-mcp-field-cwd"
                />
              </label>
            </>
          )}

          <label>
            Provider names (comma-separated)
            <input
              className="search-input"
              value={draft.provider_names_text}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({
                  ...draft,
                  provider_names_text: e.target.value,
                })
              }
              placeholder="gcp-adc, …"
              data-testid="openshell-mcp-field-providers"
            />
          </label>
          <label>
            Env (KEY=value per line)
            <textarea
              className="search-input"
              rows={4}
              value={draft.env_text}
              disabled={busy}
              onChange={(e) =>
                onDraftChange({ ...draft, env_text: e.target.value })
              }
              data-testid="openshell-mcp-field-env"
            />
          </label>
          <label>
            Policy fragment YAML (optional)
            <YamlEditor
              className="sandbox-policy-textarea"
              value={draft.policy_fragment_yaml}
              disabled={busy}
              onChange={(policy_fragment_yaml) =>
                onDraftChange({ ...draft, policy_fragment_yaml })
              }
              rows={10}
              placeholder={
                "network_policies:\n  pypi:\n    name: pypi\n    endpoints:\n      - { host: pypi.org, port: 443, access: full, tls: skip }\n"
              }
              data-testid="openshell-mcp-field-fragment"
            />
          </label>
          <div className="btns">
            <button
              type="submit"
              className="primary"
              disabled={busy}
              data-testid="openshell-mcp-server-save"
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

export function OpenShellMcpServersPanel() {
  const [servers, setServers] = useState<McpServerDesired[]>([]);
  const [draft, setDraft] = useState<ServerDraft | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const refresh = useCallback(() => {
    return api
      .listMcpServers()
      .then((out) => {
        setServers(out.servers);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <OpenShellMcpServersPanelView
      servers={servers}
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
      onEdit={(s) => {
        setEditingId(s.id);
        setDraft(draftFrom(s));
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
        setBusy(true);
        setError(null);
        setHint(null);
        const provider_names = draft.provider_names_text
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        const body = {
          ...(editingId ? { id: draft.id.trim() } : {}),
          name,
          transport: transportFrom(draft),
          policy_fragment_yaml: draft.policy_fragment_yaml.trim() || null,
          provider_names,
          env: parseEnv(draft.env_text),
          audience: draft.audience,
        };
        api
          .upsertMcpServer(body)
          .then(() => {
            setDraft(null);
            setEditingId(null);
            setHint("MCP server saved.");
            return refresh();
          })
          .catch((e) => setError(String(e)))
          .finally(() => setBusy(false));
      }}
      onDelete={(id) => {
        if (!window.confirm(`Delete MCP server ${id}?`)) return;
        setBusy(true);
        setError(null);
        setHint(null);
        api
          .deleteMcpServer(id)
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
