//! A tar reader for the archives Makepad ships itself (a toolchain, a source
//! checkout, a prebuilt target tree): ustar and GNU tar, GNU long names and
//! links, pax extended headers for `path`, `linkpath`, `size` and `mtime`.
//! Regular files, directories, symbolic links and hard links are unpacked
//! with their mode bits and modification time. Every entry path, and every
//! link target, is checked to stay inside the destination, so a hostile
//! archive cannot write outside it.
//!
//! The archive is a STREAM: [`unpack_stream`] reads one 512-byte header at
//! a time and copies each file's data through a 1 MB buffer, so an archive
//! of any size unpacks in a few megabytes of memory. An LZ4 frame
//! (makepad-lz4, by its magic) is decoded block by block on the way in. A
//! gzip archive is the exception: makepad-fast-inflate inflates it whole,
//! so it costs its uncompressed size in memory — ship LZ4 for anything big.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

const BLOCK: usize = 512;
/// File data moves through the stream in chunks this big.
const COPY_CHUNK: usize = 1 << 20;
/// A pax header or GNU long name larger than this is not a name.
const MAX_META: u64 = 1 << 20;

#[derive(Debug)]
pub enum TarError {
    /// The archive ends inside a header or an entry's data.
    Truncated,
    /// A header field could not be read.
    BadHeader(&'static str),
    /// The header at this byte offset does not sum to its checksum.
    Checksum { offset: u64 },
    /// An entry or link would leave the destination.
    UnsafePath(String),
    Gzip(String),
    /// The archive source (a file, an asset, an LZ4 frame) failed to read.
    Read(io::Error),
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
            TarError::Read(error) => write!(f, "archive read: {error}"),
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

/// Where a streaming unpack is: reported after every entry and after every
/// chunk of a large file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Entries finished (of every kind, skipped ones included).
    pub entries: usize,
    /// File bytes written so far.
    pub bytes: u64,
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

/// The fields of one header block, before the pending overrides apply.
struct RawHeader {
    typeflag: u8,
    size: u64,
    name: String,
    link: Option<String>,
    mode: u32,
    mtime: Option<u64>,
}

/// The entry a header (plus its pending overrides) describes.
struct Resolved {
    path: PathBuf,
    kind: EntryKind,
    mode: u32,
    mtime: u64,
    link: Option<PathBuf>,
}

fn raw_header(header: &[u8], offset: u64) -> Result<RawHeader, TarError> {
    verify_checksum(header, offset)?;
    let typeflag = header[156];
    let size = numeric(&header[124..136]).ok_or(TarError::BadHeader("size"))?;
    let ustar = &header[257..262] == b"ustar";
    let name = {
        let name = cstr(&header[0..100]);
        let prefix = if ustar { cstr(&header[345..500]) } else { "" };
        if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        }
    };
    let link = Some(cstr(&header[157..257]).to_owned()).filter(|s| !s.is_empty());
    let mode = numeric(&header[100..108]).unwrap_or(0) as u32 & 0o7777;
    let mtime = numeric(&header[136..148]);
    Ok(RawHeader { typeflag, size, name, link, mode, mtime })
}

/// `L`, `K`, `x` and `g` entries carry data for the NEXT entry (or nothing
/// this reader uses); everything else is an entry of its own.
fn is_meta(typeflag: u8) -> bool {
    matches!(typeflag, b'L' | b'K' | b'x' | b'g')
}

fn absorb_meta(typeflag: u8, data: &[u8], pending: &mut Pending) {
    match typeflag {
        b'L' => pending.path = Some(cstr(data).to_owned()),
        b'K' => pending.link = Some(cstr(data).to_owned()),
        b'x' => parse_pax(data, pending),
        _ => {}
    }
}

fn resolve(raw: RawHeader, pending: &mut Pending) -> Resolved {
    let mut name = pending.path.take().unwrap_or(raw.name);
    let link = pending.link.take().or(raw.link).map(PathBuf::from);
    let mtime = pending.mtime.take().or(raw.mtime).unwrap_or(0);
    *pending = Pending::default();
    let mut kind = match raw.typeflag {
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
    Resolved { path: PathBuf::from(name), kind, mode: raw.mode, mtime, link }
}

fn padded(size: u64) -> u64 {
    size.div_ceil(BLOCK as u64) * BLOCK as u64
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
        let raw = raw_header(header, at as u64)?;
        let size = if is_meta(raw.typeflag) { raw.size } else { pending.size.unwrap_or(raw.size) };
        let data_start = at + BLOCK;
        let data_end = data_start
            .checked_add(usize::try_from(size).map_err(|_| TarError::BadHeader("size"))?)
            .ok_or(TarError::BadHeader("size"))?;
        if data_end > archive.len() {
            return Err(TarError::Truncated);
        }
        let padded_end = data_start + padded(size) as usize;
        let data = &archive[data_start..data_end];
        if is_meta(raw.typeflag) {
            absorb_meta(raw.typeflag, data, &mut pending);
            at = padded_end;
            continue;
        }
        let r = resolve(raw, &mut pending);
        entries.push(Entry { path: r.path, kind: r.kind, mode: r.mode, mtime: r.mtime, link: r.link, data });
        at = padded_end;
    }
    Ok(entries)
}

/// Unpack a plain tar archive held in memory below `dest`.
pub fn unpack(archive: &[u8], dest: &Path) -> Result<UnpackReport, TarError> {
    unpack_stream(archive, dest, &mut |_| {})
}

/// Unpack an archive file below `dest`: an LZ4 frame or a plain tar is
/// streamed; a gzip file is inflated whole first.
pub fn unpack_file(archive: &Path, dest: &Path) -> Result<UnpackReport, TarError> {
    let file = fs::File::open(archive).map_err(|e| TarError::Io(archive.to_path_buf(), e))?;
    unpack_stream(io::BufReader::with_capacity(COPY_CHUNK, file), dest, &mut |_| {})
}

/// Unpack archive bytes below `dest` (LZ4 frame, gzip, or plain tar by
/// their magic).
pub fn unpack_bytes(bytes: &[u8], dest: &Path) -> Result<UnpackReport, TarError> {
    unpack_stream(bytes, dest, &mut |_| {})
}

/// Unpack an archive read from `source` below `dest`, creating it as
/// needed. The first bytes decide the codec: an LZ4 frame is decoded as it
/// streams, a gzip stream is read to the end and inflated in memory, and
/// anything else is read as plain tar. `progress` hears about every entry
/// and every chunk of a large file.
pub fn unpack_stream<R: Read>(
    mut source: R,
    dest: &Path,
    progress: &mut dyn FnMut(&Progress),
) -> Result<UnpackReport, TarError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = source.read(&mut head[got..]).map_err(TarError::Read)?;
        if n == 0 {
            break;
        }
        got += n;
    }
    let rest = io::Cursor::new(head[..got].to_vec()).chain(source);
    if got == 4 && head == makepad_lz4::frame::MAGIC.to_le_bytes() {
        let decoder = makepad_lz4::FrameDecoder::new(rest).map_err(TarError::Read)?;
        unpack_tar(decoder, dest, progress)
    } else if got >= 2 && head[..2] == [0x1f, 0x8b] {
        let mut rest = rest;
        let mut bytes = Vec::new();
        rest.read_to_end(&mut bytes).map_err(TarError::Read)?;
        let inflated = makepad_fast_inflate::gzip_decompress_vec(&bytes)
            .map_err(|e| TarError::Gzip(format!("{e:?}")))?;
        unpack_tar(&inflated[..], dest, progress)
    } else {
        unpack_tar(rest, dest, progress)
    }
}

fn read_full<R: Read>(source: &mut R, buf: &mut [u8]) -> Result<usize, TarError> {
    let mut got = 0;
    while got < buf.len() {
        let n = source.read(&mut buf[got..]).map_err(TarError::Read)?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}

fn skip<R: Read>(source: &mut R, mut n: u64, scratch: &mut [u8]) -> Result<(), TarError> {
    while n > 0 {
        let take = (n.min(scratch.len() as u64)) as usize;
        if read_full(source, &mut scratch[..take])? != take {
            return Err(TarError::Truncated);
        }
        n -= take as u64;
    }
    Ok(())
}

/// The streaming core: plain tar from `source`.
fn unpack_tar<R: Read>(
    mut source: R,
    dest: &Path,
    progress: &mut dyn FnMut(&Progress),
) -> Result<UnpackReport, TarError> {
    fs::create_dir_all(dest).map_err(|e| TarError::Io(dest.to_path_buf(), e))?;
    let mut report = UnpackReport::default();
    let mut done = Progress::default();
    // A directory's time is set once its contents are in place, since every
    // file written into it moves it again.
    let mut directory_times: Vec<(PathBuf, u64)> = Vec::new();
    let mut pending = Pending::default();
    let mut header = [0u8; BLOCK];
    let mut buf = vec![0u8; COPY_CHUNK];
    let mut offset = 0u64;
    loop {
        let got = read_full(&mut source, &mut header)?;
        if got == 0 {
            // Some writers end without the zero blocks.
            break;
        }
        if got < BLOCK {
            return Err(TarError::Truncated);
        }
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let raw = raw_header(&header, offset)?;
        offset += BLOCK as u64;
        if is_meta(raw.typeflag) {
            if raw.size > MAX_META {
                return Err(TarError::BadHeader("oversized pax header"));
            }
            let mut data = vec![0u8; raw.size as usize];
            if read_full(&mut source, &mut data)? != data.len() {
                return Err(TarError::Truncated);
            }
            skip(&mut source, padded(raw.size) - raw.size, &mut buf)?;
            offset += padded(raw.size);
            absorb_meta(raw.typeflag, &data, &mut pending);
            continue;
        }
        let size = pending.size.unwrap_or(raw.size);
        let entry = resolve(raw, &mut pending);
        let target = safe_join(dest, &entry.path)?;
        let mut consumed = 0u64;
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
                let mut file = fs::File::create(&target).map_err(|e| TarError::Io(target.clone(), e))?;
                while consumed < size {
                    let take = ((size - consumed).min(buf.len() as u64)) as usize;
                    if read_full(&mut source, &mut buf[..take])? != take {
                        return Err(TarError::Truncated);
                    }
                    file.write_all(&buf[..take]).map_err(|e| TarError::Io(target.clone(), e))?;
                    consumed += take as u64;
                    done.bytes += take as u64;
                    if consumed < size {
                        progress(&done);
                    }
                }
                drop(file);
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
                let source_path = safe_join(dest, link)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| TarError::Io(parent.to_path_buf(), e))?;
                }
                remove_existing(&target)?;
                if fs::hard_link(&source_path, &target).is_err() {
                    fs::copy(&source_path, &target).map_err(|e| TarError::Io(target.clone(), e))?;
                }
                report.hardlinks += 1;
            }
            EntryKind::Other(_) => report.skipped += 1,
        }
        // Whatever the entry did not consume of its data, plus the padding.
        skip(&mut source, padded(size) - consumed, &mut buf)?;
        offset += padded(size);
        done.entries += 1;
        progress(&done);
    }
    // The stream is read to its end: the second zero block and the record
    // padding a writer leaves after the archive, and the codec's trailer
    // behind them. An LZ4 frame ends in an end mark and a content checksum
    // that the decoder only reads, and verifies, when asked for more data;
    // stopping at the first zero block left those 8 bytes unread, so the
    // phone's count of APK bytes came up short of the manifest and the
    // checksum was never checked.
    loop {
        let n = source.read(&mut buf).map_err(TarError::Read)?;
        if n == 0 {
            break;
        }
    }
    // Deepest first, so a parent's time is not moved by a child's.
    directory_times.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));
    for (dir, mtime) in directory_times {
        set_mtime(&dir, mtime)?;
    }
    Ok(report)
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
fn verify_checksum(header: &[u8], offset: u64) -> Result<(), TarError> {
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

    fn sample_archive(long: &str) -> Vec<u8> {
        let mut archive = Vec::new();
        push(&mut archive, header("d/", 0, b'5', 0o755, 1_700_000_000, ""), b"");
        push(&mut archive, header("d/a.txt", 5, b'0', 0o644, 1_700_000_001, ""), b"hello");
        push(&mut archive, header("d/run", 3, b'0', 0o755, 1_700_000_002, ""), b"#!x");
        push(&mut archive, header("d/link", 0, b'2', 0o777, 0, "a.txt"), b"");
        push(&mut archive, header("d/hard", 0, b'1', 0o644, 0, "d/a.txt"), b"");
        push(&mut archive, header("././@LongLink", long.len() as u64 + 1, b'L', 0o644, 0, ""), format!("{long}\0").as_bytes());
        push(&mut archive, header("d/short", 4, b'0', 0o644, 1_700_000_003, ""), b"long");
        let pax = "22 path=d/from-pax.txt\n";
        push(&mut archive, header("./PaxHeaders/x", pax.len() as u64, b'x', 0o644, 0, ""), pax.as_bytes());
        push(&mut archive, header("ignored", 3, b'0', 0o600, 1_700_000_004, ""), b"pax");
        archive.extend(std::iter::repeat(0).take(BLOCK * 2));
        archive
    }

    fn check_sample(dest: &Path, long: &str) {
        assert_eq!(fs::read_to_string(dest.join("d/a.txt")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("d/link")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("d/hard")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join(long)).unwrap(), "long");
        assert_eq!(fs::read_to_string(dest.join("d/from-pax.txt")).unwrap(), "pax");
        let modified = fs::metadata(dest.join("d/a.txt")).unwrap().modified().unwrap();
        assert_eq!(modified.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), 1_700_000_001);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(dest.join("d/run")).unwrap().permissions().mode() & 0o777, 0o755);
            assert!(fs::symlink_metadata(dest.join("d/link")).unwrap().file_type().is_symlink());
        }
    }

    #[test]
    fn files_directories_links_and_long_names_round_trip() {
        let long = format!("d/{}.txt", "n".repeat(150));
        let archive = sample_archive(&long);

        let parsed = entries(&archive).unwrap();
        let names: Vec<_> = parsed.iter().map(|e| e.path.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["d", "d/a.txt", "d/run", "d/link", "d/hard", long.as_str(), "d/from-pax.txt"]);
        assert_eq!(parsed[0].kind, EntryKind::Directory);
        assert_eq!(parsed[3].link.as_deref(), Some(Path::new("a.txt")));

        let dest = scratch("roundtrip");
        let report = unpack(&archive, &dest).unwrap();
        assert_eq!(report, UnpackReport { files: 4, directories: 1, symlinks: 1, hardlinks: 1, skipped: 0 });
        check_sample(&dest, &long);
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

    /// The phone's path: an LZ4 frame streamed through a small-read source
    /// (an asset), files larger than the copy chunk, progress per chunk and
    /// per entry, and the same contents as the in-memory unpack.
    #[test]
    fn lz4_frames_stream_with_progress() {
        let long = format!("d/{}.txt", "n".repeat(150));
        let mut archive = sample_archive(&long);
        archive.truncate(archive.len() - BLOCK * 2);
        let big: Vec<u8> = (0..(COPY_CHUNK * 3 + 12345)).map(|i| (i % 251) as u8).collect();
        push(&mut archive, header("d/big.bin", big.len() as u64, b'0', 0o644, 1_700_000_005, ""), &big);
        // No trailing zero blocks: the writer that stopped short is read too.
        let frame = makepad_lz4::frame_compress(&archive);
        assert!(frame.len() < archive.len());

        struct Dribble<'a>(&'a [u8]);
        impl Read for Dribble<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = buf.len().min(self.0.len()).min(777);
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let dest = scratch("lz4");
        let mut seen = Vec::new();
        let report = unpack_stream(Dribble(&frame), &dest, &mut |p| seen.push(*p)).unwrap();
        assert_eq!(report, UnpackReport { files: 5, directories: 1, symlinks: 1, hardlinks: 1, skipped: 0 });
        check_sample(&dest, &long);
        assert_eq!(fs::read(dest.join("d/big.bin")).unwrap(), big);
        let last = seen.last().unwrap();
        assert_eq!(last.entries, 8);
        assert_eq!(last.bytes, 5 + 3 + 4 + 3 + big.len() as u64);
        // The big file reported three chunks before its entry finished.
        assert!(seen.windows(2).filter(|w| w[0].entries == w[1].entries && w[0].bytes < w[1].bytes).count() >= 3);
        let _ = fs::remove_dir_all(&dest);
    }

    /// The APK's shape: Python's tarfile ends the archive with two zero
    /// blocks and pads to the 10240-byte record, the frame is cut into
    /// stored parts, and the phone chains the parts through one counting
    /// reader whose total is checked against the manifest. Every byte of
    /// the frame is read (the end mark and content checksum too), and a
    /// damaged content checksum is now seen.
    #[test]
    fn lz4_parts_are_consumed_to_the_last_byte() {
        const RECORD: usize = 10240;
        let mut archive = Vec::new();
        let big: Vec<u8> = (0..(COPY_CHUNK + 4321)).map(|i| (i % 253) as u8).collect();
        push(&mut archive, header("p/", 0, b'5', 0o755, 1_700_000_000, ""), b"");
        push(&mut archive, header("p/big.bin", big.len() as u64, b'0', 0o644, 1_700_000_001, ""), &big);
        push(&mut archive, header("p/last.txt", 4, b'0', 0o644, 1_700_000_002, ""), b"last");
        archive.extend(std::iter::repeat(0).take(BLOCK * 2));
        let pad = (RECORD - archive.len() % RECORD) % RECORD;
        archive.extend(std::iter::repeat(0).take(pad));
        let frame = makepad_lz4::frame_compress(&archive);

        struct Parts<'a> {
            parts: Vec<&'a [u8]>,
            next: usize,
            read: std::rc::Rc<std::cell::Cell<u64>>,
        }
        impl Read for Parts<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                loop {
                    let Some(part) = self.parts.get_mut(self.next) else { return Ok(0) };
                    if part.is_empty() {
                        self.next += 1;
                        continue;
                    }
                    let n = buf.len().min(part.len()).min(1000);
                    buf[..n].copy_from_slice(&part[..n]);
                    *part = &part[n..];
                    self.read.set(self.read.get() + n as u64);
                    return Ok(n);
                }
            }
        }
        let read = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let source = Parts { parts: frame.chunks(70_000).collect(), next: 0, read: read.clone() };
        let dest = scratch("lz4-parts");
        let report = unpack_stream(source, &dest, &mut |_| {}).unwrap();
        assert_eq!(report, UnpackReport { files: 2, directories: 1, symlinks: 0, hardlinks: 0, skipped: 0 });
        assert_eq!(fs::read(dest.join("p/big.bin")).unwrap(), big);
        assert_eq!(fs::read_to_string(dest.join("p/last.txt")).unwrap(), "last");
        assert_eq!(read.get(), frame.len() as u64, "every byte of the frame is read");

        // The last byte is the content checksum's: flipping it is an error
        // the unpack reports, not a silently accepted archive.
        let mut damaged = frame.clone();
        *damaged.last_mut().unwrap() ^= 0xff;
        assert!(matches!(unpack_stream(&damaged[..], &dest, &mut |_| {}), Err(TarError::Read(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn truncated_streams_are_reported() {
        let mut archive = Vec::new();
        push(&mut archive, header("x.txt", 2000, b'0', 0o644, 1_700_000_000, ""), &[7u8; 2000]);
        let dest = scratch("truncated");
        // Cut inside the file's data.
        assert!(matches!(unpack_stream(&archive[..BLOCK + 100], &dest, &mut |_| {}), Err(TarError::Truncated)));
        // Cut inside a header.
        assert!(matches!(unpack_stream(&archive[..100], &dest, &mut |_| {}), Err(TarError::Truncated)));
        // A damaged LZ4 frame surfaces as a read error, not a panic.
        let mut frame = makepad_lz4::frame_compress(&archive);
        let n = frame.len();
        frame.truncate(n - 6);
        assert!(matches!(unpack_stream(&frame[..], &dest, &mut |_| {}), Err(TarError::Read(_))));
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
