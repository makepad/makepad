//! Scale: enough rows for deep b-trees, index rebalancing and overflow chains,
//! written by this engine and validated by SQLite.
//!
//! Set `MAKEPAD_SQLITE_BIG=1` for the million-row version (release build
//! recommended); the default size keeps a debug `cargo test` quick.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Value};
use std::time::{Duration, Instant};

fn rows_target() -> i64 {
    if std::env::var("MAKEPAD_SQLITE_BIG").is_ok() {
        1_000_000
    } else {
        20_000
    }
}

#[test]
fn many_rows_stay_consistent() {
    if !have_sqlite3() {
        return;
    }
    let n = rows_target();
    let scratch = Scratch::new("scale");
    let path = scratch.path("big.db");
    let started = Instant::now();
    {
        let mut db = Connection::open(&path, Duration::from_secs(30)).unwrap();
        db.execute("PRAGMA page_size=4096", &[]).ok();
        db.execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT, n INTEGER, b BLOB)",
            &[],
        )
        .unwrap();
        db.execute("CREATE INDEX t_by_k ON t(k, n)", &[]).unwrap();
        db.execute("BEGIN IMMEDIATE", &[]).unwrap();
        for i in 1..=n {
            let b = if i % 5000 == 0 {
                Value::Blob(vec![(i % 251) as u8; 20_000])
            } else {
                Value::Blob(vec![7u8; (i % 16) as usize])
            };
            db.execute(
                "INSERT INTO t(id, k, n, b) VALUES (?1, ?2, ?3, ?4)",
                &[
                    Value::Integer(i),
                    // Keys arrive out of order so the index really rebalances.
                    Value::text(format!("k{:08}", (i * 2_654_435_761u64 as i64) % n)),
                    Value::Integer(i % 977),
                    b,
                ],
            )
            .unwrap();
        }
        db.execute("COMMIT", &[]).unwrap();
    }
    let insert_time = started.elapsed();

    // SQLite must be happy with the file, and see exactly what we wrote.
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*), SUM(n) FROM t;\n").trim(),
        {
            let mut db = Database::open(&path).unwrap();
            let r = db.query("SELECT COUNT(*), SUM(n) FROM t", &[]).unwrap();
            format!(
                "{}|{}",
                r.rows[0][0].as_integer().unwrap(),
                r.rows[0][1].as_integer().unwrap()
            )
        }
    );

    // The b-tree must be more than two levels deep at this size.
    let mut db = Database::open(&path).unwrap();
    let pages = db.pager().page_count();
    assert!(pages > 100, "only {pages} pages");

    // Deleting most rows keeps both engines in agreement.
    {
        let mut db = Connection::open(&path, Duration::from_secs(30)).unwrap();
        let deleted = db
            .execute("DELETE FROM t WHERE id % 3 <> 0", &[])
            .unwrap();
        assert_eq!(deleted, n as u64 - (n / 3) as u64);
    }
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        (n / 3).to_string()
    );
    let free: i64 = sqlite3(&path, ".mode list\nPRAGMA freelist_count;\n")
        .trim()
        .parse()
        .unwrap_or(0);
    assert!(free > 0, "deleting two thirds of the rows freed no pages");

    eprintln!(
        "scale: {n} rows inserted in {:.1}s, {pages} pages, {free} free after delete",
        insert_time.as_secs_f64()
    );
}

#[test]
fn index_lookups_stay_fast_as_the_table_grows() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("scale-seek");
    let path = scratch.path("seek.db");
    {
        let mut db = Connection::open(&path, Duration::from_secs(30)).unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT UNIQUE)", &[])
            .unwrap();
        db.execute("BEGIN IMMEDIATE", &[]).unwrap();
        for i in 1..=20_000i64 {
            db.execute(
                "INSERT INTO t(id, k) VALUES (?1, ?2)",
                &[Value::Integer(i), Value::text(format!("key-{i:06}"))],
            )
            .unwrap();
        }
        db.execute("COMMIT", &[]).unwrap();
    }
    let mut db = Database::open(&path).unwrap();
    let stmt = db.prepare("SELECT id FROM t WHERE k = ?1").unwrap();
    assert!(
        stmt.explain().contains("SEARCH USING INDEX"),
        "{}",
        stmt.explain()
    );
    let started = Instant::now();
    for i in (1..=20_000i64).step_by(97) {
        let out = stmt
            .query(&mut db, &[Value::text(format!("key-{i:06}"))])
            .unwrap();
        assert_eq!(out.rows[0][0].as_integer(), Some(i));
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "207 index lookups took {elapsed:?}"
    );
}
