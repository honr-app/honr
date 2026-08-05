/** Shared view ids for the persistent Board | Cockpit | Help | Settings chrome. */
export type AppView = "board" | "cockpit" | "help" | "settings";

export interface PrimarySidebarProps {
  view: AppView;
  onNavigate: (view: AppView) => void;
}

/** Persistent primary nav — Board + Cockpit are primary; Help + Settings secondary. */
export function PrimarySidebar({ view, onNavigate }: PrimarySidebarProps) {
  return (
    <nav className="sidebar" aria-label="Primary" data-testid="app-sidebar">
      <button
        type="button"
        className={`sidebar-btn ${view === "board" ? "active" : ""}`}
        aria-current={view === "board" ? "page" : undefined}
        onClick={() => onNavigate("board")}
        data-testid="nav-board"
      >
        Board
      </button>
      <button
        type="button"
        className={`sidebar-btn ${view === "cockpit" ? "active" : ""}`}
        aria-current={view === "cockpit" ? "page" : undefined}
        onClick={() => onNavigate("cockpit")}
        data-testid="nav-cockpit"
      >
        Cockpit
      </button>
      <button
        type="button"
        className={`sidebar-btn ${view === "help" ? "active" : ""}`}
        aria-current={view === "help" ? "page" : undefined}
        onClick={() => onNavigate("help")}
        data-testid="nav-help"
      >
        Help
      </button>
      <button
        type="button"
        className={`sidebar-btn ${view === "settings" ? "active" : ""}`}
        aria-current={view === "settings" ? "page" : undefined}
        onClick={() => onNavigate("settings")}
        data-testid="nav-settings"
      >
        Settings
      </button>
    </nav>
  );
}
