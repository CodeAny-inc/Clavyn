# Keyboard Shortcuts

Global shortcuts handled in `App.vue`'s `handleKeydown`.

| Shortcut | Action |
|----------|--------|
| `Cmd/Ctrl+K` | Toggle command palette |
| `Cmd/Ctrl+N` | New terminal tab (navigates to Terminal + `tabs.newTab()`) |
| `Cmd/Ctrl+U` | Open update dialog (if an update is available) |
| `Cmd/Ctrl+,` | Go to Settings |
| `Escape` | Exit fullscreen pane (if one is fullscreen) / close palette |

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs press "Control+KeyK"     # open palette
    node scripts/verify/control-clavyn.mjs press "Control+Comma"    # go to settings
    node scripts/verify/control-clavyn.mjs press "Control+KeyN"     # new terminal tab
    node scripts/verify/control-clavyn.mjs press "Escape"           # close palette / exit fullscreen
    node scripts/verify/control-clavyn.mjs info --pretty            # confirm current view

## Gotchas

- Use `Control+` (not `Meta+`) in the harness on Linux/Windows. On macOS the
  app listens for `metaKey`; Playwright `Meta+` works there. The `press`
  command passes the key string straight to Playwright's `keyboard.press`.
- `Cmd/Ctrl+U` only opens the update dialog when `update.available` is true
  (the fixture reports no update by default).
- `Escape` is overloaded: it first exits a fullscreen pane, then closes the
  palette. Press twice if both conditions hold.
