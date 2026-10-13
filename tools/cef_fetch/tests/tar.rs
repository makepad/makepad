#[path = "../src/tar.rs"]
mod tar;

use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir();
        loop {
            let dir = base.join(format!("cef-tar-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("{e}"),
            }
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn octal(field: &mut [u8], value: u64) {
    let text = format!("{:0width$o}\0", value, width = field.len() - 1);
    field.copy_from_slice(text.as_bytes());
}
fn checksum(header: &mut [u8]) {
    header[148..156].fill(b' ');
    let sum = header.iter().map(|b| *b as u64).sum();
    octal(&mut header[148..156], sum);
}
fn entry(name: &str, kind: u8, link: &str, body: &[u8], mode: u64) -> Vec<u8> {
    let mut data = vec![0; 512];
    data[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut data[100..108], mode);
    octal(&mut data[124..136], body.len() as u64);
    data[156] = kind;
    data[157..157 + link.len()].copy_from_slice(link.as_bytes());
    data[257..265].copy_from_slice(b"ustar\000");
    checksum(&mut data);
    data.extend_from_slice(body);
    data.resize(data.len().div_ceil(512) * 512, 0);
    data
}
fn archive(mut data: Vec<u8>) -> Vec<u8> { data.extend_from_slice(&[0; 1024]); data }
fn pax(key: &str, value: &str) -> Vec<u8> {
    let record = format!(" {key}={value}\n");
    let mut len = record.len() + 1;
    loop {
        let text = format!("{len}{record}");
        if text.len() == len { return text.into_bytes(); }
        len = text.len();
    }
}

// Force headers, payloads and padding to span short reads, as with a decoder.
struct Chunks<'a>(&'a [u8]);
impl Read for Chunks<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out.len().min(7);
        self.0.read(&mut out[..n])
    }
}

#[test]
fn files_directories_prefixes_modes_and_streaming() {
    let temp = Temp::new();
    let mut data = entry("dir", b'5', "", b"", 0o755);
    let mut file = entry("helper", 0, "", &vec![42; 1300], 0o751);
    file[345..348].copy_from_slice(b"dir");
    checksum(&mut file[..512]);
    data.extend(file);
    let data = archive(data);
    tar::unpack(Chunks(&data), &temp.0).unwrap();
    assert_eq!(fs::read(temp.0.join("dir/helper")).unwrap(), vec![42; 1300]);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(temp.0.join("dir/helper")).unwrap().permissions().mode() & 0o777, 0o751);
    }
}

#[test]
fn gnu_long_names_links_base256_and_old_gnu_prefix_region() {
    let temp = Temp::new();
    let name = format!("dir/{}", "a".repeat(150));
    let mut data = entry("././@LongLink", b'L', "", format!("{name}\0").as_bytes(), 0);
    let mut file = entry("short", b'0', "", b"contents", 0o644);
    file[257..265].copy_from_slice(b"ustar  \0");
    file[345..356].copy_from_slice(b"12345670000");
    file[124..136].fill(0);
    file[124] = 0x80;
    file[135] = 8;
    checksum(&mut file[..512]);
    data.extend(file);
    data.extend(entry("longlink", b'K', "", format!("{name}\0").as_bytes(), 0));
    data.extend(entry("copy", b'1', "wrong", b"", 0o644));
    tar::unpack(archive(data).as_slice(), &temp.0).unwrap();
    assert_eq!(fs::read(temp.0.join(&name)).unwrap(), b"contents");
    assert_eq!(fs::read(temp.0.join("copy")).unwrap(), b"contents");
    fs::write(temp.0.join("copy"), b"changed").unwrap();
    assert_eq!(fs::read(temp.0.join(name)).unwrap(), b"contents");
}

#[test]
fn pax_path_linkpath_size_and_global_ignored() {
    let temp = Temp::new();
    let mut data = entry("global", b'g', "", &pax("path", "ignored"), 0);
    let mut extended = pax("path", "pax/actual");
    extended.extend(pax("size", "700"));
    extended.extend(pax("unknown", "ignored"));
    data.extend(entry("pax", b'x', "", &extended, 0));
    let mut file = entry("wrong", b'0', "", &vec![7; 700], 0o644);
    file[124..136].fill(b'9'); // Pax overrides even an unusable header size.
    checksum(&mut file[..512]);
    data.extend(file);
    data.extend(entry("pax", b'x', "", &pax("linkpath", "pax/actual"), 0));
    data.extend(entry("copy", b'1', "wrong", b"", 0o644));
    data.extend(entry("next", b'0', "", b"next", 0o644));
    tar::unpack(archive(data).as_slice(), &temp.0).unwrap();
    assert_eq!(fs::read(temp.0.join("copy")).unwrap(), vec![7; 700]);
    assert_eq!(fs::read(temp.0.join("next")).unwrap(), b"next");
    assert!(!temp.0.join("ignored").exists());
}

#[test]
fn symlinks() {
    let temp = Temp::new();
    let mut data = entry("Versions/A/file", b'0', "", b"framework", 0o644);
    data.extend(entry("Versions/Current", b'2', "A", b"", 0o777));
    let result = tar::unpack(archive(data).as_slice(), &temp.0);
    #[cfg(unix)] {
        result.unwrap();
        assert_eq!(fs::read_link(temp.0.join("Versions/Current")).unwrap(), PathBuf::from("A"));
        assert_eq!(fs::read(temp.0.join("Versions/Current/file")).unwrap(), b"framework");
    }
    #[cfg(not(unix))]
    assert!(result.unwrap_err().to_string().contains("Versions/Current"));
}

#[test]
fn unknown_types_are_skipped_with_padding() {
    let temp = Temp::new();
    let mut data = entry("unknown", b'Z', "", &vec![1; 513], 0);
    data.extend(entry("file", b'0', "", b"ok", 0o644));
    tar::unpack(archive(data).as_slice(), &temp.0).unwrap();
    assert!(!temp.0.join("unknown").exists());
    assert_eq!(fs::read(temp.0.join("file")).unwrap(), b"ok");
}

#[test]
fn refuses_unsafe_paths_and_link_targets_including_extensions() {
    for unsafe_path in ["/absolute", "../escape", "a/../escape", "C:/escape", "a\\..\\escape"] {
        for kind in [b'0', b'5', b'2', b'1'] {
            let temp = Temp::new();
            let data = archive(entry(unsafe_path, kind, "safe", b"", 0o644));
            assert!(tar::unpack(data.as_slice(), &temp.0).is_err());
        }
        for kind in [b'1', b'2'] {
            let temp = Temp::new();
            assert!(tar::unpack(archive(entry("link", kind, unsafe_path, b"", 0o644)).as_slice(), &temp.0).is_err());
        }
        for (kind, payload) in [(b'x', pax("path", unsafe_path)), (b'L', unsafe_path.as_bytes().to_vec())] {
            let temp = Temp::new();
            let mut data = entry("extension", kind, "", &payload, 0);
            data.extend(entry("safe", b'0', "", b"", 0o644));
            assert!(tar::unpack(archive(data).as_slice(), &temp.0).is_err());
        }
    }
}

#[cfg(unix)]
#[test]
fn never_writes_through_symlinks_or_existing_hardlinks() {
    let temp = Temp::new();
    let outside = Temp::new();
    fs::write(outside.0.join("file"), b"untouched").unwrap();
    std::os::unix::fs::symlink(&outside.0, temp.0.join("outside")).unwrap();
    // An earlier archive link points to a pre-existing escaping symlink.
    let mut data = entry("alias", b'2', "outside", b"", 0o777);
    data.extend(entry("alias/file", b'0', "", b"bad", 0o644));
    assert!(tar::unpack(archive(data).as_slice(), &temp.0).is_err());
    for (name, kind, target) in [("outside/file", b'0', ""), ("outside", b'0', ""), ("copy", b'1', "outside/file")] {
        assert!(tar::unpack(archive(entry(name, kind, target, b"bad", 0o644)).as_slice(), &temp.0).is_err());
    }
    fs::hard_link(outside.0.join("file"), temp.0.join("hard")).unwrap();
    tar::unpack(archive(entry("hard", b'0', "", b"new", 0o644)).as_slice(), &temp.0).unwrap();
    assert_eq!(fs::read(outside.0.join("file")).unwrap(), b"untouched");
}

#[test]
fn corrupt_checksums_numbers_pax_and_truncation_fail() {
    let temp = Temp::new();
    let good = archive(entry("file", b'0', "", b"data", 0o644));
    for len in [0, 100, 512, 514, 1023, 1024, 1536, 2047] {
        assert!(tar::unpack(&good[..len], &temp.0).is_err(), "length {len}");
    }
    let mut bad = good.clone(); bad[0] ^= 1;
    assert!(tar::unpack(bad.as_slice(), &temp.0).unwrap_err().to_string().contains("checksum"));
    for field in [vec![b'9'; 12], vec![0xff; 12], vec![0x81; 12]] {
        let mut bad = good.clone(); bad[124..136].copy_from_slice(&field); checksum(&mut bad[..512]);
        assert!(tar::unpack(bad.as_slice(), &temp.0).is_err());
    }
    for record in [b"0 a=b\n".as_slice(), b"999 a=b\n", b"6 a=b!", b"x a=b\n"] {
        assert!(tar::unpack(archive(entry("pax", b'x', "", record, 0)).as_slice(), &temp.0).is_err());
    }
    let mut bad = good; bad[1536] = 1;
    assert!(tar::unpack(bad.as_slice(), &temp.0).is_err());
}

#[cfg(unix)]
#[test]
fn directory_permissions_are_applied_after_children() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let mut data = entry("dir", b'5', "", b"", 0o555);
    data.extend(entry("dir/sub", b'5', "", b"", 0o555));
    data.extend(entry("dir/sub/file", b'0', "", b"ok", 0o644));
    tar::unpack(archive(data).as_slice(), &temp.0).unwrap();
    assert_eq!(fs::read(temp.0.join("dir/sub/file")).unwrap(), b"ok");
    for name in ["dir", "dir/sub"] {
        let dir = temp.0.join(name);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o555);
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
