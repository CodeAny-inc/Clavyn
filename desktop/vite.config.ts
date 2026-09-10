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
      const injection = `<script>if(!window.__TAURI_INTERNALS__){${fixtureCode}}</script>`;
      return html.replace("</head>", `${injection}\n  </head>`);
    },
  };
}

export default defineConfig({
  plugins: [vue(), tauriFixtureDevPlugin()],
  clearScreen: false,
  // Every SFC in this app is `<script setup>`, so the Options API runtime is
  // dead weight. The flags must be set explicitly: @vitejs/plugin-vue defaults
  // __VUE_OPTIONS_API__ to true when no `define` is present.
  define: {
    __VUE_OPTIONS_API__: "false",
    __VUE_PROD_DEVTOOLS__: "false",
    __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: "false",
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The app only ever runs in the Tauri webview (WebView2 / WKWebView /
    // WebKitGTK), all of which support ES2022. Vite's default 'modules' target
    // downlevels syntax these engines handle natively.
    target: "es2022",
  },
});
