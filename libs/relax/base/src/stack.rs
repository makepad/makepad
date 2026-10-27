//! The compiler's stack: deep recursions (type relation, folding, constant evaluation that runs
//! program code which compiles again) continue on a fresh segment when the thread's stack is
//! nearly used up (rustc_data_structures::stack::ensure_sufficient_stack: a red zone, then a new
//! segment via stacker). A compile runs on the program's thread (lazy compile from a stub), so
//! neither a type nested 1000 deep (`S<S<..S<Z>..>>`, rustc tests/ui/associated-consts/
//! issue-93775.rs) nor a lazy compile reached near the end of a deep program recursion overflows
//! the thread's stack.
//!
//! - `ensure_sufficient_stack(f)`: runs `f` here, or on a fresh segment when less than the red
//!   zone is left. Cheap enough for every recursion step (a thread-local read and a compare).

/// Run on a fresh segment when less than this is left
const STACK_RED_ZONE: usize = 1 << 20;
/// Size of a fresh segment (virtual; pages are touched on use)
const STACK_SEGMENT: usize = 64 << 20;

thread_local! {
    /// lowest usable address of the stack this thread runs on now (0: not known yet)
    static STACK_LO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// the program stack this thread runs on (`on_program_stack`): usable [lo, hi), or (0, 0)
    static PROGRAM_STACK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The program stack the calling thread runs on now (`on_program_stack`): usable (lo, hi).
/// Stack walkers and the fault reports bound their reads by it instead of the OS thread's stack.
pub fn program_stack_bounds() -> Option<(usize, usize)> {
    PROGRAM_STACK.try_with(|c| c.get()).ok().filter(|b| b.1 != 0)
}

/// Size of a program thread's stack reservation: a quarter of physical memory, 1..64 GiB
/// (MAKEPAD_RUN_STACK_GB overrides). Pages are committed as the stack grows into them, so the size
/// costs address space only; the bound keeps a runaway recursion from paging the machine to death
/// before it reaches the guard.
pub fn program_stack_size() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        const GB: usize = 1 << 30;
        if let Some(g) = std::env::var("MAKEPAD_RUN_STACK_GB").ok().and_then(|v| v.parse::<usize>().ok()) {
            return g.clamp(1, 1024) * GB;
        }
        if cfg!(not(target_pointer_width = "64")) {
            return 64 << 20;
        }
        (imp::physical_memory() / 4).clamp(GB, 64 * GB)
    })
}

/// Runs `f` on a program stack: one large reservation (`program_stack_size`, halved until the
/// system grants one), committed as it is touched, with a no-access guard below it. A recursion
/// that overruns it faults on the guard (reported as a stack overflow), never writes past it.
/// `bounds_changed` runs on the new stack before `f` and again back on the thread's own stack
/// after it (callers refresh their cached stack bounds there).
/// Without a reservation (none granted) `f` runs on the thread's own stack.
pub fn on_program_stack<F: FnOnce()>(bounds_changed: fn(), f: F) {
    let mut len = program_stack_size();
    let seg = loop {
        if let Some(s) = imp::reserve_stack(len) {
            break Some(s);
        }
        if len <= (64 << 20) {
            break None;
        }
        len /= 2;
    };
    let Some(seg) = seg else {
        f();
        return;
    };
    struct Job<F> {
        f: Option<F>,
        bounds_changed: fn(),
        r: Option<std::thread::Result<()>>,
    }
    extern "C" fn run<F: FnOnce()>(p: *mut u8) {
        let job = unsafe { &mut *(p as *mut Job<F>) };
        (job.bounds_changed)();
        let f = job.f.take().unwrap();
        // a Rust panic must not unwind through the stack switch (no unwind info for it)
        job.r = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    }
    let mut job: Job<F> = Job { f: Some(f), bounds_changed, r: None };
    let (old_lo, old_ps) = (STACK_LO.with(|c| c.get()), PROGRAM_STACK.with(|c| c.get()));
    STACK_LO.with(|c| c.set(seg.floor));
    PROGRAM_STACK.with(|c| c.set((seg.floor, seg.top)));
    unsafe { imp::call_on_segment(&seg, run::<F>, &mut job as *mut Job<F> as *mut u8) };
    STACK_LO.with(|c| c.set(old_lo));
    PROGRAM_STACK.with(|c| c.set(old_ps));
    bounds_changed();
    imp::free_segment(seg);
    if let Some(Err(e)) = job.r.take() {
        std::panic::resume_unwind(e);
    }
}

/// Runs `f` on a fresh stack segment when this thread's stack is nearly used up.
#[inline]
pub fn ensure_sufficient_stack<R, F: FnOnce() -> R>(f: F) -> R {
    let here = &f as *const _ as usize;
    let lo = STACK_LO.with(|c| c.get());
    if lo != 0 && here.saturating_sub(lo) >= STACK_RED_ZONE {
        return f();
    }
    grow(f)
}

#[inline(never)]
fn grow<R, F: FnOnce() -> R>(f: F) -> R {
    let here = &f as *const _ as usize;
    let mut lo = STACK_LO.with(|c| c.get());
    if lo == 0 {
        lo = program_stack_bounds().map_or_else(stack_lo, |b| b.0);
        STACK_LO.with(|c| c.set(lo));
    }
    if lo == 0 || here.saturating_sub(lo) >= STACK_RED_ZONE {
        return f();
    }
    struct Job<F, R> {
        f: Option<F>,
        r: Option<std::thread::Result<R>>,
    }
    extern "C" fn run<F: FnOnce() -> R, R>(p: *mut u8) {
        let job = unsafe { &mut *(p as *mut Job<F, R>) };
        let f = job.f.take().unwrap();
        // a Rust panic must not unwind through the stack switch (no unwind info for it)
        job.r = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    }
    let seg = match imp::stack_segment(STACK_SEGMENT) {
        Some(seg) => seg,
        None => return f(),
    };
    let mut job: Job<F, R> = Job { f: Some(f), r: None };
    STACK_LO.with(|c| c.set(seg.lo));
    unsafe { imp::call_on_segment(&seg, run::<F, R>, &mut job as *mut Job<F, R> as *mut u8) };
    STACK_LO.with(|c| c.set(lo));
    imp::free_segment(seg);
    match job.r.take().unwrap() {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// A stack segment: usable [lo, top), mapped [base, base + len); `floor`: the lowest address the
/// stack may grow to (above the guard; = lo unless the OS commits the segment as it grows).
struct Segment {
    base: usize,
    len: usize,
    lo: usize,
    floor: usize,
    top: usize,
}

/// lowest usable address of the calling thread's stack (0: unknown)
#[cfg(any(target_os = "linux", target_os = "android"))]
fn stack_lo() -> usize {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
        fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attr: *mut u64) -> i32;
    }
    unsafe {
        let mut attr = [0u64; 16];
        if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
            return 0;
        }
        let (mut addr, mut size) = (0usize, 0usize);
        pthread_attr_getstack(attr.as_ptr(), &mut addr, &mut size);
        pthread_attr_destroy(attr.as_mut_ptr());
        addr
    }
}

#[cfg(target_os = "macos")]
fn stack_lo() -> usize {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
        fn pthread_get_stacksize_np(t: usize) -> usize;
    }
    unsafe {
        let t = pthread_self();
        pthread_get_stackaddr_np(t) as usize - pthread_get_stacksize_np(t)
    }
}

#[cfg(windows)]
fn stack_lo() -> usize {
    let (mut lo, mut hi) = (0usize, 0usize);
    unsafe { relax_session::win32::GetCurrentThreadStackLimits(&mut lo, &mut hi) };
    // the reservation's base; its lowest pages are the guard region
    if lo == 0 {
        0
    } else {
        lo + 3 * 4096 + 65536
    }
}

#[cfg(not(any(unix, windows)))]
fn stack_lo() -> usize {
    0
}

#[cfg(unix)]
mod imp {
    use super::Segment;
    use std::ffi::c_void;
    extern "C" {
        #[cfg_attr(all(target_os = "linux", target_pointer_width = "32"), link_name = "mmap64")]
        fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
        fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    }
    const PROT_RW: i32 = 3;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const MAP_PRIVATE_ANON: i32 = 0x22;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const MAP_PRIVATE_ANON: i32 = 0x1002;

    /// Guard bytes below a stack segment.
    const GUARD: usize = 16384;

    pub(super) fn stack_segment(len: usize) -> Option<Segment> {
        let seg = unsafe { mmap(std::ptr::null_mut(), len, PROT_RW, MAP_PRIVATE_ANON, -1, 0) };
        if seg as isize == -1 {
            return None;
        }
        // a guard page at the low end: an overflow faults instead of writing below the segment
        unsafe { mprotect(seg, GUARD, 0) };
        crate::memstat::name_mapping(seg as usize, len, c"makepad-run-stack");
        Some(Segment { base: seg as usize, len, lo: seg as usize + GUARD, floor: seg as usize + GUARD, top: (seg as usize + len) & !15 })
    }

    pub(super) fn free_segment(s: Segment) {
        unsafe { munmap(s.base as *mut c_void, s.len) };
    }

    /// Guard bytes below a program stack (big frames probe every page, so any access below the
    /// stack's end lands here first).
    const PROGRAM_GUARD: usize = 1 << 20;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const MAP_NORESERVE: i32 = 0x4000;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const MAP_NORESERVE: i32 = 0;

    /// A program stack: a lazily committed reservation (Linux: no commit charge until touched),
    /// its lowest PROGRAM_GUARD bytes no-access.
    pub(super) fn reserve_stack(len: usize) -> Option<Segment> {
        let seg = unsafe { mmap(std::ptr::null_mut(), len, PROT_RW, MAP_PRIVATE_ANON | MAP_NORESERVE, -1, 0) };
        if seg as isize == -1 || seg.is_null() {
            return None;
        }
        if unsafe { mprotect(seg, PROGRAM_GUARD, 0) } != 0 {
            unsafe { munmap(seg, len) };
            return None;
        }
        crate::memstat::name_mapping(seg as usize, len, c"makepad-run-program-stack");
        let lo = seg as usize + PROGRAM_GUARD;
        Some(Segment { base: seg as usize, len, lo, floor: lo, top: (seg as usize + len) & !15 })
    }

    pub(super) fn physical_memory() -> usize {
        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const PAGES: (i32, i32) = (85, 30); // _SC_PHYS_PAGES, _SC_PAGESIZE
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const PAGES: (i32, i32) = (200, 29);
        let (n, p) = unsafe { (sysconf(PAGES.0), sysconf(PAGES.1)) };
        if n > 0 && p > 0 { (n as usize).saturating_mul(p as usize) } else { 16 << 30 }
    }

    /// Calls `f(data)` with the stack pointer at the segment's top, back on this stack after.
    #[inline(never)]
    pub(super) unsafe fn call_on_segment(s: &Segment, f: extern "C" fn(*mut u8), data: *mut u8) {
        let sp = s.top;
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!(
            "mov x20, sp",
            "mov sp, {sp}",
            "blr {f}",
            "mov sp, x20",
            sp = in(reg) sp,
            f = in(reg) f,
            in("x0") data,
            out("x20") _,
            clobber_abi("C"),
        );
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!(
            "mov r12, rsp",
            "mov rsp, {sp}",
            "call {f}",
            "mov rsp, r12",
            sp = in(reg) sp,
            f = in(reg) f,
            in("rdi") data,
            out("r12") _,
            clobber_abi("C"),
        );
    }
}

#[cfg(windows)]
mod imp {
    use super::Segment;
    use relax_session::win32::*;

    const GUARD: usize = 65536;

    /// A committed segment with a no-access guard at its low end.
    pub(super) fn stack_segment(len: usize) -> Option<Segment> {
        let seg = unsafe { VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        if seg.is_null() {
            return None;
        }
        let mut old = 0u32;
        unsafe { VirtualProtect(seg, GUARD, PAGE_NOACCESS, &mut old) };
        Some(Segment { base: seg as usize, len, lo: seg as usize + GUARD, floor: seg as usize + GUARD, top: (seg as usize + len) & !15 })
    }

    pub(super) fn free_segment(s: Segment) {
        let _ = s.len;
        unsafe { VirtualFree(s.base as *mut u8, 0, MEM_RELEASE) };
    }

    const PAGE_GUARD: u32 = 0x100;
    /// committed at first: the top of a program stack, then its guard page
    const INITIAL: usize = 1 << 20;
    /// kept reserved (never committed) above the reservation's base: the stack overflows here
    const PROGRAM_GUARD: usize = 1 << 20;

    /// A program stack as a fiber's: reserved, its top committed with a guard page below that
    /// the system moves down as the stack grows (TEB StackLimit; call_on_segment sets the
    /// bounds). The commit charge follows use.
    pub(super) fn reserve_stack(len: usize) -> Option<Segment> {
        let base = unsafe { VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE, PAGE_NOACCESS) };
        if base.is_null() {
            return None;
        }
        let top = (base as usize + len) & !15;
        let lo = base as usize + len - INITIAL;
        let ok = unsafe {
            !VirtualAlloc(lo as *mut u8, INITIAL, MEM_COMMIT, PAGE_READWRITE).is_null()
                && !VirtualAlloc((lo - 4096) as *mut u8, 4096, MEM_COMMIT, PAGE_READWRITE | PAGE_GUARD).is_null()
        };
        if !ok {
            unsafe { VirtualFree(base, 0, MEM_RELEASE) };
            return None;
        }
        Some(Segment { base: base as usize, len, lo, floor: base as usize + PROGRAM_GUARD, top })
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(s: *mut MemoryStatusEx) -> i32;
    }

    pub(super) fn physical_memory() -> usize {
        let mut m = MemoryStatusEx { length: std::mem::size_of::<MemoryStatusEx>() as u32, load: 0, total_phys: 0, avail_phys: 0, total_page_file: 0, avail_page_file: 0, total_virtual: 0, avail_virtual: 0, avail_extended_virtual: 0 };
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 { m.total_phys as usize } else { 16 << 30 }
    }

    /// Calls `f(data)` (Win64: data in rcx, 32 bytes of shadow space) on the segment. The TEB's
    /// stack bounds (NT_TIB StackBase gs:[8], StackLimit gs:[16], DeallocationStack gs:[0x1478])
    /// describe the segment for the call: the exception dispatcher and RtlUnwindEx check frames
    /// against them (a Rust panic caught inside `f` unwinds there), as for a fiber's stack.
    #[inline(never)]
    pub(super) unsafe fn call_on_segment(s: &Segment, f: extern "C" fn(*mut u8), data: *mut u8) {
        std::arch::asm!(
            "push qword ptr gs:[8]",
            "push qword ptr gs:[16]",
            "push qword ptr gs:[0x1478]",
            "mov r12, rsp",
            "mov gs:[8], {top}",
            "mov gs:[16], {lo}",
            "mov gs:[0x1478], {base}",
            "mov rsp, {top}",
            "sub rsp, 32",
            "call {f}",
            "mov rsp, r12",
            "pop qword ptr gs:[0x1478]",
            "pop qword ptr gs:[16]",
            "pop qword ptr gs:[8]",
            top = in(reg) s.top,
            lo = in(reg) s.lo,
            base = in(reg) s.base,
            f = in(reg) f,
            in("rcx") data,
            out("r12") _,
            clobber_abi("C"),
        );
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::Segment;
    pub(super) fn stack_segment(_len: usize) -> Option<Segment> {
        None
    }
    pub(super) fn free_segment(_s: Segment) {}
    pub(super) fn reserve_stack(_len: usize) -> Option<Segment> {
        None
    }
    pub(super) fn physical_memory() -> usize {
        1 << 30
    }
    pub(super) unsafe fn call_on_segment(_s: &Segment, _f: extern "C" fn(*mut u8), _data: *mut u8) {}
}

#[cfg(test)]
mod tests {
    /// a recursion far deeper than the test thread's stack completes, on segments
    #[test]
    fn deep_recursion_grows_onto_segments() {
        fn down(n: u32) -> u32 {
            let pad = [n as u8; 4096];
            if n == 0 {
                return 0;
            }
            super::ensure_sufficient_stack(|| down(n - 1)) + std::hint::black_box(pad)[0] as u32 / 255
        }
        // 4 KiB a level, 8192 levels: 32 MiB, beyond a 2 MiB test thread
        let h = std::thread::Builder::new().stack_size(2 << 20).spawn(|| down(8192)).unwrap();
        assert!(h.join().unwrap() <= 8192);
    }
}
