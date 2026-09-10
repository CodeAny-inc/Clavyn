---
name: release-clavyn
description: Cut a Clavyn release end-to-end — maintain CHANGELOG.md (Keep a Changelog), bump the version across Cargo.toml/package.json/tauri.conf.json, run the verification suite, tag, and publish a signed multi-platform GitHub release via the tag-driven CI workflow with rich release notes. Use whenever asked to release, ship, tag, bump the version, write a changelog, or draft release notes for Clavyn.
license: Proprietary. LICENSE.txt has complete terms
compatibility: Requires git, gh CLI (authenticated), cargo, Node.js/npm. Signing happens in CI via the TAURI_SIGNING_PRIVATE_KEY repo secret; a local key is only needed for scripts/release.sh.
metadata:
  author: CodeAny-inc
  version: "1.0"
  repository: https://github.com/CodeAny-inc/Clavyn
allowed-tools: Bash(git:*) Bash(gh:*) Bash(cargo:*) Bash(npm:*) Bash(npx:*) Bash(node:*) Bash(./scripts/*:*) Read Grep Glob Write Edit
---

# release-clavyn — releasing Clavyn with a proper changelog

Use this skill for every Clavyn release. It encodes the release mechanics
(version bump, tag, CI signing, update manifest) and the changelog discipline
(Keep a Changelog + semver + notes that explain *why*, not just *what*).

## Choose the release path first

There are two release paths. **Prefer CI** — it is the only path that signs
with the real updater key (stored as the `TAURI_SIGNING_PRIVATE_KEY` GitHub
secret) and builds all four platforms.

| Path | When | Produces |
|------|------|----------|
| **Tag push → `.github/workflows/release.yml`** | Default. Always works. | macOS arm64 + x64 DMGs, Linux deb/rpm/AppImage, Windows NSIS exe, `.sig` files, `latest.json` update manifest |
| `scripts/release.sh` | Only for a local macOS build when `TAURI_SIGNING_PRIVATE_KEY` (or `~/.config/clavyn/updater-private.key`) exists. Check `~/.tauri/` for a key whose `.pub` matches the `pubkey` in `desktop/src-tauri/tauri.conf.json` — a key from another project will produce unusable signatures. | Local `.app`, DMG, signed tarball, `latest.json` |

GitHub secrets are write-only — you cannot read `TAURI_SIGNING_PRIVATE_KEY`
back out. If no local key exists, the CI path is the correct path, not a
fallback.

## Release workflow

### 1. Survey what is shipping

    git fetch origin
    git status                      # must be clean and on main
    git tag -l | tail               # find the previous tag
    git log --oneline <prev-tag>..origin/main

Read each merged PR (`gh pr view <n>`) for the *why*. Changelog entries must
explain user-visible impact, not echo commit titles.

**Watch for a moving target.** If `origin/main` advanced past your local
HEAD, release the newest main: `git rebase origin/main` after your release
commit, then re-create the tag so it points at the rebased commit —
*before* pushing the tag. Never tag a commit that isn't on top of main.

### 2. Write the changelog entry

Maintain `CHANGELOG.md` at repo root in
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format:

- One `## [version] - YYYY-MM-DD` section per release, newest first.
- Group under `### Added`, `### Fixed`, `### Changed`, `### Security`,
  `### Removed` — omit empty groups.
- Lead with the user-facing benefit; mention the mechanism second. Bold the
  headline of the most important entries.
- Reference PRs as `(#N)`; bare commit SHAs only when there is no PR.
- Get release dates right: `git log -1 --format=%ad --date=short <tag>` on
  past tags — never guess.
- Keep link definitions at the bottom:
  `[x.y.z]: https://github.com/CodeAny-inc/Clavyn/releases/tag/vx.y.z`

### 3. Bump the version

    ./scripts/version.sh 0.1.2-alpha.4   # or: alpha | beta | patch | minor | major | stable

This updates `Cargo.toml`, `desktop/package.json`, and
`desktop/src-tauri/tauri.conf.json` together.

### 4. Verify before tagging

    cargo check --workspace
    cd desktop && npx vue-tsc --noEmit
    cd desktop && npx vitest run
    node scripts/verify/check-comments.mjs

All four must pass. The tag is the point of no return for what ships.

### 5. Commit, tag, push

    git add -A && git commit -m "Release v0.1.2-alpha.4"
    git push origin main             # remote may have moved — rebase first
    git tag v0.1.2-alpha.4
    git push origin v0.1.2-alpha.4

Commit as the configured human author only — no AI attribution or
co-author trailers.

### 6. Watch the workflow

    gh run list --workflow release.yml --limit 1
    gh run watch <run-id> --exit-status

The `prepare` → 4× `build` → `publish-manifest` pipeline takes ~10 minutes.
`publish-manifest` fails if any platform's `.sig`/asset is missing — check
its log first on failure.

### 7. Replace the placeholder release notes

The workflow creates the release with a one-line body. Rewrite it with the
changelog content:

    gh release edit v0.1.2-alpha.4 --notes "$(cat <<'EOF'
    ## What's Changed

    <changelog section for this version, prose paragraphs for headline
    changes, bullets for the rest>

    See [CHANGELOG.md](https://github.com/CodeAny-inc/Clavyn/blob/main/CHANGELOG.md)
    for the full history.

    **Full Changelog**: https://github.com/CodeAny-inc/Clavyn/compare/v<prev>...v<new>
    EOF
    )"

### 8. Verify the release

    gh release view v0.1.2-alpha.4 --json isPrerelease,assets \
      --jq '{prerelease: .isPrerelease, assets: [.assets[].name]}'

Confirm: `isPrerelease=true` for alpha/beta/rc, and assets include all
platform installers, their `.sig` files, `*.app.tar.gz` (macOS updater
payloads), and `latest.json`. Without `latest.json` the auto-updater cannot
see the release — the custom `check_with_prerelease_endpoint` in
`commands.rs` picks the highest version that has one, including prereleases.

## If the release commit missed a changelog item

When a PR lands between your changelog commit and the tag, add the entry to
`CHANGELOG.md` on main as a normal follow-up commit (`"Document X in
<version> changelog"`), and make sure the GitHub release notes include it.
Do not amend or force-push the tagged release commit.

## Gotchas

- `scripts/release.sh` prompts interactively (`read`) on a dirty tree —
  commit first or use `--dry-run` to preview.
- `release.sh --notes` content is embedded into `latest.json` via a Python
  triple-quoted string — never put `'''` in notes.
- `createUpdaterArtifacts: true` in `tauri.conf.json` is what produces the
  signed `.app.tar.gz`/installer `.sig` files the updater needs — do not
  disable it.
- The app is ad-hoc signed on macOS; users may need
  `xattr -cr /Applications/Clavyn.app`. That is expected for unsigned
  builds, not a release failure.
- `main` is a protected branch; direct pushes may be bypassed by permission
  — prefer this only for release commits, not regular work.

## Rules

- Never tag without a CHANGELOG.md entry for that version.
- Never publish a release whose notes are the workflow placeholder.
- Never guess changelog dates or PR contents — read the tags and PRs.
- Never expose or request the signing key's value; CI holds it.
- Never attribute commits to an AI agent.
