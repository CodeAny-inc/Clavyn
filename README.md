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

## Security model (summary)

| Asset                | At rest                         | In memory                |
|----------------------|---------------------------------|--------------------------|
| Private keys         | AES-256-GCM in `vault.json`     | `Zeroizing` wrappers     |
| Vault passphrase     | OS keychain (UI-managed)        | `Zeroizing<String>`      |
| Host passwords       | OS keychain only                | never in core state      |
| known_hosts          | plaintext JSON (public keys)    | n/a                      |

Host-key mismatches raise `CoreError::HostKeyMismatch` and are surfaced to
the user for explicit confirmation before any replacement.

## License

GPL-3.0-or-later (see `LICENSE`). Inspired by but unaffiliated with Termius.
