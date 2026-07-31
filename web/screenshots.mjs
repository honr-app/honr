/**
 * Capture the board so it can be looked at, not just reasoned about.
 *
 * Runs a *scratch* honr on :8081 against a fixture board, so real state is
 * never touched and the captures are deterministic. Shoots desktop and phone,
 * because §8's whole claim is that the digest is what you read on a phone.
 *
 *   cd web && npm run shots
 *
 * PNGs land in web/shots/ (gitignored).
 */
import { chromium } from "playwright";
import { spawn, execSync } from "node:child_process";
import { mkdirSync, writeFileSync, copyFileSync, rmSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const ROOT = new URL("..", import.meta.url).pathname;
const SCRATCH = "/tmp/honr-ui";
const OUT = `${ROOT}web/shots`;
const PORT = 8081;
const BASE = `http://127.0.0.1:${PORT}`;

rmSync(SCRATCH, { recursive: true, force: true });
mkdirSync(SCRATCH, { recursive: true });
mkdirSync(OUT, { recursive: true });

// A scratch honr: same level schema, no agents, its own state file.
const yaml = execSync(`sed 's/^    enabled: true/    enabled: false/' ${ROOT}honr.yaml`).toString();
writeFileSync(`${SCRATCH}/honr.yaml`, yaml);
writeFileSync(`${SCRATCH}/honr.json`, execSync(`node ${ROOT}web/ui-fixture.mjs`).toString());
mkdirSync(`${SCRATCH}/web`, { recursive: true });
execSync(`cp -R ${ROOT}web/dist ${SCRATCH}/web/dist`);
copyFileSync(`${ROOT}sandbox/policy.yaml`, `${SCRATCH}/policy.yaml`);

const honr = spawn(`${ROOT}target/debug/honr`, [], {
  cwd: SCRATCH,
  env: { ...process.env, HONR_PORT: String(PORT) },
  stdio: "inherit",
});
process.on("exit", () => honr.kill());

// Wait for it rather than guessing.
for (let i = 0; i < 40; i++) {
  try {
    if ((await fetch(`${BASE}/healthz`)).ok) break;
  } catch {}
  await sleep(250);
}

const browser = await chromium.launch();

async function shoot(name, { width, height }, prepare) {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.goto(BASE, { waitUntil: "networkidle" });
  // The board renders from a snapshot fetch; give it a beat to arrive.
  await page.waitForSelector(".app", { timeout: 10_000 });
  await sleep(600);
  if (prepare) await prepare(page);
  await page.screenshot({ path: `${OUT}/${name}.png`, fullPage: true });
  console.log(`  ${name}.png`);
  await page.close();
}

const DESKTOP = { width: 1600, height: 1000 };
const PHONE = { width: 390, height: 844 };

const tab = (name) => async (page) => {
  await page.getByRole("button", { name, exact: true }).click();
  await sleep(500);
};

console.log("capturing:");
await shoot("desktop-digest", DESKTOP, tab("Digest"));
await shoot("desktop-board", DESKTOP, tab("Board"));
await shoot("desktop-tree", DESKTOP, tab("Tree"));
await shoot("phone-digest", PHONE, tab("Digest"));
await shoot("phone-board", PHONE, tab("Board"));

// The drawer is where a human actually decides, so it needs its own look.
await shoot("desktop-drawer-needs-you", DESKTOP, async (page) => {
  await tab("Board")(page);
  await page.locator(".column-needs_you .card").first().click();
  await sleep(600);
});
await shoot("desktop-drawer-review", DESKTOP, async (page) => {
  await tab("Board")(page);
  await page.locator(".column-review .card").first().click();
  await sleep(600);
});

await browser.close();
honr.kill();
console.log(`\n${OUT}`);
