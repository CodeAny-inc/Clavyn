# Clavyn UI Verification CLI

A composable, agent-friendly CLI for driving and verifying the Clavyn Tauri
app's webview via the Chrome DevTools Protocol (through Playwright). Any AI
agent (Devin, Claude Code, Cursor, Codex, Copilot, ...) can use it — run one
command per action instead of writing throwaway scripts.

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

## Feature Map

Before navigating, read [`features/README.md`](features/README.md) — it
catalogs every major feature, what it does, how to reach it from the UI, and
the exact CLI commands to drive it. Use it to save context tokens instead of
re-deriving how the app is laid out each time.

## Agent integration

This CLI is agent-agnostic. The canonical Agent Skill (following the
[agentskills.io](https://agentskills.io) standard) lives at
`.agents/skills/verify-clavyn/SKILL.md` — this is the cross-agent entry point
supported by VS Code, Copilot, Claude Code, Cursor, Codex, Gemini CLI, Goose,
OpenHands, and more. Agent-specific pointer files reference that canonical
skill:

| Agent | Pointer file | Convention |
|-------|-------------|------------|
| **All agentskills.io-compatible** | `.agents/skills/verify-clavyn/SKILL.md` | Standard Agent Skills (agentskills.io) |
| Universal / Codex | `AGENTS.md` (root) | De facto standard |
| Devin | `.devin/skills/verify-clavyn/SKILL.md` | Devin skills |
| Claude Code | `.claude/commands/verify-clavyn.md` | Slash commands |
| Cursor | `.cursor/rules/verify-clavyn.mdc` | Rules |

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
