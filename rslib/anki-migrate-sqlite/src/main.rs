//! anki-migrate-sqlite: one-shot legacy → Doltlite database migrator.
//!
//! Invoked by the main `anki` binary's storage layer when it detects a
//! legacy SQLite-format collection/media DB on first open.
//!
//! Usage:
//!     anki-migrate-sqlite <legacy.anki2> <out.anki2>
//!
//! Flow:
//!   1. Open <legacy> read-only with rusqlite (bundled SQLite).
//!   2. Read schema DDL from sqlite_master, replay against a fresh
//!      Doltlite-backed <out>.
//!   3. Stream rows table-by-table inside a single transaction.
//!      Explicit integer PKs on notes/cards/revlog/graves preserve rowid.
//!   4. PRAGMA application_id = 0xA0C1D01D, VACUUM, exit 0.
//!
//! TRACER STATUS: the binary builds and parses args, but the dump loop
//! itself is a TODO until doltlite-sys vendors real Doltlite. The tracer
//! stub re-exports rusqlite, so running it today would just copy
//! SQLite → SQLite — not yet useful, but the shape is in place.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const DOLTLITE_APPLICATION_ID: i32 = 0xA0C1_D01D_u32 as i32;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        bail!("usage: {} <legacy.anki2> <out.anki2>", args[0]);
    }
    let src = PathBuf::from(&args[1]);
    let dst = PathBuf::from(&args[2]);

    if !src.exists() {
        bail!("source DB does not exist: {}", src.display());
    }
    if dst.exists() {
        bail!("dest path already exists (refusing to overwrite): {}", dst.display());
    }

    migrate(&src, &dst).with_context(|| format!("migrating {} -> {}", src.display(), dst.display()))
}

fn migrate(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    use rusqlite::OpenFlags;

    let source = rusqlite::Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let dest = doltlite::Connection::open(dst)?;

    // 1. Copy schema (skip internal sqlite_* tables).
    let mut stmt = source.prepare(
        "SELECT sql FROM sqlite_master \
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
         ORDER BY CASE type WHEN 'table' THEN 1 WHEN 'index' THEN 2 ELSE 3 END",
    )?;
    let ddls: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    for ddl in &ddls {
        dest.execute_batch(ddl)?;
    }

    // 2. Copy rows table-by-table. (TODO: implement row streaming with
    //    type-erased rusqlite::types::Value reads and parameterised inserts.
    //    Punted in the tracer cut — needs careful handling of BLOBs and
    //    rowid preservation.)
    eprintln!("anki-migrate-sqlite: schema replayed; row copy TODO");

    // 3. Sentinel.
    dest.pragma_update(None, "application_id", DOLTLITE_APPLICATION_ID)?;
    dest.execute_batch("VACUUM")?;
    Ok(())
}
