//! Locking: one writer at a time, readers alongside, and the `sqlite3` CLI as
//! the other party — in this process and across processes.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Value};
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
