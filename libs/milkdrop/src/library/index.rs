//! Walking a preset folder into entries, and remembering the walk.
//!
//! Reading 10,000 presets is about 120 MB of text, so a library keeps what
//! it learned from each file (version and shader flags) in a small cache
//! file keyed by path, size and modification time; the next walk reads only
//! files that are new or changed. Everything here does file I/O and belongs
//! on a worker thread.

use super::{PresetEntry, PresetLibrary};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

/// The MilkDrop generation a preset was written for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MilkVersion {
    /// MilkDrop 1.x: equations only, the fixed warp and composite.
    Md1,
    /// MilkDrop 2: may carry HLSL warp and composite shaders.
    Md2,
    #[default]
    Unknown,
}

impl MilkVersion {
    pub fn label(self) -> &'static str {
        match self {
            MilkVersion::Md1 => "MD1",
            MilkVersion::Md2 => "MD2",
            MilkVersion::Unknown => "",
        }
    }

    fn code(self) -> char {
        match self {
            MilkVersion::Md1 => '1',
            MilkVersion::Md2 => '2',
            MilkVersion::Unknown => '0',
        }
    }

    fn from_code(code: &str) -> Self {
        match code {
            "1" => MilkVersion::Md1,
            "2" => MilkVersion::Md2,
            _ => MilkVersion::Unknown,
        }
    }
}

/// What one read of a preset's text learned, without parsing its equations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sniff {
    pub version: MilkVersion,
    pub warp_shader: bool,
    pub comp_shader: bool,
}

/// Read the header keys and look for shader lines. `MILKDROP_PRESET_VERSION`
/// is the first line of an MD2 preset; an MD1 preset has none (or a value
/// below 200) and starts with `[preset00]`.
pub fn sniff(bytes: &[u8]) -> Sniff {
    let mut out = Sniff { version: MilkVersion::Md1, ..Sniff::default() };
    let mut saw_preset = false;
    for line in bytes.split(|b| *b == b'\n') {
        let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"MILKDROP_PRESET_VERSION=") {
            let digits: Vec<u8> = value.iter().copied().take_while(u8::is_ascii_digit).collect();
            let number: u32 = std::str::from_utf8(&digits).ok().and_then(|d| d.parse().ok()).unwrap_or(0);
            out.version = if number >= 200 { MilkVersion::Md2 } else { MilkVersion::Md1 };
        } else if line.starts_with(b"warp_1=") {
            out.warp_shader = true;
        } else if line.starts_with(b"comp_1=") {
            out.comp_shader = true;
        } else if line.starts_with(b"[preset") {
            saw_preset = true;
        }
    }
    if !saw_preset && out.version == MilkVersion::Md1 && !out.warp_shader && !out.comp_shader {
        out.version = MilkVersion::Unknown;
    }
    out
}

/// One file as the cache remembers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Known {
    size: u64,
    mtime: u64,
    sniff: Sniff,
}

/// How a walk went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub files: usize,
    /// Files read this time (new or changed).
    pub read: usize,
    /// Files whose cached reading was reused.
    pub reused: usize,
    pub millis: u64,
}

const CACHE_HEADER: &str = "milkdrop-index 1";

/// Walk `root` for `.milk` files, reusing what `cache` (when given)
/// remembers for unchanged files, and write the cache back when anything
/// changed. A missing root is an error; unreadable folders and files are
/// skipped. Entries come sorted by relative path.
pub fn scan_cached(root: &Path, cache: Option<&Path>) -> std::io::Result<(PresetLibrary, ScanStats)> {
    let started = Instant::now();
    fs::read_dir(root)?;
    let known = cache.map(read_cache).unwrap_or_default();
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let name = entry.file_name();
            // Hidden folders (a checkout's .git) and files are not presets.
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
                let Ok(meta) = entry.metadata() else { continue };
                let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                files.push((path, meta.len(), mtime));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut stats = ScanStats { files: files.len(), ..ScanStats::default() };
    let mut next: HashMap<String, Known> = HashMap::with_capacity(files.len());
    let mut entries = Vec::with_capacity(files.len());
    for (path, size, mtime) in files {
        let rel = relative(root, &path);
        let sniffed = match known.get(&rel) {
            Some(k) if k.size == size && k.mtime == mtime => {
                stats.reused += 1;
                k.sniff
            }
            _ => {
                let Ok(bytes) = fs::read(&path) else { continue };
                stats.read += 1;
                sniff(&bytes)
            }
        };
        next.insert(rel.clone(), Known { size, mtime, sniff: sniffed });
        entries.push(PresetEntry::new(path, rel, sniffed));
    }
    if let Some(cache) = cache {
        if stats.read > 0 || next.len() != known.len() {
            write_cache(cache, &next);
        }
    }
    stats.millis = started.elapsed().as_millis() as u64;
    Ok((PresetLibrary { root: root.to_path_buf(), entries }, stats))
}

/// `path` under `root` with `/` separators: the stable name favourites and
/// the cache use.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn read_cache(path: &Path) -> HashMap<String, Known> {
    let Ok(text) = fs::read_to_string(path) else { return HashMap::new() };
    let mut lines = text.lines();
    if lines.next() != Some(CACHE_HEADER) {
        return HashMap::new();
    }
    lines
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let rel = fields.next()?.to_string();
            let size = fields.next()?.parse().ok()?;
            let mtime = fields.next()?.parse().ok()?;
            let version = MilkVersion::from_code(fields.next()?);
            let flags = fields.next()?;
            let sniff = Sniff { version, warp_shader: flags.contains('w'), comp_shader: flags.contains('c') };
            (!rel.is_empty()).then_some((rel, Known { size, mtime, sniff }))
        })
        .collect()
}

/// Write the cache through a temporary file renamed into place, so a crash
/// leaves the old cache or the new one, never half of one.
fn write_cache(path: &Path, known: &HashMap<String, Known>) {
    let mut rels: Vec<&String> = known.keys().collect();
    rels.sort();
    let mut text = String::with_capacity(known.len() * 96);
    text.push_str(CACHE_HEADER);
    text.push('\n');
    for rel in rels {
        // A tab or newline in a file name cannot be written in this format.
        if rel.contains(['\t', '\n']) {
            continue;
        }
        let k = known[rel];
        let flags = format!("{}{}", if k.sniff.warp_shader { "w" } else { "" }, if k.sniff.comp_shader { "c" } else { "" });
        text.push_str(&format!("{rel}\t{}\t{}\t{}\t{flags}\n", k.size, k.mtime, k.sniff.version.code()));
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_reads_version_and_shaders() {
        let md2 = b"MILKDROP_PRESET_VERSION=201\nPSVERSION=2\n[preset00]\nzoom=1\nwarp_1=`shader_body {\ncomp_1=`shader_body {\n";
        assert_eq!(sniff(md2), Sniff { version: MilkVersion::Md2, warp_shader: true, comp_shader: true });
        let md1 = b"[preset00]\r\nfRating=3\r\nper_frame_1=zoom=1;\r\n";
        assert_eq!(sniff(md1), Sniff { version: MilkVersion::Md1, warp_shader: false, comp_shader: false });
        assert_eq!(sniff(b"not a preset").version, MilkVersion::Unknown);
    }

    #[test]
    fn a_second_walk_reads_only_what_changed() {
        let dir = std::env::temp_dir().join(format!("milkdrop-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("presets");
        fs::create_dir_all(root.join("Fractal/Sub")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("Fractal/Sub/Flexi - one.milk"), "MILKDROP_PRESET_VERSION=201\n[preset00]\nwarp_1=`x\n").unwrap();
        fs::write(root.join("Fractal/Geiss - two.milk"), "[preset00]\nzoom=1\n").unwrap();
        fs::write(root.join(".git/ignored.milk"), "[preset00]\n").unwrap();
        let cache = dir.join("index.txt");
        let (lib, stats) = scan_cached(&root, Some(&cache)).unwrap();
        assert_eq!((stats.files, stats.read, stats.reused), (2, 2, 0));
        assert_eq!(lib.entries[0].rel, "Fractal/Geiss - two.milk");
        assert_eq!(lib.entries[1].version, MilkVersion::Md2);
        assert!(lib.entries[1].warp_shader);
        let (_, stats) = scan_cached(&root, Some(&cache)).unwrap();
        assert_eq!((stats.read, stats.reused), (0, 2));
        fs::write(root.join("Fractal/Geiss - two.milk"), "MILKDROP_PRESET_VERSION=201\n[preset00]\ncomp_1=`x\nzoom=1.01\n").unwrap();
        fs::remove_file(root.join("Fractal/Sub/Flexi - one.milk")).unwrap();
        let (lib, stats) = scan_cached(&root, Some(&cache)).unwrap();
        assert_eq!((stats.files, stats.read, stats.reused), (1, 1, 0));
        assert!(lib.entries[0].comp_shader);
        assert_eq!(read_cache(&cache).len(), 1);
        assert!(scan_cached(&dir.join("missing"), None).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
