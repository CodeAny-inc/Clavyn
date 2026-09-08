# Clavyn

Open-source, cross-platform manager for SSH connections, keys, and known hosts.
A community-driven alternative to Termius.

> **Status:** early scaffold. Desktop (Tauri) shell + shared Rust core are in
> place; mobile (iOS/iPadOS/Android) is planned.

## Goals

- **Secure by default.** Private keys encrypted at rest (Argon2id + AES-256-GCM).
  OS keychain stores the vault passphrase. TOFU host-key verification with no
  silent auto-accept on mismatch. Private material is zeroized on drop.
- **Fast & light.** Tauri shell (~10 MB binaries) instead of Electron.
- **Cross-platform.** One Rust core, multiple UIs:
  - Desktop (Linux / Windows / macOS): Tauri
  - Mobile (iOS / iPadOS / Android): native UI over the same core via FFI
- **Auditable.** Minimal dependency surface, pinned versions, no telemetry on
  connection payloads.

## Repository layout

```
Clavyn/
  Cargo.toml              # workspace root
  core/                   # clavyn-core: shared Rust library
    src/
      connection.rs       # SSH transport (russh)
      host.rs             # host / auth models
      keys.rs             # key parse / generate (russh-keys)
      known_hosts.rs      # TOFU known_hosts store
      vault.rs            # encrypted-at-rest key vault
      error.rs
  desktop/                # Tauri desktop app
    src/                  # frontend (HTML/JS, to be upgraded to a framework)
    src-tauri/            # Rust backend + Tauri config
      src/
        commands.rs       # Tauri commands exposed to the frontend
        state.rs          # shared app state (vault, known_hosts, passphrase)
        main.rs
  mobile/                 # planned: iOS / Android (see mobile/README.md)
  docs/
    ARCHITECTURE.md
```

## Prerequisites

- **Rust** (stable, via [rustup](https://rustup.rs)) — not yet installed in
  this environment; install before building:
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Node.js** 18+ and npm
- **Tauri v2 system dependencies**:
  - macOS: `xcode-select --install`
  - Linux (Debian): `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  - Windows: Microsoft C++ Build Tools + WebView2

## Build & run (desktop)

```sh
# from repo root
cd desktop
npm install
npm run tauri dev    # launches the app with hot reload
npm run tauri build  # produces installers in src-tauri/target/release/bundle
```

## A note on "damaged app" / SmartScreen warnings

Clavyn is not yet signed with an Apple Developer ID or a Windows code-signing
certificate, so the OS may complain when you open a downloaded build for the
first time:

- **macOS:** Gatekeeper may report the app as "damaged" or "unidentified
  developer." This is expected for unsigned builds, not actual corruption. To
  run it, strip the quarantine attribute:
  ```sh
  xattr -cr /Applications/Clavyn.app
  ```
- **Windows:** SmartScreen may show "Windows protected your PC." Click
  **More info → Run anyway**.

This is temporary. Proper code signing and notarization are planned for a
future release, after which these warnings will disappear. No worries — for
now just tell your OS it's fine to open Clavyn. :)

## Security model (summary)

| Asset                | At rest                         | In memory                |
|----------------------|---------------------------------|--------------------------|
| Private keys         | AES-256-GCM in `vault.json`     | `Zeroizing` wrappers     |
| Vault passphrase     | OS keychain (UI-managed)        | `Zeroizing<String>`      |
| Host passwords       | OS keychain only                | never in core state      |
| known_hosts          | plaintext JSON (public keys)    | n/a                      |

Host-key mismatches raise `CoreError::HostKeyMismatch` and are surfaced to
the user for explicit confirmation before any replacement.

## Contributing

Clavyn is open source and contributions are welcome. The project is already
heavily used internally by CodeAny employees and is actively maintained — it
will continue to be improved consistently over time. There is **no published
roadmap**; instead, development is driven by real usage and by the issues and
pull requests the community opens.

### Reporting bugs and suggesting features

- **Bugs:** Open an issue with the steps to reproduce, your OS, the Clavyn
  version (`Settings → About` or `node scripts/verify/control-clavyn.mjs info`),
  and any relevant logs. Do **not** include private keys, passphrases, or
  vault contents in reports.
- **Feature requests:** Open an issue describing the problem you are trying to
  solve and your proposed approach. Keep scope focused — small, well-defined
  proposals are easier to review and merge.
- **Security vulnerabilities:** Do not open a public issue. See
  [SECURITY.md](SECURITY.md) for responsible disclosure.

### Development setup

See [Prerequisites](#prerequisites) and [Build & run (desktop)](#build-run-desktop)
above to get the app running locally. For the full development workflow,
conventions, and verification steps, read [AGENTS.md](AGENTS.md) — it is the
single source of truth for how the project is built, tested, and released.

### How to submit a pull request

1. **Open an issue first** for anything beyond a small fix or typo. Discussing
   the approach before writing code saves everyone time and avoids rework.
2. **Fork and branch** from `main`. Use a descriptive branch name
   (`fix/vault-lock-race`, `feat/sftp-sort`, not `patch-1`).
3. **Keep changes focused.** One logical change per PR. Mixing unrelated
   refactors with a bug fix makes review harder and risks the whole PR being
   blocked by one part.
4. **Follow the conventions** in [AGENTS.md](AGENTS.md):
   - All SSH/crypto logic lives in `core/`, never in shells.
   - Private key material is wrapped to `Zeroize` on drop.
   - Host key mismatches never auto-accept; surface to the user.
   - No secrets in logs; no plaintext keys on disk; vault is the only persisted form.
   - Pin dependency versions (no floating `latest` / `*`).
5. **Verify before requesting review:**
   ```sh
   cargo check --workspace                    # after core changes
   cargo test -p clavyn-core                  # core tests
   cd desktop && npx vue-tsc --noEmit         # frontend typecheck
   cd desktop && npm test                     # Vitest unit tests
   cd desktop/e2e && npm test                 # Playwright regression suite
   node scripts/verify/control-clavyn.mjs doctor --pretty   # UI verification
   node scripts/verify/check-comments.mjs --pretty          # comment hygiene
   ```
   The comment-hygiene check enforces that code comments document the code
   as it exists now — never narrate history or attribute work. See
   [AGENTS.md](AGENTS.md) "Comment hygiene (enforced)" for the policy.
6. **Write good commit messages.** Focus on *why*, not *what*. Keep commits
   atomic and logically ordered. Squash WIP commits before requesting review.
7. **Reference the issue** in the PR description (`Closes #123`). Include a
   Summary, what changed, and how you verified it.

### Code review

PRs are reviewed by CodeAny maintainers. Expect feedback on security,
correctness, and adherence to conventions. Please be patient and responsive —
review is a collaboration, not a gate. Small, well-scoped PRs with clear
descriptions and verification evidence are reviewed fastest.

### Release process

Releases are automated via GitHub Actions (triggered by a version tag) and the
local `scripts/release.sh` / `scripts/version.sh` helpers. See
[AGENTS.md](AGENTS.md) for the full release process, version management, and
auto-update architecture.

## License

GPL-3.0-or-later (see `LICENSE`). Inspired by but unaffiliated with Termius.
