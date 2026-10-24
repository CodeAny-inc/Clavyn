// Destructive actions ask in the page, with fixture-only Tauri IPC. The fixture
// installs the dialog plugin's async `confirm`, so a caller that still relied
// on it would go ahead without waiting for an answer, as it does in the app.
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

const KEY = {
  id: "key-1", label: "Laptop key", key_type: "ssh-ed25519",
  fingerprint: "SHA256:fixturefingerprint", public_key_base64: "AAAAC3NzaC1lZDI1NTE5AAAAIfixture",
};
// Serves one vault key, so the Keys view has something to delete.
function serveKey(key) {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = (command, args) =>
    command === "list_keys" ? (window.__terminalTest.calls.push({ command, args }), Promise.resolve([key])) : invoke(command, args);
}

const calls = (page, command) =>
  page.evaluate(command => window.__terminalTest.calls.filter(call => call.command === command), command);
const confirmation = page => page.getByRole("alertdialog");

async function deleteAtlas(page) {
  const row = page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first();
  await row.hover();
  await page.getByRole("button", { name: "Delete host" }).filter({ visible: true }).first().click();
  await confirmation(page).waitFor();
}

async function scenario(name, exercise) {
  const viewport = { width: 1440, height: 900 };
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
    await page.addInitScript(serveKey, KEY);
    await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });
    await page.goto(url);
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().waitFor();
    assert.equal(await page.locator("vite-error-overlay").count(), 0);
    assert.ok(await page.evaluate(() => {
      const answer = window.confirm("probe");
      answer.catch(() => {});
      return answer instanceof Promise;
    }),
      "The fixture's confirm is async, as the dialog plugin makes it");
    await page.evaluate(() => { window.__terminalTest.calls.length = 0; });
    await exercise(page);
    assert.deepEqual(await calls(page, "plugin:dialog|confirm"), [], "The native confirm is never used");
    assert.deepEqual(errors, [], "No browser errors or warnings");
    result.passed = true;
  } catch (error) { result.error = String(error.stack ?? error); }
  finally {
    await page.screenshot({ path: `${output}/${name}.png` });
    await context.tracing.stop({ path: `${output}/${name}-trace.zip` });
    await context.close();
    results.push(result);
    console.log(`${result.passed ? "PASS" : "FAIL"}: ${name}`);
    if (result.error) console.log(result.error);
  }
}

try {
  await scenario("confirm-cancel-keeps-the-host", async page => {
    await deleteAtlas(page);
    const dialog = confirmation(page);
    assert.match(await dialog.textContent(), /Delete host "Atlas Production"\?/);
    assert.equal(await dialog.getAttribute("aria-modal"), "true");
    // The safe answer has focus for a destructive action.
    assert.equal(await page.evaluate(() => document.activeElement?.textContent?.trim()), "Cancel");
    assert.deepEqual(await calls(page, "delete_host"), [], "Nothing is deleted while the question is open");
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await dialog.waitFor({ state: "detached" });
    assert.deepEqual(await calls(page, "delete_host"), []);
    assert.equal(await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).count(), 1);
  });

  await scenario("confirm-escape-keeps-the-host", async page => {
    await deleteAtlas(page);
    await page.keyboard.press("Escape");
    await confirmation(page).waitFor({ state: "detached" });
    assert.deepEqual(await calls(page, "delete_host"), []);
  });

  await scenario("confirm-delete-removes-the-host", async page => {
    await deleteAtlas(page);
    await confirmation(page).getByRole("button", { name: "Delete" }).click();
    await confirmation(page).waitFor({ state: "detached" });
    assert.deepEqual((await calls(page, "delete_host")).map(call => call.args.id), ["atlas"]);
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).waitFor({ state: "detached" });
  });

  await scenario("confirm-guards-deleting-a-vault-key", async page => {
    await page.getByRole("button", { name: "Keys", exact: true }).first().click();
    const row = page.getByText(KEY.label, { exact: true });
    await row.hover();
    await page.getByRole("button", { name: "Delete key" }).click();
    await confirmation(page).waitFor();
    assert.deepEqual(await calls(page, "delete_key"), [], "The key stays until the user confirms");
    await confirmation(page).getByRole("button", { name: "Cancel" }).click();
    assert.deepEqual(await calls(page, "delete_key"), []);

    await row.hover();
    await page.getByRole("button", { name: "Delete key" }).click();
    await confirmation(page).getByRole("button", { name: "Delete" }).click();
    await row.waitFor({ state: "detached" });
    assert.deepEqual((await calls(page, "delete_key")).map(call => call.args.keyId), [KEY.id]);
  });
} finally {
  await browser.close();
  await writeFile(`${output}/confirm-dialogs-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
