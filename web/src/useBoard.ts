import { useEffect, useReducer, useRef, useState } from "react";
import { api } from "./api.js";
import type { BoardEvent, GoalView, Snapshot, StoryLine, WorkItem } from "./types.js";

export type BoardEventListener = (ev: BoardEvent) => void;

const listeners = new Set<BoardEventListener>();

/**
 * Subscribe to real-time board events emitted by SSE/WebSocket stream.
 * Returns an unsubscribe function that cleans up the listener when called.
 */
export function subscribeBoardEvents(listener: BoardEventListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Emit a board event to all active subscribers.
 */
export function emitBoardEvent(ev: BoardEvent): void {
  for (const listener of listeners) {
    try {
      listener(ev);
    } catch {
      /* ignore subscriber errors */
    }
  }
}

export interface BoardState {
  items: Map<number, WorkItem>;
  goals: GoalView[];
  stories: Map<number, StoryLine[]>;
  serverTime: string | null;
  agentTimeout: number;
  loaded: boolean;
  connected: boolean;
  defaultEngine: string;
  defaultModel: string;
  /** When the last successful load happened. Drives the staleness warning. */
  lastLoadedAt: number | null;
  /** Monotonic event sequence number. */
  lastSeenSeq: number;
}

export type Action =
  | { type: "snapshot"; snap: Snapshot }
  | { type: "event"; ev: BoardEvent }
  | { type: "connected"; ok: boolean };

export const initial: BoardState = {
  items: new Map(),
  goals: [],
  stories: new Map(),
  serverTime: null,
  agentTimeout: 1800,
  loaded: false,
  connected: false,
  defaultEngine: "",
  defaultModel: "",
  lastLoadedAt: null,
  lastSeenSeq: 0,
};

/**
 * Pure reducer for board state updates.
 * Guards against stale REST snapshots with older sequence numbers overwriting
 * newer live event state.
 */
export function reduce(s: BoardState, a: Action): BoardState {
  switch (a.type) {
    case "snapshot": {
      const snapSeq = a.snap.seq ?? 0;
      // REST race guard: do not allow older snapshot updates to overwrite state
      // that has already advanced to a higher sequence number via live events.
      if (s.lastSeenSeq > 0 && snapSeq < s.lastSeenSeq) {
        return s;
      }

      const items = new Map(a.snap.items.map((i) => [i.id, i]));
      const stories = new Map(a.snap.goals.map((g) => [g.id, g.story]));
      return {
        ...s,
        items,
        stories,
        goals: a.snap.goals,
        serverTime: a.snap.server_time,
        agentTimeout: a.snap.agent_timeout_secs,
        defaultEngine: a.snap.default_engine ?? "",
        defaultModel: a.snap.default_model ?? "",
        loaded: true,
        lastLoadedAt: Date.now(),
        lastSeenSeq: Math.max(s.lastSeenSeq, snapSeq),
      };
    }
    case "event": {
      const evSeq = a.ev.seq ?? (s.lastSeenSeq + 1);
      // Ignore duplicate or out-of-order events with older/equal sequence numbers
      if (s.lastSeenSeq > 0 && evSeq <= s.lastSeenSeq) {
        return s;
      }

      const lastSeenSeq = Math.max(s.lastSeenSeq, evSeq);

      if (a.ev.type === "upsert") {
        const items = new Map(s.items);
        items.set(a.ev.item.id, a.ev.item);
        return { ...s, items, lastSeenSeq };
      }
      if (a.ev.type === "delete") {
        const items = new Map(s.items);
        items.delete(a.ev.id);
        return { ...s, items, lastSeenSeq };
      }
      if (a.ev.type === "story") {
        const stories = new Map(s.stories);
        const prev = stories.get(a.ev.goal) ?? [];
        stories.set(a.ev.goal, [...prev, { at: a.ev.at, text: a.ev.text }]);
        return { ...s, stories, lastSeenSeq };
      }
      return { ...s, lastSeenSeq };
    }
    case "connected":
      return { ...s, connected: a.ok };
  }
}

export function isSequenceGap(lastSeenSeq: number, incomingSeq: number): boolean {
  return lastSeenSeq > 0 && incomingSeq > lastSeenSeq + 1;
}

/** Past this with no successful load, what you are looking at is history. */
export const STALE_AFTER_MS = 12_000;

/**
 * Snapshot once, then apply deltas. Goal rollups are recomputed server-side on
 * a slower cadence — the deltas keep the cards live in between.
 *
 * A failed poll deliberately leaves the last snapshot on screen rather than
 * blanking the board — but it must then *say so*. Silently rendering stale
 * state as though it were current is the worst thing a control plane can do:
 * it looks healthy while you make decisions against a frozen picture.
 */
export function useBoard() {
  const [state, dispatch] = useReducer(reduce, initial);
  const [error, setError] = useState<string | null>(null);
  const wsRef = useRef<WebSocket | EventSource | null>(null);
  const lastSeenSeqRef = useRef<number>(0);
  const wasConnectedRef = useRef<boolean>(false);

  // Keep lastSeenSeqRef updated synchronously with state.lastSeenSeq
  useEffect(() => {
    lastSeenSeqRef.current = state.lastSeenSeq;
  }, [state.lastSeenSeq]);

  useEffect(() => {
    let alive = true;

    const load = () =>
      api
        .board()
        .then((snap) => {
          if (!alive) return;
          dispatch({ type: "snapshot", snap });
          setError(null);
        })
        .catch((e) => alive && setError(String(e)));

    load();

    let socket: WebSocket | EventSource | null = null;

    if (typeof WebSocket !== "undefined") {
      const protocol = typeof window !== "undefined" && window.location.protocol === "https:" ? "wss:" : "ws:";
      const host = typeof window !== "undefined" ? window.location.host : "localhost:8080";
      const wsUrl = `${protocol}//${host}/api/ws`;
      const ws = new WebSocket(wsUrl);
      wsRef.current = ws;
      socket = ws;

      ws.onopen = () => {
        if (!alive) return;
        dispatch({ type: "connected", ok: true });
        ws.send(JSON.stringify({ type: "subscribe", last_seq: lastSeenSeqRef.current || null }));
        if (wasConnectedRef.current) {
          load();
        }
        wasConnectedRef.current = true;
      };

      ws.onclose = () => {
        if (!alive) return;
        dispatch({ type: "connected", ok: false });
      };

      ws.onerror = () => {
        if (!alive) return;
        dispatch({ type: "connected", ok: false });
      };

      ws.onmessage = (m) => {
        if (!alive) return;
        try {
          const data = typeof m.data === "string" ? JSON.parse(m.data) : null;
          if (!data) return;

          if (data.type === "ping") {
            ws.send(JSON.stringify({ type: "pong" }));
            return;
          }
          if (data.type === "pong") {
            return;
          }

          const ev = data as BoardEvent;
          if (ev && typeof ev.seq === "number") {
            if (isSequenceGap(lastSeenSeqRef.current, ev.seq)) {
              load();
            }
          }
          if (ev && ev.type === "reset") {
            load();
          }
          dispatch({ type: "event", ev });
          emitBoardEvent(ev);
        } catch {
          /* ignore non-json frames */
        }
      };
    } else {
      const es = new EventSource("/api/events");
      wsRef.current = es;
      socket = es;

      es.onopen = () => {
        if (!alive) return;
        dispatch({ type: "connected", ok: true });
        if (wasConnectedRef.current) {
          load();
        }
        wasConnectedRef.current = true;
      };

      es.onerror = () => {
        if (!alive) return;
        dispatch({ type: "connected", ok: false });
      };

      es.onmessage = (m) => {
        if (!alive) return;
        try {
          const ev = JSON.parse(m.data) as BoardEvent;
          if (ev && typeof ev.seq === "number") {
            if (isSequenceGap(lastSeenSeqRef.current, ev.seq)) {
              load();
            }
          }
          if (ev && ev.type === "reset") {
            load();
          }
          dispatch({ type: "event", ev });
          emitBoardEvent(ev);
        } catch {
          /* keep-alive frames */
        }
      };
    }

    // Rollups (progress, chunk summaries, spend) are derived server-side, so
    // re-pull them periodically. Card state itself arrives over WebSocket.
    const poll = setInterval(load, 4000);

    return () => {
      alive = false;
      clearInterval(poll);
      if (socket) {
        socket.close();
      }
    };
  }, []);

  return {
    ...state,
    error,
    refresh: () => api.board().then((snap) => dispatch({ type: "snapshot", snap })),
  };
}

/** A ticking clock so relative times and countdowns stay honest between events. */
export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

