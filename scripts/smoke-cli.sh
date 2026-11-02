#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="${1:-$ROOT_DIR/target/debug/markdown-snippet-manager}"

if [[ ! -x "$BINARY" ]]; then
  echo "error: executable not found: $BINARY" >&2
  exit 1
fi

TEST_HOME="$(mktemp -d "${TMPDIR:-/tmp}/markdown-launcher-cli.XXXXXX")"
trap 'rm -rf "$TEST_HOME"' EXIT

cd "$ROOT_DIR"
EXPECTED_VERSION="$(
  cargo metadata --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; data = json.load(sys.stdin); print(next(package["version"] for package in data["packages"] if package["name"] == "markdown-snippet-manager"))'
)"

VERSION_OUTPUT="$(HOME="$TEST_HOME" "$BINARY" --version)"
[[ "$VERSION_OUTPUT" == "Markdown Launcher $EXPECTED_VERSION" ]]

HELP_OUTPUT="$(HOME="$TEST_HOME" "$BINARY" --help)"
grep -q -- "--print-paths" <<<"$HELP_OUTPUT"
grep -q -- "--shortcuts" <<<"$HELP_OUTPUT"
grep -q -- "--check-config" <<<"$HELP_OUTPUT"
grep -q -- "--check-index" <<<"$HELP_OUTPUT"
grep -q -- "--diagnostics" <<<"$HELP_OUTPUT"

SHORTCUTS_OUTPUT="$(HOME="$TEST_HOME" "$BINARY" --shortcuts)"
grep -q "Search results" <<<"$SHORTCUTS_OUTPUT"
grep -q "Command/Ctrl+Shift+C" <<<"$SHORTCUTS_OUTPUT"

PATHS_OUTPUT="$(HOME="$TEST_HOME" "$BINARY" --print-paths)"
CONFIG_PATH="$(awk 'sub(/^Configuration: /, "") { print; exit }' <<<"$PATHS_OUTPUT")"
INDEX_PATH="$(awk 'sub(/^Index: /, "") { print; exit }' <<<"$PATHS_OUTPUT")"
[[ "$CONFIG_PATH" == "$TEST_HOME"/* ]]
[[ "$INDEX_PATH" == "$TEST_HOME"/* ]]

MISSING_DIAGNOSTICS="$(HOME="$TEST_HOME" "$BINARY" --diagnostics)"
grep -q "Configuration: missing" <<<"$MISSING_DIAGNOSTICS"
grep -q "Index: missing" <<<"$MISSING_DIAGNOSTICS"
if grep -Fq "$TEST_HOME" <<<"$MISSING_DIAGNOSTICS"; then
  echo "error: diagnostics exposed the temporary home path" >&2
  exit 1
fi
if HOME="$TEST_HOME" "$BINARY" --check-config >/dev/null 2>&1; then
  echo "error: missing configuration unexpectedly passed validation" >&2
  exit 1
fi
if HOME="$TEST_HOME" "$BINARY" --check-index >/dev/null 2>&1; then
  echo "error: missing index unexpectedly passed validation" >&2
  exit 1
fi
[[ ! -e "$INDEX_PATH" ]]

mkdir -p "$(dirname "$CONFIG_PATH")"
cat >"$CONFIG_PATH" <<EOF
global_shortcut = "cmdorctrl+shift+Space"

[scan]
roots = ["$TEST_HOME"]
extensions = ["md", "markdown"]
excluded_directory_names = [".git", "node_modules", "target"]
excluded_globs = []
max_file_size_bytes = 2097152
follow_symbolic_links = false
EOF

HOME="$TEST_HOME" "$BINARY" --check-config >/dev/null
VALID_DIAGNOSTICS="$(HOME="$TEST_HOME" "$BINARY" --diagnostics)"
grep -q "Configuration: valid" <<<"$VALID_DIAGNOSTICS"
if grep -Fq "$TEST_HOME" <<<"$VALID_DIAGNOSTICS"; then
  echo "error: diagnostics exposed a configured scan root" >&2
  exit 1
fi

printf 'unknown = true\n' >"$CONFIG_PATH"
if HOME="$TEST_HOME" "$BINARY" --check-config >/dev/null 2>&1; then
  echo "error: invalid configuration unexpectedly passed validation" >&2
  exit 1
fi
[[ "$(cat "$CONFIG_PATH")" == "unknown = true" ]]

if HOME="$TEST_HOME" "$BINARY" --unknown >/dev/null 2>&1; then
  echo "error: unknown option unexpectedly succeeded" >&2
  exit 1
fi

echo "CLI smoke checks passed."
