import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // One origin in dev, so SSE and the MCP endpoint behave as they will in prod.
      "/api": { target: "http://127.0.0.1:8080", changeOrigin: true },
      "/mcp": { target: "http://127.0.0.1:8080", changeOrigin: true },
    },
  },
  build: { outDir: "dist" },
});
