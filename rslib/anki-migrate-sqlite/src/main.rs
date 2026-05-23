//! CLI shim around the migration library. See `lib.rs` for the real
//! work. Invoked by the storage layer as a subprocess to keep
//! Doltlite's `sqlite3_*` symbols isolated from any bundled SQLite
//! linked into the parent `anki` process.
//!
//! Usage:
//!     anki-migrate-sqlite <legacy.anki2> <out.anki2>

use anki_migrate_sqlite::migrate;
use anyhow::{bail, Result};
use std::path::PathBuf;

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
        bail!(
            "dest path already exists (refusing to overwrite): {}",
            dst.display()
        );
    }

    let stats = migrate(&src, &dst)?;
    eprintln!(
        "anki-migrate-sqlite: {} tables, {} rows copied",
        stats.tables, stats.rows
    );
    Ok(())
}
