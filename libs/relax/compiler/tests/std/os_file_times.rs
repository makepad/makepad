// File::set_modified / set_times (futimens), and Path components (Component::Prefix exists; unix
// paths never produce it). "| " lines must equal real std's.
use std::fs::{File, FileTimes};
use std::path::{Component, Path};
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn a01_set_times() {
    let dir = std::env::temp_dir().join(format!("relax-file-times-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("f");
    let f = File::create(&p).unwrap();
    let t = UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_000);
    f.set_modified(t).unwrap();
    let m = std::fs::metadata(&p).unwrap().modified().unwrap();
    println!("| modified {:?}", m.duration_since(UNIX_EPOCH).unwrap());
    let a = UNIX_EPOCH + Duration::new(1_500_000_000, 0);
    f.set_times(FileTimes::new().set_accessed(a)).unwrap();
    let md = std::fs::metadata(&p).unwrap();
    println!("| accessed {:?} modified kept {}", md.accessed().unwrap().duration_since(UNIX_EPOCH).unwrap(), md.modified().unwrap() == m);
    drop(f);
    std::fs::remove_file(&p).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn a02_components() {
    let kinds: Vec<String> = Path::new("/a/./b/../c").components().map(|c| match c {
        Component::Prefix(_) => "prefix".to_string(),
        Component::RootDir => "root".to_string(),
        Component::CurDir => "cur".to_string(),
        Component::ParentDir => "parent".to_string(),
        Component::Normal(s) => s.to_string_lossy().into_owned(),
    }).collect();
    println!("| {:?}", kinds);
}
