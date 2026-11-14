#!/usr/bin/env node
// Verifies that the Files view shows server-chosen names as they read, not as
// the server wants them to look. The fixture's `sftp_list_dir` is overridden to
// serve names with a right-to-left override and with blank-character padding,
// then the listing, the selection footer and the delete prompt are checked for
// the readable form and captured in a screenshot.
//
//   node scripts/verify/verify-readable-names.mjs [--label after]
//
// Needs the Vite dev server (`cd desktop && npx vite --port 1420`) or
// CLAVYN_URL pointing at one. Screenshots go to `screenshots/`.
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, "../..");
const E2E_DIR = join(REPO_ROOT, "desktop/e2e");
const FIXTURE_PATH = join(E2E_DIR, "tauri-fixture.js");
const OUT_DIR = join(REPO_ROOT, "screenshots");

const require = createRequire(join(E2E_DIR, "package.json"));
const { chromium } = require("playwright");
const URL = process.env.CLAVYN_URL ?? "http://127.0.0.1:1420";
const labelIndex = process.argv.indexOf("--label");
const LABEL = labelIndex > 0 ? process.argv[labelIndex + 1] : "after";

const file = (name, size) => ({
  name,
  long_name: name,
  is_dir: false,
  is_file: true,
  is_symlink: false,
  size,
  modified: 1788600000,
  permissions: 420,
});

// Each entry: what the server serves, and what the user must be shown.
const CASES = [
  ["report\u{202E}fdp.exe", "report_fdp.exe"],
  [`invoice.pdf${"\u{3164}".repeat(60)}.exe`, "invoice.pdf .exe"],
  [`photo.jpg \u{200B} \u{200B} ${"\u{2800}".repeat(60)}.scr`, "photo.jpg .scr"],
  ["README.md", "README.md"],
];

async function main() {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROMIUM_EXECUTABLE_PATH ? { executablePath: process.env.CHROMIUM_EXECUTABLE_PATH } : {}),
  });
  const context = await browser.newContext({ viewport: { width: 1180, height: 560 }, deviceScaleFactor: 2, colorScheme: "light" });
  const page = await context.newPage();
  await page.addInitScript({ path: FIXTURE_PATH });
  await page.addInitScript((entries) => {
    const fixture = window.__TAURI_INTERNALS__;
    const invoke = fixture.invoke.bind(fixture);
    fixture.invoke = async (command, args) => (command === "sftp_list_dir" ? entries : invoke(command, args));
  }, CASES.map(([name], i) => file(name, 1024 * (i + 1))));

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("aside[aria-label='Application navigation']", { timeout: 15000 });
  await page.locator("aside[aria-label='Application navigation'] button", { hasText: "Files" }).first().click();
  await page.locator("select:visible").selectOption({ index: 1 });
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await page.locator("tbody tr").nth(CASES.length - 1).waitFor({ timeout: 15000 });

  const rows = page.locator("tbody tr");
  await rows.nth(0).click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: join(OUT_DIR, `readable-names-${LABEL}.png`) });

  const checks = [];
  const shown = await rows.locator("td:first-child span").allTextContents();
  CASES.forEach(([, expected], i) => {
    checks.push([`row ${i + 1} reads ${JSON.stringify(expected)}`, shown[i] === expected, shown[i]]);
  });
  const footer = await page.locator("span", { hasText: "Selected:" }).last().textContent();
  checks.push(["selection footer reads the first name", footer?.trim() === `Selected: ${CASES[0][1]}`, footer]);

  let prompt = null;
  page.once("dialog", async (dialog) => {
    prompt = dialog.message();
    await dialog.dismiss();
  });
  await rows.nth(0).click({ button: "right" });
  await page.getByRole("button", { name: "Delete", exact: true }).or(page.getByRole("menuitem", { name: "Delete" })).first().click();
  await page.waitForTimeout(300);
  checks.push(["delete prompt names the readable form", prompt === `Delete "${CASES[0][1]}"?`, prompt]);

  let allPass = true;
  for (const [name, pass, actual] of checks) {
    if (!pass) allPass = false;
    console.log(`${pass ? "PASS" : "FAIL"} ${name}${pass ? "" : ` (got ${JSON.stringify(actual)})`}`);
  }
  console.log(`screenshot: screenshots/readable-names-${LABEL}.png`);
  await browser.close();
  process.exit(allPass ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
