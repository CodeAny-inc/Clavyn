// Real keyboard/xterm regressions; transport is the deterministic Tauri fixture.
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
const selector = host => host === "local" ? '.terminal-pane:not([data-host-id])' : `[data-host-id="${host}"]`;
const pane = (page, host) => page.locator(selector(host));
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
  await page.waitForFunction(css => document.querySelector(css)?.getAttribute("data-connected") === "true", selector(host));
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
async function focusIs(page, host) {
  await page.waitForFunction(host => {
    const el = document.activeElement;
    return el?.closest(".terminal-pane")?.getAttribute("data-host-id") === host && el.classList.contains("xterm-helper-textarea");
  }, host);
}
async function typeCommand(page, text) {
  // Never focus a locator here: that would hide the routing regression.
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
    assert.match(await page.title(), /Clavyn/i, "Correct page identity");
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
  const review = window.__reviewTest = { holdNext: mode === "held", pendingId: null, release: null };
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "session_write") {
      const text = new TextDecoder().decode(new Uint8Array(args.data));
      if (/^\x1b\[\d+;\d+R$/.test(text)) {
        window.__terminalTest.writes.push({ id: args.sessionId, text });
        return; // A server consumes protocol replies instead of echoing them.
      }
    }
    const result = await original(command, args);
    if (["connect_ssh", "create_local_terminal"].includes(command) && review.holdNext) {
      review.holdNext = false;
      review.pendingId = args.sessionId;
      // The backend session is live, but its IPC response has not yet arrived.
      await new Promise(resolve => { review.release = resolve; });
    }
    return result;
  };
}
async function remember(page) {
  await page.evaluate(() => {
    window.__ownerNodes = Array.from(document.querySelectorAll(".terminal-pane .xterm"));
    window.__terminalTest.writes.length = 0;
  });
}
async function preserved(page, connections = 2, closes = 0) {
  assert.ok(await page.evaluate(() => window.__ownerNodes.every(node => node.isConnected)), "Original xterm nodes survive");
  const state = await inspect(page);
  assert.equal(state.connects.length, connections, "Only expected connection attempts");
  assert.equal(state.closes.length, closes, "Only explicit reconnect closes its old session");
}
async function menuEscape(page, host) {
  await pane(page, host).getByRole("button", { name: /^Actions for/ }).click();
  await page.getByRole("menu").waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("menu").waitFor({ state: "detached" });
}
async function transcript(page, host) {
  return pane(page, host).locator(".xterm-rows").textContent();
}
async function visibleText(page, host, text) {
  await page.waitForFunction(({ css, text }) => document.querySelector(`${css} .xterm-rows`)?.textContent.includes(text), { css: selector(host), text });
}
try {
  await scenario("keyboard-owner-split", async page => {
    const sessions = await split(page);
    await remember(page);
    await menuEscape(page, "orion");
    for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+Tab");
    await focusIs(page, "atlas");
    await typeCommand(page, "KEYBOARD_OWNER_PROBE");
    const state = await inspect(page);
    assert.equal(state.active, "atlas", "Keyboard-focused terminal must also be the active owner");
    assert.equal(state.fullscreen, null);
    assert.ok(state.writes.every(write => write.id === sessions.atlas));
    assert.equal(state.writes.map(write => write.text).join(""), "KEYBOARD_OWNER_PROBE\r");
    await preserved(page);
  });
  await scenario("keyboard-owner-fullscreen", async page => {
    const sessions = await split(page);
    await remember(page);
    await pane(page, "orion").getByRole("button", { name: "Fullscreen", exact: true }).click();
    await focusIs(page, "orion");
    await menuEscape(page, "orion");
    // Exercise the reported three keystrokes and continue around the tab order.
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press("Shift+Tab");
      const state = await inspect(page);
      assert.notEqual(state.focused, "atlas", "Obscured Atlas must not be a keyboard destination");
      assert.equal(state.active, "orion");
      assert.equal(state.fullscreen, "orion");
    }
    assert.equal(await pane(page, "atlas").evaluate(el => el.inert), true);
    // Enter the visible xterm through normal forward tabbing, not locator.focus().
    await menuEscape(page, "orion");
    await page.keyboard.press("Tab");
    await focusIs(page, "orion");
    await typeCommand(page, "FULLSCREEN_OWNER_PROBE");
    const state = await inspect(page);
    assert.ok(state.writes.every(write => write.id === sessions.orion));
    assert.equal(state.writes.map(write => write.text).join(""), "FULLSCREEN_OWNER_PROBE\r");
    // Explicit pane navigation must still exit fullscreen and make Atlas usable.
    await clearWrites(page);
    await page.keyboard.press("Control+ArrowLeft");
    await focusIs(page, "atlas");
    assert.equal((await inspect(page)).fullscreen, null);
    assert.equal(await pane(page, "atlas").evaluate(el => el.inert), false);
    await typeCommand(page, "AFTER_FULLSCREEN_EXIT");
    assert.ok((await inspect(page)).writes.every(write => write.id === sessions.atlas));
    await preserved(page);
  });
  await scenario("keyboard-inert-background-protocol", async page => {
    const sessions = await split(page);
    await remember(page);
    await pane(page, "orion").getByRole("button", { name: "Fullscreen", exact: true }).click();
    await focusIs(page, "orion");
    assert.equal(await pane(page, "atlas").evaluate(el => el.inert), true);
    await page.evaluate(id => window.__terminalTest.output(id, "\x1b[3;3H\x1b[6n"), sessions.atlas);
    await page.waitForFunction(() => window.__terminalTest.writes.length === 1);
    const state = await inspect(page);
    assert.deepEqual(state.writes, [{ id: sessions.atlas, text: "\x1b[3;3R" }]);
    assert.equal(state.active, "orion");
    assert.equal(state.focused, "orion");
    assert.equal(state.fullscreen, "orion");
    await preserved(page);
  });
  for (const type of ["ssh", "local"]) {
    for (const early of [true, false]) {
      await scenario(`close-diagnostics-${type}-${early ? "early" : "ready"}`, async page => {
        await page.getByRole("button", { name: "New session", exact: true }).click();
        await page.getByRole("button", { name: type === "ssh" ? "Connect Atlas Production" : "Open local shell", exact: true }).click();
        const host = type === "ssh" ? "atlas" : "local";
        await page.waitForFunction(() => window.__reviewTest.release !== null);
        const id = await page.evaluate(() => window.__reviewTest.pendingId);
        await remember(page);
        if (!early) {
          await page.evaluate(() => window.__reviewTest.release());
          await connected(page, host);
        }
        await page.evaluate(({ id, early }) => {
          window.__terminalTest.output(id, "\r\nFIRST_DIAGNOSTIC\r\n");
          // Query split across IPC chunks: no reply is allowed after EOF.
          if (early) {
            window.__terminalTest.output(id, "\x1b[6");
            window.__terminalTest.output(id, "n");
          }
          window.__terminalTest.output(id, "STARTUP_DIAGNOSTIC_MUST_SURVIVE\r\n");
          window.__terminalTest.disconnect(id);
          if (early) window.__reviewTest.release();
        }, { id, early });
        await visibleText(page, host, "Session closed: Fixture disconnect");
        const text = await transcript(page, host);
        assert.ok(text.includes("STARTUP_DIAGNOSTIC_MUST_SURVIVE"), "Received startup diagnostic survives EOF");
        assert.ok(text.indexOf("FIRST_DIAGNOSTIC") < text.indexOf("STARTUP_DIAGNOSTIC_MUST_SURVIVE"));
        assert.ok(text.indexOf("STARTUP_DIAGNOSTIC_MUST_SURVIVE") < text.indexOf("Session closed:"), "Diagnostics precede close status");
        assert.equal(await pane(page, host).getAttribute("data-connected"), "false");
        await page.evaluate(id => {
          window.__terminalTest.output(id, "LATE_OUTPUT_MUST_NOT_APPEAR\x1b[6n");
          window.__terminalTest.disconnect(id); // Duplicate closure is ignored.
        }, id);
        await page.waitForTimeout(100); // Allow asynchronous xterm parsing/replies.
        assert.deepEqual((await inspect(page)).writes, [], "No protocol/user writes to the closed session");
        assert.doesNotMatch(await transcript(page, host), /LATE_OUTPUT_MUST_NOT_APPEAR/);
        assert.equal((await transcript(page, host)).split("Session closed:").length - 1, 1);
        await preserved(page, 1);
        await page.screenshot({ path: `${output}/close-preserved-${type}-${early ? "early" : "ready"}.png` });
        // Replacement uses the same emulator, drained/reset, with a new transport.
        await page.waitForFunction(css => !document.querySelector(`${css} [role="alert"] button`)?.disabled, selector(host));
        await pane(page, host).getByRole("button", { name: "Reconnect", exact: true }).click();
        const replacement = await connected(page, host);
        assert.notEqual(replacement, id);
        await page.evaluate(({ old, replacement }) => {
          window.__terminalTest.output(old, "LATE_OLD_SESSION\x1b[6n");
          window.__terminalTest.disconnect(old);
          window.__terminalTest.output(replacement, "\x1b[3;3H\x1b[6n");
        }, { old: id, replacement });
        await page.waitForFunction(() => window.__terminalTest.writes.length === 1);
        assert.deepEqual((await inspect(page)).writes, [{ id: replacement, text: "\x1b[3;3R" }]);
        assert.doesNotMatch(await transcript(page, host), /FIRST_DIAGNOSTIC|STARTUP_DIAGNOSTIC_MUST_SURVIVE|LATE_OLD_SESSION/);
        assert.equal(await pane(page, host).getAttribute("data-connected"), "true");
        await preserved(page, 2, 1);
      }, "held");
    }
  }
} finally {
  await writeFile(`${output}/keyboard-close-results.json`, JSON.stringify(results, null, 2));
  await browser.close();
}
if (results.some(result => !result.passed)) process.exitCode = 1;
