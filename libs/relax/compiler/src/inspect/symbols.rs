//! Names of host (compiled-in) code addresses: relax itself, the provided crates, std, libc and
//! the system libraries (DESIGN.md §3.4). `dladdr` only knows exported symbols, and the relax
//! binary exports none of its Rust functions, so each loaded image's own symbol table is read from
//! its file once (ELF .symtab / .dynsym, Mach-O LC_SYMTAB); images that exist only inside the dyld
//! shared cache (macOS system libraries) fall back to dladdr. Names are demangled (Rust legacy and
//! v0). Used after a captured thread runs again: it allocates and locks freely.

use relax_session::sync::Mutex;

struct Image {
    /// address range of its executable code (Linux: PT_LOAD with PF_X; macOS: __TEXT)
    lo: u64,
    hi: u64,
    path: String,
    /// load bias (ELF) / slide (Mach-O)
    bias: u64,
    /// (address, size or 0, name) sorted by address; None until first used, Some(empty) when the
    /// file could not be read
    syms: Option<Vec<(u64, u64, String)>>,
}

static IMAGES: Mutex<Vec<Image>> = Mutex::new(Vec::new());

/// The function containing `pc`: (demangled name, image file name, pc's offset from the symbol).
pub fn host_symbol(pc: u64) -> Option<(String, String, u64)> {
    let mut images = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    if !images.iter().any(|i| pc >= i.lo && pc < i.hi) {
        *images = enumerate();
    }
    let img = images.iter_mut().find(|i| pc >= i.lo && pc < i.hi)?;
    if img.syms.is_none() {
        let mut v = std::fs::read(&img.path).ok().map(|b| read_symbols(&b, img.bias)).unwrap_or_default();
        v.sort_by_key(|s| s.0);
        img.syms = Some(v);
    }
    let base = img.path.rsplit('/').next().unwrap_or("").to_string();
    let syms = img.syms.as_ref().unwrap();
    let k = syms.partition_point(|s| s.0 <= pc);
    if k > 0 {
        let (a, size, name) = &syms[k - 1];
        let end = if *size != 0 { a + size } else { syms.get(k).map_or(u64::MAX, |s| s.0) };
        if pc < end {
            return Some((demangle(name), base, pc - a));
        }
    }
    let lo = img.lo;
    drop(images);
    // no symbol in the file (a stripped library): dladdr's exported one, else image+offset
    Some(dladdr_name(pc).map(|(n, a)| (demangle(&n), base.clone(), pc - a)).unwrap_or_else(|| (format!("{}+{:#x}", base, pc - lo), base, 0)))
}

fn dladdr_name(pc: u64) -> Option<(String, u64)> {
    crate::os::proc::symbol_at(pc).map(|(name, addr, _)| (name, addr))
}

// ------------------------------------------------------------------ loaded images

/// glibc's `struct dl_phdr_info` (the members used)
#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
pub(crate) struct PhdrInfo {
    pub addr: u64,
    pub name: *const std::ffi::c_char,
    pub phdr: *const u8,
    pub phnum: u16,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
extern "C" {
    pub(crate) fn dl_iterate_phdr(cb: extern "C" fn(*const PhdrInfo, usize, *mut u8) -> i32, data: *mut u8) -> i32;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn enumerate() -> Vec<Image> {
    extern "C" fn cb(info: *const PhdrInfo, _size: usize, data: *mut u8) -> i32 {
        unsafe {
            let out = &mut *(data as *mut Vec<Image>);
            let i = &*info;
            let name = if i.name.is_null() { String::new() } else { std::ffi::CStr::from_ptr(i.name).to_string_lossy().into_owned() };
            let path = if name.is_empty() { std::fs::read_link("/proc/self/exe").map(|p| p.display().to_string()).unwrap_or_default() } else { name };
            for k in 0..i.phnum as usize {
                // Elf64_Phdr: type u32, flags u32, offset, vaddr, paddr, filesz, memsz, align
                let ph = i.phdr.add(k * 56);
                let ty = *(ph as *const u32);
                let flags = *(ph.add(4) as *const u32);
                if ty == 1 && flags & 1 != 0 {
                    let vaddr = *(ph.add(16) as *const u64);
                    let memsz = *(ph.add(40) as *const u64);
                    out.push(Image { lo: i.addr + vaddr, hi: i.addr + vaddr + memsz, path: path.clone(), bias: i.addr, syms: None });
                }
            }
        }
        0
    }
    let mut v: Vec<Image> = Vec::new();
    unsafe { dl_iterate_phdr(cb, &mut v as *mut Vec<Image> as *mut u8) };
    v
}

#[cfg(target_os = "macos")]
fn enumerate() -> Vec<Image> {
    extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_header(i: u32) -> *const u8;
        fn _dyld_get_image_vmaddr_slide(i: u32) -> isize;
        fn _dyld_get_image_name(i: u32) -> *const i8;
    }
    let mut v = Vec::new();
    unsafe {
        for i in 0.._dyld_image_count() {
            let h = _dyld_get_image_header(i);
            if h.is_null() {
                continue;
            }
            let slide = _dyld_get_image_vmaddr_slide(i) as u64;
            let name = std::ffi::CStr::from_ptr(_dyld_get_image_name(i)).to_string_lossy().into_owned();
            // mach_header_64: magic, cputype, cpusubtype, filetype, ncmds, sizeofcmds, flags, reserved
            let ncmds = *(h.add(16) as *const u32);
            // MH_DYLIB_IN_CACHE: the image lives in the dyld shared cache, laid out unlike its
            // file on disk (the file's symbol addresses name other code): no file symbols, dladdr
            let in_cache = *(h.add(24) as *const u32) & 0x8000_0000 != 0;
            let mut p = h.add(32);
            for _ in 0..ncmds {
                let cmd = *(p as *const u32);
                let size = *(p.add(4) as *const u32) as usize;
                // LC_SEGMENT_64: segname[16] at 8, vmaddr at 24, vmsize at 32
                if cmd == 0x19 {
                    let seg = std::slice::from_raw_parts(p.add(8), 16);
                    if seg.starts_with(b"__TEXT\0") {
                        let vmaddr = *(p.add(24) as *const u64);
                        let vmsize = *(p.add(32) as *const u64);
                        v.push(Image { lo: vmaddr.wrapping_add(slide), hi: vmaddr.wrapping_add(slide) + vmsize, path: name.clone(), bias: slide, syms: if in_cache { Some(Vec::new()) } else { None } });
                    }
                }
                p = p.add(size);
            }
        }
    }
    v
}

#[cfg(windows)]
fn enumerate() -> Vec<Image> {
    Vec::new()
}

// ------------------------------------------------------------------ symbol tables

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}
fn cstr(b: &[u8], o: usize) -> Option<&str> {
    let s = b.get(o..)?;
    let n = s.iter().position(|&c| c == 0)?;
    std::str::from_utf8(&s[..n]).ok()
}

/// Function symbols of an image file: (runtime address, size, raw name).
fn read_symbols(b: &[u8], bias: u64) -> Vec<(u64, u64, String)> {
    if b.starts_with(b"\x7fELF") {
        elf_symbols(b, bias).unwrap_or_default()
    } else if b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) {
        macho_symbols(b, bias).unwrap_or_default()
    } else if b.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) {
        // a universal binary: the arm64 slice
        let n = u32::from_be_bytes(b.get(4..8).and_then(|x| x.try_into().ok()).unwrap_or([0; 4]));
        for k in 0..n as usize {
            let at = 8 + k * 20;
            let Some(e) = b.get(at..at + 20) else { break };
            let cpu = u32::from_be_bytes(e[0..4].try_into().unwrap());
            let off = u32::from_be_bytes(e[8..12].try_into().unwrap()) as usize;
            let size = u32::from_be_bytes(e[12..16].try_into().unwrap()) as usize;
            if cpu == 0x0100_000c {
                if let Some(slice) = b.get(off..off + size) {
                    return macho_symbols(slice, bias).unwrap_or_default();
                }
            }
        }
        Vec::new()
    } else {
        Vec::new()
    }
}

fn elf_symbols(b: &[u8], bias: u64) -> Option<Vec<(u64, u64, String)>> {
    let shoff = u64_at(b, 0x28)? as usize;
    let shentsize = u16_at(b, 0x3a)? as usize;
    let shnum = u16_at(b, 0x3c)? as usize;
    let sec = |i: usize| -> Option<(u32, usize, usize, u32, usize)> {
        let o = shoff + i * shentsize;
        // (type, offset, size, link, entsize)
        Some((u32_at(b, o + 4)?, u64_at(b, o + 0x18)? as usize, u64_at(b, o + 0x20)? as usize, u32_at(b, o + 0x28)?, u64_at(b, o + 0x38)? as usize))
    };
    let mut out = Vec::new();
    // SHT_SYMTAB (2) when present, else SHT_DYNSYM (11)
    let tables: Vec<usize> = (0..shnum).filter(|&i| sec(i).map_or(false, |s| s.0 == 2)).collect();
    let tables = if tables.is_empty() { (0..shnum).filter(|&i| sec(i).map_or(false, |s| s.0 == 11)).collect() } else { tables };
    for t in tables {
        let (_, off, size, link, ent) = sec(t)?;
        let (_, stroff, _, _, _) = sec(link as usize)?;
        let ent = if ent == 0 { 24 } else { ent };
        for k in 0..size / ent {
            let o = off + k * ent;
            let info = *b.get(o + 4)?;
            let shndx = u16_at(b, o + 6)?;
            if info & 0xf != 2 || shndx == 0 {
                continue; // STT_FUNC, defined
            }
            let value = u64_at(b, o + 8)?;
            let sz = u64_at(b, o + 16)?;
            if let Some(name) = cstr(b, stroff + u32_at(b, o)? as usize) {
                out.push((bias + value, sz, name.to_string()));
            }
        }
    }
    Some(out)
}

fn macho_symbols(b: &[u8], slide: u64) -> Option<Vec<(u64, u64, String)>> {
    let ncmds = u32_at(b, 16)? as usize;
    let mut p = 32usize;
    let mut out = Vec::new();
    for _ in 0..ncmds {
        let cmd = u32_at(b, p)?;
        let size = u32_at(b, p + 4)? as usize;
        // LC_SYMTAB: symoff, nsyms, stroff, strsize
        if cmd == 2 {
            let (symoff, nsyms, stroff) = (u32_at(b, p + 8)? as usize, u32_at(b, p + 12)? as usize, u32_at(b, p + 16)? as usize);
            for k in 0..nsyms {
                // nlist_64: strx u32, type u8, sect u8, desc u16, value u64
                let o = symoff + k * 16;
                let ty = *b.get(o + 4)?;
                if ty & 0xe0 != 0 || ty & 0x0e != 0x0e {
                    continue; // stabs, or not N_SECT
                }
                let value = u64_at(b, o + 8)?;
                if let Some(name) = cstr(b, stroff + u32_at(b, o)? as usize) {
                    out.push((value.wrapping_add(slide), 0, name.strip_prefix('_').unwrap_or(name).to_string()));
                }
            }
        }
        p += size;
    }
    Some(out)
}

// ------------------------------------------------------------------ demangling

/// A Rust symbol in source form (legacy `_ZN..E` without its hash, v0 `_R..`); anything else as is.
pub fn demangle(s: &str) -> String {
    let t = s.strip_prefix('_').unwrap_or(s);
    if let Some(r) = t.strip_prefix("ZN").or_else(|| s.strip_prefix("ZN")) {
        if let Some(d) = legacy(r) {
            return d;
        }
    }
    if let Some(r) = s.strip_prefix("_R").or_else(|| s.strip_prefix("R")) {
        let mut p = V0 { s: r.as_bytes(), i: 0, out: String::new(), depth: 0, in_value: true };
        if p.path().is_some() {
            return p.out;
        }
    }
    s.to_string()
}

fn legacy(r: &str) -> Option<String> {
    let b = r.as_bytes();
    let mut i = 0;
    let mut parts: Vec<String> = Vec::new();
    while i < b.len() && b[i] != b'E' {
        let mut n = 0usize;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            n = n * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        if i == start || i + n > b.len() {
            return None;
        }
        parts.push(r[i..i + n].to_string());
        i += n;
    }
    if let Some(last) = parts.last() {
        if last.len() == 17 && last.starts_with('h') && last[1..].bytes().all(|c| c.is_ascii_hexdigit()) {
            parts.pop();
        }
    }
    let parts: Vec<String> = parts.iter().map(|p| unescape(p)).collect();
    Some(parts.join("::"))
}

fn unescape(p: &str) -> String {
    let p = p.strip_prefix("_$").map(|x| format!("${}", x)).unwrap_or_else(|| p.to_string());
    let mut out = String::new();
    let mut rest = p.as_str();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("..") {
            out.push_str("::");
            rest = r;
        } else if rest.starts_with('$') {
            let end = rest[1..].find('$').map(|k| k + 1);
            let Some(end) = end else {
                out.push_str(rest);
                break;
            };
            let code = &rest[1..end];
            let rep = match code {
                "LT" => "<".to_string(),
                "GT" => ">".to_string(),
                "RF" => "&".to_string(),
                "BP" => "*".to_string(),
                "C" => ",".to_string(),
                "SP" => "@".to_string(),
                "LP" => "(".to_string(),
                "RP" => ")".to_string(),
                c if c.starts_with('u') => u32::from_str_radix(&c[1..], 16).ok().and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_else(|| format!("${}$", c)),
                c => format!("${}$", c),
            };
            out.push_str(&rep);
            rest = &rest[end + 1..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The v0 grammar (rustc-demangle's subset that real symbols use): paths, generic args, types,
/// constants as their raw form, backrefs.
struct V0<'a> {
    s: &'a [u8],
    i: usize,
    out: String,
    depth: u32,
    /// a path in an expression (`Vec::<u8>::new`) spells its generic arguments with `::<`; one
    /// in a type (`<Vec<u8>>::push`, `<T as Trait<u8>>`) with `<` (rustc-demangle `in_value`)
    in_value: bool,
}

impl<'a> V0<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    /// base-62 number ending in `_` (`_` alone = 0)
    fn base62(&mut self) -> Option<u64> {
        if self.eat(b'_') {
            return Some(0);
        }
        let mut x: u64 = 0;
        loop {
            let c = self.next()?;
            if c == b'_' {
                return Some(x.wrapping_add(1));
            }
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'z' => 10 + c - b'a',
                b'A'..=b'Z' => 36 + c - b'A',
                _ => return None,
            } as u64;
            // disambiguators are 64-bit hashes (their value is not shown): wrapping is fine
            x = x.wrapping_mul(62).wrapping_add(d);
        }
    }
    fn disambiguator(&mut self) -> Option<()> {
        if self.eat(b's') {
            self.base62()?;
        }
        Some(())
    }
    fn ident(&mut self) -> Option<String> {
        let punycode = self.eat(b'u');
        let mut n = 0usize;
        let start = self.i;
        while let Some(c) = self.peek().filter(|c| c.is_ascii_digit()) {
            n = n * 10 + (c - b'0') as usize;
            self.i += 1;
        }
        if self.i == start {
            return None;
        }
        self.eat(b'_');
        let s = std::str::from_utf8(self.s.get(self.i..self.i + n)?).ok()?.to_string();
        self.i += n;
        Some(if punycode { format!("{}(punycode)", s) } else { s })
    }
    fn backref<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let at = self.base62()? as usize;
        if self.depth > 64 {
            return None;
        }
        let save = self.i;
        self.i = at;
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        self.i = save;
        r
    }
    /// A path in type position (a type, a trait): generic arguments without `::`.
    fn type_path(&mut self) -> Option<()> {
        let in_value = std::mem::replace(&mut self.in_value, false);
        let r = self.path();
        self.in_value = in_value;
        r
    }
    fn path(&mut self) -> Option<()> {
        self.depth += 1;
        if self.depth > 200 {
            return None;
        }
        let r = self.path_inner();
        self.depth -= 1;
        r
    }
    fn path_inner(&mut self) -> Option<()> {
        match self.next()? {
            b'C' => {
                self.disambiguator()?;
                let id = self.ident()?;
                self.out.push_str(&id);
            }
            b'N' => {
                let ns = self.next()?;
                self.path()?;
                self.disambiguator()?;
                let id = self.ident()?;
                match ns {
                    b'C' => {
                        self.out.push_str("::{closure");
                        if !id.is_empty() {
                            self.out.push(':');
                            self.out.push_str(&id);
                        }
                        self.out.push('}');
                    }
                    b'S' => {
                        self.out.push_str("::{shim");
                        if !id.is_empty() {
                            self.out.push(':');
                            self.out.push_str(&id);
                        }
                        self.out.push('}');
                    }
                    _ => {
                        if !id.is_empty() {
                            self.out.push_str("::");
                            self.out.push_str(&id);
                        }
                    }
                }
            }
            b'M' => {
                self.disambiguator()?;
                // the impl's own path is not printed (rustc-demangle shows `<Type>`)
                let save = std::mem::take(&mut self.out);
                self.path()?;
                self.out = save;
                self.out.push('<');
                self.ty()?;
                self.out.push('>');
            }
            b'X' => {
                self.disambiguator()?;
                let save = std::mem::take(&mut self.out);
                self.path()?;
                self.out = save;
                self.out.push('<');
                self.ty()?;
                self.out.push_str(" as ");
                self.type_path()?;
                self.out.push('>');
            }
            b'Y' => {
                self.out.push('<');
                self.ty()?;
                self.out.push_str(" as ");
                self.type_path()?;
                self.out.push('>');
            }
            b'I' => {
                self.path()?;
                self.out.push_str(if self.in_value { "::<" } else { "<" });
                let mut first = true;
                while !self.eat(b'E') {
                    if !first {
                        self.out.push_str(", ");
                    }
                    first = false;
                    self.generic_arg()?;
                }
                self.out.push('>');
            }
            b'B' => {
                self.backref(|p| p.path())?;
            }
            _ => return None,
        }
        Some(())
    }
    fn generic_arg(&mut self) -> Option<()> {
        if self.eat(b'L') {
            self.base62()?;
            self.out.push_str("'_");
            Some(())
        } else if self.eat(b'K') {
            self.konst()
        } else {
            self.ty()
        }
    }
    fn konst(&mut self) -> Option<()> {
        if self.eat(b'B') {
            return self.backref(|p| p.konst());
        }
        if self.eat(b'p') {
            self.out.push('_');
            return Some(());
        }
        let _ty = self.next()?;
        let neg = self.eat(b'n');
        let start = self.i;
        while self.peek().map_or(false, |c| c != b'_') {
            self.i += 1;
        }
        let hex = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        self.eat(b'_');
        let v = u128::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).ok()?;
        self.out.push_str(&format!("{}{}", if neg { "-" } else { "" }, v));
        Some(())
    }
    fn ty(&mut self) -> Option<()> {
        self.depth += 1;
        if self.depth > 200 {
            return None;
        }
        let c = self.peek()?;
        let basic = match c {
            b'a' => Some("i8"),
            b'b' => Some("bool"),
            b'c' => Some("char"),
            b'd' => Some("f64"),
            b'e' => Some("str"),
            b'f' => Some("f32"),
            b'h' => Some("u8"),
            b'i' => Some("isize"),
            b'j' => Some("usize"),
            b'l' => Some("i32"),
            b'm' => Some("u32"),
            b'n' => Some("i128"),
            b'o' => Some("u128"),
            b's' => Some("i16"),
            b't' => Some("u16"),
            b'u' => Some("()"),
            b'v' => Some("..."),
            b'x' => Some("i64"),
            b'y' => Some("u64"),
            b'z' => Some("!"),
            b'p' => Some("_"),
            _ => None,
        };
        let r = if let Some(b) = basic {
            self.i += 1;
            self.out.push_str(b);
            Some(())
        } else {
            self.i += 1;
            match c {
                b'R' | b'Q' => {
                    if self.peek() == Some(b'L') {
                        self.i += 1;
                        self.base62()?;
                    }
                    self.out.push_str(if c == b'R' { "&" } else { "&mut " });
                    self.ty()
                }
                b'P' | b'O' => {
                    self.out.push_str(if c == b'P' { "*const " } else { "*mut " });
                    self.ty()
                }
                b'A' => {
                    self.out.push('[');
                    self.ty()?;
                    self.out.push_str("; ");
                    self.konst()?;
                    self.out.push(']');
                    Some(())
                }
                b'S' => {
                    self.out.push('[');
                    self.ty()?;
                    self.out.push(']');
                    Some(())
                }
                b'T' => {
                    self.out.push('(');
                    let mut n = 0;
                    while !self.eat(b'E') {
                        if n > 0 {
                            self.out.push_str(", ");
                        }
                        self.ty()?;
                        n += 1;
                    }
                    if n == 1 {
                        self.out.push(',');
                    }
                    self.out.push(')');
                    Some(())
                }
                b'F' => {
                    // fn pointer: abi and argument list kept short
                    self.out.push_str("fn(..)");
                    let mut depth = 1;
                    while depth > 0 {
                        match self.next()? {
                            b'E' => depth -= 1,
                            _ => {}
                        }
                    }
                    Some(())
                }
                b'D' => {
                    self.out.push_str("dyn ");
                    let mut first = true;
                    while !self.eat(b'E') {
                        if !first {
                            self.out.push_str(" + ");
                        }
                        first = false;
                        self.path()?;
                        while self.eat(b'p') {
                            self.ident()?;
                            self.ty()?;
                        }
                    }
                    if self.eat(b'L') {
                        self.base62()?;
                    }
                    Some(())
                }
                b'B' => self.backref(|p| p.ty()),
                _ => {
                    self.i -= 1;
                    self.type_path()
                }
            }
        };
        self.depth -= 1;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::demangle;

    #[test]
    fn legacy() {
        assert_eq!(demangle("_ZN4core3ptr13drop_in_place17h0123456789abcdefE"), "core::ptr::drop_in_place");
        assert_eq!(demangle("_ZN66_$LT$alloc..vec..Vec$LT$T$GT$$u20$as$u20$core..ops..drop..Drop$GT$4drop17h0123456789abcdefE"), "<alloc::vec::Vec<T> as core::ops::drop::Drop>::drop");
        assert_eq!(demangle("malloc"), "malloc");
    }

    #[test]
    fn v0() {
        assert_eq!(demangle("_RNvCs1234_5hello4main"), "hello::main");
        assert_eq!(demangle("_RNvMNtCs1234_5alloc3vecINtB2_3VechE4push"), "<alloc::vec::Vec<u8>>::push");
        // a generic fn in an expression keeps its turbofish; a trait in a type does not
        assert_eq!(demangle("_RINvNtCs1234_4core3mem4swaphEB2_"), "core::mem::swap::<u8>");
        assert_eq!(demangle("_RNvXCs1234_5hellohINtNtNtCs1234_4core3ops5arith3AddhE3add"), "<u8 as core::ops::arith::Add<u8>>::add");
    }
}
