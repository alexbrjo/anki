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
# Pinned to v0.11.0-11-g04d01572eb (forked from SQLite 3.54.0).
# Bump deliberately — Doltlite is pre-1.0 and the on-disk format may change.
DOLTLITE_REV="${DOLTLITE_REV:-04d01572eb9dd07b9b9d3ec0d5d4951748808a3f}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR_DIR="$REPO_ROOT/rslib/doltlite-sys/vendor"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

echo "Cloning $DOLTLITE_REPO @ $DOLTLITE_REV into $WORK_DIR..."
git clone "$DOLTLITE_REPO" "$WORK_DIR/doltlite"

pushd "$WORK_DIR/doltlite" >/dev/null
git checkout "$DOLTLITE_REV"

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
