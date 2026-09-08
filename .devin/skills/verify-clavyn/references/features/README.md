# Clavyn — Feature Map

A compact, searchable map of every major feature in Clavyn, what it does, how
to reach it from the UI, and the `control-clavyn` commands to drive it. Use
this instead of re-reading components each time — it saves context tokens.

The app is a single-window Tauri 2 + Vue 3 desktop app. A persistent left
sidebar (`AppSidebar`) switches between views. A command palette
(`Cmd/Ctrl+K`) jumps anywhere. The renderer is driven via
`control-clavyn.mjs` against the Vite dev server with `tauri-fixture.js`
mocking Tauri IPC.

## Views at a glance

| View | Sidebar label | Purpose | Detail |
|------|---------------|---------|--------|
| Hosts | `Hosts` | List/search/group SSH hosts; connect or edit | [hosts.md](hosts.md) |
| Terminal | `Terminal` | Tabbed + split terminal panes (SSH or local) | [terminal.md](terminal.md) |
| Files | `Files` | SFTP browser for a connected host | [files.md](files.md) |
| Workspaces | `Workspaces` | Save/restore tab+pane layouts per project | [workspaces.md](workspaces.md) |
| Identities | `Identities` | SSH identities (username/auth source) | [identities.md](identities.md) |
| Keys | `Keys` | Generate/import/delete SSH key pairs | [keys.md](keys.md) |
| Known Hosts | `Known Hosts` | TOFU known_hosts review + removal | [known-hosts.md](known-hosts.md) |
| Vault | `Vault` | Encrypted vault init/unlock/lock + biometric | [vault.md](vault.md) |
| Settings | `Settings` | App info, auto-lock, update check/install | [settings.md](settings.md) |

## Cross-cutting features

| Feature | How to open | Detail |
|---------|-------------|--------|
| Command Palette | `Cmd/Ctrl+K` | [command-palette.md](command-palette.md) |
| Update banner/modal | Auto on startup; `Cmd/Ctrl+U` | [settings.md](settings.md) |
| Vault unlock modal | Auto when vault locked + action needs it | [vault.md](vault.md) |
| Keyboard shortcuts | `Cmd/Ctrl+N` new tab, `,` settings, `K` palette | [keyboard.md](keyboard.md) |

## Driving conventions

- **Navigate to a view:** `node control-clavyn.mjs navigate <view>` (view ids:
  `hosts terminal files workspaces identities keys known-hosts vault settings`).
- **Inspect before acting:** `snapshot --pretty` gives the a11y tree;
  `components --pretty` lists `data-*`/roles/labels.
- **Click by visible text:** `click --name "Add Identity"`.
- **Terminal interaction:** `new-session`, `connect "<label>"`, `send "<text>"`.
- **Evidence:** `screenshot /tmp/x.png` + `console --pretty` (must be clean).

## Fixture hosts (always available in the mocked renderer)

- `atlas` — "Atlas Production", `atlas.example.test:22`, user `deploy`, auth `agent`
- `orion` — "Orion Staging", `orion.example.test:22`, user `deploy`, auth `agent`

These come from `desktop/e2e/tauri-fixture.js`. They are not real hosts; the
fixture mocks the SSH transport and echoes typed input back.
