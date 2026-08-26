//! Crash injection: kill the process at every durable write step of a commit,
//! reopen the database, and require that it is intact and shows either the old
//! state or the new one — never a mixture, never a torn page.
//!
//! The child process is this same test binary re-executed with
//! `MAKEPAD_SQLITE_CRASH_CHILD` set; the pager's crash hook aborts it (no
//! unwinding, no flushing) after the requested number of write steps.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const ROWS_BEFORE: i64 = 60;
const ROWS_ADDED: i64 = 40;

fn setup(path: &Path) {
    setup_mode(path, false)
}

fn setup_mode(path: &Path, wal: bool) {
    let mut db = Connection::open(path, Duration::from_secs(5)).expect("open");
    if wal {
        db.execute("PRAGMA journal_mode=WAL", &[]).expect("wal");
    }
    db.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, k TEXT, v BLOB)",
        &[],
    )
    .unwrap();
    db.execute("CREATE INDEX t_by_k ON t(k)", &[]).unwrap();
    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    for i in 1..=ROWS_BEFORE {
        db.execute(
            "INSERT INTO t(id, k, v) VALUES (?1, ?2, ?3)",
            &[
                Value::Integer(i),
                Value::text(format!("k{i:04}")),
                Value::Blob(vec![1u8; 120]),
            ],
        )
        .unwrap();
    }
    db.execute("COMMIT", &[]).unwrap();
}

/// The transaction the child runs; also used with no crash to count the steps.
fn mutate(path: &Path) {
    let mut db = Connection::open(path, Duration::from_secs(5)).expect("open");
    db.execute("BEGIN IMMEDIATE", &[]).unwrap();
    for i in ROWS_BEFORE + 1..=ROWS_BEFORE + ROWS_ADDED {
        db.execute(
            "INSERT INTO t(id, k, v) VALUES (?1, ?2, ?3)",
            &[
                Value::Integer(i),
                Value::text(format!("k{i:04}")),
                Value::Blob(vec![2u8; 900]),
            ],
        )
        .unwrap();
    }
    db.execute("DELETE FROM t WHERE id <= 20", &[]).unwrap();
    db.execute("UPDATE t SET k = 'updated' WHERE id BETWEEN 30 AND 40", &[])
        .unwrap();
    db.execute("COMMIT", &[]).unwrap();
}

/// Child mode: run the transaction with the crash countdown armed.
#[test]
fn crash_child() {
    let Ok(path) = std::env::var("MAKEPAD_SQLITE_CRASH_CHILD") else {
        return; // not the child: nothing to do
    };
    let at: u64 = std::env::var("MAKEPAD_SQLITE_CRASH_AT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    makepad_sqlite::pager::set_crash_after(at);
    mutate(Path::new(&path));
    // Reaching this point means the crash point was past the end of the commit.
    println!("STEPS {}", makepad_sqlite::pager::write_steps());
}

fn run_child(db: &Path, crash_at: u64) -> (bool, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "crash_child", "--nocapture"])
        .env("MAKEPAD_SQLITE_CRASH_CHILD", db)
        .env("MAKEPAD_SQLITE_CRASH_AT", crash_at.to_string())
        .output()
        .expect("spawn child");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn state(path: &Path) -> (i64, i64, i64) {
    let mut db = Database::open(path).expect("reopen");
    let rows = db
        .query("SELECT COUNT(*) FROM t", &[])
        .expect("count")
        .rows[0][0]
        .as_integer()
        .unwrap_or(-1);
    let updated = db
        .query("SELECT COUNT(*) FROM t WHERE k = 'updated'", &[])
        .expect("count updated")
        .rows[0][0]
        .as_integer()
        .unwrap_or(-1);
    let max = db
        .query("SELECT COALESCE(MAX(id), 0) FROM t", &[])
        .expect("max")
        .rows[0][0]
        .as_integer()
        .unwrap_or(-1);
    (rows, updated, max)
}

fn recover_and_check(path: &Path, scratch: &Scratch, tag: &str) -> (i64, i64, i64) {
    // Opening read-write is what performs hot-journal recovery.
    {
        let _conn = Connection::open(path, Duration::from_secs(5)).expect("recover");
    }
    let mut db = Database::open(path).expect("reopen");
    let (pager, schema) = db.parts();
    let report = makepad_sqlite::integrity::check(pager, schema, true).expect("integrity");
    if !report.ok() {
        let keep = scratch.path(&format!("broken-{tag}.db"));
        let _ = std::fs::copy(path, &keep);
        panic!(
            "after a crash at {tag} the database is damaged:\n{}",
            report.problems.join("\n")
        );
    }
    drop(db);
    if have_sqlite3() {
        let out = sqlite3(path, "PRAGMA integrity_check;\n");
        assert_eq!(out.trim(), "ok", "sqlite3 disagrees after a crash at {tag}");
    }
    state(path)
}

#[test]
fn every_crash_point_leaves_a_consistent_database() {
    if std::env::var("MAKEPAD_SQLITE_CRASH_CHILD").is_ok() {
        return; // this process is a child
    }
    let scratch = Scratch::new("crash");
    let pristine = scratch.path("pristine.db");
    setup(&pristine);
    let before = state(&pristine);
    assert_eq!(before.0, ROWS_BEFORE);

    // How many write steps a full commit takes.
    let work = scratch.path("count.db");
    std::fs::copy(&pristine, &work).unwrap();
    let (ok, out) = run_child(&work, 0);
    assert!(ok, "uncrashed child failed: {out}");
    let steps: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix("STEPS "))
        .and_then(|v| v.trim().parse().ok())
        .expect("child reported no step count");
    assert!(steps > 5, "expected a multi-step commit, got {steps}");
    eprintln!("crash harness: {steps} durable write steps per commit");
    let after = state(&work);
    assert_eq!(after.0, ROWS_BEFORE + ROWS_ADDED - 20);
    assert_eq!(after.1, 11);

    // Crash at every single step and check what survives.
    let mut crashed = 0;
    let mut committed = 0;
    for at in 1..=steps {
        let path = scratch.path(&format!("crash-{at}.db"));
        std::fs::copy(&pristine, &path).unwrap();
        let (ok, _) = run_child(&path, at);
        if !ok {
            crashed += 1;
        }
        let recovered = recover_and_check(&path, &scratch, &at.to_string());
        // The only two acceptable outcomes: the old state or the new one.
        assert!(
            recovered == before || recovered == after,
            "crash at step {at} left a mixture: {recovered:?} (before {before:?}, after {after:?})"
        );
        if recovered == after {
            committed += 1;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-journal"));
    }
    assert!(crashed > 0, "no crash point actually aborted the child");
    assert!(
        committed > 0,
        "no crash point landed after the commit; the harness is not covering the end"
    );
}

#[test]
fn every_crash_point_in_wal_mode_leaves_a_consistent_database() {
    if std::env::var("MAKEPAD_SQLITE_CRASH_CHILD").is_ok() {
        return;
    }
    let scratch = Scratch::new("crash-wal");
    let pristine = scratch.path("pristine.db");
    setup_mode(&pristine, true);
    let pristine_wal = scratch.path("pristine.db-wal");
    let before = state(&pristine);
    assert_eq!(before.0, ROWS_BEFORE);

    let copy_pair = |from: &Path, to: &Path| {
        std::fs::copy(from, to).expect("copy db");
        let from_wal = PathBuf::from(format!("{}-wal", from.display()));
        let to_wal = PathBuf::from(format!("{}-wal", to.display()));
        if from_wal.exists() {
            std::fs::copy(&from_wal, &to_wal).expect("copy wal");
        } else {
            let _ = std::fs::remove_file(&to_wal);
        }
    };
    assert!(pristine_wal.exists(), "the fixture is not in WAL mode");

    let work = scratch.path("count.db");
    copy_pair(&pristine, &work);
    let (ok, out) = run_child(&work, 0);
    assert!(ok, "uncrashed child failed: {out}");
    let steps: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix("STEPS "))
        .and_then(|v| v.trim().parse().ok())
        .expect("child reported no step count");
    eprintln!("wal crash harness: {steps} durable write steps per commit");
    let after = state(&work);
    assert_eq!(after.0, ROWS_BEFORE + ROWS_ADDED - 20);

    let mut committed = 0;
    for at in 1..=steps {
        let path = scratch.path(&format!("wal-crash-{at}.db"));
        copy_pair(&pristine, &path);
        let (_ok, _) = run_child(&path, at);
        let recovered = recover_and_check(&path, &scratch, &format!("wal-{at}"));
        assert!(
            recovered == before || recovered == after,
            "crash at WAL step {at} left a mixture: {recovered:?} (before {before:?}, after {after:?})"
        );
        if recovered == after {
            committed += 1;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
    }
    assert!(committed > 0, "no crash point landed after the commit");
}

/// Child mode for the checkpoint sweep: fold the log with the countdown armed.
#[test]
fn crash_child_checkpoint() {
    let Ok(path) = std::env::var("MAKEPAD_SQLITE_CRASH_CHILD_CKPT") else {
        return; // not the child: nothing to do
    };
    let at: u64 = std::env::var("MAKEPAD_SQLITE_CRASH_AT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut db = Connection::open(Path::new(&path), Duration::from_secs(5)).expect("open");
    makepad_sqlite::pager::set_crash_after(at);
    db.execute("PRAGMA wal_checkpoint", &[]).expect("checkpoint");
    println!("STEPS {}", makepad_sqlite::pager::write_steps());
}

fn run_checkpoint_child(db: &Path, crash_at: u64) -> (bool, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "crash_child_checkpoint", "--nocapture"])
        .env("MAKEPAD_SQLITE_CRASH_CHILD_CKPT", db)
        .env("MAKEPAD_SQLITE_CRASH_AT", crash_at.to_string())
        .output()
        .expect("spawn child");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// CRASH SAFETY OF THE CHECKPOINT ITSELF: a writer killed at any durable
/// step of folding the log into the database leaves either the old WAL over
/// a (possibly torn) database — which recovery replays — or the folded
/// state; the logical content is identical at every point, and it is NEVER a
/// mixture. This is the property that lets a connection own its log for
/// life: nothing about crash recovery depends on idle-time lock courtesy.
/// (Every sweep iteration also re-acquires WAL ownership after an aborted
/// owner, which is the no-stale-lock property: POSIX locks die with the
/// process.)
#[test]
fn every_crash_point_in_a_wal_checkpoint_leaves_a_consistent_database() {
    if std::env::var("MAKEPAD_SQLITE_CRASH_CHILD").is_ok()
        || std::env::var("MAKEPAD_SQLITE_CRASH_CHILD_CKPT").is_ok()
    {
        return;
    }
    let scratch = Scratch::new("crash-ckpt");
    let pristine = scratch.path("pristine.db");
    setup_mode(&pristine, true);
    let pristine_wal = scratch.path("pristine.db-wal");
    assert!(pristine_wal.exists(), "the fixture is not in WAL mode");
    let before = state(&pristine);
    assert_eq!(before.0, ROWS_BEFORE);

    let copy_pair = |from: &Path, to: &Path| {
        std::fs::copy(from, to).expect("copy db");
        let from_wal = PathBuf::from(format!("{}-wal", from.display()));
        let to_wal = PathBuf::from(format!("{}-wal", to.display()));
        if from_wal.exists() {
            std::fs::copy(&from_wal, &to_wal).expect("copy wal");
        } else {
            let _ = std::fs::remove_file(&to_wal);
        }
    };

    let work = scratch.path("count.db");
    copy_pair(&pristine, &work);
    let (ok, out) = run_checkpoint_child(&work, 0);
    assert!(ok, "uncrashed checkpoint child failed: {out}");
    let steps: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix("STEPS "))
        .and_then(|v| v.trim().parse().ok())
        .expect("child reported no step count");
    assert!(steps > 3, "expected a multi-step checkpoint, got {steps}");
    eprintln!("checkpoint crash harness: {steps} durable write steps");

    let mut crashed = 0;
    for at in 1..=steps {
        let path = scratch.path(&format!("ckpt-crash-{at}.db"));
        copy_pair(&pristine, &path);
        let (ok, _) = run_checkpoint_child(&path, at);
        if !ok {
            crashed += 1;
        }
        let recovered = recover_and_check(&path, &scratch, &format!("ckpt-{at}"));
        // A checkpoint changes no logical content: every crash point must
        // recover to exactly the pre-checkpoint state.
        assert_eq!(
            recovered, before,
            "crash at checkpoint step {at} changed the database's content"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", path.display())));
    }
    assert!(crashed > 0, "no checkpoint crash point actually aborted the child");
}

#[test]
fn a_hot_journal_is_replayed_by_sqlite_too() {
    if std::env::var("MAKEPAD_SQLITE_CRASH_CHILD").is_ok() || !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("crash-cli");
    let pristine = scratch.path("p.db");
    setup(&pristine);

    // Find a crash point in the middle of writing the database file.
    let work = scratch.path("count.db");
    std::fs::copy(&pristine, &work).unwrap();
    let (_, out) = run_child(&work, 0);
    let steps: u64 = out
        .lines()
        .find_map(|l| l.strip_prefix("STEPS "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(10);

    let mid = (steps / 2).max(2);
    let path: PathBuf = scratch.path("hot.db");
    std::fs::copy(&pristine, &path).unwrap();
    let (ok, _) = run_child(&path, mid);
    assert!(!ok, "the child should have crashed at step {mid}");
    let journal = PathBuf::from(format!("{}-journal", path.display()));
    if journal.exists() {
        // SQLite must roll the journal back by itself and end up consistent.
        let out = sqlite3(&path, "PRAGMA integrity_check;\nSELECT COUNT(*) FROM t;\n");
        let mut lines = out.lines();
        assert_eq!(lines.next().unwrap_or("").trim(), "ok");
        let count: i64 = lines.next().unwrap_or("0").trim().parse().unwrap_or(-1);
        assert!(
            count == ROWS_BEFORE || count == ROWS_BEFORE + ROWS_ADDED - 20,
            "sqlite recovered a mixture: {count} rows"
        );
    }
}
