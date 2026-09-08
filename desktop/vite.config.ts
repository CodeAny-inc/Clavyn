import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

// Dev-only plugin: when the Vite dev server is opened in a regular browser
// (not inside the Tauri webview), inject the e2e fixture so the app can render
// without a real Tauri IPC surface. In Tauri, __TAURI_INTERNALS__ is already
// defined so the fixture is skipped.
function tauriFixtureDevPlugin() {
  let fixtureCode = "";
  return {
    name: "tauri-fixture-dev",
    apply: "serve" as const,
    configResolved() {
      try {
        fixtureCode = readFileSync(resolve(__dirname, "e2e/tauri-fixture.js"), "utf-8");
      } catch { /* fixture not found — skip silently */ }
    },
    transformIndexHtml(html: string) {
      if (!fixtureCode) return html;
      const injection = `<script>window.__TAURI_INTERNALS__||(${fixtureCode});</script>`;
      return html.replace("</head>", `${injection}\n  </head>`);
    },
  };
}

export default defineConfig({
  plugins: [vue(), tauriFixtureDevPlugin()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
