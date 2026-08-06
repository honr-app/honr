import type { AppView } from "./components/PrimarySidebar";

/** Primary chrome location mirrored in the URL (Settings subsections are out of scope). */
export type ChromeLocation = {
  view: AppView;
  /** Open DetailDrawer card; only meaningful when `view === "board"`. */
  cardId: number | null;
};

/**
 * Path contract (works with ServeDir → index.html SPA fallback):
 *   `/`           → board
 *   `/help`       → help
 *   `/settings`   → settings (prefix reserved for later subsection deep links)
 *   `/card/:id`   → board with DetailDrawer open on that card
 */
export function parseChromeLocation(pathname: string): ChromeLocation {
  const path = normalizePath(pathname);

  if (path === "/help") {
    return { view: "help", cardId: null };
  }
  if (path === "/settings" || path.startsWith("/settings/")) {
    return { view: "settings", cardId: null };
  }

  const card = /^\/card\/(\d+)$/.exec(path);
  if (card) {
    const id = Number(card[1]);
    if (Number.isFinite(id) && id > 0) {
      return { view: "board", cardId: id };
    }
  }

  return { view: "board", cardId: null };
}

export function formatChromePath(loc: ChromeLocation): string {
  if (loc.view === "help") return "/help";
  if (loc.view === "settings") return "/settings";
  if (loc.view === "board" && loc.cardId != null && loc.cardId > 0) {
    return `/card/${loc.cardId}`;
  }
  return "/";
}

export function chromeLocationsEqual(a: ChromeLocation, b: ChromeLocation): boolean {
  return a.view === b.view && a.cardId === b.cardId;
}

/** Read the browser URL into chrome location. */
export function readChromeLocation(
  loc: Pick<Location, "pathname"> = window.location,
): ChromeLocation {
  return parseChromeLocation(loc.pathname);
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
  const path = formatChromePath(loc);
  if (current.pathname === path) return;
  if (mode === "push") hist.pushState(null, "", path);
  else hist.replaceState(null, "", path);
}

function normalizePath(pathname: string): string {
  if (!pathname || pathname === "/") return "/";
  const trimmed = pathname.replace(/\/+$/, "");
  return trimmed === "" ? "/" : trimmed;
}
