//! A native (rustc-built) binary read from its file: function symbols with their code bytes, and
//! its DWARF line table and inline tree when it has debug info (makepad inspect `asm ... vs=rustc`
//! and the native-baseline diff). The comparison build is made with line tables only, which
//! leaves the machine code as the plain release build makes it.
//!
//! macOS: the DWARF of a cargo build lives in `<bin>.dSYM/Contents/Resources/DWARF/<name>` (packed
//! split debug info), or -- unpacked -- in the object files the binary's debug map (N_OSO stabs)
//! names. Linux: in the ELF's own .debug_* sections.
//!
//! `open` reads the file, its section headers and its symbols. Line tables and inline trees are
//! parsed per compile unit the first time a query touches that unit (dwarf.rs), and kept.

mod dwarf;
mod obj;

use crate::inspect::symbols::demangle;
use dwarf::{Dbg, Interner, MapEnt, Secs};
use relax_backend::asm::Arch;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// One function of the binary's symbol table.
#[derive(Clone, Debug)]
pub struct NativeFn {
    /// virtual address (unslid, as in the file)
    pub addr: u64,
    pub size: u64,
    /// demangled, without the legacy `::h<hash>` suffix
    pub name: String,
}

/// One row of the line table: from `addr` until the next row's address the code is `file:line`.
#[derive(Clone, Debug)]
pub struct LineRow {
    pub addr: u64,
    /// index into `NativeBin::files`
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

/// One inlined call in the inline tree: code in `ranges` is fn `name`, inlined at
/// `call_file:call_line` into its parent (the enclosing InlinedCall, else the symbol's fn).
#[derive(Clone, Debug)]
pub struct InlinedCall {
    pub ranges: Vec<(u64, u64)>,
    pub name: String,
    pub call_file: u32,
    pub call_line: u32,
    /// nesting depth below the concrete fn (1 = inlined directly into it)
    pub depth: u32,
}

/// Debug info that is read on first use.
enum Src {
    Dbg(Dbg),
    /// a relocatable object named by the binary's debug map; `funcs` are (raw symbol, binary
    /// address, size), `ranges` the binary ranges they cover (sorted)
    Oso { path: String, funcs: Vec<(String, u64, u64)>, ranges: Vec<(u64, u64)> },
    Dead,
}

struct Lazy {
    files: Interner,
    srcs: Vec<Src>,
}

pub struct NativeBin {
    pub path: String,
    pub arch: Arch,
    /// sorted by address
    pub fns: Vec<NativeFn>,
    /// where the debug info came from ("dSYM", "object files", "ELF sections", "none")
    pub debug_from: String,
    data: Arc<Vec<u8>>,
    /// (address, file offset, size) of the executable sections, sorted by address
    text: Vec<(u64, usize, usize)>,
    lazy: Mutex<Lazy>,
}

fn intersects(r: &[(u64, u64)], lo: u64, hi: u64) -> bool {
    let i = r.partition_point(|x| x.1 <= lo);
    r.get(i).map_or(false, |x| x.0 < hi)
}

/// `name` without the generic arguments of its path segments (`a::f<T>` -> `a::f`; the `<T as X>`
/// of an impl path is kept).
fn strip_generics(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut depth = 0;
    let mut i = 0;
    while i < s.len() {
        let c = b[i];
        if depth == 0 {
            if c == b'<' && i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
                depth = 1;
            } else {
                // copy the whole char
                let n = s[i..].chars().next().map_or(1, |c| c.len_utf8());
                out.push_str(&s[i..i + n]);
                i += n;
                continue;
            }
        } else if c == b'<' {
            depth += 1;
        } else if c == b'>' && b[i - 1] != b'-' {
            depth -= 1;
        }
        i += 1;
    }
    out
}

fn dedupe(fns: &mut Vec<NativeFn>) {
    fns.sort_by(|a, b| (a.addr, &a.name).cmp(&(b.addr, &b.name)));
    fns.dedup_by(|a, b| a.addr == b.addr && a.name == b.name);
}

impl NativeBin {
    /// Reads the binary at `path` (thin or universal Mach-O: the host slice; ELF) and finds its debug
    /// info. Line tables and inline trees are read lazily per compile unit (`lines`, `inlined`).
    pub fn open(path: &str) -> Result<NativeBin, String> {
        NativeBin::open_arch(path, Arch::host())
    }

    /// `open`, taking the `want` slice of a universal binary.
    pub fn open_arch(path: &str, want: Arch) -> Result<NativeBin, String> {
        let mut data = std::fs::read(path).map_err(|e| format!("cannot read {}: {}", path, e))?;
        if obj::is_fat(&data) {
            let (off, size) = obj::fat_slice(&data, want).ok_or_else(|| format!("{}: no {:?} slice", path, want))?;
            if off.checked_add(size).map_or(true, |e| e > data.len()) {
                return Err(format!("{}: bad universal header", path));
            }
            data.truncate(off + size);
            data.drain(..off);
        }
        if obj::is_elf(&data) {
            NativeBin::open_elf(path, data)
        } else if obj::is_macho(&data) {
            NativeBin::open_macho(path, data)
        } else {
            Err(format!("{}: not an ELF or Mach-O 64-bit binary", path))
        }
    }

    fn open_elf(path: &str, data: Vec<u8>) -> Result<NativeBin, String> {
        let e = obj::parse_elf(&data).ok_or_else(|| format!("{}: unreadable ELF", path))?;
        let mut fns: Vec<NativeFn> = e.fns.iter().map(|f| NativeFn { addr: f.addr, size: f.size, name: demangle(&f.name) }).collect();
        dedupe(&mut fns);
        let mut text: Vec<(u64, usize, usize)> = e.secs.iter().filter(|s| s.flags & 4 != 0 && s.ty != 8 && s.size > 0).map(|s| (s.addr, s.off, s.size)).collect();
        text.sort();
        let mut secs = Secs::default();
        let mut compressed = false;
        for s in &e.secs {
            if s.name.starts_with(".debug_") {
                compressed |= s.flags & 0x800 != 0;
                secs.set(&s.name, (s.off, s.size));
            }
        }
        let data = Arc::new(data);
        let dbg = Dbg::new(data.clone(), secs, None);
        let (srcs, from) = if compressed {
            (Vec::new(), "none (compressed ELF debug sections are not read)")
        } else if dbg.has_info() {
            (vec![Src::Dbg(dbg)], "ELF sections")
        } else {
            (Vec::new(), "none")
        };
        Ok(NativeBin { path: path.to_string(), arch: e.arch, fns, debug_from: from.to_string(), data, text, lazy: Mutex::new(Lazy { files: Interner::default(), srcs }) })
    }

    fn open_macho(path: &str, data: Vec<u8>) -> Result<NativeBin, String> {
        let m = obj::parse_macho(&data).ok_or_else(|| format!("{}: unreadable Mach-O", path))?;
        // function boundaries: every symbol of a section bounds the one before it
        let mut bounds: HashMap<u8, Vec<u64>> = HashMap::new();
        for s in &m.syms {
            if s.ty & 0xe0 == 0 && s.ty & 0x0e == 0x0e && s.sect > 0 {
                bounds.entry(s.sect).or_default().push(s.value);
            }
        }
        for v in bounds.values_mut() {
            v.sort();
            v.dedup();
        }
        let is_text = |sect: u8| m.secs.get(sect as usize - 1).map_or(false, |s| s.seg == "__TEXT" && s.name == "__text");
        let mut fns = Vec::new();
        for s in &m.syms {
            if s.ty & 0xe0 != 0 || s.ty & 0x0e != 0x0e || s.sect == 0 || !is_text(s.sect) || !s.name.starts_with('_') {
                continue;
            }
            let sec = &m.secs[s.sect as usize - 1];
            let b = &bounds[&s.sect];
            let end = b.get(b.partition_point(|&x| x <= s.value)).copied().unwrap_or(sec.addr + sec.size as u64);
            fns.push(NativeFn { addr: s.value, size: end - s.value, name: demangle(s.name.strip_prefix('_').unwrap_or(&s.name)) });
        }
        dedupe(&mut fns);
        let mut text: Vec<(u64, usize, usize)> = m.secs.iter().filter(|s| s.seg == "__TEXT" && s.name == "__text").map(|s| (s.addr, s.off, s.size)).collect();
        text.sort();
        // debug info: a dSYM next to the binary, else the objects of the debug map, else the file itself
        let mut srcs = Vec::new();
        let mut from = "none";
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = format!("{}.dSYM/Contents/Resources/DWARF", path);
        let mut dsym = format!("{}/{}", dir, name);
        // cargo makes the dSYM for the deps/ file and links it next to the binary: the DWARF file
        // inside keeps the deps name (`krate-<hash>`); a dSYM holds one
        if !std::path::Path::new(&dsym).exists() {
            if let Some(only) = std::fs::read_dir(&dir).ok().and_then(|d| {
                let v: Vec<_> = d.flatten().map(|e| e.path()).collect();
                (v.len() == 1).then(|| v[0].to_string_lossy().into_owned())
            }) {
                dsym = only;
            }
        }
        if let Some(d) = NativeBin::load_dsym(&dsym, m.arch) {
            srcs.push(Src::Dbg(d));
            from = "dSYM";
        }
        if srcs.is_empty() {
            let mut osos: Vec<(String, Vec<(String, u64, u64)>)> = Vec::new();
            let mut pending: Option<(String, u64)> = None;
            for s in &m.syms {
                if s.ty & 0xe0 == 0 {
                    continue;
                }
                match s.ty {
                    0x66 => osos.push((s.name.clone(), Vec::new())),
                    0x24 if s.sect != 0 && !s.name.is_empty() => pending = Some((s.name.clone(), s.value)),
                    0x24 if s.name.is_empty() => {
                        if let (Some((n, a)), Some(o)) = (pending.take(), osos.last_mut()) {
                            o.1.push((n, a, s.value));
                        }
                    }
                    _ => {}
                }
            }
            for (p, funcs) in osos {
                if funcs.is_empty() {
                    continue;
                }
                let mut ranges: Vec<(u64, u64)> = funcs.iter().map(|f| (f.1, f.1 + f.2)).collect();
                ranges.sort();
                srcs.push(Src::Oso { path: p, funcs, ranges });
            }
            if !srcs.is_empty() {
                from = "object files";
            }
        }
        if srcs.is_empty() {
            let mut secs = Secs::default();
            for s in m.secs.iter().filter(|s| s.seg == "__DWARF") {
                secs.set(&s.name, (s.off, s.size));
            }
            let d = Dbg::new(Arc::new(data.clone()), secs, None);
            if d.has_info() {
                srcs.push(Src::Dbg(d));
                from = "binary sections";
            }
        }
        Ok(NativeBin { path: path.to_string(), arch: m.arch, fns, debug_from: from.to_string(), data: Arc::new(data), text, lazy: Mutex::new(Lazy { files: Interner::default(), srcs }) })
    }

    fn load_dsym(path: &str, want: Arch) -> Option<Dbg> {
        let mut data = std::fs::read(path).ok()?;
        if obj::is_fat(&data) {
            let (off, size) = obj::fat_slice(&data, want)?;
            data.truncate(off.checked_add(size)?);
            data.drain(..off);
        }
        let m = obj::parse_macho(&data)?;
        let mut secs = Secs::default();
        for s in m.secs.iter().filter(|s| s.seg == "__DWARF") {
            secs.set(&s.name, (s.off, s.size));
        }
        let d = Dbg::new(Arc::new(data), secs, None);
        d.has_info().then_some(d)
    }

    /// The debug info of the relocatable object `path` (`lib.rlib(member.o)` for archive members),
    /// mapped to binary addresses through the debug map's function entries.
    fn load_oso(path: &str, funcs: &[(String, u64, u64)]) -> Option<Dbg> {
        let (file, member) = match path.strip_suffix(')').and_then(|p| p.rsplit_once('(')) {
            Some((a, m)) => (a, Some(m)),
            None => (path, None),
        };
        let raw = std::fs::read(file).ok()?;
        let mut data = match member {
            Some(m) => obj::ar_member(&raw, m)?,
            None => raw,
        };
        let m = obj::parse_macho(&data)?;
        obj::apply_dwarf_relocs(&mut data, &m);
        let mut secs = Secs::default();
        for s in m.secs.iter().filter(|s| s.seg == "__DWARF") {
            secs.set(&s.name, (s.off, s.size));
        }
        let by_name: HashMap<&str, u64> = m.syms.iter().filter(|s| s.ty & 0xe0 == 0 && s.ty & 0x0e == 0x0e).map(|s| (s.name.as_str(), s.value)).collect();
        let mut map: Vec<MapEnt> = funcs.iter().filter_map(|(n, addr, size)| by_name.get(n.as_str()).map(|&v| MapEnt { lo: v, hi: v + size, to: *addr })).collect();
        map.sort_by_key(|e| e.lo);
        let d = Dbg::new(Arc::new(data), secs, Some(map));
        d.has_info().then_some(d)
    }

    /// The functions whose demangled name is `query` or ends with `::query` (generic arguments
    /// ignored when the query has none). Exact matches win; the suffix match is the fallback.
    pub fn find(&self, query: &str) -> Vec<&NativeFn> {
        let strip = !query.contains('<');
        let suffix = format!("::{}", query);
        let key = |f: &NativeFn| if strip { strip_generics(&f.name) } else { f.name.clone() };
        let keys: Vec<String> = self.fns.iter().map(key).collect();
        let exact: Vec<&NativeFn> = self.fns.iter().zip(&keys).filter(|(_, k)| k.as_str() == query).map(|(f, _)| f).collect();
        if !exact.is_empty() {
            return exact;
        }
        self.fns.iter().zip(&keys).filter(|(_, k)| k.ends_with(&suffix)).map(|(f, _)| f).collect()
    }

    /// The function containing `addr`.
    pub fn fn_at(&self, addr: u64) -> Option<&NativeFn> {
        let i = self.fns.partition_point(|f| f.addr <= addr);
        let f = self.fns.get(i.checked_sub(1)?)?;
        (addr < f.addr + f.size).then_some(f)
    }

    /// The code bytes of `f`.
    pub fn code(&self, f: &NativeFn) -> &[u8] {
        let i = self.text.partition_point(|t| t.0 <= f.addr);
        let Some(&(addr, off, size)) = i.checked_sub(1).and_then(|i| self.text.get(i)) else { return &[] };
        let start = (f.addr - addr) as usize;
        if start >= size {
            return &[];
        }
        let end = (start + f.size as usize).min(size);
        self.data.get(off + start..off + end).unwrap_or(&[])
    }

    /// The source file path of `LineRow::file` / `InlinedCall::call_file` index `i`.
    pub fn file(&self, i: u32) -> String {
        self.lazy.lock().unwrap().files.names.get(i as usize).cloned().unwrap_or_default()
    }

    /// Every source file path seen so far (grows as queries touch more compile units).
    pub fn files(&self) -> Vec<String> {
        self.lazy.lock().unwrap().files.names.clone()
    }

    fn each_dbg(&self, lo: u64, hi: u64, mut f: impl FnMut(&mut Dbg, &mut Interner)) {
        let mut g = self.lazy.lock().unwrap();
        let Lazy { files, srcs } = &mut *g;
        for s in srcs.iter_mut() {
            let load = match s {
                Src::Oso { path, funcs, ranges } if intersects(ranges, lo, hi) => Some(NativeBin::load_oso(path, funcs)),
                Src::Oso { .. } => continue,
                _ => None,
            };
            if let Some(d) = load {
                *s = d.map(Src::Dbg).unwrap_or(Src::Dead);
            }
            if let Src::Dbg(d) = s {
                f(d, files);
            }
        }
    }

    /// Line rows covering [lo, hi), sorted by address (empty without debug info). The row that
    /// covers `lo` is included even when it starts before it. A row with `line` 0 ends a sequence.
    pub fn lines(&self, lo: u64, hi: u64) -> Vec<LineRow> {
        let mut out: Vec<LineRow> = Vec::new();
        self.each_dbg(lo, hi, |d, files| {
            for ci in d.cus_for(lo, hi) {
                let Some(cd) = d.cu_data(ci, files) else { continue };
                let rows = &cd.rows;
                let i0 = rows.partition_point(|r| r.addr <= lo).saturating_sub(1);
                let i1 = rows.partition_point(|r| r.addr < hi);
                if i0 < i1 {
                    out.extend(rows[i0..i1].iter().filter(|r| r.addr >= lo || r.line != 0).cloned());
                }
            }
        });
        out.sort_by_key(|r| r.addr);
        out.dedup_by_key(|r| r.addr);
        out
    }

    /// The inlined calls inside [lo, hi) (the concrete fn's inline tree), outermost first.
    pub fn inlined(&self, lo: u64, hi: u64) -> Vec<InlinedCall> {
        let mut out = Vec::new();
        self.each_dbg(lo, hi, |d, files| {
            for ci in d.cus_for(lo, hi) {
                let Some(cd) = d.cu_data(ci, files) else { continue };
                for s in cd.subs.iter().filter(|s| s.ranges.iter().any(|r| r.0 < hi && r.1 > lo)) {
                    out.extend(s.inl.iter().filter(|c| c.ranges.iter().any(|r| r.0 < hi && r.1 > lo)).cloned());
                }
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const FIXTURE: &str = "use std::hint::black_box;
#[inline(always)]
fn helper_inl(x: u64) -> u64 {
    black_box(x).wrapping_mul(31).wrapping_add(7)
}
#[inline(never)]
pub fn natbin_outer(x: u64) -> u64 {
    let a = helper_inl(x);
    a ^ (a >> 3)
}
fn main() {
    println!(\"{}\", natbin_outer(std::env::args().count() as u64));
}
";

    fn have_rustc() -> bool {
        Command::new("rustc").arg("--version").output().map_or(false, |o| o.status.success())
    }

    /// Builds the fixture with the system compiler into a fresh temp dir; returns (dir, binary).
    fn build(tag: &str, flags: &[&str]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("natbin-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fixture.rs"), FIXTURE).unwrap();
        let st = Command::new("rustc").current_dir(&dir).args(["-O", "-C", "codegen-units=1", "fixture.rs", "-o", "fixture_bin"]).args(flags).status().unwrap();
        assert!(st.success(), "rustc failed");
        (dir.clone(), dir.join("fixture_bin"))
    }

    fn check(tag: &str, flags: &[&str], want_inline: bool) {
        if !have_rustc() {
            eprintln!("natbin test skipped: no rustc");
            return;
        }
        let (dir, bin) = build(tag, flags);
        let nb = NativeBin::open(bin.to_str().unwrap()).unwrap();
        let hits = nb.find("natbin_outer");
        assert_eq!(hits.len(), 1, "fns: {:?}", hits);
        let f = hits[0];
        assert!(f.name.ends_with("natbin_outer") && f.size > 0, "{:?}", f);
        assert_eq!(nb.code(f).len() as u64, f.size);
        assert!(nb.fn_at(f.addr + 1).map(|g| g.addr) == Some(f.addr));
        // the same bytes through the system disassembler's section dump
        #[cfg(target_os = "linux")]
        if let Ok(o) = Command::new("objdump").args(["-s", &format!("--start-address={}", f.addr), &format!("--stop-address={}", f.addr + f.size), bin.to_str().unwrap()]).output() {
            let mut hex = Vec::new();
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                // " 401120 f30f1efa 4889f8 ...  text"
                let mut it = l.split_whitespace();
                if it.next().map_or(false, |a| u64::from_str_radix(a, 16).is_ok()) {
                    for w in it.take(4) {
                        if w.len() % 2 == 0 && w.bytes().all(|c| c.is_ascii_hexdigit()) {
                            hex.extend((0..w.len() / 2).map(|i| u8::from_str_radix(&w[2 * i..2 * i + 2], 16).unwrap()));
                        }
                    }
                }
            }
            assert_eq!(&hex[..], nb.code(f), "code bytes differ from the system dump");
        }
        assert_ne!(nb.debug_from, "none", "no debug info found");
        let rows = nb.lines(f.addr, f.addr + f.size);
        assert!(!rows.is_empty());
        assert!(rows.windows(2).all(|w| w[0].addr < w[1].addr));
        let ours: Vec<&LineRow> = rows.iter().filter(|r| nb.file(r.file).ends_with("fixture.rs")).collect();
        assert!(!ours.is_empty(), "no fixture rows: {:?}", rows.iter().map(|r| (nb.file(r.file), r.line)).collect::<Vec<_>>());
        assert!(ours.iter().all(|r| (7..=10).contains(&r.line) || (3..=5).contains(&r.line) || r.line == 0), "{:?}", ours);
        assert!(ours.iter().any(|r| r.line == 8 || r.line == 9));
        if want_inline {
            let call_line = FIXTURE.lines().position(|l| l.contains("helper_inl(x);")).unwrap() as u32 + 1;
            let inl = nb.inlined(f.addr, f.addr + f.size);
            let h = inl.iter().find(|c| c.name.ends_with("helper_inl")).unwrap_or_else(|| panic!("no helper_inl in {:?}", inl));
            assert_eq!(h.call_line, call_line);
            assert!(nb.file(h.call_file).ends_with("fixture.rs"));
            assert_eq!(h.depth, 1);
            assert!(h.ranges.iter().all(|r| r.0 >= f.addr && r.1 <= f.addr + f.size));
            // black_box / wrapping ops inlined into the helper are deeper
            assert!(inl.iter().all(|c| c.depth >= 1));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn variants() -> Vec<(&'static str, Vec<&'static str>)> {
        if cfg!(target_vendor = "apple") {
            vec![("unpacked", vec!["-C", "split-debuginfo=unpacked"]), ("packed", vec!["-C", "split-debuginfo=packed"])]
        } else {
            vec![("plain", vec![])]
        }
    }

    #[test]
    fn line_tables_only() {
        for (tag, extra) in variants() {
            let mut fl = vec!["-C", "debuginfo=line-tables-only"];
            fl.extend(extra);
            check(&format!("lt-{}", tag), &fl, false);
        }
    }

    #[test]
    fn full_debuginfo_inline_tree() {
        for (tag, extra) in variants() {
            let mut fl = vec!["-C", "debuginfo=2"];
            fl.extend(extra);
            check(&format!("full-{}", tag), &fl, true);
        }
    }

    #[test]
    fn dwarf4_and_5() {
        if !have_rustc() {
            return;
        }
        for v in ["4", "5"] {
            let flag = format!("-Zdwarf-version={}", v);
            // -Z flags need nightly (or RUSTC_BOOTSTRAP); skip when the compiler refuses
            let (dir, bin) = {
                let dir = std::env::temp_dir().join(format!("natbin-test-{}-dw{}", std::process::id(), v));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("fixture.rs"), FIXTURE).unwrap();
                let st = Command::new("rustc").env("RUSTC_BOOTSTRAP", "1").current_dir(&dir).args(["-O", "-C", "debuginfo=2", &flag, "fixture.rs", "-o", "fixture_bin"]).status().unwrap();
                if !st.success() {
                    continue;
                }
                (dir.clone(), dir.join("fixture_bin"))
            };
            let nb = NativeBin::open(bin.to_str().unwrap()).unwrap();
            let f = nb.find("natbin_outer")[0].clone();
            assert!(!nb.lines(f.addr, f.addr + f.size).is_empty(), "dwarf {}", v);
            assert!(nb.inlined(f.addr, f.addr + f.size).iter().any(|c| c.name.ends_with("helper_inl")), "dwarf {}", v);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn find_rules() {
        assert_eq!(strip_generics("a::f<T, U>::g<V>"), "a::f::g");
        assert_eq!(strip_generics("<T as X>::m<Y>"), "<T as X>::m");
        assert_eq!(strip_generics("a::f<fn(A) -> B>"), "a::f");
        assert_eq!(strip_generics("a::{closure#0}"), "a::{closure#0}");
    }

    /// NATBIN_CORPUS=<binary>: compares our line rows with the system disassembler's `-l` listing
    /// for a sample of the binary's functions.
    #[test]
    fn corpus_agreement() {
        let Ok(path) = std::env::var("NATBIN_CORPUS") else { return };
        let t0 = std::time::Instant::now();
        let nb = NativeBin::open(&path).unwrap();
        eprintln!("corpus: {} fns, debug from {}, open {:?}", nb.fns.len(), nb.debug_from, t0.elapsed());
        if let Some(f) = nb.fns.iter().find(|f| f.size > 64) {
            let t1 = std::time::Instant::now();
            let n = nb.lines(f.addr, f.addr + f.size).len();
            eprintln!("corpus: first lines() {} rows in {:?} (includes the unit index), second {:?}", n, t1.elapsed(), { let t2 = std::time::Instant::now(); nb.lines(f.addr, f.addr + f.size); t2.elapsed() });
        }
        let (mut total, mut same, mut fns_done, mut zero) = (0usize, 0usize, 0usize, 0usize);
        let step = (nb.fns.len() / 400).max(1);
        for f in nb.fns.iter().step_by(step) {
            if fns_done >= 60 || f.size < 24 {
                continue;
            }
            let rows = nb.lines(f.addr, f.addr + f.size);
            if rows.is_empty() {
                continue;
            }
            let Ok(o) = Command::new("objdump").args(["-d", "-l", "--no-show-raw-insn", &format!("--start-address={}", f.addr), &format!("--stop-address={}", f.addr + f.size), &path]).output() else { return };
            let mut cur: Option<(String, u32)> = None;
            fns_done += 1;
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                let t = l.trim_end();
                if let Some((p, rest)) = t.rsplit_once(':') {
                    let n = rest.split_whitespace().next().unwrap_or("");
                    if !t.starts_with(' ') && !p.is_empty() && !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) {
                        cur = Some((p.to_string(), n.parse().unwrap()));
                        continue;
                    }
                }
                let Some((a, _)) = t.trim_start().split_once(":\t") else { continue };
                let Ok(addr) = u64::from_str_radix(a, 16) else { continue };
                let Some((file, line)) = &cur else { continue };
                let i = rows.partition_point(|r| r.addr <= addr);
                if let Some(r) = i.checked_sub(1).map(|i| &rows[i]) {
                    if r.line == 0 {
                        zero += 1; // no source line: the system listing carries the previous line over
                        continue;
                    }
                    total += 1;
                    let ours = nb.file(r.file);
                    if r.line == *line && Path::new(&ours).file_name() == Path::new(file).file_name() {
                        same += 1;
                    } else if total - same < 25 {
                        eprintln!("mismatch {:x} {}: ours {}:{} theirs {}:{}", addr, f.name, ours, r.line, file, line);
                    }
                }
            }
        }
        eprintln!("corpus: {} instructions in line-0 rows skipped", zero);
        eprintln!("corpus agreement: {}/{} instructions over {} fns ({:.2}%)", same, total, fns_done, 100.0 * same as f64 / total.max(1) as f64);
        assert!(total == 0 || same * 100 >= total * 90);
    }
}
