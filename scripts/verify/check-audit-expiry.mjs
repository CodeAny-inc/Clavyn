#!/usr/bin/env node
// check-audit-expiry.mjs — enforce a review date on every `cargo audit`
// suppression in .cargo/audit.toml.
//
// cargo audit has no notion of an expiring ignore, so a suppression added for
// a temporary reason stays silent forever. This scanner requires each entry in
// the `ignore` list to carry a `# review-by: YYYY-MM-DD` comment in the block
// directly above it, and fails once that date is reached. The audit job then
// forces a fresh decision instead of letting the list ossify.
//
// Usage:
//   node scripts/verify/check-audit-expiry.mjs            # check .cargo/audit.toml
//   node scripts/verify/check-audit-expiry.mjs --pretty   # human-readable
//   node scripts/verify/check-audit-expiry.mjs <path>     # check a specific file
//   node scripts/verify/check-audit-expiry.mjs --help
//
// Output: one JSON object on stdout. Errors include a `remedy` field.
// Exit codes: 0 = every suppression is dated and current, 1 = at least one is
// undated or due, 2 = usage error.

import { readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(join(fileURLToPath(import.meta.url), "..", "..", ".."));
const DEFAULT_CONFIG = join(ROOT, ".cargo", "audit.toml");

const REVIEW_BY_RE = /#\s*review-by:\s*(\S+)/i;
const ISO_DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const ADVISORY_RE = /"([^"]+)"/;

// Compare dates as UTC midnights so the result does not depend on the runner's
// timezone. A Windows dev machine and a Linux CI runner must agree.
function utcMidnight(year, month, day) {
  return Date.UTC(year, month - 1, day);
}

function today() {
  const now = new Date();
  return utcMidnight(now.getUTCFullYear(), now.getUTCMonth() + 1, now.getUTCDate());
}

function parseIsoDate(text) {
  const m = text.match(ISO_DATE_RE);
  if (!m) return null;
  const [, y, mo, d] = m.map(Number);
  const stamp = utcMidnight(y, mo, d);
  // Reject impossible calendar dates such as 2026-02-31, which Date.UTC would
  // otherwise roll forward into March.
  const back = new Date(stamp);
  if (
    back.getUTCFullYear() !== y ||
    back.getUTCMonth() + 1 !== mo ||
    back.getUTCDate() !== d
  ) {
    return null;
  }
  return stamp;
}

function isoString(stamp) {
  return new Date(stamp).toISOString().slice(0, 10);
}

// Extract the `ignore = [ … ]` array from an audit.toml, returning one record
// per advisory with the comment lines that precede it inside the array.
function parseIgnoreEntries(src) {
  const lines = src.split("\n");
  const start = lines.findIndex((l) => /^\s*ignore\s*=\s*\[/.test(l));
  if (start === -1) return null;

  const entries = [];
  let comments = [];

  for (let i = start + 1; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*\]/.test(line)) return entries;

    const trimmed = line.trim();
    if (trimmed === "") continue;

    if (trimmed.startsWith("#")) {
      comments.push(trimmed);
      continue;
    }

    const m = trimmed.match(ADVISORY_RE);
    if (m) {
      entries.push({ advisory: m[1], line: i + 1, comments });
      comments = [];
    }
  }

  // No closing bracket found.
  return null;
}

function checkEntry(entry, now) {
  const dated = entry.comments
    .map((c) => c.match(REVIEW_BY_RE))
    .filter(Boolean);

  if (dated.length === 0) {
    return {
      advisory: entry.advisory,
      line: entry.line,
      status: "undated",
      desc: "Suppression carries no review date",
      remedy:
        "Add a '# review-by: YYYY-MM-DD' comment above this advisory, on a date by which the suppression must be re-justified or removed.",
    };
  }

  const raw = dated[dated.length - 1][1];
  const stamp = parseIsoDate(raw);
  if (stamp === null) {
    return {
      advisory: entry.advisory,
      line: entry.line,
      status: "malformed",
      reviewBy: raw,
      desc: "Review date is not a valid YYYY-MM-DD calendar date",
      remedy: "Rewrite the '# review-by:' value as a real date in YYYY-MM-DD form.",
    };
  }

  if (stamp <= now) {
    return {
      advisory: entry.advisory,
      line: entry.line,
      status: "due",
      reviewBy: isoString(stamp),
      desc: "Review date has been reached",
      remedy:
        "Re-check the advisory: drop the suppression if a fixed version is now reachable, otherwise confirm the justification still holds and push the review date out.",
    };
  }

  return {
    advisory: entry.advisory,
    line: entry.line,
    status: "ok",
    reviewBy: isoString(stamp),
  };
}

function printHelp() {
  process.stdout.write(
    [
      "check-audit-expiry.mjs — require a review date on every cargo audit suppression",
      "",
      "Usage:",
      "  node scripts/verify/check-audit-expiry.mjs             check .cargo/audit.toml",
      "  node scripts/verify/check-audit-expiry.mjs --pretty    human-readable output",
      "  node scripts/verify/check-audit-expiry.mjs <path>      check a specific file",
      "  node scripts/verify/check-audit-expiry.mjs --help      show this help",
      "",
      "Each advisory in the 'ignore' list needs a '# review-by: YYYY-MM-DD' comment",
      "above it. The check fails when a date is missing, malformed, or reached.",
      "",
      "Exit codes: 0 = clean, 1 = suppressions undated or due, 2 = usage error.",
    ].join("\n") + "\n",
  );
}

function emit(result, pretty) {
  process.stdout.write(
    (pretty ? JSON.stringify(result, null, 2) : JSON.stringify(result)) + "\n",
  );
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    printHelp();
    process.exit(0);
  }

  const pretty = args.includes("--pretty");
  const positional = args.filter((a) => !a.startsWith("--"));
  const configPath = positional.length > 0 ? resolve(ROOT, positional[0]) : DEFAULT_CONFIG;
  const shown = relative(ROOT, configPath).split("\\").join("/");

  let src;
  try {
    src = readFileSync(configPath, "utf8");
  } catch (err) {
    emit(
      {
        ok: false,
        file: shown,
        message: `Cannot read ${shown}: ${err.message}`,
        remedy: "Point the check at an existing cargo audit config, or restore the file.",
      },
      pretty,
    );
    process.exit(2);
  }

  const entries = parseIgnoreEntries(src);
  if (entries === null) {
    emit(
      {
        ok: false,
        file: shown,
        message: `No closed 'ignore = [ … ]' array found in ${shown}.`,
        remedy:
          "Keep the suppression list as a multi-line 'ignore = [' array so each entry can carry its own review date.",
      },
      pretty,
    );
    process.exit(2);
  }

  const now = today();
  const checked = entries.map((e) => checkEntry(e, now));
  const problems = checked.filter((c) => c.status !== "ok");

  const result = {
    ok: problems.length === 0,
    file: shown,
    checked: new Date(now).toISOString().slice(0, 10),
    count: checked.length,
    entries: checked,
  };

  if (!result.ok) {
    result.message = `${problems.length} of ${checked.length} cargo audit suppression(s) need attention.`;
    result.remedy =
      "Give every entry a future '# review-by: YYYY-MM-DD' comment, or remove the suppression.";
  }

  emit(result, pretty);
  process.exit(result.ok ? 0 : 1);
}

main(process.argv);
