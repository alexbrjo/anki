#!/usr/bin/env bash
# Download a pre-built Doltlite library for the host platform and stage it
# under rslib/doltlite-sys/ for libsqlite3-sys to find. DoltHub publishes
# per-platform archives in their GitHub releases, so we don't need to clone
# and build from source.
#
# Idempotent: re-run any time to refresh, or after bumping DOLTLITE_VERSION.

set -euo pipefail

DOLTLITE_VERSION="${DOLTLITE_VERSION:-0.11.0}"

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/rslib/doltlite-sys"

# Detect host platform → release asset name suffix.
uname_s=$(uname -s)
uname_m=$(uname -m)
case "$uname_s/$uname_m" in
    Darwin/arm64)   asset_arch="osx-arm64" ;;
    Darwin/x86_64)
        # DoltHub doesn't currently ship an osx-x64 prebuilt; macOS Intel
        # users have to build from source or run under Rosetta.
        echo "no prebuilt available for macOS x86_64 — bring your own libdoltlite.a" >&2
        exit 2 ;;
    Linux/x86_64)   asset_arch="linux-x64" ;;
    Linux/aarch64)  asset_arch="linux-arm64" ;;
    MINGW*|MSYS*|CYGWIN*)
        asset_arch="win-x64" ;;
    *)
        echo "unsupported host platform $uname_s/$uname_m" >&2
        exit 2 ;;
esac

asset="doltlite-lib-${asset_arch}-${DOLTLITE_VERSION}.zip"
url="https://github.com/dolthub/doltlite/releases/download/v${DOLTLITE_VERSION}/${asset}"

work_dir="${TMPDIR:-/tmp}/doltlite-fetch-${DOLTLITE_VERSION}-${asset_arch}"
rm -rf "$work_dir"
mkdir -p "$work_dir"
mkdir -p "$out_dir/lib" "$out_dir/include"

echo "fetching $asset"
curl -fL --progress-bar -o "$work_dir/$asset" "$url"
unzip -q "$work_dir/$asset" -d "$work_dir"

src_dir="$work_dir/doltlite-lib-${asset_arch}-${DOLTLITE_VERSION}"

cp "$src_dir/libdoltlite.a" "$out_dir/lib/libdoltlite.a"
# libsqlite3-sys links against `libsqlite3.a` by name; symlink rather than
# copy to keep the artifact size honest.
ln -sf libdoltlite.a "$out_dir/lib/libsqlite3.a"

# The archive ships `doltlite.h`, which is SQLite's standard sqlite3.h with
# the file renamed. Copy it under both names so libsqlite3-sys's `#include
# "sqlite3.h"` and any reference to "doltlite.h" both resolve.
cp "$src_dir/doltlite.h" "$out_dir/include/sqlite3.h"
cp "$src_dir/doltlite.h" "$out_dir/include/doltlite.h"
[[ -f "$src_dir/doltlite_remotesrv.h" ]] && cp "$src_dir/doltlite_remotesrv.h" "$out_dir/include/"

rm -rf "$work_dir"

echo
echo "Doltlite v${DOLTLITE_VERSION} (${asset_arch}) staged under rslib/doltlite-sys/"
echo "  lib/libdoltlite.a  ($(wc -c < "$out_dir/lib/libdoltlite.a") bytes)"
echo "  lib/libsqlite3.a   -> libdoltlite.a"
echo "  include/sqlite3.h"
echo "  include/doltlite.h"
