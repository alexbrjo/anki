// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

pub mod entry;
pub mod meta;

use std::path::Path;

use rusqlite::Connection;

use crate::prelude::*;

pub struct ServerMediaDatabase {
    pub db: Connection,
}

impl ServerMediaDatabase {
    pub fn new(path: &Path) -> Result<Self> {
        Ok(Self {
            db: open_or_create_db(path)?,
        })
    }
}

fn open_or_create_db(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(0))?;
    if !is_doltlite_db(&db) {
        db.pragma_update(None, "locking_mode", "exclusive")?;
        db.pragma_update(None, "journal_mode", "wal")?;
    }
    let ver: u32 = db.query_row("select user_version from pragma_user_version", [], |r| {
        r.get(0)
    })?;
    let doltlite = is_doltlite_db(&db);
    if ver < 3 {
        execute_schema_sql(&db, include_str!("schema_v3.sql"), doltlite)?;
    }
    if ver < 4 {
        execute_schema_sql(&db, include_str!("schema_v4.sql"), doltlite)?;
    }
    Ok(db)
}

fn is_doltlite_db(db: &Connection) -> bool {
    matches!(
        db.query_row("select doltlite_engine()", [], |row| row.get::<_, String>(0)),
        Ok(engine) if engine == "prolly"
    )
}

fn execute_schema_sql(db: &Connection, sql: &str, doltlite: bool) -> rusqlite::Result<()> {
    if doltlite {
        db.execute_batch(
            &sql.replace("BEGIN exclusive", "BEGIN")
                .replace("vacuum;", ""),
        )
    } else {
        db.execute_batch(sql)
    }
}
