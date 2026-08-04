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
import { createServer } from "node:http";
import { mkdirSync, writeFileSync, copyFileSync, rmSync, existsSync, readFileSync } from "node:fs";
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

let honr;
if (existsSync(`${ROOT}target/debug/honr`)) {
  honr = spawn(`${ROOT}target/debug/honr`, [], {
    cwd: SCRATCH,
    env: { ...process.env, HONR_PORT: String(PORT) },
    stdio: "inherit",
  });
  process.on("exit", () => honr.kill());
} else {
  // Lightweight server serving web/dist and fixture data
  const rawData = JSON.parse(readFileSync(`${SCRATCH}/honr.json`, "utf8"));
  const snapshotData = JSON.stringify({
    items: Object.values(rawData.items),
    levels: [
      { name: "Vision", horizon: null, owner: null, elaborate: null, requires: [], claimable: false },
      { name: "Project", horizon: null, owner: null, elaborate: null, requires: [], claimable: false },
      { name: "Epic", horizon: null, owner: null, elaborate: null, requires: [], claimable: false },
      { name: "Story", horizon: null, owner: null, elaborate: null, requires: [], claimable: true },
    ],
    goals: [
      {
        id: 1,
        title: "honr builds honr",
        intent: "honr takes cards against its own source and hands back reviewable pull requests.",
        progress: 0.5,
        leaves_done: 4,
        leaves_total: 8,
        agents_live: 3,
        needs_you: 1,
        columns: [],
        story: rawData.stories?.[2] ?? [],
      },
    ],
    server_time: new Date().toISOString(),
    agent_timeout_secs: 600,
    seq: 1,
  });
  const server = createServer((req, res) => {
    if (req.url === "/healthz") {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ status: "ok" }));
    } else if (req.url === "/api/snapshot" || req.url?.startsWith("/api/snapshot")) {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(snapshotData);
    } else if (req.url?.startsWith("/api/item/")) {
      const id = parseInt(req.url.split("/").pop());
      const item = rawData.items[id];
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          ...item,
          ancestry: [{ level: "Vision", title: "honr builds honr", intent: "honr takes cards" }],
          constraints: [],
          children: [],
        })
      );
    } else {
      let filePath = `${SCRATCH}/web/dist${req.url === "/" ? "/index.html" : req.url}`;
      if (!existsSync(filePath)) filePath = `${SCRATCH}/web/dist/index.html`;
      try {
        const content = readFileSync(filePath);
        const contentType = filePath.endsWith(".css")
          ? "text/css"
          : filePath.endsWith(".js")
          ? "application/javascript"
          : "text/html";
        res.writeHead(200, { "Content-Type": contentType });
        res.end(content);
      } catch {
        res.writeHead(404);
        res.end();
      }
    }
  });
  server.listen(PORT, "127.0.0.1");
  honr = { kill: () => server.close() };
}

// Wait for server readiness
for (let i = 0; i < 40; i++) {
  try {
    if ((await fetch(`${BASE}/healthz`)).ok) break;
  } catch {}
  await sleep(250);
}

let browser;
try {
  browser = await chromium.launch();
} catch (err) {
  console.log(`\n[Playwright] Skipping browser screenshots: ${err.message.split("\n")[0]}`);
  honr.kill();
  process.exit(0);
}

async function shoot(name, { width, height }, prepare) {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.goto(BASE, { waitUntil: "networkidle" });
  await page.waitForSelector(".app", { timeout: 10_000 });
  await sleep(600);
  if (prepare) await prepare(page);
  await page.screenshot({ path: `${OUT}/${name}.png`, fullPage: true });
  console.log(`  ${name}.png`);
  await page.close();
}

const DESKTOP = { width: 1600, height: 1000 };
const PHONE = { width: 390, height: 844 };

console.log("capturing & asserting:");

// Single board surface (no Home/Board tabs).
await shoot("desktop-board", DESKTOP, async (page) => {
  await page.waitForSelector(".board-page", { timeout: 5000 });

  // Needs you action block when the fixture has an escalation.
  const needs = page.locator(".board-needs");
  if ((await needs.count()) > 0) {
    await needs.first().waitFor({ state: "visible", timeout: 5000 });
    console.log(`  [Playwright Assertion] Needs you block visible`);
  }

  // Expand a lane if needed, then assert blocker chips on Backlog cards.
  const toggleGraphBtn = page.locator('[data-testid="toggle-graph-view"]');
  if ((await toggleGraphBtn.count()) === 0) {
    await page.locator(".lane-head").first().click();
    await sleep(300);
  }

  const blockerChips = page.locator(".blocker-chips");
  await blockerChips.first().waitFor({ state: "visible", timeout: 5000 });
  const text = await blockerChips.first().textContent();
  console.log(`  [Playwright Assertion] Blocker chips content: "${text?.trim()}"`);
  if (!text?.includes("Supervisor runs the gates") || !text?.includes("backlog")) {
    throw new Error(`Blocker chips missing expected human-readable text. Got: ${text}`);
  }

  const blockedCard = page.locator(".card", { has: page.locator(".blocker-chips") });
  await blockedCard.first().screenshot({ path: `${OUT}/blocked-card-chip.png` });
  console.log(`  blocked-card-chip.png`);
});

await shoot("desktop-graph", DESKTOP, async (page) => {
  const toggleGraphBtn = page.locator('[data-testid="toggle-graph-view"]');
  if ((await toggleGraphBtn.count()) === 0) {
    await page.locator(".lane-head").first().click();
    await sleep(300);
  }
  await page.locator('[data-testid="toggle-graph-view"]').first().click();
  await sleep(600);
  await page.locator('[data-testid="graph-container"]').first().waitFor({
    state: "visible",
    timeout: 5000,
  });
  console.log(`  [Playwright Assertion] Visual dependency graph loaded`);
});

await shoot("phone-board", PHONE);

await shoot("desktop-drawer-needs-you", DESKTOP, async (page) => {
  const card = page.locator(".column-needs_you .card").first();
  if ((await card.count()) === 0) {
    // Open a hot lane, or use the Needs you action title.
    const lane = page.locator(".lane-hot .lane-head").first();
    if ((await lane.count()) > 0) await lane.click();
    await sleep(300);
  }
  if ((await page.locator(".column-needs_you .card").count()) > 0) {
    await page.locator(".column-needs_you .card").first().click();
  } else {
    await page.locator(".board-need-title").first().click();
  }
  await sleep(600);
});

await shoot("desktop-drawer-review", DESKTOP, async (page) => {
  await page.getByRole("button", { name: /Review/ }).first().click();
  await sleep(400);
  const reviewCard = page.locator(".column-review .card").first();
  if ((await reviewCard.count()) === 0) {
    await page.locator(".lane-head").first().click();
    await sleep(300);
  }
  await page.locator(".column-review .card").first().click();
  await sleep(600);
});

await browser.close();
honr.kill();
console.log(`\n${OUT}`);
