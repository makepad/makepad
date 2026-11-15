//! `std::os::unix::fs::{chown, lchown, fchown}`, as real std has them, which failed to compile
//! under the JIT (E0425) while rustc took them. `None` leaves an id as it is, an owner's own ids
//! are accepted, `chown` follows a symlink and `lchown` does not, a missing path is NotFound and
//! a path with a NUL byte InvalidInput. Every call keeps the file's own owner, so it runs unprivileged.
#![cfg(unix)]
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{chown, fchown, lchown, symlink, MetadataExt};
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("relax-chown-{}-{name}", std::process::id()))
}

#[test]
fn chown_lchown_and_fchown_keep_or_set_the_owners_ids() {
    let p = path("file");
    fs::write(&p, b"data").unwrap();
    let m = fs::metadata(&p).unwrap();
    let ids = (m.uid(), m.gid());
    chown(&p, None, None).unwrap();
    chown(&p, Some(ids.0), None).unwrap();
    chown(&p, None, Some(ids.1)).unwrap();
    chown(&p, Some(ids.0), Some(ids.1)).unwrap();
    lchown(&p, Some(ids.0), Some(ids.1)).unwrap();
    fchown(fs::File::open(&p).unwrap(), Some(ids.0), Some(ids.1)).unwrap();
    let m = fs::metadata(&p).unwrap();
    assert_eq!((m.uid(), m.gid()), ids, "an owner's own ids changed the file's owner");
    assert_eq!(fs::read(&p).unwrap(), b"data");

    // a dangling symlink: chown follows it to the missing target, lchown changes the link itself
    let link = path("link");
    symlink(path("missing"), &link).unwrap();
    assert_eq!(chown(&link, None, None).unwrap_err().kind(), ErrorKind::NotFound, "chown did not follow the symlink");
    lchown(&link, None, None).expect("lchown followed the symlink");
    fs::remove_file(&link).unwrap();
    fs::remove_file(&p).unwrap();

    assert_eq!(chown(path("missing"), None, None).unwrap_err().kind(), ErrorKind::NotFound);
    let nul = std::ffi::OsStr::from_bytes(b"bad\0path");
    assert_eq!(chown(nul, None, None).unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(lchown(nul, None, None).unwrap_err().kind(), ErrorKind::InvalidInput);
}
