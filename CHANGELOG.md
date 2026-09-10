# Changelog

All notable changes to Clavyn are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.2-alpha.4]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.4
[0.1.2-alpha.3]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.3
[0.1.2-alpha.2]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.2
[0.1.2-alpha.1]: https://github.com/CodeAny-inc/Clavyn/releases/tag/v0.1.2-alpha.1
