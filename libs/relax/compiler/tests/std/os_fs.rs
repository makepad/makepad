// std-os lane: std::fs (differential: agree.sh). Works in a fresh dir under $TMPDIR. a03:
// fs::hard_link links a symlink itself, dangling or not (relax #107).
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("hr-os-fs-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn a01_files() {
    let d = scratch("files");
    let f = d.join("a.txt");
    fs::write(&f, "hello\nworld\n").unwrap();
    println!("| {:?}", fs::read_to_string(&f).unwrap());
    println!("| {:?}", fs::read(&f).unwrap().len());
    let mut h = OpenOptions::new().append(true).open(&f).unwrap();
    h.write_all(b"more").unwrap();
    drop(h);
    let mut h = File::open(&f).unwrap();
    let mut s = String::new();
    h.read_to_string(&mut s).unwrap();
    println!("| {:?}", s);
    println!("| {:?} {:?}", h.seek(SeekFrom::Start(6)), h.stream_position());
    let mut five = [0u8; 5];
    h.read_exact(&mut five).unwrap();
    println!("| {:?}", std::str::from_utf8(&five));
    let m = h.metadata().unwrap();
    println!("| {} {} {} {} {:o}", m.len(), m.is_file(), m.is_dir(), m.file_type().is_symlink(), m.permissions().mode() & 0o777 & !0o022);
    println!("| {}", m.modified().unwrap() > std::time::UNIX_EPOCH);
    let e = File::open(d.join("missing")).unwrap_err();
    println!("| {:?} {}", e.kind(), e);
    let e = OpenOptions::new().open(&f).unwrap_err();
    println!("| {:?} {:?} {} {:?}", e.kind(), e.raw_os_error(), e, e);
    let e = OpenOptions::new().read(true).truncate(true).open(&f).unwrap_err();
    println!("| {:?} {}", e.kind(), e);
    let e = OpenOptions::new().append(true).truncate(true).open(&f).unwrap_err();
    println!("| {:?} {}", e.kind(), e);
    let e = OpenOptions::new().write(true).create_new(true).open(&f).unwrap_err();
    println!("| {:?}", e.kind());
    let g = d.join("b.bin");
    let mut w = OpenOptions::new().write(true).create(true).mode(0o600).open(&g).unwrap();
    w.write_all(&[1, 2, 3, 4, 5]).unwrap();
    w.set_len(3).unwrap();
    drop(w);
    let gm = fs::metadata(&g).unwrap();
    println!("| {:?} {:o} {}", fs::read(&g).unwrap(), gm.mode() & 0o777, gm.nlink());
    let mut perm = gm.permissions();
    perm.set_readonly(true);
    fs::set_permissions(&g, perm).unwrap();
    println!("| {}", fs::metadata(&g).unwrap().permissions().readonly());
    println!("| {:?}", fs::copy(&f, d.join("c.txt")));
    fs::rename(d.join("c.txt"), d.join("d.txt")).unwrap();
    println!("| {} {}", d.join("c.txt").exists(), d.join("d.txt").is_file());
    fs::remove_file(d.join("d.txt")).unwrap();
    println!("| {:?}", fs::remove_file(d.join("d.txt")).map_err(|e| e.kind()));
    println!("| {:?} {:?}", fs::exists(&f), fs::exists(d.join("nope")));
    fs::remove_dir_all(&d).unwrap();
    println!("| {}", d.exists());
}

#[test]
fn a02_dirs() {
    let d = scratch("dirs");
    fs::create_dir_all(d.join("x/y/z")).unwrap();
    fs::create_dir_all(d.join("x/y/z")).unwrap();
    println!("| {:?}", fs::create_dir(d.join("x")).map_err(|e| e.kind()));
    fs::write(d.join("x/f1"), "1").unwrap();
    fs::write(d.join("x/f2"), "22").unwrap();
    std::os::unix::fs::symlink("f1", d.join("x/link")).unwrap();
    let mut names: Vec<(String, bool, bool)> = fs::read_dir(d.join("x")).unwrap().map(|e| {
        let e = e.unwrap();
        let t = e.file_type().unwrap();
        (e.file_name().into_string().unwrap(), t.is_dir(), t.is_symlink())
    }).collect();
    names.sort();
    println!("| {:?}", names);
    println!("| {:?} {}", fs::read_link(d.join("x/link")), d.join("x/link").is_symlink());
    println!("| {:?}", fs::read_to_string(d.join("x/link")));
    let c = fs::canonicalize(d.join("x/y/../f2")).unwrap();
    println!("| {}", c.ends_with("x/f2"));
    println!("| {:?}", fs::remove_dir(d.join("x")).map_err(|e| e.kind()));
    println!("| {:?}", fs::read_dir(d.join("nope")).map(|_| ()).map_err(|e| e.kind()));
    println!("| {:?}", fs::symlink_metadata(d.join("x/link")).unwrap().file_type().is_symlink());
    fs::remove_dir_all(&d).unwrap();
    println!("| {:?}", fs::metadata(&d).map(|_| ()).map_err(|e| e.kind() == ErrorKind::NotFound));
}

#[test]
fn a03_hard_link() {
    let d = scratch("hard-link");
    let kind = |p: &str| {
        let m = fs::symlink_metadata(d.join(p)).unwrap();
        (m.file_type().is_symlink(), m.is_file(), m.nlink())
    };
    std::os::unix::fs::symlink(d.join("missing"), d.join("dangling")).unwrap();
    println!("| {:?}", fs::hard_link(d.join("dangling"), d.join("second")));
    println!("| {:?} {:?} {}", kind("second"), kind("dangling"), fs::read_link(d.join("second")).unwrap() == d.join("missing"));
    fs::write(d.join("file"), "data").unwrap();
    std::os::unix::fs::symlink("file", d.join("to-file")).unwrap();
    println!("| {:?}", fs::hard_link(d.join("to-file"), d.join("third")));
    println!("| {:?} {:?} {:?}", kind("third"), kind("file"), fs::read_to_string(d.join("third")));
    println!("| {:?}", fs::hard_link(d.join("file"), d.join("fourth")));
    println!("| {:?} {:?}", kind("fourth"), kind("file"));
    println!("| {:?}", fs::hard_link(d.join("file"), d.join("fourth")).map_err(|e| e.kind()));
    println!("| {:?}", fs::hard_link(d.join("nope"), d.join("fifth")).map_err(|e| e.kind()));
    fs::remove_dir_all(&d).unwrap();
}
