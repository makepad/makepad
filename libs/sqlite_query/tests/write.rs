//! P2 acceptance for the storage layer: rows written by this engine are read
//! back by the `sqlite3` CLI, `PRAGMA integrity_check` says `ok`, and our own
//! checker agrees — through splits, overflow chains, deletes and freelist
//! reuse.

mod common;

use common::*;
use makepad_sqlite::btree_write::BtreeWriter;
use makepad_sqlite::value::encode_record;
use makepad_sqlite::{Database, Pager, Value};
use std::path::Path;
use std::time::Duration;

fn root_of(path: &Path, table: &str) -> u32 {
    let db = Database::open(path).expect("open");
    db.schema().table(table).expect("table").root_page
}

fn index_root(path: &Path, table: &str, index: &str) -> u32 {
    let db = Database::open(path).expect("open");
    db.schema()
        .table(table)
        .expect("table")
        .indexes
        .iter()
        .find(|i| i.name == index)
        .unwrap_or_else(|| panic!("index {index}"))
        .root_page
}

fn check_with_cli(path: &Path) {
    // Keep a copy of any file the CLI rejects, so the failure can be inspected.
    let out = std::process::Command::new("sqlite3")
        .arg(path)
        .arg("PRAGMA integrity_check;")
        .output()
        .expect("sqlite3");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text != "ok" {
        let keep = std::env::temp_dir().join("makepad-sqlite-failed.db");
        let _ = std::fs::copy(path, &keep);
        panic!("sqlite3 integrity_check said {text:?} {err:?}; copy at {keep:?}");
    }
}

fn check_ourselves(path: &Path) {
    let mut db = Database::open(path).expect("reopen");
    let (pager, schema) = db.parts();
    let report = makepad_sqlite::integrity::check(pager, schema, true).expect("check");
    assert!(
        report.ok(),
        "our integrity check found problems:\n{}",
        report.problems.join("\n")
    );
}

#[test]
fn insert_split_and_read_back_with_sqlite() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-insert");
    for page_size in [512u32, 4096] {
        let path = build_db(
            &scratch.dir,
            &format!("w{page_size}.db"),
            &format!(
                "PRAGMA page_size={page_size};\nCREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, blob BLOB);\n"
            ),
        );
        let root = root_of(&path, "t");
        let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).expect("open rw");
        pager.begin_write(true).expect("begin");
        {
            let mut w = BtreeWriter::new(&mut pager);
            for i in 1..=1000i64 {
                let big = if i % 50 == 0 { 9000 } else { (i % 30) as usize };
                let payload = encode_record(&[
                    Value::Null, // INTEGER PRIMARY KEY is the rowid
                    Value::Text(format!("name-{i}")),
                    Value::Blob(vec![(i % 251) as u8; big]),
                ]);
                w.insert_table(root, i, &payload).expect("insert");
            }
        }
        pager.commit().expect("commit");
        drop(pager);

        check_with_cli(&path);
        check_ourselves(&path);
        let count = sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n");
        assert_eq!(count.trim(), "1000", "page size {page_size}");
        let sample = sqlite3(
            &path,
            ".mode list\nSELECT id, name, length(blob) FROM t WHERE id IN (1, 50, 999, 1000) ORDER BY id;\n",
        );
        assert_eq!(
            sample.trim().lines().collect::<Vec<_>>(),
            vec!["1|name-1|1", "50|name-50|9000", "999|name-999|9", "1000|name-1000|9000"],
            "page size {page_size}"
        );
        // Our own reader agrees with the CLI, row for row.
        let mut db = Database::open(&path).unwrap();
        let ours = db
            .query("SELECT id, name, length(blob) FROM t ORDER BY id", &[])
            .unwrap();
        assert_eq!(ours.rows.len(), 1000);
        assert_eq!(ours.rows[499][0].as_integer(), Some(500));
    }
}

#[test]
fn delete_frees_pages_and_the_freelist_is_reused() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-delete");
    let path = build_db(
        &scratch.dir,
        "d.db",
        "PRAGMA page_size=512;\nCREATE TABLE t(id INTEGER PRIMARY KEY, v BLOB);\n",
    );
    let root = root_of(&path, "t");

    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=400i64 {
            let payload = encode_record(&[Value::Null, Value::Blob(vec![7u8; 300])]);
            w.insert_table(root, i, &payload).unwrap();
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_with_cli(&path);
    let pages_before = std::fs::metadata(&path).unwrap().len();

    // Delete most rows: pages go on the freelist.
    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=350i64 {
            assert!(w.delete_table(root, i).unwrap(), "row {i} should exist");
        }
        assert!(!w.delete_table(root, 9999).unwrap());
    }
    pager.commit().unwrap();
    drop(pager);
    check_with_cli(&path);
    check_ourselves(&path);
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "50"
    );
    let free = sqlite3(&path, ".mode list\nPRAGMA freelist_count;\n");
    assert!(
        free.trim().parse::<i64>().unwrap_or(0) > 0,
        "deleting 350 rows freed no pages"
    );

    // Re-inserting reuses those pages instead of growing the file.
    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=300i64 {
            let payload = encode_record(&[Value::Null, Value::Blob(vec![9u8; 300])]);
            w.insert_table(root, i, &payload).unwrap();
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_with_cli(&path);
    check_ourselves(&path);
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "350"
    );
    let pages_after = std::fs::metadata(&path).unwrap().len();
    assert!(
        pages_after <= pages_before + 8192,
        "file grew from {pages_before} to {pages_after} instead of reusing the freelist"
    );
}

#[test]
fn index_inserts_and_deletes_stay_ordered() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-index");
    let path = build_db(
        &scratch.dir,
        "i.db",
        "PRAGMA page_size=512;\nCREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT);\nCREATE INDEX t_by_k ON t(k);\n",
    );
    let root = root_of(&path, "t");
    let idx = index_root(&path, "t", "t_by_k");

    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=500i64 {
            let key = format!("key-{:04}", (i * 7919) % 500);
            let payload = encode_record(&[Value::Null, Value::Text(key.clone())]);
            w.insert_table(root, i, &payload).unwrap();
            let entry = encode_record(&[Value::Text(key), Value::Integer(i)]);
            w.insert_index(idx, &entry, &[makepad_sqlite::Collation::Binary])
                .unwrap();
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_with_cli(&path);
    check_ourselves(&path);

    // The CLI can use the index we built.
    let via_index = sqlite3(
        &path,
        ".mode list\nSELECT COUNT(*) FROM t WHERE k = 'key-0100';\n",
    );
    assert_eq!(via_index.trim(), "1");
    let ordered = sqlite3(&path, ".mode list\nSELECT k FROM t ORDER BY k LIMIT 3;\n");
    assert_eq!(
        ordered.trim().lines().collect::<Vec<_>>(),
        vec!["key-0000", "key-0001", "key-0002"]
    );

    // Delete half of the index entries and check both engines again.
    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=250i64 {
            let key = format!("key-{:04}", (i * 7919) % 500);
            let entry = encode_record(&[Value::Text(key), Value::Integer(i)]);
            assert!(
                w.delete_index(idx, &entry, &[makepad_sqlite::Collation::Binary])
                    .unwrap(),
                "index entry {i} should exist"
            );
            assert!(w.delete_table(root, i).unwrap());
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_with_cli(&path);
    check_ourselves(&path);
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "250"
    );
}

#[test]
fn rollback_undoes_everything() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-rollback");
    let path = build_db(
        &scratch.dir,
        "r.db",
        "PRAGMA page_size=512;\nCREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);\nINSERT INTO t VALUES (1,'one'),(2,'two');\n",
    );
    let root = root_of(&path, "t");
    let before = std::fs::read(&path).unwrap();

    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 3..=200i64 {
            let payload = encode_record(&[Value::Null, Value::Text(format!("v{i}"))]);
            w.insert_table(root, i, &payload).unwrap();
        }
    }
    pager.rollback().unwrap();
    drop(pager);

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "rollback left the file changed"
    );
    check_with_cli(&path);
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "2"
    );
}

#[test]
fn tiny_insert_is_well_formed() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-tiny");
    let path = build_db(
        &scratch.dir,
        "tiny.db",
        "PRAGMA page_size=4096;\nCREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);\n",
    );
    let root = root_of(&path, "t");
    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=3i64 {
            let payload = encode_record(&[Value::Null, Value::Text(format!("v{i}"))]);
            w.insert_table(root, i, &payload).unwrap();
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_ourselves(&path);
    check_with_cli(&path);
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT id, v FROM t ORDER BY id;\n")
            .trim()
            .lines()
            .collect::<Vec<_>>(),
        vec!["1|v1", "2|v2", "3|v3"]
    );
}

#[test]
fn growing_insert_stays_well_formed() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-grow");
    let path = build_db(
        &scratch.dir,
        "g.db",
        "PRAGMA page_size=512;\nCREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT);\n",
    );
    let root = root_of(&path, "t");
    let mut next = 1i64;
    for batch in 0..20 {
        let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
        pager.begin_write(true).unwrap();
        {
            let mut w = BtreeWriter::new(&mut pager);
            for _ in 0..10 {
                let payload = encode_record(&[Value::Null, Value::Text(format!("value-{next}"))]);
                w.insert_table(root, next, &payload).unwrap();
                next += 1;
            }
        }
        pager.commit().unwrap();
        drop(pager);
        let mut db = Database::open(&path).unwrap();
        let (pager, schema) = db.parts();
        let report = makepad_sqlite::integrity::check(pager, schema, true).unwrap();
        assert!(
            report.ok(),
            "after batch {batch} ({} rows):\n{}",
            next - 1,
            report.problems.join("\n")
        );
        drop(db);
        let out = sqlite3(&path, "PRAGMA integrity_check;\n");
        assert_eq!(out.trim(), "ok", "sqlite3 after batch {batch}");
        let count = sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n");
        assert_eq!(count.trim(), (next - 1).to_string(), "after batch {batch}");
    }
}

#[test]
fn deleting_batch_by_batch_stays_well_formed() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("write-del-grow");
    let path = build_db(
        &scratch.dir,
        "dg.db",
        "PRAGMA page_size=512;\nCREATE TABLE t(id INTEGER PRIMARY KEY, v BLOB);\n",
    );
    let root = root_of(&path, "t");
    let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
    pager.begin_write(true).unwrap();
    {
        let mut w = BtreeWriter::new(&mut pager);
        for i in 1..=200i64 {
            let payload = encode_record(&[Value::Null, Value::Blob(vec![3u8; 200])]);
            w.insert_table(root, i, &payload).unwrap();
        }
    }
    pager.commit().unwrap();
    drop(pager);
    check_ourselves(&path);
    check_with_cli(&path);

    for batch in 0..10 {
        let mut pager = Pager::open_rw(&path, Duration::from_secs(5)).unwrap();
        pager.begin_write(true).unwrap();
        {
            let mut w = BtreeWriter::new(&mut pager);
            for k in 0..10i64 {
                let id = batch * 10 + k + 1;
                assert!(w.delete_table(root, id).unwrap(), "row {id}");
            }
        }
        pager.commit().unwrap();
        drop(pager);
        let mut db = Database::open(&path).unwrap();
        let (p, schema) = db.parts();
        let report = makepad_sqlite::integrity::check(p, schema, true).unwrap();
        assert!(
            report.ok(),
            "after delete batch {batch}:\n{}",
            report.problems.join("\n")
        );
        drop(db);
        assert_eq!(
            sqlite3(&path, "PRAGMA integrity_check;\n").trim(),
            "ok",
            "sqlite3 after delete batch {batch}"
        );
        assert_eq!(
            sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
            (200 - (batch + 1) * 10).to_string(),
            "after delete batch {batch}"
        );
    }
}
