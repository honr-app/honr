import { useEffect, useId, useRef, useState } from "react";
import type { ThemePreference } from "../theme";

export interface AccountMenuProps {
  login: string;
  isAdmin?: boolean;
  themePref: ThemePreference;
  onThemeChange: (pref: ThemePreference) => void;
  onOpenSettings: () => void;
  onLogout: () => void;
  /** Test/SSR helper — production always starts closed. */
  defaultOpen?: boolean;
}

/** Username trigger → Settings, theme, sign out. */
export function AccountMenu({
  login,
  isAdmin,
  themePref,
  onThemeChange,
  onOpenSettings,
  onLogout,
  defaultOpen = false,
}: AccountMenuProps) {
  const [open, setOpen] = useState(defaultOpen);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const onPointer = (ev: MouseEvent) => {
      if (!rootRef.current?.contains(ev.target as Node)) setOpen(false);
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const label = isAdmin ? `${login} (admin)` : login;

  return (
    <div className="account-menu" ref={rootRef}>
      <button
        type="button"
        className="account-menu-trigger"
        aria-expanded={open}
        aria-haspopup="menu"
        aria-controls={menuId}
        data-testid="auth-user"
        onClick={() => setOpen((was) => !was)}
      >
        <span className="account-menu-name">{label}</span>
        <span className="account-menu-caret" aria-hidden="true" />
      </button>
      {open && (
        <div
          className="account-menu-panel"
          id={menuId}
          role="menu"
          data-testid="account-menu"
        >
          <div className="account-menu-theme" role="none">
            <label htmlFor={`${menuId}-theme`}>Theme</label>
            <select
              id={`${menuId}-theme`}
              className="account-menu-theme-select"
              value={themePref}
              aria-label="Color theme"
              onChange={(e) =>
                onThemeChange(e.target.value as ThemePreference)
              }
              onClick={(e) => e.stopPropagation()}
            >
              <option value="system">System</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </div>
          <button
            type="button"
            className="account-menu-item"
            role="menuitem"
            data-testid="nav-settings"
            onClick={() => {
              setOpen(false);
              onOpenSettings();
            }}
          >
            Settings
          </button>
          <button
            type="button"
            className="account-menu-item account-menu-danger"
            role="menuitem"
            data-testid="auth-logout"
            onClick={() => {
              setOpen(false);
              onLogout();
            }}
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}
