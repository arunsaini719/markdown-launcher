#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "Checking Rust formatting..."
cargo fmt --check

echo "Running Clippy with warnings denied..."
cargo clippy --all-targets --all-features -- -D warnings

echo "Running all tests..."
cargo test --all-targets --all-features
