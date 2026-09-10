#!/usr/bin/env node
// Verifies the Termius-style drag-split feature end-to-end in the mocked renderer.
// Asserts: split-on-drop, directional swap, extract-to-new-tab, and that live
// sessions survive every move (connection never interrupted).
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = resolve(__dirname, "../..");
const require = createRequire(import.meta.url);
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");

const pw = require(require.resolve("playwright", { paths: [E2E_DIR] }));
const URL = process.env.CLAVYN_URL ?? "http://127.0.0.1:1420";
const chromiumPath = process.env.CHROMIUM_EXECUTABLE_PATH ?? null;

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }
const results = [];
function check(name, cond, detail = "") {
  results.push({ name, ok: !!cond, detail });
  console.log(`${cond ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
}

async function navigateTo(page, label) {
  await page.evaluate((l) => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    const btn = Array.from(aside.querySelectorAll("button")).find(b => b.textContent?.trim() === l);
    btn?.click();
  }, label);
  await sleep(800);
}

async function openLocalShellTab(page) {
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab").catch(async () => {});
  await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
  await sleep(1800);
}

async function state(page) {
  return page.evaluate(() => {
    const panes = [...document.querySelectorAll(".terminal-pane")];
    const tabs = [...document.querySelectorAll("[data-testid='session-strip'] [data-tab-id]")];
    return {
      tabCount: tabs.length,
      paneCount: panes.length,
      sessions: panes.map(p => p.getAttribute("data-session-id")),
      connected: panes.map(p => p.getAttribute("data-connected")),
    };
  });
}

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
    for (let i = 0; i < 3; i++) target.dispatchEvent(new DragEvent("dragover", opts));
    target.dispatchEvent(new DragEvent("drop", opts));
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer }));
  }, { sourceSelector, targetSelector, dropPoint });
}

async function main() {
  const browser = await pw.chromium.launch({ headless: true, ...(chromiumPath ? { executablePath: chromiumPath } : {}) });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light" });
  const page = await context.newPage();
  if (existsSync(FIXTURE_PATH)) await page.addInitScript({ path: FIXTURE_PATH });
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);

  await navigateTo(page, "Terminal");
  await sleep(600);
  await openLocalShellTab(page);
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo A", { delay: 10 });
  await page.keyboard.press("Enter");
  await sleep(1000);
  await openLocalShellTab(page);
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo B", { delay: 10 });
  await page.keyboard.press("Enter");
  await sleep(1000);

  let s = await state(page);
  // All panes across every tab render in the DOM (v-show toggles visibility),
  // so two single-pane tabs show two .terminal-pane elements.
  check("setup: two tabs, one pane each", s.tabCount === 2 && s.paneCount === 2, `tabs=${s.tabCount} panes=${s.paneCount}`);
  const sessionB = s.sessions[1];

  // Split-on-drop: drag tab B's pane onto tab A's tab button.
  const tabButtons = page.locator("[data-testid='session-strip'] [data-tab-id]");
  await tabButtons.nth(1).click();
  await sleep(500);
  await html5DragDrop(page,
    ".terminal-pane[data-active='true'] .pane-header [draggable='true']",
    "[data-testid='session-strip'] [data-tab-id]", { x: 0.5, y: 0.5 });
  await sleep(800);
  await tabButtons.nth(0).click();
  await sleep(700);
  s = await state(page);
  check("split-on-drop: one tab with two panes", s.tabCount === 1 && s.paneCount === 2, `tabs=${s.tabCount} panes=${s.paneCount}`);
  check("split-on-drop: both panes connected", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);
  check("split-on-drop: moved session preserved", s.sessions.includes(sessionB), `sessions=${JSON.stringify(s.sessions)}`);

  // Swap: drag first pane onto second pane's center. Panes are wrapped in divs,
  // so select by index rather than nth-of-type.
  await page.evaluate(() => {
    const source = document.querySelector(".terminal-pane .pane-header [draggable='true']");
    const target = document.querySelectorAll(".terminal-pane")[1];
    if (!source || !target) throw new Error("missing swap nodes");
    const dataTransfer = new DataTransfer();
    const r = target.getBoundingClientRect();
    const opts = { bubbles: true, cancelable: true, dataTransfer, clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 };
    source.dispatchEvent(new DragEvent("dragstart", { ...opts, clientX: 0, clientY: 0 }));
    for (let i = 0; i < 3; i++) target.dispatchEvent(new DragEvent("dragover", opts));
    target.dispatchEvent(new DragEvent("drop", opts));
    document.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer }));
  });
  await sleep(700);
  s = await state(page);
  check("swap: still two panes", s.paneCount === 2, `panes=${s.paneCount}`);
  check("swap: both still connected", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);

  // Extract: drag a pane onto the New-session drop area.
  await html5DragDrop(page,
    ".terminal-pane .pane-header [draggable='true']",
    ".new-session", { x: 0.5, y: 0.5 });
  await sleep(800);
  s = await state(page);
  check("extract: two tabs again", s.tabCount === 2, `tabs=${s.tabCount}`);
  check("extract: one pane per tab", s.paneCount === 2, `panes=${s.paneCount}`);
  check("extract: both still connected", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);

  // Tab swap: drag the second tab onto the center of the visible pane — the
  // two single-pane tabs trade terminals while both keep their sessions.
  await tabButtons.nth(0).click();
  await sleep(500);
  await html5DragDrop(page,
    "[data-testid='session-strip'] .session-tab:nth-of-type(2)",
    ".terminal-pane", { x: 0.5, y: 0.5 });
  await sleep(800);
  s = await state(page);
  check("tab swap: still two tabs, one pane each", s.tabCount === 2 && s.paneCount === 2, `tabs=${s.tabCount} panes=${s.paneCount}`);
  check("tab swap: both still connected", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);

  // Tab drop: dropping the second tab onto the visible pane's edge splits it.
  await tabButtons.nth(0).click();
  await sleep(500);
  await html5DragDrop(page,
    "[data-testid='session-strip'] .session-tab:nth-of-type(2)",
    ".terminal-pane", { x: 0.9, y: 0.5 });
  await sleep(800);
  s = await state(page);
  check("tab-drop: one tab with two panes", s.tabCount === 1 && s.paneCount === 2, `tabs=${s.tabCount} panes=${s.paneCount}`);
  check("tab-drop: both still connected", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);

  // Tab reorder: open another tab, then drag it before the first in the strip.
  await openLocalShellTab(page);
  s = await state(page);
  check("reorder setup: two tabs", s.tabCount === 2, `tabs=${s.tabCount}`);
  const orderBefore = await page.evaluate(() =>
    [...document.querySelectorAll("[data-tab-id]")].map(el => el.dataset.tabId));
  await html5DragDrop(page,
    "[data-testid='session-strip'] .session-tab:nth-of-type(2)",
    "[data-testid='session-strip'] .session-tab:nth-of-type(1)", { x: 0.15, y: 0.5 });
  await sleep(600);
  const orderAfter = await page.evaluate(() =>
    [...document.querySelectorAll("[data-tab-id]")].map(el => el.dataset.tabId));
  check("tab reorder: dragged tab moved to front",
    orderAfter[0] === orderBefore[1] && orderAfter[1] === orderBefore[0],
    `order=${JSON.stringify(orderAfter.map(id => orderBefore.indexOf(id)))}`);
  s = await state(page);
  check("reorder: sessions untouched", s.connected.every(c => c === "true"), `connected=${JSON.stringify(s.connected)}`);

  await page.close();
  await context.close();
  await browser.close();

  const ok = results.every(r => r.ok);
  console.log(`\n${ok ? "ALL PASS" : "SOME FAILED"} (${results.filter(r => r.ok).length}/${results.length})`);
  process.exit(ok ? 0 : 1);
}

main().catch(e => { console.error(String(e)); process.exit(1); });
