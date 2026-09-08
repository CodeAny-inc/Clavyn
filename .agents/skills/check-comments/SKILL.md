---
name: check-comments
description: Scan Clavyn source for history/attribution comments that should be documentation instead. Use after writing or editing code to catch comments that narrate history ("This implements…", "Added…", "Changed from X to Y") or attribute work ("John asked for this", "per discussion", "PR #5") and fix them before committing. Activates on prompts about comment hygiene, code comments, history comments, or pre-commit checks.
license: Proprietary. LICENSE.txt has complete terms
compatibility: Requires Node.js. Designed for any Agent Skills-compatible agent (VS Code, Copilot, Claude Code, Cursor, Codex, Gemini CLI, Goose, OpenHands, Devin, etc.).
metadata:
  author: CodeAny-inc
  version: "1.0"
  repository: https://github.com/CodeAny-inc/Clavyn
allowed-tools: Bash(node:*) Bash(npm:*) Bash(npx:*) Read Grep Glob Edit
---

# check-comments — comment hygiene verification skill for Clavyn

Code comments must document the code as it exists now — what it does, why it
behaves this way, and any non-obvious invariants or gotchas. Comments must
**never** narrate history or attribute work. This skill enforces that rule by
scanning source files and reporting violations with a suggested fix.

## The rule

A comment is a violation if it reads like a changelog entry, a request log,
or a story about how the code got here.

**Forbidden (history / attribution):**
- Narrating a completed change: "This implements the SSH feature",
  "Added biometric support", "Changed from X to Y because…", "Removed the
  old loop", "Replaced the mock with a real call".
- Attribution to people or requests: "John asked for this", "requested by
  the client", "per discussion with the PM", "as discussed", "stakeholder
  wants", "was asked to".
- Temporal history: "was previously", "used to be", "originally", "before
  this change", "after the refactor".
- External tracking refs inside code: "PR #5", "issue #12", "JIRA-123",
  "fixes #8", "CVE-2024-xxxx". (Keep these in commit messages and PR
  descriptions, not in source comments.)
- Workaround/hack narratives: "workaround for bug X", "hack to work around",
  "TODO: remove when Y ships".

**Required (documentation):**
- Explain intent and invariants in present tense.
- Document non-obvious behavior and why, not how the code got there.

## The scanner

    node scripts/verify/check-comments.mjs                 # scan tracked source
    node scripts/verify/check-comments.mjs --pretty         # human-readable
    node scripts/verify/check-comments.mjs --staged         # only git-staged files
    node scripts/verify/check-comments.mjs <path> [path…]   # scan specific paths
    node scripts/verify/check-comments.mjs --help

Scans `.rs`, `.ts`, `.tsx`, `.vue`, `.js`, `.mjs` files under `core/`,
`desktop/src/`, `desktop/src-tauri/src/`, and `scripts/verify/`. Prints one
JSON object on stdout; `--pretty` for humans. Exits `0` if clean, `1` if
violations found, `2` on usage error. Each finding includes a `remedy` field
telling you how to fix it.

## How to use this skill

When you have written or edited code in Clavyn, work in this order:

1. **Run the scanner.** `node scripts/verify/check-comments.mjs --pretty`
   (or `--staged` to check only what you are about to commit).
2. **Read each finding.** It shows the file, line, the comment, the matched
   pattern, the confidence (`high` or `medium`), and a `remedy`.
3. **Fix every finding.** Rewrite the comment as present-tense documentation
   of current behavior, or delete it. Do not leave history comments in the
   codebase — fix them before considering the task done.
4. **Re-run to confirm clean.** `node scripts/verify/check-comments.mjs`
   must exit `0` before committing.

### Fixing violations

For each finding, choose one:

- **Rewrite as documentation** (preferred when the comment captures a real
  constraint): turn the history into a present-tense statement of why the
  code behaves this way.
  - Bad: `// Changed to agent auth because the client requested it.`
  - Good: `// Uses agent auth because credentials are never stored on disk.`
- **Delete** (when the comment only narrates history with no lasting
  insight): remove it entirely.
- **Move tracking refs** to the commit message or PR description; keep only
  present-tense documentation in the source.

### Escape hatches (use sparingly)

- **Line-level:** append `// check-comments:allow` to suppress findings on a
  line that legitimately quotes a forbidden pattern as an example.
- **File-level:** add `// check-comments:skip-file` anywhere in a file to
  exclude it entirely (e.g. tooling that documents the patterns by example).

These are for rare, legitimate exceptions. Do not use them to silence real
violations.

## Gotchas

- The scanner targets *structural* signals of history/attribution, not bare
  keywords, so it won't flag "after fingerprints are added or removed" (which
  documents current behavior). But always use judgment when reviewing
  `medium`-confidence findings.
- "ticket" is a domain term in this codebase (PTY ticket), so it is not
  flagged. "PR #5", "Issue #12", "JIRA-123" are flagged.
- The scanner excludes `node_modules/`, `target/`, `dist/`, `.git/`, etc.
- Run `--staged` as a pre-commit gate to catch violations before they land.

## Rules

- Never commit with `check-comments` findings unresolved. Fix or delete
  every violating comment.
- Tracking references (PR/Issue/JIRA/CVE numbers) belong in commit messages
  and PR descriptions, never in source comments.
- See `AGENTS.md` "Comment hygiene (enforced)" for the full policy.
- Full CLI docs: `node scripts/verify/check-comments.mjs --help`.
