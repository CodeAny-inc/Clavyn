#!/usr/bin/env bash
# version.sh — get or bump the project version across all config files.
#
# Usage:
#   ./scripts/version.sh              # print current version
#   ./scripts/version.sh 0.1.2        # bump to 0.1.2
#   ./scripts/version.sh patch        # bump patch  (0.1.1 -> 0.1.2)
#   ./scripts/version.sh minor        # bump minor  (0.1.1 -> 0.2.0)
#   ./scripts/version.sh major        # bump major  (0.1.1 -> 1.0.0)
#   ./scripts/version.sh alpha        # bump alpha  (0.1.1-alpha.9 -> 0.1.1-alpha.10)
#   ./scripts/version.sh beta         # bump beta   (0.1.1 -> 0.1.1-beta.1)
#   ./scripts/version.sh stable       # strip pre-release (0.1.1-alpha.9 -> 0.1.1)
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO_TOML="$ROOT/Cargo.toml"
CARGO_LOCK="$ROOT/Cargo.lock"
PACKAGE_JSON="$ROOT/desktop/package.json"
TAURI_CONF="$ROOT/desktop/src-tauri/tauri.conf.json"

# A semantic version as semver.org defines it, without build metadata: no
# empty identifiers and no leading zeros in numeric ones. Cargo rejects
# anything looser, and release.yml applies the same pattern to the tags it
# publishes. The value is also spliced into sed expressions, which this
# character set keeps inert.
SEMVER_RE='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?$'

# --- helpers ---

get_version() {
  grep '^version = ' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/'
}

require_semver() {
  if [[ ! "$1" =~ $SEMVER_RE ]]; then
    echo "Not a semantic version: '$1' (expected MAJOR.MINOR.PATCH[-PRERELEASE])" >&2
    exit 1
  fi
}

# Called from an `if`, where errexit does not apply, so every step returns
# on failure explicitly.
write_version() {
  local new="$1" file
  # -i.bak is the in-place form both GNU and BSD sed accept
  # Cargo.toml (workspace.package)
  sed -i.bak "s/^version = \".*\"/version = \"$new\"/" "$CARGO_TOML" || return 1
  rm -f "$CARGO_TOML.bak"
  # desktop/package.json and tauri.conf.json
  for file in "$PACKAGE_JSON" "$TAURI_CONF"; do
    sed -i.bak "s/\"version\": \".*\"/\"version\": \"$new\"/" "$file" || return 1
    rm -f "$file.bak"
  done
  # Cargo.lock records the workspace crates' own versions, and CI builds with
  # --locked, which rejects a lockfile that disagrees with Cargo.toml. Only the
  # workspace entries are rewritten; no dependency moves. The offline attempt
  # needs nothing beyond the local registry cache and is usually enough.
  if ! cargo update -w --offline --manifest-path "$CARGO_TOML" 2>/dev/null; then
    cargo update -w --manifest-path "$CARGO_TOML" || return 1
  fi
}

# The four files that record the version are copied first and put back if
# any step fails, so a bump either lands in all of them or in none.
set_version() {
  local new="$1" backup
  require_semver "$new"
  backup="$(mktemp -d)"
  cp "$CARGO_TOML" "$backup/Cargo.toml"
  cp "$CARGO_LOCK" "$backup/Cargo.lock"
  cp "$PACKAGE_JSON" "$backup/package.json"
  cp "$TAURI_CONF" "$backup/tauri.conf.json"
  if ! write_version "$new"; then
    cp "$backup/Cargo.toml" "$CARGO_TOML"
    cp "$backup/Cargo.lock" "$CARGO_LOCK"
    cp "$backup/package.json" "$PACKAGE_JSON"
    cp "$backup/tauri.conf.json" "$TAURI_CONF"
    rm -f "$CARGO_TOML.bak" "$PACKAGE_JSON.bak" "$TAURI_CONF.bak"
    rm -rf "$backup"
    echo "Bumping to $new failed; Cargo.toml, Cargo.lock, package.json and tauri.conf.json are unchanged." >&2
    exit 1
  fi
  rm -rf "$backup"
  echo "Bumped version to $new"
}

# Parse semver: MAJOR.MINOR.PATCH[-PRE]
parse_semver() {
  local v="$1"
  local base pre
  base="${v%%-*}"
  pre="${v#*-}"
  if [[ "$pre" == "$v" ]]; then
    pre=""
  fi
  local major minor patch
  IFS='.' read -r major minor patch <<< "$base"
  echo "$major $minor $patch $pre"
}

# --- commands ---

cmd_get() {
  get_version
}

cmd_bump_explicit() {
  local new="$1"
  local current
  require_semver "$new"
  current=$(get_version)
  if [[ "$current" == "$new" ]]; then
    echo "Version is already $new"
    exit 0
  fi
  set_version "$new"
}

cmd_bump_patch() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  set_version "$major.$minor.$((patch + 1))"
}

cmd_bump_minor() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  set_version "$major.$((minor + 1)).0"
}

cmd_bump_major() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  set_version "$((major + 1)).0.0"
}

cmd_bump_alpha() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  if [[ -z "$pre" ]] || [[ "$pre" != alpha.* ]]; then
    # No existing alpha — start alpha.1
    set_version "$major.$minor.$patch-alpha.1"
  else
    local num="${pre#alpha.}"
    set_version "$major.$minor.$patch-alpha.$((num + 1))"
  fi
}

cmd_bump_beta() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  if [[ -z "$pre" ]] || [[ "$pre" != beta.* ]]; then
    set_version "$major.$minor.$patch-beta.1"
  else
    local num="${pre#beta.}"
    set_version "$major.$minor.$patch-beta.$((num + 1))"
  fi
}

cmd_stable() {
  local current major minor patch pre
  current=$(get_version)
  read -r major minor patch pre <<< "$(parse_semver "$current")"
  if [[ -z "$pre" ]]; then
    echo "Version $current is already stable (no pre-release suffix)"
    exit 0
  fi
  set_version "$major.$minor.$patch"
}

# --- main ---

case "${1:-get}" in
  get|"") cmd_get ;;
  patch)  cmd_bump_patch ;;
  minor)  cmd_bump_minor ;;
  major)  cmd_bump_major ;;
  alpha)  cmd_bump_alpha ;;
  beta)   cmd_bump_beta ;;
  stable) cmd_stable ;;
  -h|--help)
    cat <<'USAGE'
version.sh — manage Clavyn version

Usage:
  ./scripts/version.sh              Print current version
  ./scripts/version.sh 0.1.2        Set version to 0.1.2
  ./scripts/version.sh patch        Bump patch:  0.1.1 -> 0.1.2
  ./scripts/version.sh minor        Bump minor:  0.1.1 -> 0.2.0
  ./scripts/version.sh major        Bump major:  0.1.1 -> 1.0.0
  ./scripts/version.sh alpha        Bump alpha:  0.1.1-alpha.9 -> 0.1.1-alpha.10
  ./scripts/version.sh beta         Bump beta:   0.1.1 -> 0.1.1-beta.1
  ./scripts/version.sh stable       Strip pre:   0.1.1-alpha.9 -> 0.1.1
USAGE
    ;;
  *)
    # Assume it's an explicit version string
    cmd_bump_explicit "$1"
    ;;
esac
