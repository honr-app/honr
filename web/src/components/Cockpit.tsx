import { useCallback, useEffect, useState } from "react";
import { api } from "../api.js";
import type { OpsSession } from "../types.js";

const POLL_MS = 4000;

function statusLabel(status: OpsSession["status"]): string {
  return status === "parked" ? "Parked" : "Running";
}

/**
 * Presentational ops-session face — exported so tests render without fetch.
 * Action enablement mirrors Board rules; mutations stay on the parent.
 */
export function CockpitSessionView({
  session,
  loading,
  busy,
  error,
  onStart,
  onPark,
  onResume,
  onStop,
}: {
  session: OpsSession | null;
  loading?: boolean;
  busy?: boolean;
  error?: string | null;
  onStart: () => void;
  onPark: () => void;
  onResume: () => void;
  onStop: () => void;
}) {
  const absent = session == null;
  const running = session?.status === "running";
  const parked = session?.status === "parked";
  const environment = session?.environment?.trim() || null;
  const conversationId = session?.conversation_id?.trim() || null;

  return (
    <section
      className="cockpit-session"
      aria-labelledby="cockpit-session-title"
      data-testid="cockpit-session"
    >
      <h2 id="cockpit-session-title">Ops session</h2>
      <p className="dim cockpit-session-lede">
        Board owns lifecycle. These controls call{" "}
        <code>/api/ops-session*</code> only — status below is polled from the
        Board, not a second state machine.
      </p>

      {error && (
        <div className="err" data-testid="cockpit-session-error">
          {error}
        </div>
      )}

      <dl className="cockpit-session-status" data-testid="cockpit-session-status">
        <div>
          <dt>Status</dt>
          <dd data-testid="cockpit-session-status-value">
            {loading && absent
              ? "Loading…"
              : absent
                ? "None"
                : statusLabel(session.status)}
          </dd>
        </div>
        <div>
          <dt>Environment</dt>
          <dd data-testid="cockpit-session-environment">
            {environment ?? "—"}
          </dd>
        </div>
        {conversationId && (
          <div>
            <dt>Conversation</dt>
            <dd data-testid="cockpit-session-conversation">{conversationId}</dd>
          </div>
        )}
      </dl>

      <div className="cockpit-session-actions" data-testid="cockpit-session-actions">
        <button
          type="button"
          className="primary"
          disabled={busy || !absent}
          onClick={onStart}
          data-testid="cockpit-session-start"
        >
          Start
        </button>
        <button
          type="button"
          disabled={busy || !running}
          onClick={onPark}
          data-testid="cockpit-session-park"
        >
          Park
        </button>
        <button
          type="button"
          disabled={busy || !parked}
          onClick={onResume}
          data-testid="cockpit-session-resume"
        >
          Resume
        </button>
        <button
          type="button"
          className="danger"
          disabled={busy || absent}
          onClick={onStop}
          data-testid="cockpit-session-stop"
        >
          Stop
        </button>
      </div>

      {environment && (
        <p className="dim cockpit-attach-fallback" data-testid="cockpit-attach-fallback">
          Optional TTY fallback:{" "}
          <code>openshell sandbox connect {environment}</code>
        </p>
      )}
    </section>
  );
}

/**
 * Cockpit — primary-nav surface for the ops-seat control plane.
 * Thin face over Board /api/ops-session*: poll status, REST for Start / Park /
 * Resume / Stop. No local session files or shadow lifecycle.
 */
export function Cockpit() {
  const [session, setSession] = useState<OpsSession | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const out = await api.getOpsSession();
      setSession(out.session ?? null);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    const tick = () => {
      if (!alive) return;
      void refresh();
    };
    tick();
    const poll = setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      clearInterval(poll);
    };
  }, [refresh]);

  const runAction = useCallback(
    async (action: () => Promise<unknown>) => {
      setBusy(true);
      setError(null);
      try {
        await action();
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        // Board may still have moved — refetch so the face stays honest.
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  return (
    <div className="cockpit-page" data-testid="cockpit-page">
      <header className="board-hero">
        <h1>Cockpit</h1>
        <p className="board-lede">
          Ops seat control plane: live session status and Start / Park / Resume
          / Stop against the Board. Optional{" "}
          <code>openshell sandbox connect</code> stays a TTY fallback — not an
          in-browser terminal.
        </p>
      </header>

      <CockpitSessionView
        session={session}
        loading={loading}
        busy={busy}
        error={error}
        onStart={() => void runAction(() => api.startOpsSession())}
        onPark={() => void runAction(() => api.parkOpsSession())}
        onResume={() => void runAction(() => api.resumeOpsSession())}
        onStop={() => void runAction(() => api.stopOpsSession())}
      />
    </div>
  );
}
