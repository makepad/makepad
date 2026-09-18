//! A tar reader for the archives Makepad ships itself (a toolchain, a source
//! checkout, a prebuilt target tree): ustar and GNU tar, GNU long names and
//! links, pax extended headers for `path`, `linkpath`, `size` and `mtime`.
//! Regular files, directories, symbolic links and hard links are unpacked
//! with their mode bits and modification time. Every entry path, and every
//! link target, is checked to stay inside the destination, so a hostile
//! archive cannot write outside it. Gzip archives are inflated whole by
//! makepad-fast-inflate; the archive is parsed from memory.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

const BLOCK: usize = 512;

#[derive(Debug)]
pub enum TarError {
    /// The archive ends inside a header or an entry's data.
    Truncated,
    /// A header field could not be read.
    BadHeader(&'static str),
    /// The header at this byte offset does not sum to its checksum.
    Checksum { offset: usize },
    /// An entry or link would leave the destination.
    UnsafePath(String),
    Gzip(String),
    Io(PathBuf, io::Error),
}

impl std::fmt::Display for TarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TarError::Truncated => write!(f, "archive is truncated"),
            TarError::BadHeader(what) => write!(f, "bad tar header: {what}"),
            TarError::Checksum { offset } => write!(f, "tar header checksum mismatch at byte {offset}"),
            TarError::UnsafePath(path) => write!(f, "entry would leave the destination: {path}"),
            TarError::Gzip(error) => write!(f, "gzip: {error}"),
            TarError::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for TarError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Hardlink,
    /// A type this reader does not unpack (devices, fifos, vendor entries).
    Other(u8),
}

/// One archive member, its data borrowed from the archive bytes.
#[derive(Debug)]
pub struct Entry<'a> {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// The permission bits of the header (the low twelve bits).
    pub mode: u32,
    /// Seconds since the Unix epoch.
    pub mtime: u64,
    /// The target of a symbolic or hard link.
    pub link: Option<PathBuf>,
    pub data: &'a [u8],
}

/// What `unpack` wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnpackReport {
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub hardlinks: usize,
    pub skipped: usize,
}

/// Overrides a pax extended header (`x`) or a GNU long-name entry carries
/// for the entry that follows it.
#[derive(Default)]
struct Pending {
    path: Option<String>,
    link: Option<String>,
    size: Option<u64>,
    mtime: Option<u64>,
}

/// Parse every member of a plain (not compressed) tar archive.
pub fn entries(archive: &[u8]) -> Result<Vec<Entry<'_>>, TarError> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    let mut pending = Pending::default();
    while at + BLOCK <= archive.len() {
        let header = &archive[at..at + BLOCK];
        if header.iter().all(|b| *b == 0) {
            // End of archive (two zero blocks by convention; one is enough).
            break;
        }
        verify_checksum(header, at)?;
        let typeflag = header[156];
        let size = numeric(&header[124..136]).ok_or(TarError::BadHeader("size"))?;
        let size = match (typeflag, pending.size) {
            (b'x' | b'g' | b'L' | b'K', _) => size,
            (_, Some(size)) => size,
            _ => size,
        };
        let data_start = at + BLOCK;
        let data_end = data_start
            .checked_add(usize::try_from(size).map_err(|_| TarError::BadHeader("size"))?)
            .ok_or(TarError::BadHeader("size"))?;
        if data_end > archive.len() {
            return Err(TarError::Truncated);
        }
        let padded_end = data_start + ((size as usize + BLOCK - 1) / BLOCK) * BLOCK;
        let data = &archive[data_start..data_end];
        match typeflag {
            b'L' => {
                pending.path = Some(cstr(data).to_owned());
                at = padded_end;
                continue;
            }
            b'K' => {
                pending.link = Some(cstr(data).to_owned());
                at = padded_end;
                continue;
            }
            b'x' => {
                parse_pax(data, &mut pending);
                at = padded_end;
                continue;
            }
            b'g' => {
                // Global pax records (vendor keys): nothing this reader uses.
                at = padded_end;
                continue;
            }
            _ => {}
        }
        let ustar = &header[257..262] == b"ustar";
        let mut name = match pending.path.take() {
            Some(path) => path,
            None => {
                let name = cstr(&header[0..100]);
                let prefix = if ustar { cstr(&header[345..500]) } else { "" };
                if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}/{name}")
                }
            }
        };
        let link = pending
            .link
            .take()
            .or_else(|| Some(cstr(&header[157..257]).to_owned()).filter(|s| !s.is_empty()))
            .map(PathBuf::from);
        let mode = numeric(&header[100..108]).unwrap_or(0) as u32 & 0o7777;
        let mtime = pending
            .mtime
            .take()
            .or_else(|| numeric(&header[136..148]))
            .unwrap_or(0);
        pending = Pending::default();
        let mut kind = match typeflag {
            0 | b'0' | b'7' => EntryKind::File,
            b'5' => EntryKind::Directory,
            b'2' => EntryKind::Symlink,
            b'1' => EntryKind::Hardlink,
            other => EntryKind::Other(other),
        };
        // Pre-ustar archives mark directories with a trailing slash only.
        if kind == EntryKind::File && name.ends_with('/') {
            kind = EntryKind::Directory;
        }
        while name.ends_with('/') {
            name.pop();
        }
        entries.push(Entry {
            path: PathBuf::from(name),
            kind,
            mode,
            mtime,
            link,
            data,
        });
        at = padded_end;
    }
    Ok(entries)
}

/// Unpack a plain tar archive below `dest`, creating it as needed.
pub fn unpack(archive: &[u8], dest: &Path) -> Result<UnpackReport, TarError> {
    fs::create_dir_all(dest).map_err(|e| TarError::Io(dest.to_path_buf(), e))?;
    let mut report = UnpackReport::default();
    // A directory's time is set once its contents are in place, since every
    // file written into it moves it again.
    let mut directory_times: Vec<(PathBuf, u64)> = Vec::new();
    for entry in entries(archive)? {
        let target = safe_join(dest, &entry.path)?;
        match entry.kind {
            EntryKind::Directory => {
                fs::create_dir_all(&target).map_err(|e| TarError::Io(target.clone(), e))?;
                set_mode(&target, entry.mode)?;
                directory_times.push((target, entry.mtime));
                report.directories += 1;
            }
            EntryKind::File => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| TarError::Io(parent.to_path_buf(), e))?;
                }
                // A stale symlink or directory in the way is not written through.
                remove_existing(&target)?;
                fs::write(&target, entry.data).map_err(|e| TarError::Io(target.clone(), e))?;
                set_mode(&target, entry.mode)?;
                set_mtime(&target, entry.mtime)?;
                report.files += 1;
            }
            EntryKind::Symlink => {
                let link = entry
                    .link
                    .as_deref()
                    .ok_or_else(|| TarError::BadHeader("symlink without a target"))?;
                check_link_stays_inside(dest, &entry.path, link)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| TarError::Io(parent.to_path_buf(), e))?;
                }
                remove_existing(&target)?;
                make_symlink(link, &target)?;
                report.symlinks += 1;
            }
            EntryKind::Hardlink => {
                let link = entry
                    .link
                    .as_deref()
                    .ok_or_else(|| TarError::BadHeader("hard link without a target"))?;
                let source = safe_join(dest, link)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| TarError::Io(parent.to_path_buf(), e))?;
                }
                remove_existing(&target)?;
                if fs::hard_link(&source, &target).is_err() {
                    fs::copy(&source, &target).map_err(|e| TarError::Io(target.clone(), e))?;
                }
                report.hardlinks += 1;
            }
            EntryKind::Other(_) => report.skipped += 1,
        }
    }
    // Deepest first, so a parent's time is not moved by a child's.
    directory_times.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));
    for (dir, mtime) in directory_times {
        set_mtime(&dir, mtime)?;
    }
    Ok(report)
}

/// Unpack an archive file below `dest`. A gzip file (by its magic) is
/// inflated first; anything else is read as a plain tar archive.
pub fn unpack_file(archive: &Path, dest: &Path) -> Result<UnpackReport, TarError> {
    let bytes = fs::read(archive).map_err(|e| TarError::Io(archive.to_path_buf(), e))?;
    unpack_bytes(&bytes, dest)
}

/// Unpack archive bytes below `dest`, inflating gzip first (by its magic).
pub fn unpack_bytes(bytes: &[u8], dest: &Path) -> Result<UnpackReport, TarError> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let inflated = makepad_fast_inflate::gzip_decompress_vec(bytes)
            .map_err(|e| TarError::Gzip(format!("{e:?}")))?;
        unpack(&inflated, dest)
    } else {
        unpack(bytes, dest)
    }
}

/// `dest/relative`, refusing anything that is absolute, empty, or climbs.
fn safe_join(dest: &Path, relative: &Path) -> Result<PathBuf, TarError> {
    let mut out = dest.to_path_buf();
    let mut depth = 0usize;
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                depth += 1;
            }
            Component::CurDir => {}
            _ => return Err(TarError::UnsafePath(relative.display().to_string())),
        }
    }
    if depth == 0 {
        return Err(TarError::UnsafePath(relative.display().to_string()));
    }
    Ok(out)
}

/// A symlink is created only when its target, resolved lexically from the
/// link's own directory, stays inside the destination. Later entries are
/// then written inside the destination too, even through the link.
fn check_link_stays_inside(_dest: &Path, link_path: &Path, target: &Path) -> Result<(), TarError> {
    if target.is_absolute() {
        return Err(TarError::UnsafePath(format!("{} -> {}", link_path.display(), target.display())));
    }
    let mut depth: isize = link_path.components().filter(|c| matches!(c, Component::Normal(_))).count() as isize - 1;
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(TarError::UnsafePath(format!("{} -> {}", link_path.display(), target.display())));
                }
            }
            Component::CurDir => {}
            _ => return Err(TarError::UnsafePath(format!("{} -> {}", link_path.display(), target.display()))),
        }
    }
    Ok(())
}

fn remove_existing(path: &Path) -> Result<(), TarError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path).map_err(|e| TarError::Io(path.to_path_buf(), e)),
        Ok(_) => fs::remove_file(path).map_err(|e| TarError::Io(path.to_path_buf(), e)),
        Err(_) => Ok(()),
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, at: &Path) -> Result<(), TarError> {
    std::os::unix::fs::symlink(target, at).map_err(|e| TarError::Io(at.to_path_buf(), e))
}

#[cfg(not(unix))]
fn make_symlink(target: &Path, at: &Path) -> Result<(), TarError> {
    // Without symlinks the archive's link is recorded as a small text file,
    // so a listing still shows where it pointed.
    fs::write(at, target.to_string_lossy().as_bytes()).map_err(|e| TarError::Io(at.to_path_buf(), e))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), TarError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = mode & 0o777;
    if mode == 0 {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| TarError::Io(path.to_path_buf(), e))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), TarError> {
    Ok(())
}

fn set_mtime(path: &Path, mtime: u64) -> Result<(), TarError> {
    if mtime == 0 {
        return Ok(());
    }
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime);
    let file = fs::File::open(path).map_err(|e| TarError::Io(path.to_path_buf(), e))?;
    file.set_modified(time).map_err(|e| TarError::Io(path.to_path_buf(), e))
}

/// Header bytes sum to the checksum field with that field read as spaces.
/// Old writers summed signed bytes; both sums are accepted.
fn verify_checksum(header: &[u8], offset: usize) -> Result<(), TarError> {
    let recorded = numeric(&header[148..156]).ok_or(TarError::BadHeader("checksum"))?;
    let mut unsigned: u64 = 0;
    let mut signed: i64 = 0;
    for (i, byte) in header.iter().enumerate() {
        let byte = if (148..156).contains(&i) { b' ' } else { *byte };
        unsigned += byte as u64;
        signed += byte as i8 as i64;
    }
    if recorded == unsigned || recorded as i64 == signed {
        Ok(())
    } else {
        Err(TarError::Checksum { offset })
    }
}

/// An octal field (digits, then a space or NUL) or a base-256 field (the
/// first byte's high bit set, the value big-endian in the remaining bits).
fn numeric(field: &[u8]) -> Option<u64> {
    let first = *field.first()?;
    if first & 0x80 != 0 {
        let mut value: u64 = (first & 0x7f) as u64;
        for byte in &field[1..] {
            value = value.checked_shl(8)? | *byte as u64;
        }
        return Some(value);
    }
    let mut value: u64 = 0;
    let mut seen = false;
    for byte in field {
        match byte {
            b'0'..=b'7' => {
                value = value.checked_mul(8)?.checked_add((byte - b'0') as u64)?;
                seen = true;
            }
            b' ' | 0 if !seen => {}
            b' ' | 0 => break,
            _ => return None,
        }
    }
    seen.then_some(value)
}

fn cstr(field: &[u8]) -> &str {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).unwrap_or("")
}

/// pax records are `<length> <key>=<value>\n`, the length covering the
/// whole record. Unknown keys are ignored.
fn parse_pax(data: &[u8], pending: &mut Pending) {
    let mut at = 0;
    while at < data.len() {
        let rest = &data[at..];
        let Some(space) = rest.iter().position(|b| *b == b' ') else { break };
        let Ok(length) = std::str::from_utf8(&rest[..space]).unwrap_or("").parse::<usize>() else { break };
        if length == 0 || length > rest.len() {
            break;
        }
        let record = &rest[space + 1..length];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(eq) = record.iter().position(|b| *b == b'=') {
            let key = std::str::from_utf8(&record[..eq]).unwrap_or("");
            let value = std::str::from_utf8(&record[eq + 1..]).unwrap_or("");
            match key {
                "path" => pending.path = Some(value.to_owned()),
                "linkpath" => pending.link = Some(value.to_owned()),
                "size" => pending.size = value.parse().ok(),
                "mtime" => pending.mtime = value.split('.').next().and_then(|s| s.parse().ok()),
                _ => {}
            }
        }
        at += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, size: u64, typeflag: u8, mode: u32, mtime: u64, link: &str) -> [u8; BLOCK] {
        let mut h = [0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(format!("{mode:07o}").as_bytes());
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[136..147].copy_from_slice(format!("{mtime:011o}").as_bytes());
        h[156] = typeflag;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u64 = h.iter().enumerate().map(|(i, b)| if (148..156).contains(&i) { 32 } else { *b as u64 }).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        h[155] = b' ';
        h
    }

    fn push(archive: &mut Vec<u8>, header: [u8; BLOCK], data: &[u8]) {
        archive.extend_from_slice(&header);
        archive.extend_from_slice(data);
        let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
        archive.extend(std::iter::repeat(0).take(pad));
    }

    fn scratch(tag: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("makepad-tar-{tag}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn files_directories_links_and_long_names_round_trip() {
        let mut archive = Vec::new();
        push(&mut archive, header("d/", 0, b'5', 0o755, 1_700_000_000, ""), b"");
        push(&mut archive, header("d/a.txt", 5, b'0', 0o644, 1_700_000_001, ""), b"hello");
        push(&mut archive, header("d/run", 3, b'0', 0o755, 1_700_000_002, ""), b"#!x");
        push(&mut archive, header("d/link", 0, b'2', 0o777, 0, "a.txt"), b"");
        push(&mut archive, header("d/hard", 0, b'1', 0o644, 0, "d/a.txt"), b"");
        let long = format!("d/{}.txt", "n".repeat(150));
        push(&mut archive, header("././@LongLink", long.len() as u64 + 1, b'L', 0o644, 0, ""), format!("{long}\0").as_bytes());
        push(&mut archive, header("d/short", 4, b'0', 0o644, 1_700_000_003, ""), b"long");
        let pax = "22 path=d/from-pax.txt\n";
        push(&mut archive, header("./PaxHeaders/x", pax.len() as u64, b'x', 0o644, 0, ""), pax.as_bytes());
        push(&mut archive, header("ignored", 3, b'0', 0o600, 1_700_000_004, ""), b"pax");
        archive.extend(std::iter::repeat(0).take(BLOCK * 2));

        let parsed = entries(&archive).unwrap();
        let names: Vec<_> = parsed.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["d", "d/a.txt", "d/run", "d/link", "d/hard", long.as_str(), "d/from-pax.txt"]);
        assert_eq!(parsed[0].kind, EntryKind::Directory);
        assert_eq!(parsed[3].link.as_deref(), Some(Path::new("a.txt")));

        let dest = scratch("roundtrip");
        let report = unpack(&archive, &dest).unwrap();
        assert_eq!(report, UnpackReport { files: 4, directories: 1, symlinks: 1, hardlinks: 1, skipped: 0 });
        assert_eq!(fs::read_to_string(dest.join("d/a.txt")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("d/link")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("d/hard")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join(&long)).unwrap(), "long");
        assert_eq!(fs::read_to_string(dest.join("d/from-pax.txt")).unwrap(), "pax");
        let modified = fs::metadata(dest.join("d/a.txt")).unwrap().modified().unwrap();
        assert_eq!(modified.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), 1_700_000_001);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(dest.join("d/run")).unwrap().permissions().mode() & 0o777, 0o755);
            assert!(fs::symlink_metadata(dest.join("d/link")).unwrap().file_type().is_symlink());
        }
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn gzip_archives_inflate_first() {
        let mut archive = Vec::new();
        push(&mut archive, header("x.txt", 2, b'0', 0o644, 1_700_000_000, ""), b"ok");
        archive.extend(std::iter::repeat(0).take(BLOCK * 2));
        let gz = makepad_fast_inflate::gzip_compress(&archive, 6);
        let dest = scratch("gzip");
        let report = unpack_bytes(&gz, &dest).unwrap();
        assert_eq!(report.files, 1);
        assert_eq!(fs::read_to_string(dest.join("x.txt")).unwrap(), "ok");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn climbing_paths_and_links_are_refused() {
        let dest = scratch("unsafe");
        for (name, typeflag, link) in [("../escape", b'0', ""), ("/abs", b'0', ""), ("d/link", b'2', "../../outside"), ("d/abs", b'2', "/etc/passwd")] {
            let mut archive = Vec::new();
            push(&mut archive, header(name, 0, typeflag, 0o644, 0, link), b"");
            archive.extend(std::iter::repeat(0).take(BLOCK * 2));
            assert!(matches!(unpack(&archive, &dest), Err(TarError::UnsafePath(_))), "{name}");
        }
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn numeric_fields_read_octal_and_base256() {
        assert_eq!(numeric(b"0000644\0"), Some(0o644));
        assert_eq!(numeric(b"   644 "), Some(0o644));
        assert_eq!(numeric(b"\0\0\0\0\0\0\0\0"), None);
        // Base-256: the flag byte, then the value big-endian in the rest.
        let mut big = [0u8; 12];
        big[0] = 0x80;
        big[7] = 0x01;
        assert_eq!(numeric(&big), Some(0x1_0000_0000));
        let mut archive = Vec::new();
        let mut h = header("bad", 1, b'0', 0o644, 0, "");
        h[148] = b'7';
        push(&mut archive, h, b"x");
        assert!(matches!(entries(&archive), Err(TarError::Checksum { offset: 0 })));
    }
}
