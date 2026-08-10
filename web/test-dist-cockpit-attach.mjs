/**
 * Guard against production Rollup DCE of Cockpit attach.
 *
 * Symptom we hit: the attach effect's setTimeout body was emptied to
 * `setTimeout(()=>{N=null},0)`, so the UI stuck on "connecting…" and never
 * opened `/api/cockpit-attach`. Source/unit tests still passed.
 *
 * Run after `vite build` (wired into `npm run build`).
 */
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const assetsDir = join(root, "dist", "assets");

let assets;
try {
  assets = readdirSync(assetsDir).filter((f) => f.endsWith(".js"));
} catch (e) {
  throw new Error(
    `web/dist/assets missing — run vite build first (${e instanceof Error ? e.message : e})`,
  );
}

assert.ok(assets.length > 0, "web/dist/assets has no JS chunks");

const bundle = assets
  .map((f) => readFileSync(join(assetsDir, f), "utf8"))
  .join("\n");

assert.match(
  bundle,
  /\/api\/cockpit-attach/,
  "production bundle must retain /api/cockpit-attach (attach WebSocket URL)",
);

assert.ok(
  (bundle.match(/new WebSocket/g) || []).length >= 2,
  "production bundle must open board WS and cockpit-attach WS (got fewer than 2 new WebSocket)",
);

assert.match(
  bundle,
  /FitAddon|addon-fit/,
  "production bundle must retain xterm FitAddon (attach terminal)",
);

// Known failure shape: attach effect reduced to a no-op timer with no WS nearby.
const noopTimer = /setTimeout\(\(\)=>\{[A-Za-z]=null\},0\)/g;
for (const m of bundle.matchAll(noopTimer)) {
  const window = bundle.slice(m.index, m.index + 800);
  if (
    window.includes('data-testid:"cockpit-attach"') ||
    window.includes("cockpit-xterm")
  ) {
    assert.fail(
      "Cockpit attach effect was tree-shaken to an empty setTimeout (UI would stick on connecting…)",
    );
  }
}

console.log("✅ production dist retains Cockpit attach WebSocket + xterm");
