/** Stored preference — what the dropdown shows. */
export type ThemePreference = "system" | "light" | "dark";

/** Concrete theme applied to `document.documentElement.dataset.theme`. */
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "honr-theme";

export function systemResolvedTheme(): ResolvedTheme {
  if (typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches) {
    return "dark";
  }
  return "light";
}

export function resolveTheme(preference: ThemePreference): ResolvedTheme {
  return preference === "system" ? systemResolvedTheme() : preference;
}

export function readThemePreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "system" || stored === "light" || stored === "dark") return stored;
    // Migrate older light/dark-only storage (already covered above).
  } catch {
    /* private mode */
  }
  return "system";
}

export function applyThemePreference(preference: ThemePreference) {
  const resolved = resolveTheme(preference);
  document.documentElement.dataset.theme = resolved;
  try {
    localStorage.setItem(STORAGE_KEY, preference);
  } catch {
    /* ignore */
  }
}
