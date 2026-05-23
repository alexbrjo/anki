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

    let source = rusqlite::Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("opening source SQLite DB")?;

    let dest = doltlite::Connection::open(dst).context("creating dest DB")?;

    // Source still needs the legacy `unicase` collation registered to
    // prepare SELECTs over tables that declare `name text COLLATE
    // unicase` (source is stock SQLite, accepts custom collations).
    register_stub_collations_rusqlite(&source)
        .context("registering placeholder collation on source")?;

    // We do NOT register on dest. Doltlite-prolly disallows user
    // collations entirely (see PROLLY_BLOCKER.md). Instead we strip
    // `COLLATE unicase` from every DDL string before replay — the
    // post-migration schema is fully prolly-friendly with BINARY
    // collation. Users lose case-insensitive name uniqueness;
    // documented in the migration changelog.

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

/// Names of every custom collation that may appear in a legacy Anki
/// schema. Only the SOURCE side needs these registered (the dest is
/// Doltlite-prolly, which doesn't permit user collations — we strip
/// the COLLATE annotations instead via `strip_collate_unicase`).
const STUB_COLLATIONS: &[&str] = &["unicase"];

fn register_stub_collations_rusqlite(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<()> {
    for &name in STUB_COLLATIONS {
        conn.create_collation(name, |a: &str, b: &str| a.cmp(b))?;
    }
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
    let select_sql = format!(r#"SELECT * FROM "{}""#, table.replace('"', "\"\""));
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
