//! `std::fs::metadata().modified()` holds across two stats of an unchanged file and moves with an
//! edit, a same-size one included: what a file watcher relies on, under rustc's std and the JIT's.
#[test]
fn modified_holds_until_an_edit() {
    let dir = std::env::temp_dir().join(format!("agree-modified-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("watched.txt");
    let at = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let write = |text: &str, secs: u64| {
        std::fs::write(&path, text).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(at(secs)).unwrap();
    };
    let modified = || std::fs::metadata(&path).unwrap().modified().unwrap();
    write("version 1", 1_700_000_000);
    assert_eq!(modified(), at(1_700_000_000), "the file's own time");
    assert_eq!(modified(), at(1_700_000_000), "two stats with no edit agree");
    write("version 2", 1_700_000_002);
    assert_eq!(modified(), at(1_700_000_002), "an edit moves it");
    let _ = std::fs::remove_dir_all(&dir);
}
