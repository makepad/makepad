//! The compiler's stack: deep recursions (type relation, folding, constant evaluation that runs
//! program code which compiles again) continue on a fresh segment when the thread's stack is
//! nearly used up (rustc_data_structures::stack::ensure_sufficient_stack: a red zone, then a new
//! segment via stacker). A compile runs on the program's thread (lazy compile from a stub), so
//! neither a type nested 1000 deep (`S<S<..S<Z>..>>`, rustc tests/ui/associated-consts/
//! issue-93775.rs) nor a lazy compile reached near the end of a deep program recursion overflows
//! the thread's stack.
//!
//! - `ensure_sufficient_stack(f)`: runs `f` here, or on a fresh segment when less than the red
//!   zone is left, or when it runs on a stack this module does not know (an alternate signal
//!   stack: a lazy compile a signal handler's first call starts). Cheap enough for every
//!   recursion step (a thread-local read and two compares).
//! - `on_program_stack(f)`: a program thread's body on a large guarded reservation; finished
//!   ones are kept for the next thread (`Spares`).

/// Run on a fresh segment when less than this is left
const STACK_RED_ZONE: usize = 1 << 20;
/// Size of a fresh segment (virtual; pages are touched on use)
const STACK_SEGMENT: usize = 64 << 20;

thread_local! {
    /// the stack this thread runs on now, usable [lo, hi): its own, a program stack or a segment
    /// (0, 0: not known yet)
    static STACK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    /// the OS thread's own stack, usable [lo, hi) (0, 0: not read yet, or unknown)
    static THREAD_STACK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    /// the program stack this thread runs on (`on_program_stack`): usable [lo, hi), or (0, 0)
    static PROGRAM_STACK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The return address of the stack switch's call (`imp::call_on_segment`: the same in every
/// switch; 0 before the first). Each switch stores it before it switches.
static SWITCH_RETURN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The stack switch as a stack walker sees it: a frame whose pc is this return address is
/// `call_on_segment`'s, which runs on the thread's own stack while its callee runs on the segment.
/// Its caller is not above it: x86_64 walkers take its sp from the switch record
/// (`SWITCH_RECORD`); arm64's frame records chain across the switch by fp. 0 before the first
/// switch.
pub fn switch_return_pc() -> u64 {
    SWITCH_RETURN.load(std::sync::atomic::Ordering::Relaxed)
}

/// x86_64: the switch record, the sp `call_on_segment` has on the thread's own stack, sits at the
/// segment's sp at `switch_return_pc` plus this (above the callee's shadow space on Windows).
#[cfg(target_arch = "x86_64")]
pub const SWITCH_RECORD: u64 = if cfg!(windows) { 32 } else { 0 };

/// The program stack the calling thread runs on now (`on_program_stack`): usable (lo, hi).
/// Stack walkers and the fault reports bound their reads by it instead of the OS thread's stack.
pub fn program_stack_bounds() -> Option<(usize, usize)> {
    PROGRAM_STACK.try_with(|c| c.get()).ok().filter(|b| b.1 != 0)
}

/// The no-access guard below a program stack (`program_stack_bounds`' lo), in bytes.
#[cfg(any(unix, windows))]
pub const PROGRAM_STACK_GUARD: usize = imp::PROGRAM_GUARD;

/// Size of a program thread's stack reservation: a quarter of physical memory, 1..64 GiB
/// (RELAX_STACK_GB overrides). Pages are committed as the stack grows into them, so the size
/// costs address space only; the bound keeps a runaway recursion from paging the machine to death
/// before it reaches the guard.
pub fn program_stack_size() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        const GB: usize = 1 << 30;
        if let Some(g) = std::env::var("RELAX_STACK_GB").ok().and_then(|v| v.parse::<usize>().ok()) {
            return g.clamp(1, 1024) * GB;
        }
        if cfg!(not(target_pointer_width = "64")) {
            return 64 << 20;
        }
        (imp::physical_memory() / 4).clamp(GB, 64 * GB)
    })
}

/// The machine's RAM in bytes (16 GB where the system does not say).
pub fn physical_memory() -> usize {
    imp::physical_memory()
}

/// Runs `f` on a program stack: one large reservation (`program_stack_size`, halved until the
/// system grants one; or one a finished thread left, `Spares`), committed as it is touched, with
/// a no-access guard below it. A recursion that overruns it faults on the guard (reported as a
/// stack overflow), never writes past it.
/// `bounds_changed` runs on the new stack before `f` and again back on the thread's own stack
/// after it (callers refresh their cached stack bounds there).
/// Without a reservation (none granted) `f` runs on the thread's own stack.
pub fn on_program_stack<F: FnOnce()>(bounds_changed: fn(), f: F) {
    on_program_stack_from(&SPARE_STACKS, bounds_changed, f)
}

/// `on_program_stack` with the stack taken from, and given back to, `spares`.
fn on_program_stack_from<F: FnOnce()>(spares: &Spares<Segment>, bounds_changed: fn(), f: F) {
    let Some(seg) = program_stack(spares) else {
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
    let (old, old_ps) = (STACK.with(|c| c.get()), PROGRAM_STACK.with(|c| c.get()));
    STACK.with(|c| c.set((seg.floor, seg.top)));
    PROGRAM_STACK.with(|c| c.set((seg.floor, seg.top)));
    unsafe { imp::call_on_segment(&seg, run::<F>, &mut job as *mut Job<F> as *mut u8) };
    STACK.with(|c| c.set(old));
    PROGRAM_STACK.with(|c| c.set(old_ps));
    bounds_changed();
    program_stack_done(spares, seg);
    if let Some(Err(e)) = job.r.take() {
        std::panic::resume_unwind(e);
    }
}

/// Bytes at the top of a program stack a finished thread may have used and the stack still be
/// kept for the next one (those pages stay committed): more than a native thread's whole stack,
/// and room for the 2 MB pages Linux may back the stack's top with (transparent huge pages).
const PROGRAM_STACK_KEEP: usize = 4 << 20;

/// Program stacks no thread runs on, for the next thread's `on_program_stack`.
static SPARE_STACKS: Spares<Segment> = Spares::new();

/// A program stack: a spare one, else a new reservation (`program_stack_size`, halved until the
/// system grants one; None: not even 64 MB).
fn program_stack(spares: &Spares<Segment>) -> Option<Segment> {
    if let Some(s) = spares.take() {
        return Some(s);
    }
    let mut len = program_stack_size();
    loop {
        if let Some(s) = imp::reserve_stack(len) {
            return Some(s);
        }
        if len <= (64 << 20) {
            return None;
        }
        len /= 2;
    }
}

/// A thread is done with program stack `s`: it is kept for the next thread unless that thread
/// went below its top PROGRAM_STACK_KEEP bytes (the deep pages would stay committed) or enough
/// are kept already; else it is unmapped.
fn program_stack_done(spares: &Spares<Segment>, s: Segment) {
    if imp::used_below(&s, s.top - PROGRAM_STACK_KEEP) {
        imp::free_segment(s);
    } else if let Some(s) = spares.keep(s) {
        imp::free_segment(s);
    }
}

/// Per-thread mappings no thread uses now (program stacks; the JIT's fault stacks and
/// thread-local blocks), kept for the next thread: a thread's start then maps nothing and its
/// end unmaps nothing. (Mapping them per thread cost more with every thread the process had
/// started on macOS: a long session that starts threads per task slowed down task by task.)
/// A fixed array under a lock, so a thread's exit returns one without allocating.
pub struct Spares<T: Copy> {
    kept: relax_session::sync::Mutex<(usize, [Option<T>; SPARES])>,
}

/// How many `Spares` keeps: as many threads as a machine's cores run at once
const SPARES: usize = if cfg!(target_pointer_width = "64") { 32 } else { 4 };

impl<T: Copy> Spares<T> {
    pub const fn new() -> Self {
        Spares { kept: relax_session::sync::Mutex::new((0, [None; SPARES])) }
    }

    /// One kept item, if any.
    pub fn take(&self) -> Option<T> {
        let mut k = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if k.0 == 0 {
            return None;
        }
        k.0 -= 1;
        let n = k.0;
        k.1[n].take()
    }

    /// `take` where the lock is free, else None: a signal handler may have interrupted its own
    /// thread inside `take` or `keep`.
    pub fn try_take(&self) -> Option<T> {
        let mut k = self.kept.try_lock().ok()?;
        if k.0 == 0 {
            return None;
        }
        k.0 -= 1;
        let n = k.0;
        k.1[n].take()
    }

    /// `keep` where the lock is free, else `x` back (as for full spares).
    pub fn try_keep(&self, x: T) -> Option<T> {
        let Ok(mut k) = self.kept.try_lock() else { return Some(x) };
        if k.0 == SPARES {
            return Some(x);
        }
        let n = k.0;
        k.1[n] = Some(x);
        k.0 += 1;
        None
    }

    /// Keeps `x` for a later `take`; gives it back when the spares are full (the caller frees it).
    pub fn keep(&self, x: T) -> Option<T> {
        let mut k = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if k.0 == SPARES {
            return Some(x);
        }
        let n = k.0;
        k.1[n] = Some(x);
        k.0 += 1;
        None
    }
}

/// Runs `f` on a fresh stack segment when this thread's stack is nearly used up, or when it runs on
/// a stack this module did not make or read (an alternate signal stack, a library's fiber), whose
/// room it cannot know.
#[inline]
pub fn ensure_sufficient_stack<R, F: FnOnce() -> R>(f: F) -> R {
    let here = &f as *const _ as usize;
    let (lo, hi) = STACK.with(|c| c.get());
    if lo <= here && here < hi && here - lo >= STACK_RED_ZONE {
        return f();
    }
    grow(f)
}

#[inline(never)]
fn grow<R, F: FnOnce() -> R>(f: F) -> R {
    let here = &f as *const _ as usize;
    let (mut lo, mut hi) = STACK.with(|c| c.get());
    if !(lo <= here && here < hi) {
        // read again: the thread may have moved to or from a program stack
        (lo, hi) = program_stack_bounds().unwrap_or_else(thread_stack);
        STACK.with(|c| c.set((lo, hi)));
    }
    if lo <= here && here < hi && here - lo >= STACK_RED_ZONE {
        return f();
    }
    // near the end of the stack, or on one outside it: a signal stack never has the red zone's
    // room (64 KB), and a compile there overflowed it with the signal blocked (the kernel's kill,
    // no report)
    struct Job<F, R> {
        f: Option<F>,
        r: Option<std::thread::Result<R>>,
        alt: Option<imp::AltRoom>,
    }
    extern "C" fn run<F: FnOnce() -> R, R>(p: *mut u8) {
        let job = unsafe { &mut *(p as *mut Job<F, R>) };
        if let Some(alt) = &job.alt {
            alt.on_segment();
        }
        let f = job.f.take().unwrap();
        // a Rust panic must not unwind through the stack switch (no unwind info for it)
        job.r = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
        if let Some(alt) = &job.alt {
            alt.off_segment();
        }
    }
    let seg = match SPARE_SEGMENTS.try_take().or_else(|| imp::stack_segment(STACK_SEGMENT)) {
        Some(seg) => seg,
        None => return f(),
    };
    let mut job: Job<F, R> = Job { f: Some(f), r: None, alt: None };
    // a signal handler's call, from the alternate signal stack: that stack is the room below the
    // handler's frames while the call runs on the segment (`imp::AltRoom`)
    job.alt = imp::AltRoom::below(&job as *const Job<F, R> as usize);
    STACK.with(|c| c.set((seg.lo, seg.top)));
    unsafe { imp::call_on_segment(&seg, run::<F, R>, &mut job as *mut Job<F, R> as *mut u8) };
    if let Some(alt) = &job.alt {
        alt.back();
    }
    STACK.with(|c| c.set((lo, hi)));
    segment_done(seg);
    match job.r.take().unwrap() {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// Segments `grow` is done with, for the next one: a signal handler whose first calls compile
/// maps one segment, not one per compile. (Taken and kept without waiting: `grow` may run in a
/// handler that interrupted its thread inside either.)
static SPARE_SEGMENTS: Spares<Segment> = Spares::new();

/// `grow` is done with segment `s`: kept for the next unless it was used deep (its pages would
/// stay committed) or enough are kept; else unmapped.
fn segment_done(s: Segment) {
    if imp::used_below(&s, s.top - PROGRAM_STACK_KEEP) {
        imp::free_segment(s);
    } else if let Some(s) = SPARE_SEGMENTS.try_keep(s) {
        imp::free_segment(s);
    }
}

/// The OS thread's own stack, usable [lo, hi), read once per thread ((0, 0): unknown).
fn thread_stack() -> (usize, usize) {
    let b = THREAD_STACK.with(|c| c.get());
    if b.1 != 0 {
        return b;
    }
    let b = stack_bounds();
    THREAD_STACK.with(|c| c.set(b));
    b
}

/// Reads the calling thread's own stack bounds now, in normal context (a thread's first lazy
/// compile), so that a check made later in a signal handler does not ask the OS there (glibc reads
/// /proc/self/maps for the main thread's, and allocates).
pub fn note_thread_stack() {
    let b = thread_stack();
    let _ = STACK.try_with(|c| if c.get().1 == 0 { c.set(program_stack_bounds().unwrap_or(b)) });
}

/// Which stack address `sp` of the calling thread is on, as this module knows them, and the bytes
/// below it there: a program stack, the thread's own, or the segment it runs on now; None for a
/// stack this module did not make or read (an alternate signal stack). For fault reports:
/// async-signal-safe (thread-locals without destructors).
pub fn stack_of(sp: usize) -> Option<(&'static str, usize)> {
    let inside = |b: (usize, usize)| b.1 != 0 && b.0 <= sp && sp < b.1;
    let thread = THREAD_STACK.try_with(|c| c.get()).unwrap_or((0, 0));
    let now = STACK.try_with(|c| c.get()).unwrap_or((0, 0));
    match program_stack_bounds() {
        Some(b) if inside(b) => Some(("a program stack", sp - b.0)),
        _ if inside(thread) => Some(("the thread's stack", sp - thread.0)),
        _ if inside(now) => Some(("a stack segment", sp - now.0)),
        _ => None,
    }
}

/// A stack segment: usable [lo, top), mapped [base, base + len); `floor`: the lowest address the
/// stack may grow to (above the guard; = lo unless the OS commits the segment as it grows).
#[derive(Clone, Copy)]
struct Segment {
    base: usize,
    len: usize,
    lo: usize,
    floor: usize,
    top: usize,
}

/// The calling thread's own stack, usable [lo, hi) ((0, 0): unknown)
#[cfg(any(target_os = "linux", target_os = "android"))]
fn stack_bounds() -> (usize, usize) {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
        fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attr: *mut u64) -> i32;
    }
    unsafe {
        let mut attr = [0u64; 16];
        if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
            return (0, 0);
        }
        let (mut addr, mut size) = (0usize, 0usize);
        let ok = pthread_attr_getstack(attr.as_ptr(), &mut addr, &mut size) == 0;
        pthread_attr_destroy(attr.as_mut_ptr());
        if ok { (addr, addr + size) } else { (0, 0) }
    }
}

#[cfg(target_vendor = "apple")]
fn stack_bounds() -> (usize, usize) {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
        fn pthread_get_stacksize_np(t: usize) -> usize;
    }
    unsafe {
        let t = pthread_self();
        let hi = pthread_get_stackaddr_np(t) as usize;
        (hi - pthread_get_stacksize_np(t), hi)
    }
}

#[cfg(windows)]
fn stack_bounds() -> (usize, usize) {
    let (mut lo, mut hi) = (0usize, 0usize);
    unsafe { relax_session::win32::GetCurrentThreadStackLimits(&mut lo, &mut hi) };
    // the reservation's base; its lowest pages are the guard region
    if lo == 0 {
        (0, 0)
    } else {
        (lo + 3 * 4096 + 65536, hi)
    }
}

#[cfg(not(any(unix, windows)))]
fn stack_bounds() -> (usize, usize) {
    (0, 0)
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
        crate::memstat::name_mapping(seg as usize, len, c"relax-stack");
        Some(Segment { base: seg as usize, len, lo: seg as usize + GUARD, floor: seg as usize + GUARD, top: (seg as usize + len) & !15 })
    }

    pub(super) fn free_segment(s: Segment) {
        unsafe { munmap(s.base as *mut c_void, s.len) };
    }

    /// Whether the stack reached below `at` (the page there is resident; every page a stack grows
    /// into is touched, big frames probe each one). A failed query counts as reached.
    pub(super) fn used_below(s: &Segment, at: usize) -> bool {
        extern "C" {
            fn mincore(addr: *mut c_void, len: usize, vec: *mut u8) -> i32;
        }
        // the page at or just below `at - 1`, 64 KB aligned (any page size up to that)
        let page = (at - 1) & !0xffff;
        if page < s.lo {
            return true;
        }
        let mut v = 0u8;
        unsafe { mincore(page as *mut c_void, 1, &mut v) != 0 || v & 1 != 0 }
    }

    /// Guard bytes below a program stack (big frames probe every page, so any access below the
    /// stack's end lands here first).
    pub(super) const PROGRAM_GUARD: usize = 1 << 20;
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
        crate::memstat::name_mapping(seg as usize, len, c"relax-program-stack");
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

    /// A signal handler's call on the alternate signal stack, moved to a segment (`grow`).
    ///
    /// Linux places a signal's frame by the stack pointer alone: on the segment the thread is off
    /// the alternate stack as the kernel sees it, so a signal that came then (a run end's kick, a
    /// program's signal) had its frame written from that stack's top, over the live frames of the
    /// handler that made the call (its interrupted context, its locals). The handler then
    /// resumed to pc 0, or wrote through a zeroed pointer with its own signal blocked: the
    /// kernel's kill, no report. For the call the alternate stack is the room below those frames,
    /// where the kernel would put a nested frame natively (no room left: none, the segment takes
    /// it). The change is made off the alternate stack (the kernel refuses it on the stack), so
    /// signals stay blocked across each switch. (macOS marks the thread as on its alternate stack
    /// until the handler returns, wherever its stack pointer is: nothing to do.)
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) struct AltRoom {
        /// the alternate stack: its base and size
        sp: usize,
        size: usize,
        /// the top of the room below the handler's frames
        floor: usize,
        /// the thread's signal mask before the switch
        mask: [u64; 16],
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[repr(C)]
    struct StackT {
        sp: usize,
        flags: i32,
        size: usize,
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    extern "C" {
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
        fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    impl AltRoom {
        const SS_DISABLE: i32 = 2;
        const SIG_BLOCK: i32 = 0;
        const SIG_SETMASK: i32 = 2;
        /// Below `live` (the lowest frame of `grow` the call returns to): the switch's own frame
        /// and the red zone.
        const MARGIN: usize = 8192;

        /// On the alternate stack at `live` (a local of `grow`): its room, and every signal
        /// blocked until the call runs on the segment (`on_segment`). None elsewhere.
        pub(super) fn below(live: usize) -> Option<AltRoom> {
            let mut cur = StackT { sp: 0, flags: 0, size: 0 };
            if unsafe { sigaltstack(std::ptr::null(), &mut cur) } != 0 || cur.flags & Self::SS_DISABLE != 0 {
                return None;
            }
            if !(cur.sp <= live && live < cur.sp + cur.size) {
                return None;
            }
            let floor = live.saturating_sub(Self::MARGIN).max(cur.sp) & !15;
            let mut room = AltRoom { sp: cur.sp, size: cur.size, floor, mask: [0; 16] };
            unsafe { pthread_sigmask(Self::SIG_BLOCK, &[!0u64; 16], &mut room.mask) };
            Some(room)
        }

        /// On the segment: the alternate stack is the room below the handler's frames (none when
        /// the kernel takes no room that small: the segment's stack takes nested frames), and the
        /// signals go through again.
        pub(super) fn on_segment(&self) {
            let room = StackT { sp: self.sp, flags: 0, size: self.floor.saturating_sub(self.sp) };
            unsafe {
                if room.size == 0 || sigaltstack(&room, std::ptr::null_mut()) != 0 {
                    let off = StackT { sp: 0, flags: Self::SS_DISABLE, size: 0 };
                    sigaltstack(&off, std::ptr::null_mut());
                }
                pthread_sigmask(Self::SIG_SETMASK, &self.mask, std::ptr::null_mut());
            }
        }

        /// Still on the segment, the call done: every signal blocked, the whole alternate stack
        /// back.
        pub(super) fn off_segment(&self) {
            let whole = StackT { sp: self.sp, flags: 0, size: self.size };
            unsafe {
                pthread_sigmask(Self::SIG_BLOCK, &[!0u64; 16], std::ptr::null_mut());
                sigaltstack(&whole, std::ptr::null_mut());
            }
        }

        /// Back on the alternate stack: the signals go through again.
        pub(super) fn back(&self) {
            unsafe { pthread_sigmask(Self::SIG_SETMASK, &self.mask, std::ptr::null_mut()) };
        }
    }

    /// (elsewhere the kernel keeps a handler's thread on its alternate stack: `AltRoom` on Linux)
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub(super) struct AltRoom;

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    impl AltRoom {
        pub(super) fn below(_live: usize) -> Option<AltRoom> {
            None
        }
        pub(super) fn on_segment(&self) {}
        pub(super) fn off_segment(&self) {}
        pub(super) fn back(&self) {}
    }

    /// Calls `f(data)` with the stack pointer at the segment's top, back on this stack after.
    /// The call's return address goes to `SWITCH_RETURN` first (stack walkers recognise this
    /// frame by it); x86_64 keeps this function's own sp at the call's sp on the segment (the
    /// switch record, `SWITCH_RECORD`), where a walk finds this frame's caller.
    #[inline(never)]
    pub(super) unsafe fn call_on_segment(s: &Segment, f: extern "C" fn(*mut u8), data: *mut u8) {
        let sp = s.top;
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!(
            "adr x20, 3f",
            "str x20, [{ret}]",
            "mov x20, sp",
            "mov sp, {sp}",
            "blr {f}",
            "3:",
            "mov sp, x20",
            sp = in(reg) sp,
            f = in(reg) f,
            ret = in(reg) super::SWITCH_RETURN.as_ptr(),
            in("x0") data,
            out("x20") _,
            clobber_abi("C"),
        );
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!(
            "lea r12, [rip + 3f]",
            "mov qword ptr [{ret}], r12",
            "mov r12, rsp",
            "mov rsp, {sp}",
            "sub rsp, 16",
            "mov qword ptr [rsp], r12",
            "call {f}",
            "3:",
            "mov rsp, r12",
            sp = in(reg) sp,
            f = in(reg) f,
            ret = in(reg) super::SWITCH_RETURN.as_ptr(),
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

    /// (no alternate signal stack here: `AltRoom` on Linux)
    pub(super) struct AltRoom;

    impl AltRoom {
        pub(super) fn below(_live: usize) -> Option<AltRoom> {
            None
        }
        pub(super) fn on_segment(&self) {}
        pub(super) fn off_segment(&self) {}
        pub(super) fn back(&self) {}
    }

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

    /// Whether the stack reached below `at` (the page there was committed: the guard below the
    /// initially committed top moves down as the stack grows). A failed query counts as reached.
    pub(super) fn used_below(s: &Segment, at: usize) -> bool {
        let page = (at - 1) & !0xfff;
        if page < s.floor {
            return true;
        }
        let mut m: MemoryBasicInformation = unsafe { std::mem::zeroed() };
        let n = unsafe { VirtualQuery(page as *const u8, &mut m, std::mem::size_of::<MemoryBasicInformation>()) };
        n == 0 || (m.state == MEM_COMMIT && m.protect & PAGE_GUARD == 0)
    }

    const PAGE_GUARD: u32 = 0x100;
    /// committed at first: the top of a program stack, then its guard page
    const INITIAL: usize = 1 << 20;
    /// kept reserved (never committed) above the reservation's base: the stack overflows here
    pub(super) const PROGRAM_GUARD: usize = 1 << 20;

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

    // The stack's last frame, what `RtlUserThreadStart` is to a thread's stack and `BaseFiberStart`
    // to a fiber's: `relax_stack_last_frame(data, f)` calls `f(data)`, and what no frame below it
    // handles goes to `relax_stack_last_frame_handler` (by its symbol: rustc does not count a
    // `sym` operand of `global_asm!` as a use). The system's search never reaches the switch's frame: it
    // cannot unwind that one, it reads above the segment, that fault is dispatched and searched
    // the same way, deeper each time, and the thread never comes back. (Its frame record is the
    // usual one: walkers go on through its return address, `SWITCH_RETURN`.)
    std::arch::global_asm!(
        ".globl relax_stack_last_frame",
        "relax_stack_last_frame:",
        ".seh_proc relax_stack_last_frame",
        ".seh_handler relax_stack_last_frame_handler, @except",
        "push rbp",
        ".seh_pushreg rbp",
        "sub rsp, 32",
        ".seh_stackalloc 32",
        "lea rbp, [rsp + 32]",
        ".seh_setframe rbp, 32",
        ".seh_endprologue",
        "call rdx",
        "nop",
        "add rsp, 32",
        "pop rbp",
        "ret",
        ".seh_endproc",
    );
    extern "C" {
        fn relax_stack_last_frame(data: *mut u8, f: extern "C" fn(*mut u8));
    }

    /// As the system's frame at the end of a thread: the process's unhandled exception filter
    /// (`SetUnhandledExceptionFilter`), and the process ends with the code unless it resumes.
    #[no_mangle]
    unsafe extern "system" fn relax_stack_last_frame_handler(record: *mut ExceptionRecord, _frame: *mut u8, context: *mut Context, _dispatch: *mut u8) -> u32 {
        const CONTINUE_EXECUTION: u32 = 0;
        let mut pointers = ExceptionPointers { record, context };
        if UnhandledExceptionFilter(&mut pointers) == EXCEPTION_CONTINUE_EXECUTION {
            return CONTINUE_EXECUTION;
        }
        TerminateProcess(GetCurrentProcess(), (*record).code);
        unreachable!()
    }

    /// Calls `f(data)` (Win64: data in rcx, 32 bytes of shadow space) on the segment. The TEB's
    /// stack bounds (NT_TIB StackBase gs:[8], StackLimit gs:[16], DeallocationStack gs:[0x1478])
    /// describe the segment for the call: the exception dispatcher and RtlUnwindEx check frames
    /// against them (a Rust panic caught inside `f` unwinds there), as for a fiber's stack.
    /// The call's return address goes to `SWITCH_RETURN` first; this function's own sp (before
    /// the pushes) is the switch record, above the callee's shadow space (`SWITCH_RECORD`).
    #[inline(never)]
    pub(super) unsafe fn call_on_segment(s: &Segment, f: extern "C" fn(*mut u8), data: *mut u8) {
        std::arch::asm!(
            "lea r12, [rip + 3f]",
            "mov qword ptr [{ret}], r12",
            "push qword ptr gs:[8]",
            "push qword ptr gs:[16]",
            "push qword ptr gs:[0x1478]",
            "mov r12, rsp",
            "mov gs:[8], {top}",
            "mov gs:[16], {lo}",
            "mov gs:[0x1478], {base}",
            "mov rsp, {top}",
            "sub rsp, 48",
            "mov qword ptr [rsp + 32], r12",
            "add qword ptr [rsp + 32], 24",
            "call {last_frame}",
            "3:",
            "mov rsp, r12",
            "pop qword ptr gs:[0x1478]",
            "pop qword ptr gs:[16]",
            "pop qword ptr gs:[8]",
            top = in(reg) s.top,
            lo = in(reg) s.lo,
            base = in(reg) s.base,
            last_frame = in(reg) relax_stack_last_frame as unsafe extern "C" fn(*mut u8, extern "C" fn(*mut u8)),
            ret = in(reg) super::SWITCH_RETURN.as_ptr(),
            in("rcx") data,
            in("rdx") f,
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
    pub(super) fn used_below(_s: &Segment, _at: usize) -> bool {
        true
    }
    pub(super) fn reserve_stack(_len: usize) -> Option<Segment> {
        None
    }
    pub(super) fn physical_memory() -> usize {
        1 << 30
    }
    pub(super) unsafe fn call_on_segment(_s: &Segment, _f: extern "C" fn(*mut u8), _data: *mut u8) {}

    /// (no alternate signal stack here: `AltRoom` on Linux)
    pub(super) struct AltRoom;

    impl AltRoom {
        pub(super) fn below(_live: usize) -> Option<AltRoom> {
            None
        }
        pub(super) fn on_segment(&self) {}
        pub(super) fn off_segment(&self) {}
        pub(super) fn back(&self) {}
    }
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

    /// A deep call started on a small stack this module did not make (a 64 KB alternate signal
    /// stack, mapped once above and once below the thread's own) runs on a segment. The check
    /// knew the low bound alone: above the thread's stack every small stack passed its red zone,
    /// and a lazy compile in a signal handler overflowed the alternate stack (the kernel's kill).
    #[cfg(unix)]
    #[test]
    fn a_deep_call_from_a_foreign_small_stack_runs_on_a_segment() {
        use std::ffi::c_void;
        extern "C" {
            fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
            fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
            fn munmap(addr: *mut c_void, len: usize) -> i32;
        }
        const SMALL: usize = 64 << 10;
        const GUARD: usize = 16 << 10;
        let anon = if cfg!(target_vendor = "apple") { 0x1002 } else { 0x22 };
        /// Recurses through `n` frames of 4 KB; the lowest frame's address.
        #[inline(never)]
        fn down(n: u32) -> usize {
            let pad = [n as u8; 4096];
            let at = std::hint::black_box(&pad) as *const _ as usize;
            if n == 0 { at } else { down(n - 1).min(at) }
        }
        /// On the small stack [small.0, small.1): where the closure ran and, run off it, the
        /// lowest frame of 256 KB below it (0: it ran on the small stack, which has no such room).
        struct Job {
            small: (usize, usize),
            ran: usize,
            lowest: usize,
        }
        extern "C" fn on_small(p: *mut u8) {
            let job = unsafe { &mut *(p as *mut Job) };
            let small = job.small;
            let r = super::ensure_sufficient_stack(|| {
                let here = std::hint::black_box(0u8);
                let here = &here as *const u8 as usize;
                (here, if here >= small.0 && here < small.1 { 0 } else { down(64) })
            });
            (job.ran, job.lowest) = r;
        }
        let (lo, hi) = super::stack_bounds();
        assert!(hi > lo, "the thread's stack is known");
        // Linux: the highest free gap under the stack, from the process's map, taken exactly (a
        // run under the JIT reserves wide ranges there, and the system places a hint whose range
        // is taken elsewhere: above). None: no gap, or the map changed meanwhile.
        #[cfg(target_os = "linux")]
        let below_gap = |len: usize| -> Option<usize> {
            const FIXED_NOREPLACE: i32 = 0x10_0000;
            let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
            let mut spans: Vec<(usize, usize)> = maps
                .lines()
                .filter_map(|l| {
                    let (a, b) = l.split_whitespace().next()?.split_once('-')?;
                    Some((usize::from_str_radix(a, 16).ok()?, usize::from_str_radix(b, 16).ok()?))
                })
                .collect();
            spans.sort_unstable();
            let (mut end, mut best) = (1usize << 20, None);
            for &(a, b) in spans.iter().take_while(|s| s.0 < lo) {
                let at = a.saturating_sub(len) & !0xffff;
                if at >= end {
                    best = Some(at);
                }
                end = end.max(b);
            }
            let at = best?;
            let got = unsafe { mmap(at as *mut c_void, len, 3, anon | FIXED_NOREPLACE, -1, 0) } as usize;
            if got != at && got as isize != -1 {
                // (a kernel without the flag took it as a hint)
                unsafe { munmap(got as *mut c_void, len) };
            }
            (got == at).then_some(at)
        };
        // (a hint: the system maps there when it is free; further out until one lands on that side)
        let map = |above: bool| -> usize {
            #[cfg(target_os = "linux")]
            if !above {
                if let Some(at) = below_gap(SMALL + GUARD) {
                    return at;
                }
            }
            for shift in 28..40 {
                let hint = if above { hi.checked_add(1 << shift) } else { lo.checked_sub(1 << shift) };
                let Some(hint) = hint.map(|h| h & !0xffff) else { continue };
                let at = unsafe { mmap(hint as *mut c_void, SMALL + GUARD, 3, anon, -1, 0) } as usize;
                if at as isize == -1 {
                    continue;
                }
                if (above && at >= hi) || (!above && at + SMALL + GUARD <= lo) {
                    return at;
                }
                unsafe { munmap(at as *mut c_void, SMALL + GUARD) };
            }
            panic!("no mapping {} the thread's stack", if above { "above" } else { "below" });
        };
        for above in [true, false] {
            let base = map(above);
            assert_eq!(unsafe { mprotect(base as *mut c_void, GUARD, 0) }, 0);
            let small = super::Segment { base, len: SMALL + GUARD, lo: base + GUARD, floor: base + GUARD, top: (base + SMALL + GUARD) & !15 };
            let mut job = Job { small: (small.base, small.base + small.len), ran: 0, lowest: 0 };
            unsafe { super::imp::call_on_segment(&small, on_small, &mut job as *mut Job as *mut u8) };
            let side = if above { "above" } else { "below" };
            let on_small = |a: usize| a >= small.base && a < small.base + small.len;
            assert!(!on_small(job.ran), "the call from the small stack {side} the thread's ran on it");
            assert!(job.lowest != 0 && !on_small(job.lowest), "its 256 KB of frames {side} the thread's stack are off the small stack");
            unsafe { munmap(base as *mut c_void, SMALL + GUARD) };
        }
    }

    /// Leaves MAGIC at the low end of an N-word frame; returns its address.
    #[inline(never)]
    fn mark<const N: usize>() -> usize {
        let mut a = [0usize; N];
        a[0] = MAGIC;
        std::hint::black_box(&mut a);
        &a[0] as *const usize as usize
    }
    const MAGIC: usize = 0x5eed_c0de;
    const WORD: usize = std::mem::size_of::<usize>();

    /// Where `peek` reads
    static AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Whether reading `at` on a program stack is a read at all. On Windows a page the stack has
    /// not committed faults; elsewhere one a new mapping left reads as zero.
    #[cfg(windows)]
    fn readable(at: usize) -> bool {
        use relax_session::win32::{MemoryBasicInformation, VirtualQuery};
        const MEM_COMMIT: u32 = 0x1000;
        const PAGE_GUARD: u32 = 0x100;
        let mut m: MemoryBasicInformation = unsafe { std::mem::zeroed() };
        let n = unsafe { VirtualQuery(at as *const u8, &mut m, std::mem::size_of::<MemoryBasicInformation>()) };
        n != 0 && m.state == MEM_COMMIT && m.protect & PAGE_GUARD == 0
    }
    #[cfg(not(windows))]
    fn readable(_at: usize) -> bool {
        true
    }

    /// The word at AT, read on the program stack it is on (usize::MAX: AT is not on this one, or
    /// that page is not committed)
    fn peek() -> usize {
        let at = AT.load(std::sync::atomic::Ordering::Relaxed);
        let (lo, hi) = super::program_stack_bounds().unwrap();
        if at > lo && at < hi && readable(at) { unsafe { std::ptr::read_volatile(at as *const usize) } } else { usize::MAX }
    }

    /// Runs on a program stack from `spares`: its bounds and what `f` returns.
    fn on_program_stack(spares: &super::Spares<super::Segment>, f: fn() -> usize) -> ((usize, usize), usize) {
        let mut r = ((0, 0), 0);
        super::on_program_stack_from(spares, || {}, || r = (super::program_stack_bounds().unwrap(), f()));
        r
    }

    /// A finished program stack is kept for the next one: the next thread runs on the same
    /// segment, its pages as the last one left them (no new mapping per thread). One a thread
    /// used deep is unmapped (its pages are not kept committed): the next one is another
    /// segment, or a new mapping at the same place. (Spares of its own: the process's are
    /// taken and given back by every test that runs on a program stack.)
    #[test]
    fn program_stacks_are_kept_unless_used_deep() {
        let spares = super::Spares::new();
        let (first, at) = on_program_stack(&spares, mark::<{ (256 << 10) / WORD }>);
        assert!(at > first.0 && at < first.1);
        AT.store(at, std::sync::atomic::Ordering::Relaxed);
        let (second, seen) = on_program_stack(&spares, peek);
        assert_eq!(first, second, "a new program stack");
        assert_eq!(seen, MAGIC, "a new mapping at the same place");

        let (first, at) = on_program_stack(&spares, mark::<{ (8 << 20) / WORD }>);
        assert!(first.1 - at > super::PROGRAM_STACK_KEEP);
        AT.store(at, std::sync::atomic::Ordering::Relaxed);
        let (_, seen) = on_program_stack(&spares, peek);
        assert_ne!(seen, MAGIC, "kept with its deep pages");
        while let Some(s) = spares.take() {
            super::imp::free_segment(s);
        }
    }

    /// A panic on a program stack comes back to the caller, on the thread's own stack.
    #[test]
    fn a_panic_on_a_program_stack_comes_back() {
        let r = std::panic::catch_unwind(|| super::on_program_stack(|| {}, || panic!("on the program stack")));
        assert_eq!(r.unwrap_err().downcast_ref::<&str>(), Some(&"on the program stack"));
        assert_eq!(super::program_stack_bounds(), None);
    }

    /// A fault nothing handles on a program stack goes to the process's unhandled exception
    /// filter, as on a thread's own stack, and resumes if that says so. Before the stack had a
    /// last frame the system's search never came back from the stack switch.
    #[cfg(windows)]
    #[test]
    fn a_fault_on_a_program_stack_reaches_the_process_filter() {
        use relax_session::win32::*;
        const CHILD: &str = "RELAX_BASE_STACK_FAULT";
        const RESUMED: u32 = 0x5e5;
        /// resumes past the faulting read (`mov rax, [rax]`: 3 bytes, the asm below) once, ends the
        /// process with RESUMED the second time
        extern "system" fn filter(ep: *mut ExceptionPointers) -> i32 {
            static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            unsafe {
                if SEEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    TerminateProcess(GetCurrentProcess(), RESUMED);
                }
                let c = &mut *(*ep).context;
                c.set(CTX_RIP, c.get(CTX_RIP) + 3);
            }
            EXCEPTION_CONTINUE_EXECUTION
        }
        fn fault() {
            unsafe { std::arch::asm!("mov rax, [rax]", inout("rax") 8u64 => _) };
        }
        if std::env::var_os(CHILD).is_some() {
            unsafe { SetUnhandledExceptionFilter(Some(filter)) };
            super::on_program_stack(|| {}, || {
                fault();
                fault();
            });
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "stack::tests::a_fault_on_a_program_stack_reaches_the_process_filter", "--test-threads=1"])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let since = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if since.elapsed().as_secs() >= 30 {
                child.kill().unwrap();
                panic!("the fault's search did not come back");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(status.code(), Some(RESUMED as i32));
    }
}
