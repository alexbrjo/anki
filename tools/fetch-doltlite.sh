#!/usr/bin/env bash
# Fetch and build the Doltlite amalgamation, then copy it into
# rslib/doltlite-sys/vendor/ so the -sys crate can compile it.
#
# After this script succeeds:
#   cargo build -p doltlite-sys --features vendored
#
# Pin the upstream revision by editing DOLTLITE_REV below. Bump
# deliberately — Doltlite is pre-1.0 and the on-disk format may change.

set -euo pipefail

DOLTLITE_REPO="${DOLTLITE_REPO:-https://github.com/dolthub/doltlite.git}"
DOLTLITE_REV="${DOLTLITE_REV:-main}"   # TODO: pin to a known-good SHA

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR_DIR="$REPO_ROOT/rslib/doltlite-sys/vendor"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "Cloning $DOLTLITE_REPO @ $DOLTLITE_REV into $WORK_DIR..."
git clone --depth 1 --branch "$DOLTLITE_REV" "$DOLTLITE_REPO" "$WORK_DIR/doltlite" \
  || git clone "$DOLTLITE_REPO" "$WORK_DIR/doltlite"

pushd "$WORK_DIR/doltlite" >/dev/null
if [ "$DOLTLITE_REV" != "main" ]; then
  git checkout "$DOLTLITE_REV"
fi

echo "Building amalgamation..."
./configure
make sqlite3.c sqlite3.h sqlite3ext.h

mkdir -p "$VENDOR_DIR"
cp sqlite3.c   "$VENDOR_DIR/doltlite.c"
cp sqlite3.h   "$VENDOR_DIR/sqlite3.h"
cp sqlite3ext.h "$VENDOR_DIR/sqlite3ext.h"
popd >/dev/null

echo "Doltlite amalgamation vendored to $VENDOR_DIR:"
ls -lh "$VENDOR_DIR"
echo
echo "Next:"
echo "  cargo build -p doltlite-sys --features vendored"
