import { defineConfig, type ProxyOptions } from "vite";
import react from "@vitejs/plugin-react";
import type { IncomingMessage } from "node:http";

/** So honr builds OAuth redirect_uri on the browser origin (not 127.0.0.1:8080). */
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

const toHonr = (extra: ProxyOptions = {}): ProxyOptions => ({
  target: "http://127.0.0.1:8080",
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
    // Tailscale Serve (https://tot.tail43beb.ts.net:5173 → Vite).
    allowedHosts: ["tot.tail43beb.ts.net"],
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
      "/mcp": toHonr(),
      "/healthz": toHonr(),
      // Public agent bootstrap guide — must not fall through to the SPA shell.
      "/llms.txt": toHonr(),
    },
  },
  build: { outDir: "dist" },
});
