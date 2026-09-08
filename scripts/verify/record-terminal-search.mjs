#!/usr/bin/env node
// Records a video of the terminal search results feedback feature for PR proof.
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

async function getSearchSummary(page) {
  return page.evaluate(() => document.querySelector('.terminal-pane [aria-live="polite"]')?.textContent ?? "");
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

  // --- Navigate to Terminal and create a local session ---
  await navigateTo(page, "Terminal");
  await sleep(1000);

  // Click "New session", select "tab", then "Open local shell"
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab").catch(async () => {});
  await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
  await sleep(2000);

  // Type output with repeated words so search has multiple matches
  await page.locator(".terminal-pane[data-active='true'] .xterm-helper-textarea").focus();
  await page.keyboard.type("echo error error error warning warning info error", { delay: 15 });
  await page.keyboard.press("Enter");
  await sleep(1500);

  // --- Scene 1: Open search and type "error" — shows "1 of 8" ---
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Search in terminal"]')?.click();
  });
  await sleep(500);
  await page.locator('.terminal-pane input[aria-label="Search terminal output"]').focus();
  await page.keyboard.type("error", { delay: 80 });
  await sleep(1000);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-01-error-1-of-8.png") });

  // --- Scene 2: Click next match — shows "2 of 8" ---
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Next match"]')?.click();
  });
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-02-error-2-of-8.png") });

  // --- Scene 3: Click next match again — shows "3 of 8" ---
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Next match"]')?.click();
  });
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-03-error-3-of-8.png") });

  // --- Scene 4: Click previous match — wraps to "8 of 8" ---
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Previous match"]')?.click();
  });
  await sleep(800);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-04-error-8-of-8.png") });

  // --- Scene 5: Search for "warning" — shows "1 of 4" ---
  await page.locator('.terminal-pane input[aria-label="Search terminal output"]').focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("warning", { delay: 80 });
  await sleep(1000);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-05-warning-1-of-4.png") });

  // --- Scene 6: Search for non-existent term — shows "No results" ---
  await page.locator('.terminal-pane input[aria-label="Search terminal output"]').focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("nonexistent", { delay: 80 });
  await sleep(1000);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-06-no-results.png") });

  // --- Scene 7: Close search ---
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Close search"]')?.click();
  });
  await sleep(500);
  await page.screenshot({ path: join(OUT_DIR, "terminal-search-07-closed.png") });

  // Close to finalize the video
  await page.close();
  await context.close();
  await browser.close();

  // Find the recorded video file
  const fs = await import("node:fs");
  const files = fs.readdirSync(OUT_DIR).filter(f => f.endsWith(".webm")).sort();
  if (files.length) {
    const videoFile = files[files.length - 1];
    const videoPath = join(OUT_DIR, "terminal-search-recording.webm");
    if (existsSync(videoPath)) fs.unlinkSync(videoPath);
    fs.renameSync(join(OUT_DIR, videoFile), videoPath);
    console.log(JSON.stringify({ ok: true, video: videoPath, screenshots: 7 }));
  } else {
    console.log(JSON.stringify({ ok: true, video: null, screenshots: 7 }));
  }
}

main().catch(e => { console.error(JSON.stringify({ ok: false, error: String(e) })); process.exit(1); });
