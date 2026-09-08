#!/usr/bin/env node
// Verifies fullscreen terminal pane rendering across multiple viewport sizes
// and scenarios. Confirms the Teleport fix: the pane escapes ancestor
// overflow:hidden / display:none containers and the terminal stays visible.
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
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

async function checkFullscreen(page, label) {
  const info = await page.evaluate(() => {
    const pane = document.querySelector(".terminal-pane");
    const container = pane?.querySelector(".min-h-0");
    const xterm = pane?.querySelector(".xterm");
    const rows = pane?.querySelector(".xterm-rows");
    const header = pane?.querySelector(".pane-header");
    const isFixed = pane?.classList.contains("fixed");
    const isInBody = pane?.parentElement === document.body;
    return {
      isFixed,
      isInBody,
      paneW: pane?.offsetWidth,
      paneH: pane?.offsetHeight,
      containerW: container?.offsetWidth,
      containerH: container?.offsetHeight,
      xtermW: xterm?.offsetWidth,
      xtermH: xterm?.offsetHeight,
      headerH: header?.offsetHeight,
      hasContent: rows?.children.length > 0,
      firstRow: rows?.children[0]?.textContent?.substring(0, 40),
      viewportW: window.innerWidth,
      viewportH: window.innerHeight,
    };
  });

  const checks = [
    ["pane covers viewport width", info.paneW === info.viewportW],
    ["pane covers viewport height", info.paneH === info.viewportH],
    ["pane is position:fixed", info.isFixed],
    ["pane is teleported to body", info.isInBody],
    ["terminal container has width", info.containerW > 0],
    ["terminal container has height", info.containerH > 0],
    ["xterm has width", info.xtermW > 0],
    ["xterm has height", info.xtermH > 0],
    ["xterm has rendered rows", info.hasContent],
    ["pane height = viewport height", info.paneH === info.viewportH],
  ];

  let allPass = true;
  for (const [name, pass] of checks) {
    const status = pass ? "PASS" : "FAIL";
    if (!pass) allPass = false;
    console.log(`${status} ${label}: ${name}`);
  }
  console.log(`  dims: pane=${info.paneW}x${info.paneH} container=${info.containerW}x${info.containerH} xterm=${info.xtermW}x${info.xtermH} viewport=${info.viewportW}x${info.viewportH}`);
  return { allPass, info };
}

async function runScenario(browser, width, height, label) {
  const context = await browser.newContext({ viewport: { width, height }, colorScheme: "light" });
  const page = await context.newPage();
  if (existsSync(FIXTURE_PATH)) await page.addInitScript({ path: FIXTURE_PATH });
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await sleep(1500);
  await openTerminal(page);

  // Type visible content
  await page.locator(".terminal-pane[data-active=true] .xterm-helper-textarea").focus();
  await page.keyboard.type(`echo ${label.replace(/\s+/g, "_")}`, { delay: 10 });
  await page.keyboard.press("Enter");
  await sleep(800);

  // Screenshot before fullscreen
  await page.screenshot({ path: join(OUT_DIR, `fullscreen-${label}-01-normal.png`) });

  // Enter fullscreen
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Fullscreen"], .terminal-pane button[title*="Maximize"]')?.click();
  });
  await sleep(1500);

  // Screenshot after fullscreen
  await page.screenshot({ path: join(OUT_DIR, `fullscreen-${label}-02-fullscreen.png`) });

  const { allPass, info } = await checkFullscreen(page, label);

  // Exit fullscreen
  await page.evaluate(() => {
    document.querySelector('.terminal-pane button[aria-label="Exit fullscreen"], .terminal-pane button[title*="Restore"]')?.click();
  });
  await sleep(800);

  // Verify pane returned to workspace
  const afterExit = await page.evaluate(() => {
    const pane = document.querySelector(".terminal-pane");
    return {
      isFixed: pane?.classList.contains("fixed"),
      isInBody: pane?.parentElement === document.body,
      isRelative: pane?.classList.contains("relative"),
    };
  });
  console.log(`${afterExit.isFixed ? "FAIL" : "PASS"} ${label}: pane not fixed after exit`);
  console.log(`${afterExit.isInBody ? "FAIL" : "PASS"} ${label}: pane returned to workspace after exit`);
  if (afterExit.isFixed || afterExit.isInBody) allPass = false;

  await page.screenshot({ path: join(OUT_DIR, `fullscreen-${label}-03-exited.png`) });
  await page.close();
  await context.close();
  return allPass;
}

async function main() {
  const browser = await pw.chromium.launch({ headless: true, ...(chromiumPath ? { executablePath: chromiumPath } : {}) });

  const scenarios = [
    [1440, 900, "standard"],
    [2882, 816, "ultrawide"],
    [1440, 408, "short"],
    [800, 600, "small"],
    [1920, 1080, "fhd"],
  ];

  let allPass = true;
  for (const [w, h, label] of scenarios) {
    console.log(`\n=== Scenario: ${label} (${w}x${h}) ===`);
    const pass = await runScenario(browser, w, h, label);
    if (!pass) allPass = false;
  }

  // Split + fullscreen scenario
  console.log("\n=== Scenario: split-fullscreen (1440x900) ===");
  {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light" });
    const page = await context.newPage();
    if (existsSync(FIXTURE_PATH)) await page.addInitScript({ path: FIXTURE_PATH });
    await page.goto(URL, { waitUntil: "domcontentloaded" });
    await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
    await sleep(1500);
    await openTerminal(page);

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
    await page.screenshot({ path: join(OUT_DIR, "fullscreen-split-01-two-panes.png") });

    // Fullscreen the first pane
    await page.evaluate(() => {
      const panes = document.querySelectorAll(".terminal-pane");
      panes[0]?.querySelector('button[aria-label="Fullscreen"], button[title*="Maximize"]')?.click();
    });
    await sleep(1500);
    await page.screenshot({ path: join(OUT_DIR, "fullscreen-split-02-first-fullscreen.png") });
    const r1 = await checkFullscreen(page, "split-first");
    if (!r1.allPass) allPass = false;

    // Exit fullscreen
    await page.evaluate(() => {
      document.querySelector('.terminal-pane button[aria-label="Exit fullscreen"], .terminal-pane button[title*="Restore"]')?.click();
    });
    await sleep(1000);
    // Verify both panes are back in the workspace (not teleported)
    const afterExit = await page.evaluate(() => {
      const panes = document.querySelectorAll(".terminal-pane");
      return Array.from(panes).map(p => ({
        fixed: p.classList.contains("fixed"),
        inBody: p.parentElement === document.body,
      }));
    });
    console.log(`${afterExit.every(p => !p.fixed) ? "PASS" : "FAIL"} split: no pane fixed after exit`);
    console.log(`${afterExit.every(p => !p.inBody) ? "PASS" : "FAIL"} split: all panes returned to workspace`);
    if (!afterExit.every(p => !p.fixed && !p.inBody)) allPass = false;

    await page.close();
    await context.close();
  }

  console.log(`\n${allPass ? "ALL PASS" : "SOME FAILED"}`);
  await browser.close();
  process.exit(allPass ? 0 : 1);
}

main().catch(e => { console.error(String(e)); process.exit(1); });
