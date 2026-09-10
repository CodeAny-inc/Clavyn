# Native E2E — test and debug the real packaged app

The canonical Agent Skill lives at `.agents/skills/native-e2e/SKILL.md`
(standard agentskills.io location). This slash command is a thin pointer so
Claude Code's `/native-e2e` command finds it.

## Quick reference

```bash
cd desktop && npm run tauri dev                  # real WKWebView build
cliclick dd:X,Y w:300 dm:X,Y w:150 du:X,Y        # REAL drag (dm: not m:)
screencapture -x /tmp/shot.png                    # screenshot (Retina: px = 2x pt)
osascript -e 'tell application "System Events" to ...'  # AX tree, frontmost, window pos
```

Decision rule: the `verify-clavyn` harness proves the renderer; anything
crossing the webview↔OS boundary (drag-drop, clipboard, native dialogs,
window management) needs this skill.

## Full instructions

Read `.agents/skills/native-e2e/SKILL.md` for the methodology (control test →
instrument → drive → evidence → revert), the native drag-drop swallow
signature, and per-platform toolkits for macOS, Linux, and Windows.
