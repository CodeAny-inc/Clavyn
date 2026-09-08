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

## Git conventions
- NEVER mention, attribute, or add a co-author trailer for Devin or any AI
  agent in commit messages. No "Generated with Devin", no
  "Co-Authored-By: Devin", no AI attribution of any kind. Commits should look
  like they were written by the human author only.
- Commit messages: concise, focus on "why" not "what".

## Architecture
See `docs/ARCHITECTURE.md`. One Rust core, Tauri desktop shell now, mobile
via FFI later.
