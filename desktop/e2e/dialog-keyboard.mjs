// Dialogs and the keyboard, with fixture-only Tauri IPC: focus starts in the
// dialog, Tab stays in it, Escape closes it and focus goes back to the button
// that opened it. A click on the backdrop leaves a form and its input alone.
// The update prompt, shown above an open form, owns Tab and Escape until it
// closes, but not above a native session picker, which keeps its own keys, and
// not under a modal that is drawn above it: the keys follow the layer on screen,
// not the order the modals opened in.
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
// Raises the update prompt the way a check that lands while the user is busy
// does, without going through a shortcut or a click that a modal would stop.
const raiseUpdatePrompt = page => page.evaluate(async () => {
  const { useUpdateStore } = await import("/src/stores/update.ts");
  useUpdateStore().showUpdateDialog();
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

  await scenario("the-modal-drawn-on-top-keeps-the-keys-when-a-lower-one-opens-later", async page => {
    // The update prompt is up first and the unlock prompt opens beneath it, the
    // way it does when a connection asks for the vault while a check lands. The
    // prompt is drawn above the unlock prompt, so Escape is the prompt's
    // although it opened earlier.
    await updatePrompt(page).waitFor();
    await page.evaluate(async () => {
      const { useUiStore } = await import("/src/stores/ui.ts");
      window.__unlockRequest = useUiStore().requestVaultUnlock();
    });
    const unlock = page.getByRole("dialog", { name: "Unlock Vault" });
    await unlock.waitFor();
    await page.keyboard.press("Escape");
    await updatePrompt(page).waitFor({ state: "detached" });
    assert.equal(await unlock.count(), 1, "Escape left the unlock prompt open");
    await page.keyboard.press("Escape");
    await unlock.waitFor({ state: "detached" });
  }, { update: release });

  await scenario("native-session-picker-keeps-its-keys-under-the-update-prompt", async page => {
    await updatePrompt(page).waitFor();
    await page.keyboard.press("Escape");
    await updatePrompt(page).waitFor({ state: "detached" });

    await page.getByRole("button", { name: "Terminal", exact: true }).first().click();
    await page.getByRole("button", { name: "New session" }).first().click();
    await page.getByRole("textbox", { name: "Search sessions" }).waitFor();
    await raiseUpdatePrompt(page);
    assert.equal(await updatePrompt(page).count(), 1, "The prompt is up behind the picker");

    // The picker is in the top layer and the page is inert, so Tab is the
    // picker's: it has to move between its controls, not stall or leave. A
    // native dialog wraps Tab through the document once per lap, so focus
    // passing `<body>` is expected; landing anywhere else on the page is not.
    const seen = new Set();
    for (let step = 0; step < 6; step++) {
      await page.keyboard.press("Tab");
      const here = await page.evaluate(() => {
        const element = document.activeElement;
        if (element?.closest("dialog[open]")) return element.getAttribute("aria-label") ?? element.tagName;
        return element === document.body ? "(document)" : null;
      });
      assert.ok(here, `Tab ${step + 1} stays in the picker`);
      seen.add(here);
    }
    assert.ok(seen.size >= 3, `Tab moves between the picker's controls (saw ${[...seen].join(", ")})`);

    // Escape closes the picker and leaves the prompt, which the user has not seen yet.
    await page.keyboard.press("Escape");
    await page.locator("dialog[open]").waitFor({ state: "hidden" });
    assert.equal(await updatePrompt(page).count(), 1, "The prompt is still up");
    await page.keyboard.press("Escape");
    await updatePrompt(page).waitFor({ state: "detached" });
  }, { update: release });
} finally {
  await browser.close();
  await writeFile(`${output}/dialog-keyboard-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
