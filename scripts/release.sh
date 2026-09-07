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

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# --- config ---
REPO="CodeAny-inc/Clavyn"
PRODUCT_NAME="Clavyn"
SIGNING_KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.config/clavyn/updater-private.key}"
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
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] && [[ ! -f "$SIGNING_KEY_PATH" ]]; then
  die "Signing key not found. Set TAURI_SIGNING_PRIVATE_KEY env var or create $SIGNING_KEY_PATH"
fi
ok "Signing key available"

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

  log "  cargo check --workspace"
  run cargo check --workspace || die "cargo check failed"

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

export TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY:-$(cat "$SIGNING_KEY_PATH")}"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

run bash -c "cd desktop && npx tauri build --bundles app" || die "Tauri build failed"
ok "Build complete"

# --- step 5: create DMG + tarball ---

log "Creating distributable assets..."

DMG="$BUNDLE_DIR/${PRODUCT_NAME}_${VERSION}_${ARCH}.dmg"
TARBALL="$BUNDLE_DIR/${PRODUCT_NAME}_${VERSION}_${ARCH}.app.tar.gz"
SIG_FILE="$TARBALL.sig"

if [[ "$DRY_RUN" == "false" ]]; then
  # DMG
  DMG_STAGING="/tmp/dmg-staging-$$"
  rm -rf "$DMG_STAGING"
  mkdir -p "$DMG_STAGING"
  cp -R "$BUNDLE_DIR/${PRODUCT_NAME}.app" "$DMG_STAGING/"
  ln -s /Applications "$DMG_STAGING/Applications"
  hdiutil create -volname "$PRODUCT_NAME" -srcfolder "$DMG_STAGING" -ov -format UDZO "$DMG" 2>/dev/null
  rm -rf "$DMG_STAGING"

  # Tarball
  cd "$BUNDLE_DIR"
  tar czf "$TARBALL" "${PRODUCT_NAME}.app"
  cd "$ROOT"

  ok "DMG: $(basename "$DMG")"
  ok "Tarball: $(basename "$TARBALL")"
else
  echo "  [dry-run] would create $DMG, $TARBALL"
fi

# --- step 6: sign tarball ---

log "Signing update tarball..."

if [[ "$DRY_RUN" == "false" ]]; then
  # Use --private-key (string) + --password instead of --private-key-path.
  # The latter triggers an interactive password prompt even when the key has
  # no password, which fails in non-interactive environments (no tty).
  cd desktop
  npx @tauri-apps/cli signer sign \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
    "$TARBALL" 2>/dev/null
  cd "$ROOT"
  ok "Signed: $(basename "$SIG_FILE")"
else
  echo "  [dry-run] would sign $TARBALL"
fi

# --- step 7: generate latest.json ---

log "Generating latest.json..."

if [[ "$DRY_RUN" == "false" ]]; then
  SIG=$(cat "$SIG_FILE")
  NOTES="${CUSTOM_NOTES:-Release $VERSION}"

  python3 -c "
import json
data = {
    'version': '$VERSION',
    'notes': '''$NOTES''',
    'pub_date': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'platforms': {
        '$PLATFORM': {
            'signature': '''$SIG''',
            'url': 'https://github.com/$REPO/releases/download/$TAG/$(basename "$TARBALL")'
        }
    }
}
with open('/tmp/latest.json', 'w') as f:
    json.dump(data, f, indent=2)
"
  ok "latest.json generated"
else
  echo "  [dry-run] would generate latest.json"
fi

# --- step 8: commit, tag, push ---

log "Committing and tagging..."

if [[ "$DRY_RUN" == "false" ]]; then
  git add -A
  git commit -m "Release $TAG" 2>/dev/null || warn "Nothing to commit"
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

  gh release create "$TAG" \
    --repo "$REPO" \
    --title "$PRODUCT_NAME $TAG" \
    --notes "$NOTES" \
    $([[ "$IS_PRERELEASE" == "true" ]] && echo "--prerelease") \
    "$DMG" \
    "$TARBALL" \
    "$SIG_FILE" \
    "/tmp/latest.json" \
    2>/dev/null

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
