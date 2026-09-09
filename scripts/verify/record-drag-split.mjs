#!/usr/bin/env node
// Records a video of the Termius-style drag-and-drop split feature for PR proof.
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

// Open a local shell in a new tab via the New session picker.
async function openLocalShellTab(page) {
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab").catch(async () => {});
  await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
  await sleep(1800);
}

// Dispatch a full HTML5 drag-and-drop sequence between two elements. Headless
// Chromium does not synthesize a native drag ghost, so we fire the DragEvents
// directly: dragstart on the source, repeated dragover on the target (which the
// Vue handlers preventDefault and use to set the drop position), then drop.
async function html5DragDrop(page, sourceSelector, targetSelector, dropPoint = { x: 0.5, y: 0.5 }) {
  await page.evaluate(({ sourceSelector, targetSelector, dropPoint }) => {
    const source = document.querySelector(sourceSelector);
    const target = document.querySelector(targetSelector);
    if (!source || !target) throw new Error(`missing node: ${sourceSelector} or ${targetSelector}`);
    const dataTransfer = new DataTransfer();
    const targetRect = target.getBoundingClientRect();
    const clientX = targetRect.left + targetRect.width * dropPoint.x;
    const clientY = targetRect.top + targetRect.height * dropPoint.y;
    const opts = { bubbles: true, cancelable: true, dataTransfer, clientX, clientY };
    source.dispatchEvent(new DragEvent("dragstart", { ...opts, clientX: 0, clientY: 0 }));
    // Several dragover events let the reactive overlay settle on a position.
    for (let i = 0; i < 3; i++) target.dispatchEvent(new DragEvent("dragover", opts));
    target.dispatchEvent(new DragEvent("drop", opts));
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer }));
  }, { sourceSelector, targetSelector, dropPoint });
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

  if (existsSync(FIXTURE_PATH)) {
    await page.addInitScript({ path: FIXTURE_PATH });
  }

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);

  // --- Setup: two tabs, each with a local shell, with identifying output ---
  await navigateTo(page, "Terminal");
  await sleep(800);
  // First tab already has a local shell from the empty state? Open one explicitly.
  await openLocalShellTab(page);
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo TAB-A session alpha", { delay: 12 });
  await page.keyboard.press("Enter");
  await sleep(1200);

  // Second tab
  await openLocalShellTab(page);
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo TAB-B session beta", { delay: 12 });
  await page.keyboard.press("Enter");
  await sleep(1200);

  await page.screenshot({ path: join(OUT_DIR, "drag-split-01-two-tabs.png") });

  // --- Scene 2: Drag TAB-B's pane header onto TAB-A's tab to split-on-drop ---
  // Switch to tab B so its pane header is the active drag source.
  const tabButtons = page.locator("[data-testid='session-strip'] [data-tab-id]");
  await tabButtons.nth(1).click();
  await sleep(600);

  // Drag the active pane header (grip handle) onto the first tab in the strip.
  await html5DragDrop(page,
    ".terminal-pane[data-active='true'] .pane-header [draggable='true']",
    "[data-testid='session-strip'] [data-tab-id]",
    { x: 0.5, y: 0.5 });
  await sleep(1000);

  // The drop moves pane B into tab A as a horizontal split; activate tab A.
  await tabButtons.nth(0).click();
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "drag-split-02-split-on-drop.png") });

  // --- Scene 3: Show the directional drop overlay by dragging one pane over the other ---
  // Start a drag of the left pane header and hover over the right pane (right edge).
  await page.evaluate(() => {
    const source = document.querySelector(".terminal-pane .pane-header [draggable='true']");
    const target = document.querySelectorAll(".terminal-pane")[1] ?? document.querySelector(".terminal-pane");
    if (!source || !target) return;
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    source.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer, clientX: 0, clientY: 0 }));
    // Hover the right edge to highlight the "Split right" zone.
    target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.85, clientY: r.top + r.height * 0.5 }));
  });
  await sleep(700);
  await page.screenshot({ path: join(OUT_DIR, "drag-split-03-drop-zones.png") });

  // --- Scene 4: Drop on center to swap the two adjacent panes ---
  await page.evaluate(() => {
    const source = document.querySelector(".terminal-pane .pane-header [draggable='true']");
    const target = document.querySelectorAll(".terminal-pane")[1] ?? document.querySelector(".terminal-pane");
    if (!source || !target) return;
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    source.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer, clientX: 0, clientY: 0 }));
    target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 }));
    target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 }));
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer }));
  });
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "drag-split-04-swapped.png") });

  // --- Scene 5: Extract a pane back into its own tab via the New-session drop area ---
  await page.evaluate(() => {
    const source = document.querySelector(".terminal-pane .pane-header [draggable='true']");
    const target = document.querySelector(".new-session-drop-target, .new-session");
    if (!source || !target) return;
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    source.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer, clientX: 0, clientY: 0 }));
    target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 }));
    target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 }));
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer }));
  });
  await sleep(900);
  await page.screenshot({ path: join(OUT_DIR, "drag-split-05-extracted-to-new-tab.png") });

  // Close to finalize the video
  await page.close();
  await context.close();
  await browser.close();

  // Find the recorded video file
  const fs = await import("node:fs");
  const files = fs.readdirSync(OUT_DIR).filter(f => f.endsWith(".webm")).sort();
  if (files.length) {
    const videoFile = files[files.length - 1];
    const videoPath = join(OUT_DIR, "drag-split-recording.webm");
    if (existsSync(videoPath)) fs.unlinkSync(videoPath);
    fs.renameSync(join(OUT_DIR, videoFile), videoPath);
    console.log(JSON.stringify({ ok: true, video: videoPath, screenshots: 5 }));
  } else {
    console.log(JSON.stringify({ ok: true, video: null, screenshots: 5 }));
  }
}

main().catch(e => { console.error(JSON.stringify({ ok: false, error: String(e) })); process.exit(1); });
