//! P0 acceptance: read files the system SQLite wrote — every page size, deep
//! b-trees, overflow chains, indexes and live WAL snapshots — and never panic
//! on a corrupt one.

mod common;

use common::*;
use makepad_sqlite::btree::{IndexCursor, TableCursor};
use makepad_sqlite::{Collation, Database, TextMode, Value};
use std::path::Path;

/// Scan a whole table with a cursor, materializing rows the way a query would.
fn scan_table(db: &mut Database, table: &str) -> Vec<(i64, Vec<Value>)> {
    let root = db.schema().table(table).expect("table in schema").root_page;
    let info = db.schema().table(table).unwrap().clone();
    let (pager, _) = db.parts();
    let mut cursor = TableCursor::new(root);
    cursor.rewind(pager).expect("rewind");
    let mut out = Vec::new();
    while let Some(row) = cursor.next(pager).expect("scan") {
        let vals = row.payload.values(pager, TextMode::Strict).expect("decode");
        out.push((row.rowid, info.materialize(row.rowid, vals)));
    }
    out
}

const FIXTURE_SQL: &str = r#"
CREATE TABLE items(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT,
    score REAL,
    payload BLOB,
    flag INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX items_by_kind ON items(kind, id);
CREATE TABLE keyed(
    k TEXT PRIMARY KEY,
    v BLOB NOT NULL,
    n INTEGER NOT NULL
);
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i < 2000)
INSERT INTO items(id, name, kind, score, payload, flag)
SELECT i,
       'name-' || i,
       CASE i % 4 WHEN 0 THEN NULL ELSE 'kind-' || (i % 7) END,
       CASE i % 3 WHEN 0 THEN NULL ELSE i * 1.5 END,
       CASE WHEN i % 100 = 0 THEN randomblob(9000) ELSE randomblob(i % 40) END,
       i % 2
FROM seq;
WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i < 500)
INSERT INTO keyed(k, v, n)
SELECT printf('key-%06d', i), randomblob(20), i FROM seq;
"#;

fn cli_rows(db: &Path, sql: &str) -> Vec<Vec<Value>> {
    let out = sqlite3(db, &format!(".mode quote\n.headers off\n.separator |\n{sql}\n"));
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('|').map(parse_quoted).collect())
        .collect()
}

#[test]
fn full_scan_matches_cli_across_page_sizes() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("scan");
    for page_size in [512u32, 1024, 4096, 65536] {
        let name = format!("p{page_size}.db");
        let path = build_db(
            &scratch.dir,
            &name,
            &format!("PRAGMA page_size={page_size};\n{FIXTURE_SQL}"),
        );
        let mut db = Database::open(&path).expect("open");
        assert_eq!(db.pager().page_size() as u32, page_size);

        for table in ["items", "keyed"] {
            let ours = scan_table(&mut db, table);
            let theirs = cli_rows(
                &path,
                &format!("SELECT * FROM {table} ORDER BY rowid;"),
            );
            assert_eq!(
                ours.len(),
                theirs.len(),
                "row count for {table} at page size {page_size}"
            );
            for (i, ((_rowid, a), b)) in ours.iter().zip(theirs.iter()).enumerate() {
                assert_eq!(a.len(), b.len(), "column count row {i} of {table}");
                for (c, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                    assert!(
                        x == y,
                        "row {i} col {c} of {table} at page size {page_size}: {} vs {}",
                        quote(x),
                        quote(y)
                    );
                }
            }
        }
    }
}

#[test]
fn btree_is_deep_and_has_overflow() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("deep");
    let path = build_db(
        &scratch.dir,
        "deep.db",
        &format!("PRAGMA page_size=512;\n{FIXTURE_SQL}"),
    );
    let mut db = Database::open(&path).expect("open");
    // A 512-byte page fixture of 2000 rows must be more than two levels deep,
    // and the 9000-byte blobs must live on overflow chains.
    let root = db.schema().table("items").unwrap().root_page;
    let (pager, _) = db.parts();
    let mut cursor = TableCursor::new(root);
    cursor.rewind(pager).unwrap();
    let mut overflowing = 0;
    let mut rows = 0;
    while let Some(row) = cursor.next(pager).unwrap() {
        if !row.payload.is_local() {
            overflowing += 1;
            let bytes = row.payload.read(pager).unwrap();
            assert_eq!(bytes.len(), row.payload.total_size());
        }
        rows += 1;
    }
    assert_eq!(rows, 2000);
    assert!(overflowing >= 20, "expected overflow rows, got {overflowing}");
    assert!(pager.page_count() > 500);
}

#[test]
fn rowid_seek_finds_every_row() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("seek");
    let path = build_db(
        &scratch.dir,
        "seek.db",
        &format!("PRAGMA page_size=1024;\n{FIXTURE_SQL}"),
    );
    let mut db = Database::open(&path).expect("open");
    let root = db.schema().table("items").unwrap().root_page;
    let (pager, _) = db.parts();
    let mut cursor = TableCursor::new(root);
    for id in [1i64, 2, 999, 1000, 1001, 2000] {
        let row = cursor.seek_exact(pager, id).unwrap();
        assert!(row.is_some(), "rowid {id} not found");
        assert_eq!(row.unwrap().rowid, id);
    }
    for id in [-5i64, 0, 2001, i64::MAX] {
        assert!(cursor.seek_exact(pager, id).unwrap().is_none());
    }
    // seek_ge lands on the first row at or after the target
    cursor.seek_ge(pager, 1500).unwrap();
    assert_eq!(cursor.next(pager).unwrap().unwrap().rowid, 1500);
}

#[test]
fn index_seek_matches_table_contents() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("index");
    let path = build_db(
        &scratch.dir,
        "index.db",
        &format!("PRAGMA page_size=1024;\n{FIXTURE_SQL}"),
    );
    let mut db = Database::open(&path).expect("open");

    // Automatic index on keyed(k): seek to a key and check the payload rowid
    // points at the row the CLI reports.
    let table = db.schema().table("keyed").unwrap().clone();
    let idx = table
        .indexes
        .iter()
        .find(|i| i.name.starts_with("sqlite_autoindex_keyed"))
        .expect("auto index for keyed")
        .clone();
    let (pager, _) = db.parts();
    let mut cursor = IndexCursor::new(idx.root_page);
    let target = vec![Value::text("key-000250")];
    cursor.seek_ge(pager, &target, &[Collation::Binary]).unwrap();
    let entry = cursor.next(pager).unwrap().expect("entry");
    let vals = entry.values(pager, TextMode::Strict).unwrap();
    assert_eq!(vals[0].as_text(), Some("key-000250"));
    let rowid = vals.last().unwrap().as_integer().expect("trailing rowid");

    let mut table_cursor = TableCursor::new(table.root_page);
    let row = table_cursor.seek_exact(pager, rowid).unwrap().expect("row");
    let row_vals = row.payload.values(pager, TextMode::Strict).unwrap();
    assert_eq!(row_vals[0].as_text(), Some("key-000250"));

    // Walking the index from the start yields keys in sorted order.
    let mut cursor = IndexCursor::new(idx.root_page);
    cursor.rewind(pager).unwrap();
    let mut prev: Option<String> = None;
    let mut count = 0;
    while let Some(entry) = cursor.next(pager).unwrap() {
        let vals = entry.values(pager, TextMode::Strict).unwrap();
        let k = vals[0].as_text().unwrap().to_string();
        if let Some(p) = &prev {
            assert!(p < &k, "index out of order: {p} then {k}");
        }
        prev = Some(k);
        count += 1;
    }
    assert_eq!(count, 500);

    // A two-column index with a prefix seek.
    let items = db.schema().table("items").unwrap().clone();
    let by_kind = items
        .indexes
        .iter()
        .find(|i| i.name == "items_by_kind")
        .expect("items_by_kind")
        .clone();
    let (pager, _) = db.parts();
    let mut cursor = IndexCursor::new(by_kind.root_page);
    cursor
        .seek_ge(pager, &[Value::text("kind-3")], &[Collation::Binary])
        .unwrap();
    let mut seen = 0;
    while let Some(entry) = cursor.next(pager).unwrap() {
        let vals = entry.values(pager, TextMode::Strict).unwrap();
        match vals[0].as_text() {
            Some("kind-3") => seen += 1,
            _ => break,
        }
    }
    let cli = sqlite3(
        &path,
        ".mode list\nSELECT count(*) FROM items WHERE kind = 'kind-3';\n",
    );
    assert_eq!(seen, cli.trim().parse::<i64>().unwrap());
}

#[test]
fn wal_snapshot_is_read() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("wal");
    let path = build_wal_db(
        &scratch.dir,
        "wal.db",
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT NOT NULL);
         INSERT INTO t(v) VALUES('checkpointed');",
        "BEGIN;
         INSERT INTO t(v) VALUES('in-wal-1');
         INSERT INTO t(v) VALUES('in-wal-2');
         COMMIT;
         INSERT INTO t(v) VALUES('in-wal-3');
         CREATE TABLE later(x TEXT);
         INSERT INTO later VALUES('after-schema-change');",
    );
    let wal = path.with_file_name("wal.db-wal");
    assert!(wal.exists(), "fixture left no WAL behind");
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);

    let mut db = Database::open(&path).expect("open");
    assert!(db.pager().wal_frames() > 0, "no WAL frames were accepted");
    let rows = scan_table(&mut db, "t");
    let texts: Vec<&str> = rows
        .iter()
        .map(|(_, v)| v[1].as_text().unwrap_or(""))
        .collect();
    assert_eq!(
        texts,
        vec![
            "checkpointed",
            "in-wal-1",
            "in-wal-2",
            "in-wal-3",
        ]
    );
    // A table created inside the WAL is visible through the schema too.
    let later = scan_table(&mut db, "later");
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].1[0].as_text(), Some("after-schema-change"));

    // ... and the CLI agrees, reading the same files.
    let cli = sqlite3(&path, ".mode list\nSELECT count(*) FROM t;\n");
    assert_eq!(cli.trim(), "4");
}

#[test]
fn wal_ignores_a_torn_tail() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("waltorn");
    let path = build_wal_db(
        &scratch.dir,
        "torn.db",
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);
         INSERT INTO t(v) VALUES('base');",
        "INSERT INTO t(v) VALUES('committed');",
    );
    let wal = path.with_file_name("torn.db-wal");
    let before = {
        let mut db = Database::open(&path).unwrap();
        scan_table(&mut db, "t").len()
    };
    assert_eq!(before, 2);

    // Append a frame-sized block of garbage: the checksum chain must reject it.
    let mut bytes = std::fs::read(&wal).unwrap();
    let page_size = 4096usize;
    bytes.extend(std::iter::repeat(0xA5).take(24 + page_size));
    std::fs::write(&wal, &bytes).unwrap();

    let mut db = Database::open(&path).unwrap();
    assert_eq!(scan_table(&mut db, "t").len(), before);

    // Truncating mid-frame must not lose the committed snapshot either.
    let mut bytes = std::fs::read(&wal).unwrap();
    bytes.truncate(bytes.len() - 17);
    std::fs::write(&wal, &bytes).unwrap();
    let mut db = Database::open(&path).unwrap();
    assert_eq!(scan_table(&mut db, "t").len(), before);
}

/// Deterministic xorshift so a failing corruption case can be replayed.
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
}

#[test]
fn corrupt_bytes_never_panic() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("corrupt");
    let path = build_db(
        &scratch.dir,
        "good.db",
        &format!("PRAGMA page_size=1024;\n{FIXTURE_SQL}"),
    );
    let good = std::fs::read(&path).unwrap();
    let mut rng = Rng(0x5eed_1234_9876_abcd);
    let victim = scratch.path("victim.db");
    let mut opened = 0;
    for round in 0..200 {
        let mut bytes = good.clone();
        let flips = 1 + (rng.next() % 16) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % bytes.len();
            bytes[at] ^= 1u8 << (rng.next() % 8);
        }
        std::fs::write(&victim, &bytes).unwrap();
        // Any outcome is fine except a panic, which fails the test outright.
        if let Ok(mut db) = Database::open(&victim) {
            opened += 1;
            let tables: Vec<String> = db
                .schema()
                .tables
                .iter()
                .map(|t| t.name.clone())
                .collect();
            for name in tables {
                let Some(info) = db.schema().table(&name).cloned() else {
                    continue;
                };
                if info.root_page == 0 {
                    continue;
                }
                let (pager, _) = db.parts();
                let mut cursor = TableCursor::new(info.root_page);
                if cursor.rewind(pager).is_err() {
                    continue;
                }
                let mut budget = 100_000;
                loop {
                    budget -= 1;
                    if budget == 0 {
                        panic!("cursor did not terminate on round {round}");
                    }
                    match cursor.next(pager) {
                        Ok(Some(row)) => {
                            let _ = row.payload.values(pager, TextMode::Strict);
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
            }
        }
    }
    assert!(opened > 0, "every corrupted file failed to open; test is vacuous");
}

#[test]
fn header_validation_rejects_junk() {
    let scratch = Scratch::new("hdr");
    let path = scratch.path("junk.db");
    std::fs::write(&path, vec![0u8; 4096]).unwrap();
    assert!(Database::open(&path).is_err());
    std::fs::write(&path, b"not a database at all").unwrap();
    assert!(Database::open(&path).is_err());
    std::fs::write(&path, b"").unwrap();
    assert!(Database::open(&path).is_err());
}

#[test]
fn live_catalog_copy_smoke() {
    // Runs only where the asset-store copy exists; skipped elsewhere.
    let copy = Path::new(
        "/private/tmp/claude-501/-Users-admin-makepad-makepad/9ffb7a56-6354-42a8-8256-89ffed8580ec/scratchpad/store-copy/catalog.sqlite3",
    );
    if !copy.exists() || !have_sqlite3() {
        return;
    }
    let mut db = Database::open(copy).expect("open catalog copy");
    assert_eq!(db.user_version(), 8);
    for t in &db.schema().tables {
        assert!(t.unsupported.is_none(), "{} -> {:?}", t.name, t.unsupported);
    }
    for table in ["assets", "asset_aliases", "search_annotations", "blobs"] {
        let ours = scan_table(&mut db, table).len();
        let cli = sqlite3(copy, &format!(".mode list\nSELECT count(*) FROM {table};\n"));
        assert_eq!(
            ours.to_string(),
            cli.trim(),
            "row count mismatch for {table}"
        );
    }
}

#[test]
fn integrity_check_passes_on_files_sqlite_wrote() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("integrity");
    for page_size in [512u32, 4096] {
        let path = build_db(
            &scratch.dir,
            &format!("i{page_size}.db"),
            &format!("PRAGMA page_size={page_size};\n{FIXTURE_SQL}\nDELETE FROM items WHERE id % 7 = 0;\nDELETE FROM keyed WHERE n % 5 = 0;\n"),
        );
        let mut db = Database::open(&path).unwrap();
        let (pager, schema) = db.parts();
        let report = makepad_sqlite::integrity::check(pager, schema, true).unwrap();
        assert!(
            report.ok(),
            "our checker found problems in a file sqlite wrote at page size {page_size}:\n{}",
            report.problems.join("\n")
        );
        assert!(report.rows > 0 && report.index_entries > 0);
        // and the CLI agrees the file is fine
        let out = sqlite3(&path, "PRAGMA integrity_check;\n");
        assert_eq!(out.trim(), "ok");
    }
}

#[test]
fn integrity_check_catches_damage() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("damage");
    let path = build_db(&scratch.dir, "d.db", &format!("PRAGMA page_size=1024;\n{FIXTURE_SQL}"));
    let mut bytes = std::fs::read(&path).unwrap();
    // Point the root of the first index at a table page.
    let victim = scratch.path("damaged.db");
    for at in [1024 * 3 + 8, 1024 * 5 + 12, 1024 * 9 + 20] {
        if at < bytes.len() {
            bytes[at] ^= 0x55;
        }
    }
    std::fs::write(&victim, &bytes).unwrap();
    let Ok(mut db) = Database::open(&victim) else {
        return; // refusing to open is also a clean outcome
    };
    let (pager, schema) = db.parts();
    match makepad_sqlite::integrity::check(pager, schema, true) {
        Ok(report) => assert!(!report.ok(), "damage went unnoticed"),
        Err(_) => {}
    }
}
