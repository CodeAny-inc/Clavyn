#!/usr/bin/env node
// Records a video of the host-address masking feature for PR proof.
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

async function currentView(page) {
  return page.evaluate(() => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    if (!aside) return null;
    const active = aside.querySelector("[aria-current='page']");
    return active?.textContent?.trim()?.toLowerCase() ?? null;
  });
}

async function navigateTo(page, label) {
  await page.evaluate((l) => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    const btn = Array.from(aside.querySelectorAll("button")).find(b => b.textContent?.trim() === l);
    btn?.click();
  }, label);
  await sleep(800);
}

async function clickMaskToggle(page, on) {
  await page.evaluate(() => {
    const switches = Array.from(document.querySelectorAll('[role="switch"]'));
    const maskSwitch = switches.find(s => s.parentElement?.textContent?.includes("Mask host addresses"));
    if (maskSwitch) maskSwitch.click();
  });
  await sleep(500);
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

  // Inject the tauri-fixture before any app script runs.
  if (existsSync(FIXTURE_PATH)) {
    await page.addInitScript({ path: FIXTURE_PATH });
  }

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);

  // --- Scene 1: Hosts view with masked addresses (default) ---
  await navigateTo(page, "Hosts");
  await sleep(2000);
  await page.screenshot({ path: join(OUT_DIR, "mask-addresses-01-hosts-masked.png") });

  // --- Scene 2: Settings — toggle is ON by default ---
  await navigateTo(page, "Settings");
  await sleep(1500);
  await page.screenshot({ path: join(OUT_DIR, "mask-addresses-02-settings-toggle-on.png") });

  // --- Scene 3: Toggle OFF ---
  await clickMaskToggle(page, false);
  await sleep(1000);
  await page.screenshot({ path: join(OUT_DIR, "mask-addresses-03-settings-toggle-off.png") });

  // --- Scene 4: Hosts view with full addresses ---
  await navigateTo(page, "Hosts");
  await sleep(2000);
  await page.screenshot({ path: join(OUT_DIR, "mask-addresses-04-hosts-unmasked.png") });

  // --- Scene 5: Toggle back ON, connect to host, show terminal ---
  await navigateTo(page, "Settings");
  await sleep(1000);
  await clickMaskToggle(page, true);
  await sleep(500);

  await navigateTo(page, "Hosts");
  await sleep(1000);
  // Connect to Atlas Production
  await page.evaluate(() => {
    const btn = Array.from(document.querySelectorAll("button")).find(b => b.textContent?.trim() === "Connect Atlas Production");
    btn?.click();
  });
  await sleep(3000);
  await page.screenshot({ path: join(OUT_DIR, "mask-addresses-05-terminal-masked.png") });

  // Close to finalize the video
  await page.close();
  await context.close();
  await browser.close();

  // Find the recorded video file
  const fs = await import("node:fs");
  const files = fs.readdirSync(OUT_DIR).filter(f => f.endsWith(".webm")).sort();
  if (files.length) {
    const videoFile = files[files.length - 1];
    const videoPath = join(OUT_DIR, "mask-addresses-recording.webm");
    if (existsSync(videoPath)) fs.unlinkSync(videoPath);
    fs.renameSync(join(OUT_DIR, videoFile), videoPath);
    console.log(JSON.stringify({ ok: true, video: videoPath, screenshots: 5 }));
  } else {
    console.log(JSON.stringify({ ok: true, video: null, screenshots: 5 }));
  }
}

main().catch(e => { console.error(JSON.stringify({ ok: false, error: String(e) })); process.exit(1); });
