# Files (SFTP Browser)

Browse a connected host's filesystem over SFTP. Lazily mounted once (preserves
SFTP state when hidden), but purges transient credentials when not visible.

## Sub-features

- sftp-connect: "Connect to SFTP" prompt shown when no SFTP session is open.
- directory-listing: list of files/dirs with name, size, permissions, mtime.
- navigation: breadcrumb / path navigation, canonicalize home dir.
- file-ops: read/write files, create/remove dirs, rename, remove files.

## How to get to it (user POV)

Click `Files` in the sidebar. If no SFTP session is open, you see "Connect to
SFTP". The fixture's `sftp_list_dir` returns `deployments/`, `logs/`,
`README.md` under `/srv/atlas`.

## Driving it with control-clavyn

    node control-clavyn.mjs navigate files
    node control-clavyn.mjs snapshot --pretty
    node control-clavyn.mjs click --name "Connect to SFTP"
    node control-clavyn.mjs wait-settle
    node control-clavyn.mjs components --pretty        # directory entries
    node control-clavyn.mjs network-summary --pretty   # sftp_* calls

## Gotchas

- The view is `v-show`-toggled (lazy mounted once). Navigating away keeps
  SFTP state; `filesOpened` stays true for the session.
- Fixture returns a fixed `/srv/atlas` listing — don't assert dynamic file
  contents. Assert that `sftp_list_dir` / `sftp_canonicalize` were invoked.
- Connect to SFTP requires a host; the fixture makes `sftp_connect` a no-op
  that succeeds immediately.
