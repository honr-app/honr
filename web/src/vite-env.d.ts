/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Optional API host[:port] for WebSockets (skips Vite proxy). */
  readonly VITE_HONR_WS_HOST?: string;
  /** API port when the UI is on Vite :5173 / preview :4173 (default 8080). */
  readonly VITE_HONR_PORT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
