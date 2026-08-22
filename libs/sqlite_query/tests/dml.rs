//! P2 acceptance for the SQL write layer: DML, DDL, constraints and
//! transactions, each verified by re-reading the file with the `sqlite3` CLI.

mod common;

use common::*;
use makepad_sqlite::{Connection, Value};
use std::path::Path;
use std::time::Duration;

fn open(path: &Path) -> Connection {
    Connection::open(path, Duration::from_secs(5)).expect("open")
}

fn cli(path: &Path, sql: &str) -> String {
    sqlite3(path, &format!(".mode list\n.headers off\n{sql}\n"))
        .trim()
        .to_string()
}

fn integrity(path: &Path) {
    assert_eq!(cli(path, "PRAGMA integrity_check;"), "ok");
}

#[test]
fn create_insert_select_roundtrip() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-basic");
    let path = scratch.path("basic.db");
    {
        let mut db = open(&path);
        db.execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL, n INTEGER, r REAL, b BLOB)",
            &[],
        )
        .unwrap();
        assert_eq!(
            db.execute(
                "INSERT INTO t(name, n, r, b) VALUES ('alpha', 1, 1.5, x'01')",
                &[]
            )
            .unwrap(),
            1
        );
        db.execute(
            "INSERT INTO t(id, name, n) VALUES (?1, ?2, ?3)",
            &[Value::Integer(10), Value::text("beta"), Value::Integer(2)],
        )
        .unwrap();
        db.execute("INSERT INTO t(name) VALUES ('gamma'), ('delta')", &[])
            .unwrap();
        assert_eq!(db.changes(), 2);
    }
    integrity(&path);
    assert_eq!(
        cli(&path, "SELECT id, name, n, r, quote(b) FROM t ORDER BY id;"),
        "1|alpha|1|1.5|X'01'\n10|beta|2||NULL\n11|gamma|||NULL\n12|delta|||NULL"
    );
    // and our own engine reads back the same thing
    let mut db = open(&path);
    let ours = db
        .query("SELECT id, name, n FROM t ORDER BY id", &[])
        .unwrap();
    assert_eq!(ours.rows.len(), 4);
    assert_eq!(ours.rows[3][1].as_text(), Some("delta"));
}

#[test]
fn update_and_delete_maintain_indexes() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-update");
    let path = scratch.path("u.db");
    {
        let mut db = open(&path);
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT, v INTEGER)", &[])
            .unwrap();
        db.execute("CREATE INDEX t_by_k ON t(k)", &[]).unwrap();
        db.execute("CREATE UNIQUE INDEX t_by_v ON t(v)", &[]).unwrap();
        for i in 1..=200i64 {
            db.execute(
                "INSERT INTO t(k, v) VALUES (?1, ?2)",
                &[Value::text(format!("k{:04}", i % 40)), Value::Integer(i)],
            )
            .unwrap();
        }
        assert_eq!(
            db.execute("UPDATE t SET k = 'zzz' WHERE v > 190", &[]).unwrap(),
            10
        );
        assert_eq!(db.execute("DELETE FROM t WHERE v <= 20", &[]).unwrap(), 20);
    }
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t;"), "180");
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t WHERE k = 'zzz';"), "10");
    // The index must agree with the table (the CLI plans through it).
    assert_eq!(
        cli(&path, "SELECT COUNT(*) FROM t INDEXED BY t_by_k WHERE k = 'zzz';"),
        "10"
    );
    assert_eq!(
        cli(&path, "SELECT v FROM t INDEXED BY t_by_v WHERE v = 200;"),
        "200"
    );
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t WHERE v <= 20;"), "0");
}

#[test]
fn constraints_are_enforced() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-constraints");
    let path = scratch.path("c.db");
    let mut db = open(&path);
    db.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT CHECK(kind IN ('a','b')))",
        &[],
    )
    .unwrap();
    db.execute("INSERT INTO t(name, kind) VALUES ('one', 'a')", &[])
        .unwrap();
    // NOT NULL
    assert!(db
        .execute("INSERT INTO t(name) VALUES (NULL)", &[])
        .is_err());
    // UNIQUE
    assert!(db
        .execute("INSERT INTO t(name, kind) VALUES ('one', 'b')", &[])
        .is_err());
    // CHECK
    assert!(db
        .execute("INSERT INTO t(name, kind) VALUES ('two', 'zzz')", &[])
        .is_err());
    // rowid PK
    db.execute("INSERT INTO t(id, name) VALUES (5, 'five')", &[])
        .unwrap();
    assert!(db
        .execute("INSERT INTO t(id, name) VALUES (5, 'again')", &[])
        .is_err());
    // OR IGNORE and OR REPLACE
    assert_eq!(
        db.execute("INSERT OR IGNORE INTO t(id, name) VALUES (5, 'again')", &[])
            .unwrap(),
        0
    );
    db.execute(
        "INSERT OR REPLACE INTO t(id, name, kind) VALUES (5, 'replaced', 'b')",
        &[],
    )
    .unwrap();
    // ON CONFLICT DO UPDATE (the store's alias upsert shape)
    db.execute(
        "INSERT INTO t(name, kind) VALUES ('one', 'b') ON CONFLICT(name) DO UPDATE SET kind = 'b'",
        &[],
    )
    .unwrap();
    drop(db);
    integrity(&path);
    assert_eq!(
        cli(&path, "SELECT id, name, kind FROM t ORDER BY id;"),
        "1|one|b\n5|replaced|b"
    );
}

#[test]
fn transactions_commit_and_roll_back() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-tx");
    let path = scratch.path("tx.db");
    let mut db = open(&path);
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    assert!(!db.autocommit());
    for i in 1..=50 {
        db.execute("INSERT INTO t(v) VALUES (?1)", &[Value::text(format!("v{i}"))])
            .unwrap();
    }
    db.execute("COMMIT", &[]).unwrap();
    assert!(db.autocommit());
    assert_eq!(db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(), Some(50));

    db.execute("BEGIN", &[]).unwrap();
    db.execute("DELETE FROM t", &[]).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(0)
    );
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(50),
        "rollback did not restore the rows"
    );
    drop(db);
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t;"), "50");
}

#[test]
fn ddl_changes_are_visible_to_sqlite() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-ddl");
    let path = scratch.path("ddl.db");
    {
        let mut db = open(&path);
        db.execute("CREATE TABLE a(id INTEGER PRIMARY KEY, x TEXT)", &[])
            .unwrap();
        db.execute("CREATE TABLE b(k TEXT PRIMARY KEY, v BLOB NOT NULL)", &[])
            .unwrap();
        db.execute("CREATE INDEX a_by_x ON a(x)", &[]).unwrap();
        db.execute("INSERT INTO a(x) VALUES ('one'), ('two')", &[])
            .unwrap();
        db.execute(
            "INSERT INTO b(k, v) VALUES ('key', x'0102')",
            &[],
        )
        .unwrap();
        // ALTER TABLE ADD COLUMN, exactly what the store does to search_annotations
        db.execute("ALTER TABLE a ADD COLUMN canon TEXT NOT NULL DEFAULT ''", &[])
            .unwrap();
        db.execute("INSERT INTO a(x, canon) VALUES ('three', 'c')", &[])
            .unwrap();
    }
    integrity(&path);
    assert_eq!(
        cli(&path, "SELECT id, x, canon FROM a ORDER BY id;"),
        "1|one|\n2|two|\n3|three|c",
        "old rows must read the new column's default"
    );
    assert_eq!(cli(&path, "SELECT quote(v) FROM b;"), "X'0102'");
    assert_eq!(
        cli(&path, "SELECT name FROM sqlite_master WHERE type='index' ORDER BY name;"),
        "a_by_x\nsqlite_autoindex_b_1"
    );
    // DROP
    {
        let mut db = open(&path);
        db.execute("DROP INDEX a_by_x", &[]).unwrap();
        db.execute("DROP TABLE b", &[]).unwrap();
        db.execute("ALTER TABLE a RENAME TO renamed", &[]).unwrap();
    }
    integrity(&path);
    assert_eq!(
        cli(&path, "SELECT name FROM sqlite_master ORDER BY name;"),
        "renamed"
    );
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM renamed;"), "3");
}

#[test]
fn pragmas_the_store_uses() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-pragma");
    let path = scratch.path("p.db");
    let mut db = open(&path);
    db.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT NOT NULL DEFAULT 'x')", &[])
        .unwrap();
    db.execute("PRAGMA user_version=8", &[]).unwrap();
    assert_eq!(db.user_version(), 8);
    let uv = db.query("PRAGMA user_version", &[]).unwrap();
    assert_eq!(uv.rows[0][0].as_integer(), Some(8));
    // accepted and ignored
    db.execute("PRAGMA synchronous=FULL", &[]).unwrap();
    db.execute("PRAGMA foreign_keys=ON", &[]).unwrap();
    let info = db.query("PRAGMA table_info(t)", &[]).unwrap();
    assert_eq!(info.rows.len(), 2);
    assert_eq!(info.rows[1][1].as_text(), Some("b"));
    assert_eq!(info.rows[1][3].as_integer(), Some(1)); // not null
    let check = db.query("PRAGMA integrity_check", &[]).unwrap();
    assert_eq!(check.rows[0][0].as_text(), Some("ok"));
    drop(db);
    assert_eq!(cli(&path, "PRAGMA user_version;"), "8");
}

#[test]
fn a_thousand_rows_match_sqlite_row_for_row() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-bulk");
    let path = scratch.path("bulk.db");
    {
        let mut db = open(&path);
        db.execute(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT, n INTEGER, big BLOB)",
            &[],
        )
        .unwrap();
        db.execute("CREATE INDEX t_by_k ON t(k, n)", &[]).unwrap();
        db.execute("BEGIN IMMEDIATE", &[]).unwrap();
        for i in 1..=1000i64 {
            let big = if i % 100 == 0 {
                Value::Blob(vec![(i % 251) as u8; 12000])
            } else {
                Value::Blob(vec![1u8; (i % 20) as usize])
            };
            db.execute(
                "INSERT INTO t(k, n, big) VALUES (?1, ?2, ?3)",
                &[Value::text(format!("k{:03}", i % 97)), Value::Integer(i), big],
            )
            .unwrap();
        }
        db.execute("COMMIT", &[]).unwrap();
    }
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*), SUM(n), SUM(length(big)) FROM t;"), {
        let mut db = open(&path);
        let r = db
            .query("SELECT COUNT(*), SUM(n), SUM(LENGTH(big)) FROM t", &[])
            .unwrap();
        format!(
            "{}|{}|{}",
            r.rows[0][0].as_integer().unwrap(),
            r.rows[0][1].as_integer().unwrap(),
            r.rows[0][2].as_integer().unwrap()
        )
    });
    assert_eq!(
        cli(&path, "SELECT k, n FROM t WHERE k = 'k005' ORDER BY n LIMIT 3;"),
        "k005|5\nk005|102\nk005|199"
    );
}

#[test]
fn a_wal_database_stays_in_wal_mode_and_sqlite_can_read_it() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("dml-wal");
    // A WAL-mode database with rows that live only in the log.
    let path = build_wal_db(
        &scratch.dir,
        "wal.db",
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);
         CREATE INDEX t_by_v ON t(v);
         INSERT INTO t(v) VALUES ('checkpointed');",
        "INSERT INTO t(v) VALUES ('in-wal-1');
         INSERT INTO t(v) VALUES ('in-wal-2');",
    );
    assert!(path.with_file_name("wal.db-wal").exists());

    {
        let mut db = open(&path);
        assert_eq!(
            db.query("PRAGMA journal_mode", &[]).unwrap().rows[0][0].as_text(),
            Some("wal"),
            "a WAL database must stay in WAL mode"
        );
        assert_eq!(
            db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
            Some(3),
            "rows that live in the WAL must be visible"
        );
        // Our own writes go into the log as frames.
        db.execute("INSERT INTO t(v) VALUES ('ours-in-wal')", &[])
            .unwrap();
        assert_eq!(
            db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
            Some(4)
        );
    }
    // With the connection gone, the log is an ordinary WAL again: SQLite
    // recovers it and sees every row, ours included.
    integrity(&path);
    assert_eq!(
        cli(&path, "SELECT v FROM t ORDER BY id;"),
        "checkpointed\nin-wal-1\nin-wal-2\nours-in-wal"
    );
    assert_eq!(
        cli(&path, "SELECT id FROM t INDEXED BY t_by_v WHERE v = 'ours-in-wal';"),
        "4"
    );
    assert_eq!(cli(&path, "PRAGMA journal_mode;"), "wal");

    // Explicitly converting to a rollback journal also works, both ways.
    {
        let mut db = open(&path);
        assert_eq!(
            db.query("PRAGMA journal_mode=delete", &[]).unwrap().rows[0][0].as_text(),
            Some("delete")
        );
        db.execute("INSERT INTO t(v) VALUES ('after-convert')", &[])
            .unwrap();
    }
    assert!(!path.with_file_name("wal.db-wal").exists());
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t;"), "5");
    {
        let mut db = open(&path);
        assert_eq!(
            db.query("PRAGMA journal_mode=wal", &[]).unwrap().rows[0][0].as_text(),
            Some("wal")
        );
        db.execute("INSERT INTO t(v) VALUES ('back-in-wal')", &[])
            .unwrap();
    }
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t;"), "6");
    assert_eq!(cli(&path, "PRAGMA journal_mode;"), "wal");
}

#[test]
fn the_stores_v8_schema_round_trips() {
    if !have_sqlite3() {
        return;
    }
    // A representative slice of the asset store's catalog schema, created by
    // this engine and then exercised by SQLite.
    let scratch = Scratch::new("dml-v8");
    let path = scratch.path("v8.db");
    let schema = "
CREATE TABLE blobs(blob_id BLOB PRIMARY KEY, size INTEGER NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE assets(asset_id BLOB PRIMARY KEY, namespace TEXT NOT NULL, created_ms INTEGER NOT NULL);
CREATE TABLE asset_aliases(alias TEXT PRIMARY KEY, asset_id BLOB NOT NULL, head_revision BLOB NOT NULL, updated_ms INTEGER NOT NULL);
CREATE INDEX asset_aliases_by_asset ON asset_aliases(asset_id, alias);
CREATE TABLE candidates(kind TEXT NOT NULL CHECK(kind IN ('asset','game')), owner_id BLOB NOT NULL, revision BLOB NOT NULL, state TEXT NOT NULL CHECK(state IN ('staged','published','quarantined')), staged_ms INTEGER NOT NULL, published_ms INTEGER, quarantined_ms INTEGER, PRIMARY KEY(kind, owner_id, revision));
CREATE TABLE search_annotations(asset_id BLOB PRIMARY KEY, namespace TEXT NOT NULL, kind TEXT, title TEXT NOT NULL, live INTEGER NOT NULL, updated_ms INTEGER NOT NULL);
CREATE INDEX search_annotations_by_ns ON search_annotations(namespace);
CREATE TABLE search_postings(term TEXT NOT NULL, asset_id BLOB NOT NULL, weight_public INTEGER NOT NULL, weight_owner INTEGER NOT NULL, PRIMARY KEY(term, asset_id));
CREATE INDEX search_postings_by_asset ON search_postings(asset_id);
";
    {
        let mut db = open(&path);
        db.execute_batch(schema).unwrap();
        db.execute("PRAGMA user_version=8", &[]).unwrap();
        db.execute("BEGIN IMMEDIATE", &[]).unwrap();
        for i in 1..=200i64 {
            let id = Value::Blob(vec![(i % 251) as u8; 16]);
            db.execute(
                "INSERT INTO assets(asset_id, namespace, created_ms) VALUES(?1, ?2, ?3)",
                &[id.clone(), Value::text("ns"), Value::Integer(i)],
            )
            .unwrap();
            db.execute(
                "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES(?1, ?2, ?3, ?4) ON CONFLICT(alias) DO UPDATE SET asset_id=?2, head_revision=?3, updated_ms=?4",
                &[
                    Value::text(format!("alias-{i:04}")),
                    id.clone(),
                    Value::Blob(vec![1, 2, 3]),
                    Value::Integer(i),
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO search_annotations(asset_id, namespace, kind, title, live, updated_ms) VALUES(?1,?2,?3,?4,?5,?6)",
                &[
                    Value::Blob(vec![(i % 251) as u8, (i / 251) as u8]),
                    Value::text("ns"),
                    Value::text("mesh"),
                    Value::text(format!("title {i}")),
                    Value::Integer(1),
                    Value::Integer(i),
                ],
            )
            .unwrap();
            db.execute(
                "INSERT INTO search_postings(term, asset_id, weight_public, weight_owner) VALUES(?1,?2,?3,?4)",
                &[
                    Value::text(format!("term{}", i % 20)),
                    Value::Blob(vec![(i % 251) as u8, (i / 251) as u8]),
                    Value::Integer(i),
                    Value::Integer(i),
                ],
            )
            .unwrap();
        }
        db.execute("COMMIT", &[]).unwrap();
        // The CHECK constraints must bite.
        assert!(db
            .execute(
                "INSERT INTO candidates(kind, owner_id, revision, state, staged_ms) VALUES('nope', x'01', x'02', 'staged', 1)",
                &[]
            )
            .is_err());
        db.execute(
            "INSERT INTO candidates(kind, owner_id, revision, state, staged_ms) VALUES('asset', x'01', x'02', 'staged', 1)",
            &[],
        )
        .unwrap();
    }
    integrity(&path);
    assert_eq!(cli(&path, "PRAGMA user_version;"), "8");
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM asset_aliases;"), "200");
    // Each alias has its own asset id in this fixture, so the reverse lookup
    // through asset_aliases_by_asset returns exactly one row.
    assert_eq!(
        cli(&path, "SELECT alias FROM asset_aliases INDEXED BY asset_aliases_by_asset WHERE asset_id = (SELECT asset_id FROM asset_aliases WHERE alias='alias-0007') ORDER BY alias;"),
        "alias-0007"
    );
    assert_eq!(
        cli(&path, "SELECT COUNT(*) FROM search_postings WHERE term = 'term3';"),
        "10"
    );
    // ... and the other direction: SQLite writes, we read.
    sqlite3(
        &path,
        "INSERT INTO assets(asset_id, namespace, created_ms) VALUES(x'ff00', 'other', 99);",
    );
    let mut db = open(&path);
    let row = db
        .query(
            "SELECT namespace, created_ms FROM assets WHERE asset_id = ?1",
            &[Value::Blob(vec![0xff, 0x00])],
        )
        .unwrap();
    assert_eq!(row.rows[0][0].as_text(), Some("other"));
    assert_eq!(row.rows[0][1].as_integer(), Some(99));
}

#[test]
fn the_stores_table_rebuild_migration_works() {
    if !have_sqlite3() {
        return;
    }
    // The shape of the store's v5/v6 migration: build a replacement table,
    // copy the rows across, drop the original and rename over it — all inside
    // one transaction, with automatic indexes following the rename.
    let scratch = Scratch::new("dml-rebuild");
    let path = scratch.path("rb.db");
    let mut db = open(&path);
    db.execute_batch(
        "CREATE TABLE t(asset_id BLOB PRIMARY KEY, ns TEXT NOT NULL, kind TEXT CHECK(kind IS NULL OR kind IN ('a','b')), canon TEXT NOT NULL DEFAULT '');
         CREATE INDEX IF NOT EXISTS t_by_ns ON t(ns);
         CREATE INDEX IF NOT EXISTS t_by_kind ON t(kind);",
    )
    .unwrap();
    for i in 1..=50i64 {
        db.execute(
            "INSERT INTO t(asset_id, ns, kind) VALUES(?1,?2,?3)",
            &[
                Value::Blob(vec![i as u8; 8]),
                Value::text("ns"),
                Value::text(if i % 2 == 0 { "a" } else { "b" }),
            ],
        )
        .unwrap();
    }
    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    db.execute_batch(
        "CREATE TABLE t_rebuild(asset_id BLOB PRIMARY KEY, ns TEXT NOT NULL, kind TEXT CHECK(kind IS NULL OR kind IN ('a','b')), canon TEXT NOT NULL DEFAULT '');
         INSERT INTO t_rebuild(asset_id, ns, kind, canon) SELECT asset_id, ns, kind, canon FROM t;
         DROP TABLE t;
         ALTER TABLE t_rebuild RENAME TO t;
         CREATE INDEX IF NOT EXISTS t_by_ns ON t(ns);
         CREATE INDEX IF NOT EXISTS t_by_kind ON t(kind);",
    )
    .unwrap();
    db.execute("COMMIT", &[]).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(50)
    );
    drop(db);
    integrity(&path);
    assert_eq!(cli(&path, "SELECT COUNT(*) FROM t;"), "50");
    assert_eq!(
        cli(&path, "SELECT name FROM sqlite_master WHERE type='index' ORDER BY name;"),
        "sqlite_autoindex_t_1\nt_by_kind\nt_by_ns",
        "the automatic index must be renamed with its table"
    );
}
