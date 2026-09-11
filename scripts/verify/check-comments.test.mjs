#!/usr/bin/env node
// check-comments.test.mjs — regression tests for check-comments.mjs.
//
// Run with: node --test scripts/verify/check-comments.test.mjs
//
// The scanner is line-oriented, and its comment regex ends in `(.*)$`, which
// `\r` does not satisfy. A working tree checked out with CRLF therefore has to
// produce the same verdict as one checked out with LF, or the check reports
// success on files it never actually read. These tests run the real CLI as a
// subprocess so they cover the contract the verification workflow depends on:
// the exit code and the JSON on stdout.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = resolve(join(fileURLToPath(import.meta.url), ".."));
const ROOT = resolve(join(HERE, "..", ".."));
const SCANNER = join(HERE, "check-comments.mjs");

// Two fixtures with byte-identical logical content, differing only in line
// terminator. The violating line is held in a string literal rather than a
// real comment so this file stays clean under its own scanner.
const VIOLATION_LINE = "  // This implements the SSH transport handshake.";
const CLEAN_LINE = "  // Establishes an SSH session over the configured transport.";

function fixtureBody(commentLine, eol) {
  return ["export function connect() {", commentLine, "  return 1;", "}", ""].join(eol);
}

// Runs the scanner over one fixture and returns { code, result }.
function scan(commentLine, eol, name) {
  const dir = mkdtempSync(join(ROOT, ".check-comments-test-"));
  try {
    const file = join(dir, name);
    writeFileSync(file, fixtureBody(commentLine, eol), "utf8");
    let code = 0;
    let stdout;
    try {
      stdout = execFileSync(process.execPath, [SCANNER, file], { encoding: "utf8" });
    } catch (err) {
      code = err.status;
      stdout = err.stdout;
    }
    return { code, result: JSON.parse(stdout) };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const ENCODINGS = [
  ["LF", "\n"],
  ["CRLF", "\r\n"],
];

for (const [label, eol] of ENCODINGS) {
  test(`flags a history comment in a ${label} file`, () => {
    const { code, result } = scan(VIOLATION_LINE, eol, "fixture.ts");
    assert.equal(result.files, 1, `${label}: the fixture should be discovered`);
    assert.equal(result.ok, false, `${label}: the violation should be reported`);
    assert.equal(result.count, 1);
    assert.equal(result.findings[0].pattern, "this-implements");
    assert.equal(code, 1, `${label}: a violation must exit non-zero`);
  });

  test(`accepts a documentation comment in a ${label} file`, () => {
    const { code, result } = scan(CLEAN_LINE, eol, "fixture.ts");
    assert.equal(result.files, 1, `${label}: the fixture should be discovered`);
    assert.equal(result.ok, true, `${label}: no violation should be reported`);
    assert.equal(result.count, 0);
    assert.equal(code, 0);
  });
}

test("CRLF and LF reach the same verdict", () => {
  const lf = scan(VIOLATION_LINE, "\n", "fixture.ts");
  const crlf = scan(VIOLATION_LINE, "\r\n", "fixture.ts");
  assert.equal(crlf.code, lf.code);
  assert.equal(crlf.result.count, lf.result.count);
  assert.equal(crlf.result.findings[0].pattern, lf.result.findings[0].pattern);
  assert.equal(crlf.result.findings[0].line, lf.result.findings[0].line);
  // The reported comment text must not carry a stray carriage return.
  assert.equal(crlf.result.findings[0].comment, lf.result.findings[0].comment);
});
