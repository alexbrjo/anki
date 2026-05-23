//! End-to-end migration test using the library entry point. Subprocess
//! invocation is exercised by `rslib/src/storage/migrate_from_sqlite.rs`.

use anki_migrate_sqlite::{migrate, DOLTLITE_APPLICATION_ID};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn migrates_schema_rows_and_indexes() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("legacy.anki2");
    let dst = tmp.path().join("migrated.anki2");

    {
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE col (id INTEGER PRIMARY KEY, crt INTEGER, mod INTEGER, conf TEXT);
            CREATE TABLE notes (id INTEGER PRIMARY KEY, guid TEXT, flds TEXT);
            CREATE TABLE cards (id INTEGER PRIMARY KEY, nid INTEGER, did INTEGER, ord INTEGER);
            CREATE TABLE revlog (id INTEGER PRIMARY KEY, cid INTEGER, ease INTEGER);
            CREATE INDEX ix_cards_nid ON cards (nid);
            CREATE INDEX ix_revlog_cid ON revlog (cid);
            PRAGMA user_version = 18;
            INSERT INTO col VALUES (1, 1700000000, 1700000001, '{"key":"value"}');
            INSERT INTO notes VALUES (100, 'guid-abc', 'front' || char(0x1f) || 'back');
            INSERT INTO notes VALUES (101, 'guid-def', 'hello' || char(0x1f) || 'world');
            INSERT INTO cards VALUES (200, 100, 1, 0);
            INSERT INTO cards VALUES (201, 100, 1, 1);
            INSERT INTO cards VALUES (202, 101, 1, 0);
            INSERT INTO revlog VALUES (300, 200, 3);
            INSERT INTO revlog VALUES (301, 201, 2);
            "#,
        )
        .unwrap();
    }

    let stats = migrate(&src, &dst).expect("migration succeeds");
    assert_eq!(stats.tables, 4, "all 4 user tables visited");
    assert_eq!(stats.rows, 8, "8 rows copied (1+2+3+2)");

    let dest = Connection::open(&dst).unwrap();

    for (table, expected) in [("col", 1), ("notes", 2), ("cards", 3), ("revlog", 2)] {
        let actual: i64 = dest
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(actual, expected, "row count for {table}");
    }

    let flds: String = dest
        .query_row("SELECT flds FROM notes WHERE id = 100", [], |r| r.get(0))
        .unwrap();
    assert_eq!(flds, "front\x1fback");

    let idx_count: i64 = dest
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name LIKE 'ix_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(idx_count, 2, "both user indexes recreated");

    let uv: i32 = dest
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(uv, 18);

    let app_id: i32 = dest
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    assert_eq!(app_id, DOLTLITE_APPLICATION_ID);
}

#[test]
fn handles_blobs() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("blob.anki2");
    let dst = tmp.path().join("blob-out.anki2");
    {
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch("CREATE TABLE media (id INTEGER PRIMARY KEY, data BLOB);")
            .unwrap();
        conn.execute(
            "INSERT INTO media VALUES (1, ?1)",
            [&[0u8, 1, 2, 255, 128, 64] as &[u8]],
        )
        .unwrap();
    }

    migrate(&src, &dst).unwrap();

    let dest = Connection::open(&dst).unwrap();
    let blob: Vec<u8> = dest
        .query_row("SELECT data FROM media WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(blob, vec![0u8, 1, 2, 255, 128, 64]);
}

#[test]
fn handles_null_values() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("null.anki2");
    let dst = tmp.path().join("null-out.anki2");
    {
        let conn = Connection::open(&src).unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, a TEXT, b INTEGER, c REAL);",
        )
        .unwrap();
        conn.execute_batch("INSERT INTO t VALUES (1, NULL, NULL, NULL);")
            .unwrap();
        conn.execute_batch("INSERT INTO t VALUES (2, 'x', 42, 3.14);")
            .unwrap();
    }
    migrate(&src, &dst).unwrap();
    let dest = Connection::open(&dst).unwrap();
    let nulls: i64 = dest
        .query_row("SELECT count(*) FROM t WHERE a IS NULL AND b IS NULL AND c IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nulls, 1);
}
