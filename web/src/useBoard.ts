import { useEffect, useReducer, useRef, useState } from "react";
import { api } from "./api";
import type { BoardEvent, GoalView, Snapshot, StoryLine, WorkItem } from "./types";

interface BoardState {
  items: Map<number, WorkItem>;
  goals: GoalView[];
  stories: Map<number, StoryLine[]>;
  serverTime: string | null;
  heartbeatExpect: number;
  loaded: boolean;
  connected: boolean;
}

type Action =
  | { type: "snapshot"; snap: Snapshot }
  | { type: "event"; ev: BoardEvent }
  | { type: "connected"; ok: boolean };

const initial: BoardState = {
  items: new Map(),
  goals: [],
  stories: new Map(),
  serverTime: null,
  heartbeatExpect: 6,
  loaded: false,
  connected: false,
};

function reduce(s: BoardState, a: Action): BoardState {
  switch (a.type) {
    case "snapshot": {
      const items = new Map(a.snap.items.map((i) => [i.id, i]));
      const stories = new Map(a.snap.goals.map((g) => [g.id, g.story]));
      return {
        ...s,
        items,
        stories,
        goals: a.snap.goals,
        serverTime: a.snap.server_time,
        heartbeatExpect: a.snap.heartbeat_expect_secs,
        loaded: true,
      };
    }
    case "event": {
      if (a.ev.type === "upsert") {
        const items = new Map(s.items);
        items.set(a.ev.item.id, a.ev.item);
        return { ...s, items };
      }
      const stories = new Map(s.stories);
      const prev = stories.get(a.ev.goal) ?? [];
      stories.set(a.ev.goal, [...prev, { at: a.ev.at, text: a.ev.text }]);
      return { ...s, stories };
    }
    case "connected":
      return { ...s, connected: a.ok };
  }
}

/**
 * Snapshot once, then apply deltas. Goal rollups are recomputed server-side on
 * a slower cadence — the deltas keep the cards live in between.
 */
export function useBoard() {
  const [state, dispatch] = useReducer(reduce, initial);
  const [error, setError] = useState<string | null>(null);
  const esRef = useRef<EventSource | null>(null);

  useEffect(() => {
    let alive = true;

    const load = () =>
      api
        .board()
        .then((snap) => alive && dispatch({ type: "snapshot", snap }))
        .catch((e) => alive && setError(String(e)));

    load();

    const es = new EventSource("/api/events");
    esRef.current = es;
    es.onopen = () => dispatch({ type: "connected", ok: true });
    es.onerror = () => dispatch({ type: "connected", ok: false });
    es.onmessage = (m) => {
      try {
        dispatch({ type: "event", ev: JSON.parse(m.data) as BoardEvent });
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

  return { ...state, error, refresh: () => api.board().then((snap) => dispatch({ type: "snapshot", snap })) };
}

/** A ticking clock so relative times ("♥ 4s") stay honest between events. */
export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}
