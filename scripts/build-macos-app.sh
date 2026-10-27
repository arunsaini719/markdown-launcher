#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this script must run on macOS" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Markdown Launcher"
EXECUTABLE_NAME="markdown-snippet-manager"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
APP_BUNDLE="$DIST_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_BUNDLE/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
ICON_SOURCE="$ROOT_DIR/assets/AppIcon-1024.png"
PLIST_TEMPLATE="$ROOT_DIR/packaging/macos/Info.plist"
OPEN_AFTER_BUILD=false

if [[ "${1:-}" == "--open" ]]; then
  OPEN_AFTER_BUILD=true
elif [[ -n "${1:-}" ]]; then
  echo "usage: $0 [--open]" >&2
  exit 2
fi

for command in cargo python3 sips iconutil plutil codesign; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "error: required command not found: $command" >&2
    exit 1
  fi
done

if [[ ! -f "$ICON_SOURCE" ]]; then
  echo "error: icon source not found: $ICON_SOURCE" >&2
  exit 1
fi

cd "$ROOT_DIR"
echo "Building release executable..."
cargo build --release --locked

VERSION="$(
  cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json, sys; data = json.load(sys.stdin); print(next(package["version"] for package in data["packages"] if package["name"] == "markdown-snippet-manager"))'
)"
BINARY_PATH="$ROOT_DIR/target/release/$EXECUTABLE_NAME"

if [[ ! -x "$BINARY_PATH" ]]; then
  echo "error: release executable was not produced at $BINARY_PATH" >&2
  exit 1
fi

rm -rf "$APP_BUNDLE"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
install -m 755 "$BINARY_PATH" "$MACOS_DIR/$EXECUTABLE_NAME"

python3 - "$PLIST_TEMPLATE" "$CONTENTS_DIR/Info.plist" "$VERSION" <<'PY'
from pathlib import Path
import sys

template_path, output_path, version = sys.argv[1:]
contents = Path(template_path).read_text(encoding="utf-8")
Path(output_path).write_text(contents.replace("@VERSION@", version), encoding="utf-8")
PY
plutil -lint "$CONTENTS_DIR/Info.plist" >/dev/null

ICONSET_TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/markdown-launcher-icon.XXXXXX")"
ICONSET_DIR="$ICONSET_TEMP_DIR/AppIcon.iconset"
mkdir -p "$ICONSET_DIR"
trap 'rm -rf "$ICONSET_TEMP_DIR"' EXIT

create_icon() {
  local pixels="$1"
  local output="$2"
  sips -z "$pixels" "$pixels" "$ICON_SOURCE" --out "$ICONSET_DIR/$output" >/dev/null
}

create_icon 16 icon_16x16.png
create_icon 32 icon_16x16@2x.png
create_icon 32 icon_32x32.png
create_icon 64 icon_32x32@2x.png
create_icon 128 icon_128x128.png
create_icon 256 icon_128x128@2x.png
create_icon 256 icon_256x256.png
create_icon 512 icon_256x256@2x.png
create_icon 512 icon_512x512.png
cp "$ICON_SOURCE" "$ICONSET_DIR/icon_512x512@2x.png"
iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/AppIcon.icns"

codesign --force --sign - --timestamp=none "$APP_BUNDLE"
codesign --verify --deep --strict "$APP_BUNDLE"

echo
echo "Created local macOS application:"
echo "  $APP_BUNDLE"
echo
echo "Launch it with:"
echo "  open \"$APP_BUNDLE\""
echo
echo "This app is ad-hoc signed for local use; it is not notarized for distribution."

if [[ "$OPEN_AFTER_BUILD" == true ]]; then
  open "$APP_BUNDLE"
fi
