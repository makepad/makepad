//! Installed SFNT fonts without a platform font service.
//!
//! Per file we read the 12-byte header (plus TTC offsets), then per face
//! 12 + 16 * table_count bytes, the name table, 64 OS/2 bytes, 46 head
//! bytes and the fvar header/axes. Glyph data is never read. Cmap bytes are
//! read only on a coverage query and their parsed ranges are cached.
//! Each query stats the directories; unchanged directories reuse their faces.
//! Work runs on the caller, so prewarm the metadata on a background thread.

use super::SystemFont;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Clone)]
pub(super) struct FaceInfo {
    pub(super) path: PathBuf,
    pub(super) index: u32,
    pub(super) families: Vec<String>,
    pub(super) full_names: Vec<String>,
    pub(super) weight: u16,
    pub(super) italic: bool,
    pub(super) has_wght_axis: bool,
    drawable: bool,
    cmap: Option<Table>,
    coverage: Arc<OnceLock<Vec<(u32, u32)>>>,
}

impl FaceInfo {
    fn system_font(&self) -> SystemFont {
        SystemFont { path: self.path.clone(), index: self.index }
    }

    fn coverage(&self) -> &[(u32, u32)] {
        if self.coverage.get().is_none() {
            // Competing callers may parse twice, but never wait for another
            // thread's disk I/O inside a OnceLock initializer.
            let ranges = self.cmap.and_then(|table| {
                let mut file = FontFile::open(&self.path)?;
                parse_cmap(&file.read_table(table)?)
            }).unwrap_or_default();
            let _ = self.coverage.set(ranges);
        }
        self.coverage.get().map(Vec::as_slice).unwrap_or(&[])
    }
}

struct Directory {
    path: PathBuf,
    modified: Option<SystemTime>,
    children: Vec<PathBuf>,
    faces: Vec<FaceInfo>,
}

struct Snapshot {
    directories: Vec<Arc<Directory>>,
    faces: Vec<FaceInfo>,
}

#[derive(Clone)]
struct Index {
    snapshot: Arc<Snapshot>,
}

static INDEX: Mutex<Option<Index>> = Mutex::new(None);

impl Index {
    fn refresh(previous: Option<&Self>, roots: &[PathBuf]) -> Self {
        let old: HashMap<&Path, &Arc<Directory>> = previous.into_iter()
            .flat_map(|index| index.snapshot.directories.iter())
            .map(|dir| (dir.path.as_path(), dir))
            .collect();
        let mut directories = Vec::new();
        let mut visited = HashSet::new();
        for root in roots {
            visit_directory(root, 0, &old, &mut visited, &mut directories);
        }
        if let Some(previous) = previous {
            let before = &previous.snapshot.directories;
            if before.len() == directories.len()
                && before.iter().zip(&directories).all(|(a, b)| Arc::ptr_eq(a, b))
            {
                return previous.clone();
            }
        }
        let faces = directories.iter().flat_map(|dir| dir.faces.iter().cloned()).collect();
        Self { snapshot: Arc::new(Snapshot { directories, faces }) }
    }
}

fn current_index() -> Index {
    // Only clone/swap the snapshot under the lock. Scans, cmap parsing,
    // callbacks and dropping the previous snapshot all happen outside it.
    let previous = INDEX.lock().unwrap_or_else(|err| err.into_inner()).clone();
    let refreshed = Index::refresh(previous.as_ref(), &font_roots());
    let retired = {
        let mut slot = INDEX.lock().unwrap_or_else(|err| err.into_inner());
        let unchanged = match (slot.as_ref(), previous.as_ref()) {
            (Some(current), Some(previous)) => Arc::ptr_eq(&current.snapshot, &previous.snapshot),
            (None, None) => true,
            _ => false,
        };
        // A concurrent refresh must not be overwritten by an older scan.
        if unchanged { slot.replace(refreshed.clone()) } else { None }
    };
    drop(retired);
    refreshed
}

pub(super) fn with_faces<R>(f: impl FnOnce(&[FaceInfo]) -> R) -> R {
    let index = current_index();
    f(&index.snapshot.faces)
}

static PREWARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The index and every face's coverage, so a later fallback query on the UI
/// thread only stats the folders.
pub(super) fn prewarm() {
    let index = current_index();
    for face in &index.snapshot.faces {
        if face.drawable {
            face.coverage();
        }
    }
    PREWARMED.store(true, std::sync::atomic::Ordering::Release);
}

/// Whether [`prewarm`] has finished once.
pub(super) fn prewarmed() -> bool {
    PREWARMED.load(std::sync::atomic::Ordering::Acquire)
}

fn font_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
        let windows = std::env::var_os("WINDIR").filter(|v| !v.is_empty())
            .unwrap_or_else(|| "C:\\Windows".into());
        roots.push(PathBuf::from(windows).join("Fonts"));
    } else if cfg!(target_os = "android") {
        roots.extend([PathBuf::from("/system/fonts"), PathBuf::from("/product/fonts")]);
    } else if cfg!(unix) {
        let home = std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
        if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(data).join("fonts"));
        } else if let Some(home) = &home {
            roots.push(home.join(".local/share/fonts"));
        }
        if let Some(home) = home {
            roots.push(home.join(".fonts"));
        }
        let data_dirs = std::env::var_os("XDG_DATA_DIRS").filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        roots.extend(std::env::split_paths(&data_dirs)
            .filter(|path| !path.as_os_str().is_empty()).map(|path| path.join("fonts")));
        roots.extend([PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")]);
    }
    // Canonicalize on every query, including roots that were absent before.
    let mut seen = HashSet::new();
    roots.into_iter().filter_map(|path| fs::canonicalize(path).ok())
        .filter(|path| seen.insert(path.clone())).collect()
}

fn visit_directory(
    path: &Path,
    depth: usize,
    old: &HashMap<&Path, &Arc<Directory>>,
    visited: &mut HashSet<PathBuf>,
    directories: &mut Vec<Arc<Directory>>,
) {
    if depth > 6 {
        return;
    }
    let Ok(path) = fs::canonicalize(path) else { return };
    let Ok(metadata) = fs::metadata(&path) else { return };
    if !metadata.is_dir() || !visited.insert(path.clone()) {
        return;
    }
    let modified = metadata.modified().ok();
    let cached = old.get(path.as_path())
        .filter(|dir| modified.is_some() && dir.modified == modified);
    let directory = match cached {
        Some(dir) => Arc::clone(dir),
        None => {
            let Some(dir) = scan_directory(path, modified) else { return };
            Arc::new(dir)
        }
    };
    directories.push(directory.clone());
    for child in &directory.children {
        visit_directory(child, depth + 1, old, visited, directories);
    }
}

fn scan_directory(path: PathBuf, modified: Option<SystemTime>) -> Option<Directory> {
    let mut entries: Vec<_> = fs::read_dir(&path).ok()?.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    let mut children = Vec::new();
    let mut faces = Vec::new();
    let mut files = HashSet::new();
    for entry in entries {
        let Ok(kind) = entry.file_type() else { continue };
        let entry_path = entry.path();
        let metadata = if kind.is_symlink() { fs::metadata(&entry_path).ok() } else { None };
        if kind.is_dir() || metadata.as_ref().is_some_and(|meta| meta.is_dir()) {
            children.push(entry_path);
        } else if (kind.is_file() || metadata.as_ref().is_some_and(|meta| meta.is_file()))
            && is_font_path(&entry_path)
        {
            let Ok(file) = fs::canonicalize(entry_path) else { continue };
            if files.insert(file.clone()) {
                if let Some(found) = scan_file(&file) {
                    faces.extend(found);
                }
            }
        }
    }
    Some(Directory { path, modified, children, faces })
}

fn is_font_path(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
        ["ttf", "otf", "ttc", "otc"].iter().any(|supported| extension.eq_ignore_ascii_case(supported))
    })
}

pub(super) fn find(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
    let name = name.to_lowercase();
    with_faces(|faces| {
        let family_match = faces.iter().any(|face| face.families.contains(&name));
        faces.iter().filter(|face| {
            if family_match { face.families.contains(&name) } else { face.full_names.contains(&name) }
        }).min_by_key(|face| {
            let distance = if face.has_wght_axis { 0 } else { face.weight.abs_diff(weight) };
            let score = u32::from(distance) + if face.italic != italic { 1000 } else { 0 };
            // Keep variable faces neutral on weight ties as well.
            let actual = if face.has_wght_axis { weight } else { face.weight };
            let tie = if weight > 500 { u16::MAX - actual } else { actual };
            (score, tie)
        }).map(FaceInfo::system_font)
    })
}

pub(super) fn covering(text: &str, bold: bool, limit: usize) -> Vec<SystemFont> {
    if text.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut chars: Vec<u32> = text.chars().map(u32::from).collect();
    chars.sort_unstable();
    chars.dedup();
    let emoji = chars.iter().any(|ch| matches!(*ch, 0x1f000..=0x1faff | 0x2600..=0x27bf));
    let weight = if bold { 700 } else { 400 };
    // A face made for the script ("Noto Sans Arabic") before a pan-script
    // one that happens to cover the same characters.
    let script = text
        .chars()
        .map(super::fallback_script)
        .find(|script| !matches!(script, super::Script::Common | super::Script::Inherited))
        .map(|script| script.full_name().to_lowercase().replace('_', " "));
    with_faces(|faces| {
        let mut ranked = Vec::new();
        for face in faces {
            // (Droid Sans Fallback is a real CJK face: only the tofu font is out.)
            if !face.drawable || face.families.iter().any(|name| name.contains("lastresort")) {
                continue;
            }
            let ranges = face.coverage();
            let covered = chars.iter().filter(|&&ch| covers(ranges, ch)).count();
            if covered == 0 {
                continue;
            }
            let has = |part: &str| face.families.iter().any(|name| name.contains(part));
            let linux_family = cfg!(all(unix, not(target_vendor = "apple"), not(target_os = "android")))
                && (has("noto") || has("dejavu") || has("droid"));
            let family_kind = if has("sans") { 0 } else if has("serif") { 2 } else { 1 };
            let distance = if face.has_wght_axis { 0 } else { face.weight.abs_diff(weight) };
            let for_script = script.as_deref().is_some_and(|script| has(script));
            ranked.push((
                (std::cmp::Reverse(covered), emoji && !has("emoji"), !for_script, face.italic,
                    distance, family_kind, !linux_family),
                face,
            ));
        }
        // Stable sorting retains user-folder priority for otherwise equal faces.
        ranked.sort_by_key(|(score, _)| *score);
        let mut seen = HashSet::new();
        ranked.into_iter().filter_map(|(_, face)| {
            let font = face.system_font();
            if seen.insert(font.clone()) { Some(font) } else { None }
        }).take(limit).collect()
    })
}

fn covers(ranges: &[(u32, u32)], ch: u32) -> bool {
    let next = ranges.partition_point(|&(start, _)| start <= ch);
    next > 0 && ch <= ranges[next - 1].1
}

#[derive(Clone, Copy)]
struct Table {
    offset: u64,
    length: usize,
}

struct FontFile {
    file: File,
    length: u64,
}

impl FontFile {
    fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() { return None }
        Some(Self { file, length: metadata.len() })
    }

    fn read_at(&mut self, offset: u64, length: usize) -> Option<Vec<u8>> {
        if offset.checked_add(u64::try_from(length).ok()?)? > self.length {
            return None;
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).ok()?;
        bytes.resize(length, 0);
        self.file.seek(SeekFrom::Start(offset)).ok()?;
        self.file.read_exact(&mut bytes).ok()?;
        Some(bytes)
    }

    fn read_table(&mut self, table: Table) -> Option<Vec<u8>> {
        self.read_at(table.offset, table.length)
    }

    fn read_part(&mut self, table: Table, offset: usize, length: usize) -> Option<Vec<u8>> {
        if offset.checked_add(length)? > table.length { return None }
        self.read_at(table.offset.checked_add(u64::try_from(offset).ok()?)?, length)
    }
}

struct Be<'a>(&'a [u8]);

impl<'a> Be<'a> {
    fn bytes(&self, offset: usize, length: usize) -> Option<&'a [u8]> {
        self.0.get(offset..offset.checked_add(length)?)
    }

    fn u16(&self, offset: usize) -> Option<u16> {
        Some(u16::from_be_bytes(self.bytes(offset, 2)?.try_into().ok()?))
    }

    fn u32(&self, offset: usize) -> Option<u32> {
        Some(u32::from_be_bytes(self.bytes(offset, 4)?.try_into().ok()?))
    }
}

fn scan_file(path: &Path) -> Option<Vec<FaceInfo>> {
    let mut file = FontFile::open(path)?;
    let header = file.read_at(0, 12)?;
    let header = Be(&header);
    let offsets: Vec<u32> = if header.bytes(0, 4)? == b"ttcf" {
        if !matches!(header.u32(4)?, 0x0001_0000 | 0x0002_0000) { return None }
        let count = usize::try_from(header.u32(8)?).ok()?;
        if count == 0 { return None }
        let bytes = file.read_at(12, count.checked_mul(4)?)?;
        bytes.chunks_exact(4).map(|word| u32::from_be_bytes(word.try_into().unwrap())).collect()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, offset) in offsets.into_iter().enumerate() {
        faces.push(scan_face(&mut file, path, u32::try_from(index).ok()?, u64::from(offset))?);
    }
    Some(faces)
}

fn scan_face(file: &mut FontFile, path: &Path, index: u32, offset: u64) -> Option<FaceInfo> {
    let header = file.read_at(offset, 12)?;
    let header = Be(&header);
    if !matches!(header.u32(0)?, 0x0001_0000 | 0x4f54_544f | 0x7472_7565) { return None }
    let count = usize::from(header.u16(4)?);
    let records = file.read_at(offset.checked_add(12)?, count.checked_mul(16)?)?;
    let mut name = None;
    let mut os2 = None;
    let mut head = None;
    let mut fvar = None;
    let mut cmap = None;
    let mut drawable = false;
    for record in records.chunks_exact(16) {
        let record = Be(record);
        let table = Table {
            offset: u64::from(record.u32(8)?),
            length: usize::try_from(record.u32(12)?).ok()?,
        };
        if table.offset.checked_add(u64::try_from(table.length).ok()?)? > file.length { return None }
        match record.bytes(0, 4)? {
            b"name" => name = Some(table),
            b"OS/2" => os2 = Some(table),
            b"head" => head = Some(table),
            b"fvar" => fvar = Some(table),
            b"cmap" => cmap = Some(table),
            b"glyf" | b"CFF " | b"CFF2" | b"CBDT" | b"sbix" => drawable = true,
            _ => {}
        }
    }
    let (families, full_names) = parse_names(&file.read_table(name?)?)?;
    let mut weight = 400;
    let mut italic = false;
    if let Some(table) = os2 {
        let data = file.read_part(table, 0, 64)?;
        let data = Be(&data);
        weight = data.u16(4)?;
        if weight == 0 { weight = 400; }
        italic = data.u16(62)? & ((1 << 0) | (1 << 9)) != 0;
    }
    if let Some(table) = head {
        let data = file.read_part(table, 0, 46)?;
        italic |= Be(&data).u16(44)? & (1 << 1) != 0;
    }
    let mut has_wght_axis = false;
    if let Some(table) = fvar {
        let data = file.read_part(table, 0, 16)?;
        let data = Be(&data);
        if data.u32(0)? != 0x0001_0000 { return None }
        let axes_offset = usize::from(data.u16(4)?);
        let axes_count = usize::from(data.u16(8)?);
        let axes_size = usize::from(data.u16(10)?);
        if axes_offset < 16 || axes_size < 20 { return None }
        let axes = file.read_part(table, axes_offset, axes_count.checked_mul(axes_size)?)?;
        has_wght_axis = axes.chunks_exact(axes_size).any(|axis| &axis[..4] == b"wght");
    }
    Some(FaceInfo {
        path: path.to_path_buf(), index, families, full_names, weight, italic, has_wght_axis,
        drawable, cmap, coverage: Arc::new(OnceLock::new()),
    })
}

fn parse_names(bytes: &[u8]) -> Option<(Vec<String>, Vec<String>)> {
    let data = Be(bytes);
    let format = data.u16(0)?;
    if format > 1 { return None }
    let count = usize::from(data.u16(2)?);
    let strings = usize::from(data.u16(4)?);
    let records_end = 6usize.checked_add(count.checked_mul(12)?)?;
    data.bytes(0, records_end)?;
    let mut storage_start = records_end;
    if format == 1 {
        let languages = usize::from(data.u16(records_end)?);
        storage_start = records_end.checked_add(2)?.checked_add(languages.checked_mul(4)?)?;
        data.bytes(records_end, storage_start - records_end)?;
        for i in 0..languages {
            let record = records_end + 2 + i * 4;
            data.bytes(strings.checked_add(usize::from(data.u16(record + 2)?))?,
                usize::from(data.u16(record)?))?;
        }
    }
    if strings < storage_start || strings > bytes.len() { return None }
    let mut families = Vec::new();
    let mut full_names = Vec::new();
    for i in 0..count {
        let record = 6 + i * 12;
        let platform = data.u16(record)?;
        let encoding = data.u16(record + 2)?;
        let name_id = data.u16(record + 6)?;
        let length = usize::from(data.u16(record + 8)?);
        let offset = strings.checked_add(usize::from(data.u16(record + 10)?))?;
        let value = data.bytes(offset, length)?;
        let names = match name_id {
            1 | 16 => &mut families,
            4 | 6 => &mut full_names,
            _ => continue,
        };
        let decoded = match (platform, encoding) {
            (0, _) | (3, 0 | 1 | 10) => {
                if value.len() % 2 != 0 { return None }
                let words: Vec<u16> = value.chunks_exact(2)
                    .map(|word| u16::from_be_bytes([word[0], word[1]])).collect();
                String::from_utf16(&words).ok()
            }
            // ASCII is shared by Mac Roman. Leave non-ASCII Mac records to
            // their Unicode counterparts instead of guessing an encoding.
            (1, 0) if value.is_ascii() => String::from_utf8(value.to_vec()).ok(),
            _ => None,
        };
        if let Some(value) = decoded {
            let value = value.to_lowercase();
            if !value.is_empty() && !names.contains(&value) {
                names.push(value);
            }
        }
    }
    Some((families, full_names))
}

fn parse_cmap(bytes: &[u8]) -> Option<Vec<(u32, u32)>> {
    let data = Be(bytes);
    if data.u16(0)? != 0 { return None }
    let count = usize::from(data.u16(2)?);
    data.bytes(4, count.checked_mul(8)?)?;
    let mut seen = HashSet::new();
    let mut ranges = Vec::new();
    for i in 0..count {
        let record = 4 + i * 8;
        let platform = data.u16(record)?;
        let encoding = data.u16(record + 2)?;
        let offset = usize::try_from(data.u32(record + 4)?).ok()?;
        if !matches!((platform, encoding), (0, _) | (3, 1 | 10)) || !seen.insert(offset) {
            continue;
        }
        let subtable = bytes.get(offset..)?;
        // A malformed subtable (a format 4 whose 16-bit length overflowed in
        // a big CJK font) keeps the ranges read before the fault; the other
        // subtables still count.
        let _ = match Be(subtable).u16(0)? {
            4 => cmap4(subtable, &mut ranges),
            12 | 13 => cmap_groups(subtable, &mut ranges),
            _ => None,
        };
    }
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    Some(merged)
}

fn cmap4(bytes: &[u8], ranges: &mut Vec<(u32, u32)>) -> Option<()> {
    let length = usize::from(Be(bytes).u16(2)?);
    let data = Be(bytes.get(..length)?);
    let seg_count_x2 = usize::from(data.u16(6)?);
    if seg_count_x2 == 0 || seg_count_x2 % 2 != 0 { return None }
    let count = seg_count_x2 / 2;
    let glyphs = 16usize.checked_add(count.checked_mul(8)?)?;
    data.bytes(0, glyphs)?;
    if data.u16(14 + 2 * count)? != 0 { return None }
    let mut previous_end = None;
    for i in 0..count {
        let end = data.u16(14 + 2 * i)?;
        let start = data.u16(16 + 2 * count + 2 * i)?;
        if start > end || previous_end.is_some_and(|prev| start <= prev) { return None }
        previous_end = Some(end);
        let delta = data.u16(16 + 4 * count + 2 * i)?;
        let range_word = 16 + 6 * count + 2 * i;
        let range_offset = usize::from(data.u16(range_word)?);
        if range_offset % 2 != 0 { return None }
        let mut run = None;
        for ch in u32::from(start)..=u32::from(end) {
            let glyph = if range_offset == 0 {
                (ch as u16).wrapping_add(delta)
            } else {
                let offset = range_word.checked_add(range_offset)?
                    .checked_add(usize::try_from(ch - u32::from(start)).ok()?.checked_mul(2)?)?;
                if offset < glyphs { return None }
                let glyph = data.u16(offset)?;
                if glyph == 0 { 0 } else { glyph.wrapping_add(delta) }
            };
            if glyph != 0 {
                if run.is_none() { run = Some(ch); }
            } else if let Some(start) = run.take() {
                ranges.push((start, ch - 1));
            }
        }
        if let Some(start) = run { ranges.push((start, u32::from(end))); }
    }
    Some(())
}

fn cmap_groups(bytes: &[u8], ranges: &mut Vec<(u32, u32)>) -> Option<()> {
    let header = Be(bytes);
    let format = header.u16(0)?;
    if header.u16(2)? != 0 { return None }
    let length = usize::try_from(header.u32(4)?).ok()?;
    let data = Be(bytes.get(..length)?);
    let count = usize::try_from(data.u32(12)?).ok()?;
    data.bytes(16, count.checked_mul(12)?)?;
    let mut previous_end = None;
    for i in 0..count {
        let record = 16 + i * 12;
        let mut start = data.u32(record)?;
        let end = data.u32(record + 4)?;
        let glyph = data.u32(record + 8)?;
        if start > end || end > 0x10ffff || previous_end.is_some_and(|prev| start <= prev) { return None }
        previous_end = Some(end);
        if format == 12 {
            glyph.checked_add(end - start)?;
            if glyph == 0 { start += 1; }
        } else if glyph == 0 {
            continue;
        }
        if start <= end { ranges.push((start, end)); }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::{AtomicUsize, Ordering}, time::{Duration, UNIX_EPOCH}};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!(
                "makepad-font-scan-{}-{time}-{unique}", std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn minimal_font(family: &str, weight: u16, selection: u16, mac_style: u16) -> Vec<u8> {
        let records = [
            (3, 1, 0x409, 1, family.to_owned()),
            (0, 3, 0, 16, family.to_owned()),
            (3, 10, 0x411, 16, "別の名前".to_owned()),
            (3, 1, 0x409, 4, format!("{family} Regular")),
            (1, 0, 0, 6, "ScanPostScript".to_owned()),
        ];
        let storage = 6 + 12 * records.len();
        let mut names = vec![0; storage];
        put16(&mut names, 2, records.len() as u16);
        put16(&mut names, 4, storage as u16);
        for (i, (platform, encoding, language, name_id, text)) in records.iter().enumerate() {
            let value: Vec<u8> = if *platform == 1 {
                text.as_bytes().to_vec()
            } else {
                text.encode_utf16().flat_map(u16::to_be_bytes).collect()
            };
            let record = 6 + i * 12;
            let offset = names.len() - storage;
            put16(&mut names, record, *platform);
            put16(&mut names, record + 2, *encoding);
            put16(&mut names, record + 4, *language);
            put16(&mut names, record + 6, *name_id);
            put16(&mut names, record + 8, value.len() as u16);
            put16(&mut names, record + 10, offset as u16);
            names.extend(value);
        }
        let mut os2 = vec![0; 78];
        put16(&mut os2, 4, weight);
        put16(&mut os2, 62, selection);
        let mut head = vec![0; 54];
        put16(&mut head, 44, mac_style);
        // A..C use glyphIdArray with a hole at B; D uses idDelta.
        // The sentinel maps to glyph zero and must never count as coverage.
        let mut format4 = vec![0; 46];
        put16(&mut format4, 0, 4);
        put16(&mut format4, 2, 46);
        put16(&mut format4, 6, 6);
        put16(&mut format4, 8, 4);
        put16(&mut format4, 10, 1);
        put16(&mut format4, 12, 2);
        put16(&mut format4, 14, 67);
        put16(&mut format4, 16, 68);
        put16(&mut format4, 18, 0xffff);
        put16(&mut format4, 22, 65);
        put16(&mut format4, 24, 68);
        put16(&mut format4, 26, 0xffff);
        put16(&mut format4, 28, 1);
        put16(&mut format4, 30, 1u16.wrapping_sub(68));
        put16(&mut format4, 32, 1);
        put16(&mut format4, 34, 6);
        put16(&mut format4, 40, 1);
        put16(&mut format4, 44, 2);
        let mut cmap = vec![0; 12];
        put16(&mut cmap, 2, 1);
        put16(&mut cmap, 4, 3);
        put16(&mut cmap, 6, 1);
        put32(&mut cmap, 8, 12);
        cmap.extend(format4);
        sfnt(&[(b"name", names), (b"OS/2", os2), (b"head", head), (b"cmap", cmap), (b"glyf", vec![0; 10])])
    }

    fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = vec![0; 12 + tables.len() * 16];
        put32(&mut bytes, 0, 0x0001_0000);
        put16(&mut bytes, 4, tables.len() as u16);
        for (i, (tag, data)) in tables.iter().enumerate() {
            while bytes.len() % 4 != 0 { bytes.push(0); }
            let offset = bytes.len();
            let record = 12 + i * 16;
            bytes[record..record + 4].copy_from_slice(*tag);
            put32(&mut bytes, record + 8, offset as u32);
            put32(&mut bytes, record + 12, data.len() as u32);
            bytes.extend_from_slice(data);
        }
        bytes
    }

    #[test]
    fn reads_names_style_and_lazy_coverage() {
        let dir = TempDir::new();
        let path = dir.0.join("sample.TTF");
        fs::write(&path, minimal_font("Scan Family", 700, 1 << 9, 0)).unwrap();
        let index = Index::refresh(None, &[dir.0.clone()]);
        let faces = &index.snapshot.faces;
        assert_eq!(faces.len(), 1);
        let face = &faces[0];
        assert_eq!(face.families, ["scan family", "別の名前"]);
        assert_eq!(face.full_names, ["scan family regular", "scanpostscript"]);
        assert_eq!(face.weight, 700);
        assert!(face.italic);
        assert!(!face.has_wght_axis);
        assert!(face.drawable);
        assert!(face.coverage.get().is_none());
        assert_eq!(face.coverage(), &[(65, 65), (67, 68)]);
        assert!(covers(face.coverage(), u32::from('A')));
        assert!(!covers(face.coverage(), u32::from('B')));
        assert!(!covers(face.coverage(), 0xffff));
        // An unchanged directory keeps the parsed cmap, including its Arc.
        let again = Index::refresh(Some(&index), &[dir.0.clone()]);
        assert!(Arc::ptr_eq(&index.snapshot, &again.snapshot));
        for (selection, mac_style) in [(1, 0), (0, 2)] {
            fs::write(&path, minimal_font("Scan Family", 400, selection, mac_style)).unwrap();
            assert!(scan_file(&path).unwrap()[0].italic);
        }
        // A bad table extent rejects the file without reading its payload.
        let mut malformed = minimal_font("Bad Font", 400, 0, 0);
        put32(&mut malformed, 20, u32::MAX);
        fs::write(&path, malformed).unwrap();
        assert!(scan_file(&path).is_none());
    }

    #[test]
    fn directory_mtime_refreshes_faces_and_children() {
        let dir = TempDir::new();
        fs::write(dir.0.join("first.ttf"), minimal_font("First", 400, 0, 0)).unwrap();
        let first = Index::refresh(None, &[dir.0.clone()]);
        assert_eq!(first.snapshot.faces.len(), 1);
        let old_mtime = fs::metadata(&dir.0).unwrap().modified().unwrap();
        // Cross a coarse filesystem timestamp tick before rewriting entries.
        std::thread::sleep(Duration::from_millis(2100));
        fs::write(dir.0.join("second.otf"), minimal_font("Second", 400, 0, 0)).unwrap();
        assert_ne!(fs::metadata(&dir.0).unwrap().modified().unwrap(), old_mtime);
        let second = Index::refresh(Some(&first), &[dir.0.clone()]);
        assert_eq!(second.snapshot.faces.len(), 2);
        assert!(second.snapshot.faces.iter().any(|face| face.families.contains(&"second".to_owned())));
        assert!(!Arc::ptr_eq(&first.snapshot.faces[0].coverage, &second.snapshot.faces[0].coverage));
        std::thread::sleep(Duration::from_millis(2100));
        let child = dir.0.join("nested");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("third.ttf"), minimal_font("Third", 400, 0, 0)).unwrap();
        let third = Index::refresh(Some(&second), &[dir.0.clone()]);
        assert_eq!(third.snapshot.faces.len(), 3);
        // Even if its parent's timestamp is unchanged, a vanished child drops.
        fs::remove_dir_all(child).unwrap();
        let after = Index::refresh(Some(&third), &[dir.0.clone()]);
        assert_eq!(after.snapshot.faces.len(), 2);
    }

    #[test]
    fn grouped_cmaps_exclude_missing_glyphs() {
        for format in [12, 13] {
            let mut cmap = vec![0; 12 + 40];
            put16(&mut cmap, 2, 1);
            put16(&mut cmap, 4, 3);
            put16(&mut cmap, 6, 10);
            put32(&mut cmap, 8, 12);
            put16(&mut cmap, 12, format);
            put32(&mut cmap, 16, 40);
            put32(&mut cmap, 24, 2);
            put32(&mut cmap, 28, 0x1f600);
            put32(&mut cmap, 32, 0x1f602);
            put32(&mut cmap, 36, 0);
            put32(&mut cmap, 40, 0x1f610);
            put32(&mut cmap, 44, 0x1f611);
            put32(&mut cmap, 48, 4);
            let ranges = parse_cmap(&cmap).unwrap();
            assert!(!covers(&ranges, 0x1f600));
            assert_eq!(covers(&ranges, 0x1f601), format == 12);
            assert!(covers(&ranges, 0x1f610));
            // A bad group ends its subtable; what came before it stays.
            put32(&mut cmap, 44, 0x110000);
            let ranges = parse_cmap(&cmap).unwrap();
            assert_eq!(covers(&ranges, 0x1f601), format == 12);
            assert!(!covers(&ranges, 0x1f610));
        }
    }

    /// Printed only: what a cold index of this machine's fonts costs.
    #[test]
    fn print_cold_prewarm_cost() {
        let started = std::time::Instant::now();
        prewarm();
        let (faces, bytes) = with_faces(|faces| {
            let names: usize = faces.iter().flat_map(|f| f.families.iter().chain(&f.full_names)).map(|n| n.len() + 24).sum();
            let ranges: usize = faces.iter().map(|f| f.coverage().len() * 8).sum();
            (faces.len(), names + ranges + faces.len() * std::mem::size_of::<FaceInfo>())
        });
        let warm = std::time::Instant::now();
        let _ = with_faces(|faces| faces.len());
        println!(
            "font index: {faces} faces, ~{} KB, cold prewarm (names + coverage) {:.1} ms, re-stat query {:.2} ms",
            bytes / 1024,
            (warm - started).as_secs_f64() * 1000.0,
            warm.elapsed().as_secs_f64() * 1000.0
        );
    }

    /// Printed only: installed font sets differ between build nodes.
    #[test]
    fn print_installed_matches() {
        for (name, weight, italic) in [
            ("DejaVu Sans", 400, false), ("DejaVu Sans", 700, true),
            ("Noto Sans", 400, false), ("Segoe UI", 400, false), ("Roboto", 700, false),
        ] {
            println!("{name} {weight} italic={italic}: {:?}", find(name, weight, italic));
        }
    }
}
