use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use makepad_fast_inflate::{deflate_decompress, DecompressError};
use makepad_zip_file::{
    CentralDirectoryFileHeader, EndOfCentralDirectory, COMPRESS_METHOD_DEFLATED,
    COMPRESS_METHOD_UNCOMPRESSED,
};

pub fn unzip_file(zip_path: &Path, dest: &Path, strip_prefix: Option<&str>) -> Result<usize, String> {
    let bytes = fs::read(zip_path).map_err(|e| format!("read {}: {e}", zip_path.display()))?;
    unzip_bytes(&bytes, dest, strip_prefix)
}

pub fn unzip_bytes(bytes: &[u8], dest: &Path, strip_prefix: Option<&str>) -> Result<usize, String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let eocd_at = find_eocd(bytes).ok_or("zip: missing end of central directory")?;
    let mut cur = Cursor::new(&bytes[eocd_at..]);
    let eocd = EndOfCentralDirectory::from_stream(&mut cur).map_err(|e| format!("zip eocd: {e:?}"))?;
    let mut cur = Cursor::new(bytes);
    cur.set_position(eocd.central_directory_offset as u64);
    let mut n = 0usize;
    for _ in 0..eocd.total_entries_all_disk {
        let hdr = CentralDirectoryFileHeader::from_stream(&mut cur)
            .map_err(|e| format!("zip cd: {e:?}"))?;
        if let Some(path) = zip_entry_path(&hdr.file_name, strip_prefix)? {
            if hdr.file_name.ends_with('/') || hdr.file_name.ends_with('\\') {
                fs::create_dir_all(dest.join(&path)).map_err(|e| e.to_string())?;
                continue;
            }
            let mut file_cur = Cursor::new(bytes);
            let data = hdr
                .extract(&mut file_cur)
                .map_err(|e| format!("unzip {}: {e:?}", hdr.file_name))?;
            write_file(&dest.join(path), &data)?;
            n += 1;
        }
    }
    let _ = (COMPRESS_METHOD_DEFLATED, COMPRESS_METHOD_UNCOMPRESSED);
    Ok(n)
}

pub fn extract_tar_gz(path: &Path, dest: &Path) -> Result<usize, String> {
    let gz = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let tar = gzip_to_vec(&gz).map_err(|e| format!("gzip {}: {e}", path.display()))?;
    extract_tar(&tar, dest)
}

fn gzip_to_vec(gz: &[u8]) -> Result<Vec<u8>, String> {
    if gz.len() < 18 {
        return Err("gzip too short".into());
    }
    let header = gzip_header_len(gz)?;
    let deflate = gz
        .get(header..gz.len() - 8)
        .ok_or("gzip missing body")?;
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
                    write_file(&dest.join(rel), payload)?;
                    n += 1;
                }
            }
            _ => {}
        }
        off = next.min(tar.len());
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
    if n.is_empty() || n.starts_with('/') {
        return Ok(None);
    }
    for part in n.split('/') {
        if part == ".." {
            return Err(format!("refusing path {name}"));
        }
    }
    Ok(Some(n))
}

pub fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
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
            fs::copy(entry.path(), &to).map_err(|e| e.to_string())?;
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
