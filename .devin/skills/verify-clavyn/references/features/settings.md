# Settings

App info, auto-lock timeout, and the update system (check / download / install
/ restart). The update check uses a custom GitHub releases endpoint that
includes prereleases.

## Sub-features

- app-info: name, version, platform, arch (from `get_app_info`).
- auto-lock: select idle timeout (Never / 5m / 10m / 15m / 30m / 1h).
- update-check: "Check for updates" button → `check_for_updates`.
- update-install: if an update is available, "Download & Restart" →
  `install_update` (downloads, verifies signature, installs, restarts).
- update-banner: `UpdateModal` shown when `update.shouldNotify` is true.
- about: links to GitHub repo, license.

## How to get to it (user POV)

Click `Settings` in the sidebar, or `Cmd/Ctrl+,`. The update banner/modal also
appears automatically on startup when an update is available; `Cmd/Ctrl+U`
opens it.

## Driving it with control-clavyn

    node control-clavyn.mjs navigate settings
    node control-clavyn.mjs snapshot --pretty
    node control-clavyn.mjs click --name "Check for updates"
    node control-clavyn.mjs wait-settle
    node control-clavyn.mjs network-summary --pretty   # check_for_updates call
    node control-clavyn.mjs info --pretty               # app version

## Gotchas

- The fixture reports `available: false` with version `0.1.1-ui-test`, so the
  "update available" UI is **not** shown by default. To test the update
  banner/modal, extend the fixture to return `available: true`.
- `install_update` actually restarts the app in a real Tauri build — never
  invoke it in the harness expecting a graceful return; it's a no-op in the
  fixture but would restart a real build.
- Auto-lock options drive `useAutoLock`; changing the timeout resets the timer.
