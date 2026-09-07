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
async function scenario(name, exercise, mode = "normal") {
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
    await page.addInitScript(reviewFixture, mode);
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

function reviewFixture(mode) {
  const original = window.__TAURI_INTERNALS__.invoke;
  const review = window.__reviewTest = { holdNext: mode === "protocol", pendingId: null, release: null };
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "list_hosts" && mode === "password") {
      const hosts = await original(command, args);
      hosts[1].auth = { password: { credential_key: "fixture-only" } };
      return hosts;
    }
    // A server consumes replies, rather than echoing them back like interactive input.
    if (command === "session_write") {
      const text = new TextDecoder().decode(new Uint8Array(args.data));
      if (/^\x1b\[\d+;\d+R$/.test(text)) {
        window.__terminalTest.writes.push({ id: args.sessionId, text });
        return;
      }
    }
    const result = await original(command, args);
    if (["connect_ssh", "create_local_terminal"].includes(command) && review.holdNext) {
      review.holdNext = false;
      review.pendingId = args.sessionId;
      // The real backend publishes output before the connect command returns.
      // Split the query between IPC chunks to cover UTF-8/parser stream ordering.
      window.__terminalTest.output(args.sessionId, "\x1b[3;3H\x1b[6");
      window.__terminalTest.output(args.sessionId, "n");
      await new Promise(resolve => { review.release = resolve; });
    }
    return result;
  };
}
async function remember(page) {
  await page.evaluate(() => {
    window.__movedNodes = Object.fromEntries(Array.from(document.querySelectorAll('.terminal-pane'))
      .map(el => [el.dataset.hostId, el.querySelector('.xterm')]));
    window.__terminalTest.writes.length = 0;
  });
}
async function preserved(page, count = 2) {
  assert.ok(await page.evaluate(() => Object.entries(window.__movedNodes).every(([host, node]) =>
    document.querySelector(`[data-host-id="${host}"] .xterm`) === node)), "No xterm remount on move");
  const state = await inspect(page);
  assert.equal(state.connects.length, count, "No reconnect on move");
  assert.deepEqual(state.closes, [], "No session closed on move");
}
async function drag(page, position) {
  const target = pane(page, "atlas");
  const box = await target.boundingBox();
  const point = position === "left" ? { x: 5, y: box.height / 2 } : position === "bottom"
    ? { x: box.width / 2, y: box.height - 5 } : { x: box.width / 2, y: box.height / 2 };
  await pane(page, "orion").getByLabel("Drag pane Orion Staging", { exact: true }).dragTo(target, { targetPosition: point });
}
try {
  for (const type of ["ssh", "local"]) {
    await scenario(`readiness-early-query-${type}`, async page => {
      await page.getByRole("button", { name: "New session", exact: true }).click();
      await page.getByRole("button", { name: type === "ssh" ? "Connect Atlas Production" : "Open local shell", exact: true }).click();
      await page.waitForFunction(() => window.__reviewTest.release !== null);
      const id = await page.evaluate(() => window.__reviewTest.pendingId);
      await page.locator('.terminal-pane .xterm-screen').click({ position: { x: 100, y: 40 } });
      await page.keyboard.type("NEVER_REPLAY_PENDING_INPUT");
      await page.waitForTimeout(100); // Give the original parser time to lose the reply.
      assert.deepEqual((await inspect(page)).writes, [], "No writes before readiness");
      await page.evaluate(() => window.__reviewTest.release());
      await page.waitForFunction(() => window.__terminalTest.writes.length === 1);
      assert.deepEqual((await inspect(page)).writes, [{ id, text: "\x1b[3;3R" }], "Exact reply, no buffered user input");
    }, "protocol");
  }
  await scenario("readiness-reconnect-query", async page => {
    const old = await atlas(page);
    await remember(page);
    await page.evaluate(id => {
      window.__reviewTest.holdNext = true;
      window.__terminalTest.disconnect(id);
    }, old);
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    await page.waitForFunction(() => window.__reviewTest.release !== null);
    const id = await page.evaluate(() => window.__reviewTest.pendingId);
    await page.evaluate(old => window.__terminalTest.output(old, "\x1b[9;9H\x1b[6n"), old);
    await page.waitForTimeout(100);
    assert.deepEqual((await inspect(page)).writes, []);
    await page.evaluate(() => window.__reviewTest.release());
    await page.waitForFunction(() => window.__terminalTest.writes.length === 1);
    assert.deepEqual((await inspect(page)).writes, [{ id, text: "\x1b[3;3R" }]);
    assert.notEqual(id, old);
    assert.ok(await page.evaluate(() => window.__movedNodes.atlas === document.querySelector('[data-host-id="atlas"] .xterm')));
  });
  for (const position of ["left", "center", "bottom"]) {
    await scenario(`drag-focus-${position}`, async page => {
      const sessions = await split(page);
      await pane(page, "orion").locator('.xterm-screen').click({ position: { x: 100, y: 40 } });
      await remember(page);
      await drag(page, position);
      await focusIs(page, "orion");
      await typeCommand(page, "echo AFTER_DRAG");
      const state = await inspect(page);
      assert.equal(state.active, "orion");
      assert.equal(state.writes.map(write => write.text).join(""), "echo AFTER_DRAG\r");
      assert.ok(state.writes.every(write => write.id === sessions.orion));
      await preserved(page);
    });
  }
  await scenario("drag-focus-cross-tab", async page => {
    const sessions = await split(page);
    await page.getByRole("button", { name: "New session", exact: true }).click();
    await page.getByRole("button", { name: "Open local shell", exact: true }).click();
    await page.waitForFunction(() => document.querySelector('.terminal-pane:not([data-host-id])')?.dataset.connected === "true");
    const buttons = page.locator('[data-tab-id]');
    await buttons.first().click();
    await focusIs(page, "orion");
    await remember(page);
    await pane(page, "orion").getByLabel("Drag pane Orion Staging", { exact: true }).dragTo(buttons.last());
    await focusIs(page, "orion");
    await typeCommand(page, "echo CROSS_TAB_DRAG");
    const state = await inspect(page);
    assert.ok(state.writes.every(write => write.id === sessions.orion));
    assert.equal(state.writes.map(write => write.text).join(""), "echo CROSS_TAB_DRAG\r");
    // Local panes have no data-host-id attribute; compare the two SSH owners explicitly.
    assert.ok(await page.evaluate(() => ["atlas", "orion"].every(host =>
      window.__movedNodes[host] === document.querySelector(`[data-host-id="${host}"] .xterm`))));
    assert.equal(state.connects.length, 3); assert.deepEqual(state.closes, []);
  });
  await scenario("drag-focus-search", async page => {
    await split(page);
    await pane(page, "orion").getByRole("button", { name: "Search in terminal", exact: true }).click();
    await focusIs(page, "orion", true);
    await remember(page);
    await drag(page, "left");
    await focusIs(page, "orion", true);
    await page.keyboard.type("SEARCH_AFTER_DRAG");
    assert.equal(await pane(page, "orion").getByRole("textbox", { name: "Search terminal output" }).inputValue(), "SEARCH_AFTER_DRAG");
    assert.deepEqual((await inspect(page)).writes, []);
    await preserved(page);
  });
  await scenario("drag-focus-password", async page => {
    await atlas(page);
    await action(page, "atlas", "Split right…");
    await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).click();
    const input = pane(page, "orion").locator('input[aria-label="SSH password"]');
    await input.waitFor();
    await remember(page);
    await drag(page, "left");
    await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === "SSH password" &&
      document.activeElement?.closest('.terminal-pane')?.getAttribute('data-host-id') === "orion");
    await page.keyboard.type("fixture-only not submitted");
    assert.equal(await input.inputValue(), "fixture-only not submitted");
    assert.deepEqual((await inspect(page)).writes, []);
    await preserved(page, 1); // Password entry must not dispatch a connection.
    await pane(page, "orion").getByRole("button", { name: "Cancel connection", exact: true }).click();
  }, "password");
} finally {
  await browser.close();
  await writeFile(`${output}/output-drag-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
