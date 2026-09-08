# Known Hosts

TOFU (trust-on-first-use) known_hosts review. First connection records a host
key; subsequent connections compare. Mismatches hard-fail and surface to the
user — never auto-accepted.

## Sub-features

- known-host-list: list of recorded host keys (host, port, key type, fingerprint).
- known-host-remove: per-entry "Remove known host" (aria-label) with a confirm.

## How to get to it (user POV)

Click `Known Hosts` in the sidebar. Each row shows a host:port and its key
fingerprint. The trash icon removes an entry (after a browser confirm).

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs navigate known-hosts
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs components --pretty
    # Removal triggers a native confirm() — see gotchas.
    node scripts/verify/control-clavyn.mjs network-summary --pretty   # list_known_hosts

## Gotchas

- The base fixture returns an empty known-hosts list. To test removal, extend
  the fixture to return entries, or seed via core tests.
- Removal uses `window.confirm()`, which Playwright auto-dismisses by default.
  To handle it, add a dialog handler in the harness (extend `control-clavyn`
  if you need this regularly) or use `eval` to pre-stub `window.confirm`.
