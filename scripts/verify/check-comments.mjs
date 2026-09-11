#!/usr/bin/env node
// check-comments:skip-file
// check-comments.mjs — detect history/attribution comments that should be
// documentation instead. Part of the Clavyn verification workflow.
//
// Comments must document the code as it exists now (what it does, why,
// invariants, gotchas). They must never narrate history ("This implements…",
// "Added…", "Changed from X to Y") or attribute work ("John asked for this",
// "per discussion", "PR #5"). This scanner flags violations and exits
// non-zero so it can gate commits and CI.
//
// Usage:
//   node scripts/verify/check-comments.mjs                 # scan tracked source
//   node scripts/verify/check-comments.mjs --pretty       # human-readable
//   node scripts/verify/check-comments.mjs --staged       # only git-staged files
//   node scripts/verify/check-comments.mjs <path> [path]  # scan specific paths
//   node scripts/verify/check-comments.mjs --help
//
// Output: one JSON object on stdout. Errors include a `remedy` field.

import { execSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(join(fileURLToPath(import.meta.url), "..", "..", ".."));

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------
// Each pattern targets a *structural* signal of history/attribution rather
// than a bare keyword, to avoid flagging legitimate documentation that
// happens to contain words like "added", "changed", or "fixed" when they
// describe current behavior ("after fingerprints are added or removed").
//
// Confidence levels:
//   high   — almost always a violation; fix before committing.
//   medium — usually a violation; review in context before fixing.

const PATTERNS = [
  // --- Attribution to people / requests ---
  {
    id: "attribution-request",
    confidence: "high",
    re: /\b(asked|requested|told|instructed)\s+(to|by|us|you|me|for)\b/i,
    desc: "Attributes work to a person or request",
    remedy: "Delete the attribution. If the surrounding context explains a non-obvious constraint, rewrite it as present-tense documentation of why the code behaves this way.",
  },
  {
    id: "per-discussion",
    confidence: "high",
    re: /\bper\s+(request|discussion|conversation|chat|email|meeting)\b/i,
    desc: "References a discussion or request",
    remedy: "Remove the reference. Document the resulting decision as a present-tense invariant if it is not obvious from the code.",
  },
  {
    id: "as-discussed",
    confidence: "high",
    re: /\bas\s+(discussed|agreed|requested|noted|mentioned|decided|suggested)\b/i,
    desc: "References a prior discussion or agreement",
    remedy: "Remove the reference. Keep only the present-tense rationale that resulted from the discussion.",
  },
  {
    id: "stakeholder-wants",
    confidence: "high",
    re: /\b(client|stakeholder|product|manager|designer|PM|QA|customer|user)\s+(wants|asked|requested|said|needs|wanted|prefers)\b/i,
    desc: "Attributes a requirement to a role or person",
    remedy: "Delete the attribution. Document the requirement as a present-tense constraint the code enforces.",
  },
  {
    id: "requested-by",
    confidence: "high",
    re: /\brequested\s+by\b/i,
    desc: "Explicit 'requested by' attribution",
    remedy: "Delete the attribution. Rewrite any useful rationale in present tense.",
  },

  // --- Change narrative (past-tense story of what was done) ---
  {
    id: "this-implements",
    confidence: "high",
    // "This implements/adds/removes/changes/fixes/introduces/replaces …"
    // describes a completed action, not current behavior. Contrast with
    // "This ensures/checks/guards/returns/handles …" which documents behavior.
    re: /\bThis\s+(implements|adds|removes|changes|fixes|introduces|replaces|deletes|updates|brings|enables)\b/i,
    desc: "Narrates a completed change ('This implements/adds/…')",
    remedy: "Rewrite as documentation of current behavior, or delete. E.g. 'This implements the SSH feature' -> 'Establishes an SSH session over the configured transport.'",
  },
  {
    id: "past-tense-narrative",
    confidence: "high",
    // Sentence starting with a past-tense change verb + a "because/since/so
    // that" rationale: "Changed X to Y because…", "Removed the loop so that…".
    re: /\b(Changed|Replaced|Removed|Added|Renamed|Moved|Refactored|Rewrote|Switched|Updated|Deleted)\s+.+\s+(to|because|since|so that|in order to|as)\b/i,
    desc: "Past-tense change narrative with rationale",
    remedy: "Rewrite as present-tense documentation of the current state and its reason. E.g. 'Changed to agent auth because…' -> 'Uses agent auth because…'.",
  },
  {
    id: "was-changed",
    confidence: "high",
    re: /\b(was|were)\s+(changed|replaced|removed|renamed|moved|refactored|rewritten|switched|updated|deleted)\b/i,
    desc: "Passive-voice change history",
    remedy: "Rewrite in present tense describing current behavior, or delete if it only narrates history.",
  },

  // --- Temporal history ---
  {
    id: "was-previously",
    confidence: "high",
    re: /\b(was|were)\s+previously\b/i,
    desc: "References a previous state",
    remedy: "Delete the historical reference. Document the current state only.",
  },
  {
    id: "used-to-be",
    confidence: "high",
    re: /\bused\s+to\s+be\b/i,
    desc: "References a former state",
    remedy: "Delete the historical reference. Document the current state only.",
  },
  {
    id: "originally",
    confidence: "medium",
    re: /\boriginally\b/i,
    desc: "References an original state",
    remedy: "Usually historical. Delete unless 'originally' describes a documented default that is still meaningful in context.",
  },
  {
    id: "before-this-change",
    confidence: "high",
    re: /\bbefore\s+this\s+(change|refactor|fix|commit|update|PR)\b/i,
    desc: "References a prior version of the code",
    remedy: "Delete the historical reference. Document the current behavior only.",
  },
  {
    id: "after-the-refactor",
    confidence: "medium",
    re: /\bafter\s+the\s+(refactor|change|fix|commit|update|PR|rewrite)\b/i,
    desc: "References a change event",
    remedy: "Delete the event reference. If the comment documents behavior that depends on the current structure, rewrite it to describe that structure directly.",
  },

  // --- External tracking references inside source ---
  {
    id: "tracking-ref",
    confidence: "high",
    // PR/Issue/JIRA/CVE/GH-NN references belong in commit messages and PR
    // descriptions, not source comments. "ticket" is excluded because it is
    // a domain term in this codebase (PTY ticket).
    re: /\b(PR|Issue|JIRA|CVE|GH-\d+|BUG-\d+)\s*#?\d+/i,
    desc: "External tracking reference in a source comment",
    remedy: "Move the reference to the commit message or PR description. Keep only present-tense documentation in the source comment.",
  },
  {
    id: "fixes-issue",
    confidence: "high",
    re: /\b(fixes|fixing|closes|closing|resolves|resolving)\s+#?\d+/i,
    desc: "Links a comment to an issue number",
    remedy: "Move the issue reference to the commit message. Document the current behavior in the comment instead.",
  },

  // --- Workaround / hack narratives ---
  {
    id: "workaround-for",
    confidence: "medium",
    re: /\b(workaround|hack|kludge|temporary fix|quick fix)\s+for\b/i,
    desc: "Describes a workaround for a specific bug",
    remedy: "If the workaround is still needed, document the constraint it satisfies in present tense and why the constraint exists. If it is no longer needed, remove the code and the comment.",
  },
  {
    id: "todo-remove-when",
    confidence: "medium",
    re: /\bTODO:?\s*remove\s+(when|after|once)\b/i,
    desc: "TODO to remove code after an event",
    remedy: "Acceptable only if it names a concrete, checkable condition ('TODO: remove when node 18 is dropped'). If it references a vague future event or a person, rewrite or remove.",
  },
];

// ---------------------------------------------------------------------------
// Comment extraction
// ---------------------------------------------------------------------------
// We extract comment text per language and test each comment line against
// the patterns. A single comment block may produce multiple findings.

const EXT_LANG = {
  ".rs": "rust",
  ".ts": "ts",
  ".tsx": "ts",
  ".vue": "vue",
  ".js": "js",
  ".mjs": "js",
  ".jsx": "js",
};

function commentLineRegex(lang) {
  // Line comments: // ... and (for shell-ish) # ...
  // Block-comment lines: leading * or /* ... */
  if (lang === "rust" || lang === "ts" || lang === "js" || lang === "vue") {
    return /^\s*(\/\/|\/\*|\*)\s?(.*)$/;
  }
  return null;
}

function isBlockCommentDelimiter(line) {
  return /^\s*(\/\*|\*\/|\*[^/])/.test(line) || /^\s*\*\s*$/.test(line);
}

function extractCommentContent(rawLine, lang) {
  const re = commentLineRegex(lang);
  if (!re) return null;
  const m = rawLine.match(re);
  if (!m) return null;
  let marker = m[1];
  let text = m[2] || "";
  // Strip trailing */
  text = text.replace(/\*\/\s*$/, "").trim();
  return { marker, text };
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

const IGNORE_DIRS = new Set([
  "node_modules",
  "target",
  "dist",
  ".git",
  ".cache",
  "coverage",
  ".turbo",
  "playwright-report",
  "test-results",
]);

function walk(dir, out) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  for (const name of entries) {
    if (IGNORE_DIRS.has(name)) continue;
    const full = join(dir, name);
    let st;
    try {
      st = statSync(full);
    } catch {
      continue;
    }
    if (st.isDirectory()) {
      walk(full, out);
    } else if (st.isFile()) {
      out.push(full);
    }
  }
}

function discoverFiles(paths) {
  const files = [];
  for (const p of paths) {
    let st;
    try {
      st = statSync(p);
    } catch {
      continue;
    }
    if (st.isDirectory()) {
      walk(p, files);
    } else {
      files.push(p);
    }
  }
  return files
    .map((f) => relative(ROOT, f))
    .filter((f) => {
      const dot = f.lastIndexOf(".");
      if (dot === -1) return false;
      return EXT_LANG[f.slice(dot)] !== undefined;
    });
}

function gitStagedFiles() {
  try {
    const out = execSync("git diff --cached --name-only --diff-filter=ACM", {
      cwd: ROOT,
      encoding: "utf8",
      stdio: ["pipe", "pipe", "pipe"],
    });
    return out
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean)
      .filter((f) => {
        const dot = f.lastIndexOf(".");
        if (dot === -1) return false;
        return EXT_LANG[f.slice(dot)] !== undefined;
      });
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

function scanFile(relPath) {
  const dot = relPath.lastIndexOf(".");
  const lang = EXT_LANG[relPath.slice(dot)];
  if (!lang) return [];

  let src;
  try {
    src = readFileSync(join(ROOT, relPath), "utf8");
  } catch {
    return [];
  }

  // File-level skip: a file containing "check-comments:skip-file" anywhere is
  // excluded entirely. Use for tooling that documents the forbidden patterns
  // by example (like this scanner) or generated code.
  if (/check-comments:skip-file/.test(src)) return [];

  // Split on either terminator. A CRLF checkout leaves a trailing \r on every
  // line, and the comment regex ends in `(.*)$` — `.` does not match \r, so a
  // \n-only split makes every comment line unmatchable and the scan silently
  // reports zero findings.
  const lines = src.split(/\r?\n/);
  const findings = [];

  for (let i = 0; i < lines.length; i++) {
    // Line-level allow: append "check-comments:allow" to suppress findings on
    // this line. Use sparingly for legitimate documentation that quotes a
    // forbidden pattern as an example.
    if (/check-comments:allow/.test(lines[i])) continue;

    const extracted = extractCommentContent(lines[i], lang);
    if (!extracted) continue;
    const text = extracted.text;
    if (!text) continue;

    for (const pat of PATTERNS) {
      const m = text.match(pat.re);
      if (!m) continue;
      findings.push({
        file: relPath,
        line: i + 1,
        comment: lines[i].trim(),
        match: m[0],
        pattern: pat.id,
        confidence: pat.confidence,
        desc: pat.desc,
        remedy: pat.remedy,
      });
      // One finding per line per pattern; don't double-report the same line
      // for overlapping patterns of the same id.
      break;
    }
  }

  return findings;
}

function scanAll(files) {
  const all = [];
  for (const f of files) {
    all.push(...scanFile(f));
  }
  // Sort: high confidence first, then by file/line.
  all.sort((a, b) => {
    if (a.confidence !== b.confidence) {
      return a.confidence === "high" ? -1 : 1;
    }
    if (a.file !== b.file) return a.file < b.file ? -1 : 1;
    return a.line - b.line;
  });
  return all;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function printHelp() {
  process.stdout.write(
    [
      "check-comments.mjs — detect history/attribution comments in Clavyn source",
      "",
      "Usage:",
      "  node scripts/verify/check-comments.mjs                 scan tracked source",
      "  node scripts/verify/check-comments.mjs --pretty         human-readable output",
      "  node scripts/verify/check-comments.mjs --staged         only git-staged files",
      "  node scripts/verify/check-comments.mjs <path> [path…]   scan specific paths",
      "  node scripts/verify/check-comments.mjs --help           show this help",
      "",
      "Comments must document the code as it exists now. They must never narrate",
      "history ('This implements…', 'Added…', 'Changed from X to Y') or attribute",
      "work ('John asked for this', 'per discussion', 'PR #5').",
      "",
      "Exit codes: 0 = clean, 1 = violations found, 2 = usage error.",
    ].join("\n") + "\n",
  );
}

function main(argv) {
  const args = argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    printHelp();
    process.exit(0);
  }

  const pretty = args.includes("--pretty");
  const staged = args.includes("--staged");
  const positional = args.filter((a) => !a.startsWith("--"));

  let files;
  if (staged && positional.length === 0) {
    files = gitStagedFiles();
    if (files.length === 0) {
      const result = { ok: true, message: "No staged source files to scan.", findings: [], count: 0 };
      process.stdout.write((pretty ? JSON.stringify(result, null, 2) : JSON.stringify(result)) + "\n");
      process.exit(0);
    }
  } else if (positional.length > 0) {
    files = discoverFiles(positional.map((p) => resolve(ROOT, p)));
  } else {
    // Default: scan the whole tracked source tree.
    files = discoverFiles([
      join(ROOT, "core"),
      join(ROOT, "desktop", "src"),
      join(ROOT, "desktop", "src-tauri", "src"),
      join(ROOT, "scripts", "verify"),
    ]);
  }

  const findings = scanAll(files);

  const result = {
    ok: findings.length === 0,
    count: findings.length,
    files: files.length,
    findings,
  };

  if (!result.ok) {
    result.message = `${findings.length} history/attribution comment(s) found. Fix them before committing.`;
    result.remedy =
      "Rewrite each comment as present-tense documentation of current behavior, or delete it. See AGENTS.md 'Comment hygiene (enforced)'.";
  }

  process.stdout.write((pretty ? JSON.stringify(result, null, 2) : JSON.stringify(result)) + "\n");
  process.exit(result.ok ? 0 : 1);
}

main(process.argv);
