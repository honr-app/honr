# Plan: Revamp Board WebSocket Sync

## Context & Objectives

Currently in `honr`, live board updates rely on a server-sent events stream (`/api/events`) and periodic REST polling (`/api/board`). In practice, several key issues cause the UI to become stale or out of sync:
1. **Missed Events & Reconnect Gaps**: When a client temporarily loses connection or experiences network jitter, events broadcast by `b.subscribe()` during the outage are dropped. Upon reconnecting, `EventSource` resumes listening without receiving missed events, leaving local state out of date.
2. **Stale Detail Drawer State**: When a user opens the card detail drawer (`DetailDrawer`), it fetches card details once from `/api/items/${id}`. Subsequent board events (state transitions, notes, PR link updates, agent progress, or plan proposals) update the main board store but do not update the drawer, forcing the user to hard-refresh or close and reopen the drawer.
3. **REST vs Stream Race Conditions**: `useBoard` polls `/api/board` every 4 seconds while processing real-time events. An in-flight REST snapshot response returning out-of-order after a newer live event has been processed can overwrite updated cards with older snapshot data.
4. **Uni-directional Transport Limitations**: SSE does not support client-initiated messages (e.g. requesting missed sequence ranges or sending ping/pong heartbeats).

The objective of **«Revamp board WebSocket sync»** is to build a reliable real-time synchronization layer that guarantees board and detail drawer state accuracy without requiring hard refreshes.

---

## Architectural Changes & Protocol Rules

### 1. Server Event Sequencing & Reconnection Catch-Up Buffer (`src/events.rs`, `src/store.rs`, `src/sse.rs` / `src/ws.rs`)
- **Monotonic Sequence Numbers**: Ensure every `BoardEvent` emitted by `Board::emit` carries an strictly increasing sequence number (`seq`).
- **Ring Buffer for Event Catch-Up**: Maintain a bounded, thread-safe event history ring buffer (e.g. last 100 events) on `Board`/`SharedBoard`.
- **Catch-Up & Replay Logic**: Allow live stream endpoints to accept `last_seq`. If the requested `last_seq` is present in the ring buffer, replay all subsequent events before streaming live events. If `last_seq` is too old or missing, emit a `Reset` event instructing the client to perform a full state snapshot fetch.

### 2. Client Sequence Tracking & Snapshot Race Guard (`web/src/useBoard.ts`, `web/src/types.ts`)
- **Sequence Tracking & Gap Detection**: `useBoard` records `lastSeenSeq`. If an incoming event has `seq > lastSeenSeq + 1`, a gap is detected: `useBoard` immediately requests a full snapshot re-fetch to restore state consistency.
- **REST Snapshot Race Protection**: Every `/api/board` snapshot response includes `server_seq` (or max item `seq`). If a REST snapshot finishes after newer live events (where `lastSeenSeq > snapshot.server_seq`), cards that received newer events are preserved rather than overwritten by stale snapshot data.

### 3. Real-Time Detail Drawer Synchronization (`web/src/components/Detail.tsx`)
- **Live Event Subscription in DetailDrawer**: Update `DetailDrawer` to react to `BoardEvent`s emitted by `useBoard`.
- **Card Merge on Upsert/Delete**: When an `Upsert` event arrives matching the drawer's `id`, `DetailDrawer` merges the updated fields (`state`, `title`, `pr_url`, `notes`, `proposal`, `history`, `progress`, `gates`) into its state without closing or resetting user edits.

### 4. WebSocket Live Transport & Bi-directional Protocol (`src/api.rs`, `src/ws.rs`, `web/src/useBoard.ts`, `web/vite.config.ts`)
- **Axum WebSocket Route**: Implement `/api/ws` using Axum WebSockets (`axum::extract::ws::WebSocketUpgrade`).
- **Bi-directional Protocol**: Client sends `{"type": "subscribe", "last_seq": N}` upon connection. Server responds with catch-up events or a reset directive, followed by live events. Include ping/pong frames to detect broken connections promptly.
- **Vite Proxy Support**: Update `web/vite.config.ts` to proxy `/api/ws` with `ws: true`.

---

## Tasks & Dependencies

```
[Task 1: Server Event Sequence & Catch-up] ───► [Task 2: Client Gap Detection & Race Guard] ───► [Task 3: Detail Drawer Sync]
                  │
                  └──────────────────────────────► [Task 4: WebSocket Transport Upgrade]
```

### Task 1: Server Event Sequencing & Reconnection Catch-up Buffer
- **Key**: `t1`
- **Intent**: Ensure every board event carries a monotonic sequence number and is stored in a server event ring buffer so reconnecting clients can request missed events without losing state.
- **Definition of Done**: `cargo test --offline --locked` and `cargo clippy --offline -- -D warnings` pass cleanly; `Board` maintains an event history buffer and monotonically increasing `seq`; event stream endpoint accepts `last_seq` and replays missed events or returns a `Reset` frame if lagged beyond buffer capacity; unit tests in `src/store.rs` verify sequence ordering and buffer catch-up.

### Task 2: Client Store Sequence Tracking, Reconnect Recovery & REST Race Guard
- **Key**: `t2`
- **Blocked By**: `t1`
- **Intent**: Ensure `useBoard` tracks `lastSeenSeq`, detects sequence gaps, re-syncs snapshot upon gaps or reconnects, and guards against stale REST snapshot responses overwriting newer live events.
- **Definition of Done**: `npm --prefix web test` (or `node web/test-card.mjs`) passes; `useBoard.ts` tracks `lastSeenSeq` from `BoardEvent`s; sequence gaps trigger snapshot re-fetches; snapshot updates with older sequence numbers than `lastSeenSeq` do not overwrite newer event state; unit tests in `web/test-card.mjs` verify gap handling and race protection.

### Task 3: Real-Time Detail Drawer Synchronization
- **Key**: `t3`
- **Blocked By**: `t2`
- **Intent**: Keep the card detail drawer (`DetailDrawer`) in sync with server state in real-time when cards transition, write notes, attach PR links, update proposals, or report progress.
- **Definition of Done**: `npm --prefix web test` passes; opening `DetailDrawer` listens for board events for card `id`; incoming `Upsert` events for `id` update drawer state live without manual page reload; closing drawer cleanly unsubscribes; unit test in `web/test-card.mjs` asserts `Detail` updates live upon receiving an `Upsert` event.

### Task 4: WebSocket Live Transport & Bi-directional Protocol
- **Key**: `t4`
- **Blocked By**: `t1`, `t2`
- **Intent**: Upgrade board live event transport from uni-directional SSE (`/api/events`) to Axum WebSockets (`/api/ws`) with bi-directional ping/pong keepalives and client-initiated sequence catch-up requests.
- **Definition of Done**: `cargo test --offline --locked` and `npm --prefix web test` pass; Axum handles `/api/ws` WebSocket connections; `useBoard.ts` connects via `WebSocket`, sends `subscribe` with `last_seq`, and handles ping/pong; `vite.config.ts` proxies `/api/ws` with `ws: true`; server and client handle reconnects cleanly.
