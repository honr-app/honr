import type { Snapshot, WorkItem } from "./types";

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

export const api = {
  board: (): Promise<Snapshot> => fetch("/api/board").then(jsonOrThrow),
  digest: () => fetch("/api/digest").then(jsonOrThrow),
  detail: (id: number) => fetch(`/api/items/${id}`).then(jsonOrThrow),

  // The human verbs. Each costs the system something different.
  steer: (id: number, text: string): Promise<WorkItem> =>
    post(`/items/${id}/steer`, { text }),
  pin: (id: number, text: string): Promise<WorkItem> =>
    post(`/items/${id}/pin`, { text }),
  halt: (id: number, reason?: string): Promise<WorkItem> =>
    post(`/items/${id}/halt`, { reason }),
  answer: (id: number, choice: string): Promise<WorkItem> =>
    post(`/items/${id}/answer`, { choice }),
  approve: (id: number): Promise<WorkItem> => post(`/items/${id}/approve`),
  requestChanges: (id: number, text: string): Promise<WorkItem> =>
    post(`/items/${id}/request-changes`, { text }),
  cut: (id: number, reason?: string): Promise<number[]> =>
    post(`/items/${id}/cut`, { reason }),
};

export const money = (cents: number) => `$${(cents / 100).toFixed(2)}`;

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
