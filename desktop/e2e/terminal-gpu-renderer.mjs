// The GPU renderer path, with fixture-only Tauri IPC. Every other suite reports
// no WebGL2 so it can read a pane's transcript out of `.xterm-rows`; this one
// opts back in and asserts against the renderer the app actually ships with.
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

// The GPU driver reports performance notices through the console. They come
// from the graphics stack rather than the application, so they are not
// application faults the way every other console warning is.
const driverNoise = /WebGL|GroupMarkerNotSet|swiftshader|GL Driver Message/i;

const renderer = page => page.evaluate(async () => {
  const budget = await import("/src/lib/terminalRenderer.ts");
  return {
    backed: budget.webglBackedPaneIds(),
    limit: budget.WEBGL_PANE_BUDGET,
    canvases: document.querySelectorAll(".xterm canvas").length,
    domRows: document.querySelectorAll(".xterm-rows").length,
  };
});
const atlas = page => page.locator('[data-host-id="atlas"]');
async function openAtlas(page) {
  await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().dblclick();
  await atlas(page).locator('[role="status"]').filter({ hasText: /^Connected$/ }).waitFor();
}

async function scenario(name, exercise) {
  const viewport = { width: 1440, height: 900 };
  const context = await browser.newContext({ viewport,
    ...(record ? { recordVideo: { dir: `${output}/raw-video`, size: viewport } } : {}) });
  await context.tracing.start({ screenshots: true, snapshots: true });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  page.on("console", message => {
    if (["error", "warning"].includes(message.type()) && !driverNoise.test(message.text())) errors.push(message.text());
  });
  const result = { name, passed: false, viewport, errors };
  try {
    await page.addInitScript({ content: "window.__clavynAllowWebgl = true;" });
    await page.addInitScript({ path: fixture });
    await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });
    await page.goto(url);
    await page.getByText("Atlas Production", { exact: true }).filter({ visible: true }).first().waitFor();
    assert.equal(await page.locator("vite-error-overlay").count(), 0);
    assert.ok(await page.evaluate(() => !!document.createElement("canvas").getContext("webgl2")),
      "The harness browser has WebGL2, so this suite tests the GPU renderer");
    await exercise(page);
    assert.deepEqual(errors, [], "No application console errors/warnings");
    result.passed = true;
  } catch (error) { result.error = String(error.stack ?? error); }
  finally {
    result.renderer = await renderer(page).catch(() => null);
    await page.screenshot({ path: `${output}/${name}.png` });
    await context.tracing.stop({ path: `${output}/${name}-trace.zip` });
    await context.close();
    results.push(result);
    console.log(`${result.passed ? "PASS" : "FAIL"}: ${name}`);
    if (result.error) console.log(result.error);
  }
}

try {
  await scenario("gpu-visible-pane-is-accelerated", async page => {
    await openAtlas(page);
    const state = await renderer(page);
    assert.equal(state.backed.length, 1, "The pane on screen is GPU-backed");
    assert.ok(state.canvases > 0, "xterm draws to a canvas");
    assert.equal(state.domRows, 0, "The DOM row renderer is gone");
    assert.equal(await atlas(page).getAttribute("data-connected"), "true");
  });

  await scenario("gpu-input-still-reaches-the-session", async page => {
    await openAtlas(page);
    await atlas(page).locator(".xterm-helper-textarea").focus();
    await page.keyboard.type("whoami");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__terminalTest.writes.length > 0);
    const typed = await page.evaluate(() => window.__terminalTest.writes.map(write => write.text).join(""));
    assert.ok(typed.includes("whoami"), `Keystrokes reach the transport (got ${JSON.stringify(typed)})`);
  });

  await scenario("gpu-context-loss-falls-back-to-the-dom-renderer", async page => {
    await openAtlas(page);
    assert.equal((await renderer(page)).backed.length, 1);
    // A GPU reset, a driver update or the browser reclaiming contexts looks
    // exactly like this to the page.
    await page.evaluate(() => {
      const canvas = [...document.querySelectorAll(".xterm canvas")]
        .find(element => element.getContext("webgl2"));
      canvas.getContext("webgl2").getExtension("WEBGL_lose_context").loseContext();
    });
    // The addon allows three seconds for the context to come back before it
    // gives up and reports the loss.
    await page.waitForFunction(async () =>
      (await import("/src/lib/terminalRenderer.ts")).webglBackedPaneIds().length === 0, null, { timeout: 15000 });
    await page.waitForFunction(() => document.querySelectorAll(".xterm-rows").length > 0, null, { timeout: 15000 });
    assert.equal(await atlas(page).getAttribute("data-connected"), "true", "The session survives the renderer");
    // The pane keeps working: new output lands in the restored DOM renderer.
    const sessionId = await atlas(page).getAttribute("data-session-id");
    await page.evaluate(id => window.__terminalTest.output(id, "OUTPUT_AFTER_CONTEXT_LOSS\r\n"), sessionId);
    await page.waitForFunction(() =>
      document.querySelector('[data-host-id="atlas"] .xterm-rows')?.textContent.includes("OUTPUT_AFTER_CONTEXT_LOSS"));
  });

  await scenario("gpu-budget-caps-a-heavy-workspace", async page => {
    await openAtlas(page);
    const limit = (await renderer(page)).limit;
    // Every pane of every tab stays mounted, so a workspace can ask for far
    // more GPU contexts than the browser will keep alive.
    for (let opened = 1; opened <= limit + 3; opened++) {
      await page.getByRole("button", { name: "New session", exact: true }).click();
      await page.getByRole("combobox", { name: "Open session in" }).selectOption("tab");
      await page.getByRole("button", { name: "Open local shell", exact: true }).click();
      // Panes of background tabs stay mounted, so wait on the one in front.
      await page.locator('.terminal-pane[data-active="true"][data-connected="true"]').waitFor();
    }
    const state = await renderer(page);
    assert.equal(state.backed.length, limit, `At most ${limit} panes hold a GPU context`);
    // The tab in front is the one that kept its context.
    assert.equal(await page.locator('.terminal-pane[data-active="true"]').count(), 1);
  });
} finally {
  await browser.close();
  await writeFile(`${output}/gpu-renderer-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
