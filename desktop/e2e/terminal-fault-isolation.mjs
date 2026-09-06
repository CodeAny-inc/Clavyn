// Real Vue/xterm regressions with fault-injected, fixture-only Tauri IPC.
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
const pane = (page, local = false) => page.locator(local ? '.terminal-pane:not([data-host-id])' : '[data-host-id="atlas"]');

function faultFixture(options) {
  const original = window.__TAURI_INTERNALS__.invoke;
  const identity = { id: "shared", label: "Fixture identity", username: "root", auth: "agent", tags: [] };
  const state = window.__faultTest = {
    holdWrite: false, rejectWrite: null, heldWriteId: null,
    holdIdentities: options.identities === "pending", failIdentities: options.identities === "failed",
    identityWaiters: [], identityCalls: 0, attempts: [],
    releaseIdentities() { state.holdIdentities = false; state.identityWaiters.splice(0).forEach(resolve => resolve()); },
  };
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "list_hosts") {
      const hosts = await original(command, args);
      hosts[0].identity_id = null;
      hosts[0].auth = options.auth === "password" ? { password: { credential_key: "fixture-only" } } : options.auth ?? "agent";
      if (options.auth === "publickey") hosts[0].key_id = "fixture-key";
      hosts[1].identity_id = identity.id;
      return hosts;
    }
    if (command === "list_identities") {
      state.identityCalls++;
      if (state.holdIdentities) await new Promise(resolve => state.identityWaiters.push(resolve));
      if (state.failIdentities) throw new Error("Fixture identity service unavailable");
      return [structuredClone(identity)];
    }
    if (command === "connect_ssh") {
      const effective = args.host.identity_id ? identity : args.host;
      if (args.expectedUsername !== effective.username) throw new Error("Fixture expected account mismatch");
      const needsPassword = typeof effective.auth === "object";
      if (needsPassword && args.password !== "fixture-only password") throw new Error("Fixture password mismatch");
      state.attempts.push({ id: args.sessionId, username: effective.username, auth: needsPassword ? "password" : effective.auth });
      await original(command, { ...args, password: args.password === null ? null : "[redacted]" });
      return { username: effective.username, hostname: args.host.hostname, port: args.host.port };
    }
    if (command === "session_write" && state.holdWrite) {
      state.holdWrite = false;
      state.heldWriteId = args.sessionId;
      await new Promise((_, reject) => { state.rejectWrite = () => reject(new Error("LATE_WRITE_FAILURE_FIXTURE")); });
      return;
    }
    return original(command, args);
  };
}
async function connected(page, local = false) {
  const target = pane(page, local);
  await target.locator('[role="status"]').filter({ hasText: /^Connected$/ }).waitFor();
  return target.getAttribute("data-session-id");
}
async function open(page, local = false) {
  if (local) {
    await page.getByRole("button", { name: "New session", exact: true }).click();
    await page.getByRole("button", { name: "Open local shell", exact: true }).click();
  } else {
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  }
  return connected(page, local);
}
async function transcript(page, local = false) { return pane(page, local).locator('.xterm-rows').textContent(); }
async function password(page) {
  const input = pane(page).locator('input[aria-label="SSH password"]');
  await input.fill("fixture-only password");
  await page.getByRole("button", { name: "Connect with password", exact: true }).click();
}
async function scenario(name, options, exercise) {
  const viewport = options.narrow ? { width: 390, height: 844 } : { width: 1440, height: 900 };
  const context = await browser.newContext({ viewport,
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: viewport } } : {}) });
  await context.tracing.start({ screenshots: true, snapshots: true });
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  page.on("console", message => { if (["error", "warning"].includes(message.type())) errors.push(message.text()); });
  const result = { name, passed: false, viewport, errors };
  try {
    await page.addInitScript({ path: fixture });
    await page.addInitScript(faultFixture, options);
    await page.goto(url);
    assert.match(await page.title(), /OpenTermius/i, "Correct page title");
    assert.ok(page.url().startsWith(url), "Correct app URL");
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().waitFor();
    assert.equal(await page.locator("vite-error-overlay").count(), 0);
    await exercise(page);
    assert.deepEqual(errors, [], "No application console errors/warnings");
    result.passed = true;
  } catch (error) { result.error = String(error.stack ?? error); }
  finally {
    result.state = await page.evaluate(() => ({
      connects: window.__terminalTest.connects, closes: window.__terminalTest.closes, writes: window.__terminalTest.writes,
      attempts: window.__faultTest.attempts, identityCalls: window.__faultTest.identityCalls,
    })).catch(() => null);
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
  for (const local of [false, true]) for (const boundary of ["eof", "replacement", "current"]) {
    await scenario(`fault-write-${local ? "local" : "ssh"}-${boundary}`, {}, async page => {
      const oldId = await open(page, local);
      const target = pane(page, local);
      await page.evaluate(() => { window.__savedXterm = document.querySelector('.terminal-pane .xterm'); window.__faultTest.holdWrite = true; });
      await target.locator('.xterm').click();
      await page.keyboard.type("x");
      await page.waitForFunction(() => typeof window.__faultTest.rejectWrite === "function");
      if (boundary !== "current") {
        await page.evaluate(id => window.__terminalTest.disconnect(id), oldId);
        await target.getByRole("alert").waitFor();
        if (boundary === "replacement") {
          await target.getByRole("button", { name: "Reconnect", exact: true }).click();
          assert.notEqual(await connected(page, local), oldId);
        }
      }
      // Allow queued xterm output to settle before comparing the rendered screen.
      await page.waitForTimeout(100);
      const before = await transcript(page, local);
      const alertBefore = await target.getByRole("alert").allTextContents();
      await page.evaluate(() => window.__faultTest.rejectWrite());
      await page.waitForTimeout(150);
      if (boundary === "current") {
        assert.match(await target.getByRole("alert").textContent(), /LATE_WRITE_FAILURE_FIXTURE/);
        assert.match(await transcript(page, local), /LATE_WRITE_FAILURE_FIXTURE/);
      } else {
        assert.equal(await transcript(page, local), before, "A stale failure must not change the terminal transcript");
        assert.deepEqual(await target.getByRole("alert").allTextContents(), alertBefore, "A stale failure must not replace the current status");
      }
      assert.ok(await page.evaluate(() => window.__savedXterm === document.querySelector('.terminal-pane .xterm')));
      if (boundary === "replacement") {
        await page.evaluate(() => { window.__terminalTest.writes.length = 0; });
        await target.locator('.xterm').click();
        await page.keyboard.type("echo REPLACEMENT_OK"); await page.keyboard.press("Enter");
        const writes = await page.evaluate(() => window.__terminalTest.writes);
        assert.equal(writes.map(write => write.text).join(""), "echo REPLACEMENT_OK\r");
        assert.ok(writes.every(write => write.id !== oldId));
      }
    });
  }
  for (const identities of ["pending", "failed"]) for (const auth of ["agent", "publickey", "password"]) {
    await scenario(`fault-direct-${auth}-${identities}`, { identities, auth, narrow: auth === "password" && identities === "failed" }, async page => {
      await page.waitForFunction(() => window.__faultTest.identityCalls > 0);
      await page.getByRole("button", { name: "New session", exact: true }).click();
      const choice = page.getByRole("button", { name: "Connect Atlas Production", exact: true });
      assert.equal(await choice.isDisabled(), false, "Direct host remains usable without the identity list");
      assert.match(await choice.textContent(), /deploy@atlas.example.test:22/);
      assert.equal(await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).isDisabled(), true, "Linked host remains blocked");
      await page.getByRole("textbox", { name: "Search sessions", exact: true }).fill("deploy");
      assert.equal(await choice.count(), 1, "Direct account remains searchable");
      await page.getByRole("textbox", { name: "Search sessions", exact: true }).press("Enter");
      if (auth === "password") await password(page);
      const oldId = await connected(page);
      await page.evaluate(id => window.__terminalTest.disconnect(id), oldId);
      await pane(page).getByRole("button", { name: "Reconnect", exact: true }).click();
      if (auth === "password") await password(page);
      assert.notEqual(await connected(page), oldId, "Direct reconnect is independent too");
      const attempts = await page.evaluate(() => window.__faultTest.attempts);
      assert.equal(attempts.length, 2);
      assert.ok(attempts.every(attempt => attempt.username === "deploy" && attempt.auth === auth));
      assert.equal(await page.evaluate(() => window.__terminalTest.writes.length), 0, "Password never enters terminal transport");
    });
  }
  await scenario("fault-linked-retry", { identities: "failed" }, async page => {
    await page.getByRole("button", { name: "New session", exact: true }).click();
    const orion = page.getByRole("button", { name: "Connect Orion Staging", exact: true });
    assert.equal(await orion.isDisabled(), true);
    assert.equal(await page.evaluate(() => window.__faultTest.attempts.length), 0);
    await page.evaluate(() => { window.__faultTest.failIdentities = false; });
    await page.getByRole("button", { name: "Retry identities", exact: true }).click();
    await page.waitForFunction(() => !document.querySelector('[aria-label="Connect Orion Staging"]').disabled);
    await orion.click();
    await page.locator('[data-host-id="orion"] [role="status"]').filter({ hasText: /^Connected$/ }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.__faultTest.attempts.map(attempt => attempt.username)), ["root"]);
  });
} finally {
  await browser.close();
  await writeFile(`${output}/fault-isolation-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
