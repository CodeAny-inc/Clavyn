# Changelog

All notable changes to Clavyn are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2-alpha.7] - 2026-11-15

### Added

- **Appearance setting with a real dark mode.** Settings → Appearance →
  Theme offers System (the default), Light and Dark, and "System" follows
  `prefers-color-scheme` live. The dark palette had shipped in the
  stylesheet from the start but nothing ever applied the class, so the app
  rendered light whatever the OS was set to; dark-theme switches and red
  text are corrected with it. (#187)
- **The command palette reaches hosts, Files and the vault lock.** Searching
  a host name offers "Connect to <label>", which opens a terminal exactly as
  double-clicking the host does; Files and Lock vault are commands too, and
  the results render as a real `listbox` a screen reader can navigate. (#188)
- **Keyboard-interactive SSH sign-in.** Password hosts first probe which
  methods the server accepts, as OpenSSH does, then use `password` when
  offered and `keyboard-interactive` otherwise — answered the way PAM
  prompts a password — so servers that only offer keyboard-interactive now
  connect. Rejected authentication names the method that failed, and each
  resolved address is dialed with its own timeout. (#177)

### Changed

- **Vault unlock is about 3.5x faster.** Argon2id's four lanes now compute
  in parallel (unlock 194 ms → 56 ms). Parameters and the cost to an
  attacker are unchanged; only the wait is. (#185)
- Terminal panes render as one warm surface with inset output and a tuned
  palette; the active pane mark is subtle instead of a blue ring, and the
  empty Terminal view is readable in every theme with a centered New
  session button. (#183, #184)
- Ctrl+F, Ctrl+K and Ctrl+arrows pass through to the shell on Linux and
  Windows instead of triggering pane/find/palette shortcuts; macOS is
  unchanged. (#180)
- Host, identity and workspace lists tell a failed or in-flight load apart
  from genuinely empty — a failed first read no longer masquerades as "No
  hosts yet", and it gets a retry. (#179)
- Row actions show on keyboard focus and touch screens instead of only on
  hover, unlabeled controls have accessible names, and host key
  fingerprints are selectable text. (#182)

### Fixed

- **Destructive actions actually ask.** `window.confirm` never blocks in the
  packaged app — the dialog plugin replaces it with an always-truthy async
  stub and the capability refused the native dialog — so deleting a vault
  key, host, identity, known host, workspace or remote file took a single
  click, and an SFTP transfer overwrote an existing file silently.
  Destructive confirmations now render in the page, deleting a key warns
  that it is permanent, and removing a known host masks the address.
  (#172)
- **Dialogs manage keyboard focus.** Opening a dialog focuses its first
  field, Tab is trapped inside, Escape closes it and returns focus to the
  opener, and a click on the backdrop no longer throws away a filled-in
  form. (#181)
- **App-wide shortcuts no longer act on the page behind an open modal** —
  Ctrl+N cannot open a tab under the unlock prompt and Ctrl+, cannot
  unmount a filled form; the chord is claimed so the webview's own binding
  does not fire either. (#196)
- **Terminal output can no longer grow memory without bound.** A producer
  faster than the webview — a fast server or a local `cat` — grew the IPC
  queue until the app ran out of memory. Every session now has a 2 MiB
  window of unacknowledged output that holds the producer back. (#159)
- A download never leaves a partial file under its name: writes stage
  through `.<name>.<uuid>.part`, fsync, and rename into place, so a failed
  or cancelled download leaves the original file alone; uploads widen
  their writes. (#142)
- Session maps are no longer held locked across network I/O, so a peer
  that stops reading stalls only its own session instead of every SSH and
  SFTP operation. (#140)
- Closing a local shell tab ends and reaps the process (SIGHUP on Unix,
  TerminateProcess on Windows, `wait()` off the caller's thread), and the
  unused os and process plugins are dropped. (#154)
- SFTP entry actions act on the directory the entry was picked from —
  navigating mid-action no longer deletes, renames or downloads from the
  wrong folder — and actions captured from a since-closed session are
  refused. (#163)
- Host, identity, workspace and vault forms are guarded against double
  submits and surface their failures; retrying an identity save links the
  key it already imported instead of creating another. (#165)
- Auto-lock counts terminal typing and scrolling as activity even when
  xterm stops the keys, error text masks configured hostnames, and UI
  state stays honest after failures. (#167)
- A held host key change is withdrawn once the pinned key answers again,
  so one click can no longer pin a key the server has stopped using. (#138)
- An identity or group delete landing during the first load no longer
  leaves the host list empty. (#195)
- Terminal geometry shares one bounds-checked creation path, and dead
  components and stale docs are removed. (#161)
- Key import caps bcrypt rounds before deriving and decrypts once instead
  of twice, and keychain calls no longer run inside the vault lock. (#170)
- The verification harness probes the dev server and waits for the GPU
  renderer correctly in full-suite e2e runs. (#171)

### Security

- **The webview can no longer choose where vault keys sign in.**
  `connect_ssh` and `sftp_connect` take a saved host id the backend
  resolves instead of a `Host` object supplied by the page, and a store
  write that would let a vault key reach a new server now asks first —
  the first-seen host key is still never pinned without confirmation.
  (#99, #120, #122)
- **Every app command sits behind the capability ACL**, plugin grants are
  trimmed to what the UI uses, and the window cannot be navigated off the
  app's own pages. (#123)
- **SSH and SFTP refuse a session id that is already in use**, closing the
  path that let a new session split or steal a live one's input and
  output. (#100, #120)
- **Host key pins normalize equivalent spellings of a host**, so a new
  spelling of a pinned host is not treated as a first use. (#101, #120)
- **The app fails closed on state it cannot check.** An unreadable state
  file lands on the recovery screen instead of looking absent, a vault
  file rolled back to an older copy is recognized through a high-water
  mark kept outside the file, and a vault is never created over an
  existing one. (#102, #106, #120, #174)
- **Key metadata is listed only while the vault is unlocked** — a locked
  vault no longer serves attacker-rewritten header entries to the UI.
  (#105, #120)
- **Decrypted key material is wiped on every path.** Vault saves serialize
  private keys into a zeroizing buffer that wipes on growth and drop,
  `get_key` and key generation return zeroizing types, and imported keys
  are stored without their passphrase. (#107, #152)
- **Server-initiated SSH channels are refused** — agent forwarding, X11,
  forwarded and direct tcpip/streamlocal, and types russh has no hook
  for — and a host's startup command runs in the shell only behind a
  native confirmation. (#148)
- **Terminal input is hardened against hostile content:** pastes are
  sanitized before xterm sees them, an OSC 8 link shows its target in a
  chip while hovered, and input reordering cannot inject bytes. (#150)
- **A server-chosen file name can no longer disguise itself** in the save
  dialog or the Files list: bidirectional controls, line/paragraph
  separators and path tricks are stripped. (#155)
- **Update signatures are bound to the version they were signed for**
  (`requireSignedVersion`), so a signature from an older release cannot
  pair with a newer manifest, and manifests are only taken from this
  repository's releases. (#144)
- **The updater key signs in isolation.** The `sign` job checks out no
  code, installs nothing, runs a signer binary whose digest a separate job
  produced, and verifies every `.sig` against the declared updater pubkey
  before upload; release builds use `--locked`, every CI job is
  time-bounded, and every release publishes `SHA256SUMS`. (#108, #109,
  #146)
- Windows state lives in the non-roaming app data directory with
  owner-only ACLs, and the biometric enrollment marker writes through the
  same hardened private writer as every other state file. (#103, #104,
  #120)
- **russh 0.65.1** fixes RUSTSEC-2026-0495 (a path Clavyn never runs, but
  the audit suppression is now removed rather than carried), and the host
  key is checked at the point where its algorithm is known. (#194)

### Dependencies

- Rust: russh 0.63 → 0.65.1, thiserror 2, aes-gcm 0.11, argon2 0.6 with
  the parallel feature, rand 0.10, plus the cargo minor-and-patch groups
  and a `cargo audit` back to green — the three `zip` extraction
  advisories the updater pins the tree to are suppressed with a review
  date. (#98, #121, #128, #185, #186, #193, #194, #197)
- Frontend: @xterm/xterm 6, pinia 4, vitest 5, vue-tsc 3, @tauri-apps/api
  2.12.4, tailwind moved to devDependencies (clearing the npm audit),
  happy-dom 21. (#98, #121, #128, #135, #191)
- CI: setup-node 7.1.0, upload-artifact 7.0.2, download-artifact 8.0.2,
  rust-toolchain and rust-cache bumps, Playwright 1.65 for e2e.
  (#129, #130, #131, #132, #133, #190)

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

[0.1.2-alpha.7]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.7
[0.1.2-alpha.6]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.6
[0.1.2-alpha.5]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.5
[0.1.2-alpha.4]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.4
[0.1.2-alpha.3]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.3
[0.1.2-alpha.2]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.2
[0.1.2-alpha.1]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.1
