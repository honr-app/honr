import { defineConfig, type ProxyOptions } from "vite";
import react from "@vitejs/plugin-react";
import type { IncomingMessage } from "node:http";

/**
 * Forward the browser Host/proto so honr's `public_origin` matches
 * `window.location` (Vite / Tailscale / reverse proxy), not the bind address.
 */
function forwardBrowserHost(proxyReq: { setHeader: (k: string, v: string) => void }, req: IncomingMessage) {
  const host = req.headers.host;
  if (host) proxyReq.setHeader("X-Forwarded-Host", host);
  // Preserve Tailscale Serve HTTPS; fall back to http for local Vite.
  const proto = String(req.headers["x-forwarded-proto"] ?? "").split(",")[0]?.trim();
  proxyReq.setHeader(
    "X-Forwarded-Proto",
    proto === "https" || proto === "http" ? proto : "http",
  );
}

/**
 * Backend reachability for the Vite proxy only — not the public board origin.
 * Public URLs come from X-Forwarded-* / Host (see forwardBrowserHost).
 * Override with HONR_URL; otherwise loopback + HONR_PORT (cargo run bind).
 */
const HONR_PROXY_TARGET =
  process.env.HONR_URL?.trim() ||
  `http://127.0.0.1:${process.env.HONR_PORT?.trim() || "8080"}`;

const toHonr = (extra: ProxyOptions = {}): ProxyOptions => ({
  target: HONR_PROXY_TARGET,
  changeOrigin: true,
  configure: (proxy) => {
    proxy.on("proxyReq", (proxyReq, req) => forwardBrowserHost(proxyReq, req));
    // Upgrade path does not fire proxyReq — keep forwarded host for any
    // future WS handlers that read X-Forwarded-* during the handshake.
    proxy.on("proxyReqWs", (proxyReq, req) => forwardBrowserHost(proxyReq, req));
  },
  ...extra,
});

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    // Tailscale Serve (https://tot.example.ts.net:5173 → Vite).
    allowedHosts: ["tot.example.ts.net"],
    proxy: {
      // SSE must be declared before the general `/api` rule. Zero timeouts keep
      // the long-lived stream open; the backend also flushes an immediate
      // comment so Vite does not buffer until the first 15s keep-alive.
      "/api/events": toHonr({ timeout: 0, proxyTimeout: 0 }),
      "/api/ws": toHonr({ ws: true }),
      "/api/cockpit-attach": toHonr({ ws: true, timeout: 0, proxyTimeout: 0 }),
      // ws:true so any future /api/* WebSocket is proxied even without a
      // dedicated rule (Vite skips upgrade when the matching rule lacks ws).
      "/api": toHonr({ ws: true }),
      "/auth": toHonr(),
      // Host-mediated OAuth callbacks (MCP client, Antigravity, board AS).
      // Without this, Google/Atlassian redirect_uri on :5173 falls through to
      // the SPA and looks like a silent bounce to home.
      "/oauth": toHonr(),
      "/.well-known": toHonr(),
      "/mcp": toHonr(),
      "/healthz": toHonr(),
      // Public agent bootstrap guide — must not fall through to the SPA shell.
      "/llms.txt": toHonr(),
    },
  },
  build: { outDir: "dist" },
});
