//! Linux/glibc: a `mallinfo` that does not wrap, for libcef (a compat for a Chromium bug).
//!
//! glibc's legacy `mallinfo()` reports in `int` fields, which wrap negative once the process's
//! malloc heap passes 2 GiB (Stage under Relax holds ~3 GB). Chromium reads it in its memory-dump
//! provider (`MallocDumpProvider::OnMemoryDump`) and `checked_cast`s `arena + hblkhd` and
//! `uordblks` to `size_t`; a negative value hits its CHECK (`ud2`), so the host dies of SIGILL on
//! the `MemoryInfra` thread. The dump is requested by Chrome's `TabMemoryMetricsReporter` (a timer
//! started for every browser by `TabHelpers::AttachTabHelpers`, with no feature flag or switch
//! that disables it), so it cannot be configured away.
//!
//! One implementation, [`saturating_mallinfo`] (glibc's `mallinfo2` clamped to `int`, with
//! `arena + hblkhd` clamped as a sum), reaches libcef two ways:
//! - [`install`], called by makepad-cef right after it loads libcef: points libcef's own
//!   `mallinfo` import (its GOT slot) at it, unless the slot already binds somewhere other than
//!   libc's `mallinfo`;
//! - a host executable that exports a `mallinfo` symbol (the makepad loader: it includes this file
//!   by path and exports `mallinfo` calling [`saturating_mallinfo`]), which the dynamic linker
//!   then binds libcef to, so `install` finds nothing to do.
#![allow(dead_code)]

#[repr(C)]
pub struct Mallinfo {
    arena: i32,
    ordblks: i32,
    smblks: i32,
    hblks: i32,
    hblkhd: i32,
    usmblks: i32,
    fsmblks: i32,
    uordblks: i32,
    fordblks: i32,
    keepcost: i32,
}

#[repr(C)]
pub struct Mallinfo2 {
    pub arena: usize,
    pub ordblks: usize,
    pub smblks: usize,
    pub hblks: usize,
    pub hblkhd: usize,
    pub usmblks: usize,
    pub fsmblks: usize,
    pub uordblks: usize,
    pub fordblks: usize,
    pub keepcost: usize,
}

extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

fn clamp(v: usize) -> i32 {
    v.min(i32::MAX as usize) as i32
}

/// `m` as the `int` struct: every field saturates at `i32::MAX`, and `arena` is cut so that
/// `arena + hblkhd` (what Chromium adds) also stays within it.
pub fn saturate(m: &Mallinfo2) -> Mallinfo {
    let hblkhd = clamp(m.hblkhd);
    Mallinfo {
        arena: clamp(m.arena).min(i32::MAX - hblkhd),
        ordblks: clamp(m.ordblks),
        smblks: clamp(m.smblks),
        hblks: clamp(m.hblks),
        hblkhd,
        usmblks: clamp(m.usmblks),
        fsmblks: clamp(m.fsmblks),
        uordblks: clamp(m.uordblks),
        fordblks: clamp(m.fordblks),
        keepcost: clamp(m.keepcost),
    }
}

pub extern "C" fn saturating_mallinfo() -> Mallinfo {
    saturate(&unsafe { mallinfo2() })
}

mod got {
    use std::ffi::{c_char, c_int, c_void, CStr};

    #[repr(C)]
    struct Phdr { p_type: u32, p_flags: u32, p_offset: u64, p_vaddr: u64, p_paddr: u64, p_filesz: u64, p_memsz: u64, p_align: u64 }
    #[repr(C)]
    struct DlPhdrInfo { addr: usize, name: *const c_char, phdr: *const Phdr, phnum: u16 }
    #[repr(C)]
    struct Dyn { tag: i64, val: u64 }
    #[repr(C)]
    struct Rela { offset: u64, info: u64, addend: i64 }
    #[repr(C)]
    struct Sym { name: u32, info: u8, other: u8, shndx: u16, value: u64, size: u64 }

    extern "C" {
        fn dl_iterate_phdr(cb: unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int, data: *mut c_void) -> c_int;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
        fn sysconf(name: c_int) -> i64;
    }

    const PT_DYNAMIC: u32 = 2;
    const DT_PLTRELSZ: i64 = 2;
    const DT_STRTAB: i64 = 5;
    const DT_SYMTAB: i64 = 6;
    const DT_RELA: i64 = 7;
    const DT_RELASZ: i64 = 8;
    const DT_JMPREL: i64 = 23;
    // R_*_GLOB_DAT / R_*_JUMP_SLOT of the two architectures glibc hosts run on here
    #[cfg(target_arch = "x86_64")]
    const SLOT_TYPES: [u32; 2] = [6, 7];
    #[cfg(target_arch = "aarch64")]
    const SLOT_TYPES: [u32; 2] = [1025, 1026];
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const SLOT_TYPES: [u32; 0] = [];

    struct Job { replacement: usize, libc_mallinfo: usize, patched: usize }

    /// The GOT/PLT slots of `mallinfo` in one object: (slot address) of those that hold libc's.
    unsafe extern "C" fn visit(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
        let job = &mut *(data as *mut Job);
        let info = &*info;
        if info.name.is_null() || !CStr::from_ptr(info.name).to_bytes().ends_with(b"libcef.so") {
            return 0;
        }
        let base = info.addr;
        let abs = |v: u64| if (v as usize) < base { base + v as usize } else { v as usize };
        let (mut symtab, mut strtab, mut jmprel, mut pltrelsz, mut rela, mut relasz) = (0, 0, 0, 0usize, 0, 0usize);
        for i in 0..info.phnum as usize {
            let ph = &*info.phdr.add(i);
            if ph.p_type != PT_DYNAMIC {
                continue;
            }
            let mut d = (base + ph.p_vaddr as usize) as *const Dyn;
            while (*d).tag != 0 {
                match (*d).tag {
                    DT_SYMTAB => symtab = abs((*d).val),
                    DT_STRTAB => strtab = abs((*d).val),
                    DT_JMPREL => jmprel = abs((*d).val),
                    DT_PLTRELSZ => pltrelsz = (*d).val as usize,
                    DT_RELA => rela = abs((*d).val),
                    DT_RELASZ => relasz = (*d).val as usize,
                    _ => {}
                }
                d = d.add(1);
            }
        }
        if symtab == 0 || strtab == 0 {
            return 1;
        }
        for (table, bytes) in [(jmprel, pltrelsz), (rela, relasz)] {
            if table == 0 {
                continue;
            }
            for k in 0..bytes / std::mem::size_of::<Rela>() {
                let r = &*(table as *const Rela).add(k);
                let (sym, kind) = ((r.info >> 32) as usize, r.info as u32);
                if sym == 0 || !SLOT_TYPES.contains(&kind) {
                    continue;
                }
                let name = CStr::from_ptr((strtab + (*(symtab as *const Sym).add(sym)).name as usize) as *const c_char);
                if name.to_bytes() != b"mallinfo" {
                    continue;
                }
                let slot = (base + r.offset as usize) as *mut usize;
                if *slot != job.libc_mallinfo {
                    continue; // bound to some other definition: leave it
                }
                let page = sysconf(30).max(4096) as usize;
                let start = (slot as usize) & !(page - 1);
                // RELRO leaves the slot read-only once relocated: make it writable, then back
                if mprotect(start as *mut c_void, page, 3) != 0 {
                    continue;
                }
                std::ptr::write_volatile(slot, job.replacement);
                mprotect(start as *mut c_void, page, 1);
                job.patched += 1;
            }
        }
        1
    }

    /// Redirect libcef's `mallinfo` import to `replacement` when it binds to libc's. Returns the
    /// number of slots changed (0 when libcef is not loaded or already bound elsewhere).
    pub unsafe fn redirect(replacement: usize) -> usize {
        let libc = dlopen(b"libc.so.6\0".as_ptr() as *const c_char, 2);
        let libc_mallinfo = if libc.is_null() { 0 } else { dlsym(libc, b"mallinfo\0".as_ptr() as *const c_char) as usize };
        if libc_mallinfo == 0 {
            return 0;
        }
        let mut job = Job { replacement, libc_mallinfo, patched: 0 };
        dl_iterate_phdr(visit, &mut job as *mut Job as *mut c_void);
        job.patched
    }
}

/// Call after libcef is loaded (any thread, before its browser process starts working). Returns
/// how many import slots were redirected.
pub fn install() -> usize {
    unsafe { got::redirect(saturating_mallinfo as extern "C" fn() -> Mallinfo as usize) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonneg_sums(m: &Mallinfo) {
        assert!(m.arena >= 0 && m.hblkhd >= 0 && m.uordblks >= 0 && m.fordblks >= 0);
        assert!(m.arena.checked_add(m.hblkhd).is_some_and(|s| s >= 0), "arena + hblkhd wraps");
    }

    #[test]
    fn values_past_two_gib_saturate() {
        let gib = 1usize << 30;
        let m = saturate(&Mallinfo2 { arena: 3 * gib, ordblks: 5, smblks: 0, hblks: 2, hblkhd: gib, usmblks: 0, fsmblks: 0, uordblks: 5 * gib, fordblks: 4 * gib, keepcost: 7 });
        nonneg_sums(&m);
        assert_eq!(m.hblkhd, gib as i32);
        assert_eq!(m.arena, i32::MAX - gib as i32);
        assert_eq!(m.uordblks, i32::MAX);
        assert_eq!((m.ordblks, m.keepcost), (5, 7));
        // a mapped total past 2 GiB on its own
        let m = saturate(&Mallinfo2 { arena: 0, ordblks: 0, smblks: 0, hblks: 0, hblkhd: 3 * gib, usmblks: 0, fsmblks: 0, uordblks: 0, fordblks: 0, keepcost: 0 });
        nonneg_sums(&m);
        assert_eq!((m.hblkhd, m.arena), (i32::MAX, 0));
    }

    /// The live heap past 2 GiB: glibc's own `mallinfo` returns wrapped (negative) fields here,
    /// which is what killed Chromium's memory dump; ours must not. The 3 GiB block is mmap'd and
    /// never touched.
    #[test]
    fn real_heap_past_two_gib_does_not_wrap() {
        extern "C" {
            fn malloc(n: usize) -> *mut u8;
            fn free(p: *mut u8);
        }
        let big = unsafe { malloc(3usize << 30) };
        if big.is_null() {
            return; // address space limited (ulimit -v): nothing to measure
        }
        // one touched page, so the allocation cannot be optimized away
        unsafe { std::ptr::write_volatile(std::hint::black_box(big), 1) };
        let m = saturating_mallinfo();
        unsafe { free(big) };
        nonneg_sums(&m);
        assert!(m.hblkhd as i64 + m.arena as i64 > 1 << 30, "the 3 GiB block is not in the figures: arena {} hblkhd {} uordblks {}", m.arena, m.hblkhd, m.uordblks);
    }

    /// With a real libcef at hand (MAKEPAD_CEF_DIST_DIR): after dlopen its `mallinfo` import is
    /// libc's, `install` redirects it, and a second `install` finds nothing left to do.
    #[test]
    fn install_redirects_libcef_import() {
        extern "C" {
            fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut std::ffi::c_void;
        }
        let Some(dir) = std::env::var_os("MAKEPAD_CEF_DIST_DIR") else { return };
        let lib = std::path::Path::new(&dir).join("Release/libcef.so");
        if !lib.exists() {
            return;
        }
        let c = format!("{}\0", lib.display());
        assert!(!unsafe { dlopen(c.as_ptr() as *const _, 2) }.is_null(), "libcef did not load");
        assert_eq!(install(), 1, "libcef's mallinfo import was not redirected");
        assert_eq!(install(), 0, "a redirected slot was patched again");
    }
}
