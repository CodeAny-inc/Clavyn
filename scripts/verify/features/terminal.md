# Terminal

Tabbed, split-able terminal panes. Each pane is an xterm.js instance backed by
a Tauri session (SSH via `connect_ssh` or a local shell via
`create_local_terminal`). Session data streams over Tauri events
(`session-data`, `session-closed`).

## Sub-features

- tab-strip: top tab bar with New-session picker (open in tab vs split).
  Drag a tab sideways to reorder it (insertion bar marks the landing edge);
  drag a tab onto a terminal pane to split it into that tab.
- terminal-pane: xterm.js pane; `data-host-id`, `data-session-id`,
  `data-connected`, `data-active` attributes for inspection.
- split-view: split panes horizontally/vertically; fullscreen a pane.
- pane-actions: per-pane action menu (split, close, fullscreen, etc.).
- pane-drag: drag a pane header grip onto another pane — edge zones split
  (above/below/left/right), center swaps; drop on a strip tab to merge the
  pane into that tab; drop on the strip/New-session area to extract the pane
  into its own tab. Dragging a whole tab onto a pane splits or swaps it too.
- local-shell: "Open local shell" from the New-session picker.

## How to get to it (user POV)

Click `Terminal` in the sidebar, or `Cmd/Ctrl+N` (opens a new tab directly),
or double-click a host in Hosts. Use the "New session" button → choose tab or
split → optionally "Open local shell".

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs navigate terminal
    node scripts/verify/control-clavyn.mjs new-session              # local shell in a new tab
    node scripts/verify/control-clavyn.mjs send "echo hello"
    node scripts/verify/control-clavyn.mjs fixture state --pretty   # see writes + live session
    node scripts/verify/control-clavyn.mjs screenshot /tmp/term.png

    # From Hosts: connect a fixture host, then interact
    node scripts/verify/control-clavyn.mjs home
    node scripts/verify/control-clavyn.mjs connect "Atlas Production"
    node scripts/verify/control-clavyn.mjs send "ls /srv"

## Gotchas

- xterm.js renders into a canvas; **text is not in the DOM**. Use
  `fixture state` (`writes`) to assert what was typed, and the fixture's
  echoed output to assert responses — don't try to read `.xterm-rows` text
  for exact assertions (it works but is fragile).
- `send` targets the active pane's `.xterm-helper-textarea`. If no pane is
  active, it errors with a remedy — run `new-session` or `connect` first.
- The fixture echoes `echo <text>` back as `<text>`; other input echoes a
  fake prompt. See `tauri-fixture.js` for the exact mock behavior.
- `data-connected="true"` on a pane marks a successful connect; wait for it
  with `wait-settle` before sending.
