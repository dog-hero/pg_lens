#!/usr/bin/env bash
# scripts/generate_demo.sh — Regenerates docs/demo.gif via VHS and mirrors it to site assets.
#
# Usage:
#   bash scripts/generate_demo.sh
#
# Prerequisites:
#   - vhs installed (`brew install vhs`)
#   - Rust release toolchain

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "=== pg_lens Demo GIF Generator ==="

# 1. Check VHS prerequisite
if ! command -v vhs >/dev/null 2>&1; then
  echo "Error: 'vhs' is not installed or not in PATH." >&2
  echo "Install VHS via Homebrew: brew install vhs" >&2
  echo "Or see: https://github.com/charmbracelet/vhs" >&2
  exit 1
fi

# 2. Check tape file
if [ ! -f "docs/demo.tape" ]; then
  echo "Error: docs/demo.tape not found in $REPO_ROOT" >&2
  exit 1
fi

# 3. Build release binary directly into ./target/release regardless of environment CARGO_TARGET_DIR
echo "Step 1/3: Compiling release binary (pg_lens_tui)..."
CARGO_TARGET_DIR="$REPO_ROOT/target" cargo build --release -p pg_lens_tui

# Verify binary version matches workspace version in Cargo.toml
VERSION=$(grep -m1 '^version = ' Cargo.toml | cut -d '"' -f2)
BIN_VERSION=$("$REPO_ROOT/target/release/pg_lens" --version | awk '{print $2}')
if [ "$VERSION" != "$BIN_VERSION" ]; then
  echo "Error: target binary version ($BIN_VERSION) does not match Cargo.toml ($VERSION)" >&2
  exit 1
fi
echo "Verified binary: pg_lens v$BIN_VERSION"

# 4. Run VHS recording
echo "Step 2/3: Recording terminal demo via VHS (this takes ~45-60s)..."
vhs docs/demo.tape

# 5. Validate output artifact
if [ ! -f "docs/demo.gif" ]; then
  echo "Error: docs/demo.gif was not produced by VHS." >&2
  exit 1
fi

SIZE=$(wc -c < "docs/demo.gif" | tr -d ' ')
if [ "$SIZE" -lt 500000 ]; then
  echo "Error: docs/demo.gif is suspiciously small ($SIZE bytes). Recording may have aborted." >&2
  exit 1
fi

HUMAN_SIZE=$(du -h "docs/demo.gif" | cut -f1)
echo "Generated docs/demo.gif: $HUMAN_SIZE ($SIZE bytes)"
echo "v$VERSION" > "$REPO_ROOT/docs/demo.version"

# 6. Rebuild documentation site to mirror assets
if [ -f "site/build.mjs" ]; then
  echo "Step 3/3: Rebuilding site documentation & assets..."
  node site/build.mjs
fi

echo "=== Demo GIF successfully generated and verified for v$VERSION! ==="
