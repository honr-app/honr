import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // SSE must be declared before the general `/api` rule. Zero timeouts keep
      // the long-lived stream open; the backend also flushes an immediate
      // comment so Vite does not buffer until the first 15s keep-alive.
      "/api/events": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
        timeout: 0,
        proxyTimeout: 0,
      },
      // One origin in dev, so SSE and the MCP endpoint behave as they will in prod.
      "/api": { target: "http://127.0.0.1:8080", changeOrigin: true },
      "/mcp": { target: "http://127.0.0.1:8080", changeOrigin: true },
    },
  },
  build: { outDir: "dist" },
});
