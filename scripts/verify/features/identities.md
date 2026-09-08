# Identities

SSH identities — a named username + auth source (agent, key, password) that
can be linked to a host so the effective auth is resolved at connect time.

## Sub-features

- identity-list: list of identities with label/username/auth.
- identity-form: add/edit dialog — label, username, auth method, key link.
- identity-groups: group identities (add/delete group).
- identity-link: hosts reference an identity via `identity_id`; the connect
  path resolves the identity's username/auth.

## How to get to it (user POV)

Click `Identities` in the sidebar. "Add Identity" to create; click an identity
to edit; trash icon to delete.

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs navigate identities
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs click --name "Add Identity"
    node scripts/verify/control-clavyn.mjs snapshot --pretty        # dialog open

## Gotchas

- The fixture's identity extension (see `terminal-session-creation.mjs`)
  links `atlas` to a `fixture-identity` (username `root`). The base fixture
  returns an empty identity list — extend the fixture in-test if you need a
  linked identity to verify the effective-username resolution path.
- `effectiveSshIdentity()` resolves the identity at connect time; verify via
  `network-log` that `connect_ssh` receives the identity's username, not the
  host's stored fallback.
