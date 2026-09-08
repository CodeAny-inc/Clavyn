---
name: verify-clavyn
description: Drive, inspect, and verify the Clavyn Tauri app's UI with a composable CLI over CDP/Playwright. Use after any frontend change to confirm behavior, capture evidence, and navigate features.
argument-hint: "[what to verify]"
allowed-tools:
  - read
  - grep
  - glob
  - exec
---

# verify-clavyn — UI verification skill for Clavyn

You verify Clavyn's Tauri renderer with a composable CLI that drives the live
app over the Chrome DevTools Protocol (via Playwright). The canonical CLI and
docs live in `scripts/verify/` (agent-agnostic). This skill is the Devin
pointer; Claude Code and Cursor have equivalent pointers in
`.claude/commands/` and `.cursor/rules/`.

## The CLI

    node scripts/verify/control-clavyn.mjs <command> [options]

It holds a persistent Chromium + page (the "harness") so state survives across
commands — `navigate` then `snapshot` then `screenshot` all share one session.
The renderer is loaded from the Vite dev server with `tauri-fixture.js` mocking
the full Tauri IPC surface, so SSH/SFTP/vault calls are deterministic and never
touch the network. Every command prints one JSON object on stdout; use
`--pretty` for humans. Errors include a `remedy` field telling you what to do.

### Prerequisites (run once per machine)

    cd desktop && npm install                       # frontend deps + vite
    cd desktop/e2e && npm install                    # playwright (pinned 1.56.1)
    npx playwright install chromium                 # or set CHROMIUM_EXECUTABLE_PATH
    cd desktop && npm run dev                       # serves http://127.0.0.1:1420

### Always start here

    node scripts/verify/control-clavyn.mjs doctor --pretty

`doctor` checks node, playwright, the browser binary, the dev server, the
fixture, and the harness. Fix anything it flags before continuing.

## How to use this skill

When asked to verify a change in Clavyn, work in this order:

1. **Bring up the app.** Ensure `npm run dev` is running (or let the harness
   auto-start it). Run `doctor`.
2. **Reproduce / navigate.** Use `home`, `navigate <view>`,
   `open-command-palette`, or keyboard `press`. See the Feature Map
   (`scripts/verify/features/README.md`) for how to reach any feature.
3. **Inspect.** `snapshot` (a11y tree), `components` (data-*/roles/labels),
   `info` (app version + current view), `console` (errors/warnings),
   `network-summary` (which Tauri commands were invoked).
4. **Interact.** `click --name "text"`, `type`, `press`, `new-session`,
   `connect "<host label>"`, `send "<text>"`.
5. **Capture evidence.** `screenshot /tmp/proof.png` and `snapshot` for the
   record. `console` and `network-log` prove no errors / correct IPC.
6. **Clean up.** `cleanup` (disconnect fixture sessions) or `reset` (reload).
   `stop` tears down the harness when fully done.

### Composing commands (the point of the lever)

    # Verify the Hosts view loads and lists fixture hosts with no console errors
    node scripts/verify/control-clavyn.mjs home
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs console --pretty

    # Verify a terminal session can be opened and echo round-trips
    node scripts/verify/control-clavyn.mjs navigate terminal
    node scripts/verify/control-clavyn.mjs new-session
    node scripts/verify/control-clavyn.mjs send "echo ok"
    node scripts/verify/control-clavyn.mjs fixture state --pretty
    node scripts/verify/control-clavyn.mjs screenshot /tmp/term.png

    # Verify SSH connect path against a fixture host
    node scripts/verify/control-clavyn.mjs home
    node scripts/verify/control-clavyn.mjs connect "Atlas Production"
    node scripts/verify/control-clavyn.mjs network-summary --pretty

## Feature Map

Before navigating, read `scripts/verify/features/README.md` — it catalogs
every major feature, what it does, how to reach it from the UI, and the exact
CLI commands to drive it. Use it to save context tokens instead of re-deriving
how the app is laid out each time.

## Rules

- Never assert "it works" without a `snapshot` or `screenshot` and a clean
  `console` (no errors/warnings) as evidence.
- The fixture mocks SSH/SFTP/vault — these prove the **renderer** behaves
  correctly. Core Rust logic is verified separately with `cargo test -p
  clavyn-core`. Don't claim end-to-end SSH was tested via this CLI.
- Destructive commands (`reset`, `cleanup`, `connect`, `stop`) accept
  `--dry-run`. Use it when unsure.
- If a command fails, read the `remedy` field, run `doctor`, and retry. Run
  `stop` + retry if the harness is wedged.
- Full docs: `scripts/verify/README.md`. Command help:
  `node scripts/verify/control-clavyn.mjs help`.
