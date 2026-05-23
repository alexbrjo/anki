// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// Items here are called only from `SqliteStorage::open_or_create_collection_db`
// and `MediaDatabase::open_or_create`; tests cover the rest.
#![allow(dead_code)]

//! Migration shim: legacy SQLite `.anki2` / `.mdb` → Doltlite-format file.
//!
//! ## Lifecycle
//!
//! The `anki-migrate-sqlite` helper binary is *shipped by default* (it
//! has to be — any user with an existing collection needs it once) but
//! *invoked only on demand*: `ensure_doltlite()` short-circuits with a
//! file-header check and never spawns the subprocess unless a legacy
//! SQLite file is actually present. Fresh installs and
//! already-migrated collections never touch it.
//!
//! ## Format detection
//!
//! Follows SQLite's documented file header
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
/// Doltlite's prolly-tree file format magic: bytes 0..4 of every
/// chunk-store-backed database. Stands for the Doltlite chunk store
/// container ("CTLD"). Verified empirically by writing a fresh DB
/// against `libdoltlite.a`.
const DOLTLITE_PROLLY_MAGIC: &[u8; 4] = b"CTLD";

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
        // Doltlite treats an empty file as "create fresh" — match that.
        return Ok(DbFormat::Absent);
    }
    // Prolly-tree native: 4-byte CTLD magic at offset 0. This is the
    // post-migration format we want.
    if n >= 4 && &header[..4] == DOLTLITE_PROLLY_MAGIC {
        return Ok(DbFormat::Doltlite);
    }
    // Legacy SQLite (B-tree). The 16-byte magic + 4-byte application_id
    // at offset 68 follow the SQLite file-format spec. Files we migrated
    // under a prior (B-tree-Doltlite) build carried our
    // DOLTLITE_APPLICATION_ID sentinel but were still SQLite-format on
    // disk — under the prolly build those need re-migrating, so we
    // unconditionally treat any SQLite-magic file as legacy.
    if n >= 16 && &header[..16] == SQLITE_MAGIC {
        return Ok(DbFormat::LegacySqlite);
    }
    Ok(DbFormat::Unknown)
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
            "anki-migrate-sqlite helper binary not found. Looked next to the \
             current executable, next to the loaded librsbridge dylib, in \
             ./target/{debug,release}/ from the current working directory, \
             and on PATH. Set the environment variable \
             ANKI_MIGRATE_SQLITE_BIN to an absolute path to override.",
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

/// Locate the `anki-migrate-sqlite` binary. Search order:
///   1. `ANKI_MIGRATE_SQLITE_BIN` env var (escape hatch)
///   2. Next to `std::env::current_exe()` — works for direct `anki`
///      binary invocation and `cargo test`
///   3. One directory up (covers `cargo test`'s `target/debug/deps/`)
///   4. Next to the loaded librsbridge dylib — works for launches via
///      Python/aqt where `current_exe()` is the Python interpreter
///   5. `./target/{debug,release}/` from the current working directory
///      — covers `python -m aqt` from the project root in dev
///   6. PATH lookup
fn locate_helper() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "anki-migrate-sqlite.exe"
    } else {
        "anki-migrate-sqlite"
    };

    // 1. Explicit override.
    if let Some(p) = std::env::var_os("ANKI_MIGRATE_SQLITE_BIN") {
        let candidate = PathBuf::from(p);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // 2-3. Next to current_exe and one dir up.
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let candidate = dir.join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
            if let Some(parent) = dir.parent() {
                let candidate = parent.join(exe_name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    // 4. Next to the loaded librsbridge dylib (Python-launched case).
    if let Some(dylib_dir) = current_dylib_dir() {
        let candidate = dylib_dir.join(exe_name);
        if candidate.exists() {
            return Some(candidate);
        }
        // The dylib lives at out/rust/{debug,release}/librsbridge.dylib;
        // bins land in target/{debug,release}/. Try both.
        if let Some(parent) = dylib_dir.parent() {
            let candidate = parent.join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 5. Cargo / Anki-build output dirs from cwd. Profile-agnostic:
    //    whichever build profile is active for this run, look there.
    if let Ok(cwd) = std::env::current_dir() {
        for sub in [
            "out/rust/debug",
            "out/rust/release",
            "out/rust/release-lto",
            "target/debug",
            "target/release",
        ] {
            let candidate = cwd.join(sub).join(exe_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 6. PATH.
    which_on_path(exe_name)
}

/// Find the directory containing the dynamic library that this code is
/// linked into. Uses `dladdr` on a stable symbol from this module to
/// resolve the dylib path at runtime. Returns None on Windows or if the
/// lookup fails.
#[cfg(unix)]
fn current_dylib_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::raw::{c_int, c_void};

    #[repr(C)]
    struct DlInfo {
        dli_fname: *const std::os::raw::c_char,
        dli_fbase: *mut c_void,
        dli_sname: *const std::os::raw::c_char,
        dli_saddr: *mut c_void,
    }
    unsafe extern "C" {
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    }

    let mut info: DlInfo = unsafe { std::mem::zeroed() };
    // Take the address of a function in this crate as the probe.
    let probe = current_dylib_dir as *const c_void;
    let ok = unsafe { dladdr(probe, &mut info) };
    if ok == 0 || info.dli_fname.is_null() {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(info.dli_fname) };
    let path = PathBuf::from(cstr.to_str().ok()?);
    path.parent().map(|p| p.to_path_buf())
}

#[cfg(not(unix))]
fn current_dylib_dir() -> Option<PathBuf> {
    None
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
    fn doltlite_prolly_magic_recognised() {
        // CTLD magic at offset 0 → Doltlite-native prolly format.
        let mut header = [0u8; 72];
        header[..4].copy_from_slice(DOLTLITE_PROLLY_MAGIC);
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
        header[..4].copy_from_slice(DOLTLITE_PROLLY_MAGIC);
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
    ///
    /// NOTE: we are currently linking Doltlite's stock-SQLite-compat
    /// amalgamation, *not* the prolly tree engine — see
    /// `rslib/doltlite-sys/PROLLY_BLOCKER.md`. So this only verifies
    /// we're running Doltlite-the-binary, not Doltlite-the-prolly-tree.
    /// On-disk format remains legacy SQLite until the upstream
    /// collation restriction is lifted.
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

    /// Confirms the prolly tree backend is active for new databases.
    /// This is the load-bearing test for the whole Doltlite-minded
    /// design: if it fails, we're back to legacy SQLite on disk and
    /// none of the VC payoff is reachable.
    #[test]
    fn fresh_dbs_use_prolly_engine() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let engine: String = conn
            .query_row("SELECT doltlite_engine()", [], |r| r.get(0))
            .expect("doltlite_engine() missing — linked against compat \
                     amalgamation, not libdoltlite.a");
        assert_eq!(engine, "prolly", "expected prolly engine, got {engine}");
    }

    #[test]
    fn probe_isolate_media_corruption() {
        // Narrow down WHICH statement in the media schema breaks
        // sqlite_master round-trip on prolly.
        let cases: &[(&str, &str)] = &[
            ("simple", "CREATE TABLE t (id INTEGER PRIMARY KEY, x TEXT);"),
            (
                "without_rowid",
                "CREATE TABLE t (id INTEGER PRIMARY KEY, x TEXT) WITHOUT ROWID;",
            ),
            (
                "comments_in_ddl",
                "CREATE TABLE t (id INTEGER PRIMARY KEY, -- a comment\n x TEXT);",
            ),
            (
                "partial_index",
                "CREATE TABLE t (id INTEGER PRIMARY KEY, dirty INT);\nCREATE INDEX i ON t(dirty) WHERE dirty = 1;",
            ),
        ];
        for (label, sql) in cases {
            let tmp = tempfile::NamedTempFile::new().unwrap();
            let p = tmp.path().to_owned();
            drop(tmp);
            {
                let c = rusqlite::Connection::open(&p).unwrap();
                c.execute_batch(sql).unwrap();
            }
            let c2 = rusqlite::Connection::open(&p).unwrap();
            let r: Result<i64, _> = c2.query_row(
                "SELECT count(*) FROM sqlite_master",
                [],
                |r| r.get(0),
            );
            eprintln!("{label}: re-open -> {r:?}");
            std::fs::remove_file(&p).ok();
        }
    }

    #[test]
    fn probe_prolly_supports_media_schema() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let p = tmp.path().to_owned();
        drop(tmp);
        let c = rusqlite::Connection::open(&p).unwrap();
        let r = c.execute_batch(include_str!(
            "../sync/media/database/client/schema.sql"
        ));
        eprintln!("media schema replay -> {r:?}");
        drop(c);
        // Re-open and try to read sqlite_master
        let c2 = rusqlite::Connection::open(&p).unwrap();
        let r2: Result<i64, _> =
            c2.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0));
        eprintln!("re-open sqlite_master count -> {r2:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn probe_prolly_file_header() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let p = tmp.path().to_owned();
        drop(tmp);
        {
            let c = rusqlite::Connection::open(&p).unwrap();
            c.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);")
                .unwrap();
        }
        let bytes = std::fs::read(&p).unwrap();
        eprintln!("prolly file size: {}", bytes.len());
        let n = 32.min(bytes.len());
        eprintln!("first 32 bytes (hex): {:02x?}", &bytes[..n]);
        let printable: String = bytes[..16.min(bytes.len())]
            .iter()
            .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' })
            .collect();
        eprintln!("first 16 ascii: {printable:?}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn env_var_override_takes_priority() {
        let f = tempfile::NamedTempFile::new().unwrap();
        // Write some bytes so the path "exists" semantically.
        std::fs::write(f.path(), b"ignored").unwrap();
        let prev = std::env::var_os("ANKI_MIGRATE_SQLITE_BIN");
        // SAFETY: tests in this crate are not run in parallel with code
        // that consults this env var.
        unsafe { std::env::set_var("ANKI_MIGRATE_SQLITE_BIN", f.path()) };
        let located = locate_helper();
        match prev {
            Some(v) => unsafe { std::env::set_var("ANKI_MIGRATE_SQLITE_BIN", v) },
            None => unsafe { std::env::remove_var("ANKI_MIGRATE_SQLITE_BIN") },
        }
        assert_eq!(located.as_deref(), Some(f.path()));
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
    /// the file ends up Doltlite-prolly with intact data and a
    /// .legacy-backup copy preserved.
    ///
    /// `#[ignore]`'d in the prolly world because creating a true
    /// SQLite-format file requires linking against stock SQLite, which
    /// our workspace `[patch.crates-io]` replaces with Doltlite-prolly.
    /// To exercise this path: `cp /some/real/collection.anki2 ...` from
    /// a fixture, or shell out to the `sqlite3` CLI. Manual verification
    /// of the migration happens by opening Anki against a real
    /// pre-existing collection (the user's path).
    #[test]
    #[ignore = "requires a real SQLite-format file; rusqlite now writes prolly"]
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
