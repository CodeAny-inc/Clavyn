# Keyboard Shortcuts

Global shortcuts handled in `App.vue`'s `handleKeydown`.

| Shortcut | Action |
|----------|--------|
| `Cmd/Ctrl+K` | Toggle command palette |
| `Cmd/Ctrl+N` | New terminal tab (navigates to Terminal + `tabs.newTab()`) |
| `Cmd/Ctrl+U` | Open update dialog (if an update is available) |
| `Cmd/Ctrl+,` | Go to Settings |
| `Escape` | Exit fullscreen pane (if one is fullscreen) / close palette |

Shortcuts that also work while a terminal has focus (`lib/shortcuts.ts`). On
Linux and Windows plain `Ctrl+<key>` belongs to the shell (`Ctrl+F`, `Ctrl+K`,
`Ctrl+Left/Right`), so these use `Ctrl+Shift`; macOS uses `Cmd`.

| macOS | Linux / Windows | Action |
|-------|-----------------|--------|
| `Cmd+F` | `Ctrl+Shift+F` | Find in the terminal |
| `Cmd+Arrow` | `Ctrl+Shift+Arrow` | Move to the neighbouring pane |
| `Cmd+K` | `Ctrl+Shift+K` | Command palette from inside a terminal |

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs press "Control+KeyK"     # open palette
    node scripts/verify/control-clavyn.mjs press "Control+Comma"    # go to settings
    node scripts/verify/control-clavyn.mjs press "Control+KeyN"     # new terminal tab
    node scripts/verify/control-clavyn.mjs press "Escape"           # close palette / exit fullscreen
    node scripts/verify/control-clavyn.mjs info --pretty            # confirm current view

## Gotchas

- The harness browser reports a Linux platform, so terminal shortcuts are
  `Control+Shift+...` there (e.g. `Control+Shift+ArrowLeft`).
- Use `Control+` (not `Meta+`) in the harness on Linux/Windows. On macOS the
  app listens for `metaKey`; Playwright `Meta+` works there. The `press`
  command passes the key string straight to Playwright's `keyboard.press`.
- `Cmd/Ctrl+U` only opens the update dialog when `update.available` is true
  (the fixture reports no update by default).
- `Escape` is overloaded: it first exits a fullscreen pane, then closes the
  palette. Press twice if both conditions hold.
