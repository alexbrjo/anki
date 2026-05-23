# doltlite-sys

Raw FFI bindings to [Doltlite](https://github.com/dolthub/doltlite), a fork
of SQLite with a Dolt-style Prolly Tree storage backend. Doltlite preserves
SQLite's C ABI; this crate is structured as a sibling of `libsqlite3-sys`.

## Status: tracer-bullet

The 9 MB amalgamation is generated, not checked in. Run once after clone:

```sh
tools/fetch-doltlite.sh
cargo test -p doltlite-sys --features vendored
```

The smoke test calls `sqlite3_libversion()` / `sqlite3_sourceid()` through
the FFI to prove the vendored library is linkable. Expected: SQLite
**3.54.0**, sourceid ending in `alt1` (Doltlite fork marker).

The full bindgen-generated FFI surface is **not yet written** — see
`rslib/doltlite` (the consumer-facing wrapper). For now the `-sys` crate
declares only the version-introspection functions used by the smoke test.

## Why a separate -sys crate

Two reasons:
1. We can't reuse `libsqlite3-sys` directly because we need a different C
   source compiled in.
2. Doltlite and SQLite can't be statically linked into the same process
   (symbol collision). The legacy-format reader for one-shot migration
   lives in `rslib/anki-migrate-sqlite`, a separate binary that keeps
   stock `rusqlite { bundled }` for its SQLite linkage.
