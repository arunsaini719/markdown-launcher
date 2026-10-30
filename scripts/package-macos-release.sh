#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this script must run on macOS" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${DIST_DIR:-$ROOT_DIR/dist}"
APP_BUNDLE="$DIST_DIR/Markdown Launcher.app"
EXECUTABLE_NAME="markdown-snippet-manager"

for command in cargo python3 ditto shasum codesign; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "error: required command not found: $command" >&2
    exit 1
  fi
done

cd "$ROOT_DIR"
VERSION="$(
  cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json, sys; data = json.load(sys.stdin); print(next(package["version"] for package in data["packages"] if package["name"] == "markdown-snippet-manager"))'
)"
ARCHITECTURE="$(uname -m)"
case "$ARCHITECTURE" in
  arm64 | x86_64) ;;
  *)
    echo "error: unsupported macOS architecture: $ARCHITECTURE" >&2
    exit 1
    ;;
esac

DIST_DIR="$DIST_DIR" "$ROOT_DIR/scripts/build-macos-app.sh"

ARCHIVE_NAME="markdown-launcher-v${VERSION}-macos-${ARCHITECTURE}.zip"
ARCHIVE_PATH="$DIST_DIR/$ARCHIVE_NAME"
CHECKSUM_PATH="$ARCHIVE_PATH.sha256"
rm -f "$ARCHIVE_PATH" "$CHECKSUM_PATH"
ditto -c -k --sequesterRsrc --keepParent "$APP_BUNDLE" "$ARCHIVE_PATH"
(
  cd "$DIST_DIR"
  shasum -a 256 "$ARCHIVE_NAME" >"$ARCHIVE_NAME.sha256"
)

VERIFY_DIR="$(mktemp -d "${TMPDIR:-/tmp}/markdown-launcher-release.XXXXXX")"
trap 'rm -rf "$VERIFY_DIR"' EXIT
ditto -x -k "$ARCHIVE_PATH" "$VERIFY_DIR"
EXTRACTED_APP="$VERIFY_DIR/Markdown Launcher.app"
codesign --verify --deep --strict "$EXTRACTED_APP"
ACTUAL_VERSION="$("$EXTRACTED_APP/Contents/MacOS/$EXECUTABLE_NAME" --version)"
EXPECTED_VERSION="Markdown Launcher $VERSION"
if [[ "$ACTUAL_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "error: packaged binary reported '$ACTUAL_VERSION', expected '$EXPECTED_VERSION'" >&2
  exit 1
fi

echo
echo "Created release archive and checksum:"
echo "  $ARCHIVE_PATH"
echo "  $CHECKSUM_PATH"
echo
echo "The application is ad-hoc signed and not notarized."
