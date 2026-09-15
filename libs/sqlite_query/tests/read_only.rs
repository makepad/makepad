//! A database that can only be read: shipped in an application bundle, on a
//! read-only volume, or simply not ours to write. `Connection::open_read_only`
//! reads it in place, refuses every write, and leaves no journal, WAL or
//! shared-memory file behind — while the same file, somewhere writable,
//! still opens read-write.

mod common;

use common::Scratch;
use makepad_sqlite::{Connection, Error, Value, READ_ONLY_CONNECTION};
use std::path::Path;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(200);

fn bake(path: &Path) {
    let mut conn = Connection::open(path, TIMEOUT).expect("create the fixture");
    conn.execute_batch(
        "CREATE TABLE items(id INTEGER PRIMARY KEY, title TEXT NOT NULL);
         INSERT INTO items(title) VALUES('first');
         INSERT INTO items(title) VALUES('second');",
    )
    .expect("fill the fixture");
}

fn titles(conn: &mut Connection) -> Vec<String> {
    conn.query("SELECT title FROM items ORDER BY id", &[])
        .expect("read rows")
        .rows
        .iter()
        .map(|row| row[0].as_text().unwrap_or("").to_string())
        .collect()
}

fn sidecars(path: &Path) -> Vec<String> {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let mut found = std::fs::read_dir(path.parent().unwrap())
        .expect("list the directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|entry| entry != &name && entry.starts_with(&name))
        .collect::<Vec<_>>();
    found.sort();
    found
}

#[cfg(unix)]
fn set_read_only(path: &Path, read_only: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if read_only { 0o555 } else { 0o755 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

/// Restores write permission on drop so the scratch directory can be removed
/// even when an assertion fails halfway.
struct WritableAgain<'a>(&'a Path, &'a Path);

impl Drop for WritableAgain<'_> {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            set_read_only(self.0, false);
            set_read_only(self.1, false);
        }
    }
}

#[test]
fn a_read_only_directory_is_read_in_place_and_never_written() {
    let scratch = Scratch::new("read-only");
    let dir = scratch.path("bundle");
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("library.sqlite");
    bake(&db);
    assert!(sidecars(&db).is_empty(), "a closed rollback-journal database has no siblings");

    #[cfg(unix)]
    {
        set_read_only(&db, true);
        set_read_only(&dir, true);
    }
    let _restore = WritableAgain(&dir, &db);

    let mut conn = Connection::open_read_only(&db, TIMEOUT).expect("open read-only in place");
    assert!(conn.is_read_only());
    assert_eq!(titles(&mut conn), ["first", "second"]);

    // Every write path answers the same way, before touching the file.
    for sql in [
        "INSERT INTO items(title) VALUES('third')",
        "UPDATE items SET title = 'x'",
        "DELETE FROM items",
        "CREATE TABLE more(id INTEGER)",
        "CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, title TEXT NOT NULL)",
        "DROP TABLE items",
        "BEGIN IMMEDIATE",
        "PRAGMA user_version = 3",
    ] {
        match conn.execute(sql, &[]) {
            Err(Error::Unsupported(message)) => {
                assert_eq!(message, READ_ONLY_CONNECTION, "{sql}")
            }
            other => panic!("{sql}: expected the read-only refusal, got {other:?}"),
        }
    }
    // Still readable afterwards, still autocommit, nothing left behind.
    assert!(conn.autocommit());
    assert_eq!(titles(&mut conn), ["first", "second"]);
    assert_eq!(sidecars(&db), Vec::<String>::new(), "no journal, WAL or shm was created");

    // A read-write open of the same file is refused by the OS, with the
    // file and the access named.
    #[cfg(unix)]
    {
        let refused = Connection::open(&db, TIMEOUT).err().expect("no write access");
        let text = refused.to_string();
        assert!(text.contains("library.sqlite") && text.contains("writing"), "{text}");
    }

    // The same bytes, somewhere writable, are a normal database.
    let copy = scratch.path("copy.sqlite");
    // The bytes, not the read-only mode: `fs::copy` would carry that over.
    std::fs::write(&copy, std::fs::read(&db).unwrap()).unwrap();
    let mut rw = Connection::open(&copy, TIMEOUT).expect("open the copy read-write");
    assert!(!rw.is_read_only());
    rw.execute("INSERT INTO items(title) VALUES(?)", &[Value::text("third")])
        .expect("write the copy");
    assert_eq!(titles(&mut rw), ["first", "second", "third"]);
}

#[test]
fn a_read_only_open_never_creates_or_accepts_an_empty_file() {
    let scratch = Scratch::new("read-only-missing");
    let missing = scratch.path("missing.sqlite");
    assert!(matches!(Connection::open_read_only(&missing, TIMEOUT), Err(Error::Io(_))));
    assert!(!missing.exists(), "a read-only open must not create the file");

    let empty = scratch.path("empty.sqlite");
    std::fs::write(&empty, b"").unwrap();
    assert!(matches!(Connection::open_read_only(&empty, TIMEOUT), Err(Error::NotADatabase)));
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0, "an empty file is left as it was");
}

#[test]
fn a_read_only_reader_sees_what_a_writer_commits() {
    let scratch = Scratch::new("read-only-live");
    let db = scratch.path("live.sqlite");
    bake(&db);
    let mut reader = Connection::open_read_only(&db, TIMEOUT).unwrap();
    assert_eq!(titles(&mut reader), ["first", "second"]);
    let mut writer = Connection::open(&db, TIMEOUT).unwrap();
    writer
        .execute("INSERT INTO items(title) VALUES('third')", &[])
        .unwrap();
    // Between statements the reader holds no lock, so the writer got in,
    // and the next statement moves to the newest committed snapshot.
    assert_eq!(titles(&mut reader), ["first", "second", "third"]);
    assert_eq!(sidecars(&db), Vec::<String>::new());
}
