// Real xterm/DOM regressions. SSH is the deterministic, test-only Tauri fixture.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const url = process.env.UI_URL ?? "http://127.0.0.1:1420";
const output = process.env.UI_RESULTS ?? "ui-test-results";
const record = process.env.UI_RECORD !== "0";
const fixture = fileURLToPath(new URL("./tauri-fixture.js", import.meta.url));
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true,
  executablePath: process.env.CHROMIUM_EXECUTABLE_PATH,
  slowMo: Number(process.env.UI_SLOW_MO ?? 0) });
const results = [];
const pane = (page, host) => page.locator(`[data-host-id="${host}"]`);
const inspect = page => page.evaluate(() => ({
  fullscreen: document.querySelector(".terminal-pane.fixed")?.getAttribute("data-host-id") ?? null,
  active: document.querySelector('.terminal-pane[data-active="true"]')?.getAttribute("data-host-id") ?? null,
  focused: document.activeElement?.closest(".terminal-pane")?.getAttribute("data-host-id") ?? null,
  focusLabel: document.activeElement?.getAttribute("aria-label") ?? null,
  writes: window.__terminalTest.writes,
  connects: window.__terminalTest.connects,
  closes: window.__terminalTest.closes,
}));
async function connected(page, host) {
  await page.waitForFunction(host => document.querySelector(`[data-host-id="${host}"]`)?.getAttribute("data-connected") === "true", host);
  return pane(page, host).getAttribute("data-session-id");
}
async function action(page, host, label) {
  await pane(page, host).getByRole("button", { name: /^Actions for/ }).click();
  await page.getByRole("menuitem", { name: label, exact: true }).click();
}
async function atlas(page) {
  await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  return connected(page, "atlas");
}
async function split(page) {
  const first = await atlas(page);
  await action(page, "atlas", "Split right…");
  await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).click();
  return { atlas: first, orion: await connected(page, "orion") };
}
async function focusIs(page, host, search = false) {
  await page.waitForFunction(({ host, search }) => {
    const el = document.activeElement;
    return el?.closest(".terminal-pane")?.getAttribute("data-host-id") === host &&
      (search ? el.getAttribute("aria-label") === "Search terminal output" : el.classList.contains("xterm-helper-textarea"));
  }, { host, search });
}
async function typeCommand(page, text) {
  // Intentionally do NOT focus a locator here: that would hide a routing bug.
  await page.keyboard.type(text);
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => window.__terminalTest.writes.some(write => write.text.includes("\r")));
}
async function clearWrites(page) {
  await page.evaluate(() => { window.__terminalTest.writes.length = 0; });
}
async function scenario(name, exercise) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 },
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: { width: 1440, height: 900 } } } : {}) });
  await context.tracing.start({ screenshots: true, snapshots: true });
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  page.on("console", message => { if (["error", "warning"].includes(message.type())) errors.push(message.text()); });
  const result = { name, passed: false, errors };
  try {
    await page.addInitScript({ path: fixture });
    await page.goto(url);
    assert.match(await page.title(), /OpenTermius/i, "Correct page identity");
    assert.ok(page.url().startsWith(url), "Correct app URL");
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().waitFor();
    assert.equal(await page.locator("vite-error-overlay").count(), 0, "No framework overlay");
    await exercise(page);
    assert.deepEqual(errors, [], "No browser errors or warnings");
    result.passed = true;
  } catch (error) {
    result.error = String(error.stack ?? error);
  } finally {
    result.state = await inspect(page).catch(() => null);
    await page.screenshot({ path: `${output}/${name}.png` });
    await context.tracing.stop({ path: `${output}/${name}-trace.zip` });
    const video = page.video();
    await context.close();
    if (record && video) await video.saveAs(`${output}/${name}.webm`);
    results.push(result);
    console.log(`${result.passed ? "PASS" : "FAIL"}: ${name}`);
    if (result.error) console.error(result.error);
  }
}
try {
  for (const [modifier, source, target, arrow] of [
    ["Control", "atlas", "orion", "ArrowRight"],
    ["Control", "orion", "atlas", "ArrowLeft"],
    ["Meta", "atlas", "orion", "ArrowRight"],
  ]) {
    await scenario(`input-fullscreen-${modifier}-${source}`, async page => {
      const sessions = await split(page);
      await page.evaluate(() => { window.__routingNodes = Array.from(document.querySelectorAll(".terminal-pane .xterm")); });
      await pane(page, source).getByRole("button", { name: "Fullscreen", exact: true }).click();
      await focusIs(page, source);
      await clearWrites(page);
      await page.keyboard.press(`${modifier}+${arrow}`);
      await page.waitForFunction(host => document.querySelector('.terminal-pane[data-active="true"]')?.getAttribute("data-host-id") === host, target);
      await typeCommand(page, "echo VISIBLE_HOST_ONLY");
      const state = await inspect(page);
      assert.equal(state.fullscreen, null, "Navigating away restores the split before input");
      assert.equal(state.focused, target, "Focus follows the active pane");
      assert.ok(state.writes.length > 0);
      assert.ok(state.writes.every(write => write.id === sessions[target]), "Only the visible destination receives input");
      assert.equal(state.writes.map(write => write.text).join(""), "echo VISIBLE_HOST_ONLY\r");
      assert.equal(state.connects.length, 2, "Navigation must not reconnect");
      assert.deepEqual(state.closes, [], "Navigation must not close sessions");
      assert.ok(await page.evaluate(() => window.__routingNodes.every(node => node.isConnected)), "xterm owners survive navigation");
    });
  }
  await scenario("input-search-navigation", async page => {
    const sessions = await split(page);
    await pane(page, "atlas").getByRole("button", { name: "Search in terminal", exact: true }).click();
    await focusIs(page, "atlas", true);
    await pane(page, "orion").locator(".xterm-screen").click({ position: { x: 100, y: 40 } });
    await focusIs(page, "orion");
    await clearWrites(page);
    await page.keyboard.press("Control+ArrowLeft");
    await focusIs(page, "atlas", true);
    await page.keyboard.type("SEARCH_ONLY_ATLAS");
    assert.equal(await pane(page, "atlas").getByRole("textbox", { name: "Search terminal output" }).inputValue(), "SEARCH_ONLY_ATLAS");
    assert.deepEqual((await inspect(page)).writes, [], "Search text must not reach any SSH session");
    await page.screenshot({ path: `${output}/input-search-focus-restored.png` });
    await page.keyboard.press("Escape");
    await focusIs(page, "atlas");
    await typeCommand(page, "echo ATLAS_AFTER_SEARCH");
    const state = await inspect(page);
    assert.equal(state.active, "atlas");
    assert.ok(state.writes.every(write => write.id === sessions.atlas));
    assert.equal(state.writes.map(write => write.text).join(""), "echo ATLAS_AFTER_SEARCH\r");
  });
  await scenario("input-search-tab-roundtrip", async page => {
    await split(page);
    await pane(page, "atlas").getByRole("button", { name: "Search in terminal", exact: true }).click();
    await focusIs(page, "atlas", true);
    await page.getByRole("button", { name: "New session", exact: true }).click();
    await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab");
    await page.getByRole("button", { name: "Open local shell", exact: true }).click();
    await page.locator('[title^="Show Atlas Production terminal"]').click();
    await focusIs(page, "atlas", true);
    await clearWrites(page);
    await page.keyboard.type("SEARCH_AFTER_TAB_SWITCH");
    assert.equal(await pane(page, "atlas").getByRole("textbox", { name: "Search terminal output" }).inputValue(), "SEARCH_AFTER_TAB_SWITCH");
    assert.deepEqual((await inspect(page)).writes, []);
  });
  await scenario("input-reconnect-protocol-reset", async page => {
    const old = await atlas(page);
    await focusIs(page, "atlas");
    await page.evaluate(id => {
      window.__routingTerminal = document.querySelector('[data-host-id="atlas"] .xterm');
      window.__terminalTest.output(id, "\x1b[?1000h\x1b[?1006h\x1b[?1h\x1b[?2004h\r\nOLD_MODES_READY\r\n");
    }, old);
    await page.waitForFunction(() => document.querySelector('[data-host-id="atlas"] .xterm-rows')?.textContent.includes("OLD_MODES_READY"));
    await clearWrites(page);
    await pane(page, "atlas").locator(".xterm-screen").click({ position: { x: 130, y: 80 } });
    await page.keyboard.press("ArrowUp");
    const oldWrites = (await inspect(page)).writes;
    assert.ok(oldWrites.some(write => write.text.startsWith("\x1b[<")), "Precondition: the old session enabled mouse reports");
    assert.ok(oldWrites.some(write => write.text === "\x1bOA"), "Precondition: application cursor mode is enabled");
    await page.evaluate(id => window.__terminalTest.disconnect(id), old);
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    const replacement = await connected(page, "atlas");
    assert.notEqual(replacement, old);
    await focusIs(page, "atlas");
    await clearWrites(page);
    await pane(page, "atlas").locator(".xterm-screen").click({ position: { x: 130, y: 80 } });
    await page.keyboard.press("ArrowUp");
    await pane(page, "atlas").locator(".xterm-helper-textarea").evaluate(el => {
      const clipboardData = new DataTransfer();
      clipboardData.setData("text/plain", "PASTE_AFTER_RECONNECT");
      el.dispatchEvent(new ClipboardEvent("paste", { clipboardData, bubbles: true, cancelable: true }));
    });
    await page.waitForFunction(() => window.__terminalTest.writes.some(write => write.text.includes("PASTE_AFTER_RECONNECT")));
    const state = await inspect(page);
    assert.ok(state.writes.every(write => write.id === replacement));
    assert.deepEqual(state.writes.map(write => write.text), ["\x1b[A", "PASTE_AFTER_RECONNECT"], "Fresh session has normal cursor keys, no mouse reports, and no inherited bracketed paste");
    assert.equal(state.connects.length, 2, "Only an explicit reconnect creates the replacement");
    assert.deepEqual(state.closes, [old]);
    assert.ok(await page.evaluate(() => window.__routingTerminal === document.querySelector('[data-host-id="atlas"] .xterm')), "Reset protocol state without remounting xterm");
  });
} finally {
  await browser.close();
  await writeFile(`${output}/input-routing-results.json`, JSON.stringify({ transport: "mocked Tauri; real Vue/xterm", results }, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
