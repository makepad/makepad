//! Memory: the unit's one reservation ([code][rest]), committed ranges, zeroed read-write
//! pages, code writes, and the compiler's stack segments.
//!
//! - `reserve(code, rest)`: `code` bytes of code memory at the base (executable; written only
//!   through `write_code`), then `rest` bytes of read-write memory, so code reaches its table,
//!   data and statics pc-relatively. Pages may be committed lazily: `commit` makes a range
//!   usable before its first use (no-op where the OS overcommits).
//! - `release(base, code, rest)`: unmaps a whole reservation (an auxiliary unit dropped mid-run).
//! - `alloc_rw(len)`: zeroed read-write pages; `free(addr, len)` returns them.
//! - `write_code(addr, bytes)`: copies machine code into code memory (macOS: through the
//!   writable view; aarch64 Linux/Android: a plain copy, then the icache flush).

include!("../rt_abi.rs");
pub use imp::*;

/// no-codegen mode (MAKEPAD_RUN_NO_CODEGEN): reservations get no executable view, and any code
/// write is an error naming its caller -- the process must run without machine code of its own.
static NO_CODEGEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_no_codegen() {
    NO_CODEGEN.store(true, std::sync::atomic::Ordering::Release);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn no_codegen() -> bool {
    NO_CODEGEN.load(std::sync::atomic::Ordering::Acquire)
}

/// The process's executable memory that no loaded image backs (macOS): (start, size, prot) per
/// region. No-codegen mode checks it is empty when the process ends -- generated code anywhere
/// in the process (the JIT, a kernel compiler, a library) shows up here.
#[cfg(target_os = "macos")]
pub fn anonymous_exec_regions() -> Vec<(u64, u64, i32)> {
    #[repr(C)]
    #[derive(Default)]
    struct BasicInfo64 {
        protection: i32,
        max_protection: i32,
        inheritance: u32,
        shared: u32,
        reserved: u32,
        offset: u64,
        behavior: i32,
        user_wired_count: u16,
    }
    #[repr(C)]
    struct DlInfo {
        fname: *const std::ffi::c_char,
        fbase: *mut u8,
        sname: *const std::ffi::c_char,
        saddr: *mut u8,
    }
    extern "C" {
        fn mach_task_self() -> u32;
        fn mach_vm_region(task: u32, addr: *mut u64, size: *mut u64, flavor: i32, info: *mut BasicInfo64, count: *mut u32, object: *mut u32) -> i32;
        fn dladdr(addr: *const u8, info: *mut DlInfo) -> i32;
    }
    const VM_REGION_BASIC_INFO_64: i32 = 9;
    const VM_PROT_EXECUTE: i32 = 4;
    let mut out = Vec::new();
    let mut addr: u64 = 0;
    unsafe {
        loop {
            let mut size: u64 = 0;
            let mut info = BasicInfo64::default();
            let mut count = (std::mem::size_of::<BasicInfo64>() / 4) as u32;
            let mut object = 0u32;
            if mach_vm_region(mach_task_self(), &mut addr, &mut size, VM_REGION_BASIC_INFO_64, &mut info, &mut count, &mut object) != 0 {
                break;
            }
            if info.protection & VM_PROT_EXECUTE != 0 {
                let mut d = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
                // an image's text (or the shared cache) is backed; anything else is generated
                if dladdr(addr as *const u8, &mut d) == 0 || d.fname.is_null() {
                    out.push((addr, size, info.protection));
                }
            }
            addr += size;
        }
    }
    out
}

#[track_caller]
#[cold]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn refuse_code(what: &str, addr: u64, len: usize) -> ! {
    panic!("makepad run: no-codegen mode: {} of {} bytes at {:#x} (machine code is not allowed in this mode)", what, len, addr)
}

/// Replaces the 8 code bytes at `addr` (8-aligned) with one atomic store: a thread executing
/// them meanwhile runs the old or the new instruction, never a mix (x64 thunk heads).
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
pub unsafe fn patch_code_u64(addr: u64, w: u64) {
    debug_assert!(addr & 7 == 0, "patch_code_u64: unaligned {:#x}", addr);
    fill_code(addr, 8, |b| (*(b.as_mut_ptr() as *const std::sync::atomic::AtomicU64)).store(w, std::sync::atomic::Ordering::Release));
}

/// Replaces the instruction at `addr` (4-aligned) with one atomic store, then makes the
/// instruction cache see it (arm64 thunk heads: a `b` may be modified while it runs).
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub unsafe fn patch_code_u32(addr: u64, w: u32) {
    debug_assert!(addr & 3 == 0, "patch_code_u32: unaligned {:#x}", addr);
    fill_code(addr, 4, |b| (*(b.as_mut_ptr() as *const std::sync::atomic::AtomicU32)).store(w, std::sync::atomic::Ordering::Release));
}

/// The OS page size (mprotect's granularity): 4 KB on x86-64, 16 KB on Apple arm64.
pub fn page_size() -> u64 {
    static PAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = PAGE.load(std::sync::atomic::Ordering::Relaxed);
    if p != 0 {
        return p;
    }
    #[cfg(unix)]
    let p = {
        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        #[cfg(target_os = "macos")]
        const SC_PAGESIZE: i32 = 29;
        #[cfg(target_os = "android")]
        const SC_PAGESIZE: i32 = 0x27;
        #[cfg(not(any(target_os = "macos", target_os = "android")))]
        const SC_PAGESIZE: i32 = 30;
        let v = unsafe { sysconf(SC_PAGESIZE) };
        if v > 0 { v as u64 } else { 4096 }
    };
    #[cfg(not(unix))]
    let p = 4096u64;
    PAGE.store(p, std::sync::atomic::Ordering::Relaxed);
    p
}

/// Makes invalidated code [addr, addr + len) (whole pages that hold nothing else, invalid.rs)
/// inaccessible for good: any fetch, read or write there faults (a stale access). Its addresses
/// stay taken. A failure is fatal: the code would stay runnable.
pub fn invalidate_code(addr: u64, len: usize) {
    if no_codegen() {
        return;
    }
    #[cfg(unix)]
    let ok = unsafe { unix::mprotect(addr as *mut u8, len, 0) } == 0;
    #[cfg(windows)]
    let ok = {
        let mut old = 0u32;
        unsafe { crate::os::win32::VirtualProtect(addr as *mut u8, len, crate::os::win32::PAGE_NOACCESS, &mut old) != 0 }
    };
    if !ok {
        eprintln!("makepad run: invalidated code at {:#x} ({} bytes) could not be made inaccessible", addr, len);
        std::process::abort();
    }
}

#[cfg(unix)]
mod unix {
    extern "C" {
        pub fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        pub fn munmap(addr: *mut u8, len: usize) -> i32;
        pub fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    }
    pub const PROT_RW: i32 = 3;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const MAP_PRIVATE_ANON: i32 = 0x22;
    #[cfg(target_os = "macos")]
    pub const MAP_PRIVATE_ANON: i32 = 0x1002;

    /// Zeroed read-write pages.
    pub fn alloc_rw(size: usize) -> u64 {
        let p = unsafe { mmap(std::ptr::null_mut(), size, PROT_RW, MAP_PRIVATE_ANON, -1, 0) };
        if p as isize == -1 || p.is_null() {
            panic!("mmap failed");
        }
        p as u64
    }

    /// Makes [addr, addr+len) inaccessible (a guard page).
    pub fn guard_page(addr: u64, len: usize) {
        unsafe {
            mprotect(addr as *mut u8, len, 0);
        }
    }

    /// Gives a range's pages back but keeps its addresses taken (PROT_NONE, no reserve): a
    /// retired unit's code addresses are never handed out again, so nothing that names them --
    /// a stale code pointer, a profiler's perf map / symbol table written for that unit --
    /// can be taken for another unit's code (backend2's glyph_cache profile named the program's
    /// hot loop after the freed macro world's HashMap::resize at the same address).
    pub fn retire(addr: u64, len: usize) {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const MAP_FIXED_NORESERVE: i32 = 0x10 | 0x4000;
        #[cfg(target_os = "macos")]
        const MAP_FIXED_NORESERVE: i32 = 0x10 | 0x40;
        unsafe {
            if mmap(addr as *mut u8, len, 0, MAP_PRIVATE_ANON | MAP_FIXED_NORESERVE, -1, 0) as u64 == addr {
                return;
            }
            // the pages stay committed, but nothing can execute or name them for other code
            if mprotect(addr as *mut u8, len, 0) != 0 {
                // (a failed MAP_FIXED may already have removed the old mapping: the range could be
                // handed out again while stale return addresses name it. Never contained: abort)
                eprintln!("makepad run: retired code memory at {:#x} ({} bytes) could not be made unreachable", addr, len);
                std::process::abort();
            }
        }
    }

    pub fn free(addr: u64, len: usize) {
        unsafe {
            munmap(addr as *mut u8, len);
        }
    }

    extern "C" {
        fn memcpy(d: *mut std::ffi::c_void, s: *const std::ffi::c_void, n: usize) -> *mut std::ffi::c_void;
        fn memmove(d: *mut std::ffi::c_void, s: *const std::ffi::c_void, n: usize) -> *mut std::ffi::c_void;
        fn memset(d: *mut std::ffi::c_void, c: i32, n: usize) -> *mut std::ffi::c_void;
    }

    /// memcpy / memmove / memset as compiled code calls them (Relax's convention = the C one here).
    pub fn mem_fns() -> (u64, u64, u64) {
        (memcpy as *const () as usize as u64, memmove as *const () as usize as u64, memset as *const () as usize as u64)
    }

    /// bytes at the end of the code area the OS layer keeps for itself
    pub const CODE_TAIL: u64 = 0;

    /// Unix overcommits: reserved pages are usable as they are.
    #[inline(always)]
    pub fn commit(_addr: u64, _len: usize) {}

    /// Maps the pages of [addr, addr + len) writable now, in one call, instead of a fault per
    /// page as they are first written (a block about to be filled whole: the startup image's
    /// code and data). Linux 5.14+ (MADV_POPULATE_WRITE); elsewhere, or on an older kernel,
    /// the pages fault in as before.
    pub fn prefault(addr: u64, len: usize) {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            extern "C" {
                fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
            }
            const MADV_POPULATE_WRITE: i32 = 23;
            let lo = addr & !4095;
            let hi = (addr + len as u64 + 4095) & !4095;
            if hi > lo {
                unsafe { madvise(lo as *mut u8, (hi - lo) as usize, MADV_POPULATE_WRITE) };
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let _ = (addr, len);
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    pub use super::unix::{alloc_rw, commit, free, guard_page, mem_fns, CODE_TAIL};
    use super::unix::{mmap, mprotect, munmap, PROT_RW};
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

    // W^X (JITSAFE fix 15): code memory is two views of one memfd -- the read-execute view at the
    // code addresses (what everyone calls and patches by address) and a read-write alias that
    // only `write_code` / `fill_code` write through. No page of the process is writable and
    // executable at once, so a stray write in the program or the JIT cannot become code.
    // MAKEPAD_RUN_WX=0 (and the perf map, whose JIT symbols perf applies only to anonymous
    // memory) keeps the single read-write-execute view. A program's fork gives the child copies
    // (fork_prepare / fork_child).

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn ftruncate(fd: i32, len: i64) -> i32;
        fn close(fd: i32) -> i32;
        fn lseek(fd: i32, off: i64, whence: i32) -> i64;
        fn pwrite(fd: i32, buf: *const u8, n: usize, off: i64) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
        fn _exit(code: i32) -> !;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_MEMFD_CREATE: i64 = 319;
    #[cfg(target_arch = "aarch64")]
    const SYS_MEMFD_CREATE: i64 = 279;
    const MFD_CLOEXEC: i64 = 1;
    const PROT_RX: i32 = 5;
    const PROT_RWX: i32 = 7;
    const MAP_SHARED: i32 = 0x01;
    const MAP_FIXED: i32 = 0x10;
    const SEEK_DATA: i32 = 3;
    const SEEK_HOLE: i32 = 4;

    /// Code regions: (RX base, size, RW base, memfd); append-only, published by REGIONS_N (a
    /// released region keeps its entry with size 0).
    const REGIONS_CAP: usize = 4096;
    static REGIONS: [[AtomicU64; 3]; REGIONS_CAP] = [const { [const { AtomicU64::new(0) }; 3] }; REGIONS_CAP];
    static REGION_FD: [AtomicI64; REGIONS_CAP] = [const { AtomicI64::new(-1) }; REGIONS_CAP];
    static REGIONS_N: AtomicUsize = AtomicUsize::new(0);
    static REGIONS_LOCK: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

    /// W^X is on (decided once: before the first reservation).
    fn wx() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| !std::env::var_os("MAKEPAD_RUN_WX").is_some_and(|v| v == "0") && !crate::jit::perf_map_on())
    }
    static WX_FAILED: AtomicBool = AtomicBool::new(false);

    /// A copy of every code region in a new memfd, made by `fork_prepare` for the child of a
    /// program's fork (jit.rs relax_fork): -1 none.
    static FORK_FD: [AtomicI64; REGIONS_CAP] = [const { AtomicI64::new(-1) }; REGIONS_CAP];

    /// A memfd holding what code region `r` (memfd `old`) holds now: the written ranges only, the
    /// rest stays a hole (zero pages). -1: no memfd.
    unsafe fn copy_region(r: &[AtomicU64; 3], old: i32) -> i32 {
        let (size, rw) = (r[1].load(Ordering::Relaxed), r[2].load(Ordering::Relaxed));
        let fd = syscall(SYS_MEMFD_CREATE, c"makepad-run-jit-code".as_ptr(), MFD_CLOEXEC) as i32;
        let mut ok = fd >= 0 && ftruncate(fd, size as i64) == 0;
        let mut at = 0i64;
        while ok && (at as u64) < size {
            let d = lseek(old, at, SEEK_DATA);
            if d < 0 {
                break;
            }
            let h = lseek(old, d, SEEK_HOLE).min(size as i64);
            let mut o = d;
            while ok && o < h {
                let w = pwrite(fd, (rw + o as u64) as *const u8, (h - o) as usize, o);
                ok = w > 0;
                o += w.max(0) as i64;
            }
            at = h;
        }
        if !ok && fd >= 0 {
            close(fd);
            return -1;
        }
        fd
    }

    /// Before a program's fork (every code writer paused: jit.rs fork_prepare): each code region
    /// is copied into a memfd of its own for the child. MAP_SHARED views would otherwise share
    /// the parent's code pages with the child, which compiles on (a lazy compile in the child)
    /// and would write into the parent's code. A fork of the loader's own native code execs at
    /// once and never writes code: it shares the pages, nothing is copied.
    pub fn fork_prepare() {
        let n = REGIONS_N.load(Ordering::Acquire);
        for (k, (r, fd)) in REGIONS[..n].iter().zip(&REGION_FD[..n]).enumerate() {
            let old = fd.load(Ordering::Relaxed) as i32;
            if r[1].load(Ordering::Acquire) != 0 && old >= 0 {
                FORK_FD[k].store(unsafe { copy_region(r, old) } as i64, Ordering::Relaxed);
            }
        }
    }

    /// The parent after the fork: the copies are the child's.
    pub fn fork_parent() {
        for fd in &FORK_FD[..REGIONS_N.load(Ordering::Acquire)] {
            let f = fd.swap(-1, Ordering::Relaxed);
            if f >= 0 {
                unsafe { close(f as i32) };
            }
        }
    }

    /// The child after the fork (its one thread, before it writes any code): both views of every
    /// region are mapped again, at the same addresses, from the copy fork_prepare made.
    pub fn fork_child() {
        let n = REGIONS_N.load(Ordering::Acquire);
        for (k, (r, fdc)) in REGIONS[..n].iter().zip(&REGION_FD[..n]).enumerate() {
            let (rx, size, rw) = (r[0].load(Ordering::Relaxed), r[1].load(Ordering::Relaxed), r[2].load(Ordering::Relaxed));
            if size == 0 || fdc.load(Ordering::Relaxed) < 0 {
                continue;
            }
            unsafe {
                let fd = FORK_FD[k].swap(-1, Ordering::Relaxed) as i32;
                let ok = fd >= 0
                    && mmap(rx as *mut u8, size as usize, PROT_RX, MAP_SHARED | MAP_FIXED, fd, 0) as u64 == rx
                    && mmap(rw as *mut u8, size as usize, PROT_RW, MAP_SHARED | MAP_FIXED, fd, 0) as u64 == rw;
                if !ok {
                    let msg = b"makepad run: the forked child has no copy of its code memory, exiting\n";
                    write(2, msg.as_ptr(), msg.len());
                    _exit(134);
                }
                close(fdc.swap(fd as i64, Ordering::Relaxed) as i32);
            }
        }
    }

    /// Code memory at `base` (already reserved read-write): the read-execute view of a new memfd
    /// replaces it, and its read-write alias goes elsewhere. false: no memfd here (old kernel,
    /// seccomp) -- the caller keeps the single read-write-execute view.
    fn code_views(base: u64, size: usize) -> bool {
        let _g = REGIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // (a region's entry is never reused: code_rw_of reads entries without the lock)
        let n = REGIONS_N.load(Ordering::Acquire);
        if n == REGIONS_CAP {
            panic!("code memory: more than {} code reservations in this process", REGIONS_CAP);
        }
        unsafe {
            let fd = syscall(SYS_MEMFD_CREATE, c"makepad-run-jit-code".as_ptr(), MFD_CLOEXEC) as i32;
            if fd < 0 {
                return false;
            }
            if ftruncate(fd, size as i64) != 0 {
                close(fd);
                return false;
            }
            let rw = mmap(std::ptr::null_mut(), size, PROT_RW, MAP_SHARED, fd, 0);
            if rw as isize == -1 || rw.is_null() {
                close(fd);
                return false;
            }
            if mmap(base as *mut u8, size, PROT_RX, MAP_SHARED | MAP_FIXED, fd, 0) as u64 != base {
                // MAP_FIXED failing may have unmapped the range: take it back as before
                munmap(rw, size);
                close(fd);
                if mmap(base as *mut u8, size, PROT_RWX, super::unix::MAP_PRIVATE_ANON | MAP_FIXED, -1, 0) as u64 != base {
                    panic!("reservation: code memory lost while mapping its views");
                }
                return false;
            }
            REGIONS[n][0].store(base, Ordering::Relaxed);
            REGIONS[n][2].store(rw as u64, Ordering::Relaxed);
            REGION_FD[n].store(fd as i64, Ordering::Relaxed);
            REGIONS[n][1].store(size as u64, Ordering::Release);
            REGIONS_N.store(n + 1, Ordering::Release);
        }
        true
    }

    /// The region code_rw_of found last (writes come in runs into one region: no scan then).
    static LAST: AtomicUsize = AtomicUsize::new(0);

    /// The RW alias of code address `addr` (None: no W^X view holds it).
    #[inline]
    fn code_rw_of(addr: u64) -> Option<u64> {
        let hit = |r: &[AtomicU64; 3]| -> Option<u64> {
            let size = r[1].load(Ordering::Acquire);
            let base = r[0].load(Ordering::Relaxed);
            (addr >= base && addr - base < size).then(|| addr - base + r[2].load(Ordering::Relaxed))
        };
        let n = REGIONS_N.load(Ordering::Acquire);
        let last = LAST.load(Ordering::Relaxed);
        if last < n {
            if let Some(w) = hit(&REGIONS[last]) {
                return Some(w);
            }
        }
        for (k, r) in REGIONS[..n].iter().enumerate() {
            if let Some(w) = hit(r) {
                LAST.store(k, Ordering::Relaxed);
                return Some(w);
            }
        }
        None
    }

    /// The code part of a fresh reservation: W^X views, or (W^X off or unavailable) one RWX view.
    fn make_code(base: u64, code: usize) {
        if code == 0 {
            return;
        }
        if wx() && !WX_FAILED.load(Ordering::Relaxed) {
            if code_views(base, code) {
                return;
            }
            WX_FAILED.store(true, Ordering::Relaxed);
            eprintln!("makepad run: no memfd for W^X code memory here: code pages are read-write-execute");
        }
        if unsafe { mprotect(base as *mut u8, code, PROT_RWX) } != 0 {
            panic!("reservation: mprotect RWX failed");
        }
    }

    pub fn reserve(code: usize, rest: usize) -> u64 {
        let base = alloc_rw(code + rest);
        crate::memstat::name_mapping(base as usize + code, rest, c"makepad-run-jit-data");
        make_code(base, code);
        // perf names samples in anonymous memory from /tmp/perf-<pid>.map, but only for mappings
        // it sees as anonymous ("//anon"): a prctl-named mapping ("[anon:makepad-run-jit-code]") is a
        // DSO to it, and the JIT's names would never appear. With the perf map on, the code
        // stays unnamed (and W^X is off: a memfd view is a file to perf too).
        if !crate::jit::perf_map_on() && code_rw_of(base).is_none() {
            crate::memstat::name_mapping(base as usize, code, c"makepad-run-jit-code");
        }
        huge_code_pages(base, code);
        base
    }

    /// Code memory in 2-MB pages where the kernel gives them (MADV_HUGEPAGE: THP with direct
    /// compaction for this range, khugepaged collapses the rest): the JIT's hot code is spread
    /// over ~1.8x the 4-KB pages of a rustc binary's (installs land in time order, between cold
    /// code), and the race's UI thread took 2.2x native's L1 iTLB misses (RACEPAR, Zen 5). With
    /// THP `always` but defrag `madvise` the code region stayed all 4-KB pages. A no-op where THP
    /// is off; costs up to one partly used 2-MB page per touched region in RSS. (A W^X memfd is
    /// shmem: its huge pages follow /sys/kernel/mm/transparent_hugepage/shmem_enabled.)
    /// MAKEPAD_RUN_HUGE_CODE=0 keeps 4-KB pages (A/B).
    fn huge_code_pages(addr: u64, len: usize) {
        if std::env::var_os("MAKEPAD_RUN_HUGE_CODE").is_some_and(|v| v == "0") {
            return;
        }
        extern "C" {
            fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
        }
        const MADV_HUGEPAGE: i32 = 14;
        unsafe { madvise(addr as *mut u8, len, MADV_HUGEPAGE) };
    }

    /// `reserve` at exactly `at` (the wasm32 web target's window, web32.rs); 0 when the range is
    /// taken.
    pub fn reserve_at(at: u64, code: usize, rest: usize) -> u64 {
        const MAP_FIXED_NOREPLACE: i32 = 0x100000;
        let p = unsafe { super::unix::mmap(at as *mut u8, code + rest, super::unix::PROT_RW, super::unix::MAP_PRIVATE_ANON | MAP_FIXED_NOREPLACE, -1, 0) };
        if p as isize == -1 || p as u64 != at {
            return 0;
        }
        crate::memstat::name_mapping(at as usize + code, rest, c"makepad-run-jit-data");
        make_code(at, code);
        if code_rw_of(at).is_none() {
            crate::memstat::name_mapping(at as usize, code, c"makepad-run-jit-code");
        }
        at
    }

    /// Gives a reservation back: its code views (the alias and the memfd go) and the rest.
    pub fn release(base: u64, code: usize, rest: usize) {
        {
            let _g = REGIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let n = REGIONS_N.load(Ordering::Acquire);
            for (r, fd) in REGIONS[..n].iter().zip(&REGION_FD[..n]) {
                if r[0].load(Ordering::Relaxed) == base && r[1].load(Ordering::Relaxed) != 0 {
                    let rw = r[2].load(Ordering::Relaxed);
                    r[1].store(0, Ordering::Release);
                    unsafe {
                        munmap(rw as *mut u8, code);
                        close(fd.swap(-1, Ordering::Relaxed) as i32);
                    }
                    break;
                }
            }
        }
        super::unix::retire(base, code + rest);
    }

    /// unix::prefault for code memory too: a W^X range's pages are made in its RW alias (which
    /// the write goes through) and mapped in its RX view (read: the view is not writable).
    pub fn prefault(addr: u64, len: usize) {
        extern "C" {
            fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
        }
        const MADV_POPULATE_READ: i32 = 22;
        match code_rw_of(addr) {
            Some(w) => {
                super::unix::prefault(w, len);
                let lo = addr & !4095;
                let hi = (addr + len as u64 + 4095) & !4095;
                if hi > lo {
                    unsafe { madvise(lo as *mut u8, (hi - lo) as usize, MADV_POPULATE_READ) };
                }
            }
            None => super::unix::prefault(addr, len),
        }
    }

    /// Where code bytes at `addr` are written: the RW alias, or (single RWX view) the address.
    #[inline]
    #[track_caller]
    fn writable(addr: u64, len: usize) -> u64 {
        match code_rw_of(addr) {
            Some(w) => {
                debug_assert!(code_rw_of(addr + len.max(1) as u64 - 1) == Some(w + len.max(1) as u64 - 1), "code write across regions at {:#x}", addr);
                w
            }
            None if !wx() || WX_FAILED.load(Ordering::Relaxed) => addr,
            None => panic!("write_code: {:#x} is not code memory", addr),
        }
    }

    /// Writes machine code: a copy through the RW alias, then the icache flush for the RX range.
    #[track_caller]
    pub unsafe fn write_code(addr: u64, b: &[u8]) {
        std::ptr::copy_nonoverlapping(b.as_ptr(), writable(addr, b.len()) as *mut u8, b.len());
        flush_icache(addr, b.len());
    }

    /// Writes `len` bytes of machine code at `addr` in place (`fill` gets them writable, through
    /// the RW alias; what they hold before is unspecified), as `write_code` would.
    #[track_caller]
    pub unsafe fn fill_code(addr: u64, len: usize, fill: impl FnOnce(&mut [u8])) {
        fill(std::slice::from_raw_parts_mut(writable(addr, len) as *mut u8, len));
        flush_icache(addr, len);
    }

    /// Makes the instruction cache see code just written at [start, end) (what libgcc's
    /// __clear_cache does; bionic has no such symbol): clean the data cache to the point of
    /// unification, then invalidate the instruction cache, by the line sizes CTR_EL0 reports.
    #[cfg(target_arch = "aarch64")]
    unsafe fn clear_cache(start: usize, end: usize) {
        let ctr: usize;
        std::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags));
        let dline = 4usize << ((ctr >> 16) & 0xf);
        let iline = 4usize << (ctr & 0xf);
        let mut a = start & !(dline - 1);
        while a < end {
            std::arch::asm!("dc cvau, {}", in(reg) a, options(nostack, preserves_flags));
            a += dline;
        }
        std::arch::asm!("dsb ish", options(nostack, preserves_flags));
        let mut a = start & !(iline - 1);
        while a < end {
            std::arch::asm!("ic ivau, {}", in(reg) a, options(nostack, preserves_flags));
            a += iline;
        }
        std::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }

    /// aarch64 does not keep the instruction cache coherent with data writes (x86-64 does).
    #[inline(always)]
    unsafe fn flush_icache(addr: u64, len: usize) {
        #[cfg(target_arch = "aarch64")]
        {
            clear_cache(addr as usize, addr as usize + len);
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = (addr, len);
    }
}

/// macOS (Apple Silicon): code memory as two views of the same pages, executed through the
/// RX view (the address everyone uses) and written through the RW view by `write_code`
/// (sys_icache_invalidate after a write). Not MAP_JIT: the program's own
/// `pthread_jit_write_protect_np(0)` (makepad's compute-kernel JIT writing its MAP_JIT pages)
/// flips every MAP_JIT page of the thread to non-executable, which faulted the JIT code making
/// that call.
#[cfg(target_os = "macos")]
mod imp {
    pub use super::unix::{alloc_rw, commit, free, guard_page, mem_fns, prefault, CODE_TAIL};
    use super::unix::mprotect;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    extern "C" {
        fn sys_icache_invalidate(start: *mut u8, len: usize);
        fn mach_task_self() -> u32;
        fn mach_vm_allocate(task: u32, addr: *mut u64, size: u64, flags: i32) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn mach_vm_remap(target: u32, addr: *mut u64, size: u64, mask: u64, flags: i32, src_task: u32, src_addr: u64, copy: i32, cur: *mut i32, max: *mut i32, inherit: u32) -> i32;
        fn pthread_atfork(prepare: Option<extern "C" fn()>, parent: Option<extern "C" fn()>, child: Option<extern "C" fn()>) -> i32;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
        fn _exit(code: i32) -> !;
    }

    /// Code regions: (RX base, size, RW base); append-only, published by CODE_REGIONS_N.
    const CODE_REGIONS_CAP: usize = 64;
    static CODE_REGIONS: [[AtomicU64; 3]; CODE_REGIONS_CAP] = [const { [const { AtomicU64::new(0) }; 3] }; CODE_REGIONS_CAP];
    static CODE_REGIONS_N: AtomicUsize = AtomicUsize::new(0);
    static CODE_REGIONS_LOCK: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

    /// After fork() the child has its own copy-on-write copy of each RW view, but no RX view
    /// (it is mapped VM_INHERIT_NONE: an inherited alias would share or copy the parent's pages
    /// and stop tracking the child's RW copy). The child handler maps the RX alias again, at the
    /// same address, onto the child's RW pages, so forked children run (and compile) their own code.
    extern "C" fn code_regions_after_fork() {
        const VM_FLAGS_FIXED: i32 = 0;
        const VM_INHERIT_NONE: u32 = 2;
        let n = CODE_REGIONS_N.load(Ordering::Acquire);
        unsafe {
            let task = mach_task_self();
            for r in &CODE_REGIONS[..n] {
                let (context, size, rw) = (r[0].load(Ordering::Relaxed), r[1].load(Ordering::Relaxed), r[2].load(Ordering::Relaxed));
                // released
                if size == 0 {
                    continue;
                }
                let mut at = context;
                let (mut cur, mut max) = (0i32, 0i32);
                if mach_vm_remap(task, &mut at, size, 0, VM_FLAGS_FIXED, task, rw, 0, &mut cur, &mut max, VM_INHERIT_NONE) != 0 || at != context || mprotect(context as *mut u8, size as usize, 5) != 0 {
                    let msg = b"makepad: forked child cannot map its code memory, exiting\n";
                    write(2, msg.as_ptr(), msg.len());
                    _exit(134);
                }
            }
        }
    }

    /// One reservation for a unit: `code` bytes of code memory (RX view, written through its RW
    /// alias) at the start, then `rest` bytes of plain read-write memory right after it, so code
    /// can reach its table, data and statics pc-relatively. Returns the base.
    pub fn reserve(code: usize, rest: usize) -> u64 {
        const VM_FLAGS_ANYWHERE: i32 = 1;
        unsafe {
            let task = mach_task_self();
            let mut base: u64 = 0;
            if mach_vm_allocate(task, &mut base, (code + rest) as u64, VM_FLAGS_ANYWHERE) != 0 {
                panic!("reservation: mach_vm_allocate failed");
            }
            // no-codegen: the code part stays plain read-write memory (never executed)
            if super::no_codegen() {
                return base;
            }
            let at = code_alloc_at(code, base);
            assert_eq!(at, base, "reservation: code view not at the base");
            base
        }
    }

    /// `reserve` at exactly `at` (the wasm32 web target's window, web32.rs); 0 when the range is
    /// taken.
    pub fn reserve_at(at: u64, code: usize, rest: usize) -> u64 {
        const VM_FLAGS_FIXED: i32 = 0;
        unsafe {
            let task = mach_task_self();
            let mut base: u64 = at;
            if mach_vm_allocate(task, &mut base, (code + rest) as u64, VM_FLAGS_FIXED) != 0 || base != at {
                return 0;
            }
            if super::no_codegen() {
                return base;
            }
            let c = code_alloc_at(code, base);
            assert_eq!(c, base, "reservation: code view not at the base");
            base
        }
    }

    /// Gives a reservation back: both views of its code pages and the rest. Its code region
    /// slot is emptied first (size 0: no lookup or forked child sees it) and reused by the next
    /// code region.
    pub fn release(base: u64, code: usize, rest: usize) {
        let _g = CODE_REGIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = CODE_REGIONS_N.load(Ordering::Acquire);
        for r in &CODE_REGIONS[..n] {
            if r[0].load(Ordering::Relaxed) == base && r[1].load(Ordering::Relaxed) != 0 {
                let rw = r[2].load(Ordering::Relaxed);
                r[1].store(0, Ordering::Release);
                // the RW alias is never executed: it goes
                free(rw, code);
                break;
            }
        }
        super::unix::retire(base, code + rest);
    }

    /// Code memory; `fixed` != 0: its RX view replaces the pages at that address.
    fn code_alloc_at(size: usize, fixed: u64) -> u64 {
        static ATFORK: std::sync::Once = std::sync::Once::new();
        ATFORK.call_once(|| unsafe {
            pthread_atfork(None, None, Some(code_regions_after_fork));
        });
        const VM_FLAGS_ANYWHERE: i32 = 1;
        const VM_INHERIT_NONE: u32 = 2;
        let _g = CODE_REGIONS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let used = CODE_REGIONS_N.load(Ordering::Acquire);
        // a slot a released reservation emptied, else the next one
        let n = CODE_REGIONS[..used].iter().position(|r| r[1].load(Ordering::Acquire) == 0).unwrap_or(used);
        if n == CODE_REGIONS_CAP {
            panic!("code memory: more than {} regions", CODE_REGIONS_CAP);
        }
        unsafe {
            let task = mach_task_self();
            let mut rw: u64 = 0;
            if mach_vm_allocate(task, &mut rw, size as u64, VM_FLAGS_ANYWHERE) != 0 {
                panic!("code memory: mach_vm_allocate failed");
            }
            // the second view shares the pages (copy = false)
            const VM_FLAGS_FIXED_OVERWRITE: i32 = 0x4000;
            let mut context: u64 = fixed;
            let (mut cur, mut max) = (0i32, 0i32);
            let flags = if fixed != 0 { VM_FLAGS_FIXED_OVERWRITE } else { VM_FLAGS_ANYWHERE };
            if mach_vm_remap(task, &mut context, size as u64, 0, flags, task, rw, 0, &mut cur, &mut max, VM_INHERIT_NONE) != 0 {
                panic!("code memory: mach_vm_remap failed");
            }
            if mprotect(context as *mut u8, size, 5) != 0 {
                panic!("code memory: mprotect RX failed");
            }
            // the size last: a slot with a size has its views
            CODE_REGIONS[n][0].store(context, Ordering::Relaxed);
            CODE_REGIONS[n][2].store(rw, Ordering::Relaxed);
            CODE_REGIONS[n][1].store(size as u64, Ordering::Release);
            if n == used {
                CODE_REGIONS_N.store(n + 1, Ordering::Release);
            }
            context
        }
    }

    /// The RW view of code address `addr`, if it is code memory.
    fn code_rw_of(addr: u64) -> Option<u64> {
        let n = CODE_REGIONS_N.load(Ordering::Acquire);
        for r in &CODE_REGIONS[..n] {
            let size = r[1].load(Ordering::Acquire);
            let base = r[0].load(Ordering::Relaxed);
            if addr >= base && addr - base < size {
                return Some(addr - base + r[2].load(Ordering::Relaxed));
            }
        }
        None
    }

    /// Writes machine code: copies through the RW view, then invalidates the instruction cache
    /// for the RX range.
    #[track_caller]
    pub unsafe fn write_code(addr: u64, b: &[u8]) {
        if super::no_codegen() {
            super::refuse_code("write_code", addr, b.len());
        }
        let w = match code_rw_of(addr) {
            Some(w) => w,
            None => panic!("write_code: {:#x} is not code memory", addr),
        };
        std::ptr::copy_nonoverlapping(b.as_ptr(), w as *mut u8, b.len());
        sys_icache_invalidate(addr as *mut u8, b.len());
    }

    /// Writes `len` bytes of machine code at `addr` in place through the RW view (`fill` gets
    /// them writable; what they hold before is unspecified), then invalidates the instruction
    /// cache for the RX range, as `write_code` does.
    #[track_caller]
    pub unsafe fn fill_code(addr: u64, len: usize, fill: impl FnOnce(&mut [u8])) {
        if super::no_codegen() {
            super::refuse_code("fill_code", addr, len);
        }
        let w = match code_rw_of(addr) {
            Some(w) => w,
            None => panic!("fill_code: {:#x} is not code memory", addr),
        };
        fill(std::slice::from_raw_parts_mut(w as *mut u8, len));
        sys_icache_invalidate(addr as *mut u8, len);
    }
}

/// Windows: one reservation (MEM_RESERVE) whose pages are committed on demand in 1 MB granules
/// (`commit`, called by the unit's bump allocators and for its fixed parts) -- committing the
/// whole 1.8 GB up front would charge it against the commit limit. Code pages are committed
/// read-write-execute (no W^X: Windows allows RWX for ordinary processes; ACG-enabled hosts are
/// not supported), the rest read-write.
#[cfg(windows)]
mod imp {
    use crate::os::win32::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    const GRANULE: u64 = 1 << 20;
    /// the last page of the code area holds its unwind table entry (register_unwind)
    pub const CODE_TAIL: u64 = 4096;
    const REGIONS: usize = 1024;
    /// (base, code bytes, total bytes, RW alias of the code or 0, section handle) of each
    /// reservation; append-only (lookups read entries without the lock), a released one keeps
    /// its entry with total 0
    static RES: [[AtomicU64; 5]; REGIONS] = [const { [const { AtomicU64::new(0) }; 5] }; REGIONS];
    static RES_N: AtomicUsize = AtomicUsize::new(0);

    // W^X (JITSAFE fix 15): the code part of a reservation is two views of one pagefile-backed
    // section -- read-execute at the code addresses (what everyone calls and patches by address)
    // and a read-write alias elsewhere that only write_code / fill_code / the unwind entry write
    // through. The views replace a placeholder (VirtualAlloc2 / MapViewOfFile3), so the code
    // stays right below the reservation's data. No page is writable and executable at once.
    // MAKEPAD_RUN_WX=0, or a system without placeholders / sections, keeps one
    // PAGE_EXECUTE_READWRITE view.
    fn wx() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| !std::env::var_os("MAKEPAD_RUN_WX").is_some_and(|v| v == "0"))
    }
    static WX_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// [base, base + code + rest) with the code part as an RX view and an RW alias of one
    /// section: (base, alias, section). None: no placeholders here (nothing is left reserved).
    unsafe fn reserve_views(code: usize, rest: usize) -> Option<(u64, u64, u64)> {
        let me = GetCurrentProcess();
        let total = code + rest;
        let base = VirtualAlloc2(me, std::ptr::null_mut(), total, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, std::ptr::null_mut(), 0);
        if base.is_null() {
            return None;
        }
        // the code placeholder and the rest's
        if rest != 0 && VirtualFree(base, code, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) == 0 {
            VirtualFree(base, 0, MEM_RELEASE);
            return None;
        }
        let undo = |code_too: bool| {
            if code_too {
                VirtualFree(base, 0, MEM_RELEASE);
            }
            if rest != 0 {
                VirtualFree(base.add(code), 0, MEM_RELEASE);
            }
        };
        let sec = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_EXECUTE_READWRITE | SEC_RESERVE, (code as u64 >> 32) as u32, code as u32, std::ptr::null());
        if sec == 0 {
            undo(true);
            return None;
        }
        let rx = MapViewOfFile3(sec, me, base, 0, code, MEM_REPLACE_PLACEHOLDER, PAGE_EXECUTE_READ, std::ptr::null_mut(), 0);
        if rx != base {
            CloseHandle(sec);
            undo(true);
            return None;
        }
        let rw = MapViewOfFile3(sec, me, std::ptr::null_mut(), 0, code, 0, PAGE_READWRITE, std::ptr::null_mut(), 0);
        let rest_ok = rest == 0 || VirtualAlloc2(me, base.add(code), rest, MEM_RESERVE | MEM_REPLACE_PLACEHOLDER, PAGE_NOACCESS, std::ptr::null_mut(), 0) == base.add(code);
        if rw.is_null() || !rest_ok {
            if !rw.is_null() {
                UnmapViewOfFile(rw);
            }
            UnmapViewOfFile2(me, base, 0);
            CloseHandle(sec);
            if !rest_ok && rest != 0 {
                VirtualFree(base.add(code), 0, MEM_RELEASE);
            }
            return None;
        }
        Some((base as u64, rw as u64, sec as u64))
    }

    /// The RW alias of code address `addr` (None: no W^X view holds it).
    fn code_rw_of(addr: u64) -> Option<u64> {
        let n = RES_N.load(Ordering::Acquire);
        for r in &RES[..n] {
            if r[2].load(Ordering::Acquire) == 0 {
                continue;
            }
            let (base, code, rw) = (r[0].load(Ordering::Relaxed), r[1].load(Ordering::Relaxed), r[3].load(Ordering::Relaxed));
            if rw != 0 && addr >= base && addr - base < code {
                return Some(addr - base + rw);
            }
        }
        None
    }

    /// Where code bytes at `addr` are written: the RW alias, or (single RWX view) the address.
    #[track_caller]
    fn writable(addr: u64) -> u64 {
        match code_rw_of(addr) {
            Some(w) => w,
            None if !wx() || WX_FAILED.load(Ordering::Relaxed) => addr,
            None => panic!("write_code: {:#x} is not code memory", addr),
        }
    }
    /// committed granules: one bit each, per reservation (up to 4 GB each)
    static BITS: [[AtomicU64; 64]; REGIONS] = [const { [const { AtomicU64::new(0) }; 64] }; REGIONS];

    /// One RUNTIME_FUNCTION for the whole code area, its UNWIND_INFO describing the frame every
    /// Relax function has (`push rbp; mov rbp, rsp; ...`): unwinding a JIT frame sets rsp = rbp,
    /// pops rbp and returns -- exact for rip / rsp / rbp in every function body, so the SEH
    /// dispatcher, RtlVirtualUnwind and debuggers walk through JIT frames instead of stopping
    /// at them. Not described: callee-saved registers other than rbp (only a handler that
    /// resumes in a frame above JIT code would need them; Rust panics never cross JIT frames)
    /// and the first 4 bytes of each prologue / frameless glue (there the walk skips one frame).
    fn register_unwind(base: u64, code: usize) {
        // [RUNTIME_FUNCTION { begin, end, unwind rva }][UNWIND_INFO]: in the last page of the code
        // area, which the unit's code allocator leaves alone (CODE_TAIL)
        let at = base + code as u64 - 4096;
        commit(at, 4096);
        let info_rva = (at - base) as u32 + 16;
        unsafe {
            // written through the RW alias (W^X); the system reads it at `at`
            let rf = writable(at) as *mut u32;
            *rf = 0;
            *rf.add(1) = (code - 4096) as u32;
            *rf.add(2) = info_rva;
            // UNWIND_INFO: version 1, flags 0, prolog 4 bytes, 2 codes, frame register rbp (5) at offset 0
            let ui = (rf as *mut u8).add(16);
            *ui = 1;
            *ui.add(1) = 4;
            *ui.add(2) = 2;
            *ui.add(3) = 5;
            // codes in reverse prologue order: UWOP_SET_FPREG (3) at 4, UWOP_PUSH_NONVOL (0) rbp at 1
            *ui.add(4) = 4;
            *ui.add(5) = 3;
            *ui.add(6) = 1;
            *ui.add(7) = (5 << 4) | 0;
            if RtlAddFunctionTable(at as *const u32, 1, base) == 0 {
                panic!("RtlAddFunctionTable failed");
            }
        }
    }

    /// reserve / release
    static RES_LOCK: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

    pub fn reserve(code: usize, rest: usize) -> u64 {
        let total = code + rest;
        assert!(total as u64 <= 64 * 64 * GRANULE, "reservation: too large");
        let mut views = None;
        if code != 0 && wx() && !WX_FAILED.load(Ordering::Relaxed) {
            views = unsafe { reserve_views(code, rest) };
            if views.is_none() {
                WX_FAILED.store(true, Ordering::Relaxed);
                eprintln!("makepad run: no placeholder views for W^X code memory here: code pages are read-write-execute");
            }
        }
        let (base, rw, sec) = match views {
            Some(v) => v,
            None => {
                let base = unsafe { VirtualAlloc(std::ptr::null_mut(), total, MEM_RESERVE, PAGE_NOACCESS) };
                if base.is_null() {
                    panic!("reservation: VirtualAlloc MEM_RESERVE failed");
                }
                (base as u64, 0, 0)
            }
        };
        let _g = RES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = RES_N.load(Ordering::Acquire);
        assert!(n < REGIONS, "reservation: more than {} code reservations in this process", REGIONS);
        RES[n][0].store(base, Ordering::Relaxed);
        RES[n][1].store(code as u64, Ordering::Relaxed);
        RES[n][3].store(rw, Ordering::Relaxed);
        RES[n][4].store(sec, Ordering::Relaxed);
        // the size last: an entry with a size is live
        RES[n][2].store(total as u64, Ordering::Release);
        RES_N.store(n + 1, Ordering::Release);
        drop(_g);
        register_unwind(base, code);
        base
    }

    /// Gives a reservation back (an auxiliary unit dropped mid-run): its unwind table entry,
    /// its slot (emptied, reused by the next reservation) and its address space.
    pub fn release(base: u64, code: usize, _rest: usize) {
        let _g = RES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = RES_N.load(Ordering::Acquire);
        for r in 0..n {
            if RES[r][0].load(Ordering::Relaxed) == base && RES[r][2].load(Ordering::Relaxed) != 0 {
                let total = RES[r][2].load(Ordering::Relaxed) as usize;
                let (rw, sec) = (RES[r][3].load(Ordering::Relaxed), RES[r][4].load(Ordering::Relaxed));
                RES[r][2].store(0, Ordering::Release);
                unsafe {
                    RtlDeleteFunctionTable((base + code as u64 - 4096) as *const u32);
                    // decommitted, still reserved: its addresses are never handed out again
                    // (see unix::retire); W^X views: the alias goes, the RX view becomes a
                    // placeholder again (reserved, nothing maps there unless asked to)
                    if rw != 0 {
                        UnmapViewOfFile(rw as *mut u8);
                        UnmapViewOfFile2(GetCurrentProcess(), base as *mut u8, MEM_PRESERVE_PLACEHOLDER);
                        CloseHandle(sec as HANDLE);
                        if total > code {
                            VirtualFree((base + code as u64) as *mut u8, total - code, MEM_DECOMMIT);
                        }
                    } else {
                        VirtualFree(base as *mut u8, total, MEM_DECOMMIT);
                    }
                }
                return;
            }
        }
    }

    /// Committed pages fault in as they are first written (see unix::prefault).
    pub fn prefault(_addr: u64, _len: usize) {}

    /// Makes [addr, addr + len) of a reservation usable (whole granules, once each).
    pub fn commit(addr: u64, len: usize) {
        if len == 0 {
            return;
        }
        let n = RES_N.load(Ordering::Acquire);
        for r in 0..n {
            let total = RES[r][2].load(Ordering::Acquire);
            let (base, code, rw) = (RES[r][0].load(Ordering::Relaxed), RES[r][1].load(Ordering::Relaxed), RES[r][3].load(Ordering::Relaxed));
            if addr < base || addr - base >= total {
                continue;
            }
            let first = (addr - base) / GRANULE;
            let last = (addr - base + len as u64 - 1).min(total - 1) / GRANULE;
            for g in first..=last {
                let (w, b) = ((g / 64) as usize, 1u64 << (g % 64));
                if BITS[r][w].load(Ordering::Acquire) & b != 0 {
                    continue;
                }
                let at = base + g * GRANULE;
                let size = GRANULE.min(base + total - at);
                // the granule's code part and its data part (one of them may be empty)
                let split = (base + code).clamp(at, at + size);
                let (csize, dsize) = ((split - at) as usize, (at + size - split) as usize);
                let mut ok = dsize == 0 || !unsafe { VirtualAlloc(split as *mut u8, dsize, MEM_COMMIT, PAGE_READWRITE) }.is_null();
                if csize != 0 {
                    ok &= if rw != 0 {
                        // the section's pages, committed through the alias (read-write) and the
                        // code view (read-execute)
                        unsafe { !VirtualAlloc((rw + (at - base)) as *mut u8, csize, MEM_COMMIT, PAGE_READWRITE).is_null() && !VirtualAlloc(at as *mut u8, csize, MEM_COMMIT, PAGE_EXECUTE_READ).is_null() }
                    } else {
                        !unsafe { VirtualAlloc(at as *mut u8, csize, MEM_COMMIT, PAGE_EXECUTE_READWRITE) }.is_null()
                    };
                }
                if !ok {
                    panic!("out of memory: committing {:#x} (+{:#x})", at, size);
                }
                BITS[r][w].fetch_or(b, Ordering::AcqRel);
            }
            return;
        }
    }

    pub fn alloc_rw(size: usize) -> u64 {
        let p = unsafe { VirtualAlloc(std::ptr::null_mut(), size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        if p.is_null() {
            panic!("VirtualAlloc failed");
        }
        p as u64
    }

    pub fn free(addr: u64, _len: usize) {
        unsafe {
            VirtualFree(addr as *mut u8, 0, MEM_RELEASE);
        }
    }

    /// Writes machine code: a copy through the RW alias (W^X; the RWX view itself without it),
    /// then the instruction cache flush the docs ask for after writing code (FlushInstructionCache).
    #[track_caller]
    pub unsafe fn write_code(addr: u64, b: &[u8]) {
        std::ptr::copy_nonoverlapping(b.as_ptr(), writable(addr) as *mut u8, b.len());
        FlushInstructionCache(GetCurrentProcess(), addr as *const u8, b.len());
    }

    /// Writes `len` bytes of machine code at `addr` in place (`fill` gets them writable; what
    /// they hold before is unspecified), as `write_code` would.
    #[track_caller]
    pub unsafe fn fill_code(addr: u64, len: usize, fill: impl FnOnce(&mut [u8])) {
        fill(std::slice::from_raw_parts_mut(writable(addr) as *mut u8, len));
        FlushInstructionCache(GetCurrentProcess(), addr as *const u8, len);
    }

    rt_fn! {
        leaf fn rt_memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
            unsafe { std::ptr::copy_nonoverlapping(s, d, n) };
            d
        }
    }
    rt_fn! {
        leaf fn rt_memmove(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
            unsafe { std::ptr::copy(s, d, n) };
            d
        }
    }
    rt_fn! {
        leaf fn rt_memset(d: *mut u8, c: i32, n: usize) -> *mut u8 {
            unsafe { std::ptr::write_bytes(d, c as u8, n) };
            d
        }
    }

    /// memcpy / memmove / memset in Relax's convention (SysV): the C runtime's are Win64.
    pub fn mem_fns() -> (u64, u64, u64) {
        (rt_memcpy as *const () as usize as u64, rt_memmove as *const () as usize as u64, rt_memset as *const () as usize as u64)
    }
}
