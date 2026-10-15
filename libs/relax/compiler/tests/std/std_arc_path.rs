// Arc<Path>/Rc<Path>/Arc<OsStr>/Rc<OsStr> from borrowed and owned values, as real std
// (bzip2-sys's build script: `Arc::<Path>::from(&*path)`). "| " lines must equal real std's (core_diff.sh).
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_arc_path() {
    let a: Arc<Path> = Arc::from(Path::new("/usr/lib/x.so"));
    let b: Arc<Path> = PathBuf::from("rel/dir").into();
    let c = a.clone();
    println!("| {} {} {} {}", a.display(), b.display(), Arc::strong_count(&a), c.extension().unwrap().to_str().unwrap());
    let e: Arc<Path> = Arc::from(Path::new(""));
    println!("| empty {}", e.as_os_str().len());
}

#[test]
fn a02_rc_path_and_os_str() {
    let r: Rc<Path> = Rc::from(Path::new("a/b.txt"));
    let o: Arc<OsStr> = Arc::from(OsStr::new("osstr"));
    let p: Rc<OsStr> = OsString::from("owned").into();
    println!("| {} {} {}", r.file_name().unwrap().to_str().unwrap(), o.to_str().unwrap(), p.to_str().unwrap());
}
