# Command Palette

A quick-jump overlay (`Cmd/Ctrl+K`) listing navigation + action commands.
Filtered by typed query; arrow keys + Enter to select.

## Sub-features

- palette-open: `Cmd/Ctrl+K` toggles; Escape closes.
- palette-search: filters commands by label (case-insensitive substring).
- palette-commands: "Go to <view>" for every view, "New Terminal Tab",
  "Check for Updates", and (when an update exists) "Update to v<x>".

## How to get to it (user POV)

Press `Cmd/Ctrl+K` anywhere. Type to filter; arrow keys to move; Enter to run.
Escape or clicking away closes it.

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs open-command-palette
    node scripts/verify/control-clavyn.mjs snapshot --pretty        # dialog with command list
    node scripts/verify/control-clavyn.mjs type "term"
    node scripts/verify/control-clavyn.mjs press Enter              # runs "Go to Terminal"
    node scripts/verify/control-clavyn.mjs info --pretty            # view should be terminal
    node scripts/verify/control-clavyn.mjs close-command-palette

## Gotchas

- The palette is a `role: dialog` with a search input. After running a
  command it auto-navigates and closes.
- `Cmd/Ctrl+K` is also the global palette toggle — pressing it again closes.
- While the palette is open, focus is in its input; `type` goes there, not
  into the main view. Close it before interacting with the underlying view.
