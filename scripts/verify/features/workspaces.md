# Workspaces

Save and restore tab + split-pane layouts per project. A workspace captures
which hosts are open in which panes/tabs so you can switch contexts.

## Sub-features

- workspace-list: list of saved workspaces; set active.
- workspace-create: name + create.
- workspace-save: persist current tab/pane layout into a workspace.
- workspace-delete: remove a workspace.
- workspace-restore: "Play" a workspace to open its saved layout.

## How to get to it (user POV)

Click `Workspaces` in the sidebar. "New Workspace" to create; "Save" to
persist the current terminal layout; the play button to restore.

## Driving it with control-clavyn

    node scripts/verify/control-clavyn.mjs navigate workspaces
    node scripts/verify/control-clavyn.mjs snapshot --pretty
    node scripts/verify/control-clavyn.mjs click --name "New Workspace"
    # ... type name, confirm
    node scripts/verify/control-clavyn.mjs network-summary --pretty   # create_workspace / save_workspace

## Gotchas

- Workspaces reference host ids; restoring a workspace whose hosts no longer
  exist will skip those panes.
- The fixture returns an empty workspace list initially; created workspaces
  live in the Pinia store (fixture `create_workspace`/`save_workspace` are
  no-ops that don't persist).
