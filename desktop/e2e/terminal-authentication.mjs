// Real Vue/xterm input checks. All hosts and passwords here are test fixtures.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";

const url = process.env.UI_URL ?? "http://127.0.0.1:1420";
const output = process.env.UI_RESULTS ?? "ui-test-results";
const record = process.env.UI_RECORD !== "0";
const fixture = await readFile(new URL("./tauri-fixture.js", import.meta.url), "utf8");
const fixturePassword = "fixture-only password";
function authenticationFixture(mode) {
  const original = window.__TAURI_INTERNALS__.invoke;
  const passwordAuth = { password: { credential_key: "fixture-metadata" } };
  const state = window.__authTest = {
    identity: { id: "shared", label: "Shared admin", username: "root", tags: [],
      auth: mode.startsWith("password") ? passwordAuth : "agent" },
    hold: mode === "delayed", failLoad: mode === "failed", pending: [], attempts: [],
    release() { state.hold = false; state.pending.splice(0).forEach(resolve => resolve()); },
  };
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === "list_hosts") {
      const hosts = await original(command, args);
      if (mode === "password-host") hosts[0].auth = passwordAuth;
      else hosts[0].identity_id = state.identity.id;
      return hosts;
    }
    if (command === "list_identities") {
      if (state.hold) await new Promise(resolve => state.pending.push(resolve));
      if (state.failLoad) throw new Error("Fixture: identity loading failed");
      return [structuredClone(state.identity)];
    }
    if (command === "update_identity") {
      state.identity = structuredClone(args.identity);
      return structuredClone(state.identity);
    }
    if (command === "connect_ssh") {
      const effective = args.host.identity_id === state.identity.id ? state.identity : args.host;
      if (args.expectedUsername !== effective.username)
        throw new Error("SSH identity changed. Reload identities and reconnect to review the account.");
      const needsPassword = typeof effective.auth === "object";
      const accepted = !needsPassword || args.password === "fixture-only password";
      state.attempts.push({ id: args.sessionId, username: effective.username, passwordRequired: needsPassword, accepted });
      if (!accepted) throw new Error("Fixture: authentication rejected");
      // Never retain the password in the base fixture's call log or JSON artifacts.
      await original(command, { ...args, password: args.password === null ? null : "[redacted]",
        host: { ...args.host, username: effective.username } });
      return { username: effective.username, hostname: args.host.hostname, port: args.host.port };
    }
    return original(command, args);
  };
}
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_EXECUTABLE_PATH,
  slowMo: Number(process.env.UI_SLOW_MO ?? 0) });
const results = [];
const pane = page => page.locator('[data-host-id="atlas"]');
async function connectAtlas(page) {
  await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
}
async function connected(page) {
  await page.waitForFunction(() => document.querySelector('[data-host-id="atlas"]')?.getAttribute("data-connected") === "true");
  return pane(page).getAttribute("data-session-id");
}
async function scenario(name, mode, exercise, narrow = false) {
  const viewport = narrow ? { width: 390, height: 844 } : { width: 1440, height: 900 };
  const context = await browser.newContext({ viewport,
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: viewport } } : {}) });
  await context.tracing.start({ screenshots: true, snapshots: true });
  const page = await context.newPage();
  page.setDefaultTimeout(12000);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  page.on("console", message => { if (["error", "warning"].includes(message.type())) errors.push(message.text()); });
  const result = { name, passed: false, viewport, errors };
  try {
    await page.addInitScript({ content: fixture + `\n(${authenticationFixture.toString()})(${JSON.stringify(mode)});` });
    await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });
    await page.goto(url);
    assert.match(await page.title(), /Clavyn/i);
    assert.ok(page.url().startsWith(url));
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().waitFor();
    assert.equal(await page.locator("vite-error-overlay").count(), 0);
    await exercise(page);
    assert.deepEqual(errors, []);
    result.passed = true;
  } catch (error) {
    result.error = String(error.stack ?? error);
  } finally {
    result.state = await page.evaluate(() => ({ attempts: window.__authTest.attempts,
      connects: window.__terminalTest.connects, writes: window.__terminalTest.writes,
      activeHost: document.querySelector('.terminal-pane[data-active="true"]')?.getAttribute("data-host-id"),
      focusedHost: document.activeElement?.closest(".terminal-pane")?.getAttribute("data-host-id"),
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
  await scenario("auth-delayed-identities", "delayed", async page => {
    // Opening the terminal is allowed immediately, but authentication must wait
    // for the linked identity and never advertise/dispatch the host fallback.
    await connectAtlas(page);
    await page.waitForFunction(() => window.__authTest.pending.length > 0);
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);
    assert.equal(await pane(page).count(), 1);
    assert.equal(await pane(page).getAttribute("data-connected"), "false");
    assert.match(await pane(page).locator("header").innerText(), /Resolving SSH identity/);
    assert.equal(await page.getByText("deploy@atlas.example.test:22", { exact: true }).count(), 0);
    await page.getByRole("button", { name: "New session", exact: true }).click();
    const picker = page.locator("dialog[open]");
    assert.ok(await picker.getByRole("button", { name: "Connect Atlas Production", exact: true }).isDisabled());
    assert.ok(await picker.getByRole("button", { name: "Open local shell", exact: true }).isEnabled());
    await picker.getByRole("button", { name: "Close session picker", exact: true }).click();
    await page.evaluate(() => window.__authTest.release());
    await connected(page);
    assert.match(await pane(page).locator("header").innerText(), /root@atlas\.example\.test:22/);
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 1);
  });
  await scenario("auth-identity-load-retry", "failed", async page => {
    await page.getByRole("button", { name: "New session", exact: true }).click();
    const picker = page.locator("dialog[open]");
    await picker.getByRole("alert").filter({ hasText: "Could not load SSH identities" }).waitFor();
    assert.ok(await picker.getByRole("button", { name: "Connect Atlas Production", exact: true }).isDisabled());
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);
    await page.evaluate(() => { window.__authTest.failLoad = false; });
    await picker.getByRole("button", { name: "Retry identities", exact: true }).click();
    const choice = picker.getByRole("button", { name: "Connect Atlas Production", exact: true });
    await page.waitForFunction(() => !document.querySelector('dialog[open] [aria-label="Connect Atlas Production"]')?.disabled);
    assert.match(await choice.innerText(), /root@atlas\.example\.test:22/);
    await picker.getByRole("textbox", { name: "Search sessions", exact: true }).fill("root");
    await page.keyboard.press("Enter");
    await connected(page);
  });
  for (const mode of ["password-identity", "password-host"]) {
    await scenario(`auth-${mode}`, mode, async page => {
      await connectAtlas(page);
      const password = page.locator('input[aria-label="SSH password"]');
      await password.waitFor();
      await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "SSH password");
      assert.match(await pane(page).getByRole("region").innerText(), mode === "password-host" ? /deploy@/ : /root@/);
      assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);
      await password.fill(fixturePassword);
      await page.screenshot({ path: `${output}/auth-${mode}-prompt.png` });
      assert.deepEqual(await page.evaluate(() => window.__terminalTest.writes), []);
      await page.keyboard.press("Enter");
      const original = await connected(page);
      await page.waitForFunction(() => document.activeElement?.classList.contains("xterm-helper-textarea"));
      assert.deepEqual(await page.evaluate(() => window.__terminalTest.writes), []);
      assert.ok(await page.evaluate(() => window.__authTest.attempts.every(attempt => attempt.accepted)));
      assert.ok(await page.evaluate(secret => !JSON.stringify(window.__terminalTest.calls).includes(secret) &&
        !JSON.stringify(localStorage).includes(secret), fixturePassword));
      await page.evaluate(() => { window.__authXterm = document.querySelector('[data-host-id="atlas"] .xterm'); });
      await page.keyboard.type("echo AUTH_INPUT"); await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__terminalTest.writes.some(write => write.text === "\r"));
      assert.ok(await page.evaluate(id => window.__terminalTest.writes.every(write => write.id === id), original));
      assert.equal(await page.evaluate(() => window.__terminalTest.writes.map(write => write.text).join("")), "echo AUTH_INPUT\r");
      await page.evaluate(id => window.__terminalTest.disconnect(id), original);
      await pane(page).getByRole("button", { name: "Reconnect", exact: true }).click();
      await password.waitFor();
      assert.equal(await password.inputValue(), "");
      await password.fill(fixturePassword);
      await pane(page).getByRole("button", { name: "Connect with password", exact: true }).click();
      assert.notEqual(await connected(page), original);
      assert.ok(await page.evaluate(() => window.__authXterm === document.querySelector('[data-host-id="atlas"] .xterm')));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    }, mode === "password-host");
  }
  await scenario("auth-password-cancel-and-close", "password-identity", async page => {
    await connectAtlas(page);
    const password = page.locator('input[aria-label="SSH password"]');
    await password.fill(fixturePassword);
    await page.keyboard.press("Escape");
    await pane(page).getByRole("alert").filter({ hasText: "Connection cancelled" }).waitFor();
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);
    await pane(page).getByRole("button", { name: "Reconnect", exact: true }).click();
    await password.waitFor(); assert.equal(await password.inputValue(), "");
    await password.fill(fixturePassword);
    await page.getByRole("button", { name: "Close tab Atlas Production", exact: true }).click();
    await password.waitFor({ state: "hidden" });
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);
    assert.deepEqual(await page.evaluate(() => window.__terminalTest.writes), []);
  });
  await scenario("auth-account-edited-during-prompt", "password-identity", async page => {
    await connectAtlas(page);
    const password = page.locator('input[aria-label="SSH password"]');
    await password.waitFor();
    await password.fill(fixturePassword);

    // Leaving the persistent pane must cancel and clear the pending credential.
    await page.getByRole("button", { name: "Identities", exact: true }).click();
    await password.waitFor({ state: "hidden" });
    assert.equal(await page.evaluate(() => window.__terminalTest.connects.length), 0);

    await page.getByRole("button", { name: "Edit identity", exact: true }).click();
    await page.getByRole("textbox", { name: "Username", exact: true }).fill("ops");
    await page.getByRole("button", { name: "Save changes", exact: true }).click();
    await page.getByRole("textbox", { name: "Username", exact: true }).waitFor({ state: "hidden" });
    await page.locator("[data-tab-id]").first().click();
    await pane(page).getByRole("alert").filter({ hasText: "Connection cancelled" }).waitFor();

    // Reconnect requests a fresh password for the updated effective account.
    await pane(page).getByRole("button", { name: "Reconnect", exact: true }).click();
    await password.waitFor();
    assert.equal(await password.inputValue(), "");
    assert.match(await pane(page).getByRole("region").innerText(), /ops@atlas\.example\.test:22/);
    await password.fill(fixturePassword);
    await page.keyboard.press("Enter");
    await connected(page);
    assert.match(await pane(page).locator("header").innerText(), /ops@atlas\.example\.test:22/);
  });
} finally {
  await browser.close();
  await writeFile(`${output}/authentication-results.json`, JSON.stringify({ transport: "mocked Tauri; real Vue/xterm; fixture-only passwords", results }, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
