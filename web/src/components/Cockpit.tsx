import { useCallback, useEffect, useId, useRef, useState } from "react";
import { api } from "../api.js";
import { xtermThemeFromDocument } from "../theme.js";
import type { CockpitSession } from "../types.js";

const POLL_MS = 4000;

/** Why attach is locked — mirrors Board session readiness only. */
export function cockpitAttachGate(
  session: CockpitSession | null,
): { canAttach: boolean; reason: string | null } {
  if (session == null) {
    return { canAttach: false, reason: "Start a cockpit session to open the terminal." };
  }
  if (session.status === "parked") {
    return {
      canAttach: false,
      reason: "Cockpit session is parked. Stop it, then Start again.",
    };
  }
  const environment = session.environment?.trim();
  if (!environment) {
    return {
      canAttach: false,
      reason: "Waiting for the supervisor to provision the cockpit environment…",
    };
  }
  return { canAttach: true, reason: null };
}

/** @deprecated alias — prefer cockpitAttachGate */
export function cockpitChatGate(session: CockpitSession | null): {
  canSend: boolean;
  reason: string | null;
} {
  const g = cockpitAttachGate(session);
  return { canSend: g.canAttach, reason: g.reason };
}

function cockpitAttachWsUrl(): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/api/cockpit-attach`;
}

/** Exponential backoff for attach reconnect after honr/proxy drops the socket. */
export function cockpitAttachRetryDelayMs(attempt: number): number {
  const n = Math.max(0, Math.min(attempt, 5));
  return Math.min(1000 * 2 ** n, 15_000);
}

/**
 * Start / Stop only — exported so tests render without fetch.
 * Session metadata stays on the Board; Cockpit does not dump it.
 */
export function CockpitSessionView({
  session,
  busy,
  error,
  onStart,
  onStop,
}: {
  session: CockpitSession | null;
  busy?: boolean;
  error?: string | null;
  onStart: () => void;
  onStop: () => void;
}) {
  const absent = session == null;

  return (
    <div className="cockpit-session" data-testid="cockpit-session">
      {error && (
        <div className="err" data-testid="cockpit-session-error">
          {error}
        </div>
      )}
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
          className="danger"
          disabled={busy || absent}
          onClick={onStop}
          data-testid="cockpit-session-stop"
        >
          Stop
        </button>
      </div>
    </div>
  );
}

/**
 * Real attach face — xterm.js over `/api/cockpit-attach` (ExecSandboxInteractive).
 * SSR-safe: terminal + WebSocket only mount in the browser when attachable.
 */
export function CockpitAttachView({
  canAttach,
  disabledReason,
  environment,
  sessionStatus,
  reconnectKey = 0,
  /** When the drop re-opens, refit xterm (attach stays mounted while collapsed). */
  panelOpen = true,
}: {
  canAttach: boolean;
  disabledReason?: string | null;
  environment?: string | null;
  sessionStatus?: CockpitSession["status"] | null;
  /** Bump to force a fresh WebSocket (e.g. after Stop/Start). */
  reconnectKey?: number;
  panelOpen?: boolean;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const [attachError, setAttachError] = useState<string | null>(null);
  const [connected, setConnected] = useState(false);
  /** Internal remount counter — bumped on unexpected socket close for backoff retry. */
  const [attachGen, setAttachGen] = useState(0);
  const retryAttemptRef = useRef(0);
  const titleEnv = environment?.trim() || "cockpit";
  const titleStatus =
    sessionStatus === "parked"
      ? "parked"
      : sessionStatus === "running"
        ? connected
          ? "attached"
          : attachError
            ? "reconnecting…"
            : "connecting…"
        : "offline";

  // Parent Start/Stop (or env change) should not inherit a prior backoff streak.
  useEffect(() => {
    retryAttemptRef.current = 0;
    setAttachGen(0);
    setAttachError(null);
  }, [reconnectKey, environment, canAttach]);

  useEffect(() => {
    if (!canAttach || !hostRef.current) {
      setConnected(false);
      return;
    }

    // Dynamic import keeps SSR / node tests free of xterm's CJS surface.
    let disposed = false;
    let cleanup: (() => void) | undefined;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let reconnectScheduled = false;
    setAttachError(null);
    setConnected(false);

    const scheduleReconnect = (hint?: string) => {
      if (disposed || reconnectScheduled) return;
      reconnectScheduled = true;
      const attempt = retryAttemptRef.current;
      retryAttemptRef.current = attempt + 1;
      const delay = cockpitAttachRetryDelayMs(attempt);
      const secs = Math.max(1, Math.round(delay / 1000));
      setAttachError(
        hint
          ? `${hint} — retrying in ${secs}s…`
          : `attach disconnected — retrying in ${secs}s…`,
      );
      setConnected(false);
      retryTimer = setTimeout(() => {
        retryTimer = null;
        if (disposed) return;
        setAttachGen((g) => g + 1);
      }, delay);
    };

    // React StrictMode (Vite dev) mount→unmount→remounts effects. Opening the
    // attach WebSocket in the phantom first mount still hits the server, which
    // pkills the agent on the real remount — death spiral of exit 143. Defer
    // past that cleanup so only the surviving mount connects. Production
    // (:8080 embedded UI) does not double-invoke, which is why attach looked
    // "Vite-only broken".
    let startTimer: ReturnType<typeof setTimeout> | null = setTimeout(() => {
      startTimer = null;
      if (disposed) return;
      void (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import("@xterm/xterm"),
        import("@xterm/addon-fit"),
      ]);
      if (disposed || !hostRef.current) return;

      const host = hostRef.current;
      host.replaceChildren();

      const term = new Terminal({
        cursorBlink: true,
        fontSize: 13,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        theme: xtermThemeFromDocument(),
        allowProposedApi: true,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(host);
      fit.fit();
      if (disposed) {
        term.dispose();
        return;
      }

      let ws: WebSocket | null = null;
      try {
        ws = new WebSocket(cockpitAttachWsUrl());
        ws.binaryType = "arraybuffer";
      } catch (e) {
        scheduleReconnect(e instanceof Error ? e.message : String(e));
        term.dispose();
        return;
      }

      // After `ready`, agent stdout can lag a few seconds — animate in-terminal
      // so "attached" doesn't look like a dead TTY. Cleared on first PTY bytes.
      let awaitingAgent = false;
      let spinnerTimer: ReturnType<typeof setInterval> | null = null;
      let spinnerTick = 0;
      const stopAgentSpinner = (clearLine: boolean) => {
        if (spinnerTimer != null) {
          clearInterval(spinnerTimer);
          spinnerTimer = null;
        }
        if (awaitingAgent && clearLine) {
          term.write("\r\x1b[2K");
        }
        awaitingAgent = false;
      };
      const startAgentSpinner = () => {
        stopAgentSpinner(false);
        awaitingAgent = true;
        spinnerTick = 0;
        const paint = () => {
          if (!awaitingAgent || disposed) return;
          const dots = ".".repeat((spinnerTick % 3) + 1);
          term.write(`\r\x1b[2K\x1b[90mstarting agent${dots}\x1b[0m`);
          spinnerTick += 1;
        };
        paint();
        spinnerTimer = setInterval(paint, 400);
      };

      const sendResize = () => {
        fit.fit();
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(
            JSON.stringify({
              type: "resize",
              cols: term.cols,
              rows: term.rows,
            }),
          );
        }
      };

      // Apply after paint so --term-* have resolved under the new data-theme.
      // clearTextureAtlas + a one-row resize nudge refreshes Cursor's TUI chrome
      // (palette/truecolor cells) the way a hard reload would.
      let themeSyncRaf = 0;
      const syncTheme = () => {
        cancelAnimationFrame(themeSyncRaf);
        themeSyncRaf = requestAnimationFrame(() => {
          themeSyncRaf = requestAnimationFrame(() => {
            if (disposed) return;
            term.options.theme = { ...xtermThemeFromDocument() };
            term.clearTextureAtlas();
            term.refresh(0, Math.max(0, term.rows - 1));
            if (ws && ws.readyState === WebSocket.OPEN) {
              const cols = term.cols;
              const rows = Math.max(1, term.rows);
              ws.send(
                JSON.stringify({ type: "resize", cols, rows: rows + 1 }),
              );
              ws.send(JSON.stringify({ type: "resize", cols, rows }));
            }
          });
        });
      };
      const themeObs = new MutationObserver(syncTheme);
      themeObs.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme"],
      });

      ws.onopen = () => {
        if (disposed) return;
        // WS open ≠ agent ready — create-chat + exec_interactive still run.
        sendResize();
      };
      ws.onmessage = (ev) => {
        if (disposed) return;
        if (typeof ev.data === "string") {
          try {
            const msg = JSON.parse(ev.data) as {
              type?: string;
              message?: string;
              code?: number;
            };
            if (msg.type === "ready") {
              retryAttemptRef.current = 0;
              setAttachError(null);
              setConnected(true);
              sendResize();
              startAgentSpinner();
            } else if (msg.type === "error" && msg.message) {
              stopAgentSpinner(true);
              setAttachError(msg.message);
              term.writeln(`\r\n\x1b[31m${msg.message}\x1b[0m`);
            } else if (msg.type === "exit") {
              stopAgentSpinner(true);
              term.writeln(
                `\r\n\x1b[90m[shell exited${msg.code != null ? ` ${msg.code}` : ""}]\x1b[0m`,
              );
              setConnected(false);
              // Socket close follows; onclose schedules reconnect once.
            }
          } catch {
            /* ignore non-JSON control */
          }
          return;
        }
        stopAgentSpinner(true);
        const bytes =
          ev.data instanceof ArrayBuffer
            ? new Uint8Array(ev.data)
            : new Uint8Array(ev.data as ArrayBuffer);
        term.write(bytes);
      };
      // onerror is usually followed by onclose; reconnect there so we do not
      // stick a permanent "attach WebSocket error" after a honr restart.
      ws.onerror = () => {};
      ws.onclose = () => {
        stopAgentSpinner(true);
        if (disposed) return;
        setConnected(false);
        scheduleReconnect("attach WebSocket closed");
      };

      const dataSub = term.onData((data) => {
        if (ws && ws.readyState === WebSocket.OPEN) {
          ws.send(new TextEncoder().encode(data));
        }
      });

      const onWinResize = () => sendResize();
      window.addEventListener("resize", onWinResize);
      const ro =
        typeof ResizeObserver !== "undefined"
          ? new ResizeObserver(() => sendResize())
          : null;
      ro?.observe(host);

      cleanup = () => {
        stopAgentSpinner(false);
        cancelAnimationFrame(themeSyncRaf);
        themeObs.disconnect();
        window.removeEventListener("resize", onWinResize);
        ro?.disconnect();
        dataSub.dispose();
        // Prevent onclose from scheduling another retry while we tear down.
        if (ws) {
          ws.onclose = null;
          ws.onerror = null;
          ws.close();
        }
        term.dispose();
        setConnected(false);
      };
      })();
    }, 0);

    return () => {
      disposed = true;
      if (startTimer != null) clearTimeout(startTimer);
      if (retryTimer != null) clearTimeout(retryTimer);
      cleanup?.();
    };
  }, [canAttach, environment, reconnectKey, attachGen]);

  // Collapse only hides the drop — keep the WebSocket. Refit when shown again.
  useEffect(() => {
    if (!panelOpen || !canAttach) return;
    window.dispatchEvent(new Event("resize"));
  }, [panelOpen, canAttach]);

  return (
    <section
      className="cockpit-term"
      aria-labelledby={titleId}
      data-testid="cockpit-attach"
    >
      <div className="cockpit-term-window" data-testid="cockpit-term-window">
        <div className="cockpit-term-titlebar">
          <span className="cockpit-term-traffic" aria-hidden="true">
            <i /><i /><i />
          </span>
          <h2 id={titleId} className="cockpit-term-title">
            {titleEnv}
            <span className="cockpit-term-title-status"> — {titleStatus}</span>
          </h2>
        </div>

        {attachError && (
          <div className="err cockpit-term-error" data-testid="cockpit-attach-error">
            {attachError}
          </div>
        )}

        {!canAttach && (
          <p className="dim cockpit-term-gate" data-testid="cockpit-attach-gate">
            {disabledReason ?? "Start a cockpit session to attach."}
          </p>
        )}

        <div
          className="cockpit-xterm"
          ref={hostRef}
          data-testid="cockpit-xterm"
          // Keep a mount point even when gated so layout stays stable; effect
          // only opens the WebSocket when canAttach.
          style={{ display: canAttach ? undefined : "none" }}
        />
      </div>
    </section>
  );
}

/** Lucide-style chevrons-down — flips when the drop is open. */
function CockpitChevrons() {
  return (
    <svg
      className="cockpit-bar-icon"
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <path
        d="m7 6 5 5 5-5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="m7 13 5 5 5-5"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** Centered top-bar control — opens the Cockpit drop below the header. */
export function CockpitToggle({
  open,
  onToggle,
}: {
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={`cockpit-bar-btn${open ? " open" : ""}`}
      aria-expanded={open}
      aria-controls="cockpit-drop"
      aria-label={open ? "Collapse Cockpit" : "Open Cockpit"}
      title={open ? "Collapse Cockpit" : "Open Cockpit"}
      data-testid="cockpit-toggle"
      onClick={onToggle}
    >
      <CockpitChevrons />
    </button>
  );
}

/**
 * Panel under the top bar. Stays mounted after the first open so collapse does
 * not tear down the attach WebSocket / interactive agent. `shown` lags one
 * frame behind `open` so open/close both slide via CSS.
 */
export function CockpitDrop({ open }: { open: boolean }) {
  const [kept, setKept] = useState(false);
  const [shown, setShown] = useState(false);

  useEffect(() => {
    if (open) {
      setKept(true);
      const id = requestAnimationFrame(() => setShown(true));
      return () => cancelAnimationFrame(id);
    }
    setShown(false);
  }, [open]);

  if (!open && !kept) return null;

  return (
    <section
      id="cockpit-drop"
      className={`cockpit-drop${shown ? " open" : ""}`}
      data-testid="cockpit-drop"
      aria-label="Cockpit"
      aria-hidden={!open}
      inert={!open || undefined}
    >
      <div className="cockpit-drop-inner">
        <Cockpit panelOpen={open} />
      </div>
    </section>
  );
}

/**
 * Cockpit — Start/Stop the Board cockpit session; terminal attaches when ready.
 * MCP inject stays silent in the background.
 */
export function Cockpit({ panelOpen = true }: { panelOpen?: boolean } = {}) {
  const [session, setSession] = useState<CockpitSession | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reconnectKey, setReconnectKey] = useState(0);
  const provisionedEnv = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const out = await api.getCockpitSession();
      setSession(out.session ?? null);
      setError(null);
      if (!out.session) {
        provisionedEnv.current = null;
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const provisionMcp = useCallback(async () => {
    const out = await api.provisionCockpitMcp();
    provisionedEnv.current = out.environment;
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

  // When the supervisor fills environment, inject MCP once for this env.
  useEffect(() => {
    const env = session?.environment?.trim();
    if (
      session?.status === "running" &&
      env &&
      provisionedEnv.current !== env
    ) {
      void provisionMcp().catch(() => {
        /* attach still works; next Start retries inject */
      });
    }
  }, [session?.status, session?.environment, provisionMcp]);

  const runAction = useCallback(
    async (action: () => Promise<unknown>, opts?: { reconnect?: boolean }) => {
      setBusy(true);
      setError(null);
      try {
        await action();
        await refresh();
        if (opts?.reconnect) setReconnectKey((k) => k + 1);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        await refresh();
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const gate = cockpitAttachGate(session);

  return (
    <div className="cockpit-pane" data-testid="cockpit-pane">
      <CockpitSessionView
        session={session}
        busy={busy}
        error={error}
        onStart={() =>
          void runAction(() => api.startCockpitSession(), { reconnect: true })
        }
        onStop={() =>
          void runAction(async () => {
            provisionedEnv.current = null;
            await api.stopCockpitSession();
          })
        }
      />

      <CockpitAttachView
        canAttach={gate.canAttach}
        disabledReason={gate.reason}
        environment={session?.environment}
        sessionStatus={session?.status ?? null}
        reconnectKey={reconnectKey}
        panelOpen={panelOpen}
      />
    </div>
  );
}
