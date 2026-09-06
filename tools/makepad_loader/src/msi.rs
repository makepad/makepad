//! Enough of the Microsoft Compound File + MSI table format to unpack
//! Windows SDK MSIs onto a directory tree. No msiexec.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cab;
use crate::extract;

const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const TABLE_PREFIX: char = '\u{4840}';

pub fn dump(msi: &[u8]) -> Result<(), String> {
    let cfb = Cfb::open(msi)?;
    println!(
        "cfb sector_size={} streams={}",
        cfb.sector_size,
        cfb.dir.iter().filter(|d| d.kind == 2).count()
    );
    for d in &cfb.dir {
        if d.kind == 2 {
            println!("  stream {} kind={} start={} size={}", d.name, d.kind, d.start, d.size);
        }
    }
    let (strings, long_str) = read_string_pool(&cfb)?;
    println!("string pool {} entries long_str={long_str}", strings.len().saturating_sub(1));
    let schemas = read_columns_schema(&cfb, &strings, long_str)?;
    let mut tables: Vec<_> = schemas.keys().cloned().collect();
    tables.sort();
    println!("tables: {}", tables.join(", "));
    if let Ok(media) = read_table(&cfb, &strings, &schemas, "Media", long_str) {
        println!("Media ({} rows):", media.len());
        for row in &media {
            println!("  {}", row.join(" | "));
        }
    }
    let dirs = if let Ok(dir_rows) = read_table(&cfb, &strings, &schemas, "Directory", long_str) {
        build_directories(&dir_rows)
    } else {
        HashMap::new()
    };
    let mut comp_dir: HashMap<String, PathBuf> = HashMap::new();
    if let Ok(comp_rows) = read_table(&cfb, &strings, &schemas, "Component", long_str) {
        for row in &comp_rows {
            let id = row.get(0).cloned().unwrap_or_default();
            let dir_id = row.get(2).cloned().unwrap_or_default();
            comp_dir.insert(id, dirs.get(&dir_id).cloned().unwrap_or_default());
        }
    }
    if let Ok(files) = read_table(&cfb, &strings, &schemas, "File", long_str) {
        println!("File rows={}", files.len());
        let mut hits = 0usize;
        for row in &files {
            let name = pretty_name(row.get(2).map(String::as_str).unwrap_or(""));
            let lname = name.to_ascii_lowercase();
            if lname.contains("kernel32")
                || lname.contains("ucrt.lib")
                || lname.contains("d3d11.lib")
                || lname.contains("user32.lib")
            {
                let component = row.get(1).cloned().unwrap_or_default();
                let rel = comp_dir.get(&component).cloned().unwrap_or_default().join(name);
                println!("  {} -> {}", row.join(" | "), rel.display());
                hits += 1;
            }
        }
        println!("interesting File hits={hits}");
        if hits == 0 {
            for row in files.iter().take(15) {
                println!("  sample {}", row.join(" | "));
            }
        }
    }
    Ok(())
}

pub fn unpack_msi(
    msi: &[u8],
    cabs: &HashMap<String, Vec<u8>>,
    dest: &Path,
) -> Result<usize, String> {
    let cfb = Cfb::open(msi)?;
    let (strings, long_str) = read_string_pool(&cfb)?;
    let schemas = read_columns_schema(&cfb, &strings, long_str)?;
    let file_rows = read_table(&cfb, &strings, &schemas, "File", long_str)?;
    let dir_rows = read_table(&cfb, &strings, &schemas, "Directory", long_str)?;
    let comp_rows = read_table(&cfb, &strings, &schemas, "Component", long_str)?;
    let media_rows = read_table(&cfb, &strings, &schemas, "Media", long_str).unwrap_or_default();

    let dirs = build_directories(&dir_rows);
    let mut comp_dir: HashMap<String, PathBuf> = HashMap::new();
    for row in &comp_rows {
        let id = row.get(0).cloned().unwrap_or_default();
        let dir_id = row.get(2).cloned().unwrap_or_default();
        comp_dir.insert(id, dirs.get(&dir_id).cloned().unwrap_or_default());
    }

    let mut needed: HashSet<String> = HashSet::new();
    let mut interesting: Vec<(String, String, u32, PathBuf)> = Vec::new();
    for row in &file_rows {
        let file_id = row.first().cloned().unwrap_or_default();
        let component = row.get(1).cloned().unwrap_or_default();
        let file_name = pretty_name(row.get(2).map(String::as_str).unwrap_or("")).to_string();
        if skip_sdk_file(&file_name) {
            continue;
        }
        let seq = row.get(7).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let dir = comp_dir.get(&component).cloned().unwrap_or_default();
        if file_name.to_ascii_lowercase().ends_with(".lib")
            && (file_name.eq_ignore_ascii_case("kernel32.lib")
                || file_name.eq_ignore_ascii_case("ucrt.lib")
                || file_name.eq_ignore_ascii_case("d3d11.lib")
                || file_name.eq_ignore_ascii_case("user32.lib")
                || file_name.eq_ignore_ascii_case("vcruntime.lib"))
        {
            interesting.push((file_id.clone(), file_name.clone(), seq, dir.join(&file_name)));
        }
        if !file_id.is_empty() {
            needed.insert(file_id.to_ascii_lowercase());
        }
    }

    if !media_rows.is_empty() {
        println!("    Media cabinets:");
        for row in &media_rows {
            let last = row.get(1).cloned().unwrap_or_default();
            let cab_name = row.get(3).cloned().unwrap_or_default();
            println!("      last={last} cab={cab_name}");
        }
    }
    for (id, name, seq, rel) in &interesting {
        println!("    {name} id={id} seq={seq} -> {}", rel.display());
    }

    let extracted = extract_needed_cabs(cabs, &needed)?;

    for (id, name, _, _) in &interesting {
        match extracted.get(&id.to_ascii_lowercase()) {
            Some(_) => println!("    {name} found in cab as {id}"),
            None => println!("    {name} id={id} NOT in any cab"),
        }
    }

    let mut written = 0usize;
    let mut missing = 0usize;
    for row in &file_rows {
        let file_id = row.first().cloned().unwrap_or_default();
        let component = row.get(1).cloned().unwrap_or_default();
        let file_name = pretty_name(row.get(2).map(String::as_str).unwrap_or(""));
        if skip_sdk_file(file_name) {
            continue;
        }
        let dir = comp_dir.get(&component).cloned().unwrap_or_default();
        let rel = dir.join(file_name);
        let Some(data) = extracted.get(&file_id.to_ascii_lowercase()) else {
            missing += 1;
            continue;
        };
        extract::write_file(&dest.join(&rel), data)?;
        written += 1;
    }
    if missing > 0 {
        println!("    {missing} File rows had no cab payload");
    }
    Ok(written)
}

fn extract_needed_cabs(
    cabs: &HashMap<String, Vec<u8>>,
    needed: &HashSet<String>,
) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut extracted: HashMap<String, Vec<u8>> = HashMap::new();
    let mut seen_len_name: HashSet<(usize, String)> = HashSet::new();
    for (name, bytes) in cabs {
        let key = name.to_ascii_lowercase();
        if !seen_len_name.insert((bytes.len(), key.clone())) {
            continue;
        }
        let members = match cab::list(bytes) {
            Ok(m) => m,
            Err(e) => {
                println!("    WARN cab list {name}: {e}");
                continue;
            }
        };
        let hits: Vec<_> = members
            .iter()
            .filter(|m| needed.contains(&m.to_ascii_lowercase()))
            .cloned()
            .collect();
        if hits.is_empty() {
            continue;
        }
        println!(
            "    cab {name} contains {} needed file(s) e.g. {}",
            hits.len(),
            hits.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
        match cab::extract(bytes) {
            Ok(files) => {
                for (fname, data) in files {
                    extracted.insert(fname.to_ascii_lowercase(), data);
                }
            }
            Err(e) => println!("    WARN cab extract {name}: {e}"),
        }
    }
    Ok(extracted)
}

fn skip_sdk_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "libucrt.lib"
        || n == "libucrtd.lib"
        || n.starts_with("libucrt")
        || n.ends_with(".pdb")
}

fn pretty_name(raw: &str) -> &str {
    raw.split('|').last().unwrap_or(raw)
}

fn build_directories(rows: &[Vec<String>]) -> HashMap<String, PathBuf> {
    let mut by_id: HashMap<String, (Option<String>, String)> = HashMap::new();
    for row in rows {
        let id = row.first().cloned().unwrap_or_default();
        let parent = row.get(1).cloned().filter(|s| !s.is_empty());
        let name = pretty_name(row.get(2).map(String::as_str).unwrap_or("")).to_string();
        by_id.insert(id, (parent, name));
    }
    let mut out = HashMap::new();
    for id in by_id.keys() {
        let path = resolve_dir(&by_id, id);
        out.insert(id.clone(), path);
    }
    out
}

fn resolve_dir(map: &HashMap<String, (Option<String>, String)>, id: &str) -> PathBuf {
    let mut parts = Vec::new();
    let mut cur = Some(id.to_string());
    let mut guard = 0;
    while let Some(c) = cur {
        if guard > 64 {
            break;
        }
        guard += 1;
        if let Some((parent, name)) = map.get(&c) {
            if !name.is_empty()
                && name != "."
                && name != "SourceDir"
                && name != "TARGETDIR"
                && name != "ProgramFilesFolder"
                && name != "ProgramFiles64Folder"
                && name != "ProgramFiles128Folder"
            {
                parts.push(name.clone());
            }
            cur = parent.clone();
        } else {
            break;
        }
    }
    parts.reverse();
    let mut p = PathBuf::new();
    for part in parts {
        p.push(part);
    }
    p
}

struct Cfb<'a> {
    data: &'a [u8],
    sector_size: usize,
    fat: Vec<u32>,
    dir: Vec<DirEntry>,
    mini_fat: Vec<u32>,
    mini_cutoff: usize,
    mini_stream: Vec<u8>,
}

#[derive(Clone)]
struct DirEntry {
    name: String,
    kind: u8,
    start: u32,
    size: u64,
}

impl<'a> Cfb<'a> {
    fn open(data: &'a [u8]) -> Result<Self, String> {
        if data.len() < 512 || data[0..8] != MAGIC {
            return Err("not a compound file (msi)".into());
        }
        let shift = u16::from_le_bytes([data[0x1E], data[0x1F]]) as usize;
        let sector_size = 1usize << shift;
        let fat_count = u32::from_le_bytes(data[0x2C..0x30].try_into().unwrap()) as usize;
        let dir_start = u32::from_le_bytes(data[0x30..0x34].try_into().unwrap());
        let mini_cutoff = u32::from_le_bytes(data[0x38..0x3C].try_into().unwrap()) as usize;
        let mini_fat_start = u32::from_le_bytes(data[0x3C..0x40].try_into().unwrap());
        let mini_fat_count = u32::from_le_bytes(data[0x40..0x44].try_into().unwrap());
        let difat_start = u32::from_le_bytes(data[0x44..0x48].try_into().unwrap());
        let difat_count = u32::from_le_bytes(data[0x48..0x4C].try_into().unwrap());

        let mut fat_sectors = Vec::new();
        for i in 0..109 {
            if fat_sectors.len() >= fat_count {
                break;
            }
            let s = u32::from_le_bytes(data[0x4C + i * 4..0x50 + i * 4].try_into().unwrap());
            if s < 0xFFFFFFFA {
                fat_sectors.push(s);
            }
        }
        let mut difat = difat_start;
        for _ in 0..difat_count {
            if fat_sectors.len() >= fat_count {
                break;
            }
            if difat >= 0xFFFFFFFA {
                break;
            }
            let sec = sector(data, sector_size, difat)?;
            let n = (sector_size / 4).saturating_sub(1);
            for i in 0..n {
                if fat_sectors.len() >= fat_count {
                    break;
                }
                let s = u32::from_le_bytes(sec[i * 4..i * 4 + 4].try_into().unwrap());
                if s < 0xFFFFFFFA {
                    fat_sectors.push(s);
                }
            }
            difat = u32::from_le_bytes(sec[n * 4..n * 4 + 4].try_into().unwrap());
        }

        let mut fat = Vec::new();
        for s in fat_sectors {
            let sec = sector(data, sector_size, s)?;
            for chunk in sec.chunks_exact(4) {
                fat.push(u32::from_le_bytes(chunk.try_into().unwrap()));
            }
        }

        let dir_bytes = read_chain(data, sector_size, &fat, dir_start)?;
        let mut dir = Vec::new();
        for chunk in dir_bytes.chunks(128) {
            if chunk.len() < 128 {
                break;
            }
            let kind = chunk[66];
            if kind == 0 {
                continue;
            }
            let name_len = u16::from_le_bytes([chunk[64], chunk[65]]) as usize;
            let name_bytes = &chunk[..name_len.min(64).saturating_sub(2).min(64)];
            let raw_name = String::from_utf16_lossy(
                &name_bytes
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            );
            let name = msi_decode_name(&raw_name);
            let start = u32::from_le_bytes(chunk[116..120].try_into().unwrap());
            let size = u64::from_le_bytes(chunk[120..128].try_into().unwrap());
            dir.push(DirEntry {
                name,
                kind,
                start,
                size,
            });
        }

        let mut mini_fat = Vec::new();
        if mini_fat_count > 0 && mini_fat_start < 0xFFFFFFFA {
            let bytes = read_chain(data, sector_size, &fat, mini_fat_start)?;
            for chunk in bytes.chunks_exact(4) {
                mini_fat.push(u32::from_le_bytes(chunk.try_into().unwrap()));
            }
        }

        let mini_stream = if let Some(root) = dir.iter().find(|d| d.kind == 5) {
            read_chain(data, sector_size, &fat, root.start).unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(Cfb {
            data,
            sector_size,
            fat,
            dir,
            mini_fat,
            mini_cutoff,
            mini_stream,
        })
    }

    fn stream(&self, name: &str) -> Result<Vec<u8>, String> {
        let want = normalize_stream_name(name);
        let ent = self
            .dir
            .iter()
            .find(|d| d.kind == 2 && normalize_stream_name(&d.name) == want)
            .ok_or_else(|| {
                let have: Vec<_> = self
                    .dir
                    .iter()
                    .filter(|d| d.kind == 2)
                    .map(|d| d.name.clone())
                    .collect();
                format!("msi stream {name} missing (have: {})", have.join(" | "))
            })?;
        let size = ent.size as usize;
        if size < self.mini_cutoff && !self.mini_fat.is_empty() {
            let mut out = Vec::new();
            let mini_size = 64usize;
            let mut sec = ent.start;
            let mut seen = HashSet::new();
            while sec < 0xFFFFFFFA && out.len() < size {
                if !seen.insert(sec) {
                    break;
                }
                let off = sec as usize * mini_size;
                let end = (off + mini_size).min(self.mini_stream.len());
                if off >= self.mini_stream.len() {
                    break;
                }
                out.extend_from_slice(&self.mini_stream[off..end]);
                sec = *self.mini_fat.get(sec as usize).unwrap_or(&0xFFFFFFFE);
            }
            out.truncate(size);
            Ok(out)
        } else {
            let mut out = read_chain(self.data, self.sector_size, &self.fat, ent.start)?;
            out.truncate(size);
            Ok(out)
        }
    }
}

fn normalize_stream_name(name: &str) -> String {
    name.trim()
        .trim_start_matches('\u{5}')
        .trim_start_matches('\u{1}')
        .trim_start_matches("Table.")
        .trim_start_matches('_')
        .to_ascii_lowercase()
}

/// MSI packs stream names into CJK-range codepoints (Wine/libmsi decode).
fn msi_decode_name(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    if chars.peek() == Some(&TABLE_PREFIX) {
        chars.next();
    }
    for chr in chars {
        if chr == '\0' {
            continue;
        }
        let v = chr as u32;
        if (0x3800..0x4800).contains(&v) {
            let v = v - 0x3800;
            out.push(from_b64(v & 0x3f));
            out.push(from_b64(v >> 6));
        } else if (0x4800..0x4840).contains(&v) {
            out.push(from_b64(v - 0x4800));
        } else {
            out.push(chr);
        }
    }
    out
}

fn from_b64(v: u32) -> char {
    if v < 10 {
        (b'0' + v as u8) as char
    } else if v < 36 {
        (b'A' + (v as u8 - 10)) as char
    } else if v < 62 {
        (b'a' + (v as u8 - 36)) as char
    } else if v == 62 {
        '.'
    } else {
        '_'
    }
}

/// CFB sector N lives at `(N+1)*sector_size`. The 512-byte header occupies the
/// start of sector slot -1; for 4096-byte sectors the rest of that slot is
/// padding, so `512 + N*sector_size` points at garbage.
fn sector(data: &[u8], sector_size: usize, id: u32) -> Result<&[u8], String> {
    let start = (id as usize + 1) * sector_size;
    data.get(start..start + sector_size)
        .ok_or_else(|| format!("cfb sector {id} oob"))
}

fn read_chain(data: &[u8], sector_size: usize, fat: &[u32], start: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut sec = start;
    let mut seen = HashSet::new();
    while sec < 0xFFFFFFFA {
        if !seen.insert(sec) {
            break;
        }
        if seen.len() > fat.len().saturating_add(2) {
            return Err("cfb fat loop".into());
        }
        out.extend_from_slice(sector(data, sector_size, sec)?);
        sec = *fat.get(sec as usize).unwrap_or(&0xFFFFFFFE);
    }
    Ok(out)
}

fn read_string_pool(cfb: &Cfb<'_>) -> Result<(Vec<String>, bool), String> {
    let pool = cfb.stream("_StringPool")?;
    let data = cfb.stream("_StringData")?;
    if pool.len() < 4 {
        return Err("string pool short".into());
    }
    let codepage = u32::from_le_bytes(pool[0..4].try_into().unwrap());
    let long_str = (codepage & 0x8000_0000) != 0;
    let mut strings = vec![String::new()];
    let mut poff = 4usize;
    let mut doff = 0usize;
    while poff + 4 <= pool.len() {
        let mut len = u16::from_le_bytes([pool[poff], pool[poff + 1]]) as u32;
        let refs = u16::from_le_bytes([pool[poff + 2], pool[poff + 3]]);
        poff += 4;
        if len == 0 && refs != 0 {
            if poff + 4 > pool.len() {
                break;
            }
            len = u32::from_le_bytes(pool[poff..poff + 4].try_into().unwrap());
            poff += 4;
        }
        let n = len as usize;
        let end = doff.saturating_add(n);
        if end > data.len() {
            strings.push(String::new());
            break;
        }
        strings.push(String::from_utf8_lossy(&data[doff..end]).into_owned());
        doff = end;
    }
    Ok((strings, long_str))
}

struct Col {
    size: usize,
    string: bool,
}

fn read_columns_schema(
    cfb: &Cfb<'_>,
    strings: &[String],
    long_str: bool,
) -> Result<HashMap<String, Vec<(i32, Col)>>, String> {
    let raw = cfb.stream("_Columns")?;
    let str_w = if long_str { 3 } else { 2 };
    let row_size = str_w + 2 + str_w + 2;
    if row_size == 0 || raw.len() < row_size {
        return Err("empty _Columns".into());
    }
    let n = raw.len() / row_size;
    let mut off = 0usize;
    let mut tables = Vec::with_capacity(n);
    for _ in 0..n {
        tables.push(string_ref(&raw[off..off + str_w], strings));
        off += str_w;
    }
    let mut numbers = Vec::with_capacity(n);
    for _ in 0..n {
        numbers.push(msi_i16(&raw[off..off + 2]));
        off += 2;
    }
    off += str_w * n;
    let mut types = Vec::with_capacity(n);
    for _ in 0..n {
        types.push(msi_i16(&raw[off..off + 2]));
        off += 2;
    }
    let mut by_table: HashMap<String, Vec<(i32, Col)>> = HashMap::new();
    for i in 0..n {
        let ty = types[i] as u16;
        by_table
            .entry(tables[i].clone())
            .or_default()
            .push((numbers[i], col_from_type(ty, long_str)));
    }
    for cols in by_table.values_mut() {
        cols.sort_by_key(|(n, _)| *n);
    }
    Ok(by_table)
}

fn col_from_type(ty: u16, long_str: bool) -> Col {
    let string = is_string_col(ty);
    let size = if string {
        if long_str {
            3
        } else {
            2
        }
    } else if (ty & 0xff) > 2 {
        4
    } else {
        2
    };
    Col { size, string }
}

fn is_string_col(ty: u16) -> bool {
    (ty & 0x0800) != 0 && (ty & 0x0fff) != 0x0900
}

fn read_table(
    cfb: &Cfb<'_>,
    strings: &[String],
    schemas: &HashMap<String, Vec<(i32, Col)>>,
    name: &str,
    _long_str: bool,
) -> Result<Vec<Vec<String>>, String> {
    let cols = schemas.get(name).ok_or_else(|| format!("no columns for {name}"))?;
    if cols.is_empty() {
        return Err(format!("no columns for {name}"));
    }
    let raw = cfb.stream(name)?;
    let row_size: usize = cols.iter().map(|(_, c)| c.size).sum();
    if row_size == 0 {
        return Ok(Vec::new());
    }
    let n_rows = raw.len() / row_size;
    let mut rows = vec![vec![String::new(); cols.len()]; n_rows];
    let mut off = 0usize;
    for (ci, (_, col)) in cols.iter().enumerate() {
        for ri in 0..n_rows {
            let end = off + col.size;
            if end > raw.len() {
                break;
            }
            rows[ri][ci] = decode_cell(&raw[off..end], col, strings);
            off = end;
        }
    }
    Ok(rows)
}

fn string_ref(bytes: &[u8], strings: &[String]) -> String {
    let id = if bytes.len() >= 3 {
        u16::from_le_bytes([bytes[0], bytes[1]]) as usize | ((bytes[2] as usize) << 16)
    } else if bytes.len() >= 2 {
        u16::from_le_bytes([bytes[0], bytes[1]]) as usize
    } else {
        0
    };
    strings.get(id).cloned().unwrap_or_default()
}

fn msi_i16(bytes: &[u8]) -> i32 {
    let v = u16::from_le_bytes([bytes[0], bytes.get(1).copied().unwrap_or(0)]);
    if v == 0 {
        0
    } else {
        v as i32 - 0x8000
    }
}

fn msi_i32(bytes: &[u8]) -> i32 {
    let v = u32::from_le_bytes(bytes[..4].try_into().unwrap_or([0; 4]));
    if v == 0 {
        0
    } else {
        (v as i64 - 0x8000_0000) as i32
    }
}

fn decode_cell(bytes: &[u8], col: &Col, strings: &[String]) -> String {
    if col.string {
        string_ref(bytes, strings)
    } else if col.size >= 4 {
        msi_i32(bytes).to_string()
    } else {
        msi_i16(bytes).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_columns_stream_name() {
        assert_eq!(
            msi_decode_name("\u{4840}\u{3b3f}\u{43f2}\u{4438}\u{45b1}"),
            "_Columns"
        );
        assert_eq!(
            msi_decode_name("\u{4840}\u{3f7f}\u{4164}\u{422f}\u{4836}"),
            "_Tables"
        );
        assert_eq!(normalize_stream_name("_StringPool"), "stringpool");
        assert_eq!(
            normalize_stream_name(&msi_decode_name(
                "\u{4840}\u{3b3f}\u{43f2}\u{4438}\u{45b1}"
            )),
            "columns"
        );
    }

    #[test]
    fn sector_offset_4096() {
        assert_eq!((0u32 as usize + 1) * 4096, 4096);
        assert_eq!((1u32 as usize + 1) * 4096, 8192);
    }
}
