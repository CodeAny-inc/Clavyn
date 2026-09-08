# Vault

The encrypted vault holds all secrets (host credentials, private keys). It's
AES-256-GCM with an Argon2id KDF. The passphrase lives in the OS keychain
(macOS) or is entered manually. The vault auto-locks after a configurable
idle timeout.

## Sub-features

- vault-status: sidebar Vault button shows locked (gray) / unlocked (green).
- vault-init: first-run creates the vault (passphrase + confirm).
- vault-unlock: enter passphrase to unlock; `VaultUnlockModal` pops when a
  locked-vault action is attempted.
- vault-lock: explicit lock; also auto-lock on idle (`useAutoLock`).
- vault-biometric: Touch ID / platform biometric unlock (macOS). Enable/disable
  storing the passphrase in the keychain.

## How to get to it (user POV)

Click `Vault` in the sidebar (bottom section). If not initialized, you're
prompted to create it. If locked, enter passphrase (or use biometric on macOS).
"Lock" button to lock manually.

## Driving it with control-clavyn

    node control-clavyn.mjs navigate vault
    node control-clavyn.mjs snapshot --pretty
    node control-clavyn.mjs info --pretty            # vault status via store
    # The fixture reports vault_is_initialized=true, is_vault_unlocked=true,
    # biometric_available=false — so the unlocked state is the default.

## Gotchas

- The fixture **always reports the vault as initialized + unlocked** and
  biometric unavailable. To test the locked/init flows, extend the fixture to
  return `false` for `vault_is_initialized`/`is_vault_unlocked`.
- `VaultUnlockModal` is a global overlay mounted in `App.vue`; it appears when
  a locked-vault action is attempted. Watch for it with `snapshot` (a
  `role: dialog` named "Unlock vault").
- Sensitive form state is cleared synchronously on lock/unlock transitions
  (`clearSensitiveFormState`); don't assert passphrase text persists.
- Biometric is macOS-only and not available in the harness environment.
