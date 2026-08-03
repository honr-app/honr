import { useEffect, useReducer, useRef, useState } from "react";
import { api } from "./api.js";
import type { BoardEvent, GoalView, Snapshot, StoryLine, WorkItem } from "./types.js";

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
  const esRef = useRef<EventSource | null>(null);
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

    const es = new EventSource("/api/events");
    esRef.current = es;

    es.onopen = () => {
      dispatch({ type: "connected", ok: true });
      // Reconnect recovery: re-sync snapshot upon reconnecting after a drop
      if (wasConnectedRef.current) {
        load();
      }
      wasConnectedRef.current = true;
    };

    es.onerror = () => {
      dispatch({ type: "connected", ok: false });
    };

    es.onmessage = (m) => {
      try {
        const ev = JSON.parse(m.data) as BoardEvent;
        if (ev && typeof ev.seq === "number") {
          // Detect sequence gap: if incoming event seq > lastSeenSeq + 1, trigger snapshot re-fetch
          if (isSequenceGap(lastSeenSeqRef.current, ev.seq)) {
            load();
          }
        }
        dispatch({ type: "event", ev });
      } catch {
        /* keep-alive frames */
      }
    };

    // Rollups (progress, chunk summaries, spend) are derived server-side, so
    // re-pull them periodically. Card state itself arrives over SSE.
    const poll = setInterval(load, 4000);

    return () => {
      alive = false;
      clearInterval(poll);
      es.close();
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

