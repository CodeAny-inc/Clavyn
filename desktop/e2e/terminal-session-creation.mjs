// Real dialog/menu focus and xterm regressions; SSH remains a test-only transport.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";

const url = process.env.UI_URL ?? "http://127.0.0.1:1420";
const output = process.env.UI_RESULTS ?? "ui-test-results";
const record = process.env.UI_RECORD !== "0";
const fixture = await readFile(new URL("./tauri-fixture.js", import.meta.url), "utf8");
// Extend IPC only for these identity scenarios. The saved host deliberately keeps
// its old username. Resolve the linked identity as core/src/connection.rs does.
//
// resolvedSshHost() deliberately clears identity_id when freezing the transport
// host, so the fixture cannot match on args.host.identity_id at connect_ssh
// time. Instead it tracks linked host IDs (and their original username) from
// list_hosts and matches connect_ssh by host.id.
const identityFixture = `(() => {
  let identity = { id: "fixture-identity", label: "Shared admin", username: "root", auth: "agent", tags: [] };
  const original = window.__TAURI_INTERNALS__.invoke;
  const state = window.__terminalTest;
  state.effectiveAttempts = [];
  const linkedHosts = new Map();
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "list_hosts") {
      const hosts = await original(command, args);
      hosts[0].identity_id = identity.id;
      linkedHosts.set(hosts[0].id, hosts[0].username);
      return hosts;
    }
    if (command === "list_identities") return [structuredClone(identity)];
    if (command === "update_identity") {
      identity = structuredClone(args.identity);
      return structuredClone(identity);
    }
    if (command === "connect_ssh" && linkedHosts.has(args.host?.id)) {
      const username = identity.username;
      state.effectiveAttempts.push({ id: args.sessionId, username, hostUsername: linkedHosts.get(args.host.id) });
      return original(command, { ...args, host: { ...args.host, username } });
    }
    return original(command, args);
  };
})();`;
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true,
  executablePath: process.env.CHROMIUM_EXECUTABLE_PATH,
  slowMo: Number(process.env.UI_SLOW_MO ?? 0) });
const results = [];
const pane = (page, host) => page.locator(`[data-host-id="${host}"]`);
const inspect = page => page.evaluate(() => ({
  active: document.querySelector('.terminal-pane[data-active="true"]')?.getAttribute("data-host-id") ?? null,
  focused: document.activeElement?.closest(".terminal-pane")?.getAttribute("data-host-id") ?? null,
  focusLabel: document.activeElement?.getAttribute("aria-label") ?? null,
  terminalFocused: document.activeElement?.classList.contains("xterm-helper-textarea") ?? false,
  selectedTab: document.querySelector('[data-tab-id][aria-pressed="true"]')?.getAttribute("data-tab-id") ?? null,
  writes: window.__terminalTest.writes,
  connects: window.__terminalTest.connects,
  closes: window.__terminalTest.closes,
  effectiveAttempts: window.__terminalTest.effectiveAttempts,
}));
async function connected(page, host) {
  await page.waitForFunction(host => document.querySelector(`[data-host-id="${host}"]`)?.getAttribute("data-connected") === "true", host);
  return pane(page, host).getAttribute("data-session-id");
}
async function openAtlas(page) {
  await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  return connected(page, "atlas");
}
async function openFromPane(page) {
  await pane(page, "atlas").getByRole("button", { name: "Actions for Atlas Production", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split right…", exact: true }).click();
  await page.getByRole("textbox", { name: "Search sessions" }).waitFor();
}
async function scenario(name, exercise, identity = false) {
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
    // One init script guarantees the base transport is installed before its extension.
    await page.addInitScript({ content: fixture + (identity ? "\n" + identityFixture : "") });
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
async function editIdentity(page, username) {
  await page.getByRole("button", { name: "Identities", exact: true }).click();
  await page.getByRole("button", { name: "Edit identity", exact: true }).click();
  await page.getByRole("textbox", { name: "Username", exact: true }).fill(username);
  await page.getByRole("button", { name: "Save changes", exact: true }).click();
  await page.getByRole("textbox", { name: "Username", exact: true }).waitFor({ state: "hidden" });
  await page.locator('[data-tab-id]').first().click();
}
try {
  for (const placement of ["horizontal", "vertical", "tab"]) {
    await scenario(`creation-picker-${placement}`, async page => {
      const atlas = await openAtlas(page);
      const sourceTab = await page.locator('[data-tab-id][aria-pressed="true"]').getAttribute("data-tab-id");
      await page.evaluate(() => { window.__creationSource = document.querySelector('[data-host-id="atlas"] .xterm'); });
      await openFromPane(page);
      await page.getByRole("combobox", { name: "Open session in" }).selectOption(placement);
      await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).click();
      const orion = await connected(page, "orion");
      // Do not focus/click the new terminal: creation itself must route input.
      await page.waitForFunction(() => document.activeElement?.classList.contains("xterm-helper-textarea") &&
        document.activeElement.closest(".terminal-pane")?.getAttribute("data-host-id") === "orion");
      await page.evaluate(() => { window.__terminalTest.writes.length = 0; });
      await page.keyboard.type("echo NEW_SESSION_INPUT");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__terminalTest.writes.some(write => write.text === "\r"));
      const state = await inspect(page);
      assert.equal(state.active, "orion");
      assert.equal(state.focused, "orion");
      assert.equal(state.terminalFocused, true);
      assert.ok(await pane(page, "orion").isVisible());
      assert.equal(await page.locator('[data-tab-id]').count(), placement === "tab" ? 2 : 1);
      if (placement === "tab") assert.notEqual(state.selectedTab, sourceTab);
      else assert.equal(state.selectedTab, sourceTab);
      assert.ok(state.writes.every(write => write.id === orion));
      assert.equal(state.writes.map(write => write.text).join(""), "echo NEW_SESSION_INPUT\r");
      assert.equal(await pane(page, "atlas").getAttribute("data-session-id"), atlas);
      assert.equal(state.connects.length, 2);
      assert.deepEqual(state.closes, []);
      assert.ok(await page.evaluate(() => window.__creationSource === document.querySelector('[data-host-id="atlas"] .xterm')));
    });
  }
  for (const cancel of ["escape", "button"]) {
    await scenario(`creation-picker-cancel-${cancel}`, async page => {
      const atlas = await openAtlas(page);
      await openFromPane(page);
      if (cancel === "escape") await page.keyboard.press("Escape");
      else await page.getByRole("button", { name: "Close session picker", exact: true }).click();
      await page.locator("dialog[open]").waitFor({ state: "hidden" });
      const state = await inspect(page);
      assert.equal(state.active, "atlas");
      assert.equal(state.focusLabel, "Actions for Atlas Production", "Cancellation returns focus to the source control");
      assert.equal(await pane(page, "atlas").getAttribute("data-session-id"), atlas);
      assert.equal(state.connects.length, 1);
      assert.deepEqual(state.closes, []);
      assert.deepEqual(state.writes, []);
    });
  }
  await scenario("creation-identity-snapshot", async page => {
    const original = await openAtlas(page);
    const header = pane(page, "atlas").locator('[data-testid="pane-header"]');
    assert.match(await header.innerText(), /root@atlas\.example\.test:22/);
    assert.deepEqual((await inspect(page)).effectiveAttempts, [{ id: original, username: "root", hostUsername: "deploy" }]);
    await page.evaluate(() => { window.__identityTerminal = document.querySelector('[data-host-id="atlas"] .xterm'); });
    await editIdentity(page, "ops");
    assert.match(await header.innerText(), /root@atlas\.example\.test:22/, "Editing an identity must not relabel a live session");
    await page.evaluate(id => { window.__terminalTest.disconnect(id); window.__terminalTest.failNext = true; }, original);
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    await pane(page, "atlas").getByRole("alert").filter({ hasText: "connection refused" }).waitFor();
    assert.match(await header.innerText(), /root@atlas\.example\.test:22/, "A failed attempt must not publish its endpoint");
    await pane(page, "atlas").getByRole("button", { name: "Reconnect", exact: true }).click();
    const replacement = await connected(page, "atlas");
    assert.notEqual(replacement, original);
    assert.match(await header.innerText(), /ops@atlas\.example\.test:22/);
    const state = await inspect(page);
    assert.equal(state.effectiveAttempts.at(-1).username, "ops");
    assert.equal(state.effectiveAttempts.at(-1).hostUsername, "deploy", "Host fallback is deliberately stale");
    assert.equal(state.connects.length, 3, "One initial connection, one failed attempt, one successful retry");
    assert.ok(await page.evaluate(() => window.__identityTerminal === document.querySelector('[data-host-id="atlas"] .xterm')));
  }, true);
} finally {
  await browser.close();
  await writeFile(`${output}/session-creation-results.json`, JSON.stringify({ transport: "mocked Tauri; real Vue/dialog/menu/xterm", results }, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
