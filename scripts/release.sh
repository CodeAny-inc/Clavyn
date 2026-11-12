#!/usr/bin/env bash
# release.sh — build, sign, and publish a release to GitHub.
#
# This script automates the entire local release process:
#   1. Verify prerequisites (signing key, gh auth, clean git state)
#   2. Bump version (optional, or use current)
#   3. Run checks (cargo check, typecheck, tests)
#   4. Build the Tauri app (release mode)
#   5. Create DMG (macOS) and update tarball
#   6. Sign the update tarball
#   7. Generate latest.json
#   8. Commit, tag, push
#   9. Create GitHub release with all assets
#
# Usage:
#   ./scripts/release.sh                    # release current version
#   ./scripts/release.sh alpha              # bump alpha then release
#   ./scripts/release.sh 0.2.0              # bump to 0.2.0 then release
#   ./scripts/release.sh --skip-checks      # skip cargo check / tests
#   ./scripts/release.sh --dry-run          # show what would happen
#   ./scripts/release.sh --notes "..."      # custom release notes
#
set -euo pipefail

CALLER_DIR="$PWD"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# --- config ---
REPO="CodeAny-inc/Clavyn"
PRODUCT_NAME="Clavyn"
_signing_key_default="$HOME/.config/clavyn/updater-private.key"
SIGNING_KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$_signing_key_default}"
# A relative key path means relative to where the script was started, not to
# the repository root it changes into, so it is made absolute here.
case "$SIGNING_KEY_PATH" in
  /*) ;;
  *) SIGNING_KEY_PATH="$CALLER_DIR/$SIGNING_KEY_PATH" ;;
esac
# The key and its password are held in unexported shell variables and the
# exported copies are dropped, so the environments of npm, vite, cargo build
# scripts and the bundler never carry them; only the signer command in step 6
# receives them. That code still runs as the current user and can read a key
# file under $HOME, which is why a key kept on disk needs a password.
SIGNING_KEY="${TAURI_SIGNING_PRIVATE_KEY:-}"
SIGNING_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY_PATH
BUNDLE_DIR="$ROOT/target/release/bundle/macos"
PLATFORM="darwin-aarch64"
ARCH="aarch64"

# --- parse args ---
VERSION_ARG=""
SKIP_CHECKS=false
DRY_RUN=false
CUSTOM_NOTES=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-checks) SKIP_CHECKS=true; shift ;;
    --dry-run)     DRY_RUN=true; shift ;;
    --notes)       CUSTOM_NOTES="$2"; shift 2 ;;
    -h|--help)
      cat <<'USAGE'
release.sh — build and publish a release to GitHub

Usage:
  ./scripts/release.sh                    Release current version
  ./scripts/release.sh alpha              Bump alpha, then release
  ./scripts/release.sh 0.2.0              Set version, then release
  ./scripts/release.sh --skip-checks      Skip cargo check / tests
  ./scripts/release.sh --dry-run          Show steps without executing
  ./scripts/release.sh --notes "..."      Custom release notes

Environment:
  TAURI_SIGNING_PRIVATE_KEY_PATH  Path to signing key (default: ~/.config/clavyn/updater-private.key)
  TAURI_SIGNING_PRIVATE_KEY       Base64 signing key (if set, overrides path)
USAGE
      exit 0
      ;;
    *)
      VERSION_ARG="$1"
      shift
      ;;
  esac
done

# --- helpers ---

log()  { echo -e "\033[1;34m▶\033[0m $*"; }
ok()   { echo -e "\033[1;32m✓\033[0m $*"; }
warn() { echo -e "\033[1;33m⚠\033[0m $*"; }
err()  { echo -e "\033[1;31m✗\033[0m $*" >&2; }
die()  { err "$*"; exit 1; }

run() {
  if $DRY_RUN; then
    echo "  [dry-run] $*"
  else
    "$@"
  fi
}

get_version() {
  "$ROOT/scripts/version.sh" get
}

if command -v shasum &>/dev/null; then
  SHA256=(shasum -a 256)
else
  SHA256=(sha256sum)
fi

# One digest over the node binary and every file under
# desktop/node_modules/@tauri-apps, which holds the CLI's JavaScript entry
# point and the native binary it loads. Paths are part of each line, so a
# renamed, added or removed file changes it too.
signer_digest() {
  {
    "${SHA256[@]}" "$NODE_BIN"
    find -L "$ROOT/desktop/node_modules/@tauri-apps" -type f -print0 \
      | LC_ALL=C sort -z | xargs -0 "${SHA256[@]}"
  } | "${SHA256[@]}"
}

# --- step 1: prerequisites ---

log "Checking prerequisites..."

# gh CLI
if ! command -v gh &>/dev/null; then
  die "gh CLI not found. Install from https://cli.github.com"
fi

# gh auth
if ! gh auth status &>/dev/null; then
  die "Not authenticated with gh. Run: gh auth login"
fi
ok "gh authenticated"

# Signing key
if [[ -z "$SIGNING_KEY" ]] && [[ ! -f "$SIGNING_KEY_PATH" ]]; then
  die "Signing key not found. Set TAURI_SIGNING_PRIVATE_KEY env var or create $SIGNING_KEY_PATH"
fi
ok "Signing key available"
if [[ -z "$SIGNING_KEY" ]] && [[ -z "$SIGNING_KEY_PASSWORD" ]]; then
  warn "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is empty, so $SIGNING_KEY_PATH has no password: code the checks and the build run can read that file and sign with it"
fi

# Signer
# The update tarball is signed by the Tauri CLI installed in desktop/, run by
# the node binary itself rather than through npx, a `.bin` link or a version
# manager's shim, so nothing is fetched and nothing stands in between. The
# checks and the build below run code from npm packages and cargo build
# scripts as the current user, and any of it could rewrite that CLI before
# step 6 hands it the key. It is digested here, before any of that code runs,
# and step 6 refuses to sign if the digest has changed.
NODE_BIN="$(node -p 'process.execPath' 2>/dev/null)" || die "node not found"
SIGNER_JS="$ROOT/desktop/node_modules/@tauri-apps/cli/tauri.js"
if [[ ! -f "$SIGNER_JS" ]]; then
  die "Tauri CLI not installed in desktop/. Run: cd desktop && npm ci"
fi
SIGNER_DIGEST="$(signer_digest)"
ok "Signer digested"

# Git clean state (allow untracked)
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  warn "Git working tree has uncommitted changes"
  if [[ "$DRY_RUN" == "false" ]]; then
    read -rp "Continue anyway? (y/N) " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || die "Aborted"
  fi
fi
ok "Git state checked"

# --- step 2: version bump ---

if [[ -n "$VERSION_ARG" ]]; then
  log "Bumping version: $VERSION_ARG"
  run "$ROOT/scripts/version.sh" "$VERSION_ARG"
fi

VERSION=$(get_version)
TAG="v$VERSION"
log "Releasing $TAG"

if [[ "$DRY_RUN" == "false" ]]; then
  if git rev-parse "$TAG" &>/dev/null; then
    die "Tag $TAG already exists. Use a different version."
  fi
fi

# --- step 3: checks ---

if [[ "$SKIP_CHECKS" == "false" ]]; then
  log "Running checks..."

  log "  cargo check --workspace --locked"
  run cargo check --workspace --locked || die "cargo check failed"

  log "  vue-tsc --noEmit"
  run bash -c "cd desktop && npx vue-tsc --noEmit" || die "typecheck failed"

  log "  vitest run"
  run bash -c "cd desktop && npx vitest run" || die "tests failed"

  ok "All checks passed"
else
  warn "Skipping checks"
fi

# --- step 4: build ---

log "Building Tauri app (release)..."

# Updater artifacts are switched off because the bundler refuses to build them
# without the signing key, and the key stays out of this step; the update
# tarball is packed and signed separately below. Arguments after `--` go to
# cargo, and `--locked` builds against the committed Cargo.lock.
(cd desktop && run npx tauri build --bundles app \
  --config '{"bundle":{"createUpdaterArtifacts":false}}' -- --locked) || die "Tauri build failed"
ok "Build complete"

# --- step 5: create DMG + tarball ---

log "Creating distributable assets..."

DMG="$BUNDLE_DIR/${PRODUCT_NAME}_${VERSION}_${ARCH}.dmg"
TARBALL="$BUNDLE_DIR/${PRODUCT_NAME}_${VERSION}_${ARCH}.app.tar.gz"
SIG_FILE="$TARBALL.sig"
# latest.json sits beside the tarball and its signature, so a failure in step
# 9, after the tag is already pushed, leaves every release asset in place for
# the retry.
LATEST_JSON="$BUNDLE_DIR/latest.json"

if [[ "$DRY_RUN" == "false" ]]; then
  # DMG staging lives in a private directory that is removed on exit, never
  # at a predictable /tmp path another user could pre-create.
  WORK_DIR="$(mktemp -d)"
  trap 'rm -rf "$WORK_DIR"' EXIT

  # DMG
  DMG_STAGING="$WORK_DIR/dmg-staging"
  mkdir -p "$DMG_STAGING"
  cp -R "$BUNDLE_DIR/${PRODUCT_NAME}.app" "$DMG_STAGING/"
  ln -s /Applications "$DMG_STAGING/Applications"
  hdiutil create -volname "$PRODUCT_NAME" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DMG" 2>/dev/null
  rm -rf "$DMG_STAGING"

  # Tarball, in the shape release.yml packs: a gzip tar holding only the
  # `.app` directory. COPYFILE_DISABLE and --no-mac-metadata keep macOS tar
  # from adding AppleDouble `._` entries for extended attributes, which the
  # updater would unpack as stray files inside the installed bundle.
  COPYFILE_DISABLE=1 tar --no-mac-metadata -czf "$TARBALL" -C "$BUNDLE_DIR" "${PRODUCT_NAME}.app"

  ok "DMG: $(basename "$DMG")"
  ok "Tarball: $(basename "$TARBALL")"
else
  echo "  [dry-run] would create $DMG, $TARBALL"
fi

# --- step 6: sign tarball ---

log "Signing update tarball..."

if [[ "$DRY_RUN" == "false" ]]; then
  if [[ "$(signer_digest)" != "$SIGNER_DIGEST" ]]; then
    die "The Tauri CLI in desktop/node_modules or the node binary changed since the checks started; the signing key is not handed to it. Reinstall with: cd desktop && npm ci"
  fi
  # The key reaches the signer through its environment for this one command,
  # never as an argument, where any local user could read it from `ps`. The
  # password is always set, empty included: with none at all the signer
  # prompts for one, which fails without a tty. `--app-version` binds the
  # signature to this version through its signed trusted comment, and an
  # updater with `requireSignedVersion` rejects a manifest announcing any
  # other version. Only the signer's stdout is dropped, so its errors show.
  if [[ -n "$SIGNING_KEY" ]]; then
    TAURI_SIGNING_PRIVATE_KEY="$SIGNING_KEY" \
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$SIGNING_KEY_PASSWORD" \
      "$NODE_BIN" "$SIGNER_JS" signer sign --app-version "$VERSION" "$TARBALL" >/dev/null
  else
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$SIGNING_KEY_PASSWORD" \
      "$NODE_BIN" "$SIGNER_JS" signer sign --app-version "$VERSION" \
      --private-key-path "$SIGNING_KEY_PATH" "$TARBALL" >/dev/null
  fi
  ok "Signed: $(basename "$SIG_FILE")"
else
  echo "  [dry-run] would sign $TARBALL"
fi

# --- step 7: generate latest.json ---

log "Generating latest.json..."

if [[ "$DRY_RUN" == "false" ]]; then
  # Every value reaches Python through the environment and leaves through
  # json.dump, so quotes or newlines in --notes stay data and cannot alter
  # the manifest's structure.
  LJ_VERSION="$VERSION" \
  LJ_NOTES="${CUSTOM_NOTES:-Release $VERSION}" \
  LJ_PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  LJ_PLATFORM="$PLATFORM" \
  LJ_SIGNATURE="$(cat "$SIG_FILE")" \
  LJ_URL="https://github.com/$REPO/releases/download/$TAG/$(basename "$TARBALL")" \
  LJ_OUT="$LATEST_JSON" \
  python3 -I - <<'PY'
import json, os

env = os.environ
data = {
    "version": env["LJ_VERSION"],
    "notes": env["LJ_NOTES"],
    "pub_date": env["LJ_PUB_DATE"],
    "platforms": {
        env["LJ_PLATFORM"]: {
            "signature": env["LJ_SIGNATURE"],
            "url": env["LJ_URL"],
        }
    },
}
with open(env["LJ_OUT"], "w") as f:
    json.dump(data, f, indent=2)
PY
  ok "latest.json generated"
else
  echo "  [dry-run] would generate latest.json"
fi

# --- step 8: commit, tag, push ---

log "Committing and tagging..."

if [[ "$DRY_RUN" == "false" ]]; then
  # Only the files the version bump rewrites, plus the changelog that carries
  # this release's entry, are committed. Anything else left in the working
  # tree stays out of the release commit.
  RELEASE_FILES=(Cargo.toml Cargo.lock CHANGELOG.md desktop/package.json desktop/src-tauri/tauri.conf.json)
  git add -- "${RELEASE_FILES[@]}"
  git commit -m "Release $TAG" -- "${RELEASE_FILES[@]}" 2>/dev/null || warn "Nothing to commit"
  git tag "$TAG"
  git push origin main
  git push origin "$TAG"
  ok "Pushed $TAG"
else
  echo "  [dry-run] would commit, tag $TAG, and push"
fi

# --- step 9: GitHub release ---

log "Creating GitHub release..."

IS_PRERELEASE=false
if [[ "$VERSION" == *alpha* ]] || [[ "$VERSION" == *beta* ]] || [[ "$VERSION" == *rc* ]]; then
  IS_PRERELEASE=true
fi

if [[ "$DRY_RUN" == "false" ]]; then
  NOTES="${CUSTOM_NOTES:-Release $VERSION}"

  # The tag is already on origin here, so a failure names the assets and the
  # commands that finish the release from them.
  if ! gh release create "$TAG" \
    --repo "$REPO" \
    --title "$PRODUCT_NAME $TAG" \
    --notes "$NOTES" \
    $([[ "$IS_PRERELEASE" == "true" ]] && echo "--prerelease") \
    "$DMG" \
    "$TARBALL" \
    "$SIG_FILE" \
    "$LATEST_JSON"; then
    err "Creating the GitHub release failed, but $TAG is already pushed. Its assets:"
    err "  $DMG"
    err "  $TARBALL"
    err "  $SIG_FILE"
    err "  $LATEST_JSON"
    err "Finish with 'gh release create $TAG --repo $REPO <assets>', or with"
    err "'gh release upload $TAG --repo $REPO --clobber <assets>' if the release exists."
    exit 1
  fi

  ok "Release created: https://github.com/$REPO/releases/tag/$TAG"
else
  echo "  [dry-run] would create release $TAG (prerelease=$IS_PRERELEASE)"
fi

# --- done ---

echo ""
ok "Release $TAG complete!"
if [[ "$DRY_RUN" == "false" ]]; then
  echo "  URL: https://github.com/$REPO/releases/tag/$TAG"
fi
if [[ "$IS_PRERELEASE" == "true" ]]; then
  echo "  Type: prerelease"
else
  echo "  Type: stable"
fi
echo "  Version: $VERSION"
