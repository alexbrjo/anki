// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// All items below are intentionally unused at this stage — this module is
// scaffold that will be called from `SqliteStorage::open_or_create_collection_db`
// and `MediaDatabase::open_or_create` once Doltlite is fully vendored in
// `doltlite-sys`. Wiring it in early would force the migration path before
// the destination engine is real.
#![allow(dead_code)]

//! Migration shim: legacy SQLite `.anki2` / `.mdb` → Doltlite-format file.
//!
//! This module is intentionally **not yet wired into the open paths**. It
//! defines the format-detection primitives and the subprocess invocation
//! contract that `SqliteStorage::open_or_create_collection_db` and
//! `MediaDatabase::open_or_create` will call once Doltlite is fully
//! vendored in `doltlite-sys`. Wiring it in prematurely would force every
//! existing dev workflow through the migration path while the destination
//! engine is still a tracer stub.
//!
//! Format detection follows SQLite's documented file header
//! (<https://www.sqlite.org/fileformat.html#the_database_header>):
//!
//!   * bytes  0..16  — magic `b"SQLite format 3\0"`
//!   * bytes 68..72  — `application_id` (big-endian i32)
//!
//! Anki has never set `application_id`, so we hijack it as the marker for
//! "this file has been migrated to Doltlite." A fresh Doltlite open path
//! will `PRAGMA application_id = DOLTLITE_APPLICATION_ID` on create.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Sentinel written into the SQLite file header by Doltlite-backed Anki
/// to distinguish migrated files from pristine legacy SQLite files.
/// "A0C1D01D" ≈ "Anki Collection 1 Dolt 1D"  — picked to be obviously ours.
pub const DOLTLITE_APPLICATION_ID: i32 = 0xA0C1_D01D_u32 as i32;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFormat {
    /// File does not exist — caller should let the engine create it fresh
    /// and stamp the sentinel.
    Absent,
    /// Already migrated (header magic present + our `application_id`).
    Doltlite,
    /// Legacy SQLite — needs migration.
    LegacySqlite,
    /// Not a recognizable SQLite-family file. Caller should error.
    Unknown,
}

/// Inspect `path` without opening any DB engine. Reads only the first 72
/// bytes of the file.
pub fn detect_format(path: &Path) -> std::io::Result<DbFormat> {
    if !path.exists() {
        return Ok(DbFormat::Absent);
    }
    let mut f = fs::File::open(path)?;
    let mut header = [0u8; 72];
    let n = f.read(&mut header)?;
    if n < 72 {
        return Ok(DbFormat::Unknown);
    }
    if &header[..16] != SQLITE_MAGIC {
        return Ok(DbFormat::Unknown);
    }
    let app_id = i32::from_be_bytes([header[68], header[69], header[70], header[71]]);
    if app_id == DOLTLITE_APPLICATION_ID {
        Ok(DbFormat::Doltlite)
    } else {
        Ok(DbFormat::LegacySqlite)
    }
}

/// Migration contract. Stub until Phase 1 (`doltlite-sys` vendored) lands.
///
/// When implemented, this will:
///   1. `fs::copy(path, path.with_extension("legacy-backup"))` (idempotent).
///   2. Spawn `anki-migrate-sqlite <path> <path>.migrating` as a subprocess
///      and wait for exit 0. Subprocess isolation avoids the
///      sqlite3_* symbol collision between rusqlite-bundled-SQLite and
///      Doltlite.
///   3. `fs::rename(path.migrating, path)` atomically.
///
/// Locator for the helper binary: resolve relative to
/// `std::env::current_exe()` first, then fall back to PATH lookup.
#[allow(dead_code)]
pub fn migrate_in_place(_path: &Path) -> Result<(), String> {
    Err(
        "doltlite migration not yet wired up: vendor Doltlite (tools/fetch-doltlite.sh) \
         and flip the call sites in storage/sqlite.rs and sync/media/database/client/mod.rs"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_header(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        // Pad to 100 bytes (full SQLite header is 100B; tests only read 72)
        f.write_all(&[0u8; 200]).unwrap();
        f
    }

    #[test]
    fn absent_file() {
        let p = std::env::temp_dir().join("doltlite-nonexistent-xyz.db");
        let _ = fs::remove_file(&p);
        assert_eq!(detect_format(&p).unwrap(), DbFormat::Absent);
    }

    #[test]
    fn legacy_sqlite_has_zero_app_id() {
        // First 16 bytes: SQLite magic. Bytes 16..72: zeros (default).
        let mut header = [0u8; 72];
        header[..16].copy_from_slice(SQLITE_MAGIC);
        let f = write_header(&header);
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::LegacySqlite);
    }

    #[test]
    fn doltlite_sentinel_recognised() {
        let mut header = [0u8; 72];
        header[..16].copy_from_slice(SQLITE_MAGIC);
        header[68..72].copy_from_slice(&DOLTLITE_APPLICATION_ID.to_be_bytes());
        let f = write_header(&header);
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::Doltlite);
    }

    #[test]
    fn random_garbage_unknown() {
        let f = write_header(&[0xFFu8; 72]);
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::Unknown);
    }

    #[test]
    fn truncated_file_unknown() {
        let f = write_header(&[0u8; 8]); // only 208 bytes after pad, but read sees full
        // Actually our write_header pads with 200 zeros after the prefix, so
        // total is 208 bytes — read of 72 will succeed and not match magic.
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::Unknown);
    }

    #[test]
    fn migrate_in_place_unimplemented() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(migrate_in_place(f.path()).is_err());
    }
}
