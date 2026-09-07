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
PACKAGE_JSON="$ROOT/desktop/package.json"
TAURI_CONF="$ROOT/desktop/src-tauri/tauri.conf.json"

# --- helpers ---

get_version() {
  grep '^version = ' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/'
}

set_version() {
  local new="$1"
  # Cargo.toml (workspace.package)
  sed -i '' "s/^version = \".*\"/version = \"$new\"/" "$CARGO_TOML"
  # desktop/package.json
  sed -i '' "s/\"version\": \".*\"/\"version\": \"$new\"/" "$PACKAGE_JSON"
  # tauri.conf.json
  sed -i '' "s/\"version\": \".*\"/\"version\": \"$new\"/" "$TAURI_CONF"
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
