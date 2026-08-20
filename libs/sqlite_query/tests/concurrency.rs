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
    // Nothing is in the file yet: a fresh reader still sees two rows.
    let mut reader = Database::open(&path).unwrap();
    let before = reader.query("SELECT COUNT(*) FROM t", &[]).unwrap();
    assert_eq!(before.rows[0][0].as_integer(), Some(2));

    writer.execute("COMMIT", &[]).unwrap();
    let mut reader = Database::open(&path).unwrap();
    let after = reader.query("SELECT COUNT(*) FROM t", &[]).unwrap();
    assert_eq!(after.rows[0][0].as_integer(), Some(52));
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
fn sqlite_can_read_a_wal_database_between_our_transactions() {
    if !have_sqlite3() {
        return;
    }
    // The engine owns the log only while a statement runs. When it goes idle
    // it zeroes the wal-index header, which is SQLite's signal to rebuild the
    // index from the log — so a `sqlite3` process sees frames we appended.
    let scratch = Scratch::new("conc-wal-share");
    let path = scratch.path("shared.db");
    let mut db = Connection::open(&path, Duration::from_millis(500)).unwrap();
    db.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
    db.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", &[])
        .unwrap();
    db.execute("INSERT INTO t(v) VALUES ('first')", &[]).unwrap();

    // The connection stays open and idle; the CLI must still read our rows.
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT COUNT(*) FROM t;\n").trim(),
        "1"
    );
    db.execute("INSERT INTO t(v) VALUES ('second')", &[]).unwrap();
    assert_eq!(
        sqlite3(&path, ".mode list\nSELECT v FROM t ORDER BY id;\n").trim(),
        "first\nsecond"
    );
    assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");

    // And the other direction: rows the CLI wrote show up for us.
    sqlite3(&path, "INSERT INTO t(v) VALUES ('theirs');");
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
        // is gone.
        Err(Error::Busy(_)) => {}
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
}
