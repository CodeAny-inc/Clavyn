#!/usr/bin/env node
// Records a multi-scenario video demonstrating the fullscreen fix:
// 1. Standard viewport — enter/exit fullscreen
// 2. Ultrawide viewport (2882x816, the reported issue dimensions)
// 3. Split panes — fullscreen one pane, exit, both panes intact
// 4. Short window — fullscreen works even with limited vertical space
import { createRequire } from "node:module";
import { existsSync, readdirSync, unlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const REPO_ROOT = resolve(__dirname, "../..");
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");
const OUT_DIR = join(REPO_ROOT, "screenshots");

const require = createRequire(import.meta.url);
const pw = require(require.resolve("playwright", { paths: [E2E_DIR] }));
const URL = process.env.CLAVYN_URL ?? "http://127.0.0.1:1420";
const chromiumPath = process.env.CHROMIUM_EXECUTABLE_PATH ?? null;

function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }

async function openTerminal(page) {
  await page.evaluate(() => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    const btn = Array.from(aside.querySelectorAll("button")).find(b => b.textContent?.trim() === "Terminal");
    btn?.click();
  });
  await sleep(800);
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
  await sleep(2000);
}

async function typeContent(page, text) {
  await page.locator(".terminal-pane[data-active=true] .xterm-helper-textarea").focus();
  for (const line of text) {
    await page.keyboard.type(line, { delay: 12 });
    await page.keyboard.press("Enter");
    await sleep(400);
  }
}

async function recordScenario(context, page, recorder, label, lines) {
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);
  await openTerminal(page);
  await typeContent(page, lines);
  await sleep(500);

  // Enter fullscreen
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Fullscreen"], .terminal-pane button[title*="Maximize"]')?.click();
  });
  await sleep(2500);

  // Type while in fullscreen to show it's live
  await page.locator(".terminal-pane[data-active=true] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo 'fullscreen is live'", { delay: 12 });
  await page.keyboard.press("Enter");
  await sleep(1500);

  // Exit fullscreen
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Exit fullscreen"], .terminal-pane button[title*="Restore"]')?.click();
  });
  await sleep(2000);
}

async function main() {
  // Clean old fullscreen recordings
  for (const f of readdirSync(OUT_DIR)) {
    if (f.startsWith("fullscreen-recording")) unlinkSync(join(OUT_DIR, f));
  }

  const browser = await pw.chromium.launch({
    headless: true,
    ...(chromiumPath ? { executablePath: chromiumPath } : {}),
    args: ["--no-sandbox"],
  });

  const scenarios = [
    { width: 1440, height: 900, label: "standard", lines: ["echo 'Standard viewport 1440x900'", "ls -la"] },
    { width: 2882, height: 816, label: "ultrawide", lines: ["echo 'Ultrawide 2882x816 — the reported issue'", "pwd"] },
    { width: 1440, height: 408, label: "short", lines: ["echo 'Short window 1440x408'", "whoami"] },
  ];

  const videoFile = join(OUT_DIR, "fullscreen-recording.webm");
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    colorScheme: "light",
    recordVideo: { dir: OUT_DIR, size: { width: 1440, height: 900 } },
  });
  const page = await context.newPage();
  if (existsSync(FIXTURE_PATH)) await page.addInitScript({ path: FIXTURE_PATH });

  for (const scenario of scenarios) {
    await page.setViewportSize({ width: scenario.width, height: scenario.height });
    await sleep(500);
    await recordScenario(context, page, null, scenario.label, scenario.lines);
  }

  // Split + fullscreen scenario
  await page.setViewportSize({ width: 1440, height: 900 });
  await sleep(500);
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);
  await openTerminal(page);
  await typeContent(page, ["echo 'Split scenario'"]);

  // Split right
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Actions for Local shell"]')?.click();
  });
  await sleep(500);
  await page.evaluate(() => {
    const items = document.querySelectorAll('[role="menu"] [role="menuitem"], [role="menu"] button');
    const split = Array.from(items).find(b => b.textContent?.includes("Split right"));
    split?.click();
  });
  await sleep(1500);
  await page.screenshot({ path: join(OUT_DIR, "fullscreen-recording-split-01.png") });

  // Fullscreen first pane
  await page.evaluate(() => {
    const panes = document.querySelectorAll(".terminal-pane");
    panes[0]?.querySelector('button[aria-label="Fullscreen"], button[title*="Maximize"]')?.click();
  });
  await sleep(2500);
  await page.screenshot({ path: join(OUT_DIR, "fullscreen-recording-split-02.png") });

  // Type in fullscreen
  await page.locator(".terminal-pane[data-active=true] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo 'fullscreen from split'", { delay: 12 });
  await page.keyboard.press("Enter");
  await sleep(1500);

  // Exit fullscreen
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Exit fullscreen"], .terminal-pane button[title*="Restore"]')?.click();
  });
  await sleep(2000);
  await page.screenshot({ path: join(OUT_DIR, "fullscreen-recording-split-03.png") });

  await page.close();
  await context.close();

  // Rename the generated video
  const generated = readdirSync(OUT_DIR).find(f => f.endsWith(".webm") && !f.startsWith("drag-split") && !f.startsWith("fullscreen-recording"));
  if (generated) {
    const from = join(OUT_DIR, generated);
    const fs = require("fs");
    fs.renameSync(from, videoFile);
    console.log(`Video saved: ${videoFile}`);
  } else {
    // Try finding the video in the default location
    const webmFiles = readdirSync(OUT_DIR).filter(f => f.endsWith(".webm"));
    console.log("WebM files:", webmFiles);
  }

  await browser.close();
}

main().catch(e => { console.error(String(e)); process.exit(1); });
