# Prolly-tree mode currently unusable with Anki

## Finding

Building against `libdoltlite.a` (the real prolly-tree engine, via
`make doltlite-lib`) produces a working SQLite-ABI library where
`SELECT doltlite_engine()` returns `'prolly'` — but Anki cannot run on
it because **Doltlite v0.11's prolly mode disables custom collations**.

Source: [`src/main.c:3746-3753`](https://github.com/dolthub/doltlite/blob/main/src/main.c) in dolthub/doltlite @ `04d01572eb`:

```c
#if defined(DOLTLITE_PROLLY) && !defined(SQLITE_TEST)
static int doltliteCreateCollationUnsupported(sqlite3 *db){
  int rc = SQLITE_ERROR;
  sqlite3_mutex_enter(db->mutex);
  sqlite3ErrorWithMsg(db, rc, "not supported");
  ...
}
#endif
```

All four flavors of `sqlite3_create_collation*` dispatch to this stub
on prolly builds. The likely reason: prolly trees are sorted by
hash-derived keys for content-addressing, and arbitrary user-defined
collations would break that invariant.

## Why this blocks Anki

Anki registers one custom collation, `unicase` (Unicode
case-insensitive), and uses it on every text-name column:

- `decks.name` — case-insensitive deck name lookup
- `notetypes.name`
- `fields.name`
- `templates.name`
- `deck_config.name`

Schema files (`rslib/src/storage/upgrades/schema15_upgrade.sql`)
literally declare `name text NOT NULL COLLATE unicase` — schema replay
fails with `SQLITE_ERROR_MISSING_COLLSEQ` if the collation isn't
registered.

## What we tried

Switching `rslib/libsqlite3-sys-doltlite/build.rs` and
`rslib/doltlite-sys/build.rs` to link the vendored `libdoltlite.a`
instead of compiling the `make sqlite3.c` amalgamation. With prolly
active:

- 185 of 339 rslib tests pass (basic SQL works)
- 154 fail with either `"journal_mode is not configurable on
  doltlite-format databases"` (worked around in
  `open_or_create_collection_db`) or `"not supported"` from
  `sqlite3_create_collation` (no in-tree workaround possible)

## Resolution path

We've reverted both build scripts to the amalgamation
(stock-SQLite-via-Doltlite-binary) so the tree stays green and the
migration helper machinery is exercised end-to-end.

To revisit:

1. Watch <https://github.com/dolthub/doltlite/issues> for collation
   support in prolly mode.
2. Or fork the C source to allow `create_collation` registration
   (treating the collation as advisory — used for SELECT but not for
   index key ordering). Risk: breaks Doltlite's content-addressing
   invariants for the affected columns.
3. Or migrate Anki's schema to use SQLite's built-in `NOCASE`
   (ASCII-only, lossy for Unicode names) instead of `unicase`.

When upstream lifts the restriction, this swap is a 4-line edit in
the two `build.rs` files plus dropping the `is_prolly_engine()` gates
in `rslib/src/storage/sqlite.rs`.
