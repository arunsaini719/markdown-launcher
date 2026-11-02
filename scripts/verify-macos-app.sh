#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this script must run on macOS" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="${1:-$ROOT_DIR/dist/Markdown Launcher.app}"
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
EXPECTED_ARCHITECTURE="${EXPECTED_ARCHITECTURE:-$(uname -m)}"
EXPECTED_EXECUTABLE="markdown-snippet-manager"
BINARY_PATH="$APP_BUNDLE/Contents/MacOS/$EXPECTED_EXECUTABLE"
ICON_PATH="$APP_BUNDLE/Contents/Resources/AppIcon.icns"

for command in cargo python3 plutil codesign lipo; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "error: required command not found: $command" >&2
    exit 1
  fi
done

if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "error: application bundle not found: $APP_BUNDLE" >&2
  exit 1
fi
if [[ ! -x "$BINARY_PATH" ]]; then
  echo "error: application executable not found: $BINARY_PATH" >&2
  exit 1
fi
if [[ ! -f "$ICON_PATH" ]]; then
  echo "error: application icon not found: $ICON_PATH" >&2
  exit 1
fi

cd "$ROOT_DIR"
EXPECTED_VERSION="$(
  cargo metadata --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; data = json.load(sys.stdin); print(next(package["version"] for package in data["packages"] if package["name"] == "markdown-snippet-manager"))'
)"

plutil -lint "$INFO_PLIST" >/dev/null
[[ "$(plutil -extract CFBundleExecutable raw -o - "$INFO_PLIST")" == "$EXPECTED_EXECUTABLE" ]]
[[ "$(plutil -extract CFBundleIdentifier raw -o - "$INFO_PLIST")" == "com.markdown-launcher.MarkdownSnippetManager" ]]
[[ "$(plutil -extract CFBundleShortVersionString raw -o - "$INFO_PLIST")" == "$EXPECTED_VERSION" ]]
[[ "$(plutil -extract CFBundleVersion raw -o - "$INFO_PLIST")" == "$EXPECTED_VERSION" ]]
[[ "$(plutil -extract LSMinimumSystemVersion raw -o - "$INFO_PLIST")" == "15.0" ]]
[[ "$(plutil -extract LSUIElement raw -o - "$INFO_PLIST")" == "true" ]]

ARCHITECTURES="$(lipo -archs "$BINARY_PATH")"
if [[ " $ARCHITECTURES " != *" $EXPECTED_ARCHITECTURE "* ]]; then
  echo "error: expected $EXPECTED_ARCHITECTURE executable, found: $ARCHITECTURES" >&2
  exit 1
fi

codesign --verify --deep --strict "$APP_BUNDLE"
ACTUAL_VERSION="$("$BINARY_PATH" --version)"
if [[ "$ACTUAL_VERSION" != "Markdown Launcher $EXPECTED_VERSION" ]]; then
  echo "error: executable reported '$ACTUAL_VERSION', expected 'Markdown Launcher $EXPECTED_VERSION'" >&2
  exit 1
fi

echo "Verified macOS application:"
echo "  Bundle: $APP_BUNDLE"
echo "  Version: $EXPECTED_VERSION"
echo "  Architecture: $EXPECTED_ARCHITECTURE"
