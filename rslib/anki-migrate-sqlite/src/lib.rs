//! Library entry point for the one-shot legacy SQLite → Doltlite
//! migrator. See `main.rs` for the CLI shim.
//!
//! Flow:
//!   1. Open <src> read-only with rusqlite (bundled SQLite).
//!   2. Read schema DDL from sqlite_master, replay tables against a
//!      fresh Doltlite-backed <dst>.
//!   3. Stream rows table-by-table inside a single transaction.
//!      Explicit integer PKs preserve rowid.
//!   4. Create indexes/triggers/views after bulk inserts.
//!   5. Preserve user_version, stamp Doltlite application_id sentinel.
//!
//! TRACER NOTE: while `doltlite` re-exports `rusqlite`, this copies
//! SQLite → SQLite. Once doltlite-sys provides real Doltlite-backed
//! rusqlite, the destination becomes a prolly-tree DB with no changes
//! here.

use anyhow::{Context, Result};
use rusqlite::types::Value;
use std::path::Path;

/// The Doltlite application_id sentinel: bytes 68..72 of the DB header
/// after a successful migration. Lets the storage layer skip the
/// migration shim on subsequent opens.
pub const DOLTLITE_APPLICATION_ID: i32 = 0xA0C1_D01D_u32 as i32;

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub tables: usize,
    pub rows: usize,
}

pub fn migrate(src: &Path, dst: &Path) -> Result<Stats> {
    use rusqlite::OpenFlags;

    // Open source READ-WRITE: we need to rewrite sqlite_master in place
    // to strip COLLATE annotations before SELECT can prepare against
    // tables that reference them. The shim's `.legacy-backup` copy
    // preserves the user's original file, so mutating this one is safe.
    let source = rusqlite::Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("opening source SQLite DB read-write")?;

    let dest = doltlite::Connection::open(dst).context("creating dest DB")?;

    // Rewrite the source's stored DDL to drop `COLLATE unicase` and
    // line comments. Doltlite-prolly disallows registering custom
    // collations, so we cannot supply a stub; we must instead make
    // the schema not reference the unknown collation at all.
    //
    // `PRAGMA writable_schema = ON` makes sqlite_master writable in
    // this connection (changes persist to disk but only become visible
    // to other connections after the conn is closed). After mutating
    // we don't need to bring it back to OFF — connection-scoped.
    sanitize_source_schema_in_place(&source)
        .context("sanitizing source DDL for prolly compatibility")?;

    dest.execute_batch("BEGIN")?;

    let table_ddls = read_ddls(&source, "table")?;
    for (_, ddl) in &table_ddls {
        let sanitized = sanitize_ddl(ddl);
        dest.execute_batch(&sanitized)
            .with_context(|| format!("replaying table DDL: {sanitized}"))?;
    }

    let mut stats = Stats::default();
    for (name, _) in &table_ddls {
        let copied = copy_table(&source, &dest, name)
            .with_context(|| format!("copying rows from {name}"))?;
        stats.rows += copied;
        stats.tables += 1;
    }

    for kind in ["index", "trigger", "view"] {
        for (_, ddl) in read_ddls(&source, kind)? {
            let sanitized = sanitize_ddl(&ddl);
            dest.execute_batch(&sanitized)
                .with_context(|| format!("replaying {kind} DDL"))?;
        }
    }

    let user_version: i32 = source.pragma_query_value(None, "user_version", |r| r.get(0))?;
    dest.pragma_update(None, "user_version", user_version)?;
    dest.pragma_update(None, "application_id", DOLTLITE_APPLICATION_ID)?;
    dest.execute_batch("COMMIT")?;
    Ok(stats)
}

/// Build an `ORDER BY` clause that gives Doltlite-prolly's planner a
/// concrete walk order for `SELECT * FROM "<table>"`. Uses the table's
/// declared PRIMARY KEY columns (sorted by pk-position) if any; falls
/// back to `ORDER BY rowid` for legacy ROWID tables with no declared PK.
fn order_by_clause(conn: &rusqlite::Connection, table: &str) -> Result<String> {
    // PRAGMA table_info returns rows of (cid, name, type, notnull, dflt, pk).
    // pk = 0 means "not part of a primary key"; for composite PKs the
    // pk column holds the 1-based position within the key.
    let pragma = format!(r#"PRAGMA table_info("{}")"#, table.replace('"', "\"\""));
    let mut stmt = conn.prepare(&pragma)?;
    let mut pk_cols: Vec<(i32, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, i32>(5)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(pk, _)| *pk > 0)
        .collect();
    if pk_cols.is_empty() {
        // No declared PK → ROWID table; rowid is always plannable.
        return Ok(" ORDER BY rowid".into());
    }
    pk_cols.sort_by_key(|(pk, _)| *pk);
    let cols: Vec<String> = pk_cols
        .into_iter()
        .map(|(_, name)| format!(r#""{}""#, name.replace('"', "\"\"")))
        .collect();
    Ok(format!(" ORDER BY {}", cols.join(",")))
}

/// Rewrite the source's `sqlite_master.sql` rows in place to strip
/// `COLLATE unicase` and SQL line comments, so subsequent SELECTs can
/// be prepared by Doltlite-prolly (which doesn't permit registering
/// the original collation).
///
/// Uses `PRAGMA writable_schema` — the legacy SQLite escape hatch for
/// editing sqlite_master directly. Doltlite-prolly inherits SQLite's
/// pragma handling, so this works.
fn sanitize_source_schema_in_place(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA writable_schema = ON")?;
    // Collect rows that need rewriting; mutate after to avoid
    // iterator-invalidation under in-place UPDATE.
    let mut to_rewrite: Vec<(String, String, String)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (ty, name, sql) = row?;
            let sanitized = sanitize_ddl(&sql);
            if sanitized != sql {
                to_rewrite.push((ty, name, sanitized));
            }
        }
    }
    for (ty, name, sanitized) in to_rewrite {
        conn.execute(
            "UPDATE sqlite_master SET sql = ?1 WHERE type = ?2 AND name = ?3",
            rusqlite::params![sanitized, ty, name],
        )?;
    }
    // No need to flip writable_schema back to OFF — it's
    // connection-scoped and the conn is dropped shortly.
    Ok(())
}

/// Normalise a DDL string read from a legacy SQLite source into a form
/// Doltlite-prolly accepts:
///   - strips `COLLATE unicase` (prolly disallows user collations)
///   - strips `-- line comments` (prolly's sqlite_master round-trip
///     truncates at line comments and reports the schema as corrupt)
///
/// The result is semantically equivalent to the original for everything
/// Anki cares about, but is safe to replay against a prolly engine.
fn sanitize_ddl(ddl: &str) -> String {
    strip_line_comments(&strip_collate_unicase(ddl))
}

/// Remove `-- ... \n` line comments from a DDL string. Block comments
/// (`/* ... */`) are kept since prolly handles them fine.
fn strip_line_comments(ddl: &str) -> String {
    let mut out = String::with_capacity(ddl.len());
    let bytes = ddl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            // Skip to end of line.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Remove `COLLATE unicase` substrings from a DDL string so it can be
/// replayed against a Doltlite-prolly connection. We accept the
/// semantic loss (case-sensitive name uniqueness instead of Unicode
/// case-insensitive) as the Doltlite-minded tradeoff.
///
/// Match is case-insensitive on the keyword + collation name, with
/// arbitrary whitespace allowed between them. Handles both
/// `name text COLLATE unicase,` and trailing forms like
/// `tag text NOT NULL PRIMARY KEY COLLATE unicase`.
fn strip_collate_unicase(ddl: &str) -> String {
    // Walk the string char-by-char, matching `COLLATE\s+unicase` case-insensitively.
    let bytes = ddl.as_bytes();
    let mut out = String::with_capacity(ddl.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = try_match_collate_unicase(&bytes[i..]) {
            i += end;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// If `slice` starts with `COLLATE\s+unicase` (case-insensitive,
/// followed by a non-identifier char or EOI), return the length
/// consumed. Otherwise None.
fn try_match_collate_unicase(slice: &[u8]) -> Option<usize> {
    const KW: &[u8] = b"collate";
    const NAME: &[u8] = b"unicase";
    if slice.len() < KW.len() {
        return None;
    }
    for (i, &b) in KW.iter().enumerate() {
        if slice[i].to_ascii_lowercase() != b {
            return None;
        }
    }
    let mut j = KW.len();
    let mut saw_ws = false;
    while j < slice.len() && (slice[j] == b' ' || slice[j] == b'\t' || slice[j] == b'\n') {
        saw_ws = true;
        j += 1;
    }
    if !saw_ws || slice.len() - j < NAME.len() {
        return None;
    }
    for (i, &b) in NAME.iter().enumerate() {
        if slice[j + i].to_ascii_lowercase() != b {
            return None;
        }
    }
    let end = j + NAME.len();
    // Ensure the match doesn't extend into a longer identifier
    // (e.g. `unicase_foo`).
    if let Some(&next) = slice.get(end) {
        if next.is_ascii_alphanumeric() || next == b'_' {
            return None;
        }
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_ddl, strip_collate_unicase, strip_line_comments};

    #[test]
    fn sanitize_strips_collate_and_comments() {
        let ddl = "CREATE TABLE t (\n  id INTEGER, -- comment\n  name TEXT COLLATE unicase\n);";
        let out = sanitize_ddl(ddl);
        assert!(!out.to_lowercase().contains("collate"));
        assert!(!out.contains("--"));
        assert!(out.contains("CREATE TABLE t"));
    }

    #[test]
    fn strip_line_comments_leaves_block_comments() {
        assert_eq!(
            strip_line_comments("a -- gone\nb /* kept */ c"),
            "a \nb /* kept */ c"
        );
    }

    #[test]
    fn strips_lowercase() {
        assert_eq!(
            strip_collate_unicase("name text COLLATE unicase NOT NULL"),
            "name text  NOT NULL"
        );
    }

    #[test]
    fn strips_mixed_case() {
        assert_eq!(
            strip_collate_unicase("Name TEXT collate UNICASE,"),
            "Name TEXT ,"
        );
    }

    #[test]
    fn strips_multiline() {
        assert_eq!(
            strip_collate_unicase("name text NOT NULL COLLATE\n   unicase\n,"),
            "name text NOT NULL \n,"
        );
    }

    #[test]
    fn leaves_other_collations_alone() {
        let ddl = "name text COLLATE nocase NOT NULL";
        assert_eq!(strip_collate_unicase(ddl), ddl);
    }

    #[test]
    fn leaves_unicase_substring_alone() {
        let ddl = "comment text -- contains the word collate unicase_v2 here";
        assert_eq!(strip_collate_unicase(ddl), ddl);
    }
}

fn read_ddls(conn: &rusqlite::Connection, kind: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT name, sql FROM sqlite_master \
         WHERE type = ?1 AND sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
         ORDER BY rowid",
    )?;
    let rows = stmt
        .query_map([kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn copy_table(
    src: &rusqlite::Connection,
    dst: &doltlite::Connection,
    table: &str,
) -> Result<usize> {
    // Doltlite-prolly's planner returns "no query solution" for bare
    // `SELECT * FROM t` against WITHOUT ROWID tables (Anki's `fields`,
    // `templates`, `tags`, `config`). Always ORDER BY the PK so the
    // planner has a key it can walk; works equally well for plain
    // ROWID tables with a declared PK.
    let order = order_by_clause(src, table)?;
    let select_sql = format!(
        r#"SELECT * FROM "{}"{}"#,
        table.replace('"', "\"\""),
        order
    );
    let mut select = src.prepare(&select_sql)?;
    let col_count = select.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| select.column_name(i).unwrap_or("?").to_string())
        .collect();

    let placeholders = (1..=col_count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let col_list = col_names
        .iter()
        .map(|n| format!(r#""{}""#, n.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(",");
    let insert_sql = format!(
        r#"INSERT INTO "{}" ({}) VALUES ({})"#,
        table.replace('"', "\"\""),
        col_list,
        placeholders
    );
    let mut insert = dst.prepare(&insert_sql)?;

    let mut rows = select.query([])?;
    let mut n = 0usize;
    while let Some(row) = rows.next()? {
        let values: Vec<Value> = (0..col_count)
            .map(|i| row.get::<_, Value>(i))
            .collect::<rusqlite::Result<_>>()?;
        insert.execute(rusqlite::params_from_iter(values.iter()))?;
        n += 1;
    }
    Ok(n)
}
