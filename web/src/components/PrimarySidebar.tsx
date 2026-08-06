/** Shared view ids for the persistent Board | Help chrome. Settings lives in the account menu. */
export type AppView = "board" | "help" | "settings";

export interface PrimarySidebarProps {
  view: AppView;
  onNavigate: (view: AppView) => void;
}

/** Persistent primary nav — Board and Help. Settings is under the account menu. */
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
        className={`sidebar-btn ${view === "help" ? "active" : ""}`}
        aria-current={view === "help" ? "page" : undefined}
        onClick={() => onNavigate("help")}
        data-testid="nav-help"
      >
        Help
      </button>
    </nav>
  );
}
