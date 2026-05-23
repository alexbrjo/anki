// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// Items here are called only from `SqliteStorage::open_or_create_collection_db`
// and `MediaDatabase::open_or_create`; tests cover the rest.
#![allow(dead_code)]

//! Migration shim: legacy SQLite `.anki2` / `.mdb` → Doltlite-format file.
//!
//! Format detection follows SQLite's documented file header
//! (<https://www.sqlite.org/fileformat.html#the_database_header>):
//!
//!   * bytes  0..16  — magic `b"SQLite format 3\0"`
//!   * bytes 68..72  — `application_id` (big-endian i32)
//!
//! Anki has never set `application_id`, so we hijack it as the marker for
//! "this file has been migrated to Doltlite." A fresh Doltlite open path
//! `PRAGMA application_id = DOLTLITE_APPLICATION_ID` on create.
//!
//! Actual migration is performed by the `anki-migrate-sqlite` helper bin
//! spawned as a subprocess — this keeps Doltlite's `sqlite3_*` symbols
//! isolated from any bundled SQLite linked into the parent `anki`
//! process.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Sentinel written into the SQLite file header by Doltlite-backed Anki
/// to distinguish migrated files from pristine legacy SQLite files.
/// "A0C1D01D" ≈ "Anki Collection 1 Dolt 1D" — picked to be obviously ours.
pub const DOLTLITE_APPLICATION_ID: i32 = 0xA0C1_D01D_u32 as i32;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFormat {
    /// File does not exist — caller should let the engine create it
    /// fresh and stamp the sentinel.
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
    if n == 0 {
        // SQLite treats an empty file as "create fresh" — match that.
        return Ok(DbFormat::Absent);
    }
    if n < 72 || &header[..16] != SQLITE_MAGIC {
        return Ok(DbFormat::Unknown);
    }
    let app_id = i32::from_be_bytes([header[68], header[69], header[70], header[71]]);
    if app_id == DOLTLITE_APPLICATION_ID {
        Ok(DbFormat::Doltlite)
    } else {
        Ok(DbFormat::LegacySqlite)
    }
}

/// High-level entry point. Idempotent: a no-op for Absent/Doltlite/Unknown.
///
/// For LegacySqlite:
///   1. Copy `path` → `path.with_extension("legacy-backup")` (idempotent).
///   2. Spawn `anki-migrate-sqlite <path> <path>.migrating` and wait
///      for exit 0.
///   3. `fs::rename(path.migrating, path)` atomically (overwriting).
pub fn ensure_doltlite(path: &Path) -> std::io::Result<()> {
    match detect_format(path)? {
        DbFormat::Absent | DbFormat::Doltlite => Ok(()),
        DbFormat::Unknown => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is not a SQLite-format database; refusing to open",
                path.display()
            ),
        )),
        DbFormat::LegacySqlite => migrate_in_place(path),
    }
}

fn migrate_in_place(path: &Path) -> std::io::Result<()> {
    let backup = path.with_extension("legacy-backup");
    if !backup.exists() {
        fs::copy(path, &backup)?;
    }

    let migrating = with_suffix(path, ".migrating");
    if migrating.exists() {
        fs::remove_file(&migrating)?;
    }

    let helper = locate_helper().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "anki-migrate-sqlite helper binary not found next to anki or on PATH",
        )
    })?;

    let status = Command::new(&helper).arg(path).arg(&migrating).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "anki-migrate-sqlite exited with status {status}"
        )));
    }

    // Atomic on POSIX. On Windows fs::rename overwrites unless
    // both paths exist on different volumes, which we control.
    fs::rename(&migrating, path)?;
    Ok(())
}

/// Locate the `anki-migrate-sqlite` binary, looking next to the current
/// exe first, then in CARGO_BIN_EXE_* (set during `cargo test`), then on
/// PATH.
fn locate_helper() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "anki-migrate-sqlite.exe"
    } else {
        "anki-migrate-sqlite"
    };

    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
            // cargo puts test binaries in target/debug/deps; bins in target/debug
            if let Some(parent) = dir.parent() {
                let candidate = parent.join(exe_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    // CARGO_BIN_EXE_<name> is injected for integration tests of the same
    // package; for cross-package use we fall back to PATH lookup.
    which_on_path(exe_name)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_header(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
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
        let f = write_header(&[0u8; 8]);
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::Unknown);
    }

    #[test]
    fn empty_file_treated_as_absent() {
        // Matches SQLite's own behaviour: opening a zero-byte file
        // produces a fresh DB. CollectionBuilder tests rely on this.
        let f = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(detect_format(f.path()).unwrap(), DbFormat::Absent);
    }

    #[test]
    fn ensure_noop_on_absent_file() {
        let p = std::env::temp_dir().join("doltlite-ensure-absent-xyz.db");
        let _ = fs::remove_file(&p);
        assert!(ensure_doltlite(&p).is_ok());
    }

    #[test]
    fn ensure_noop_on_doltlite_file() {
        let mut header = [0u8; 72];
        header[..16].copy_from_slice(SQLITE_MAGIC);
        header[68..72].copy_from_slice(&DOLTLITE_APPLICATION_ID.to_be_bytes());
        let f = write_header(&header);
        assert!(ensure_doltlite(f.path()).is_ok());
    }

    #[test]
    fn ensure_rejects_unknown_file() {
        let f = write_header(&[0xFFu8; 72]);
        let err = ensure_doltlite(f.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Sanity check that the in-process SQL engine is actually Doltlite,
    /// not stock SQLite. Doltlite identifies itself via the `alt1`
    /// suffix on `sqlite_source_id()`.
    #[test]
    fn runtime_is_doltlite() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let source_id: String = conn
            .query_row("SELECT sqlite_source_id()", [], |r| r.get(0))
            .unwrap();
        let version: String = conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .unwrap();
        eprintln!("linked engine: version={version} sourceid={source_id}");
        assert!(
            source_id.contains("alt1"),
            "expected Doltlite marker 'alt1' in sourceid, got: {source_id}"
        );
    }

    #[test]
    fn with_suffix_appends() {
        assert_eq!(
            with_suffix(Path::new("/foo/bar.anki2"), ".migrating"),
            PathBuf::from("/foo/bar.anki2.migrating")
        );
    }

    /// End-to-end: build a legacy SQLite file, run the migration shim
    /// (spawning the real `anki-migrate-sqlite` subprocess), and verify
    /// the file ends up Doltlite-stamped with intact data and a
    /// .legacy-backup copy preserved.
    ///
    /// Skipped (with a warning) if the helper binary isn't built —
    /// `cargo test --workspace` from the project root builds it
    /// automatically; running this test in isolation requires a prior
    /// `cargo build -p anki-migrate-sqlite`.
    #[test]
    fn e2e_legacy_sqlite_is_migrated() {
        if locate_helper().is_none() {
            eprintln!(
                "SKIP: anki-migrate-sqlite helper not found; \
                 run `cargo build -p anki-migrate-sqlite` first"
            );
            return;
        }

        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("collection.anki2");

        // Build a tiny Anki-shaped legacy SQLite file.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE notes (id INTEGER PRIMARY KEY, flds TEXT);
                 INSERT INTO notes VALUES (42, 'hello');",
            )
            .unwrap();
            // Important: do NOT set application_id, so detect_format
            // classifies this as LegacySqlite.
        }

        // Confirm pre-condition.
        assert_eq!(
            detect_format(&db_path).unwrap(),
            DbFormat::LegacySqlite,
            "pre-migration file should be detected as legacy SQLite"
        );

        // Run the shim — this spawns the subprocess.
        ensure_doltlite(&db_path).expect("migration succeeds");

        // Post-condition #1: file is now Doltlite-stamped.
        assert_eq!(
            detect_format(&db_path).unwrap(),
            DbFormat::Doltlite,
            "migrated file should carry the Doltlite sentinel"
        );

        // Post-condition #2: legacy backup exists alongside.
        let backup = db_path.with_extension("legacy-backup");
        assert!(backup.exists(), "legacy-backup should be preserved");

        // Post-condition #3: data survived.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let flds: String = conn
            .query_row("SELECT flds FROM notes WHERE id = 42", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flds, "hello");

        // Post-condition #4: idempotent on re-run (no-op).
        ensure_doltlite(&db_path).expect("second call is a no-op");
        assert_eq!(detect_format(&db_path).unwrap(), DbFormat::Doltlite);
    }
}
