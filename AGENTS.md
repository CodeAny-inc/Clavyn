# AGENTS.md — project notes for AI assistants working on Clavyn

## Build & run
- Desktop: `cd desktop && npm install && npm run tauri dev`
- Rust not yet installed in dev env; install via rustup first.
- Workspace root is the repo root; `cargo build` from root builds core + desktop.

## Verification
- `cargo check --workspace` after core changes.
- `cargo test -p clavyn-core` once tests are added.
- Tauri build: `cd desktop && npm run tauri build`.
- Frontend typecheck: `cd desktop && npx vue-tsc --noEmit`.
- Frontend unit tests: `cd desktop && npm test` (Vitest).
- Browser regression suite: `cd desktop/e2e && npm test` (Playwright + tauri-fixture.js mock).
- **UI verification with the agent CLI** — see "Agent UI verification" below.
- **Comment hygiene check** — `node scripts/verify/check-comments.mjs` before
  committing. See "Comment hygiene (enforced)" below.
- **Audit suppression expiry** — `node scripts/verify/check-audit-expiry.mjs`
  after touching `.cargo/audit.toml`. Every entry in the `ignore` list needs a
  `# review-by: YYYY-MM-DD` comment; the check fails once a date is reached, so
  no `cargo audit` suppression can sit there unexamined.

## Agent UI verification

There is a composable CLI for driving and verifying the live Tauri renderer
over the Chrome DevTools Protocol (via Playwright). Use it after any frontend
change to confirm behavior, capture evidence, and navigate features — it's
far cheaper than writing throwaway scripts.

    node scripts/verify/control-clavyn.mjs doctor --pretty

The CLI holds a persistent Chromium + page (the "harness") so state survives
across commands. It loads the Vite dev server with `desktop/e2e/tauri-fixture.js`
mocking the full Tauri IPC surface, so SSH/SFTP/vault calls are deterministic
and never touch the network. Every command prints one JSON object; errors
include a `remedy` field.

The canonical CLI + docs live in `scripts/verify/` (agent-agnostic). The
canonical Agent Skill (following the agentskills.io standard) lives at
`.agents/skills/verify-clavyn/SKILL.md` — this is the cross-agent entry point
supported by VS Code, Copilot, Claude Code, Cursor, Codex, Gemini CLI, Goose,
OpenHands, and more. Agent-specific pointer files reference that canonical
skill:

| Agent | Pointer | Convention |
|-------|---------|------------|
| **All agentskills.io-compatible** | `.agents/skills/verify-clavyn/SKILL.md` | Standard Agent Skills (agentskills.io) |
| Universal / Codex | `AGENTS.md` (this file) | De facto standard |
| Devin | `.devin/skills/verify-clavyn/SKILL.md` | Devin skills |
| Claude Code | `.claude/commands/verify-clavyn.md` | Slash commands |
| Cursor | `.cursor/rules/verify-clavyn.mdc` | Rules |

## Comment hygiene verification

The comment hygiene rule (see "Comment hygiene (enforced)" below) is enforced
by `scripts/verify/check-comments.mjs`, which has its own canonical Agent
Skill at `.agents/skills/check-comments/SKILL.md` with the same multi-agent
pointer pattern:

| Agent | Pointer | Convention |
|-------|---------|------------|
| **All agentskills.io-compatible** | `.agents/skills/check-comments/SKILL.md` | Standard Agent Skills (agentskills.io) |
| Universal / Codex | `AGENTS.md` (this file) | De facto standard |
| Devin | `.devin/skills/check-comments/SKILL.md` | Devin skills |
| Claude Code | `.claude/commands/check-comments.md` | Slash commands |
| Cursor | `.cursor/rules/check-comments.mdc` | Rules |

Prerequisites (once per machine):

    cd desktop && npm install
    cd desktop/e2e && npm install && npx playwright install chromium
    # or set CHROMIUM_EXECUTABLE_PATH to a system Chrome/Chromium

Common commands: `doctor`, `status`, `info`, `snapshot`, `screenshot`,
`components`, `navigate <view>`, `home`, `click --name "text"`, `type`,
`press`, `new-session`, `connect "<host>"`, `send "<text>"`, `console`,
`network-summary`, `fixture state`, `cleanup`, `reset`, `stop`. Run with no
args for full `--help`. Destructive commands accept `--dry-run`.

The **Feature Map** (`scripts/verify/features/README.md`) catalogs every view,
what it does, how to reach it, and the exact CLI commands to drive it — read
it before navigating to save context tokens. Full docs: `scripts/verify/README.md`.

Notes:
- The fixture mocks the transport; this verifies the **renderer**, not
  end-to-end SSH. Core Rust logic is verified with `cargo test -p clavyn-core`.
- To drive a real `tauri dev`/packaged webview, set `CLAVYN_URL` and
  `CHROMIUM_EXECUTABLE_PATH` to point at its webview endpoint.

## Browser UI test pitfalls (e2e)

The e2e suite (`desktop/e2e/*.mjs`, run via `cd desktop/e2e && npm test`) uses
Playwright + the tauri-fixture mock. The following pitfalls have caused
real CI failures — check against this list before pushing frontend changes.

### Always run e2e tests locally before pushing
    cd desktop/e2e && UI_RECORD=0 npm test

CI runners are slower than local machines; a 30-second `waitForFunction`
timeout can pass locally but fail in CI. Always run the full e2e suite
locally with `UI_RECORD=0` (disables video recording for speed) before
pushing. If a test times out, reproduce with `UI_SLOW_MO=50` to see the
exact interaction that stalls.

### xterm stops propagation for certain keys
xterm's `evaluateKeyboardEvent` sets `cancel: true` for keys like Escape
(keyCode 27), Enter (13), and Tab (9). When `cancel` is true, xterm calls
`event.stopPropagation()`, which prevents the event from bubbling up to
window-level listeners (e.g. the Escape handler in `App.vue`).

When adding a keyboard shortcut that must work while an xterm terminal has
focus, handle it inside `attachCustomKeyEventHandler` in `TerminalPane.vue`
and `return false` for that key. Returning `false` prevents xterm from
calling `stopPropagation()`, so the event bubbles up to window-level
handlers. Returning `true` lets xterm process the key AND stop propagation.

### Teleport changes DOM location but not component identity
`<Teleport to="body">` moves the DOM element to `<body>` without remounting
the Vue component — refs, state, and xterm instances are preserved. However:
- **Unit tests** (`@vue/test-utils`): `wrapper.get(selector)` only searches
  the component's subtree, not teleported content. Use `document.querySelector`
  for elements that may be teleported (e.g. a fullscreen pane).
- **E2e tests** (Playwright): `page.locator(selector)` searches the entire
  page, so teleported elements are found automatically.
- Scoped styles still apply to teleported elements (Vue preserves the
  `data-v-xxxx` attribute), but CSS variables from `:root` are the only
  guaranteed inherited values.

### Fullscreen panes must escape ancestor containers
A `position: fixed` element inside a container with `overflow: hidden` or
`v-show` (which sets `display: none`) can be clipped or hidden in some
webview renderers (Tauri WKWebView, WebView2), even though the CSS spec says
`overflow: hidden` should not clip fixed descendants. Use
`<Teleport to="body" :disabled="!isFullscreen">` to move the fullscreen
element out of all ancestor containers. The element returns to its original
position when the Teleport is disabled.

### Vite fixture injection must be syntactically valid
The dev-only Vite plugin in `vite.config.ts` injects `tauri-fixture.js` into
`index.html` so the app renders in a regular browser (not just Tauri). The
fixture is an IIFE that ends with `();`. Do NOT wrap it in parentheses
(`(fixture);` → `(););` → SyntaxError). Use an `if` block:
`if(!window.__TAURI_INTERNALS__){fixtureCode}`. In Tauri,
`__TAURI_INTERNALS__` is already defined so the fixture is skipped. The
plugin uses `apply: "serve"` so production builds are unaffected.

### Verify the app loads without the harness
After changes to `vite.config.ts`, `index.html`, or the fixture, verify the
app loads in a fresh browser without the Playwright `addInitScript` fixture:
the Vite-injected fixture should be sufficient. Check for console errors
and that the sidebar, terminal, and xterm all render.

## Building distributable versions
- Local build (current platform only): `cd desktop && npm run tauri build`
  - macOS: produces `.dmg` + `.app` in `desktop/src-tauri/target/release/bundle/`
  - Linux: produces `.deb`, `.AppImage`, `.rpm`
  - Windows: produces `.msi` + `.exe` (NSIS)
- Cross-platform releases via GitHub Actions:
  - Push a tag `v0.1.0` → triggers `.github/workflows/release.yml`
  - Builds on macOS (arm64 + x86_64), Linux, Windows runners
  - Signs update packages with `TAURI_SIGNING_PRIVATE_KEY` secret
  - Publishes artifacts + `latest.json` to GitHub Releases
  - Automatically detects prereleases (alpha/beta/rc in version)

## Release automation

**Agents MUST use the `release-clavyn` skill for every release.** It lives at
`.agents/skills/release-clavyn/SKILL.md` (agentskills.io standard) with
pointers at `.devin/skills/release-clavyn/`, `.claude/commands/release-clavyn.md`,
and `.cursor/rules/release-clavyn.mdc`. It covers: maintaining `CHANGELOG.md`
(Keep a Changelog format — every release gets an entry), choosing between the
CI tag-push path (preferred — signs with the `TAURI_SIGNING_PRIVATE_KEY`
GitHub secret and builds all platforms) and `scripts/release.sh` (local macOS
build, requires a local key), the verification suite, the moving-main rebase
dance, and replacing the workflow's placeholder release notes with real
changelog content via `gh release edit`.

| Agent | Pointer | Convention |
|-------|---------|------------|
| **All agentskills.io-compatible** | `.agents/skills/release-clavyn/SKILL.md` | Standard Agent Skills (agentskills.io) |
| Universal / Codex | `AGENTS.md` (this file) | De facto standard |
| Devin | `.devin/skills/release-clavyn/SKILL.md` | Devin skills |
| Claude Code | `.claude/commands/release-clavyn.md` | Slash commands |
| Cursor | `.cursor/rules/release-clavyn.mdc` | Rules |

### Local release script: `scripts/release.sh`
Automates the entire local release process for macOS (current platform):
```bash
# Release next alpha (bumps version, builds, signs, uploads)
./scripts/release.sh alpha

# Release a specific version
./scripts/release.sh 0.2.0

# Release current version with custom notes
./scripts/release.sh --notes "Fixed critical bug"

# Dry run (see what would happen without executing)
./scripts/release.sh --dry-run alpha

# Skip checks (faster, use with caution)
./scripts/release.sh --skip-checks alpha
```
The script: checks prerequisites → bumps version → runs checks → builds →
creates DMG + tarball → signs tarball → generates latest.json → commits,
tags, pushes → creates GitHub release with all assets.

### Version management: `scripts/version.sh`
```bash
./scripts/version.sh              # print current version
./scripts/version.sh 0.1.2        # set to 0.1.2
./scripts/version.sh patch        # 0.1.1 -> 0.1.2
./scripts/version.sh minor        # 0.1.1 -> 0.2.0
./scripts/version.sh major        # 0.1.1 -> 1.0.0
./scripts/version.sh alpha        # 0.1.1-alpha.9 -> 0.1.1-alpha.10
./scripts/version.sh beta         # 0.1.1 -> 0.1.1-beta.1
./scripts/version.sh stable       # 0.1.1-alpha.9 -> 0.1.1
```
Updates version in Cargo.toml, package.json, and tauri.conf.json.

### GitHub Actions: `.github/workflows/release.yml`
Triggers on tag push (`v*.*.*`) or manual dispatch.
- Builds on macOS (arm64 + x86_64), Linux, Windows in parallel
- Signs all bundles with `TAURI_SIGNING_PRIVATE_KEY` (requires `createUpdaterArtifacts: true` in tauri.conf.json)
- Auto-detects prerelease (alpha/beta/rc in version string)
- Generates `latest.json` with all platform signatures
- Uploads everything to the GitHub release

## Auto-update
- Uses `tauri-plugin-updater` (configured in `tauri.conf.json` under `plugins.updater`)
- **Important**: The endpoint in `tauri.conf.json` is only a fallback. The actual
  update check uses a custom implementation in `commands.rs` that queries the
  GitHub API (`/releases` endpoint) to find the newest release — **including
  prereleases**. GitHub's `releases/latest` redirect excludes prereleases, so
  the default Tauri updater endpoint doesn't work for alpha/beta releases.
- The custom check (`check_with_prerelease_endpoint`):
  1. Calls `https://api.github.com/repos/CodeAny-inc/Clavyn/releases`
  2. Parses all release tags as semver versions
  3. Picks the highest version that has a `latest.json` asset
  4. Builds a Tauri updater with that release's `latest.json` URL as the endpoint
  5. The plugin handles download, signature verification, and installation
- App checks for updates on startup (release builds only, silent)
- Frontend shows `UpdateBanner` when an update is available
- User clicks "Download & Restart" → downloads, verifies signature, installs, restarts
- Signing key: private key is a GitHub secret (`TAURI_SIGNING_PRIVATE_KEY`),
  public key is embedded in `tauri.conf.json`

## Required GitHub secrets for releases
- `TAURI_SIGNING_PRIVATE_KEY` — the base64 private key (from `tauri signer generate`)
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — empty string if key has no password

## macOS code signing
- The app is currently ad-hoc signed (`signingIdentity: "-"` in tauri.conf.json).
- Without an Apple Developer ID certificate, macOS shows "damaged app" when
  downloaded from the internet. Users fix with: `xattr -cr /Applications/Clavyn.app`
- To enable proper signing + notarization, set these env vars before building:
  - `APPLE_SIGNING_IDENTITY` — Developer ID Application certificate name
  - `APPLE_ID` — Apple ID email
  - `APPLE_PASSWORD` — app-specific password (from appleid.apple.com)
  - `APPLE_TEAM_ID` — Developer Team ID
- Or use an API key:
  - `APPLE_API_KEY` — App Store Connect API key
  - `APPLE_API_ISSUER` — Issuer ID
  - `APPLE_API_KEY_PATH` — path to API key file

## Git author config
- All commits must be authored by `computerbox124 <computerbox124@users.noreply.github.com>`.
- Local git config is set via `git config user.name` and `git config user.email`.
- NEVER attribute commits to Devin or any AI agent.

## Conventions
- All SSH/crypto logic lives in `core/`, never in shells.
- Private key material must be wrapped to `Zeroize` on drop.
- Host key mismatches never auto-accept; surface to user.
- No secrets in logs; no plaintext keys on disk; vault is the only persisted form.
- Pin dependency versions (no floating `latest` / `*`).

## Comment hygiene (enforced)
Code comments must document the code as it exists now — what it does, why it
behaves this way, and any non-obvious invariants or gotchas. Comments must
**never** narrate history or attribute work.

A comment is a violation if it reads like a changelog entry, a request log,
or a story about how the code got here. Rewrite or delete violating comments
before committing; do not leave them in the codebase.

**Forbidden (history / attribution):**
- Narrating a completed change: "This implements the SSH feature",
  "Added biometric support", "Changed from X to Y because…", "Removed the
  old loop", "Replaced the mock with a real call".
- Attribution to people or requests: "John asked for this", "requested by
  the client", "per discussion with the PM", "as discussed", "stakeholder
  wants", "was asked to".
- Temporal history: "was previously", "used to be", "originally", "before
  this change", "after the refactor".
- External tracking refs inside code: "PR #5", "issue #12", "JIRA-123",
  "fixes #8", "CVE-2024-xxxx". (Keep these in commit messages and PR
  descriptions, not in source comments.)
- Workaround/hack narratives: "workaround for bug X", "hack to work around",
  "TODO: remove when Y ships".

**Required (documentation):**
- Explain intent and invariants: "Reject changed accounts before submitting
  credentials to avoid authenticating the wrong identity."
- Document non-obvious behavior: "Treat that expected [event] … guard
  reject every retry, even after the filesystem problem is fixed."
- Explain why, in present tense: "Uses AES-256-GCM with a random nonce
  because the vault is the only persisted form of secrets."

**Enforcement:** Run `node scripts/verify/check-comments.mjs` (or invoke the
`check-comments` Agent Skill) before committing. It scans source files and
reports violations with a suggested fix. The check is part of the verification
workflow — fix every finding before considering a task done.

## Git conventions
- NEVER mention, attribute, or add a co-author trailer for Devin or any AI
  agent in commit messages. No "Generated with Devin", no
  "Co-Authored-By: Devin", no AI attribution of any kind. Commits should look
  like they were written by the human author only.
- Commit messages: concise, focus on "why" not "what".

## Pull request conventions
- PR title and description must be in English.
- PR description must explain which changes were made and what the
  implementation solves — not just a diff summary.
- Inline screenshots and recordings are required in the PR description for any
  UI/UX change. They must render as **inline images**, not plain links — use
  Markdown image syntax `![alt text](url)` so reviewers see the image directly
  in the PR body. Never paste a bare URL; GitHub will not auto-embed it.
  Use absolute raw URLs, never relative paths — GitHub does not resolve
  `./screenshots/foo.png` in a PR body. Use the form:
  `![Vault unlocked](https://github.com/CodeAny-inc/Clavyn/raw/<branch>/screenshots/foo.png)`
- Verify every media URL resolves to real content (HTTP 200, correct
  content-type) before considering the PR done.
- Run the full verification suite and record the results in the
  PR's test-plan checklist:
  1. `cd desktop && npx vue-tsc --noEmit` — typecheck
  2. `cd desktop && npm test` — unit tests (Vitest)
  3. `cd desktop/e2e && UI_RECORD=0 npm test` — browser UI tests (Playwright)
  4. `node scripts/verify/check-comments.mjs` — comment hygiene
  5. `node scripts/verify/control-clavyn.mjs doctor --pretty` — harness health
  6. Scenario-specific verification scripts (e.g. `verify-fullscreen.mjs`)

## Architecture
See `docs/ARCHITECTURE.md`. One Rust core, Tauri desktop shell now, mobile
via FFI later.
