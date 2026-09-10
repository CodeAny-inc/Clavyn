// Real xterm/DOM regressions for dragging whole session tabs: drop onto a pane
// edge to split it, drop on center to swap two terminals across tabs, reorder
// tabs in the strip, and keep live sessions + focus intact through every move.
// SSH is the deterministic, test-only Tauri fixture.
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
const tabStrip = page => page.locator("[data-testid='session-strip'] .session-tab");
const tabButtons = page => page.locator("[data-tab-id]");
const inspect = page => page.evaluate(() => ({
  tabs: [...document.querySelectorAll("[data-tab-id]")].map(el => el.dataset.tabId),
  activeTab: document.querySelector(".session-tab-active [data-tab-id]")?.dataset.tabId ?? null,
  panes: [...document.querySelectorAll(".terminal-pane")].map(p => ({
    host: p.getAttribute("data-host-id"),
    session: p.getAttribute("data-session-id"),
    connected: p.getAttribute("data-connected"),
    active: p.getAttribute("data-active") === "true",
    visible: p.offsetParent !== null,
  })),
  fullscreen: document.querySelector(".terminal-pane.fixed") !== null,
  writes: window.__terminalTest.writes,
  connects: window.__terminalTest.connects,
  closes: window.__terminalTest.closes,
}));
async function connected(page, host) {
  await page.waitForFunction(host => document.querySelector(`[data-host-id="${host}"]`)?.getAttribute("data-connected") === "true", host);
  return pane(page, host).getAttribute("data-session-id");
}
// First host: double-click it in the Hosts list. Later hosts: New session picker.
async function connectHost(page, label, host) {
  await page.getByText(label, { exact: true }).filter({ visible: true }).first().dblclick();
  await connected(page, host);
}
async function newTabHost(page, label, host) {
  await page.getByRole("button", { name: "New session", exact: true }).click();
  await page.getByRole("button", { name: `Connect ${label}`, exact: true }).click();
  await connected(page, host);
}
async function openLocalTab(page) {
  await page.getByRole("button", { name: "New session", exact: true }).click();
  await page.getByRole("button", { name: "Open local shell", exact: true }).click();
  await page.waitForFunction(() => document.querySelector(".terminal-pane:not([data-host-id])")?.dataset.connected === "true");
}
async function action(page, host, label) {
  await pane(page, host).getByRole("button", { name: /^Actions for/ }).click();
  await page.getByRole("menuitem", { name: label, exact: true }).click();
}
async function focusIs(page, host) {
  await page.waitForFunction(host => {
    const el = document.activeElement;
    return el?.closest(".terminal-pane")?.getAttribute("data-host-id") === host &&
      el.classList.contains("xterm-helper-textarea");
  }, host);
}
async function typeCommand(page, text) {
  // Intentionally do NOT focus a locator here: that would hide a routing bug.
  await page.keyboard.type(text);
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => window.__terminalTest.writes.some(write => write.text.includes("\r")));
}
async function remember(page) {
  await page.evaluate(() => {
    window.__movedNodes = Array.from(document.querySelectorAll(".terminal-pane .xterm"));
    window.__terminalTest.writes.length = 0;
  });
}
async function preserved(page, count) {
  assert.ok(await page.evaluate(() => window.__movedNodes.every(node =>
    [...document.querySelectorAll(".terminal-pane .xterm")].includes(node))), "No xterm remount on move");
  const state = await inspect(page);
  assert.equal(state.connects.length, count, "No reconnect on move");
  assert.deepEqual(state.closes, [], "No session closed on move");
}
// Drag a session-strip tab onto a visible pane at a fractional position.
async function dragTab(page, tabIndex, targetHost, at) {
  const target = pane(page, targetHost);
  const box = await target.boundingBox();
  await tabStrip(page).nth(tabIndex).dragTo(target, { targetPosition: { x: box.width * at.x, y: box.height * at.y } });
}
async function scenario(name, exercise) {
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
    await page.addInitScript({ content: "localStorage.setItem('clavyn-settings', JSON.stringify({maskAddresses:false}))" });
    await page.goto(url);
    assert.match(await page.title(), /Clavyn/i, "Correct page identity");
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
try {
  // The core issue: dragging a tab onto the terminal splits it like Termius.
  await scenario("tab-drop-right-splits-pane", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    const atlasSession = await pane(page, "atlas").getAttribute("data-session-id");
    const orionSession = await pane(page, "orion").getAttribute("data-session-id");
    await remember(page);
    await dragTab(page, 1, "atlas", { x: 0.9, y: 0.5 });
    await focusIs(page, "orion");
    const state = await inspect(page);
    assert.equal(state.tabs.length, 1, "Dragged tab merged into the visible tab");
    assert.deepEqual(state.panes.map(p => p.host).sort(), ["atlas", "orion"], "Both terminals share one tab");
    assert.ok(state.panes.every(p => p.connected === "true" && p.visible), "Both panes connected and visible");
    assert.ok(state.panes.some(p => p.host === "orion" && p.active), "Moved pane became active");
    await typeCommand(page, "echo SPLIT_OK");
    const state2 = await inspect(page);
    assert.ok(state2.writes.every(write => write.id === orionSession));
    assert.equal(state2.writes.map(write => write.text).join(""), "echo SPLIT_OK\r");
    assert.deepEqual(state2.panes.map(p => p.session).sort(), [atlasSession, orionSession].sort());
    await preserved(page, 2);
  });
  // Every edge direction splits correctly.
  for (const [name, at] of [["left", { x: 0.08, y: 0.5 }], ["top", { x: 0.5, y: 0.08 }], ["bottom", { x: 0.5, y: 0.92 }]]) {
    await scenario(`tab-drop-${name}-splits-pane`, async page => {
      await connectHost(page, "Atlas Production", "atlas");
      await newTabHost(page, "Orion Staging", "orion");
      await tabButtons(page).nth(0).click();
      await remember(page);
      await dragTab(page, 1, "atlas", at);
      const state = await inspect(page);
      assert.equal(state.tabs.length, 1, "Dragged tab merged into the visible tab");
      assert.equal(state.panes.filter(p => p.visible).length, 2, "Two visible panes");
      assert.ok(state.panes.every(p => p.connected === "true"), "Both still connected");
      await preserved(page, 2);
    });
  }
  // Center drop swaps two terminals across tabs — "switch two terminals".
  await scenario("tab-drop-center-swaps-terminals", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    const atlasSession = await pane(page, "atlas").getAttribute("data-session-id");
    const orionSession = await pane(page, "orion").getAttribute("data-session-id");
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    await remember(page);
    await dragTab(page, 1, "atlas", { x: 0.5, y: 0.5 });
    await focusIs(page, "orion");
    const state = await inspect(page);
    assert.equal(state.tabs.length, 2, "Swap keeps both tabs");
    assert.equal(state.activeTab, state.tabs[0], "First tab stays active");
    const visible = state.panes.filter(p => p.visible);
    assert.equal(visible.length, 1, "Active tab shows one pane");
    assert.equal(visible[0].host, "orion", "Dragged terminal took the visible tab");
    assert.equal(visible[0].session, orionSession, "Orion session intact");
    assert.ok(state.panes.some(p => p.host === "atlas" && p.session === atlasSession), "Atlas session intact in background tab");
    const closeLabels = await page.evaluate(() =>
      [...document.querySelectorAll(".session-tab-close")].map(b => b.getAttribute("aria-label")));
    assert.equal(closeLabels[0], "Close tab Orion Staging", "Swapped-in tab's a11y label names its new terminal");
    assert.equal(closeLabels[1], "Close tab Atlas Production", "Receiving tab's a11y label names its new terminal");
    await typeCommand(page, "echo SWAPPED");
    const state2 = await inspect(page);
    assert.ok(state2.writes.every(write => write.id === orionSession));
    await preserved(page, 2);
  });
  // Reorder tabs by dragging one onto a neighbour's edge in the strip.
  await scenario("tab-drag-reorders-strip", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    await openLocalTab(page);
    const before = (await inspect(page)).tabs;
    assert.equal(before.length, 3, "Three tabs");
    await tabStrip(page).nth(2).dragTo(tabStrip(page).nth(0), { targetPosition: { x: 8, y: 18 } });
    const after = (await inspect(page)).tabs;
    assert.deepEqual(after, [before[2], before[0], before[1]], "Dropped before first tab");
    await tabStrip(page).nth(2).dragTo(tabStrip(page).nth(0), { targetPosition: { x: 90, y: 18 } });
    const last = (await inspect(page)).tabs;
    assert.deepEqual(last, [before[2], before[1], before[0]], "Dropped after first tab");
    assert.deepEqual((await inspect(page)).closes, [], "Reorder never closes a session");
  });
  // The directional drop overlay appears while a tab hovers a pane.
  await scenario("tab-drag-shows-drop-zones", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    await page.evaluate(() => {
      const source = document.querySelectorAll("[data-testid='session-strip'] .session-tab")[1];
      const target = document.querySelector('[data-host-id="atlas"]');
      const dataTransfer = new DataTransfer();
      source.dispatchEvent(new DragEvent("dragstart", { bubbles: true, cancelable: true, dataTransfer, clientX: 0, clientY: 0 }));
      const r = target.getBoundingClientRect();
      target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer,
        clientX: r.left + r.width * 0.9, clientY: r.top + r.height * 0.5 }));
    });
    const zone = page.locator(".drop-zone-right.drop-zone-active");
    await zone.waitFor();
    assert.equal(await zone.locator("span").textContent(), "Split right");
    await page.evaluate(() => {
      const target = document.querySelector('[data-host-id="atlas"]');
      const dataTransfer = new DataTransfer();
      const r = target.getBoundingClientRect();
      target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer,
        clientX: r.left + r.width * 0.5, clientY: r.top + r.height * 0.5 }));
    });
    const center = page.locator(".drop-zone-center.drop-zone-active");
    await center.waitFor();
    assert.equal(await center.locator("span").textContent(), "Swap");
    await page.evaluate(() => {
      const source = document.querySelectorAll("[data-testid='session-strip'] .session-tab")[1];
      const target = document.querySelector('[data-host-id="atlas"]');
      const dataTransfer = new DataTransfer();
      const r = target.getBoundingClientRect();
      const opts = { bubbles: true, cancelable: true, dataTransfer,
        clientX: r.left + r.width * 0.9, clientY: r.top + r.height * 0.5 };
      target.dispatchEvent(new DragEvent("dragover", opts));
      target.dispatchEvent(new DragEvent("drop", opts));
      source.dispatchEvent(new DragEvent("dragend", { bubbles: true, cancelable: true, dataTransfer, clientX: 0, clientY: 0 }));
    });
    const state = await inspect(page);
    assert.equal(state.tabs.length, 1, "Held drag still drops correctly");
    assert.equal(state.panes.filter(p => p.visible).length, 2);
  });
  // A tab dropped onto its own pane changes nothing.
  await scenario("tab-drop-on-own-pane-is-noop", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await openLocalTab(page);
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    await remember(page);
    await dragTab(page, 0, "atlas", { x: 0.9, y: 0.5 });
    const state = await inspect(page);
    assert.equal(state.tabs.length, 2, "No tab removed");
    assert.equal(state.panes.filter(p => p.visible).length, 1, "No split created");
    await preserved(page, 2);
  });
  // A multi-pane tab drops its whole split tree onto an edge.
  await scenario("tab-drop-multi-pane-merge", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    await action(page, "orion", "Split right…");
    await page.getByRole("button", { name: "Open local shell", exact: true }).click();
    await page.waitForFunction(() => document.querySelector(".terminal-pane:not([data-host-id])")?.dataset.connected === "true");
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    await remember(page);
    await dragTab(page, 1, "atlas", { x: 0.9, y: 0.5 });
    const state = await inspect(page);
    assert.equal(state.tabs.length, 1, "Multi-pane tab merged");
    assert.equal(state.panes.length, 3, "All three panes live in one tab");
    assert.ok(state.panes.every(p => p.connected === "true"), "All still connected");
    await preserved(page, 3);
  });
  // Dropping a tab onto a fullscreened pane exits fullscreen to show the split.
  await scenario("tab-drop-exits-fullscreen", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await newTabHost(page, "Orion Staging", "orion");
    await tabButtons(page).nth(0).click();
    await focusIs(page, "atlas");
    await action(page, "atlas", "Maximize pane");
    await page.waitForFunction(() => document.querySelector(".terminal-pane.fixed") !== null);
    // The fullscreen pane is teleported to body; it is still the drop target.
    await page.evaluate(() => {
      const source = document.querySelectorAll("[data-testid='session-strip'] .session-tab")[1];
      const target = document.querySelector(".terminal-pane.fixed");
      const dataTransfer = new DataTransfer();
      const r = target.getBoundingClientRect();
      const opts = { bubbles: true, cancelable: true, dataTransfer,
        clientX: r.left + r.width * 0.9, clientY: r.top + r.height * 0.5 };
      source.dispatchEvent(new DragEvent("dragstart", { ...opts, clientX: 0, clientY: 0 }));
      target.dispatchEvent(new DragEvent("dragover", opts));
      target.dispatchEvent(new DragEvent("drop", opts));
      source.dispatchEvent(new DragEvent("dragend", { ...opts, clientX: 0, clientY: 0 }));
    });
    const state = await inspect(page);
    assert.equal(state.fullscreen, false, "Fullscreen exited to reveal the split");
    assert.equal(state.tabs.length, 1);
    assert.equal(state.panes.filter(p => p.visible).length, 2, "Both panes visible after split");
  });
  // The existing pane-grip drag still swaps panes after the handler refactor.
  await scenario("pane-drag-center-swap-still-works", async page => {
    await connectHost(page, "Atlas Production", "atlas");
    await action(page, "atlas", "Split right…");
    await page.getByRole("button", { name: "Connect Orion Staging", exact: true }).click();
    await connected(page, "orion");
    await remember(page);
    const target = pane(page, "atlas");
    const box = await target.boundingBox();
    await pane(page, "orion").getByLabel("Drag pane Orion Staging", { exact: true })
      .dragTo(target, { targetPosition: { x: box.width * 0.5, y: box.height * 0.5 } });
    const state = await inspect(page);
    assert.equal(state.tabs.length, 1);
    assert.equal(state.panes.length, 2, "Swap keeps both panes");
    assert.ok(state.panes.every(p => p.connected === "true"), "Both still connected");
    await preserved(page, 2);
  });
} finally {
  await browser.close();
  await writeFile(`${output}/tab-drag-results.json`, JSON.stringify(results, null, 2));
}
if (results.some(result => !result.passed)) process.exitCode = 1;
