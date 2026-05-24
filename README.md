# Anki on Doltlite

This branch (`doltlite`) replaces Anki's SQLite storage engine with
[Doltlite](https://www.dolthub.com/blog/2026-03-25-doltlite/) — a SQLite
fork that swaps the B-tree pager for a content-addressed prolly tree.
The substrate for `dolt_commit` / `dolt_branch` / `dolt_diff_*` is now
live inside every Anki connection; surfacing those in the UI is a
separate, follow-up project.

## What changes for the user

- **One-time blocking migration on first open.** Every existing
  `collection.anki2` (and `collection.media.db2`) is rewritten in place
  from stock SQLite to the prolly format. A modal "Converting
  collection…" spinner is shown for the duration. Back up before first
  launch — no `.legacy-backup` snapshot is kept.
- **Case-insensitive name uniqueness is gone.** "Default" and "default"
  can coexist as separate decks, notetypes, tags, etc. Rust-side
  `UniCase` still folds case for in-memory lookups and tag-tree
  display, so most reads behave as before — but the DB no longer
  rejects duplicate-by-case inserts.
- **No interop with stock Anki.** `.colpkg` exports from this build
  are prolly-format and unopenable by upstream Anki. AnkiWeb sync is
  not preserved.

## How the port works

| Concern | Approach |
|---|---|
| FFI | Pre-built `libdoltlite.a` from DoltHub's GitHub releases, staged under `rslib/doltlite-sys/lib/` and linked via the standard `libsqlite3-sys` env-var redirect (`.cargo/config.toml`). No spoof crate, no patched bindings. |
| Engine activation | The prolly engine ships as object files that self-register via `sqlite3_auto_extension`. `build/doltlite_force_load.rs` emits per-platform whole-archive linker flags from every crate that produces a binary or cdylib that links `rslib`. |
| Migration | `rslib/src/storage/migrate_from_sqlite.rs` detects the file format from the first 16 bytes (`CTLD` vs `SQLite format 3`), copies the file to a scratch path, strips `COLLATE unicase` and `-- ` line comments from the stored DDL via `PRAGMA writable_schema`, then replays sanitized DDL into a fresh prolly DB and row-copies every user table `ORDER BY` primary key (prolly's planner refuses bare scans on `WITHOUT ROWID`). |
| Pragma compatibility | `is_prolly_engine()` runs `SELECT doltlite_engine()` at open; B-tree-only pragmas (`journal_mode`, `locking_mode`, `page_size`, `legacy_file_format`, `wal_checkpoint`) are skipped on prolly. |
| Qt UX | `AnkiQt._loadCollection` peeks the first 16 bytes; if the file is still SQLite-format it wraps the open call with `mw.progress.start(label=tr.qt_misc_converting_collection())`. Already-migrated files skip the spinner. |
| Tests | 294 pass, 35 marked `#[ignore]` (unicase casualties + 2 sync round-trips, both explicitly out of scope). `migrate_real_legacy_file` is the end-to-end check: synthesize a stock SQLite fixture via the system `sqlite3` CLI, migrate, confirm CTLD magic + prolly engine + data round-trip including a `WITHOUT ROWID` table. |

## Setup

```bash
# Once per dev machine (or after bumping the Doltlite pin).
bash tools/fetch-doltlite.sh
# Then build/check as usual.
cargo build --workspace
```

`tools/fetch-doltlite.sh` downloads the pre-built lib for the host
platform from DoltHub's GitHub releases (pin: `DOLTLITE_VERSION=0.11.0`,
overridable via env). The result is staged under
`rslib/doltlite-sys/{lib,include}/` — both directories are gitignored.

## Verifying you're actually on Doltlite

```bash
# 1. File header should start with CTLD, not "SQLite format 3":
xxd -l 16 ~/Library/Application\ Support/Anki2/User\ 1/collection.anki2

# 2. Stock sqlite3 cannot read prolly files; this should fail loudly:
sqlite3 ~/Library/Application\ Support/Anki2/User\ 1/collection.anki2 'SELECT 1'

# 3. Doltlite's own CLI does (download from the same release as the lib):
doltlite ~/Library/Application\ Support/Anki2/User\ 1/collection.anki2 \
    'SELECT doltlite_engine();'
# prolly
```

Inside Anki itself, the debug console (`Ctrl+Shift+;`) accepts
`mw.col.db.scalar("SELECT doltlite_engine()")` — returns `'prolly'`.

## Scope explicitly deferred

- Surfacing `dolt_commit` / `dolt_log` / `dolt_diff_*` / `dolt_merge`
  in the Anki UI, Python API, or protobuf surface. The functions are
  available from any SQL connection; wrapping them is a follow-up.
- `.apkg` / `.colpkg` import of stock-SQLite shared decks. Decision
  punted — addressed in a separate plan.
- Sync protocol triage. May break against stock-Anki peers.
- `Tools → Check Database` count discrepancies. Noted, not investigated.

## License

[LICENSE](./LICENSE)
