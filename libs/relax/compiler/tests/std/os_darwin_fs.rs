// relax #89: std::os::darwin::fs (rustc library/std/src/os/darwin/fs.rs): MetadataExt's `stat`
// fields and FileTimesExt::set_created (File::set_times through fsetattrlist); std::os::macos::fs
// re-exports both. tokio's `fs` imports them. `rustc --test` and `makepad test` pass it on Apple
// targets; the file is empty elsewhere.
#![cfg(target_vendor = "apple")]
use std::fs::{self, File, FileTimes};
use std::os::darwin::fs::{FileTimesExt, MetadataExt};
use std::time::{Duration, SystemTime};

#[test]
fn stat_fields_and_creation_time() {
    let dir = std::env::temp_dir().join(format!("relax-darwin-fs-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f");
    fs::write(&path, b"hello").unwrap();
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.st_size(), 5, "st_size");
    assert_eq!(meta.st_mode() & 0o170000, 0o100000, "st_mode: a regular file");
    assert_eq!(meta.st_nlink(), 1, "st_nlink");
    assert_eq!(meta.st_ino(), std::os::unix::fs::MetadataExt::ino(&meta), "st_ino");
    assert_eq!(meta.st_uid(), std::os::unix::fs::MetadataExt::uid(&meta), "st_uid");
    let m = meta.modified().unwrap().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!((meta.st_mtime(), meta.st_mtime_nsec()), (m.as_secs() as i64, m.subsec_nanos() as i64), "st_mtime");
    assert!(meta.st_blksize() > 0, "st_blksize");

    // the creation time and the modification time in one set_times
    let created = SystemTime::UNIX_EPOCH + Duration::new(1_000_000_000, 123_000_000);
    let modified = SystemTime::UNIX_EPOCH + Duration::new(1_500_000_000, 0);
    let f = File::options().write(true).open(&path).unwrap();
    f.set_times(FileTimes::new().set_created(created).set_modified(modified)).unwrap();
    drop(f);
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.created().unwrap(), created, "created");
    assert_eq!(meta.modified().unwrap(), modified, "modified");
    assert_eq!((meta.st_birthtime(), meta.st_birthtime_nsec()), (1_000_000_000, 123_000_000), "st_birthtime");
    assert_eq!(std::os::macos::fs::MetadataExt::st_size(&meta), 5, "std::os::macos::fs re-exports darwin's");
    fs::remove_dir_all(&dir).unwrap();
}
