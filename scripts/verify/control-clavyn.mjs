#!/usr/bin/env node
// control-clavyn.mjs — agent-friendly CLI for driving & verifying the Clavyn
// Tauri app's webview via the Chrome DevTools Protocol (through Playwright).
//
// Architecture ("Build the Lever"):
//   - A persistent *harness server* holds the Chromium browser + page in
//     memory so state survives across CLI invocations (navigate, then
//     snapshot, then screenshot ...). It speaks a tiny JSON-over-HTTP RPC.
//   - Each subcommand is a thin client that talks to the running server,
//     auto-spawning one if no session is found. This makes single commands
//     self-contained yet composable.
//   - The renderer is loaded from the Vite dev server (http://127.0.0.1:1420)
//     with the same tauri-fixture.js used by the existing e2e suite, so the
//     full Tauri IPC surface is mocked and deterministic. A real Tauri build
//     can also be driven by pointing CLAVYN_URL at its webview via CDP.
//
// Output contract: every command prints a single JSON object on stdout
// (machine-readable). Use --pretty for indented, human-friendly output.
// Errors print JSON with { ok:false, error, remedy } on stdout and exit 1.

import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import http from "node:http";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = resolve(__dirname, "../..");
const require = createRequire(import.meta.url);

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const SESSION_FILE = join(__dirname, ".session.json");
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");
const DESKTOP_DIR = join(REPO_ROOT, "desktop");

const CFG = {
  url: process.env.CLAVYN_URL ?? "http://127.0.0.1:1420",
  // Reuse the Playwright install already present for the e2e suite instead of
  // pulling a second copy. CHROMIUM_EXECUTABLE_PATH overrides the detected
  // binary (matches the convention used by desktop/e2e/*.mjs).
  chromiumPath: process.env.CHROMIUM_EXECUTABLE_PATH ?? null,
  headless: process.env.CLAVYN_HEADLESS !== "0",
  slowMo: Number(process.env.CLAVYN_SLOW_MO ?? 0),
  viewport: { width: Number(process.env.CLAVYN_VIEWPORT_W ?? 1440), height: Number(process.env.CLAVYN_VIEWPORT_H ?? 900) },
  rpcTimeout: Number(process.env.CLAVYN_RPC_TIMEOUT ?? 30000),
};

// ---------------------------------------------------------------------------
// Tiny helpers
// ---------------------------------------------------------------------------

function out(obj, pretty) {
  if (pretty) process.stdout.write(JSON.stringify(obj, null, 2) + "\n");
  else process.stdout.write(JSON.stringify(obj) + "\n");
}

function fail(error, remedy, code = 1) {
  out({ ok: false, error, remedy: remedy ?? null }, process.env.CLAVYN_PRETTY === "1");
  process.exit(code);
}

function loadSession() {
  if (!existsSync(SESSION_FILE)) return null;
  try { return JSON.parse(readFileSync(SESSION_FILE, "utf8")); }
  catch { return null; }
}

function saveSession(s) { writeFileSync(SESSION_FILE, JSON.stringify(s)); }
function clearSession() { if (existsSync(SESSION_FILE)) rmSync(SESSION_FILE); }

async function httpJson(port, body, timeout = CFG.rpcTimeout) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    const res = await fetch(`http://127.0.0.1:${port}/rpc`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    return await res.json();
  } finally {
    clearTimeout(timer);
  }
}

// Resolve the Playwright module from the e2e install (pinned 1.56.1).
function loadPlaywright() {
  try {
    const pwPath = require.resolve("playwright", { paths: [E2E_DIR] });
    return require(pwPath);
  } catch {
    try {
      // Fall back to a skill-local install if present.
      return require("playwright");
    } catch {
      return null;
    }
  }
}

async function isPortResponding(port) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 1500);
  try {
    const res = await fetch(`http://127.0.0.1:${port}/`, { signal: controller.signal });
    return res.ok || res.status === 404;
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
  }
}

// ---------------------------------------------------------------------------
// Harness server (holds browser + page; speaks JSON RPC)
// ---------------------------------------------------------------------------

async function startHarnessServer({ detached = true } = {}) {
  const pw = loadPlaywright();
  if (!pw) {
    fail(
      "Playwright is not installed.",
      "Run `npm install` inside desktop/e2e (pins playwright 1.56.1), or set NODE_PATH to a tree that contains the `playwright` package. Then install a browser with `npx playwright install chromium` or point CHROMIUM_EXECUTABLE_PATH at a system Chrome/Chromium."
    );
  }

  // Ensure the Vite dev server is up so the renderer has something to load.
  const devUp = await isPortResponding(1420);
  let devChild = null;
  if (!devUp) {
    if (process.env.CLAVYN_AUTO_START_DEV === "0") {
      fail(
        `Vite dev server is not responding at ${CFG.url}.`,
        "Start it with `cd desktop && npm run dev` (port 1420), or set CLAVYN_URL to an already-running server, or allow auto-start by leaving CLAVYN_AUTO_START_DEV unset."
      );
    }
    devChild = spawn(process.platform === "win32" ? "npm.cmd" : "npm", ["run", "dev"], {
      cwd: DESKTOP_DIR, stdio: "ignore", detached: true,
      env: { ...process.env, FORCE_COLOR: "0" },
    });
    devChild.unref?.();
    // Wait for Vite to come up (up to ~30s).
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 500));
      if (await isPortResponding(1420)) break;
    }
    if (!(await isPortResponding(1420))) {
      fail("Vite dev server did not come up within 30s.", "Run `cd desktop && npm run dev` manually and inspect its output, then retry.");
    }
  }

  let browser, context, page;
  const consoleLog = [];
  const ipcLog = [];
  let ready = false;

  try {
    const launchOpts = { headless: CFG.headless, slowMo: CFG.slowMo };
    if (CFG.chromiumPath) launchOpts.executablePath = CFG.chromiumPath;
    browser = await pw.chromium.launch(launchOpts);
  } catch (e) {
    fail(
      `Could not launch Chromium: ${e.message}`,
      "Install a browser: `cd desktop/e2e && npx playwright install chromium`, or set CHROMIUM_EXECUTABLE_PATH to a system Chrome/Chromium binary."
    );
  }

  context = await browser.newContext({ viewport: CFG.viewport, colorScheme: "light" });
  page = await context.newPage();

  page.on("console", msg => {
    consoleLog.push({ type: msg.type(), text: msg.text(), time: Date.now() });
    if (consoleLog.length > 500) consoleLog.splice(0, consoleLog.length - 500);
  });
  page.on("pageerror", err => {
    consoleLog.push({ type: "pageerror", text: String(err), time: Date.now() });
  });

  // Inject the tauri-fixture before any app script runs so Tauri IPC is mocked.
  if (existsSync(FIXTURE_PATH)) {
    await page.addInitScript({ path: FIXTURE_PATH });
  } else {
    fail(`tauri-fixture.js not found at ${FIXTURE_PATH}.`, "Ensure the desktop/e2e directory is intact.");
  }

  await page.goto(CFG.url, { waitUntil: "domcontentloaded" });
  // Wait for the app shell to render (the sidebar brand or a host label).
  try {
    await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
    ready = true;
  } catch {
    ready = false;
  }

  const handlers = {
    async status() {
      return {
        ok: true,
        running: true,
        ready,
        url: CFG.url,
        headless: CFG.headless,
        viewport: CFG.viewport,
        chromium: CFG.chromiumPath ?? "playwright-bundled",
        devServerAutoStarted: !!devChild,
      };
    },
    async info() {
      const info = await page.evaluate(() => window.__TAURI_INTERNALS__?.invoke?.("get_app_info") ?? null).catch(() => null);
      const view = await currentView(page);
      return { ok: true, app: info, view, url: page.url() };
    },
    async snapshot() {
      const snap = await page.accessibility.snapshot();
      return { ok: true, snapshot: snap };
    },
    async screenshot({ path }) {
      const p = path ?? join(__dirname, `screenshot-${Date.now()}.png`);
      await page.screenshot({ path: p, fullPage: false });
      return { ok: true, path: p };
    },
    async components() {
      const comps = await page.evaluate(() => {
        const pick = el => ({
          tag: el.tagName.toLowerCase(),
          role: el.getAttribute("role"),
          name: el.getAttribute("aria-label") || el.getAttribute("title") || (el.textContent || "").trim().slice(0, 60) || null,
          testid: el.getAttribute("data-testid"),
          host: el.getAttribute("data-host-id"),
          connected: el.getAttribute("data-connected"),
          active: el.getAttribute("data-active"),
        });
        const sels = [
          "aside[aria-label='Application navigation']",
          "[data-host-id]",
          ".terminal-pane",
          "[role='dialog']",
          "[data-testid]",
          "button[aria-label]",
        ];
        const seen = new Set();
        const out = [];
        for (const s of sels) for (const el of document.querySelectorAll(s)) {
          if (seen.has(el)) continue; seen.add(el);
          out.push(pick(el));
        }
        return out;
      });
      return { ok: true, components: comps };
    },
    async eval({ expr }) {
      const value = await page.evaluate(expr);
      return { ok: true, value };
    },
    async console({ since }) {
      return { ok: true, logs: consoleLog.filter(l => !since || l.time >= since) };
    },
    async networkLog() {
      // The fixture records every invoke() into window.__terminalTest.calls.
      const calls = await page.evaluate(() => window.__terminalTest?.calls ?? []).catch(() => []);
      return { ok: true, calls };
    },
    async networkSummary() {
      const calls = await page.evaluate(() => window.__terminalTest?.calls ?? []).catch(() => []);
      const byCmd = {};
      for (const c of calls) byCmd[c.command] = (byCmd[c.command] ?? 0) + 1;
      return { ok: true, total: calls.length, byCommand: byCmd };
    },
    async navigate({ view }) {
      const valid = ["hosts", "terminal", "files", "workspaces", "identities", "keys", "known-hosts", "vault", "settings"];
      if (!valid.includes(view)) return { ok: false, error: `Unknown view '${view}'.`, remedy: `Valid views: ${valid.join(", ")}` };
      // Vault & Settings live in a separate sidebar section outside <nav> and
      // have dynamic aria-labels ("Vault, unlocked", "Settings, update available"),
      // so match by prefix regex across the whole <aside>.
      const label = labelFor(view);
      const nameRe = new RegExp("^" + label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "i");
      await page.locator("aside").getByRole("button", { name: nameRe }).first().click({ timeout: 5000 });
      await settle(page);
      return { ok: true, view: await currentView(page) };
    },
    async home() { return handlers.navigate({ view: "hosts" }); },
    async openPalette() {
      await page.keyboard.press("Control+KeyK");
      await page.waitForSelector("[role='dialog'], input[placeholder*='Search' i]", { timeout: 3000 }).catch(() => {});
      return { ok: true };
    },
    async closePalette() {
      await page.keyboard.press("Escape");
      return { ok: true };
    },
    async click({ selector, name, role }) {
      if (name) {
        const loc = role ? page.getByRole(role, { name, exact: true }) : page.getByText(name, { exact: true }).first();
        await loc.first().click({ timeout: 5000 });
      } else if (selector) {
        await page.locator(selector).first().click({ timeout: 5000 });
      } else {
        return { ok: false, error: "click needs --selector or --name.", remedy: "Pass --name 'Visible text' or --selector 'css'." };
      }
      await settle(page);
      return { ok: true };
    },
    async clickXY({ x, y }) {
      await page.mouse.click(x, y);
      await settle(page);
      return { ok: true };
    },
    async type({ text }) {
      await page.keyboard.type(text, { delay: CFG.slowMo ? 12 : 0 });
      return { ok: true };
    },
    async press({ key }) {
      await page.keyboard.press(key);
      await settle(page);
      return { ok: true };
    },
    async newSession() {
      await page.getByRole("button", { name: "New session", exact: true }).first().click();
      await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab").catch(async () => {});
      await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
      await settle(page);
      const sessions = await page.evaluate(() => window.__terminalTest?.live ? Object.keys(window.__terminalTest.live) : []);
      return { ok: true, sessions };
    },
    async connect({ host }) {
      // host may be an id or a visible label.
      const byLabel = page.getByText(host, { exact: true }).filter({ visible: true }).first();
      const byId = page.locator(`[data-host-id="${host}"]`);
      if (await byLabel.count()) await byLabel.dblclick();
      else if (await byId.count()) await byId.dblclick();
      else return { ok: false, error: `Host '${host}' not found in the Hosts view.`, remedy: "Run `snapshot` or `components` to list visible hosts, or `navigate hosts` first." };
      await settle(page);
      const connects = await page.evaluate(() => window.__terminalTest?.connects ?? []);
      return { ok: true, connects };
    },
    async send({ text }) {
      // Type into the focused terminal pane's helper textarea.
      const active = page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").first();
      if (await active.count()) {
        await active.focus();
      } else {
        const any = page.locator(".xterm-helper-textarea").first();
        if (!await any.count()) return { ok: false, error: "No terminal pane is open.", remedy: "Run `new-session` or `connect <host>` first." };
        await any.focus();
      }
      await page.keyboard.type(text, { delay: CFG.slowMo ? 18 : 0 });
      await page.keyboard.press("Enter");
      await settle(page);
      return { ok: true };
    },
    async waitSettle() { await settle(page); return { ok: true }; },
    async fixture({ op }) {
      const ops = ["state", "hold-next", "fail-next", "release"];
      if (!ops.includes(op)) return { ok: false, error: `Unknown fixture op '${op}'.`, remedy: `Valid ops: ${ops.join(", ")}` };
      if (op === "state") {
        const state = await page.evaluate(() => { const s = window.__terminalTest; return s ? { connects: s.connects, closes: s.closes, writes: s.writes, live: s.live } : null; });
        return { ok: true, state };
      }
      await page.evaluate(op => { const s = window.__terminalTest; if (!s) throw new Error("fixture not present"); if (op === "hold-next") s.holdNext = true; if (op === "fail-next") s.failNext = true; if (op === "release") s.release(); }, op);
      return { ok: true, op };
    },
    async cleanup() {
      await page.evaluate(() => { const s = window.__terminalTest; if (s) for (const id of Object.keys(s.live)) s.disconnect(id); });
      return { ok: true };
    },
    async reset() {
      await page.reload({ waitUntil: "domcontentloaded" });
      await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 }).catch(() => {});
      consoleLog.length = 0;
      return { ok: true };
    },
    async stop() {
      try { await context.close(); } catch {}
      try { await browser.close(); } catch {}
      if (devChild) try { process.kill(-devChild.pid); } catch {}
      // Respond before exiting.
      setTimeout(() => process.exit(0), 50);
      return { ok: true };
    },
  };

  const server = http.createServer(async (req, res) => {
    if (req.method === "GET" && req.url === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true, ready }));
      return;
    }
    if (req.method === "POST" && req.url === "/rpc") {
      let body = "";
      for await (const chunk of req) body += chunk;
      let msg;
      try { msg = JSON.parse(body); } catch { res.writeHead(400); res.end(JSON.stringify({ ok: false, error: "invalid JSON body" })); return; }
      const fn = handlers[msg.cmd];
      if (!fn) { res.writeHead(404, { "content-type": "application/json" }); res.end(JSON.stringify({ ok: false, error: `unknown command '${msg.cmd}'` })); return; }
      try {
        const result = await fn(msg.args ?? {});
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(result));
      } catch (e) {
        res.writeHead(500, { "content-type": "application/json" });
        res.end(JSON.stringify({ ok: false, error: String(e.message ?? e), remedy: "Run `doctor` to check harness health, or `reset` to reload the page." }));
      }
      return;
    }
    res.writeHead(404); res.end("not found");
  });

  await new Promise(r => server.listen(0, "127.0.0.1", r));
  const port = server.address().port;
  saveSession({ port, pid: process.pid, startedAt: Date.now(), url: CFG.url });

  // Keep the server alive; detach from terminal when run as a daemon.
  process.on("SIGTERM", async () => { try { await handlers.stop(); } catch {} process.exit(0); });

  if (detached) {
    // Signal readiness to the spawning parent, then stay alive.
    process.stdout.write(JSON.stringify({ ok: true, port, ready }) + "\n");
  }
}

// Map a view id to the visible sidebar label used for getByRole({name}).
function labelFor(view) {
  switch (view) {
    case "known-hosts": return "Known Hosts";
    default: return view.charAt(0).toUpperCase() + view.slice(1);
  }
}

async function currentView(page) {
  return await page.evaluate(() => {
    // Vault & Settings are outside <nav> but still inside <aside>; search both.
    const active = document.querySelector("aside .sidebar-link-active, aside [aria-current='page']");
    if (!active) return null;
    const label = active.getAttribute("aria-label") ?? active.textContent?.trim() ?? "";
    // aria-labels like "Vault, unlocked" / "Settings, update available" → take the first word.
    return label.toLowerCase().split(",")[0].trim();
  }).catch(() => null);
}

async function settle(page) {
  // Wait for the fixture to drain pending calls and for layout to settle.
  await page.waitForFunction(() => {
    const s = window.__terminalTest;
    return !s || (s.pending && s.pending.length === 0);
  }, { timeout: 5000 }).catch(() => {});
  await page.waitForTimeout(150);
}

// ---------------------------------------------------------------------------
// Client: ensure a server is running, then dispatch the command.
// ---------------------------------------------------------------------------

async function ensureServer() {
  const sess = loadSession();
  if (sess) {
    // Verify it's actually alive.
    try {
      const res = await fetch(`http://127.0.0.1:${sess.port}/health`, { signal: AbortSignal.timeout(1500) });
      if (res.ok) return sess.port;
    } catch {}
  }
  // Spawn a detached server. It writes .session.json once the HTTP listener is
  // up, so poll for that file + a healthy /health endpoint instead of relying
  // on piped stdout (which buffers under pipes and is unreliable).
  const child = spawn(process.execPath, [__filename, "__serve__"], {
    stdio: "ignore",
    detached: true,
    env: process.env,
  });
  child.unref();
  const deadline = Date.now() + 90000;
  while (Date.now() < deadline) {
    await new Promise(r => setTimeout(r, 500));
    const s = loadSession();
    if (!s) continue;
    try {
      const res = await fetch(`http://127.0.0.1:${s.port}/health`, { signal: AbortSignal.timeout(1500) });
      if (res.ok) return s.port;
    } catch {}
  }
  throw new Error("harness server did not become healthy within 90s. Run `doctor` and check that the Vite dev server is up and a Chromium binary is configured.");
}

async function client(cmd, args = {}) {
  const port = await ensureServer();
  const res = await httpJson(port, { cmd, args });
  return res;
}

// ---------------------------------------------------------------------------
// CLI argument parsing & command dispatch
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  // Minimal, dependency-free parser. Supports --flag, --opt=value, --opt value,
  // positional values, and --dry-run / --pretty / --help globally.
  const opts = { _: [], dryRun: false, pretty: process.env.CLAVYN_PRETTY === "1" };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--help" || a === "-h") { opts.help = true; continue; }
    if (a === "--dry-run") { opts.dryRun = true; continue; }
    if (a === "--pretty") { opts.pretty = true; continue; }
    if (a.startsWith("--")) {
      const key = a.slice(2);
      if (i + 1 < argv.length && !argv[i + 1].startsWith("--")) { opts[key] = argv[++i]; }
      else if (a.includes("=")) { const [k, v] = a.slice(2).split("="); opts[k] = v; }
      else { opts[key] = true; }
    } else {
      opts._.push(a);
    }
  }
  return opts;
}

const HELP = `control-clavyn — agent-friendly CLI for driving & verifying the Clavyn Tauri app.

USAGE
  node scripts/verify/control-clavyn.mjs <command> [options]

The harness holds a Chromium browser + the Clavyn renderer (loaded from the
Vite dev server with tauri-fixture.js mocking Tauri IPC). State persists
across commands so you can compose: navigate -> snapshot -> screenshot.

OUTPUT
  Every command prints one JSON object on stdout. Use --pretty for indented
  output. Errors: { ok:false, error, remedy } and exit code 1.

HEALTH & LIFECYCLE
  doctor                 Check node, playwright, browser, dev server, page.
  serve                  Start the harness server in the foreground (debug).
  stop                   Stop the harness server and close the browser.
  status                 Is the server up? Current view, url, readiness.

INSPECTION
  info                   App version/platform + current view.
  snapshot               Accessibility tree of the live UI (JSON).
  screenshot [path]      Save a PNG (default ./screenshot-<ts>.png).
  components             List key regions/controls (data-*, roles, labels).
  eval "<expr>"          Evaluate JS in the page, return JSON.
  console [--since N]    Captured console logs (errors/warnings/pageerrors).
  network-log            Every mocked Tauri invoke() call, in order.
  network-summary        Counts of invoke() calls grouped by command.

NAVIGATION
  home                   Go to the Hosts view.
  navigate <view>        Click a sidebar item. Views: hosts, terminal, files,
                         workspaces, identities, keys, known-hosts, vault,
                         settings.
  open-command-palette   Press Ctrl/Cmd+K.
  close-command-palette  Press Escape.
  scroll <up|down> [n]   Scroll the main content area.

INTERACTION
  click --name "text"    Click by visible text (or --role dialog/button).
  click --selector css   Click by CSS selector.
  click-xy <x> <y>       Click at viewport coordinates.
  type "<text>"          Type into the focused element.
  press "<key>"          Press a key, e.g. "Meta+KeyN", "Enter", "Escape".

TERMINAL (mocked transport via fixture)
  new-session            Open a new local-shell terminal tab.
  connect <host>         Connect SSH by host id or visible label (dblclick).
  send "<text>"          Type into the active terminal + Enter.
  wait-settle            Wait for fixture calls to drain + layout to settle.

FIXTURE CONTROL
  fixture state          Inspect connects/closes/writes/live sessions.
  fixture hold-next      Make the next connect() hang until 'release'.
  fixture fail-next      Make the next connect() throw.
  fixture release        Release a held connect.

CLEANUP
  cleanup                Disconnect all live fixture sessions.
  reset                  Reload the page fresh (clears console log too).

GLOBAL OPTIONS
  --dry-run              For destructive commands: show what would happen.
  --pretty               Indent JSON output for humans.
  --help, -h             Show this help.

ENVIRONMENT
  CLAVYN_URL             Renderer URL (default http://127.0.0.1:1420).
  CHROMIUM_EXECUTABLE_PATH  Use a specific Chrome/Chromium binary.
  CLAVYN_HEADLESS=0      Show the browser window.
  CLAVYN_SLOW_MO         Slow down actions by N ms (debugging).
  CLAVYN_AUTO_START_DEV=0  Don't auto-start Vite if :1420 is down.
  CLAVYN_VIEWPORT_W/H    Browser viewport (default 1440x900).

This CLI is agent-agnostic. Agent-specific pointer files:
  AGENTS.md (root)          — universal / Codex
  .devin/skills/verify-clavyn/SKILL.md — Devin
  .claude/commands/verify-clavyn.md    — Claude Code
  .cursor/rules/verify-clavyn.mdc       — Cursor
Full docs: scripts/verify/README.md. Feature map: scripts/verify/features/README.md.
`;

const DESTRUCTIVE = new Set(["stop", "reset", "cleanup", "connect"]);

async function main() {
  const argv = process.argv.slice(2);

  // Internal entry point used by ensureServer() to spawn the daemon.
  if (argv[0] === "__serve__") {
    return startHarnessServer({ detached: true });
  }

  if (argv.length === 0 || argv[0] === "--help" || argv[0] === "-h" || argv[0] === "help") {
    process.stdout.write(HELP);
    return;
  }

  const cmd = argv[0];
  const opts = parseArgs(argv.slice(1));

  if (opts.help) { process.stdout.write(HELP); return; }

  const pretty = opts.pretty;
  const rest = opts._;

  // doctor runs locally (no server needed) so it can diagnose a missing server.
  if (cmd === "doctor") { return runDoctor(pretty); }
  if (cmd === "serve") { return startHarnessServer({ detached: false }); }

  if (DESTRUCTIVE.has(cmd) && opts.dryRun) {
    return out({ ok: true, dryRun: true, cmd, args: rest, note: "No changes would be made." }, pretty);
  }

  try {
    let res;
    switch (cmd) {
      case "stop": res = await client("stop"); clearSession(); break;
      case "status": res = await client("status"); break;
      case "info": res = await client("info"); break;
      case "snapshot": res = await client("snapshot"); break;
      case "screenshot": res = await client("screenshot", { path: rest[0] ?? opts.path }); break;
      case "components": res = await client("components"); break;
      case "eval": res = await client("eval", { expr: rest[0] ?? opts.expr }); break;
      case "console": res = await client("console", { since: opts.since ? Number(opts.since) : 0 }); break;
      case "network-log": res = await client("networkLog"); break;
      case "network-summary": res = await client("networkSummary"); break;
      case "home": res = await client("home"); break;
      case "navigate": res = await client("navigate", { view: rest[0] ?? opts.view }); break;
      case "open-command-palette": res = await client("openPalette"); break;
      case "close-command-palette": res = await client("closePalette"); break;
      case "scroll": res = await client("eval", { expr: `(()=>{const m=document.querySelector('main');m&&m.scrollBy({top:${opts.dir === "up" ? "-" : ""}${Number(rest[1] ?? 400)},behavior:'smooth'});return m?m.scrollTop:null;})()` }); break;
      case "click": res = await client("click", { selector: opts.selector, name: opts.name, role: opts.role }); break;
      case "click-xy": res = await client("clickXY", { x: Number(rest[0]), y: Number(rest[1]) }); break;
      case "type": res = await client("type", { text: rest.join(" ") }); break;
      case "press": res = await client("press", { key: rest[0] ?? opts.key }); break;
      case "new-session": res = await client("newSession"); break;
      case "connect": res = await client("connect", { host: rest[0] ?? opts.host }); break;
      case "send": res = await client("send", { text: rest.join(" ") }); break;
      case "wait-settle": res = await client("waitSettle"); break;
      case "fixture": res = await client("fixture", { op: rest[0] ?? opts.op }); break;
      case "cleanup": res = await client("cleanup"); break;
      case "reset": res = await client("reset"); break;
      default:
        return fail(`Unknown command '${cmd}'.`, "Run `node scripts/verify/control-clavyn.mjs help` to see all commands.");
    }
    out(res, pretty);
    if (res && res.ok === false) process.exit(1);
  } catch (e) {
    fail(String(e.message ?? e), "Run `node scripts/verify/control-clavyn.mjs doctor` to diagnose, or `node scripts/verify/control-clavyn.mjs stop` then retry.");
  }
}

// ---------------------------------------------------------------------------
// doctor — local diagnostics (no running server required)
// ---------------------------------------------------------------------------

async function runDoctor(pretty) {
  const report = { ok: true, checks: {} };

  report.checks.node = { ok: true, version: process.version };

  const pw = loadPlaywright();
  if (pw) report.checks.playwright = { ok: true, version: pw._version ?? "unknown" };
  else report.checks.playwright = { ok: false, remedy: "Run `npm install` in desktop/e2e (pins playwright 1.56.1)." };

  // Browser binary
  let browserOk = !!CFG.chromiumPath;
  if (!browserOk && pw) {
    try {
      const exe = pw.chromium.executablePath();
      browserOk = !!exe && existsSync(exe);
      if (browserOk) report.checks.browser = { ok: true, path: exe };
    } catch {
      browserOk = false;
    }
  } else if (CFG.chromiumPath) {
    browserOk = existsSync(CFG.chromiumPath);
    report.checks.browser = { ok: browserOk, path: CFG.chromiumPath };
  }
  if (!browserOk) {
    report.checks.browser = report.checks.browser ?? { ok: false };
    report.checks.browser.remedy = "Install with `cd desktop/e2e && npx playwright install chromium`, or set CHROMIUM_EXECUTABLE_PATH to a system Chrome.";
  }

  // Dev server
  const devUp = await isPortResponding(1420);
  report.checks.devServer = { ok: devUp, url: CFG.url, remedy: devUp ? null : "Run `cd desktop && npm run dev`, or set CLAVYN_URL. The harness will auto-start Vite if CLAVYN_AUTO_START_DEV is unset." };

  // Fixture file
  report.checks.fixture = { ok: existsSync(FIXTURE_PATH), path: FIXTURE_PATH };

  // Harness server
  const sess = loadSession();
  let serverUp = false;
  if (sess) {
    try {
      const res = await fetch(`http://127.0.0.1:${sess.port}/health`, { signal: AbortSignal.timeout(1500) });
      serverUp = res.ok;
      const body = await res.json();
      report.checks.harness = { ok: true, port: sess.port, ready: body.ready };
    } catch {
      report.checks.harness = { ok: false, remedy: "Stale session file. Run `node scripts/verify/control-clavyn.mjs stop` then retry." };
    }
  } else {
    report.checks.harness = { ok: false, remedy: "Not running yet — it auto-starts on the next command, or run `serve`." };
  }

  report.ok = Object.values(report.checks).every(c => c.ok);
  out(report, pretty);
  if (!report.ok) process.exit(1);
}

main().catch(e => fail(String(e), "Run `node scripts/verify/control-clavyn.mjs doctor`."));
