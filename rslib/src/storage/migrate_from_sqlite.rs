// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! One-shot migration of stock-SQLite collection files to Doltlite's prolly
//! engine. Runs the first time Anki opens an existing `.anki2` / `.mdb` after
//! the Doltlite port; subsequent opens see the prolly format and skip past.
//!
//! The migration is in-process: a single `libdoltlite.a` is linked, and it
//! opens both formats transparently. The wrinkle is that prolly mode stubs
//! `sqlite3_create_collation` unconditionally, so we cannot re-register
//! `unicase` to read source DDL that still has `COLLATE unicase` annotations.
//! Instead we open a scratch copy, run `PRAGMA writable_schema=ON` and rewrite
//! `sqlite_master.sql` in-place to strip the COLLATE clauses (and `-- ` line
//! comments, which prolly truncates DDL at when round-tripping). After that
//! the scratch file is plain enough for both engines.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use regex::Regex;
use rusqlite::params;
use rusqlite::Connection;

use crate::prelude::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Format {
    /// File doesn't exist or is empty — a fresh prolly DB will be created.
    Empty,
    /// Doltlite prolly-format file (4-byte `CTLD` magic at offset 0).
    Prolly,
    /// Stock SQLite B-tree file — needs migration before regular open.
    Sqlite,
}

const PROLLY_MAGIC: &[u8] = b"CTLD";
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

pub(crate) fn detect_format(path: &Path) -> Result<Format> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Format::Empty),
        Err(e) => return Err(e.into()),
    };
    if bytes.is_empty() {
        return Ok(Format::Empty);
    }
    if bytes.starts_with(PROLLY_MAGIC) {
        return Ok(Format::Prolly);
    }
    if bytes.starts_with(SQLITE_MAGIC) {
        return Ok(Format::Sqlite);
    }
    invalid_input!("unknown database format at {}", path.display());
}

/// Convert a stock-SQLite collection in place to Doltlite prolly format. The
/// caller arranges any user-facing progress UI. The file at `path` is
/// overwritten only on success (atomic rename from a side file).
pub(crate) fn migrate_in_place(path: &Path) -> Result<()> {
    let scratch = with_suffix(path, ".scratch");
    let migrating = with_suffix(path, ".migrating");

    let _ = fs::remove_file(&scratch);
    let _ = fs::remove_file(&migrating);
    fs::copy(path, &scratch)?;

    sanitize_sqlite_master(&scratch)?;
    copy_rows(&scratch, &migrating)?;

    fs::rename(&migrating, path)?;
    fs::remove_file(&scratch).ok();
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Rewrite the scratch file's `sqlite_master.sql` rows to drop COLLATE
/// annotations and `-- ` line comments. `writable_schema` is dangerous in
/// general — we use it because we're about to throw the file away after
/// reading.
fn sanitize_sqlite_master(scratch: &Path) -> Result<()> {
    let db = Connection::open(scratch)?;
    db.pragma_update(None, "writable_schema", "ON")?;

    let rows: Vec<(i64, Option<String>)> = db
        .prepare("SELECT rowid, sql FROM sqlite_master")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    for (rowid, sql) in rows {
        let Some(sql) = sql else { continue };
        let sanitized = sanitize_ddl(&sql);
        if sanitized != sql {
            db.execute(
                "UPDATE sqlite_master SET sql = ?1 WHERE rowid = ?2",
                params![sanitized, rowid],
            )?;
        }
    }
    db.pragma_update(None, "writable_schema", "OFF")?;
    Ok(())
}

fn sanitize_ddl(sql: &str) -> String {
    // Strip ` COLLATE unicase` annotations (case-insensitive on the keyword).
    let collate = Regex::new(r"(?i)\s+COLLATE\s+unicase").unwrap();
    let s = collate.replace_all(sql, "");

    // Rewrite `-- foo\n` line comments to plain newlines. Block comments and
    // string literals are left alone — prolly only chokes on the line form
    // inside DDL.
    let line_comment = Regex::new(r"--[^\n]*\n?").unwrap();
    line_comment.replace_all(&s, "\n").into_owned()
}

/// Open `src` (stock SQLite) and `dest` (fresh prolly), replay sanitized DDL,
/// then copy every user-table row ordered by primary key. ORDER BY PK is not
/// optional: prolly's planner refuses bare `SELECT * FROM t` on WITHOUT ROWID
/// tables, and Anki has several of those (fields, templates, tags,
/// deck_config).
fn copy_rows(src: &Path, dest: &Path) -> Result<()> {
    let src_db = Connection::open(src)?;
    let dest_db = Connection::open(dest)?;

    let objects: Vec<(String, String, Option<String>)> = src_db
        .prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE type IN ('table', 'index') AND sql IS NOT NULL \
                AND name NOT LIKE 'sqlite_%' \
             ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, rowid",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;

    dest_db.execute_batch("BEGIN")?;
    for (_kind, _name, sql) in &objects {
        if let Some(sql) = sql {
            dest_db.execute_batch(sql)?;
        }
    }

    let tables: Vec<String> = objects
        .iter()
        .filter(|(t, _, _)| t == "table")
        .map(|(_, n, _)| n.clone())
        .collect();
    for table in &tables {
        copy_table(&src_db, &dest_db, table)?;
    }

    dest_db.execute_batch("COMMIT")?;
    Ok(())
}

fn copy_table(src: &Connection, dest: &Connection, table: &str) -> Result<()> {
    let pk_cols = primary_key_columns(src, table)?;
    let order_by = if pk_cols.is_empty() {
        String::new()
    } else {
        format!(
            " ORDER BY {}",
            pk_cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let select_sql = format!("SELECT * FROM \"{table}\"{order_by}");
    let mut stmt = src.prepare(&select_sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let placeholders = (1..=column_count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let col_list = column_names
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!("INSERT INTO \"{table}\" ({col_list}) VALUES ({placeholders})");
    let mut insert = dest.prepare(&insert_sql)?;

    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let values: Vec<rusqlite::types::Value> = (0..column_count)
            .map(|i| row.get::<_, rusqlite::types::Value>(i))
            .collect::<rusqlite::Result<_>>()?;
        insert.execute(rusqlite::params_from_iter(values.iter()))?;
    }
    Ok(())
}

fn primary_key_columns(db: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let mut pk: Vec<(i64, String)> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get("name")?;
        let pk_pos: i64 = row.get("pk")?;
        if pk_pos > 0 {
            pk.push((pk_pos, name));
        }
    }
    pk.sort_by_key(|(pos, _)| *pos);
    Ok(pk.into_iter().map(|(_, n)| n).collect())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn sanitize_strips_collate_and_line_comments() {
        let input = "CREATE TABLE decks (\
            \n  id integer PRIMARY KEY,\
            \n  name text NOT NULL COLLATE unicase,\
            \n  -- inline note\
            \n  usn integer NOT NULL\
            \n)";
        let out = sanitize_ddl(input);
        assert!(!out.contains("COLLATE"), "COLLATE not stripped: {out}");
        assert!(
            !out.contains("-- inline"),
            "line comment not stripped: {out}"
        );
        assert!(out.contains("name text NOT NULL"));
    }

    #[test]
    fn detect_empty_missing_file() {
        let p = std::env::temp_dir().join("anki-migrate-nonexistent.db");
        let _ = fs::remove_file(&p);
        assert_eq!(detect_format(&p).unwrap(), Format::Empty);
    }

    #[test]
    fn fresh_connection_uses_prolly_engine() {
        crate::storage::sqlite::install_doltlite_auto_extension();
        let p = std::env::temp_dir().join("anki-engine-smoke.db");
        let _ = fs::remove_file(&p);
        let db = rusqlite::Connection::open(&p).unwrap();
        let engine: String = db
            .query_row("SELECT doltlite_engine()", [], |r| r.get(0))
            .expect("doltlite_engine() should resolve when the prolly engine is linked");
        assert_eq!(engine, "prolly");
        let head = fs::read(&p).unwrap_or_default();
        assert!(
            head.starts_with(PROLLY_MAGIC),
            "fresh file should start with CTLD, got {:02x?}",
            &head[..head.len().min(16)]
        );
    }
}
