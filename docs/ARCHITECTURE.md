# Clavyn — Architecture

## Principle: one core, many shells

```
                ┌─────────────────────────────────────────┐
                │           clavyn-core (Rust)        │
                │  connection · keys · vault · known_hosts │
                └──────────┬──────────────┬───────────────┘
                           │              │
            FFI (in-proc)  │              │ FFI (uniffi/cbindgen)
                           │              │
              ┌────────────▼─────┐   ┌────▼──────────────────────┐
              │  Tauri desktop   │   │  iOS / Android / iPadOS   │
              │  (Linux/Win/Mac) │   │  native UI over core      │
              └──────────────────┘   └───────────────────────────┘
```

The core never depends on a UI toolkit, a filesystem layout owned by a
specific shell, or a process-spawning capability (so it works inside the
iOS sandbox). All platform-specific concerns (where the vault file lives,
how the passphrase is retrieved from the OS keychain) are injected by the
shell.

## Layers

### core (`clavyn-core`)

- `host` — pure data model. No secrets in serialized form.
- `keys` — parse / generate OpenSSH keys via `russh-keys`. In-memory private
  material is wrapped in `PrivateKeyMaterial` which `Zeroize`s on drop.
- `vault` — Argon2id(passphrase, salt) -> AES-256-GCM. The only thing
  written to disk is ciphertext + salt + public metadata.
- `known_hosts` — TOFU. `verify()` records on first sight and compares
  thereafter; `check_mismatch()` returns `HostKeyMismatch` with both
  fingerprints and runs first, so the connection handler fails with that error
  rather than a generic unknown-key rejection. The presented key is held in
  memory and `trust_presented_key()` — the only caller of `replace()` — pins it
  after the user confirms the fingerprint that was displayed. `remove()`
  tombstones an entry instead of deleting it, so unpinning cannot silently
  downgrade a host back to first-use. `removed()` lists the tombstones and
  `forget()` erases one, which is the only path back to first-use and refuses a
  live pin, so it always takes two deliberate steps. The commands wrapping
  `forget()` and `trust_presented_key()` show a native dialog first
  (`desktop/src-tauri/src/host_key_prompt.rs`), carrying the fingerprint read
  from the store rather than from the caller's arguments; `remove()` does not,
  because the tombstone it leaves keeps the host under the mismatch check.
- `connection` — `russh` async client. Auth resolved from `AuthMethod` +
  vault + (optional) password. The returned `Handle` is owned by the shell,
  which streams channel data to the UI.

### desktop (`desktop/src-tauri`)

- `state` — owns `Vault`, `KnownHosts`, and the in-memory `passphrase`
  (`Zeroizing<String>`, cleared on lock).
- `commands` — Tauri `#[command]`s: `list_hosts`, `add_host`, `list_keys`,
  `generate_key`, `import_key`, `initialize_vault`, `connect_ssh`.
- Frontend (`desktop/src`) — minimal HTML/JS now; upgrade to a real
  framework (React/Svelte/Solid) once the command surface is stable.

### mobile (planned)

- Core compiled per target (`staticlib`/`cdylib`).
- iOS: `xcframework` + Swift bindings (uniffi or cbindgen).
- Android: per-ABI `.so` + Kotlin bindings (uniffi or jnigen).
- OS keychain/keystore for the passphrase; same vault file format.

## Threat model & mitigations

| Threat                         | Mitigation                                        |
|--------------------------------|---------------------------------------------------|
| Stolen laptop / disk read      | Vault is AES-256-GCM; KDF is Argon2id (64MiB)     |
| Passphrase leak                | Stored in OS keychain, never in core config       |
| MITM host key swap             | TOFU + hard fail on mismatch, explicit replace    |
| Memory scrape after lock       | `Zeroize` on drop for keys & passphrase           |
| Supply-chain (deps)            | Pinned versions, minimal surface, audit russh     |
| XSS in renderer                | Strict CSP in `tauri.conf.json`; no remote code   |
| Logging secrets                | No tracing of payloads/passphrases; redact keys   |

## Open design questions (to resolve before MVP)

1. Terminal emulation: xterm.js in the renderer, with PTY allocation in the
   Rust backend (portable-pty). Channel data flows via Tauri events.
2. Sync between devices: out of scope for MVP; later via end-to-end
   encrypted sync (vault is already a ciphertext blob — sync is trivial).
3. SFTP / port forwarding: russh supports both; wire as separate commands.
4. Agent forwarding: supported by russh; gate behind an explicit toggle.
