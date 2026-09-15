use makepad_sqlite::{
    Connection, FileStoreSet, MemoryStoreSet, PageStoreSet, StoreKind, StoreOpenOptions, Value,
};
use std::sync::Arc;

fn exercise_sql(stores: Arc<dyn PageStoreSet>) {
    let mut db = Connection::open_with(stores).unwrap();
    db.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL, payload BLOB)",
        &[],
    )
    .unwrap();
    db.execute("CREATE INDEX t_by_name ON t(name)", &[]).unwrap();
    db.execute(
        "INSERT INTO t(name, payload) VALUES ('one', x'0102'), ('two', x'03')",
        &[],
    )
    .unwrap();
    let rows = db
        .query("SELECT id, name, length(payload) FROM t ORDER BY id", &[])
        .unwrap();
    assert_eq!(rows.rows.len(), 2);
    assert_eq!(rows.rows[1][1].as_text(), Some("two"));
    assert_eq!(rows.rows[1][2].as_integer(), Some(1));

    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    db.execute("DELETE FROM t", &[]).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(0)
    );
    db.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(2)
    );
    assert_eq!(
        db.query("PRAGMA quick_check", &[]).unwrap().rows[0][0].as_text(),
        Some("ok")
    );
}

#[test]
fn backend_agnostic_sql_runs_on_file_and_memory() {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "makepad-sqlite-page-store-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    exercise_sql(Arc::new(FileStoreSet::new(&path)));
    exercise_sql(Arc::new(MemoryStoreSet::new()));
    let _ = std::fs::remove_file(path);
}

#[test]
fn memory_rollback_journal_is_created_and_removed() {
    let stores = MemoryStoreSet::new();
    let mut db = Connection::open_with(stores.clone()).unwrap();
    assert_eq!(
        db.query("PRAGMA journal_mode", &[]).unwrap().rows[0][0].as_text(),
        Some("delete")
    );
    db.execute("CREATE TABLE t(v TEXT)", &[]).unwrap();
    assert!(stores
        .open(StoreKind::Journal, StoreOpenOptions::READ_ONLY)
        .unwrap()
        .is_none());

    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    db.execute("INSERT INTO t VALUES ('uncommitted')", &[])
        .unwrap();
    let journal = stores
        .open(StoreKind::Journal, StoreOpenOptions::READ_ONLY)
        .unwrap()
        .expect("rollback journal while transaction is open");
    assert!(journal.len().unwrap() >= 512);
    db.execute("ROLLBACK", &[]).unwrap();
    assert!(stores
        .open(StoreKind::Journal, StoreOpenOptions::READ_ONLY)
        .unwrap()
        .is_none());
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(0)
    );
}

#[test]
fn memory_wal_appends_checkpoints_and_converts_back() {
    let stores = MemoryStoreSet::new();
    let mut db = Connection::open_with(stores.clone()).unwrap();
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    assert_eq!(
        db.query("PRAGMA journal_mode=WAL", &[]).unwrap().rows[0][0].as_text(),
        Some("wal")
    );
    db.execute("INSERT INTO t(v) VALUES ('in-wal')", &[])
        .unwrap();
    assert!(db.pager().wal_frames() > 0);
    let wal = stores
        .open(StoreKind::Wal, StoreOpenOptions::READ_ONLY)
        .unwrap()
        .expect("WAL store");
    assert!(wal.len().unwrap() > 32);
    assert_eq!(
        db.query("SELECT v FROM t", &[]).unwrap().rows[0][0].as_text(),
        Some("in-wal")
    );
    drop(db);
    let mut db = Connection::open_with(stores.clone()).unwrap();
    assert_eq!(
        db.query("SELECT v FROM t", &[]).unwrap().rows[0][0].as_text(),
        Some("in-wal"),
        "a reopened memory connection did not recover its WAL"
    );
    db.query("PRAGMA wal_checkpoint", &[]).unwrap();
    assert_eq!(db.pager().wal_frames(), 0);
    assert_eq!(
        db.query("PRAGMA journal_mode=DELETE", &[]).unwrap().rows[0][0].as_text(),
        Some("delete")
    );
    assert!(stores
        .open(StoreKind::Wal, StoreOpenOptions::READ_ONLY)
        .unwrap()
        .is_none());
}

#[test]
fn memory_quick_check_after_many_pages() {
    let mut db = Connection::open_memory().unwrap();
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT, payload BLOB)", &[])
        .unwrap();
    db.execute("CREATE INDEX t_by_k ON t(k)", &[]).unwrap();
    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    for i in 0..400i64 {
        db.execute(
            "INSERT INTO t(k, payload) VALUES (?1, ?2)",
            &[
                Value::text(format!("key-{:04}", i % 73)),
                Value::Blob(vec![(i % 251) as u8; 6000]),
            ],
        )
        .unwrap();
    }
    db.execute("COMMIT", &[]).unwrap();
    assert!(db.pager().page_count() > 400);
    assert_eq!(
        db.query("PRAGMA quick_check", &[]).unwrap().rows[0][0].as_text(),
        Some("ok")
    );
}
