# doltlite-sys

Raw FFI bindings to [Doltlite](https://github.com/dolthub/doltlite), a fork
of SQLite with a Dolt-style Prolly Tree storage backend. Doltlite preserves
SQLite's C ABI; this crate is structured as a sibling of `libsqlite3-sys`.

## Status: tracer-bullet stub

The C amalgamation is **not yet vendored**. The crate currently compiles to
an empty Rust library so the workspace still builds. To complete the wiring:

```sh
tools/fetch-doltlite.sh
cargo build -p doltlite-sys --features vendored
```

`fetch-doltlite.sh` clones dolthub/doltlite, runs `./configure && make
sqlite3.c sqlite3.h`, and copies the amalgamation to `vendor/`.

## Why a separate -sys crate

Two reasons:
1. We can't reuse `libsqlite3-sys` directly because we need a different C
   source compiled in.
2. Doltlite and SQLite can't be statically linked into the same process
   (symbol collision). The legacy-format reader for one-shot migration
   lives in `rslib/anki-migrate-sqlite`, a separate binary that keeps
   stock `rusqlite { bundled }` for its SQLite linkage.
