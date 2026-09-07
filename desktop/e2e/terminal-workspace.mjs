import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const url = process.env.UI_URL ?? "http://127.0.0.1:1420";
const output = process.env.UI_RESULTS ?? "ui-test-results";
await mkdir(output, { recursive: true });
const record = process.env.UI_RECORD !== "0";
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_EXECUTABLE_PATH, slowMo: Number(process.env.UI_SLOW_MO ?? 0) });
const contexts = new Map();
const recordings = [];
async function finish(context) {
  const { tab, name } = contexts.get(context);
  await context.tracing.stop({ path: `${output}/${name}-trace.zip` });
  await context.close();
  if (record) { await tab.video().saveAs(`${output}/${name}.webm`); recordings.push(`${name}.webm`); }
  contexts.delete(context);
}
const checks = [];
const errors = [];
let page;
const fixture = fileURLToPath(new URL("./tauri-fixture.js", import.meta.url));
const check = (name, actual, expected = true) => { assert.deepEqual(actual, expected, name); checks.push({ name, passed: true }); };
const screenshot = async name => {
  if (record) await page.waitForTimeout(750);
  await page.screenshot({ path: `${output}/${name}.png`, fullPage: false });
};
const atlasLabel = () => page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first();
async function fresh(target = url, baseline = false, name = "01-workspace-scenarios") {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: "light",
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: { width: 1440, height: 900 } } } : {}) });
  await context.tracing.start({ screenshots: true, snapshots: true });
  page = await context.newPage();
  contexts.set(context, { tab: page, name });
  page.on("pageerror", error => { if (!baseline) errors.push(String(error)); });
  page.on("console", message => {
    if (!baseline && ["error", "warning"].includes(message.type())) errors.push(message.text());
  });
  await page.addInitScript({ path: fixture });
  await page.goto(target);
  await atlasLabel().waitFor();
  return context;
}
const state = () => page.evaluate(() => ({ connects: window.__terminalTest.connects, closes: window.__terminalTest.closes, live: window.__terminalTest.live }));
const pane = host => page.locator(`[data-host-id="${host}"]`);
const showAtlas = () => page.locator('[title^="Show Atlas Production terminal"]').click();
async function connected(host) {
  await page.waitForFunction(host => document.querySelector(`[data-host-id="${host}"]`)?.getAttribute("data-connected") === "true", host);
  return pane(host).getAttribute("data-session-id");
}
async function command(host, text) {
  await pane(host).locator(".xterm-helper-textarea").focus();
  await page.keyboard.type(text, { delay: record ? 18 : 0 });
  await page.keyboard.press("Enter");
  await page.waitForFunction(({ host, text }) => document.querySelector(`[data-host-id="${host}"] .xterm-rows`)?.textContent.includes(text), { host, text });
}
async function navigate(name) {
  await page.locator("aside nav").getByRole("button", { name, exact: true }).click();
}
async function newLocalTab() {
  await page.getByRole("button", { name: "New session", exact: true }).click();
  await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab");
  await page.getByRole("button", { name: "Open local shell", exact: true }).click();
}
async function paneAction(host, action) {
  await pane(host).getByRole("button", { name: /^Actions for/ }).click();
  await page.getByRole("menuitem", { name: action, exact: true }).click();
}
try {
  if (process.env.BASELINE_URL) {
    const context = await fresh(process.env.BASELINE_URL, true, "00-historical-bugs");
    await atlasLabel().dblclick();
    await page.waitForFunction(() => window.__terminalTest.connects.length === 1);
    await navigate("Files");
    await page.getByText("Connect to SFTP", { exact: true }).waitFor();
    await atlasLabel().click();
    const stuckInFiles = await page.getByText("Connect to SFTP", { exact: true }).isVisible();
    await screenshot("00-before-files-navigation");
    await page.locator("aside nav").getByRole("button", { name: /^Terminal/ }).click();
    await page.getByRole("button", { name: "Split horizontally", exact: true }).first().click();
    await page.waitForFunction(() => window.__terminalTest.connects.length >= 3);
    const baseline = { stuckInFiles, ...await state() };
    await screenshot("00-before-split-reconnect");
    await writeFile(`${output}/baseline.json`, JSON.stringify(baseline, null, 2));
    check("Main reproduces Files navigation bug", stuckInFiles);
    check("Main reconnects the original host on split", baseline.connects.filter(c => c.host === "atlas").length > 1);
    await finish(context);
  }
  const context = await fresh();
  check("Page identity", /OpenTermius/i.test(await page.title()));
  check("Expected URL", page.url().startsWith(url));
  check("Meaningful initial content", await atlasLabel().isVisible());
  await atlasLabel().dblclick();
  const atlas = await connected("atlas");
  await page.evaluate(() => { window.__atlasTerminal = document.querySelector('[data-host-id="atlas"] .xterm'); });
  await command("atlas", "echo KEEP_ATLAS");
  await navigate("Files");
  check("Files hides terminal content", await page.getByTestId("terminal-workspace").isVisible(), false);
  await page.getByRole("combobox").selectOption("atlas");
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await page.getByText("README.md", { exact: true }).waitFor();
  await screenshot("01-files-with-terminal-tabs");
  await showAtlas();
  check("Host tab returns from Files", await page.getByTestId("terminal-workspace").isVisible());
  check("Original session survives Files", await connected("atlas"), atlas);
  check("Original output survives Files", (await pane("atlas").innerText()).includes("KEEP_ATLAS"));
  check("No reconnect while navigating", (await state()).connects.length, 1);
  check("No session close while navigating", (await state()).closes, []);
  await screenshot("02-return-to-existing-terminal");
  await navigate("Files");
  check("SFTP listing survives navigation", await page.getByText("README.md", { exact: true }).isVisible());
  await showAtlas();
  await paneAction("atlas", "Split right…");
  check("Pane split targets the clicked host", await page.getByRole("combobox", { name: "Open session in" }).inputValue(), "horizontal");
  check("Session picker autofocus", await page.getByRole("textbox", { name: "Search sessions" }).evaluate(el => el === document.activeElement));
  await screenshot("08-unified-session-picker");
  await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).click();
  const orion = await connected("orion");
  await command("orion", "echo KEEP_ORION");
  check("Two hosts share a tab", await page.locator("[data-tab-id]").count(), 1);
  check("Only the new host connects on split", (await state()).connects.length, 2);
  check("One session navigation strip", await page.getByTestId("session-strip").count(), 1);
  check("Redundant workspace toolbar removed", await page.getByText("Terminal workspace", { exact: true }).count(), 0);
  check("One visible creation entry point", await page.getByRole("button", { name: "New session", exact: true }).count(), 1);
  check("Vault is not duplicated in navigation", await page.locator("aside").getByRole("button", { name: /^Vault/ }).count(), 1);
  const terminalTop = (await pane("atlas").locator(".xterm").boundingBox()).y;
  check("Terminal content starts within 86 pixels", terminalTop <= 86);
  await pane("atlas").getByRole("button", { name: "Actions for Atlas Production", exact: true }).focus();
  await page.keyboard.press("ArrowDown");
  check("Menu opens from the keyboard", await page.getByRole("menu", { name: "Actions for Atlas Production" }).isVisible());
  check("Menu initially focuses first action", await page.getByRole("menuitem", { name: "Split right…", exact: true }).evaluate(el => el === document.activeElement));
  await page.keyboard.press("ArrowDown");
  check("Menu arrow navigation", await page.getByRole("menuitem", { name: "Split below…", exact: true }).evaluate(el => el === document.activeElement));
  await screenshot("09-contextual-pane-actions");
  await page.keyboard.press("Escape");
  check("Escape returns focus to action trigger", await pane("atlas").getByRole("button", { name: "Actions for Atlas Production", exact: true }).evaluate(el => el === document.activeElement));
  check("Split preserves original xterm DOM", await page.evaluate(() => document.querySelector('[data-host-id="atlas"] .xterm') === window.__atlasTerminal));
  await screenshot("03-two-hosts-one-tab");
  const divider = page.getByRole("separator", { name: "Resize terminal panes" });
  await divider.focus();
  await page.keyboard.press("ArrowRight");
  check("Keyboard resizing", await divider.getAttribute("aria-valuenow"), "55");
  await divider.dblclick();
  check("Equal split reset", await divider.getAttribute("aria-valuenow"), "50");
  const dividerBounds = await divider.boundingBox();
  await page.mouse.move(dividerBounds.x + 3, dividerBounds.y + 150);
  await page.mouse.down();
  await page.mouse.move(dividerBounds.x + 70, dividerBounds.y + 150, { steps: 4 });
  const firstRatio = Number(await divider.getAttribute("aria-valuenow"));
  await page.mouse.move(dividerBounds.x + 140, dividerBounds.y + 150, { steps: 4 });
  const secondRatio = Number(await divider.getAttribute("aria-valuenow"));
  await page.mouse.up();
  check("Pointer resize continues across multiple updates", secondRatio > firstRatio && firstRatio > 50);
  await divider.dblclick();
  await pane("orion").click({ position: { x: 50, y: 16 } });
  await newLocalTab();
  await page.waitForFunction(() => window.__terminalTest.connects.length === 3);
  const local = (await state()).connects.find(c => c.host === "local").id;
  await showAtlas();
  check("Last focused split is restored", await pane("orion").getAttribute("data-active"), "true");
  await page.waitForFunction(() => document.querySelector('[data-host-id="orion"]')?.contains(document.activeElement));
  check("Only focused split receives focus", true);
  await page.getByRole("button", { name: "Close tab Local", exact: true }).click();
  check("Closing another tab leaves hosts alive", (await state()).closes, [local]);
  await page.locator('[aria-label="Drag pane Atlas Production"]').dragTo(pane("orion"));
  check("Swap retains Atlas identity", await connected("atlas"), atlas);
  check("Swap retains Orion identity", await connected("orion"), orion);
  check("Swap causes no connections", (await state()).connects.length, 3);
  await command("atlas", "echo ATLAS_AFTER_SWAP");
  check("Input still targets Atlas after swap", await page.evaluate(() => window.__terminalTest.writes.at(-1).id), atlas);
  const bounds = await pane("orion").boundingBox();
  await page.locator('[aria-label="Drag pane Atlas Production"]').dragTo(pane("orion"), { targetPosition: { x: Math.floor(bounds.width / 2), y: bounds.height - 12 } });
  check("Edge move does not close sessions", (await state()).closes, [local]);
  await screenshot("04-rearranged-with-history");
  await newLocalTab();
  await page.waitForFunction(() => window.__terminalTest.connects.length === 4);
  await showAtlas();
  await paneAction("atlas", "Move to Local");
  check("Cross-tab move keeps original session", await connected("atlas"), atlas);
  check("Cross-tab move keeps original xterm DOM", await page.evaluate(() => document.querySelector('[data-host-id="atlas"] .xterm') === window.__atlasTerminal));
  check("Cross-tab move does not reconnect", (await state()).connects.length, 4);
  check("Cross-tab move does not close host", (await state()).closes, [local]);
  await command("atlas", "echo ATLAS_AFTER_TAB_MOVE");
  check("Input targets the original session after moving tabs", await page.evaluate(() => window.__terminalTest.writes.at(-1).id), atlas);
  await screenshot("05-session-moved-to-another-tab");
  await pane("atlas").getByRole("button", { name: "Search in terminal", exact: true }).click();
  await page.getByRole("textbox", { name: "Search terminal output" }).fill("KEEP_ATLAS");
  check("Search remains available", await page.getByRole("textbox", { name: "Search terminal output" }).isVisible());
  await page.getByRole("button", { name: "Close search", exact: true }).click();
  await pane("atlas").getByRole("button", { name: "Fullscreen", exact: true }).click();
  const full = await pane("atlas").boundingBox();
  check("Fullscreen fills the window", full.width >= 1439 && full.height >= 899);
  await pane("atlas").locator(".xterm-helper-textarea").focus();
  const writesBeforeEscape = await page.evaluate(() => window.__terminalTest.writes.length);
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector('[data-host-id="atlas"]')?.classList.contains("fixed"));
  check("Escape exits fullscreen with terminal focus", await pane("atlas").getByRole("button", { name: "Fullscreen", exact: true }).isVisible());
  check("Fullscreen Escape is not sent to the remote shell", await page.evaluate(() => window.__terminalTest.writes.length), writesBeforeEscape);
  await page.setViewportSize({ width: 900, height: 700 });
  await screenshot("06-compact-window");
  check("Compact window has no page overflow", await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
  check("Sidebar collapses to an icon rail", (await page.locator("aside").boundingBox()).width, 60);
  await screenshot("10-collapsed-navigation");
  await page.getByRole("button", { name: "Expand sidebar", exact: true }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  check("Narrow window has no page overflow", await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await page.getByRole("button", { name: "Open navigation", exact: true }).click();
  await navigate("Files");
  await showAtlas();
  await page.locator('[title^="Show Local terminal"]').click();
  await pane("atlas").getByRole("button", { name: "Actions for Atlas Production", exact: true }).click();
  const menuBounds = await page.getByRole("menu", { name: "Actions for Atlas Production" }).boundingBox();
  check("Pane menu stays within narrow viewport", menuBounds.x >= 0 && menuBounds.x + menuBounds.width <= 390);
  await screenshot("11-narrow-window-menu");
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await screenshot("12-dark-theme");
  await page.evaluate(() => document.documentElement.classList.remove("dark"));
  check("No Vite error overlay", await page.locator("vite-error-overlay").count(), 0);
  await finish(context);
  const failureContext = await fresh(url, false, "02-reconnect-scenario");
  await page.evaluate(() => { window.__terminalTest.failNext = true; });
  await atlasLabel().dblclick();
  await page.getByRole("alert").waitFor();
  check("Connection failure is actionable", (await page.getByRole("alert").innerText()).includes("connection refused"));
  await screenshot("07-connection-error");
  await page.getByRole("alert").getByRole("button", { name: "Reconnect", exact: true }).click();
  await connected("atlas");
  check("Explicit reconnect succeeds", (await state()).connects.length, 2);
  await finish(failureContext);
  const raceContext = await fresh(url, false, "03-pending-connection-close");
  await page.evaluate(() => { window.__terminalTest.holdNext = true; });
  await atlasLabel().dblclick();
  await page.waitForFunction(() => window.__terminalTest.pending.length === 1);
  const pendingId = (await state()).connects[0].id;
  check("Pending connection displays Connecting", (await pane("atlas").innerText()).includes("Connecting"));
  await pane("atlas").getByRole("button", { name: "Actions for Atlas Production", exact: true }).click();
  check("Reconnect is disabled while connecting", await page.getByRole("menuitem", { name: "Reconnect", exact: true }).isDisabled());
  await screenshot("13-pending-connection");
  await page.getByRole("menuitem", { name: "Close session", exact: true }).click();
  await page.evaluate(() => window.__terminalTest.release());
  await page.waitForFunction(id => !window.__terminalTest.live[id] && window.__terminalTest.closes.filter(c => c === id).length >= 2, pendingId);
  check("Late connection cannot resurrect closed pane", await page.locator("[data-pane-id]").count(), 0);
  await finish(raceContext);
  check("Console and runtime health", errors, []);
  await writeFile(`${output}/results.json`, JSON.stringify({ passed: true, checks, errors, transport: "Mock Tauri IPC; real Vue and xterm UI; no live SSH/SFTP server", browser: "Chromium", viewports: ["1440x900", "900x700", "390x844"], recordings }, null, 2));
  console.log(JSON.stringify({ passed: true, checks: checks.length }, null, 2));
} catch (error) {
  if (page && !page.isClosed()) await screenshot("failure").catch(() => {});
  await writeFile(`${output}/results.json`, JSON.stringify({ passed: false, checks, errors, failure: String(error), stack: error.stack }, null, 2));
  throw error;
} finally {
  for (const context of contexts.keys()) await finish(context).catch(() => {});
  await browser.close();
}
