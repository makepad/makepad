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
        lo = stack_lo();
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

/// A stack segment: usable [lo, top), mapped [base, base + len).
struct Segment {
    base: usize,
    len: usize,
    lo: usize,
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
        Some(Segment { base: seg as usize, len, lo: seg as usize + GUARD, top: (seg as usize + len) & !15 })
    }

    pub(super) fn free_segment(s: Segment) {
        unsafe { munmap(s.base as *mut c_void, s.len) };
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
        Some(Segment { base: seg as usize, len, lo: seg as usize + GUARD, top: (seg as usize + len) & !15 })
    }

    pub(super) fn free_segment(s: Segment) {
        let _ = s.len;
        unsafe { VirtualFree(s.base as *mut u8, 0, MEM_RELEASE) };
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
