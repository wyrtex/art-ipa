#!/usr/bin/env bash
# Build one ArtCraft Crafting App as an unsigned iOS / iPadOS .ipa.
#
#   scripts/build-ipa.sh <app-id> [out-dir]
#
# <app-id> is an `id` from apps.json (photocraft, gridcraft, ...). Needs macOS with Xcode (the
# iOS SDK and linker) and rustup. The upstream source is cloned into build/<app-id> at the commit
# pinned in apps.json, prepared by scripts/prepare_app.py, and built for aarch64-apple-ios.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:?usage: build-ipa.sh <app-id> [out-dir]}"
OUT="$(mkdir -p "${2:-$ROOT/dist}" && cd "${2:-$ROOT/dist}" && pwd)"
TARGET=aarch64-apple-ios

read -r NAME REPO REV < <(python3 - "$ROOT/apps.json" "$APP" <<'PY'
import json, sys
apps = {a["id"]: a for a in json.load(open(sys.argv[1]))["apps"]}
a = apps.get(sys.argv[2]) or sys.exit(f"unknown app {sys.argv[2]!r}; known: {', '.join(apps)}")
print(a["name"], a["repo"], a["rev"])
PY
)
[ -n "${REV:-}" ] || exit 1

SRC="$ROOT/build/$APP"
if [ ! -d "$SRC/.git" ]; then
  git init -q "$SRC"
  git -C "$SRC" remote add origin "https://github.com/$REPO.git"
fi
git -C "$SRC" fetch -q --depth 1 origin "$REV"
git -C "$SRC" checkout -q --force FETCH_HEAD
git -C "$SRC" clean -qfdx -e target

python3 "$ROOT/scripts/prepare_app.py" "$SRC" "$APP" "$ROOT"

rustup target add "$TARGET" >/dev/null
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}"
# Thin LTO keeps the link within a CI runner's memory and time; no debug info in the .ipa.
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-thin}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"
export CARGO_PROFILE_RELEASE_DEBUG="${CARGO_PROFILE_RELEASE_DEBUG:-false}"
export CARGO_PROFILE_RELEASE_STRIP="${CARGO_PROFILE_RELEASE_STRIP:-symbols}"
export CARGO_PROFILE_RELEASE_PANIC="${CARGO_PROFILE_RELEASE_PANIC:-unwind}"

(cd "$SRC" && cargo build --release --target "$TARGET" -p "$APP" --bin "$APP")

VERSION="$(cd "$SRC" && cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["version"] for p in m["packages"] if p["name"]==sys.argv[1]))' "$APP")"

ICON=""
for c in "assets/app-icon/$APP-1024.png" "assets/app-icon/hicolor/512x512/apps/ai.storyteller.$APP.png" "assets/app-icon/$APP-macos-512.png"; do
  if [ -f "$SRC/$c" ]; then ICON="$SRC/$c"; break; fi
done
[ -n "$ICON" ] || { echo "no app icon found for $APP" >&2; exit 1; }

STAGE="$ROOT/build/stage/$APP"
rm -rf "$STAGE"; mkdir -p "$STAGE"
IPA="$(python3 "$ROOT/scripts/make_bundle.py" "$APP" "$NAME" "$VERSION" "$SRC/target/$TARGET/release/$APP" "$ICON" "$STAGE")"
mv "$IPA" "$OUT/"
echo "$OUT/$(basename "$IPA")"
