# Verify Clavyn UI

Drive, inspect, and verify the Clavyn Tauri app's UI with the composable
verification CLI. Run one command per action instead of writing throwaway
scripts.

## Prerequisites

```bash
cd desktop && npm install
cd desktop/e2e && npm install && npx playwright install chromium
cd desktop && npm run dev   # Vite dev server on :1420
```

## Workflow

1. **Check environment:** `node scripts/verify/control-clavyn.mjs doctor --pretty`
2. **Navigate:** `node scripts/verify/control-clavyn.mjs home` or `navigate <view>`
3. **Inspect:** `snapshot --pretty` (a11y tree), `components --pretty`, `info --pretty`
4. **Interact:** `click --name "text"`, `type "<text>"`, `press "<key>`, `new-session`, `connect "<host>"`, `send "<text>"`
5. **Evidence:** `screenshot /tmp/proof.png`, `console --pretty` (must be clean), `network-summary --pretty`
6. **Clean up:** `cleanup`, `reset`, or `stop`

## Feature Map

Read `scripts/verify/features/README.md` before navigating — it catalogs every
view, what it does, how to reach it, and the exact CLI commands. This saves
context tokens.

## Rules

- Never assert "it works" without a `snapshot` or `screenshot` and a clean
  `console` (no errors/warnings) as evidence.
- The fixture mocks SSH/SFTP/vault — this verifies the **renderer**, not
  end-to-end SSH. Core Rust logic is verified with `cargo test -p clavyn-core`.
- Destructive commands (`reset`, `cleanup`, `connect`, `stop`) accept `--dry-run`.
- If a command fails, read the `remedy` field, run `doctor`, and retry.
- Full docs: `scripts/verify/README.md`. Command help: `node scripts/verify/control-clavyn.mjs help`.
