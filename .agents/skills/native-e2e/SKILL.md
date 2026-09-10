---
name: native-e2e
description: End-to-end test and debug functionality in the real packaged Clavyn app (tauri dev or release binary) on macOS, Linux, and Windows. Use when a feature crosses the webview↔OS boundary — drag-and-drop, file drop, clipboard, native dialogs/menus, shortcuts, window management, tray, updater — or when the mocked Chromium harness can't reproduce a bug. Covers OS-level input/synthesis, accessibility-tree inspection, DOM event instrumentation, and per-platform toolkits.
license: Proprietary. LICENSE.txt has complete terms
compatibility: Requires the platform's native input/inspection tools (see per-OS recipes). Designed for any Agent Skills-compatible agent (VS Code, Copilot, Claude Code, Codex, Cursor, Gemini CLI, Goose, OpenHands, Devin, etc.).
metadata:
  author: CodeAny-inc
  version: "1.0"
  repository: https://github.com/CodeAny-inc/Clavyn
allowed-tools: Bash Read Grep Glob Edit
---

# native-e2e — verify and debug the real native app

The `verify-clavyn` harness (Playwright + `tauri-fixture.js`) proves the
**renderer** is correct — but it runs the frontend in Chromium with all Tauri
IPC mocked. Anything that lives in or crosses the **native layer** is invisible
to it. This skill is the complementary path: drive and inspect the real
`tauri dev` build (or a packaged binary) with OS-level input, and instrument
the page to observe what the webview actually receives.

## When the harness is not enough

Reach for this skill when the feature or bug touches the webview↔OS boundary:

- **HTML5 drag-and-drop / file drop** — Tauri's `dragDropEnabled` (default
  `true`) installs a native drop target that consumes drags before the DOM
  sees them. The renderer harness never exercises this layer; the issue is
  only reproducible in a real build.
- Clipboard, native file dialogs, context menus, global shortcuts, window
  management (move/resize/fullscreen), tray, auto-updater, OS theme
  detection, permissions prompts.
- Any bug that "works in tests but not in the app" — first suspect the
  native layer, not the Vue code.

If the harness reproduces it, stay there — it's faster and deterministic.

## Methodology

Follow this order; it isolates the failing layer quickly:

1. **Decide the layer.** If the bug involves a gesture, OS integration, or
   config in `tauri.conf.json`/`setup()`/`main.rs`, assume native until
   proven otherwise.
2. **Control test.** Reproduce the *gesture* in a plain browser or a 30-line
   HTML test page using the same synthetic input tool. This proves whether
   your input method can drive the feature at all — without it, an app
   failure is ambiguous (tooling vs. app bug).
3. **Instrument, don't guess.** Inject a temporary debug overlay into
   `desktop/index.html` that counts DOM events. Vite hot-reloads it into the
   running dev webview — no rebuild needed. This separates "event never
   dispatched" from "handler broken". See the snippet below.
4. **Drive the real build.** `cd desktop && npm run tauri dev`, then use the
   per-OS recipes. Screenshot mid-gesture, not just after — transient state
   (drop zones, overlays, counters) vanishes on release.
5. **Compare against the control.** Same gesture working in the browser but
   dead in the app = native layer. Dead in both = your input tooling.
6. **Revert instrumentation** before finishing.

### Event-counter overlay (paste into `index.html` before the module script)

```html
<pre id="dnd-debug" style="position:fixed;right:8px;top:90px;z-index:99999;background:#000c;color:#0f0;padding:8px;font:12px monospace;pointer-events:none">DND-DBG</pre>
<script>
  const dbg = document.getElementById("dnd-debug");
  const seen = {}; let last = "";
  const render = () => { dbg.textContent = "DND-DBG\n" + last +
    Object.entries(seen).map(([k,v]) => k+":"+v).join("\n"); };
  for (const t of ["dragstart","dragenter","dragover","dragleave","drop","dragend"]) {
    window.addEventListener(t, e => { seen[t]=(seen[t]||0)+1;
      last = "last:"+t+"@"+Math.round(e.clientX)+","+Math.round(e.clientY)+"\n"; render(); }, true);
  }
  window.addEventListener("mousedown", e => {
    last = "down@"+Math.round(e.clientX)+","+Math.round(e.clientY)+"\n"; render(); }, true);
</script>
```

Adapt the event list to the feature (`copy`/`paste`, `keydown`, `focus`,
pointer events, `beforeinput`, etc.). For store-level state you can't see in
the DOM, temporarily `console.log` and read webview devtools (below), or add a
`#[tauri::command]` that prints to stdout.

### Signature: native drag-drop swallow (the #24 pattern)

`dragstart` + `dragend` fire but **zero** `dragenter`/`dragover`/`drop` — the
gesture *looks* alive (ghost chip follows the cursor) while every destination
handler is dead code. On macOS this is wry's `NSDraggingDestination` on
`WryWebView` returning "handled" without calling `super` whenever
`dragDropEnabled` is unset/true. On Windows the same default eats drops in
WebView2. If you see this signature, check `tauri.conf.json` for
`dragDropEnabled` before touching frontend code.

## macOS recipe

Tooling: `brew install cliclick` (input), `screencapture -x <file>` (screens),
`osascript` + System Events (accessibility tree, window management).

**Permissions** — two TCC grants, both verified with a probe before assuming
they work: Accessibility (`cliclick p` should move the real cursor) and
Screen Recording (`screencapture` output must show real windows, not
wallpaper). If `osascript` errors with "not allowed assistive access (-1728)",
Accessibility is missing.

**Coordinate math.** `screencapture` on Retina produces pixels at 2× logical
points (3024×1964 px = 1512×982 pt). `cliclick` and AX positions are in
logical points — divide screenshot pixels by 2.

**Synthetic input:**

```bash
cliclick c:400,300                    # click
cliclick t:text                       # type
cliclick kp:esc / kp:return           # key press
cliclick w:300                        # wait ms
# A REAL drag is three distinct commands — m: is a plain move and never drags:
cliclick dd:X,Y   w:300               # press (drag start)
cliclick dm:X,Y   w:150               # continue drag — repeat in small steps
cliclick du:X,Y                       # release (drag end)
```

Pitfalls learned the hard way:

- `m:` sends `kCGEventMouseMoved` even with the button held — a drag needs
  `dm:` (`kCGEventLeftMouseDragged`). A press followed by `m:` moves + `du`
  is a click-select, not a drag.
- **Release at the last `dm` point.** WKWebView decides drop vs. cancel from
  the dragover state at the release location — jumping the release away from
  the last hover can yield `dragleave`/`dragend` instead of `drop` even when
  everything is healthy.
- Real human drags move continuously; mirror that with several `dm:` steps
  and short `w:` waits so the webview's async dragover IPC keeps up.

**Window management:**

```applescript
tell application "System Events"
  repeat with p in (every process whose name is "clavyn-desktop")
    if (count of windows of p) > 0 then
      set frontmost of p to true
      tell window "Clavyn" of p
        set position to {0, 40}      -- pin for deterministic coords
        set size to {1200, 800}
        perform action "AXRaise"
      end tell
    end if
  end repeat
end tell
```

- Dev and installed builds share the process name `clavyn-desktop` —
  enumerate processes and distinguish by window count/PID, and never kill a
  PID that might be the user's real app.
- Other apps steal focus between commands; re-`frontmost` immediately before
  each gesture.
- AX tree: `entire contents of window "Clavyn"` yields role/description/
  position/size — find elements by `description contains "..."` instead of
  guessing pixels.

**Webview devtools:** dev builds set `isInspectable` — right-click →
Inspect Element, or Safari → Develop → \<machine\> → the webview.

## Linux recipe

X11 (check with `echo $XDG_SESSION_TYPE`):

```bash
xdotool mousemove X Y mousedown 1 mousemove ... mouseup 1   # real drags work
import -window root out.png    # or scrot / maim
xdotool search --name Clavyn windowactivate windowmove 0 40
```

Wayland: `xdotool` does nothing. Use `ydotool` (kernel uinput — needs the
`ydotoold` daemon running) or `wdotool` (libei/wlr protocols, respects the
compositor). Screenshots via `grim` (wlroots) or `gnome-screenshot`; window
control goes through compositor IPC (`swaymsg`, `hyprctl`, KWin scripts) —
Wayland has no global "move another window" API by design.

Inspection: AT-SPI via `dogtail`/`pyatspi` scripts or `accerciser`.
WebKitGTK is the same engine family as WKWebView — WebKit quirks (drag-drop
acceptance, user-select behavior) often reproduce here before macOS.

Scripted e2e: `cargo install tauri-driver --locked` + distro
`WebKitWebDriver` (e.g. `webkit2gtk-driver`), then Selenium/WDIO.

## Windows recipe

- `pyautogui` — `moveTo`, `dragTo(x, y, duration, button="left")`,
  `screenshot()`. Simplest reliable real-drag driver.
- `pywinauto` (`backend="uia"`) — full UIA tree, `set_focus()`,
  `click_input()`, `drag_mouse_input()`. Best for element-accurate tests.
- PowerShell fallbacks: `[System.Windows.Forms]` + `CopyFromScreen` for
  screenshots; `SendInput` via `Add-Type` for input; `nircmd savescreenshot`.
- UIA inspection: `Inspect.exe` (Windows SDK) or Accessibility Insights.
- WebView2 is evergreen — the embedded runtime usually matches Edge;
  `msedgedriver` + `tauri-driver` is the scripted-e2e path (version must
  match the installed runtime or sessions hang).
- GUI agents on Windows CI/VMs need an interactive session — RDP-locked or
  service sessions can't receive synthesized input; run the VM with an
  autologon console.

## Cross-platform scripted option

`@wdio/tauri-service` (WebdriverIO) is the official real-app e2e path. With
`tauri-plugin-wdio-webdriver` it runs an **embedded** WebDriver server inside
the app — the only route that works on macOS, where no WKWebView driver
exists. External `tauri-driver` covers Windows (msedgedriver) and Linux
(WebKitWebDriver). It also mocks IPC via `browser.tauri.execute()`, so it
sits between the mocked harness and hand-driven OS input: real binary, real
webview, scripted actions — but note embedded WebDriver synthesizes DOM-level
events, which does **not** exercise the native interception layer that
OS-level input does. Use OS-level input when the native boundary itself is
suspect.

## Debugging patterns

- **Ground truth is the dependency source.** For wry/Tauri behavior, read
  `~/.cargo/registry/src/*/wry-*/src/` and `tauri-runtime-wry-*/src/lib.rs` —
  `drag_drop.rs`, `class/wry_web_view.rs`, and where `with_drag_drop_handler`
  is installed tell you exactly what the native layer does.
- **Check config before code.** `tauri.conf.json` window flags
  (`dragDropEnabled`, `center`, `decorations`, `transparent`, `focus`,
  `acceptFirstMouse`) install native behavior that no amount of frontend
  work can override.
- **Distinguish three failure shapes:** event absent (native swallow) →
  fix config/native layer; event present but state wrong → fix store logic;
  state right but UI wrong → fix render. The counter overlay separates the
  first from the rest in one screenshot.
- **Multiple instances lie.** A dev build and an installed copy look
  identical. Verify which binary your input lands in (titlebar, version,
  debug marker, `lsof -iTCP:1420`) before trusting a negative result.
- **A stale page can masquerade as a bug.** If the webview loaded before
  Vite was up (or the server died), HMR silently stops; confirm the page
  actually reloaded (a marker element) before reading results.

## Rules

- Never declare a native-boundary feature working (or broken) from harness
  results alone — run the real build.
- Always run the browser control test before trusting a synthetic-input
  negative.
- Evidence = mid-gesture screenshot + post-release screenshot + event
  counters. A single end-state screenshot can't distinguish "swallowed" from
  "handled wrong".
- Revert all instrumentation (`index.html` overlay, auto-mount helpers,
  debug commands) and leave `git status` clean of test scaffolding.
- Prefer the project's `verify-clavyn` skill for anything the harness can
  see; reach for this skill only across the native boundary.
