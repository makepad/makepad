//! Differential DML fuzzing: the same random sequence of INSERT / UPDATE /
//! DELETE / upsert statements is applied by this engine and by the system
//! `sqlite3` CLI to two copies of the same database. After every batch the two
//! files must contain exactly the same rows, and ours must still pass
//! `PRAGMA integrity_check` — run by SQLite, on the file we wrote.

mod common;

use common::*;
use makepad_sqlite::Connection;
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT UNIQUE, n INTEGER, b BLOB);\
     CREATE INDEX t_by_n ON t(n);";

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn gen_statement(rng: &mut Rng) -> String {
    let id = rng.below(40) + 1;
    let key = format!("k{}", rng.below(25));
    let n = rng.below(100) as i64 - 20;
    let blob = format!("x'{}'", "ab".repeat((rng.below(6) + 1) as usize));
    match rng.below(12) {
        0 => format!("INSERT OR IGNORE INTO t(id, k, n, b) VALUES ({id}, '{key}', {n}, {blob})"),
        1 => format!("INSERT OR REPLACE INTO t(id, k, n, b) VALUES ({id}, '{key}', {n}, {blob})"),
        2 => format!(
            "INSERT INTO t(k, n) VALUES ('{key}', {n}) ON CONFLICT(k) DO UPDATE SET n = n + 1"
        ),
        3 => format!(
            "INSERT INTO t(k, n) VALUES ('{key}', {n}) ON CONFLICT(k) DO NOTHING"
        ),
        4 | 5 => format!("INSERT OR IGNORE INTO t(k, n, b) VALUES ('{key}', {n}, {blob})"),
        6 => format!("UPDATE t SET n = n + {n} WHERE id = {id}"),
        7 => format!("UPDATE t SET b = {blob} WHERE n > {n}"),
        8 => format!("UPDATE OR IGNORE t SET k = '{key}' WHERE id = {id}"),
        9 => format!("DELETE FROM t WHERE id = {id}"),
        10 => format!("DELETE FROM t WHERE n < {n}"),
        _ => format!("DELETE FROM t WHERE k = '{key}'"),
    }
}

/// Rows of `t` as the CLI renders them, so both files are compared identically.
fn dump(path: &Path) -> String {
    sqlite3(
        path,
        ".mode quote\n.headers off\n.separator |\nSELECT id, k, n, quote(b) FROM t ORDER BY id;\n",
    )
}

fn apply_to_cli(path: &Path, sql: &str) -> bool {
    let out = std::process::Command::new("sqlite3")
        .arg(path)
        .arg(sql)
        .output()
        .expect("sqlite3");
    out.status.success()
}

#[test]
fn random_dml_matches_sqlite() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("fuzz-dml");
    let ours = scratch.path("ours.db");
    let theirs = scratch.path("theirs.db");
    {
        let mut db = Connection::open(&ours, Duration::from_secs(5)).expect("open");
        db.execute_batch(SCHEMA).expect("schema");
    }
    sqlite3(&theirs, SCHEMA);

    let mut rng = Rng(0x1234_5678_9abc_def0);
    let mut db = Connection::open(&ours, Duration::from_secs(5)).expect("open");
    let mut applied = 0;
    let mut skipped = 0;
    for round in 0..300 {
        let sql = gen_statement(&mut rng);
        let our_result = db.execute(&sql, &[]);
        let their_ok = apply_to_cli(&theirs, &sql);
        match (&our_result, their_ok) {
            (Ok(_), true) => applied += 1,
            (Err(_), false) => skipped += 1,
            (Ok(_), false) => panic!("round {round}: sqlite3 rejected {sql:?} but we accepted it"),
            (Err(e), true) => {
                panic!("round {round}: we rejected {sql:?} ({e}) but sqlite3 accepted it")
            }
        }
        if round % 10 == 0 {
            // Compare through the CLI so both sides are rendered the same way.
            let a = dump(&ours);
            let b = dump(&theirs);
            assert_eq!(
                a, b,
                "round {round} diverged after {sql:?}\nours:\n{a}\ntheirs:\n{b}"
            );
            assert_eq!(
                sqlite3(&ours, "PRAGMA integrity_check;\n").trim(),
                "ok",
                "round {round}: our file is damaged after {sql:?}"
            );
        }
    }
    assert_eq!(dump(&ours), dump(&theirs), "final state diverged");
    assert!(applied > 100, "only {applied} statements applied");
    let _ = skipped;
}

#[test]
fn upsert_and_replace_match_sqlite_exactly() {
    if !have_sqlite3() {
        return;
    }
    // The store's real upsert shapes, applied to both engines in order.
    let scratch = Scratch::new("fuzz-upsert");
    let ours = scratch.path("ours.db");
    let theirs = scratch.path("theirs.db");
    let schema = "CREATE TABLE asset_aliases(alias TEXT PRIMARY KEY, asset_id BLOB NOT NULL, head_revision BLOB NOT NULL, updated_ms INTEGER NOT NULL);\
         CREATE INDEX asset_aliases_by_asset ON asset_aliases(asset_id, alias);";
    {
        let mut db = Connection::open(&ours, Duration::from_secs(5)).unwrap();
        db.execute_batch(schema).unwrap();
    }
    sqlite3(&theirs, schema);

    let statements = [
        "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('a', x'01', x'aa', 1) ON CONFLICT(alias) DO UPDATE SET asset_id=x'01', head_revision=x'aa', updated_ms=1",
        "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('a', x'02', x'bb', 2) ON CONFLICT(alias) DO UPDATE SET asset_id=x'02', head_revision=x'bb', updated_ms=2",
        "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('b', x'03', x'cc', 3) ON CONFLICT(alias) DO UPDATE SET asset_id=x'03', head_revision=x'cc', updated_ms=3",
        "DELETE FROM asset_aliases WHERE asset_id = x'02' AND head_revision = x'bb'",
        "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('c', x'04', x'dd', 4)",
        "INSERT OR IGNORE INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('c', x'05', x'ee', 5)",
        "INSERT OR REPLACE INTO asset_aliases(alias, asset_id, head_revision, updated_ms) VALUES('c', x'06', x'ff', 6)",
        "UPDATE asset_aliases SET updated_ms = updated_ms + 10 WHERE alias >= 'b'",
        "DELETE FROM asset_aliases WHERE alias = 'zzz'",
    ];
    let mut db = Connection::open(&ours, Duration::from_secs(5)).unwrap();
    for (i, sql) in statements.iter().enumerate() {
        db.execute(sql, &[]).unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert!(apply_to_cli(&theirs, sql), "sqlite3 rejected {sql}");
        let q = ".mode quote\n.headers off\n.separator |\nSELECT alias, quote(asset_id), quote(head_revision), updated_ms FROM asset_aliases ORDER BY alias;\n";
        assert_eq!(
            sqlite3(&ours, q),
            sqlite3(&theirs, q),
            "statement {i} diverged: {sql}"
        );
    }
    assert_eq!(sqlite3(&ours, "PRAGMA integrity_check;\n").trim(), "ok");
}
