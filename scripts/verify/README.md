# Clavyn Verification Tools

Agent-friendly CLIs for verifying the Clavyn Tauri app. Any AI agent (Devin,
Claude Code, Cursor, Codex, Copilot, ...) can use them — run one command per
action instead of writing throwaway scripts.

- **`control-clavyn.mjs`** — drives the live Tauri renderer over CDP
  (Playwright) to navigate, inspect, interact, and capture evidence.
- **`check-comments.mjs`** — scans source for history/attribution comments
  that should be documentation instead. Gates commits.

## Quick start

```bash
# Prerequisites (once per machine)
cd desktop && npm install
cd desktop/e2e && npm install && npx playwright install chromium
# ...or set CHROMIUM_EXECUTABLE_PATH to a system Chrome/Chromium

# Start the Vite dev server (the harness also auto-starts it if needed)
cd desktop && npm run dev   # serves http://127.0.0.1:1420

# Check your environment
node scripts/verify/control-clavyn.mjs doctor --pretty

# Drive the app
node scripts/verify/control-clavyn.mjs home
node scripts/verify/control-clavyn.mjs navigate terminal
node scripts/verify/control-clavyn.mjs snapshot --pretty
node scripts/verify/control-clavyn.mjs screenshot /tmp/proof.png
node scripts/verify/control-clavyn.mjs stop
```

## How it works

The CLI holds a persistent **harness server** (a Node process with a Chromium
browser + page) so state survives across commands — `navigate` then `snapshot`
then `screenshot` all share one session. It auto-starts on the first command
and stays alive (a `.session.json` file tracks the port).

The renderer is loaded from the Vite dev server with `desktop/e2e/tauri-fixture.js`
injected before any app script, so the full Tauri IPC surface (SSH, SFTP,
vault, keys, updates) is mocked and deterministic — no network, no real
credentials.

Every command prints one JSON object on stdout (machine-readable). Use
`--pretty` for indented human output. Errors include a `remedy` field telling
you what to do next. Destructive commands accept `--dry-run`.

## Commands

| Category | Commands |
|----------|----------|
| Health & lifecycle | `doctor`, `serve`, `stop`, `status` |
| Inspection | `info`, `snapshot`, `screenshot`, `components`, `eval`, `console`, `network-log`, `network-summary` |
| Navigation | `home`, `navigate <view>`, `open-command-palette`, `close-command-palette`, `scroll` |
| Interaction | `click`, `click-xy`, `type`, `press` |
| Terminal | `new-session`, `connect <host>`, `send <text>`, `wait-settle` |
| Fixture control | `fixture state`, `fixture hold-next`, `fixture fail-next`, `fixture release` |
| Cleanup | `cleanup`, `reset` |

Run `node scripts/verify/control-clavyn.mjs help` for full usage.

## Comment hygiene — `check-comments.mjs`

Scans source (`.rs`, `.ts`, `.tsx`, `.vue`, `.js`, `.mjs`) for comments that
narrate history or attribute work instead of documenting the code. Comments
must describe the code as it exists now — never "This implements…", "Added…",
"Changed from X to Y", "John asked for this", "per discussion", "PR #5".

```bash
node scripts/verify/check-comments.mjs                 # scan tracked source
node scripts/verify/check-comments.mjs --pretty         # human-readable
node scripts/verify/check-comments.mjs --staged         # only git-staged files
node scripts/verify/check-comments.mjs <path> [path…]   # scan specific paths
```

Exits `0` if clean, `1` if violations found. Each finding includes a `remedy`
field. Fix or delete every violating comment before committing. See
`AGENTS.md` "Comment hygiene (enforced)" for the full policy.

Escape hatches (use sparingly): append `// check-comments:allow` to suppress
a line, or add `// check-comments:skip-file` to exclude a file (e.g. tooling
that documents the patterns by example).

## Feature Map

Before navigating, read [`features/README.md`](features/README.md) — it
catalogs every major feature, what it does, how to reach it from the UI, and
the exact CLI commands to drive it. Use it to save context tokens instead of
re-deriving how the app is laid out each time.

## Agent integration

These CLIs are agent-agnostic. The canonical Agent Skills (following the
[agentskills.io](https://agentskills.io) standard) live in `.agents/skills/`
— the cross-agent entry point supported by VS Code, Copilot, Claude Code,
Cursor, Codex, Gemini CLI, Goose, OpenHands, and more. Agent-specific pointer
files reference the canonical skills:

| Skill | Canonical | Agent pointers |
|-------|-----------|----------------|
| verify-clavyn (UI) | `.agents/skills/verify-clavyn/SKILL.md` | `AGENTS.md`, `.devin/skills/`, `.claude/commands/`, `.cursor/rules/` |
| check-comments (comment hygiene) | `.agents/skills/check-comments/SKILL.md` | `AGENTS.md`, `.devin/skills/`, `.claude/commands/`, `.cursor/rules/` |

| Agent | Pointer file convention |
|-------|--------------------------|
| **All agentskills.io-compatible** | `.agents/skills/<name>/SKILL.md` (standard) |
| Universal / Codex | `AGENTS.md` (root) |
| Devin | `.devin/skills/<name>/SKILL.md` |
| Claude Code | `.claude/commands/<name>.md` |
| Cursor | `.cursor/rules/<name>.mdc` |

## Driving a real Tauri build

The harness defaults to the Vite dev server + fixture mock (verifies the
**renderer**). To drive a real `tauri dev`/packaged webview, set `CLAVYN_URL`
and `CHROMIUM_EXECUTABLE_PATH` to point at its webview endpoint. Core Rust
logic is verified separately with `cargo test -p clavyn-core`.

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `CLAVYN_URL` | `http://127.0.0.1:1420` | Renderer URL |
| `CHROMIUM_EXECUTABLE_PATH` | playwright-bundled | Use a specific Chrome/Chromium |
| `CLAVYN_HEADLESS` | `1` | Set to `0` to show the browser |
| `CLAVYN_SLOW_MO` | `0` | Slow down actions by N ms |
| `CLAVYN_AUTO_START_DEV` | unset (auto) | Set to `0` to disable Vite auto-start |
| `CLAVYN_VIEWPORT_W` / `CLAVYN_VIEWPORT_H` | `1440` / `900` | Browser viewport |
