use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use makepad_fast_inflate::{deflate_decompress, DecompressError};
use makepad_zip_file::{
    CentralDirectoryFileHeader, EndOfCentralDirectory, COMPRESS_METHOD_DEFLATED,
    COMPRESS_METHOD_UNCOMPRESSED,
};

pub fn unzip_file(
    zip_path: &Path,
    dest: &Path,
    strip_prefix: Option<&str>,
) -> Result<usize, String> {
    let bytes = read(zip_path)?;
    unzip_bytes(&bytes, dest, strip_prefix)
}

pub fn unzip_bytes(bytes: &[u8], dest: &Path, strip_prefix: Option<&str>) -> Result<usize, String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let eocd_at = find_eocd(bytes).ok_or("zip: missing end of central directory")?;
    let mut cur = Cursor::new(&bytes[eocd_at..]);
    let eocd =
        EndOfCentralDirectory::from_stream(&mut cur).map_err(|e| format!("zip eocd: {e:?}"))?;
    let mut cur = Cursor::new(bytes);
    cur.set_position(eocd.central_directory_offset as u64);
    let mut n = 0usize;
    for index in 0..eocd.total_entries_all_disk {
        crate::progress::measured("Unpacking", "Archive entries", index as u64, eocd.total_entries_all_disk as u64, crate::progress::Unit::Files);
        let hdr = CentralDirectoryFileHeader::from_stream(&mut cur)
            .map_err(|e| format!("zip cd: {e:?}"))?;
        if let Some(path) = zip_entry_path(&hdr.file_name, strip_prefix)? {
            if hdr.file_name.ends_with('/') || hdr.file_name.ends_with('\\') {
                fs::create_dir_all(dest.join(&path)).map_err(|e| e.to_string())?;
                continue;
            }
            let mut file_cur = Cursor::new(bytes);
            let mut span = crate::timing::span(crate::timing::Phase::Decompress);
            let data = hdr
                .extract(&mut file_cur)
                .map_err(|e| format!("unzip {}: {e:?}", hdr.file_name))?;
            span.bytes(data.len() as u64);
            drop(span);
            write_file(&dest.join(path), &data)?;
            n += 1;
        }
    }
    crate::progress::measured("Unpacking", "Archive entries", eocd.total_entries_all_disk as u64, eocd.total_entries_all_disk as u64, crate::progress::Unit::Files);
    let _ = (COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED);
    Ok(n)
}

pub fn extract_tar_gz(path: &Path, dest: &Path) -> Result<usize, String> {
    let gz = read(path)?;
    crate::progress::stage("Decompressing", &path.file_name().unwrap_or_default().to_string_lossy(), 0.0);
    let mut span = crate::timing::span(crate::timing::Phase::Decompress);
    let tar = gzip_to_vec(&gz).map_err(|e| format!("gzip {}: {e}", path.display()))?;
    span.bytes(tar.len() as u64);
    drop(span);
    extract_tar(&tar, dest)
}

fn gzip_to_vec(gz: &[u8]) -> Result<Vec<u8>, String> {
    if gz.len() < 18 {
        return Err("gzip too short".into());
    }
    let header = gzip_header_len(gz)?;
    let deflate = gz.get(header..gz.len() - 8).ok_or("gzip missing body")?;
    let isize = u32::from_le_bytes(gz[gz.len() - 4..].try_into().unwrap()) as usize;
    let mut cap = isize.max(deflate.len().saturating_mul(4)).max(1);
    loop {
        let mut out = vec![0u8; cap];
        match deflate_decompress(deflate, &mut out) {
            Ok((_, written)) => {
                out.truncate(written);
                return Ok(out);
            }
            Err(DecompressError::InsufficientSpace) => {
                cap = cap.saturating_mul(2);
                if cap > 2 * 1024 * 1024 * 1024 {
                    return Err("gzip output too large".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn gzip_header_len(gz: &[u8]) -> Result<usize, String> {
    if gz.len() < 10 || gz[0] != 0x1f || gz[1] != 0x8b || gz[2] != 8 {
        return Err("not gzip".into());
    }
    let flg = gz[3];
    let mut pos = 10usize;
    if flg & 4 != 0 {
        if pos + 2 > gz.len() {
            return Err("gzip extra".into());
        }
        let xlen = u16::from_le_bytes([gz[pos], gz[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    if flg & 8 != 0 {
        while pos < gz.len() && gz[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if flg & 16 != 0 {
        while pos < gz.len() && gz[pos] != 0 {
            pos += 1;
        }
        pos += 1;
    }
    if flg & 2 != 0 {
        pos += 2;
    }
    if pos + 8 > gz.len() {
        return Err("gzip header overruns".into());
    }
    Ok(pos)
}

pub fn extract_tar(tar: &[u8], dest: &Path) -> Result<usize, String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut off = 0usize;
    let mut n = 0usize;
    let mut long_name: Option<String> = None;
    while off + 512 <= tar.len() {
        let hdr = &tar[off..off + 512];
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        let size = tar_octal(&hdr[124..136])?;
        let typeflag = hdr[156];
        let mut name = tar_name(hdr)?;
        if let Some(ln) = long_name.take() {
            name = ln;
        }
        let data_off = off + 512;
        let next = data_off + ((size + 511) / 512) * 512;
        let payload = tar.get(data_off..data_off + size).ok_or("tar truncated")?;
        match typeflag {
            b'L' => {
                long_name = Some(
                    String::from_utf8_lossy(payload)
                        .trim_end_matches('\0')
                        .to_string(),
                );
            }
            b'x' | b'g' => {
                if let Some(pax_name) = pax_path(payload) {
                    long_name = Some(pax_name);
                }
            }
            b'5' | b'D' => {
                if let Some(rel) = safe_rel(&name)? {
                    fs::create_dir_all(dest.join(rel)).map_err(|e| e.to_string())?;
                }
            }
            b'0' | b'\0' | b'7' => {
                if let Some(rel) = safe_rel(&name)? {
                    let path = dest.join(rel);
                    write_file(&path, payload)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = tar_octal(&hdr[100..108])? as u32 & 0o777;
                        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                            .map_err(|e| e.to_string())?;
                    }
                    n += 1;
                }
            }
            _ => {}
        }
        off = next.min(tar.len());
        crate::progress::measured("Unpacking", &name, off as u64, tar.len() as u64, crate::progress::Unit::Bytes);
    }
    Ok(n)
}

fn tar_name(hdr: &[u8]) -> Result<String, String> {
    let name = cstr(&hdr[0..100]);
    let prefix = cstr(&hdr[345..500]);
    if prefix.is_empty() {
        Ok(name)
    } else {
        Ok(format!("{prefix}/{name}"))
    }
}

fn tar_octal(bytes: &[u8]) -> Result<usize, String> {
    let s = std::str::from_utf8(bytes)
        .unwrap_or("")
        .trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if s.is_empty() {
        return Ok(0);
    }
    usize::from_str_radix(s, 8).map_err(|_| format!("bad tar size {s}"))
}

fn pax_path(payload: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(payload).ok()?;
    for line in text.split('\n') {
        if let Some(rest) = line.split_once(" path=") {
            let v = rest.1.trim_end_matches('\n').trim_end_matches('\0');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn zip_entry_path(name: &str, strip: Option<&str>) -> Result<Option<String>, String> {
    let mut n = name.replace('\\', "/");
    if let Some(prefix) = strip {
        let p = prefix.trim_end_matches('/');
        if let Some(rest) = n.strip_prefix(p) {
            n = rest.trim_start_matches('/').to_string();
        } else {
            return Ok(None);
        }
    }
    if n.is_empty() {
        return Ok(None);
    }
    safe_rel(&n)
}

fn safe_rel(name: &str) -> Result<Option<String>, String> {
    let n = name.replace('\\', "/");
    if n.is_empty() {
        return Ok(None);
    }
    if n.starts_with('/') || n.contains(':') || n.contains('\0') {
        return Err(format!("refusing archive path {name}"));
    }
    for part in n.split('/') {
        if part == ".." {
            return Err(format!("refusing path {name}"));
        }
    }
    Ok(Some(n))
}

/// A whole file from disk, counted as reading.
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    let mut span = crate::timing::span(crate::timing::Phase::Read);
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    span.bytes(bytes.len() as u64);
    Ok(bytes)
}

pub fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let mut span = crate::timing::span(crate::timing::Phase::Write);
    span.bytes(data.len() as u64);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = fs::File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    Ok(())
}

fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 65535);
    for i in (start..=data.len() - 22).rev() {
        if data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let comment = u16::from_le_bytes([data[i + 20], data[i + 21]]) as usize;
            if i + 22 + comment == data.len() {
                return Some(i);
            }
        }
    }
    None
}

/// Copy directory contents into dest, merging.
pub fn merge_dir(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let to = dest.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            merge_dir(&entry.path(), &to)?;
        } else if ft.is_file() {
            crate::progress::stage("Installing", &entry.file_name().to_string_lossy(), 0.0);
            let mut span = crate::timing::span(crate::timing::Phase::Write);
            span.bytes(fs::copy(entry.path(), &to).map_err(|e| e.to_string())?);
        }
    }
    Ok(())
}

/// If `dir` contains a single subdirectory, return it (rust dist tarball layout).
pub fn single_child_dir(dir: &Path) -> Option<std::path::PathBuf> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        if entry.file_type().ok()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    if dirs.len() == 1 {
        Some(dirs.remove(0))
    } else {
        None
    }
}

// ---- Parallel unpacking ------------------------------------------------------

/// Where parallel unpacking writes. One writer per path at a time; when two
/// payloads carry the same path, the one a one-after-another install would
/// have written last (the higher `order`) wins, whichever finishes first.
#[derive(Default)]
pub struct Claims {
    paths: Mutex<HashMap<String, Arc<Mutex<Option<u64>>>>>,
    dirs: Mutex<HashSet<PathBuf>>,
    overlaps: AtomicU64,
    examples: Mutex<Vec<String>>,
}
impl Claims {
    fn key(path: &Path) -> String {
        let text = path.to_string_lossy();
        if cfg!(windows) { text.replace('/', "\\").to_lowercase() } else { text.into_owned() }
    }
    /// Create `dir` and its parents once per install.
    pub fn ensure_dir(&self, dir: &Path) -> Result<(), String> {
        if self.dirs.lock().unwrap_or_else(|e| e.into_inner()).contains(dir) {
            return Ok(());
        }
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        self.dirs.lock().unwrap_or_else(|e| e.into_inner()).insert(dir.to_path_buf());
        Ok(())
    }
    /// Write `data` to `path` unless a later payload already did. True when written.
    pub fn write(&self, path: &Path, data: &[u8], order: u64) -> Result<bool, String> {
        let entry = self.paths.lock().unwrap_or_else(|e| e.into_inner()).entry(Self::key(path)).or_default().clone();
        let mut last = entry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = *last {
            if self.overlaps.fetch_add(1, Ordering::Relaxed) < 12 {
                let example = format!("{} ({previous:#x}, {order:#x})", crate::shown(path));
                self.examples.lock().unwrap_or_else(|e| e.into_inner()).push(example);
            }
            if previous > order {
                return Ok(false);
            }
        }
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent)?;
        }
        let mut span = crate::timing::span(crate::timing::Phase::Write);
        span.bytes(data.len() as u64);
        fs::write(path, data).map_err(|e| format!("create {}: {e}", path.display()))?;
        *last = Some(order);
        Ok(true)
    }
    /// Paths more than one payload wrote (or skipped for a later one), and
    /// the first few with the payloads' order (component << 32 | payload).
    pub fn overlaps(&self) -> (u64, Vec<String>) {
        (self.overlaps.load(Ordering::Relaxed), self.examples.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
}
/// Write a file of a parallel unpack: through the install's claims when
/// one runs, else directly.
pub fn write_ordered(path: &Path, data: &[u8], order: u64) -> Result<bool, String> {
    match crate::jobs::env() {
        Some(env) => env.claims.write(path, data, order),
        None => write_file(path, data).map(|_| true),
    }
}
fn ensure_dir(dir: &Path) -> Result<(), String> {
    match crate::jobs::env() {
        Some(env) => env.claims.ensure_dir(dir),
        None => fs::create_dir_all(dir).map_err(|e| e.to_string()),
    }
}

/// How archive paths map into the destination.
#[derive(Clone, Debug)]
pub enum Strip {
    /// Keep every path.
    Nothing,
    /// Only entries under this directory, without it ("Contents").
    Prefix(String),
    /// Without the one top directory all entries share, if they do (the
    /// layout of NVIDIA's archives); entries outside it are skipped.
    TopDir,
}
fn strip_path(name: &str, strip: &Strip) -> Result<Option<String>, String> {
    let n = name.replace('\\', "/");
    let rest = match strip {
        Strip::Nothing | Strip::TopDir => n,
        Strip::Prefix(p) => {
            let p = p.trim_end_matches('/');
            match n.strip_prefix(p) {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
                _ => return Ok(None),
            }
        }
    };
    if rest.is_empty() { Ok(None) } else { safe_rel(&rest) }
}
/// The directory every name lies under, when there is exactly one.
fn common_top<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut top: Option<String> = None;
    for name in names {
        let n = name.replace('\\', "/");
        let (first, _) = n.split_once('/')?;
        match &top {
            None => top = Some(first.to_string()),
            Some(t) if t == first => {}
            Some(_) => return None,
        }
    }
    top
}

struct ZipEntry {
    rel: String,
    local: usize,
    compressed: usize,
    size: usize,
    method: u16,
}
/// Unzip `bytes` into `dest` on the unpack pool, entries spread over
/// tasks of a few MB each; `weight` units of the component's bar are
/// shared out among them. Returns the files written.
pub fn unzip_parallel(bytes: Arc<Vec<u8>>, dest: &Path, strip: Strip, order: u64, weight: u64) -> Result<usize, String> {
    let eocd_at = find_eocd(&bytes).ok_or("zip: missing end of central directory")?;
    let mut cur = Cursor::new(&bytes[eocd_at..]);
    let eocd = EndOfCentralDirectory::from_stream(&mut cur).map_err(|e| format!("zip eocd: {e:?}"))?;
    let mut cur = Cursor::new(bytes.as_slice());
    cur.set_position(eocd.central_directory_offset as u64);
    let mut headers = Vec::with_capacity(eocd.total_entries_all_disk as usize);
    for _ in 0..eocd.total_entries_all_disk {
        headers.push(CentralDirectoryFileHeader::from_stream(&mut cur).map_err(|e| format!("zip cd: {e:?}"))?);
    }
    let strip = match strip {
        Strip::TopDir => match common_top(headers.iter().map(|h| h.file_name.as_str())) {
            Some(top) => Strip::Prefix(top),
            None => Strip::Nothing,
        },
        other => other,
    };
    ensure_dir(dest)?;
    let mut entries = Vec::new();
    for h in &headers {
        let Some(rel) = strip_path(&h.file_name, &strip)? else { continue };
        if h.file_name.ends_with('/') || h.file_name.ends_with('\\') {
            ensure_dir(&dest.join(&rel))?;
            continue;
        }
        entries.push(ZipEntry {
            rel,
            local: h.relative_offset_of_local_header as usize,
            compressed: h.compressed_size as usize,
            size: h.uncompressed_size as usize,
            method: h.compression_method,
        });
    }
    let count = entries.len();
    let chunks = chunk_by(entries, |e| e.compressed.max(e.size / 4) + 4096);
    let total: usize = chunks.iter().map(|c| c.1).sum();
    let last = chunks.len().saturating_sub(1);
    let mut given = 0u64;
    let mut pending = Vec::new();
    for (i, (chunk, cost)) in chunks.into_iter().enumerate() {
        let share = if i == last { weight - given } else { (weight as u128 * cost as u128 / total.max(1) as u128) as u64 };
        given += share;
        let (bytes, dest) = (bytes.clone(), dest.to_path_buf());
        pending.push(crate::jobs::spawn(move || {
            let _unpacking = crate::progress::activity(crate::progress::Activity::Unpack);
            crate::progress::step(share);
            for e in &chunk {
                let data = zip_entry(&bytes, e)?;
                write_ordered(&dest.join(&e.rel), &data, order)?;
            }
            crate::progress::step_done();
            Ok(())
        }));
    }
    if pending.is_empty() {
        crate::progress::advance(weight);
    }
    crate::jobs::wait_all(pending)?;
    Ok(count)
}
/// One entry's data, inflated straight from the archive in memory.
fn zip_entry(bytes: &[u8], e: &ZipEntry) -> Result<Vec<u8>, String> {
    let head = bytes.get(e.local..e.local + 30).ok_or("zip: local header out of range")?;
    if head[0..4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err(format!("zip: bad local header for {}", e.rel));
    }
    let name_len = u16::from_le_bytes([head[26], head[27]]) as usize;
    let extra_len = u16::from_le_bytes([head[28], head[29]]) as usize;
    let start = e.local + 30 + name_len + extra_len;
    let data = bytes.get(start..start + e.compressed).ok_or_else(|| format!("zip: {} out of range", e.rel))?;
    let mut span = crate::timing::span(crate::timing::Phase::Decompress);
    span.bytes(e.size as u64);
    match e.method {
        COMPRESS_METHOD_UNCOMPRESSED => Ok(data.to_vec()),
        COMPRESS_METHOD_DEFLATED => {
            let mut out = vec![0u8; e.size];
            match deflate_decompress(data, &mut out) {
                Ok((_, written)) if written == e.size => Ok(out),
                Ok((_, written)) => Err(format!("unzip {}: {written} != {} bytes", e.rel, e.size)),
                Err(err) => Err(format!("unzip {}: {err}", e.rel)),
            }
        }
        other => Err(format!("unzip {}: compression {other} unsupported", e.rel)),
    }
}
/// Split `items` into runs of about 8 MB of `cost` (and at most 256 items).
pub fn chunk_by<T>(items: Vec<T>, cost: impl Fn(&T) -> usize) -> Vec<(Vec<T>, usize)> {
    const TARGET: usize = 8 << 20;
    let mut out = Vec::new();
    let mut chunk = Vec::new();
    let mut sum = 0;
    for item in items {
        let c = cost(&item);
        if !chunk.is_empty() && (sum + c > TARGET || chunk.len() >= 256) {
            out.push((std::mem::take(&mut chunk), sum));
            sum = 0;
        }
        sum += c;
        chunk.push(item);
    }
    if !chunk.is_empty() {
        out.push((chunk, sum));
    }
    out
}

/// A gzip file's contents.
pub fn gunzip(gz: &[u8]) -> Result<Vec<u8>, String> {
    let mut span = crate::timing::span(crate::timing::Phase::Decompress);
    let out = gzip_to_vec(gz)?;
    span.bytes(out.len() as u64);
    Ok(out)
}

pub struct TarEntry {
    pub name: String,
    offset: usize,
    size: usize,
    #[cfg_attr(not(unix), allow(dead_code))]
    mode: u32,
    dir: bool,
}
/// The directories and regular files of a tar archive, in order.
pub fn tar_entries(tar: &[u8]) -> Result<Vec<TarEntry>, String> {
    let mut off = 0usize;
    let mut out = Vec::new();
    let mut long_name: Option<String> = None;
    while off + 512 <= tar.len() {
        let hdr = &tar[off..off + 512];
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        let size = tar_octal(&hdr[124..136])?;
        let typeflag = hdr[156];
        let mut name = tar_name(hdr)?;
        if let Some(ln) = long_name.take() {
            name = ln;
        }
        let data_off = off + 512;
        let next = data_off + size.div_ceil(512) * 512;
        let payload = tar.get(data_off..data_off + size).ok_or("tar truncated")?;
        match typeflag {
            b'L' => long_name = Some(String::from_utf8_lossy(payload).trim_end_matches('\0').to_string()),
            b'x' | b'g' => {
                if let Some(pax_name) = pax_path(payload) {
                    long_name = Some(pax_name);
                }
            }
            b'5' | b'D' => out.push(TarEntry { name, offset: data_off, size: 0, mode: 0, dir: true }),
            b'0' | b'\0' | b'7' => {
                let mode = tar_octal(&hdr[100..108])? as u32 & 0o777;
                out.push(TarEntry { name, offset: data_off, size, mode, dir: false });
            }
            _ => {}
        }
        off = next.min(tar.len());
    }
    Ok(out)
}
/// Write the entries under `prefix` (without it) into `dest` on the unpack
/// pool; `weight` units of the bar are shared out. Returns the files written.
pub fn write_tar_parallel(tar: Arc<Vec<u8>>, entries: Vec<TarEntry>, dest: &Path, prefix: &str, order: u64, weight: u64) -> Result<usize, String> {
    let strip = if prefix.is_empty() { Strip::Nothing } else { Strip::Prefix(prefix.into()) };
    let mut files = Vec::new();
    for e in entries {
        let Some(rel) = strip_path(&e.name, &strip)? else { continue };
        if e.dir {
            ensure_dir(&dest.join(&rel))?;
        } else {
            files.push((rel, e));
        }
    }
    let count = files.len();
    let chunks = chunk_by(files, |(_, e)| e.size + 4096);
    let total: usize = chunks.iter().map(|c| c.1).sum();
    let last = chunks.len().saturating_sub(1);
    let mut given = 0u64;
    let mut pending = Vec::new();
    for (i, (chunk, cost)) in chunks.into_iter().enumerate() {
        let share = if i == last { weight - given } else { (weight as u128 * cost as u128 / total.max(1) as u128) as u64 };
        given += share;
        let (tar, dest) = (tar.clone(), dest.to_path_buf());
        pending.push(crate::jobs::spawn(move || {
            let _writing = crate::progress::activity(crate::progress::Activity::Write);
            crate::progress::step(share);
            for (rel, e) in &chunk {
                let path = dest.join(rel);
                let written = write_ordered(&path, &tar[e.offset..e.offset + e.size], order)?;
                #[cfg(unix)]
                if written {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(e.mode)).map_err(|e| e.to_string())?;
                }
                let _ = written;
            }
            crate::progress::step_done();
            Ok(())
        }));
    }
    if pending.is_empty() {
        crate::progress::advance(weight);
    }
    crate::jobs::wait_all(pending)?;
    Ok(count)
}
