import type { AppView } from "./components/PrimarySidebar";
import type { OpenShellTab } from "./components/OpenShellSettings";

/** Settings nav sections mirrored under `/settings/...`. */
export type SettingsSection =
  | "openshell"
  | "access"
  | "workspace"
  | "agent-runtime";

export const DEFAULT_SETTINGS_SECTION: SettingsSection = "openshell";
export const DEFAULT_OPENSHELL_TAB: OpenShellTab = "connectivity";

const SETTINGS_SECTIONS = new Set<string>([
  "openshell",
  "access",
  "workspace",
  "agent-runtime",
]);

const OPENSHELL_TABS = new Set<string>([
  "connectivity",
  "providers",
  "provider-types",
  "policies",
  "mcp-servers",
  "profiles",
]);

/** Chrome + Settings location mirrored in the URL. */
export type ChromeLocation = {
  view: AppView;
  /** Open DetailDrawer card; only meaningful when `view === "board"`. */
  cardId: number | null;
  /** Settings section; only meaningful when `view === "settings"`. */
  settingsSection: SettingsSection;
  /** OpenShell sub-tab; only meaningful when settings + openshell. */
  openShellTab: OpenShellTab;
};

/**
 * Path contract (works with ServeDir → index.html SPA fallback):
 *   `/`                                  → board
 *   `/help`                              → help
 *   `/settings`                          → settings / OpenShell / Connectivity
 *   `/settings/openshell`                → settings / OpenShell / Connectivity
 *   `/settings/openshell/:tab`           → settings / OpenShell / tab
 *   `/settings/:section`                 → settings / section
 *   `/settings/github-app`               → redirect target: OpenShell / Providers
 *   `/card/:id`                          → board with DetailDrawer open on that card
 */
export function parseChromeLocation(pathname: string): ChromeLocation {
  const path = normalizePath(pathname);
  const defaults = {
    cardId: null as number | null,
    settingsSection: DEFAULT_SETTINGS_SECTION,
    openShellTab: DEFAULT_OPENSHELL_TAB,
  };

  if (path === "/help") {
    return { view: "help", ...defaults };
  }

  if (path === "/settings" || path.startsWith("/settings/")) {
    return { view: "settings", ...defaults, ...parseSettingsPath(path) };
  }

  const card = /^\/card\/(\d+)$/.exec(path);
  if (card) {
    const id = Number(card[1]);
    if (Number.isFinite(id) && id > 0) {
      return { view: "board", ...defaults, cardId: id };
    }
  }

  return { view: "board", ...defaults };
}

export function formatChromePath(loc: ChromeLocation): string {
  if (loc.view === "help") return "/help";
  if (loc.view === "settings") {
    const section = loc.settingsSection || DEFAULT_SETTINGS_SECTION;
    if (section === "openshell") {
      const tab = loc.openShellTab || DEFAULT_OPENSHELL_TAB;
      if (tab === DEFAULT_OPENSHELL_TAB) return "/settings";
      return `/settings/openshell/${tab}`;
    }
    return `/settings/${section}`;
  }
  if (loc.view === "board" && loc.cardId != null && loc.cardId > 0) {
    return `/card/${loc.cardId}`;
  }
  return "/";
}

export function chromeLocationsEqual(a: ChromeLocation, b: ChromeLocation): boolean {
  if (a.view !== b.view) return false;
  if (a.view === "board") return a.cardId === b.cardId;
  if (a.view === "settings") {
    const aSection = a.settingsSection || DEFAULT_SETTINGS_SECTION;
    const bSection = b.settingsSection || DEFAULT_SETTINGS_SECTION;
    if (aSection !== bSection) return false;
    if (aSection === "openshell") {
      return (
        (a.openShellTab || DEFAULT_OPENSHELL_TAB) ===
        (b.openShellTab || DEFAULT_OPENSHELL_TAB)
      );
    }
    return true;
  }
  return true;
}

/** Normalize location fields so non-active axes use defaults. */
export function normalizeChromeLocation(loc: ChromeLocation): ChromeLocation {
  if (loc.view === "board") {
    return {
      view: "board",
      cardId: loc.cardId != null && loc.cardId > 0 ? loc.cardId : null,
      settingsSection: DEFAULT_SETTINGS_SECTION,
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }
  if (loc.view === "help") {
    return {
      view: "help",
      cardId: null,
      settingsSection: DEFAULT_SETTINGS_SECTION,
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }
  const section = SETTINGS_SECTIONS.has(loc.settingsSection)
    ? loc.settingsSection
    : DEFAULT_SETTINGS_SECTION;
  const tab =
    section === "openshell" && OPENSHELL_TABS.has(loc.openShellTab)
      ? loc.openShellTab
      : DEFAULT_OPENSHELL_TAB;
  return {
    view: "settings",
    cardId: null,
    settingsSection: section,
    openShellTab: tab,
  };
}

/** Read the browser URL into chrome location. */
export function readChromeLocation(
  loc: Pick<Location, "pathname"> = window.location,
): ChromeLocation {
  return normalizeChromeLocation(parseChromeLocation(loc.pathname));
}

/**
 * Write chrome location to the URL via the History API.
 * Skips when the pathname is already exactly the canonical path.
 */
export function writeChromeLocation(
  loc: ChromeLocation,
  mode: "push" | "replace",
  hist: Pick<History, "pushState" | "replaceState"> = window.history,
  current: Pick<Location, "pathname"> = window.location,
): void {
  const path = formatChromePath(normalizeChromeLocation(loc));
  if (current.pathname === path) return;
  if (mode === "push") hist.pushState(null, "", path);
  else hist.replaceState(null, "", path);
}

function parseSettingsPath(
  path: string,
): Pick<ChromeLocation, "settingsSection" | "openShellTab"> {
  if (path === "/settings") {
    return {
      settingsSection: DEFAULT_SETTINGS_SECTION,
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }

  const rest = path.slice("/settings/".length);
  const parts = rest.split("/").filter(Boolean);
  if (parts.length === 0) {
    return {
      settingsSection: DEFAULT_SETTINGS_SECTION,
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }

  const [head, tab] = parts;
  // Retired top-level GitHub App page → Providers (App access token lives there).
  if (head === "github-app") {
    return {
      settingsSection: "openshell",
      openShellTab: "providers",
    };
  }
  if (head === "openshell") {
    if (tab && OPENSHELL_TABS.has(tab)) {
      return {
        settingsSection: "openshell",
        openShellTab: tab as OpenShellTab,
      };
    }
    return {
      settingsSection: "openshell",
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }

  if (SETTINGS_SECTIONS.has(head)) {
    return {
      settingsSection: head as SettingsSection,
      openShellTab: DEFAULT_OPENSHELL_TAB,
    };
  }

  return {
    settingsSection: DEFAULT_SETTINGS_SECTION,
    openShellTab: DEFAULT_OPENSHELL_TAB,
  };
}

function normalizePath(pathname: string): string {
  if (!pathname || pathname === "/") return "/";
  const trimmed = pathname.replace(/\/+$/, "");
  return trimmed === "" ? "/" : trimmed;
}
