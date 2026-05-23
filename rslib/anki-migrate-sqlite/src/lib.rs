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
    dest.execute_batch("BEGIN")?;

    let table_ddls = read_ddls(&source, "table")?;
    for (_, ddl) in &table_ddls {
        dest.execute_batch(ddl)
            .with_context(|| format!("replaying table DDL: {ddl}"))?;
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
            dest.execute_batch(&ddl)
                .with_context(|| format!("replaying {kind} DDL"))?;
        }
    }

    let user_version: i32 = source.pragma_query_value(None, "user_version", |r| r.get(0))?;
    dest.pragma_update(None, "user_version", user_version)?;
    dest.pragma_update(None, "application_id", DOLTLITE_APPLICATION_ID)?;
    dest.execute_batch("COMMIT")?;
    Ok(stats)
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
