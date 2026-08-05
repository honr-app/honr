import { useCallback, useEffect, useMemo, useState } from "react";
import { api, AuthRequiredError } from "./api.js";
import { Board } from "./components/Board";
import { CockpitDrop, CockpitToggle } from "./components/Cockpit";
import { DetailDrawer } from "./components/Detail";
import { Help } from "./components/Help";
import { Login } from "./components/Login";
import { PrimarySidebar, type AppView } from "./components/PrimarySidebar";
import { Settings } from "./components/Settings";
import { STALE_AFTER_MS, useBoard, useNow } from "./useBoard";
import type { AuthStatus, WorkItem } from "./types";
import {
  applyThemePreference,
  readThemePreference,
  type ThemePreference,
} from "./theme";

export default function App() {
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  const [authLoading, setAuthLoading] = useState(true);
  const [authError, setAuthError] = useState<string | null>(null);

  const refreshAuth = useCallback(() => {
    setAuthLoading(true);
    return api
      .getAuthStatus()
      .then((st) => {
        setAuth(st);
        setAuthError(null);
      })
      .catch((e) => setAuthError(String(e)))
      .finally(() => setAuthLoading(false));
  }, []);

  useEffect(() => {
    refreshAuth();
  }, [refreshAuth]);

  const needsLogin = !authLoading && (!auth?.user || auth.bootstrap);

  if (authLoading) {
    return (
      <div className="login-shell">
        <p className="dim">loading…</p>
      </div>
    );
  }

  if (authError && !auth) {
    return (
      <div className="login-shell">
        <div className="err">{authError}</div>
        <button type="button" className="primary" onClick={() => refreshAuth()}>
          Retry
        </button>
      </div>
    );
  }

  if (needsLogin && auth) {
    return (
      <Login
        status={auth}
        onAuthed={(next) => {
          setAuth(next);
        }}
      />
    );
  }

  return (
    <AuthedApp
      auth={auth!}
      onLogout={() => {
        api
          .logout()
          .catch(() => {})
          .finally(() => refreshAuth());
      }}
      onAuthLost={() => refreshAuth()}
    />
  );
}

function AuthedApp({
  auth,
  onLogout,
  onAuthLost,
}: {
  auth: AuthStatus;
  onLogout: () => void;
  onAuthLost: () => void;
}) {
  const b = useBoard();
  const now = useNow();
  const [open, setOpen] = useState<number | null>(null);
  const [view, setView] = useState<AppView>("board");
  const [cockpitOpen, setCockpitOpen] = useState(false);
  const [themePref, setThemePref] = useState<ThemePreference>(() =>
    readThemePreference(),
  );

  useEffect(() => {
    applyThemePreference(themePref);
    if (themePref !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyThemePreference("system");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [themePref]);

  useEffect(() => {
    if (b.error && /authentication required/i.test(b.error)) {
      onAuthLost();
    }
  }, [b.error, onAuthLost]);

  useEffect(() => {
    const onUnhandled = (ev: PromiseRejectionEvent) => {
      if (ev.reason instanceof AuthRequiredError) {
        onAuthLost();
      }
    };
    window.addEventListener("unhandledrejection", onUnhandled);
    return () => window.removeEventListener("unhandledrejection", onUnhandled);
  }, [onAuthLost]);

  const { goalOf, breadcrumbOf } = useMemo(() => buildLookups(b.items), [b.items]);

  const activeGoals = b.goals.filter((g) => b.items.get(g.id)?.state !== "retired");
  const totalNeedsYou = activeGoals.reduce((n, g) => n + g.needs_you, 0);
  const live = activeGoals.reduce((n, g) => n + g.agents_live, 0);

  const age = b.lastLoadedAt === null ? null : now - b.lastLoadedAt;
  const staleFor = age !== null && age > STALE_AFTER_MS ? age : null;

  return (
    <div className="app">
      <header className="top">
        <div className="brand">
          <button
            type="button"
            className="brand-home"
            onClick={() => {
              setOpen(null);
              setView("board");
            }}
            aria-label="Go to board"
            data-testid="brand-mark"
          >
            <img
              src="/honr-logo.png"
              alt=""
              className="brand-mark"
              width={44}
              height={44}
            />
          </button>
          {totalNeedsYou > 0 && <span className="pip">{totalNeedsYou}</span>}
        </div>
        <div className="cockpit-bar-slot">
          <CockpitToggle
            open={cockpitOpen}
            onToggle={() => setCockpitOpen((was) => !was)}
          />
        </div>
        <div className="stats">
          <span className="live">{live} working</span>
          <span className={b.connected ? "conn ok" : "conn off"}>
            {b.connected ? "live" : "reconnecting…"}
          </span>
          <span className="dim" data-testid="auth-user">
            {auth.user?.login}
            {auth.user?.kind === "admin" ? " (admin)" : ""}
          </span>
          <button
            type="button"
            className="link"
            onClick={onLogout}
            data-testid="auth-logout"
          >
            Sign out
          </button>
          <label className="theme-picker">
            <span className="dim">Theme</span>
            <select
              className="theme-toggle"
              value={themePref}
              aria-label="Color theme"
              onChange={(e) =>
                setThemePref(e.target.value as ThemePreference)
              }
            >
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </label>
        </div>
      </header>

      <CockpitDrop open={cockpitOpen} />

      {staleFor !== null && (
        <div className="err banner">
          ⚠ NOT LIVE — showing state from {Math.round(staleFor / 1000)}s ago.
          honr is unreachable; nothing here is current.
          <button className="link" onClick={b.refresh}>
            retry now
          </button>
        </div>
      )}
      {b.error && staleFor === null && <div className="err banner">{b.error}</div>}

      <div className="shell">
        <PrimarySidebar
          view={view}
          onNavigate={(next) => {
            if (next !== "board") setOpen(null);
            setView(next);
          }}
        />

        <main className={view === "board" && open != null ? "with-side-panes" : ""}>
          {view === "board" ? (
            <>
              {!b.loaded ? (
                <div className="dim pad">loading…</div>
              ) : (
                <Board
                  goals={b.goals}
                  items={b.items}
                  stories={b.stories}
                  goalOf={goalOf}
                  breadcrumbOf={breadcrumbOf}
                  now={now}
                  agentTimeout={b.agentTimeout}
                  defaultEngine={b.defaultEngine}
                  defaultModel={b.defaultModel}
                  onOpen={setOpen}
                  onChanged={b.refresh}
                />
              )}

              {open != null && (
                <DetailDrawer
                  id={open}
                  now={now}
                  onClose={() => setOpen(null)}
                  onChanged={b.refresh}
                  defaultEngine={b.defaultEngine}
                  defaultModel={b.defaultModel}
                />
              )}
            </>
          ) : view === "help" ? (
            <Help />
          ) : (
            <Settings />
          )}
        </main>
      </div>
    </div>
  );
}

function buildLookups(items: Map<number, WorkItem>) {
  const chainOf = (id: number): number[] => {
    const out: number[] = [];
    let cur: number | null | undefined = id;
    while (cur != null && out.length < 32) {
      out.push(cur);
      cur = items.get(cur)?.parent ?? null;
    }
    return out.reverse();
  };

  const goalOf = (id: number) => {
    const c = chainOf(id);
    return c[0] ?? id;
  };

  const breadcrumbOf = (id: number) => {
    const c = chainOf(id);
    const parent = c[c.length - 2];
    return parent != null ? (items.get(parent)?.title ?? "") : "";
  };

  return { goalOf, breadcrumbOf };
}
