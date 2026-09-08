# Hosts

The landing view. Lists SSH hosts (from the vault), supports search, grouping,
add/edit/delete, and connecting (double-click opens a terminal session).

## Sub-features

- host-list: searchable list of hosts with label/hostname/user/tags.
- host-search: text filter that resolves the effective auth identity.
- host-groups: collapsible groups (add/delete group, filter by group).
- host-form: add/edit dialog (`HostForm.vue`) — label, hostname, port,
  username, auth method, identity link, tags, group.
- host-connect: double-click a host → opens a terminal pane (SSH via fixture).

## How to get to it (user POV)

It's the default view on launch. Click `Hosts` in the sidebar, or `Cmd/Ctrl+K`
→ "Go to Hosts". Search box filters by label/hostname/username.

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs home
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs components --pretty

    # Connect the Atlas fixture host (double-click by visible label)
    node scripts/verify/control-clavyn.mjs connect "Atlas Production"
    node scripts/verify/control-clavyn.mjs network-summary --pretty   # expect a connect_ssh call

    # Add a host via the form
    node scripts/verify/control-clavyn.mjs click --name "Add Host"
    node scripts/verify/control-clavyn.mjs snapshot --pretty           # dialog open
    # ... type into fields, then click Save

## Gotchas

- The fixture returns `atlas` and `orion` only. New hosts added via the form
  are held in the Pinia store but `add_host` is a no-op in the fixture.
- Search resolves the **effective** identity username, not the host's stored
  fallback — verify with `network-log` that `connect_ssh` sees the resolved
  username when an identity is linked.
- Double-clicking a host navigates to the Terminal view automatically.
