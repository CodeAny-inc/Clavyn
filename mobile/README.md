# Mobile (iOS / iPadOS / Android)

Status: **planned, not yet implemented.** Tracked for after the desktop MVP.

## Strategy

The shared `clavyn-core` Rust crate compiles to each mobile target via
FFI. The UI is native (or React Native / Flutter — to be decided) and calls
into the core through a generated binding layer.

## Recommended bridge

- **iOS / iPadOS**: `swift-uniffi` or `cbindgen` + a thin Swift wrapper.
  Core built as a `staticlib`/`xcframework`. SSH transport uses `russh`
  (pure Rust, no process spawning required by iOS sandbox).
- **Android**: `jnigen` / `uniffi-rs` -> Kotlin bindings, or
  `flutter_rust_bridge` if Flutter is chosen. Core built as a `.so` per ABI
  (arm64-v8a, armeabi-v7a, x86_64).

## Why this works on mobile

`russh` is a pure-Rust SSH implementation — no dependency on the system
`ssh` binary, which iOS does not allow apps to spawn. This is the same
library the desktop app uses, so transport behavior is identical across
platforms.

## Secrets on mobile

- iOS: Keychain Services (`Security` framework) for the vault passphrase.
- Android: Android Keystore for the vault passphrase / master key.

Never store private keys or passphrases in app preferences or plain files.

## What lives here eventually

```
mobile/
  ios/            # Xcode project, Swift bindings
  android/        # Gradle project, Kotlin bindings
  bindings/       # uniffi .udl + generated code
```
