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
  focusTag: document.activeElement?.tagName,
  paletteValue: document.querySelector('input[placeholder="Search commands..."]')?.value,
  attempts: window.__reviewTest?.attempts,
  writes: window.__terminalTest.writes,
  connects: window.__terminalTest.connects,
  closes: window.__terminalTest.closes,
}));
async function connected(page, host) {
  await page.waitForFunction(host => document.querySelector(`[data-host-id="${host}"]`)?.getAttribute("data-connected") === "true", host);
  return pane(page, host).getAttribute("data-session-id");
}
async function atlas(page) {
  await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  return connected(page, "atlas");
}
async function clearWrites(page) {
  await page.evaluate(() => { window.__terminalTest.writes.length = 0; });
}
async function scenario(name, exercise, mode = "normal") {
  if (process.env.UI_SCENARIO_FILTER && !new RegExp(process.env.UI_SCENARIO_FILTER).test(name)) return;
  const viewport = mode === "narrow" ? { width: 390, height: 844 } : { width: 1440, height: 900 };
  const context = await browser.newContext({ viewport,
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: viewport } } : {}) });
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
  const state = window.__reviewTest = {
    unlocked: !mode.startsWith("vault") || mode === "vault-fallback",
    holdIdentities: mode === "password-delayed", waiting: [], attempts: [], seen: new Set(),
    releaseIdentities() { state.holdIdentities = false; state.waiting.splice(0).forEach(done => done()); },
  };
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "is_vault_unlocked") return state.unlocked;
    if (command === "unlock_vault") { state.unlocked = true; return; }
    if (command === "list_hosts") {
      const hosts = await original(command, args);
      if (mode.startsWith("vault")) hosts.forEach(host => { host.auth = "publickey"; });
      if (mode === "password-delayed") hosts[0].auth = { password: { credential_key: "fixture-only" } };
      return hosts;
    }
    if (command === "list_identities" && state.holdIdentities)
      await new Promise(done => state.waiting.push(done));
    if (command === "list_workspaces" && mode.startsWith("vault")) return [{
      id: "protected", name: "Protected pair", host_ids: [], auto_connect: false,
      tabs: [{ id: "protected-tab", title: "Protected pair", layout: {
        type: "split", direction: "horizontal", ratio: 0.5,
        first: { type: "pane", host_id: "atlas", terminal_type: "ssh" },
        second: { type: "pane", host_id: "orion", terminal_type: "ssh" },
      } }],
    }];
    if (command === "connect_ssh") {
      state.attempts.push({ id: args.sessionId, host: args.host.id });
      if (mode === "vault-fallback" && !state.seen.has(args.sessionId)) {
        state.seen.add(args.sessionId);
        throw new Error("vault required");
      }
    }
    return original(command, args);
  };
}
async function startPending(page, type = "ssh") {
  await page.evaluate(() => { window.__terminalTest.holdNext = true; });
  if (type === "local") {
    await page.getByRole("button", { name: "New session", exact: true }).click();
    await page.getByRole("button", { name: "Open local shell", exact: true }).click();
  } else await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  await page.waitForFunction(() => window.__terminalTest.pending.length === 1);
}
async function openPalette(page) {
  if (page.viewportSize().width < 768) await page.getByRole("button", { name: "Open navigation", exact: true }).click();
  await page.getByRole("button", { name: "Search commands", exact: true }).click();
  const input = page.getByPlaceholder("Search commands...");
  await input.fill("Go to ");
  return input;
}
async function paletteStillOwnsInput(page, input) {
  // Do not repair focus by typing through a locator after the asynchronous event.
  assert.ok(await input.evaluate(el => document.activeElement === el), "Palette retains DOM focus");
  await page.keyboard.type("Hosts");
  assert.equal(await input.inputValue(), "Go to Hosts");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(100);
  assert.deepEqual((await inspect(page)).writes, [], "Palette input never dispatches to a shell");
}
async function restoreProtectedPair(page) {
  await page.getByRole("button", { name: "Workspaces", exact: true }).click();
  await page.getByRole("button", { name: "Restore workspace", exact: true }).click();
  await page.getByLabel("Master passphrase", { exact: true }).waitFor();
  // Both panes must reach their wait before the user settles the shared modal.
  await page.waitForFunction(() => document.querySelectorAll('.terminal-pane [aria-label="Connecting"]').length === 2);
}
async function unlock(page) {
  await page.getByLabel("Master passphrase", { exact: true }).fill("fixture-only passphrase");
  await page.getByRole("button", { name: "Unlock", exact: true }).click();
}
try {
  for (const type of ["ssh", "local"]) {
    await scenario(`focus-palette-initial-${type}`, async page => {
      await startPending(page, type);
      const input = await openPalette(page);
      await clearWrites(page);
      await page.evaluate(() => window.__terminalTest.release());
      if (type === "local") await page.waitForFunction(() => document.querySelector('.terminal-pane')?.dataset.connected === "true");
      else await connected(page, "atlas");
      await paletteStillOwnsInput(page, input);
    });
  }
  await scenario("focus-palette-reconnect", async page => {
    const old = await atlas(page);
    await page.evaluate(id => { window.__terminalTest.disconnect(id); window.__terminalTest.holdNext = true; }, old);
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    await page.waitForFunction(() => window.__terminalTest.pending.length === 1);
    const input = await openPalette(page);
    await clearWrites(page);
    await page.evaluate(() => window.__terminalTest.release());
    assert.notEqual(await connected(page, "atlas"), old);
    await paletteStillOwnsInput(page, input);
    assert.equal((await inspect(page)).connects.length, 2);
  });
  await scenario("focus-palette-narrow", async page => {
    await startPending(page);
    const input = await openPalette(page);
    await clearWrites(page);
    await page.evaluate(() => window.__terminalTest.release());
    await connected(page, "atlas");
    await paletteStillOwnsInput(page, input);
  }, "narrow");
  await scenario("focus-menu-during-connect", async page => {
    await startPending(page);
    await pane(page, "atlas").getByRole("button", { name: /^Actions for/ }).click();
    await page.waitForFunction(() => document.activeElement?.getAttribute("role") === "menuitem");
    await clearWrites(page);
    await page.evaluate(() => window.__terminalTest.release());
    await connected(page, "atlas");
    assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("role")), "menuitem");
    await page.keyboard.press("ArrowDown");
    assert.match(await page.evaluate(() => document.activeElement?.textContent), /Split below/);
    assert.deepEqual((await inspect(page)).writes, []);
  });
  await scenario("focus-delayed-password-palette", async page => {
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
    const input = await openPalette(page);
    await page.evaluate(() => window.__reviewTest.releaseIdentities());
    await page.locator('input[aria-label="SSH password"]').waitFor();
    await paletteStillOwnsInput(page, input);
    assert.equal((await inspect(page)).connects.length, 0, "No authentication without password submission");
  }, "password-delayed");
  for (const mode of ["vault-success", "vault-fallback"]) {
    await scenario(mode, async page => {
      await restoreProtectedPair(page);
      if (mode === "vault-fallback") await page.waitForFunction(() => window.__reviewTest.attempts.length === 2);
      await unlock(page);
      await connected(page, "atlas"); await connected(page, "orion");
      await page.locator('[data-tab-id="protected-tab"]').click();
      const state = await inspect(page);
      assert.equal(state.connects.length, 2, "Both waiters connect exactly once");
      assert.equal(state.attempts.length, mode === "vault-fallback" ? 4 : 2);
      assert.deepEqual(state.closes, []);
      assert.deepEqual(state.writes, []);
    }, mode);
  }
  await scenario("vault-cancel-retry", async page => {
    await restoreProtectedPair(page);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.locator('[data-tab-id="protected-tab"]').click();
    for (const host of ["atlas", "orion"]) {
      await pane(page, host).getByRole("alert").waitFor();
      assert.equal(await pane(page, host).getByRole("button", { name: "Reconnect", exact: true }).isDisabled(), false);
    }
    assert.equal((await inspect(page)).connects.length, 0);
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    await unlock(page);
    await connected(page, "atlas");
    await pane(page, "orion").getByRole("button", { name: "Reconnect", exact: true }).click();
    await connected(page, "orion");
    assert.equal((await inspect(page)).connects.length, 2);
  }, "vault-cancel");
  await scenario("focus-vault-passphrase-new-local", async page => {
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
    const input = page.getByLabel("Master passphrase", { exact: true });
    await input.fill("fixture-");
    // A global new-tab shortcut must not redirect subsequent passphrase typing.
    await page.keyboard.press("Control+n");
    await page.waitForFunction(() => window.__terminalTest.connects.some(item => item.host === "local"));
    await page.waitForFunction(() => Array.from(document.querySelectorAll('.terminal-pane')).some(el =>
      !el.dataset.hostId && el.dataset.connected === "true"));
    assert.ok(await input.evaluate(el => el === document.activeElement), "Vault retains focus above a new local shell");
    await page.keyboard.type("only passphrase");
    assert.equal(await input.inputValue(), "fixture-only passphrase");
    assert.deepEqual((await inspect(page)).writes, [], "No passphrase characters reach local terminal");
    await page.keyboard.press("Enter");
    await connected(page, "atlas");
    assert.equal((await inspect(page)).connects.length, 2);
  }, "vault-local");
} finally {
  await browser.close();
  await writeFile(`${output}/focus-vault-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
