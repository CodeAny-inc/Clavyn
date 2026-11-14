// Dialogs and the keyboard, with fixture-only Tauri IPC: focus starts in the
// dialog, Tab stays in it, Escape closes it and focus goes back to the button
// that opened it. A click on the backdrop leaves a form and its input alone.
// The update prompt, shown above an open form, owns Tab and Escape until it
// closes.
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

const dialog = page => page.getByRole("dialog");
const insideDialog = page => page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'));
const release = { available: true, version: "9.9.9", current_version: "0.1.1-ui-test", date: null, body: "Fixture release notes" };
const updatePrompt = page => page.getByRole("dialog", { name: "Update Available" });
const insideUpdatePrompt = page => page.evaluate(() => {
  const prompt = [...document.querySelectorAll('[role="dialog"]')].find(el => el.textContent.includes("Update Available"));
  return !!prompt?.contains(document.activeElement);
});
const addHostButton = page => page.getByRole("button", { name: "Add Host", exact: true }).first();

async function openAddHost(page) {
  await addHostButton(page).focus();
  await page.keyboard.press("Enter");
  await dialog(page).waitFor();
}

// `update` is the release the fixture reports, so the startup check raises the
// update prompt.
async function scenario(name, exercise, { update = null } = {}) {
  const viewport = { width: 1440, height: 900 };
  const context = await browser.newContext({ viewport,
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: viewport } } : {}) });
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  page.on("console", message => { if (["error", "warning"].includes(message.type())) errors.push(message.text()); });
  const result = { name, passed: false, viewport, errors };
  try {
    await page.addInitScript({ path: fixture });
    await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });
    if (update) await page.addInitScript(value => { window.__terminalTest.update = value; }, update);
    await page.goto(url);
    await addHostButton(page).waitFor();
    await exercise(page);
    assert.deepEqual(errors, [], "No browser errors or warnings");
    result.passed = true;
  } catch (error) { result.error = String(error.stack ?? error); }
  finally {
    await page.screenshot({ path: `${output}/${name}.png` });
    await context.close();
    results.push(result);
    console.log(`${result.passed ? "PASS" : "FAIL"}: ${name}`);
    if (result.error) console.log(result.error);
  }
}

try {
  await scenario("dialog-focus-starts-in-the-first-field", async page => {
    await openAddHost(page);
    assert.equal(await page.evaluate(() => document.activeElement?.id), "host-label");
  });

  await scenario("dialog-tab-stays-inside", async page => {
    await openAddHost(page);
    for (let step = 0; step < 25; step++) {
      await page.keyboard.press("Tab");
      assert.ok(await insideDialog(page), `Tab ${step + 1} keeps focus in the dialog`);
    }
    for (let step = 0; step < 5; step++) {
      await page.keyboard.press("Shift+Tab");
      assert.ok(await insideDialog(page), `Shift+Tab ${step + 1} keeps focus in the dialog`);
    }
  });

  await scenario("dialog-escape-closes-and-returns-focus", async page => {
    await openAddHost(page);
    await page.keyboard.press("Escape");
    await dialog(page).waitFor({ state: "detached" });
    assert.equal(await page.evaluate(() => document.activeElement?.textContent?.trim()), "Add Host");
  });

  await scenario("dialog-backdrop-click-keeps-the-form", async page => {
    await openAddHost(page);
    await page.locator("#host-label").fill("Build box");
    await page.mouse.click(10, 10);
    assert.equal(await dialog(page).count(), 1, "The form is still open");
    assert.equal(await page.locator("#host-label").inputValue(), "Build box", "Typed input survives");
  });

  await scenario("update-prompt-owns-keys-above-a-dialog", async page => {
    const form = dialog(page).filter({ hasText: "Add Host" });
    // The prompt raised at startup takes focus, and Escape dismisses it.
    await updatePrompt(page).waitFor();
    assert.ok(await insideUpdatePrompt(page), "Focus starts in the update prompt");
    await page.keyboard.press("Escape");
    await updatePrompt(page).waitFor({ state: "detached" });

    // Ctrl+U raises it again above an open form.
    await openAddHost(page);
    await page.keyboard.press("Control+u");
    await updatePrompt(page).waitFor();
    assert.equal(await form.count(), 1, "The form is still open behind the prompt");
    assert.ok(await insideUpdatePrompt(page), "Focus moves into the prompt");
    for (let step = 0; step < 10; step++) {
      await page.keyboard.press("Tab");
      assert.ok(await insideUpdatePrompt(page), `Tab ${step + 1} keeps focus in the prompt`);
    }
    for (let step = 0; step < 4; step++) {
      await page.keyboard.press("Shift+Tab");
      assert.ok(await insideUpdatePrompt(page), `Shift+Tab ${step + 1} keeps focus in the prompt`);
    }

    // Escape closes the prompt, not the form behind it.
    await page.keyboard.press("Escape");
    await updatePrompt(page).waitFor({ state: "detached" });
    assert.equal(await form.count(), 1, "Escape left the form open");
    assert.ok(await insideDialog(page), "Focus is back in the form");
    await page.keyboard.press("Tab");
    assert.ok(await insideDialog(page), "The form traps Tab again");
    await page.keyboard.press("Escape");
    await form.waitFor({ state: "detached" });
  }, { update: release });
} finally {
  await browser.close();
  await writeFile(`${output}/dialog-keyboard-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
