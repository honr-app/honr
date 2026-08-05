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

/** What `data-theme` currently resolves to on the document. */
export function readDocumentTheme(): ResolvedTheme {
  if (typeof document === "undefined") return "light";
  return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
}

/** xterm.js ITheme fields — sourced from `--term-*` CSS vars when possible. */
export type XtermTheme = {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
};

function cssVar(name: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  const v = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return v || fallback;
}

/** Build an xterm theme from the live site CSS variables. */
export function xtermThemeFromDocument(): XtermTheme {
  const bg = cssVar("--term-bg", "#eef3ef");
  const fg = cssVar("--term-fg", "#29353c");
  const cursor = cssVar("--term-cursor", "#2377d2");
  return {
    background: bg,
    foreground: fg,
    cursor,
    cursorAccent: bg,
    selectionBackground: cssVar("--term-selection", "rgba(35, 119, 210, 0.28)"),
    black: cssVar("--term-black", "#29353c"),
    red: cssVar("--term-red", "#dd5942"),
    green: cssVar("--term-green", "#19874d"),
    yellow: cssVar("--term-yellow", "#c9a227"),
    blue: cssVar("--term-blue", "#2377d2"),
    magenta: cssVar("--term-magenta", "#a34b2e"),
    cyan: cssVar("--term-cyan", "#5a7a88"),
    white: cssVar("--term-white", "#eef3ef"),
    brightBlack: cssVar("--term-bright-black", "#8a9297"),
    brightRed: cssVar("--term-red", "#dd5942"),
    brightGreen: cssVar("--term-green", "#19874d"),
    brightYellow: cssVar("--term-yellow", "#c9a227"),
    brightBlue: cssVar("--term-blue", "#2377d2"),
    brightMagenta: cssVar("--term-magenta", "#a34b2e"),
    brightCyan: cssVar("--term-cyan", "#5a7a88"),
    brightWhite: cssVar("--term-bright-white", "#ffffff"),
  };
}
