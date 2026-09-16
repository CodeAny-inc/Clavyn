# Changelog

All notable changes to Clavyn are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2-alpha.6] - 2026-09-16

### Added

- **GPU-accelerated terminal rendering.** Panes now render through the
  xterm WebGL addon: a render-bound stream measured 5.9x faster per frame
  (4.5 ms → 0.73 ms at 120x40) and up to 10.7 MiB/s of throughput. A
  least-recently-used budget of eight panes keeps the app under the
  browser's 16-context WebGL cap, so opening a 17th terminal no longer
  blanks the first; evicted panes fall back to the DOM renderer, and a lost
  GPU context restores the DOM renderer with the transcript intact. The
  addon loads on demand in its own chunk, so the entry bundle grows by
  1.3 kB rather than 101 kB. (#63)
- **Changed host keys are now reported with both fingerprints instead of a
  bare "Unknown server key".** The presented key is held in memory and
  listed under Known Hosts, where trusting it requires confirming the
  displayed fingerprint. Removing a host tombstones its entry rather than
  deleting it, so an unpinned host can no longer silently return to
  trust-on-first-use and re-pin a different key as trusted. A "Removed
  hosts" section offers a deliberate "Forget permanently" step, and both
  commands that end a pin confirm through a native OS dialog that prints
  the real stored fingerprint — unforgeable from the webview. (#38)

### Changed

- **Startup is dramatically faster: the app boots in 3 IPC round-trips
  instead of 12.** The independent boot queries are dispatched in parallel
  and the update check no longer blocks first paint — it fires in the
  background and still opens the modal when a release is found. Host and
  group loads are de-duplicated across callers and versioned, so an
  in-flight read can no longer overwrite a newer change (which previously
  made a host added during startup appear and then vanish). (#55)
- **Terminal output is coalesced and moved off the JSON event path.**
  Output is batched (64 KiB or a 6 ms window, whichever comes first) and
  delivered as binary over a per-session IPC channel, replacing one
  JSON-number-array event per protocol packet. Bulk output drops from 128
  to 16 IPC messages per MiB and a program writing one byte at a time
  drops from 19,519 messages to 30; a 32 KiB chunk that used to become
  ~114 KiB of evaluated JavaScript now costs nothing to serialize.
  Keystroke echo is never held back — the first chunk after a quiet spell
  goes out immediately. (#49)
- **The frontend entry chunk is 25% smaller.** Views reachable only
  through navigation and the terminal search addon now load on first
  render instead of before first paint, `tailwind-merge` is gone in favor
  of explicit `!`-marked overrides, Vue production feature flags strip the
  unused Options API runtime, and the build targets es2022. (#37)
- **SSH connects no longer freeze the vault.** A connect previously held
  the vault mutex across the whole network exchange, so one host that
  accepted TCP and then went silent blocked unlock, reset and every key
  operation until the app was restarted. The lock is now held only for key
  auth, the connect and authentication exchange are time-bounded with
  keepalives, and the Argon2id KDF runs on a blocking thread with the
  master key cached for the unlocked session — key imports no longer
  derive it twice per operation. (#52)
- The update check now runs once per launch instead of twice: the
  backend's duplicate check emitted an event nothing subscribed to, so the
  renderer path the user actually sees is the single source of update
  notifications. (#66)
- Agent-facing tooling: vendored Greptile PR-review skills (check-pr,
  cli-review, greploop) under `.agents/skills/`, and the comment hygiene
  scanner now works on CRLF checkouts, has its own tests, and runs in CI.
  (#68, #70)

### Fixed

- **A live session id can no longer be hijacked by a second terminal.**
  Reusing an id previously killed the existing local shell or split SSH
  output into a new PTY pane; the collision is now rejected at admission,
  and a cap on concurrent local terminals bounds PTYs, shells and reader
  threads. The slot is reserved before the blocking spawn, so a burst of
  simultaneous requests cannot race past the limit. (#31, #49)
- **A session's final output is never lost to the close notification.**
  The end of the stream now travels in band on the session's own channel,
  so the last batch is delivered before the pane closes — `cat bigfile;
  exit` previously dropped the final batch entirely. A closed pane's
  transcript also finishes on a grace period even if the end-of-stream
  frame never arrives, and a detached PTY reader can no longer emit stale
  output or a stray disconnect into a reused session id. (#49)
- **A second instance can no longer clobber shared app data.** Launching a
  second copy focuses the running window instead of opening a second
  writer that could silently drop a freshly pinned host key. Workspace
  layouts are validated on write, so a corrupt split ratio can no longer
  produce a state file the app cannot start with, and a refused write
  rolls the in-memory store back instead of failing every later save. (#40)
- Importing a key no longer leaves the private key behind in the dialog
  after Cancel, a failed import, or a vault auto-lock; a failed import
  keeps the label and Import tab instead of resetting the whole form. (#58)
- Out-of-range terminal dimensions are rejected at the boundary instead of
  silently wrapping: a request for 65,536 columns produced a zero-column
  PTY locally and went out on the wire unchecked over SSH. (#49, #61)

### Security

- **The vault file now authenticates its own header.** The stored key
  list, salt, version and epoch are bound to the ciphertext as AES-GCM
  associated data, so anyone able to write `vault.json` can no longer swap
  in their own public key under the user's label — editing any of them
  fails the tag check. Decrypted payload memory is zeroized through a
  `SecretText` newtype on every path, and migrating a legacy vault
  rebuilds the key metadata from the private keys themselves rather than
  trusting the unauthenticated old header. (#49)
- **russh is built without zlib, closing an unbounded decompression
  path.** russh 0.46's zlib loop doubled its output buffer with no
  ceiling, so a server advertising zlib alone could drive the client's
  CryptoVec until realloc panicked — and `panic = "abort"` took the whole
  process down. Clavyn never negotiates compression, so the vulnerable
  code is compiled out entirely, and a zlib-only server now gets a connect
  error that names compression as the mismatch. (#29)
- **Vault reset now tears down live sessions.** SSH connections, SFTP
  channels and local PTYs authenticated before a reset stayed writable
  afterwards even though the dialog claimed credentials were permanently
  deleted; they are now closed under the same destructive boundary. (#58)
- **Disabling biometric unlock now requires an unlocked vault.** The
  command previously proved nothing, so any direct IPC caller could
  destroy the vault-bound Keychain credential — and every stored key with
  it — from the lock screen. The webview's unused updater/process
  capability, which permitted downgrades and an unconditional app-kill, is
  removed. (#33)
- **The webview CSP now declares every directive that does not inherit
  from `default-src`** (base-uri, form-action, frame-ancestors,
  child-src), drops the unused GitHub connect-src origins that made
  user-published content a permitted source, and scopes shell open to the
  two URLs the UI actually opens instead of any https URL. (#47)
- Windows state files now carry an explicit owner-only DACL instead of
  inheriting the profile's, and the temporary file is created with
  `CREATE_NEW` so a planted file cannot be written through and renamed
  into place. (#46)
- **The release pipeline is hardened against supply-chain attacks.** All
  CI actions are pinned to commit SHAs, `npm ci` runs without install
  scripts, release permissions are scoped per job, fork PRs can no longer
  reach self-hosted runners, and dispatched release versions must be valid
  semver — closing the paths by which a moved tag, a postinstall hook or
  `$GITHUB_OUTPUT` injection could reach `TAURI_SIGNING_PRIVATE_KEY`. (#42)
- Two unregistered SFTP commands — an unbounded remote read and an
  unconstrained remote write on a caller-supplied path — are deleted
  instead of sitting one line away from exposure to the webview, and tokio
  is built with only the features the workspace uses. (#61)

### Dependencies

- Rust: russh-sftp 3.0, russh-keys 0.49.2, russh-cryptovec 0.62, argon2
  0.6, rand 0.10, base64 0.23, uuid (cargo group). (#80, #82, #83, #84,
  #85, #88, #89)
- Frontend: Vite 8, TypeScript 7, @xterm/addon-fit 0.11 and
  @xterm/addon-webgl 0.19; Playwright 1.63 for e2e. (#75, #78, #79, #90)
- CI: actions/checkout 7, actions/setup-node 7, actions/upload-artifact 7,
  tauri-action 1.0. (#72, #73, #74, #76)

## [0.1.2-alpha.5] - 2026-09-11

### Fixed

- **Tab drag-and-drop now works in the shipped desktop app, not just in
  the test harness.** Tauri's `dragDropEnabled` window flag defaults to
  `true`, which installs a native drag-drop handler on the webview that
  swallows every drag before it reaches the DOM — `dragstart`/`dragend`
  fired, but `dragenter`/`dragover`/`drop` never did, leaving the split,
  swap, and reorder handlers from alpha.4 as unreachable dead code. The
  app registers no `tauri://drag-drop` listeners, so disabling the flag
  removes nothing and fixes the issue on WKWebView, WebView2, and
  WebKitGTK alike. (#34)

## [0.1.2-alpha.4] - 2026-09-10

### Fixed

- **Vault reset is now reachable from the locked unlock screen.** The reset
  button previously lived only inside the unlocked "Danger Zone", which made
  it impossible to reset the vault after forgetting the master passphrase —
  the exact situation reset exists for. The unlock screen now shows a
  "Forgot passphrase? Reset vault" link that opens the same
  passphrase-gated confirmation form; cancelling returns to the normal
  unlock form. (#26)
- **Tab drag-and-drop** now follows the Termius model: dropping a tab onto
  a terminal splits the pane, dragging across tabs swaps them, and the tab
  strip supports reordering. (#27)

### Changed

- The reset confirmation form is extracted into a shared `VaultResetForm`
  component so the locked and unlocked entry points render one
  implementation and cannot drift apart. (#26)

## [0.1.2-alpha.3] - 2026-09-10

### Fixed

- The Windows installer now embeds the WebView2 bootstrapper instead of
  downloading it at install time, so installs work offline and in locked-down
  environments. (#22)
- Reset-form errors are localized, and biometric enrollment state is
  reconciled when a vault reset fails partway through. (4ea641e)

## [0.1.2-alpha.2] - 2026-09-10

### Added

- **Vault reset with passphrase-gated authorization.** A "Danger Zone"
  section in the vault view lets users wipe the vault after re-entering
  their master passphrase, recovering from a forgotten or corrupted vault
  without reinstalling. (#25)

### Security

- Persisted state files are written atomically with owner-only permissions,
  so a crash mid-write can no longer leave a truncated file behind. (#21)
- The app fails closed when persisted state cannot be parsed instead of
  silently falling back to defaults. (#20)
- `Cargo.lock` is now tracked and dependencies are audited in CI. (#23)

## [0.1.2-alpha.1] - 2026-09-09

### Added

- **Termius-style drag-and-drop pane management:** terminal panes can be
  dragged to split, swap, or extract them into new panes. (#11)
- Agent verification CLI and a feature map for driving the live UI over
  Chrome DevTools Protocol, enabling scripted UI regression checks. (#8)

### Fixed

- Terminal search now highlights matches and shows the result count. (#10)

### Changed

- Host addresses are masked by default so they are not exposed at a glance
  on shared screens; hovering reveals them. (#9)
- Completed the OpenTermius → Clavyn rename and documented the unsigned-build
  warning workaround (`xattr -cr`). (38c6d3d)

[0.1.2-alpha.6]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.6
[0.1.2-alpha.5]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.5
[0.1.2-alpha.4]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.4
[0.1.2-alpha.3]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.3
[0.1.2-alpha.2]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.2
[0.1.2-alpha.1]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.1
