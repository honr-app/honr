import type {
  AgentRuntimeConfig,
  AuthSettings,
  AuthStatus,
  GitHubAppSettings,
  OpenShellProviderView,
  OpenShellProviderWrite,
  OpenShellProvidersOut,
  OpenShellSettings,
  OpenShellStatus,
  ProviderTypeProfile,
  SandboxProfile,
  SandboxProfilesOut,
  Snapshot,
  SyncProvidersOut,
  WorkItem,
  WorkspaceBinding,
} from "./types";

export class AuthRequiredError extends Error {
  bootstrap: boolean;
  constructor(message: string, bootstrap = false) {
    super(message);
    this.name = "AuthRequiredError";
    this.bootstrap = bootstrap;
  }
}

const fetchOpts: RequestInit = { credentials: "include" };

async function jsonOrThrow(r: Response) {
  const body = await r.json().catch(() => ({}));
  if (r.status === 401) {
    throw new AuthRequiredError(
      body?.error ?? "authentication required",
      !!body?.bootstrap,
    );
  }
  if (!r.ok) throw new Error(body?.error ?? `${r.status} ${r.statusText}`);
  return body;
}

const post = (path: string, body?: unknown) =>
  fetch(`/api${path}`, {
    ...fetchOpts,
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  }).then(jsonOrThrow);

const put = (path: string, body?: unknown) =>
  fetch(`/api${path}`, {
    ...fetchOpts,
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  }).then(jsonOrThrow);

const del = (path: string) =>
  fetch(`/api${path}`, { ...fetchOpts, method: "DELETE" }).then(async (r) => {
    if (r.status === 204) return null;
    return jsonOrThrow(r);
  });

export const api = {
  getAuthStatus: (): Promise<AuthStatus> =>
    fetch("/auth/status", fetchOpts).then(jsonOrThrow),
  bootstrap: (body: { username: string; password: string }): Promise<AuthStatus> =>
    fetch("/auth/bootstrap", {
      ...fetchOpts,
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }).then(jsonOrThrow),
  login: (body: { username: string; password: string }): Promise<AuthStatus> =>
    fetch("/auth/login", {
      ...fetchOpts,
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }).then(jsonOrThrow),
  logout: (): Promise<void> =>
    fetch("/auth/logout", { ...fetchOpts, method: "POST" }).then(async (r) => {
      if (r.status === 204) return;
      await jsonOrThrow(r);
    }),
  getAuthSettings: (): Promise<AuthSettings> =>
    fetch("/api/auth/settings", fetchOpts).then(jsonOrThrow),
  putAuthSettings: (body: {
    allowed_users?: string[];
    allowed_teams?: string[];
    new_password?: string | null;
  }): Promise<AuthSettings> => put("/auth/settings", body),

  board: (): Promise<Snapshot> => fetch("/api/board", fetchOpts).then(jsonOrThrow),
  digest: () => fetch("/api/digest", fetchOpts).then(jsonOrThrow),
  detail: (id: number) => fetch(`/api/items/${id}`, fetchOpts).then(jsonOrThrow),
  logs: (id: number): Promise<{ agent: string[]; openshell: string[] }> =>
    fetch(`/api/items/${id}/logs`, fetchOpts).then(jsonOrThrow),
  // The human verbs. Each costs the system something different.
  steer: (id: number, text: string): Promise<WorkItem> =>
    post(`/items/${id}/steer`, { text }),
  /** Write / revise Initial plan proposal (does not materialize Tasks). */
  savePlan: (
    id: number,
    body: {
      summary?: string;
      tasks: {
        key: string;
        title: string;
        intent: string;
        definition_of_done: string;
        blocked_by_keys: string[];
        capability?: string | null;
      }[];
      cancel_keys?: string[];
    },
  ): Promise<import("./types").TaskProposal> => post(`/items/${id}/plan`, body),
  park: (id: number, reason?: string): Promise<WorkItem> =>
    post(`/items/${id}/park`, { reason }),
  unpark: (id: number): Promise<WorkItem> => post(`/items/${id}/unpark`),
  halt: (id: number, reason?: string): Promise<WorkItem> =>
    post(`/items/${id}/halt`, { reason }),
  answer: (id: number, choice: string): Promise<WorkItem> =>
    post(`/items/${id}/answer`, { choice }),
  approve: (id: number): Promise<WorkItem> => post(`/items/${id}/approve`),
  /** Approve Initial plan proposal → Backlog Tasks. Id = Project or Initial plan. */
  approvePlan: (id: number): Promise<number[]> => post(`/items/${id}/approve-plan`),
  /** Seed Initial plan Task with Task-scoped remotes. Id = Project. */
  initPlan: (
    id: number,
    repo: { upstream: string; fork?: string; base?: string },
  ): Promise<WorkItem> => post(`/items/${id}/init-plan`, { repo }),
  /** Queue a Backlog card for the supervisor to claim. Explicit start. */
  dispatch: (id: number): Promise<WorkItem> => post(`/items/${id}/dispatch`),
  /** Play/pause Project auto mode (queue claimable Backlog leaves). */
  setAutoDispatch: (id: number, enabled: boolean): Promise<WorkItem> =>
    post(`/items/${id}/auto-dispatch`, { enabled }),
  requestChanges: (id: number, text: string): Promise<WorkItem> =>
    post(`/items/${id}/request-changes`, { text }),
  transition: (id: number, to: string, reason?: string): Promise<WorkItem> =>
    post(`/items/${id}/transition`, { to, reason }),
  update: (
    id: number,
    fields: {
      title?: string;
      intent?: string;
      definition_of_done?: string;
      engine?: string;
      project_prompt?: string;
    },
  ): Promise<WorkItem> => post(`/items/${id}/update`, fields),
  cut: (id: number, reason?: string): Promise<number[]> =>
    post(`/items/${id}/cut`, { reason }),
  deleteItem: (id: number): Promise<void> =>
    fetch(`/api/items/${id}`, { ...fetchOpts, method: "DELETE" }).then(jsonOrThrow),

  listSandboxProfiles: (): Promise<SandboxProfilesOut> =>
    fetch("/api/sandbox-profiles", fetchOpts).then(jsonOrThrow),
  upsertSandboxProfile: (profile: {
    /** Omit on create — server derives a slug from `name`. */
    id?: string;
    name: string;
    image: string;
    policy: string;
    cpu?: string | null;
    memory?: string | null;
  }): Promise<SandboxProfile> => post("/sandbox-profiles", profile),
  setDefaultSandboxProfile: (id: string): Promise<SandboxProfilesOut> =>
    post(`/sandbox-profiles/${encodeURIComponent(id)}/default`),
  /** Project only. Pass `null` (or omit) to inherit the global default. */
  setProjectSandboxProfile: (
    id: number,
    sandbox_profile_id: string | null,
  ): Promise<WorkItem> =>
    post(`/items/${id}/sandbox-profile`, { sandbox_profile_id }),

  getWorkspace: (): Promise<WorkspaceBinding> =>
    fetch("/api/workspace", fetchOpts).then(jsonOrThrow),
  putWorkspace: (binding: WorkspaceBinding): Promise<WorkspaceBinding> =>
    put("/workspace", binding),

  getAgentRuntime: (): Promise<AgentRuntimeConfig> =>
    fetch("/api/agent-runtime", fetchOpts).then(jsonOrThrow),
  putAgentRuntime: (settings: AgentRuntimeConfig): Promise<AgentRuntimeConfig> =>
    put("/agent-runtime", settings),

  getOpenShell: (): Promise<OpenShellSettings> =>
    fetch("/api/openshell", fetchOpts).then(jsonOrThrow),
  putOpenShell: (settings: OpenShellSettings): Promise<OpenShellSettings> =>
    put("/openshell", settings),
  getOpenShellStatus: (): Promise<OpenShellStatus> =>
    fetch("/api/openshell/status", fetchOpts).then(jsonOrThrow),

  getGitHubApp: (): Promise<GitHubAppSettings> =>
    fetch("/api/github-app", fetchOpts).then(jsonOrThrow),
  putGitHubApp: (settings: GitHubAppSettings): Promise<GitHubAppSettings> =>
    put("/github-app", settings),

  listOpenShellProviders: (): Promise<OpenShellProvidersOut> =>
    fetch("/api/openshell/providers", fetchOpts).then(jsonOrThrow),
  createOpenShellProvider: (body: OpenShellProviderWrite): Promise<OpenShellProviderView> =>
    post("/openshell/providers", body),
  updateOpenShellProvider: (
    name: string,
    body: OpenShellProviderWrite,
  ): Promise<OpenShellProviderView> =>
    put(`/openshell/providers/${encodeURIComponent(name)}`, body),
  deleteOpenShellProvider: (name: string): Promise<null> =>
    del(`/openshell/providers/${encodeURIComponent(name)}`),
  syncOpenShellProviders: (): Promise<SyncProvidersOut> =>
    post("/openshell/providers/sync"),
  listOpenShellProviderProfiles: (): Promise<ProviderTypeProfile[]> =>
    fetch("/api/openshell/provider-profiles", fetchOpts).then(jsonOrThrow),
};

/** `4s`, `12m`, `3h 5m` — matches the server's own formatting. */
export function since(iso: string, now: number): string {
  const secs = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return m ? `${h}h ${m}m` : `${h}h`;
}

export const secsSince = (iso: string, now: number) =>
  Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));

/** Seconds until an ISO deadline (0 if already past). */
export const secsUntil = (iso: string, now: number) =>
  Math.max(0, Math.floor((new Date(iso).getTime() - now) / 1000));

/** `12m 04s`, `4s`, `1h 02m` — countdown on a Running card. */
export function formatCountdown(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}m ${String(s).padStart(2, "0")}s`;
  }
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${String(m).padStart(2, "0")}m`;
}
