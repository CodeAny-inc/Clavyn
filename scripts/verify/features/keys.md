# Keys

Generate, import, and delete OpenSSH key pairs. Public keys can be copied/
revealed. Private material is held in the vault (encrypted) — never shown.

## Sub-features

- key-list: list of keys with label, type, fingerprint.
- key-generate: "Generate Key" dialog — label → core generates a keypair.
- key-import: "Import Key" dialog — paste an OpenSSH private key (+ optional
  passphrase) or pick a file via the native dialog.
- key-public: reveal/copy the public key for a key.
- key-delete: remove a key.

## How to get to it (user POV)

Click `Keys` in the sidebar. "Generate Key" or "Import Key" buttons. Click a
key's eye icon to reveal its public key; copy icon to copy; trash to delete.

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs navigate keys
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs click --name "Generate Key"
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    # ... type label, confirm
    node scripts/verify/control-clavyn.mjs network-summary --pretty   # generate_key call

## Gotchas

- The native file dialog (`@tauri-apps/plugin-dialog` `open`) is **not**
  mocked by the base fixture. Importing from a file will hang/fail in the
  harness; use the paste-textarea import path instead, or extend the fixture.
- Private key material is never rendered. Assert only on public keys /
  metadata (`list_keys` returns `KeyMeta` without private material).
