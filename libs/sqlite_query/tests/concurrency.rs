//! Locking: one writer at a time, readers alongside, and the `sqlite3` CLI as
//! the other party — in this process and across processes.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Error, Value};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn setup(path: &Path) {
    let mut db = Connection::open(path, Duration::from_secs(5)).expect("open");
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    db.execute("INSERT INTO t(v) VALUES ('one'), ('two')", &[])
        .unwrap();
}

#[test]
fn a_second_writer_in_this_process_waits_then_fails() {
    let scratch = Scratch::new("conc-inproc");
    let path = scratch.path("w.db");
    setup(&path);

    let mut first = Connection::open(&path, Duration::from_millis(200)).unwrap();
    first.execute("BEGIN IMMEDIATE", &[]).unwrap();
    first
        .execute("INSERT INTO t(v) VALUES ('from first')", &[])
        .unwrap();

    let mut second = Connection::open(&path, Duration::from_millis(200)).unwrap();
    let started = Instant::now();
    let err = second.execute("BEGIN IMMEDIATE", &[]);
    assert!(
        matches!(err, Err(makepad_sqlite::Error::Busy(_))),
        "second writer was allowed in: {err:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the busy timeout was not honoured"
    );

    // Once the first commits, the second can write.
    first.execute("COMMIT", &[]).unwrap();
    second.execute("BEGIN IMMEDIATE", &[]).unwrap();
    second
        .execute("INSERT INTO t(v) VALUES ('from second')", &[])
        .unwrap();
    second.execute("COMMIT", &[]).unwrap();

    let mut reader = Database::open(&path).unwrap();
    let rows = reader.query("SELECT v FROM t ORDER BY id", &[]).unwrap();
    assert_eq!(rows.rows.len(), 4);
}

#[test]
fn readers_see_the_state_before_an_open_write_transaction() {
    let scratch = Scratch::new("conc-read");
    let path = scratch.path("r.db");
    setup(&path);

    let mut writer = Connection::open(&path, Duration::from_millis(500)).unwrap();
    writer.execute("BEGIN IMMEDIATE", &[]).unwrap();
    for i in 0..50 {
        writer
            .execute("INSERT INTO t(v) VALUES (?1)", &[Value::text(format!("w{i}"))])
            .unwrap();
    }
    // The writer reads back the header fields it wrote, as SQLite does: a migration that sets
    // user_version step by step inside one transaction reads each step's number (it read the
    // committed one and repeated its first step forever, 11-22).
    let cookie = |c: &mut Connection| c.query("PRAGMA schema_version", &[]).unwrap().rows[0][0].as_integer();
    let cookie_before = cookie(&mut writer);
    writer.execute("PRAGMA user_version=7", &[]).unwrap();
    writer.execute("CREATE TABLE u(x)", &[]).unwrap();
    assert_eq!(writer.user_version(), 7, "the writer reads back the user_version it set");
    assert_eq!(writer.query("PRAGMA user_version", &[]).unwrap().rows[0][0].as_integer(), Some(7));
    assert_ne!(cookie(&mut writer), cookie_before, "the writer reads back the schema cookie its DDL bumped");
    // Nothing is in the file yet: a fresh reader still sees two rows, and the old header.
    let mut reader = Database::open(&path).unwrap();
    let before = reader.query("SELECT COUNT(*) FROM t", &[]).unwrap();
    assert_eq!(before.rows[0][0].as_integer(), Some(2));
    assert_eq!(reader.user_version(), 0);

    writer.execute("COMMIT", &[]).unwrap();
    let mut reader = Database::open(&path).unwrap();
    let after = reader.query("SELECT COUNT(*) FROM t", &[]).unwrap();
    assert_eq!(after.rows[0][0].as_integer(), Some(52));
    assert_eq!(reader.user_version(), 7);

    // A rolled-back header write is gone from the writer's reads too.
    writer.execute("BEGIN IMMEDIATE", &[]).unwrap();
    writer.execute("PRAGMA user_version=9", &[]).unwrap();
    assert_eq!(writer.user_version(), 9);
    writer.execute("ROLLBACK", &[]).unwrap();
    assert_eq!(writer.user_version(), 7, "a rollback restores the committed user_version");
    assert_eq!(writer.query("PRAGMA user_version", &[]).unwrap().rows[0][0].as_integer(), Some(7));
}

#[test]
fn the_sqlite_cli_cannot_write_while_we_hold_the_lock() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("conc-cli");
    let path = scratch.path("c.db");
    setup(&path);

    let mut writer = Connection::open(&path, Duration::from_millis(200)).unwrap();
    writer.execute("BEGIN IMMEDIATE", &[]).unwrap();
    writer
        .execute("INSERT INTO t(v) VALUES ('ours')", &[])
        .unwrap();

    // The CLI must report SQLITE_BUSY rather than corrupting the file.
    let out = Command::new("sqlite3")
        .arg("-cmd")
        .arg(".timeout 200")
        .arg(&path)
        .arg("INSERT INTO t(v) VALUES ('theirs');")
        .output()
        .expect("sqlite3");
    assert!(
        !out.status.success(),
        "sqlite3 wrote while we held the write lock"
    );
    let msg = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(msg.contains("locked") || msg.contains("busy"), "{msg}");

    // Reading is still allowed while we hold RESERVED.
    let read = Command::new("sqlite3")
        .arg(&path)
        .arg("SELECT COUNT(*) FROM t;")
        .output()
        .expect("sqlite3");
    assert!(read.status.success());
    assert_eq!(String::from_utf8_lossy(&read.stdout).trim(), "2");

    writer.execute("COMMIT", &[]).unwrap();
    let after = Command::new("sqlite3")
        .arg(&path)
        .arg("INSERT INTO t(v) VALUES ('theirs'); SELECT COUNT(*) FROM t;")
        .output()
        .expect("sqlite3");
    assert!(after.status.success());
    assert_eq!(String::from_utf8_lossy(&after.stdout).trim(), "4");
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
}

#[test]
fn we_wait_for_a_writer_from_another_process() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("conc-wait");
    let path = scratch.path("w2.db");
    setup(&path);

    // A CLI process that holds a write transaction open for a moment.
    let mut child = Command::new("sqlite3")
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn sqlite3");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"BEGIN IMMEDIATE;\nINSERT INTO t(v) VALUES('cli');\nSELECT 'HELD';\n")
            .unwrap();
        stdin.flush().unwrap();
    }
    // Wait until the CLI reports it holds the transaction.
    {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(child.stdout.as_mut().expect("stdout"));
        let mut line = String::new();
        let mut held = false;
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line.contains("HELD") {
                held = true;
                break;
            }
            line.clear();
        }
        assert!(held, "the CLI never reached its transaction");
    }

    let mut ours = Connection::open(&path, Duration::from_millis(200)).unwrap();
    let err = ours.execute("INSERT INTO t(v) VALUES ('ours')", &[]);
    assert!(
        matches!(err, Err(makepad_sqlite::Error::Busy(_))),
        "we wrote while another process held the lock: {err:?}"
    );

    let _ = child.kill();
    let _ = child.wait();
    // The killed CLI leaves a hot journal; opening rolls it back.
    let mut ours = Connection::open(&path, Duration::from_secs(2)).unwrap();
    ours.execute("INSERT INTO t(v) VALUES ('ours')", &[])
        .unwrap();
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "3",
        "the interrupted CLI transaction should have been rolled back"
    );
}

#[test]
fn sqlite_can_read_a_wal_database_after_our_connection_closes() {
    if !have_sqlite3() {
        return;
    }
    // A live connection owns the log for its whole life (there is exactly one
    // serving process per store root). The interop contract is at CLOSE: the
    // guard's release zeroes the wal-index header, which is SQLite's signal
    // to rebuild the index from the log — so a `sqlite3` process sees every
    // frame we appended, and a fresh connection of ours recovers frames a
    // `sqlite3` process wrote in between.
    let scratch = Scratch::new("conc-wal-share");
    let path = scratch.path("shared.db");
    {
        let mut db = Connection::open(&path, Duration::from_millis(500)).unwrap();
        db.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
        db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
            .unwrap();
        db.execute("INSERT INTO t(v) VALUES ('first')", &[]).unwrap();
        db.execute("INSERT INTO t(v) VALUES ('second')", &[]).unwrap();
    }
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT v FROM t ORDER BY id;\n").trim(),
        "first\nsecond"
    );
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");

    // And the other direction: rows the CLI wrote show up for a fresh
    // connection of ours.
    sqlite3(&path, "INSERT INTO t(v) VALUES ('theirs');");
    let mut db = Connection::open(&path, Duration::from_millis(500)).unwrap();
    assert_eq!(
        db.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(3),
        "a row written by sqlite3 into the same WAL must be visible"
    );
    assert_eq!(
        db.query("SELECT v FROM t ORDER BY id", &[]).unwrap().rows[2][0].as_text(),
        Some("theirs")
    );
}
/// A commit that fails partway leaves its frames in the log with no commit
/// frame behind them, so the file is longer than its committed content. The
/// next transaction writes over exactly those offsets: same salts, same file
/// length, different content. A reader that takes an unchanged length as proof
/// that nothing happened stays parked on the older snapshot for as long as the
/// log does not grow past that mark.
#[test]
fn a_reader_sees_a_commit_that_reuses_abandoned_frames() {
    let scratch = Scratch::new("live-reader-abandoned");
    let path = scratch.path("r.db");
    let mut w = Connection::open(&path, Duration::from_secs(5)).unwrap();
    w.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
    w.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    w.execute("INSERT INTO t(v) VALUES('seed')", &[]).unwrap();

    let mut reader = Database::open(&path).unwrap();
    assert_eq!(
        reader.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(1)
    );
    let page_size = reader.pager().page_size();

    // Frames appended and never committed, exactly what a commit interrupted
    // between its first frame and its sync leaves behind.
    {
        let mut wal = makepad_sqlite::wal::Wal::open(&path, page_size as u32, true)
            .unwrap()
            .expect("the database is in WAL mode");
        let filler = vec![0x5au8; page_size];
        for _ in 0..40 {
            wal.append(2, &filler, 0).unwrap();
        }
        // No commit(): the frames are in the file and belong to nothing.
    }

    // The reader looks while those abandoned frames are still in the file.
    reader.refresh().unwrap();
    assert_eq!(
        reader.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(1),
        "frames without a commit frame must not be visible"
    );

    // A small commit now lands on the very offsets those frames used, leaving
    // the log exactly as long as it already was.
    w.execute("INSERT INTO t(v) VALUES('after')", &[]).unwrap();
    reader.refresh().unwrap();
    let rows = reader.query("SELECT v FROM t ORDER BY id", &[]).unwrap();
    assert_eq!(
        rows.rows.len(),
        2,
        "the reader missed a commit that reused the abandoned frames"
    );
    assert_eq!(rows.rows[1][0].as_text(), Some("after"));
}

/// A checkpoint truncates the log, bumps its salts and starts writing fresh
/// frames from the top — over the exact byte offsets an open snapshot's index
/// still points at, now holding entirely different pages. Reading through that
/// index must never hand out whatever page happens to sit there: a b-tree page
/// of some other tree decodes perfectly well, and an index scan that walks into
/// one produces rowids that satisfy no predicate at all.
#[test]
fn a_checkpointed_away_snapshot_never_answers_with_another_page() {
    let scratch = Scratch::new("live-reader-checkpoint");
    let path = scratch.path("c.db");
    let mut w = Connection::open(&path, Duration::from_secs(5)).unwrap();
    w.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
    w.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    let filler = "y".repeat(500);
    let fill = |w: &mut Connection, from: usize, to: usize| {
        w.execute("BEGIN", &[]).unwrap();
        for i in from..to {
            w.execute(
                "INSERT INTO t(v) VALUES(?1)",
                &[Value::text(format!("{filler}{i}"))],
            )
            .unwrap();
        }
        w.execute("COMMIT", &[]).unwrap();
    };
    fill(&mut w, 0, 900);

    // A small cache, so the snapshot's answers really do come off the log
    // rather than out of pages this handle happens to still hold.
    let mut reader = Database::open_with_cache(&path, 4).unwrap();
    assert_eq!(
        reader
            .query("SELECT v FROM t WHERE id = 1", &[])
            .unwrap()
            .rows
            .len(),
        1
    );
    let pages = reader.pager().page_count();
    assert!(pages > 40, "the fixture needs a multi-page table");
    // A page from the middle of the log the reader is holding open: early
    // enough that the next generation of frames covers its offset.
    let probe = 20;

    // The log is folded away and a new generation is written over it, long
    // enough to reach the offsets the reader's snapshot still names.
    w.execute("PRAGMA wal_checkpoint", &[]).unwrap();
    fill(&mut w, 900, 1800);

    let truth = Database::open(&path).unwrap().pager().page(probe).unwrap();
    match reader.pager().page(probe) {
        // Refusing is the right answer: the snapshot this offset belonged to
        // is gone, and the error says so (a fresh snapshot answers).
        Err(e @ Error::Busy(_)) => assert!(e.is_snapshot_reset(), "a lost snapshot reads as one: {e}"),
        Ok(bytes) => assert_eq!(
            &bytes[..],
            &truth[..],
            "the reader answered page {probe} with the bytes of a different page"
        ),
        Err(e) => panic!("unexpected error reading page {probe}: {e}"),
    }

    // And the handle recovers: refreshing moves it onto the new snapshot.
    reader.refresh().unwrap();
    assert_eq!(
        reader.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(1800)
    );

    // A reader in the middle of a statement holds its snapshot while the
    // writer commits past the autocheckpoint (1000 frames). Folding the log
    // then would overwrite database pages the snapshot still reads, and
    // truncating it would take its frames away: no checkpoint runs until the
    // reader is done, so every row it reads is the snapshot's.
    let before: Vec<String> = reader
        .query("SELECT v FROM t ORDER BY id", &[])
        .unwrap()
        .rows
        .iter()
        .map(|r| r[0].as_text().unwrap().to_string())
        .collect();
    let stmt = reader.prepare("SELECT v FROM t ORDER BY id").unwrap();
    let mut seen = Vec::new();
    let mut log_at_end = 0;
    stmt.for_each(&mut reader, &[], |row| {
        if seen.is_empty() {
            for round in 0..6 {
                w.execute("UPDATE t SET v = ?1 || v", &[Value::text(format!("r{round}"))])
                    .unwrap();
            }
            log_at_end = w.pager().wal_frames();
        }
        seen.push(row[0].as_text().unwrap().to_string());
        Ok(true)
    })
    .unwrap();
    assert!(
        log_at_end >= 1000,
        "the writer's commits passed the autocheckpoint while the reader read ({log_at_end} frames)"
    );
    assert!(
        seen == before,
        "a reader's statement reads the snapshot it began with across a writer's commits past the autocheckpoint ({} rows, {} differ)",
        seen.len(),
        seen.iter().zip(&before).filter(|(a, b)| a != b).count()
    );
    // Once the reader is done, the next commit folds the log.
    fill(&mut w, 1800, 1801);
    assert_eq!(
        w.pager().wal_frames(),
        0,
        "the first commit after the reader is done checkpoints the log"
    );

    // A reader that opens while a checkpoint starts the log over: it reads the
    // log's length, the checkpoint truncates the log and writes the next
    // generation's header, and the reader's scan then runs past the new end
    // (a store's connection threads open readers on demand, so this happens
    // right after a deferred checkpoint runs). The stale length is replayed
    // here exactly: the open takes a fresh snapshot, never an I/O error.
    fill(&mut w, 1801, 1900);
    let stale_len = std::fs::metadata(scratch.path("c.db-wal")).unwrap().len();
    assert!(w.pager().wal_frames() > 8, "the log holds frames to scan");
    w.execute("PRAGMA wal_checkpoint", &[]).unwrap();
    assert_eq!(w.pager().wal_frames(), 0, "the checkpoint started the log over");
    let stale = std::sync::Arc::new(std::sync::Mutex::new(Some(stale_len)));
    let stores = std::sync::Arc::new(StaleLogLength {
        inner: makepad_sqlite::FileStoreSet::new(&path),
        stale_len: stale.clone(),
    });
    let mut r = Connection::open_store_read_only(stores, Duration::from_secs(5))
        .expect("a reader whose first scan ran past a restarted log opens on a fresh snapshot");
    assert!(stale.lock().unwrap().is_none(), "the open read the stale length");
    assert_eq!(
        r.query("SELECT COUNT(*) FROM t", &[]).unwrap().rows[0][0].as_integer(),
        Some(1900),
        "the fresh snapshot is the checkpointed database"
    );
}

/// A file store set whose log answers one length query with the length it had
/// before a checkpoint truncated it, as a reader sees it when the truncation
/// lands between its length query and its reads.
struct StaleLogLength {
    inner: makepad_sqlite::FileStoreSet,
    stale_len: std::sync::Arc<std::sync::Mutex<Option<u64>>>,
}

struct StaleLog {
    inner: std::sync::Arc<dyn makepad_sqlite::PageStore>,
    stale_len: std::sync::Arc<std::sync::Mutex<Option<u64>>>,
}

impl makepad_sqlite::PageStoreSet for StaleLogLength {
    fn open(
        &self,
        kind: makepad_sqlite::StoreKind,
        options: makepad_sqlite::StoreOpenOptions,
    ) -> std::io::Result<Option<std::sync::Arc<dyn makepad_sqlite::PageStore>>> {
        let opened = self.inner.open(kind, options)?;
        Ok(match (kind, opened) {
            (makepad_sqlite::StoreKind::Wal, Some(inner)) => Some(std::sync::Arc::new(StaleLog { inner, stale_len: self.stale_len.clone() })),
            (_, opened) => opened,
        })
    }
    fn remove(&self, kind: makepad_sqlite::StoreKind) -> std::io::Result<()> {
        self.inner.remove(kind)
    }
    fn process_lock(&self) -> std::sync::Arc<makepad_sqlite::lock::ProcessLock> {
        self.inner.process_lock()
    }
    fn path(&self, kind: makepad_sqlite::StoreKind) -> Option<std::path::PathBuf> {
        self.inner.path(kind)
    }
}

impl makepad_sqlite::PageStore for StaleLog {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.inner.read_at(offset, buf)
    }
    fn write_at(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.inner.write_at(offset, data)
    }
    fn len(&self) -> std::io::Result<u64> {
        match self.stale_len.lock().unwrap().take() {
            Some(stale) => Ok(stale),
            None => self.inner.len(),
        }
    }
    fn truncate(&self, len: u64) -> std::io::Result<()> {
        self.inner.truncate(len)
    }
    fn sync(&self) -> std::io::Result<()> {
        self.inner.sync()
    }
    fn lock(&self, kind: makepad_sqlite::StoreLock, start: u64, len: u64) -> std::io::Result<bool> {
        self.inner.lock(kind, start, len)
    }
    fn unlock(&self, start: u64, len: u64) -> std::io::Result<()> {
        self.inner.unlock(start, len)
    }
    fn is_write_locked(&self, start: u64, len: u64) -> std::io::Result<bool> {
        self.inner.is_write_locked(start, len)
    }
}
