#!/usr/bin/env node
// Records a video of tab-level drag-and-drop for PR proof: dragging a session
// tab onto a terminal pane splits it (Termius-style), dropping on the center
// swaps two terminals across tabs, and dragging along the strip reorders tabs.
// Uses the same tauri-fixture.js as the e2e suite so the UI is fully mocked.
import { createRequire } from "node:module";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = resolve(__dirname, "../..");
const require = createRequire(import.meta.url);
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");
const OUT_DIR = join(REPO_ROOT, "screenshots");

if (!existsSync(OUT_DIR)) mkdirSync(OUT_DIR, { recursive: true });

const pw = require(require.resolve("playwright", { paths: [E2E_DIR] }));

const URL = process.env.CLAVYN_URL ?? "http://127.0.0.1:1420";
const chromiumPath = process.env.CHROMIUM_EXECUTABLE_PATH ?? null;

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

async function navigateTo(page, label) {
  await page.evaluate((l) => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    const btn = Array.from(aside.querySelectorAll("button")).find(b => b.textContent?.trim() === l);
    btn?.click();
  }, label);
  await sleep(800);
}

// Connect a saved fixture host into its own tab via the New session picker.
async function openHostTab(page, hostId, label) {
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await sleep(400);
  await page.getByRole("button", { name: `Connect ${label}`, exact: true }).click();
  await page.waitForFunction(h =>
    document.querySelector(`[data-host-id="${h}"]`)?.getAttribute("data-connected") === "true",
    hostId, { timeout: 10000 });
  await sleep(800);
}

async function typeInActivePane(page, text) {
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type(text, { delay: 14 });
  await page.keyboard.press("Enter");
  await sleep(900);
}

// Dispatch HTML5 drag events directly: headless Chromium does not synthesize a
// native drag ghost, so dragstart fires on the source tab, dragover on the
// target (the Vue handlers preventDefault and resolve the drop position), then
// drop and dragend complete the gesture.
// Pause mid-drag on the overlay: dragstart + one dragover, no drop.
async function tabDragHover(page, sourceTabIndex, targetSelector, dropPoint) {
  await page.evaluate(({ sourceTabIndex, targetSelector, dropPoint }) => {
    const source = document.querySelectorAll("[data-testid='session-strip'] .session-tab")[sourceTabIndex];
    const target = document.querySelector(targetSelector);
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    const opts = { bubbles: true, cancelable: true, dataTransfer,
      clientX: r.left + r.width * dropPoint.x, clientY: r.top + r.height * dropPoint.y };
    source.dispatchEvent(new DragEvent("dragstart", { ...opts, clientX: 0, clientY: 0 }));
    target.dispatchEvent(new DragEvent("dragover", opts));
  }, { sourceTabIndex, targetSelector, dropPoint });
}
async function finishTabDrag(page, sourceTabIndex, targetSelector, dropPoint) {
  await page.evaluate(({ sourceTabIndex, targetSelector, dropPoint }) => {
    const source = document.querySelectorAll("[data-testid='session-strip'] .session-tab")[sourceTabIndex];
    const target = document.querySelector(targetSelector);
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    const opts = { bubbles: true, cancelable: true, dataTransfer,
      clientX: r.left + r.width * dropPoint.x, clientY: r.top + r.height * dropPoint.y };
    target.dispatchEvent(new DragEvent("drop", opts));
    source.dispatchEvent(new DragEvent("dragend", { ...opts, clientX: 0, clientY: 0 }));
  }, { sourceTabIndex, targetSelector, dropPoint });
}

async function main() {
  const browser = await pw.chromium.launch({
    headless: true,
    ...(chromiumPath ? { executablePath: chromiumPath } : {}),
  });

  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    colorScheme: "light",
    recordVideo: { dir: OUT_DIR, size: { width: 1440, height: 900 } },
  });

  const page = await context.newPage();

  if (existsSync(FIXTURE_PATH)) await page.addInitScript({ path: FIXTURE_PATH });
  await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);

  // --- Setup: two tabs, Atlas + Orion, each with identifying output ---
  await navigateTo(page, "Terminal");
  await openHostTab(page, "atlas", "Atlas Production");
  await typeInActivePane(page, "echo TAB A — atlas");
  await openHostTab(page, "orion", "Orion Staging");
  await typeInActivePane(page, "echo TAB B — orion");
  await page.locator("[data-tab-id]").nth(0).click();
  await sleep(700);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-01-two-tabs.png") });

  // --- Scene: drag tab B over tab A's terminal — the drop zones appear ---
  await tabDragHover(page, 1, ".terminal-pane[data-host-id='atlas']", { x: 0.9, y: 0.5 });
  await sleep(900);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-02-drop-zones.png") });

  // --- Drop on the right edge: the terminal splits in two automatically ---
  await finishTabDrag(page, 1, ".terminal-pane[data-host-id='atlas']", { x: 0.9, y: 0.5 });
  await sleep(1000);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-03-split-on-drop.png") });
  await typeInActivePane(page, "echo still live after drop");

  // --- Scene: open a third (local) tab and reorder it in front of the strip ---
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await sleep(400);
  await page.getByRole("button", { name: "Open local shell", exact: true }).click();
  await sleep(1400);
  // Pause mid-drag on the first tab's left half so the insertion bar shows.
  await page.evaluate(() => {
    const tabs = document.querySelectorAll("[data-testid='session-strip'] .session-tab");
    const source = tabs[1];
    const target = tabs[0];
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    const opts = { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.2, clientY: r.top + r.height / 2 };
    source.dispatchEvent(new DragEvent("dragstart", { ...opts, clientX: 0, clientY: 0 }));
    target.dispatchEvent(new DragEvent("dragover", opts));
  });
  await sleep(700);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-04-reorder-hint.png") });
  await page.evaluate(() => {
    const tabs = document.querySelectorAll("[data-testid='session-strip'] .session-tab");
    const source = tabs[1];
    const target = tabs[0];
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    const opts = { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.2, clientY: r.top + r.height / 2 };
    target.dispatchEvent(new DragEvent("drop", opts));
    source.dispatchEvent(new DragEvent("dragend", { ...opts, clientX: 0, clientY: 0 }));
  });
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-05-reordered.png") });

  // --- Scene: drag the second tab onto the visible terminal's center — swap ---
  await page.locator("[data-tab-id]").nth(1).click();
  await sleep(600);
  await tabDragHover(page, 0, ".terminal-pane[data-active='true']", { x: 0.5, y: 0.5 });
  await sleep(700);
  await finishTabDrag(page, 0, ".terminal-pane[data-active='true']", { x: 0.5, y: 0.5 });
  await sleep(900);
  await page.screenshot({ path: join(OUT_DIR, "tab-drag-06-swapped.png") });

  await page.close();
  await context.close();
  await browser.close();

  const fs = await import("node:fs");
  const files = fs.readdirSync(OUT_DIR).filter(f => f.endsWith(".webm"))
    .sort((a, b) => fs.statSync(join(OUT_DIR, b)).mtimeMs - fs.statSync(join(OUT_DIR, a)).mtimeMs);
  if (files.length) {
    const videoFile = files[0];
    const videoPath = join(OUT_DIR, "tab-drag-recording.webm");
    if (existsSync(videoPath)) fs.unlinkSync(videoPath);
    fs.renameSync(join(OUT_DIR, videoFile), videoPath);
    console.log(JSON.stringify({ ok: true, video: videoPath, screenshots: 6 }));
  } else {
    console.log(JSON.stringify({ ok: true, video: null, screenshots: 6 }));
  }
}

main().catch(e => { console.error(JSON.stringify({ ok: false, error: String(e) })); process.exit(1); });
