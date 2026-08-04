import type {
  AgentRuntimeConfig,
  OpenShellSettings,
  OpenShellStatus,
  SandboxProfile,
  SandboxProfilesOut,
  Snapshot,
  WorkItem,
  WorkspaceBinding,
} from "./types";

async function jsonOrThrow(r: Response) {
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body?.error ?? `${r.status} ${r.statusText}`);
  return body;
}

const post = (path: string, body?: unknown) =>
  fetch(`/api${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  }).then(jsonOrThrow);

const put = (path: string, body?: unknown) =>
  fetch(`/api${path}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  }).then(jsonOrThrow);

export const api = {
  board: (): Promise<Snapshot> => fetch("/api/board").then(jsonOrThrow),
  digest: () => fetch("/api/digest").then(jsonOrThrow),
  detail: (id: number) => fetch(`/api/items/${id}`).then(jsonOrThrow),
  logs: (id: number): Promise<{ claude: string[]; openshell: string[] }> => fetch(`/api/items/${id}/logs`).then(jsonOrThrow),
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
    fetch(`/api/items/${id}`, { method: "DELETE" }).then(jsonOrThrow),

  listSandboxProfiles: (): Promise<SandboxProfilesOut> =>
    fetch("/api/sandbox-profiles").then(jsonOrThrow),
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
    fetch("/api/workspace").then(jsonOrThrow),
  putWorkspace: (binding: WorkspaceBinding): Promise<WorkspaceBinding> =>
    put("/workspace", binding),

  getAgentRuntime: (): Promise<AgentRuntimeConfig> =>
    fetch("/api/agent-runtime").then(jsonOrThrow),
  putAgentRuntime: (settings: AgentRuntimeConfig): Promise<AgentRuntimeConfig> =>
    put("/agent-runtime", settings),

  getOpenShell: (): Promise<OpenShellSettings> =>
    fetch("/api/openshell").then(jsonOrThrow),
  putOpenShell: (settings: OpenShellSettings): Promise<OpenShellSettings> =>
    put("/openshell", settings),
  getOpenShellStatus: (): Promise<OpenShellStatus> =>
    fetch("/api/openshell/status").then(jsonOrThrow),
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
