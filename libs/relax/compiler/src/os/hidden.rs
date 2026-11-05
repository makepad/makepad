//! Symbols a native library keeps but does not export.
//!
//! `--allow-c` links the native build scripts' archives into one shared library and resolves the
//! program's foreign items in it with dlsym. C code built with `-fvisibility=hidden`, and
//! assembly with `.private_extern` (ring's whole C/asm core is), survives that link only as local
//! symbols: the symbol table still names them, the dynamic loader does not. This reads the
//! library file's own symbol table (Mach-O LC_SYMTAB, ELF .symtab) and adds the image's load
//! address, which is what the linker would have resolved a static call to.

use std::collections::HashMap;
use std::sync::Mutex;

/// The native libraries loaded by path, each with its parsed table once it was needed.
static LIBS: Mutex<Vec<(String, Option<Table>)>> = Mutex::new(Vec::new());

struct Table {
    /// Symbol name (as the file spells it) to its address in this process.
    syms: HashMap<Vec<u8>, u64>,
}

/// A native library `load_path` loaded: a candidate for the fallback.
pub fn note_native_lib(path: &str) {
    let mut libs = LIBS.lock().unwrap_or_else(|e| e.into_inner());
    if !libs.iter().any(|(p, _)| p == path) {
        crate::zone_alloc::kept_with(|| libs.push((path.to_string(), None)));
    }
}

/// `name` (NUL-free) as a local symbol of a loaded native library: its address, or null.
pub fn find_hidden(name: &[u8]) -> *mut u8 {
    let mut libs = LIBS.lock().unwrap_or_else(|e| e.into_inner());
    for (path, table) in libs.iter_mut() {
        if table.is_none() {
            let t = crate::zone_alloc::kept_with(|| read_table(path));
            *table = Some(t.unwrap_or(Table { syms: HashMap::new() }));
        }
        let t = table.as_ref().unwrap();
        if let Some(&a) = t.syms.get(name) {
            return a as *mut u8;
        }
        // Mach-O spells C symbols with a leading underscore.
        let mut under = Vec::with_capacity(name.len() + 1);
        under.push(b'_');
        under.extend_from_slice(name);
        if let Some(&a) = t.syms.get(&under) {
            return a as *mut u8;
        }
    }
    std::ptr::null_mut()
}

fn read_table(path: &str) -> Option<Table> {
    let data = std::fs::read(path).ok()?;
    let base = imp::load_address(path)?;
    let mut syms = HashMap::new();
    if data.len() >= 4 && data[..4] == [0xcf, 0xfa, 0xed, 0xfe] {
        macho64(&data, base, &mut syms);
    } else if data.len() >= 4 && data[..4] == [0x7f, b'E', b'L', b'F'] {
        elf64(&data, base, &mut syms);
    } else {
        return None;
    }
    Some(Table { syms })
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> { Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?)) }
fn u32_at(d: &[u8], o: usize) -> Option<u32> { Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?)) }
fn u64_at(d: &[u8], o: usize) -> Option<u64> { Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?)) }
fn cstr_at(d: &[u8], o: usize) -> Option<&[u8]> {
    let s = d.get(o..)?;
    let n = s.iter().position(|&b| b == 0)?;
    Some(&s[..n])
}

/// Mach-O 64: `base` is the image's slide (dylib segments are laid out from vmaddr 0).
fn macho64(d: &[u8], slide: u64, out: &mut HashMap<Vec<u8>, u64>) {
    let Some(ncmds) = u32_at(d, 16) else { return };
    let mut off = 32usize;
    for _ in 0..ncmds {
        let (Some(cmd), Some(size)) = (u32_at(d, off), u32_at(d, off + 4)) else { return };
        if size < 8 { return; }
        // LC_SYMTAB
        if cmd == 0x2 {
            let (Some(symoff), Some(nsyms), Some(stroff), Some(strsize)) = (u32_at(d, off + 8), u32_at(d, off + 12), u32_at(d, off + 16), u32_at(d, off + 20)) else { return };
            let Some(strtab) = d.get(stroff as usize..(stroff as usize).saturating_add(strsize as usize).min(d.len())) else { return };
            for i in 0..nsyms as usize {
                let e = symoff as usize + i * 16;
                let (Some(n_strx), Some(&n_type), Some(&n_sect), Some(n_value)) = (u32_at(d, e), d.get(e + 4), d.get(e + 5), u64_at(d, e + 8)) else { break };
                // debugging stabs (N_STAB), undefined and absolute entries are not addresses here
                if n_type & 0xe0 != 0 || n_type & 0x0e != 0x0e || n_sect == 0 { continue; }
                if let Some(name) = cstr_at(strtab, n_strx as usize) {
                    if !name.is_empty() { out.insert(name.to_vec(), n_value.wrapping_add(slide)); }
                }
            }
            return;
        }
        off += size as usize;
    }
}

/// ELF64: `base` is the object's load bias (dl_iterate_phdr's dlpi_addr).
fn elf64(d: &[u8], base: u64, out: &mut HashMap<Vec<u8>, u64>) {
    let (Some(shoff), Some(shentsize), Some(shnum)) = (u64_at(d, 0x28), u16_at(d, 0x3a), u16_at(d, 0x3c)) else { return };
    let sh = |i: usize| -> Option<(u32, u64, u64, u32, u64)> {
        let o = shoff as usize + i * shentsize as usize;
        Some((u32_at(d, o + 4)?, u64_at(d, o + 24)?, u64_at(d, o + 32)?, u32_at(d, o + 40)?, u64_at(d, o + 56)?))
    };
    for i in 0..shnum as usize {
        let Some((ty, offset, size, link, entsize)) = sh(i) else { return };
        // SHT_SYMTAB (the static table; .dynsym lacks the hidden ones)
        if ty != 2 || entsize < 24 { continue; }
        let Some((_, stroff, strsize, _, _)) = sh(link as usize) else { return };
        let Some(strtab) = d.get(stroff as usize..(stroff as usize).saturating_add(strsize as usize).min(d.len())) else { return };
        let n = (size / entsize) as usize;
        for k in 0..n {
            let e = offset as usize + k * entsize as usize;
            let (Some(st_name), Some(&st_info), Some(st_shndx), Some(st_value)) = (u32_at(d, e), d.get(e + 4), u16_at(d, e + 6), u64_at(d, e + 8)) else { break };
            // defined functions and objects only (STT_OBJECT 1, STT_FUNC 2; SHN_UNDEF 0)
            if st_shndx == 0 || !matches!(st_info & 0xf, 1 | 2) { continue; }
            if let Some(name) = cstr_at(strtab, st_name as usize) {
                if !name.is_empty() { out.insert(name.to_vec(), st_value.wrapping_add(base)); }
            }
        }
        return;
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(i: u32) -> *const i8;
        fn _dyld_get_image_vmaddr_slide(i: u32) -> isize;
    }

    /// The slide of the loaded image whose path is `path`.
    pub fn load_address(path: &str) -> Option<u64> {
        let want = std::fs::canonicalize(path).ok();
        unsafe {
            for i in 0..=_dyld_image_count() {
                let p = _dyld_get_image_name(i);
                if p.is_null() { continue; }
                let name = std::ffi::CStr::from_ptr(p as *const _).to_bytes();
                let name = std::str::from_utf8(name).unwrap_or("");
                if name == path || want.as_deref().is_some_and(|w| std::fs::canonicalize(name).ok().as_deref() == Some(w)) {
                    return Some(_dyld_get_image_vmaddr_slide(i) as u64);
                }
            }
        }
        None
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
mod imp {
    #[repr(C)]
    struct DlPhdrInfo {
        addr: usize,
        name: *const u8,
        // the rest is not read
    }
    extern "C" {
        fn dl_iterate_phdr(cb: extern "C" fn(*mut DlPhdrInfo, usize, *mut u8) -> i32, data: *mut u8) -> i32;
    }
    struct Query { want: Vec<u8>, found: Option<u64> }
    extern "C" fn visit(info: *mut DlPhdrInfo, _size: usize, data: *mut u8) -> i32 {
        unsafe {
            let q = &mut *(data as *mut Query);
            let name = if (*info).name.is_null() { &[][..] } else { std::ffi::CStr::from_ptr((*info).name as *const _).to_bytes() };
            let same = name == q.want.as_slice()
                || (std::str::from_utf8(name).ok().and_then(|n| std::fs::canonicalize(n).ok())
                    == std::str::from_utf8(&q.want).ok().and_then(|w| std::fs::canonicalize(w).ok()) && !name.is_empty());
            if same {
                q.found = Some((*info).addr as u64);
                return 1;
            }
        }
        0
    }

    /// The load bias of the shared object loaded from `path`.
    pub fn load_address(path: &str) -> Option<u64> {
        let mut q = Query { want: path.as_bytes().to_vec(), found: None };
        unsafe { dl_iterate_phdr(visit, &mut q as *mut Query as *mut u8) };
        q.found
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn load_address(_path: &str) -> Option<u64> {
        None
    }
}
