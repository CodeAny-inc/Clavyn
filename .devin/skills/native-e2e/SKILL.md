---
name: native-e2e
description: End-to-end test and debug functionality in the real packaged Clavyn app on macOS, Linux, and Windows. Use when a feature crosses the webview-OS boundary (drag-and-drop, clipboard, native dialogs, window management) or the mocked Chromium harness can't reproduce a bug.
allowed-tools: Bash Read Grep Glob Edit
---

# native-e2e — Devin pointer to the canonical Agent Skill

The canonical skill lives at `.agents/skills/native-e2e/SKILL.md` (the
standard Agent Skills location from agentskills.io, supported by Claude Code,
Cursor, Codex, VS Code, Copilot, Gemini CLI, Goose, OpenHands, and more).
This file exists so Devin's own discovery (`.devin/skills/`) finds it; the
content is identical.

## Quick reference

```bash
cd desktop && npm run tauri dev                  # real WKWebView build
cliclick dd:X,Y w:300 dm:X,Y w:150 du:X,Y        # REAL drag (dm: not m:)
screencapture -x /tmp/shot.png                    # screenshot (Retina: px = 2x pt)
osascript -e 'tell application "System Events" to ...'  # AX tree, frontmost, window pos
```

Decision rule: the `verify-clavyn` harness (Playwright + fixture) proves the
renderer; anything crossing the webview↔OS boundary needs this skill. Debug
overlay snippet for counting DOM events inside the webview lives in the
canonical file.

## Full instructions

Read `.agents/skills/native-e2e/SKILL.md` for the methodology (control test →
instrument → drive → evidence → revert), the native drag-drop swallow
signature, and the per-platform toolkits (cliclick/osascript, xdotool/ydotool,
pyautogui/pywinauto, tauri-driver vs embedded WebDriver).
