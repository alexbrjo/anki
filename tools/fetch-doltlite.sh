#!/usr/bin/env bash
# Fetch and build Doltlite, staging the artifacts under rslib/doltlite-sys/
# so that:
#   - libsqlite3-sys (via SQLITE3_LIB_DIR / SQLITE3_INCLUDE_DIR in
#     .cargo/config.toml) links our libdoltlite.a as if it were libsqlite3.
#   - rslib/build.rs picks up libdoltlite.a from `lib/` and emits the
#     force-load linker flags that pull in the prolly engine.
#
# Idempotent: re-run after pulling a new pin and it will re-clone + rebuild.

set -euo pipefail

# Pin to a known-good Doltlite revision. Bump intentionally; do not float.
# TODO: set this to the actual full SHA of the Doltlite commit whose
# `libdoltlite.a` is currently in `rslib/doltlite-sys/lib/`. The previous
# tracer mentioned `04d01572eb` (10-char prefix) as the basis; please verify
# by running `git -C <doltlite-clone> log -1 --format=%H` on the same tree.
DOLTLITE_SHA="${DOLTLITE_SHA:-}"
DOLTLITE_REPO="https://github.com/dolthub/doltlite.git"

if [[ -z "$DOLTLITE_SHA" ]]; then
    echo "DOLTLITE_SHA is not set in $0 (and no override in env)." >&2
    echo "Edit the script to pin the actual commit, or run:" >&2
    echo "  DOLTLITE_SHA=<full-sha> $0" >&2
    exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/rslib/doltlite-sys"
work_dir="${TMPDIR:-/tmp}/doltlite-build"

mkdir -p "$out_dir/lib" "$out_dir/include"

if [[ -d "$work_dir/.git" ]]; then
    git -C "$work_dir" fetch --quiet origin
    git -C "$work_dir" reset --hard --quiet "$DOLTLITE_SHA"
else
    rm -rf "$work_dir"
    git clone --quiet "$DOLTLITE_REPO" "$work_dir"
    git -C "$work_dir" reset --hard --quiet "$DOLTLITE_SHA"
fi

cd "$work_dir"

# `make doltlite-lib` is the real Doltlite build (prolly engine baked in).
# `make sqlite3.c` produces a SQLite-compat amalgamation without prolly —
# easy to grab by mistake, never what we want.
make -j"$(getconf _NPROCESSORS_ONLN || echo 4)" doltlite-lib

# The doltlite-lib target doesn't ship sqlite3ext.h on its own; build the
# headers explicitly.
make sqlite3.h sqlite3ext.h

cp libdoltlite.a "$out_dir/lib/libdoltlite.a"
cp sqlite3.h sqlite3ext.h "$out_dir/include/"

# libsqlite3-sys links against `libsqlite3.a` by name; provide a symlink
# rather than a copy so we don't ship 10 MB twice.
ln -sf libdoltlite.a "$out_dir/lib/libsqlite3.a"

echo
echo "Doltlite $DOLTLITE_SHA staged under rslib/doltlite-sys/"
echo "  lib/libdoltlite.a  ($(wc -c < "$out_dir/lib/libdoltlite.a") bytes)"
echo "  lib/libsqlite3.a   -> libdoltlite.a"
echo "  include/sqlite3.h"
echo "  include/sqlite3ext.h"
