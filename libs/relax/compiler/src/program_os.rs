//! What a program asks the OS about the thread and heap it runs on, answered for the program
//! (loader_symbol hands these to JIT code in place of libc's).
//!
//! Stack: a program thread runs on a program stack (relax_base::stack::on_program_stack), not on
//! the OS thread's. std's stack-overflow guard, stacker-style segment growth and frame walkers ask
//! for the calling thread's stack bounds, and must get the stack they actually run on: the
//! program stack's usable range and its guard (Windows needs nothing here: the stack switch sets
//! the TEB's bounds, which GetCurrentThreadStackLimits reads). Another thread, or a thread that is
//! not on a program stack, gets the OS answer.
//!
//! Thread names (pthread_setname_np, Linux): a thread that names itself as MAKEPAD_RUN_RT_THREADS
//! lists is a live real-time thread for the JIT's queue (jit::realtime_live).
//!
//! Heap (glibc mallinfo2): a program thread that runs in a domain allocates in that domain's heap
//! (zone_alloc::domain), which is what the program's memory is and what `makepad inspect` and the
//! domain teardown account (DomainInfo::arena_in_use / arena_committed); mallinfo2 reports that heap.
//! A thread on the system heap (a program without domains) gets glibc's figures less what the
//! loader's own allocator holds there (zone_alloc::system_held): the program's allocations, as a
//! native build of it would see them, without the JIT's tables and runtime state.

#[cfg(any(target_os = "linux", target_os = "android"))]
extern "C" {
    fn pthread_self() -> usize;
    #[link_name = "pthread_getattr_np"]
    fn libc_pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
    fn pthread_attr_setstack(attr: *mut u64, addr: *mut u8, size: usize) -> i32;
    fn pthread_attr_setguardsize(attr: *mut u64, size: usize) -> i32;
    #[link_name = "pthread_setname_np"]
    fn libc_pthread_setname_np(t: usize, name: *const std::ffi::c_char) -> i32;
}

/// pthread_setname_np: a thread that takes a name MAKEPAD_RUN_RT_THREADS lists is a live real-time
/// thread (jit::realtime_live: the tier-up builds go below it from now on).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub extern "C" fn pthread_setname_np(t: usize, name: *const std::ffi::c_char) -> i32 {
    let r = unsafe { libc_pthread_setname_np(t, name) };
    if r == 0 && !name.is_null() && !crate::jit::rt_live() {
        let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_str().unwrap_or("");
        if crate::jit::rt_named(name) {
            crate::jit::realtime_live(Some(name));
        }
    }
    r
}

/// pthread_getattr_np: for the calling thread on a program stack, the program stack's range
/// (usable lo..hi) and its guard size.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub extern "C" fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32 {
    let r = unsafe { libc_pthread_getattr_np(t, attr) };
    if r != 0 || t != unsafe { pthread_self() } {
        return r;
    }
    if let Some((lo, hi)) = crate::stack::program_stack_bounds() {
        unsafe {
            pthread_attr_setstack(attr, lo as *mut u8, hi - lo);
            pthread_attr_setguardsize(attr, crate::stack::PROGRAM_STACK_GUARD);
        }
    }
    0
}

#[cfg(target_vendor = "apple")]
extern "C" {
    fn pthread_self() -> usize;
    #[link_name = "pthread_get_stackaddr_np"]
    fn libc_pthread_get_stackaddr_np(t: usize) -> *mut u8;
    #[link_name = "pthread_get_stacksize_np"]
    fn libc_pthread_get_stacksize_np(t: usize) -> usize;
}

/// The calling thread's program stack, when `t` is the calling thread and it runs on one.
#[cfg(target_vendor = "apple")]
fn own_program_stack(t: usize) -> Option<(usize, usize)> {
    if t != unsafe { pthread_self() } {
        return None;
    }
    crate::stack::program_stack_bounds()
}

/// pthread_get_stackaddr_np: the stack's top (it grows down from there).
#[cfg(target_vendor = "apple")]
pub extern "C" fn pthread_get_stackaddr_np(t: usize) -> *mut u8 {
    match own_program_stack(t) {
        Some((_, hi)) => hi as *mut u8,
        None => unsafe { libc_pthread_get_stackaddr_np(t) },
    }
}

/// pthread_get_stacksize_np: the usable size below the top (the guard below it excluded, as
/// for a thread pthread created).
#[cfg(target_vendor = "apple")]
pub extern "C" fn pthread_get_stacksize_np(t: usize) -> usize {
    match own_program_stack(t) {
        Some((lo, hi)) => hi - lo,
        None => unsafe { libc_pthread_get_stacksize_np(t) },
    }
}

/// glibc's `struct mallinfo2`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
#[derive(Default)]
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

#[cfg(all(target_os = "linux", target_env = "gnu"))]
extern "C" {
    #[link_name = "mallinfo2"]
    fn libc_mallinfo2() -> Mallinfo2;
}

/// mallinfo2: the calling thread's domain heap (in use, committed; the rest of the committed
/// bytes as free), else glibc's.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub extern "C" fn mallinfo2() -> Mallinfo2 {
    match crate::zone_alloc::domain::current_stats() {
        Some(stats) => domain_mallinfo2(stats),
        None => program_mallinfo2(unsafe { libc_mallinfo2() }, crate::zone_alloc::system_held::bytes()),
    }
}

/// The system heap's figures without the `loader` bytes the loader's own allocator holds in it
/// (in-use first, then mmapped).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn program_mallinfo2(mut m: Mallinfo2, loader: usize) -> Mallinfo2 {
    let from_used = loader.min(m.uordblks);
    m.uordblks -= from_used;
    m.hblkhd -= (loader - from_used).min(m.hblkhd);
    m
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn domain_mallinfo2((in_use, committed): (usize, usize)) -> Mallinfo2 {
    Mallinfo2 { arena: committed, uordblks: in_use, fordblks: committed.saturating_sub(in_use), ..Default::default() }
}

#[cfg(test)]
mod tests {
    /// On a program stack the calling thread's bounds are the program stack's, as every
    /// platform's query reports them; another thread's are its own.
    #[test]
    fn stack_queries_report_the_program_stack() {
        let mut seen = None;
        crate::stack::on_program_stack(|| {}, || {
            let (lo, hi) = crate::stack::program_stack_bounds().expect("on a program stack");
            let here = &lo as *const usize as usize;
            assert!(lo < here && here < hi);
            seen = Some((lo, hi, query()));
        });
        let (lo, hi, got) = seen.expect("ran");
        assert_eq!(got, Some((lo, hi)));
        // off the program stack: the OS answer, which does not hold the program stack
        if let Some((a, b)) = query() {
            assert!(b <= lo || a >= hi);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn query() -> Option<(usize, usize)> {
        extern "C" {
            fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
            fn pthread_attr_getguardsize(attr: *const u64, size: *mut usize) -> i32;
            fn pthread_attr_destroy(attr: *mut u64) -> i32;
        }
        let mut attr = [0u64; 16];
        let a = attr.as_mut_ptr();
        if super::pthread_getattr_np(unsafe { super::pthread_self() }, a) != 0 {
            return None;
        }
        let (mut addr, mut size, mut guard) = (0usize, 0usize, 0usize);
        unsafe {
            pthread_attr_getstack(a, &mut addr, &mut size);
            pthread_attr_getguardsize(a, &mut guard);
            pthread_attr_destroy(a);
        }
        if crate::stack::program_stack_bounds().is_some() {
            assert_eq!(guard, crate::stack::PROGRAM_STACK_GUARD);
        }
        Some((addr, addr + size))
    }

    #[cfg(target_vendor = "apple")]
    fn query() -> Option<(usize, usize)> {
        let t = unsafe { super::pthread_self() };
        let hi = super::pthread_get_stackaddr_np(t) as usize;
        Some((hi - super::pthread_get_stacksize_np(t), hi))
    }

    #[cfg(windows)]
    fn query() -> Option<(usize, usize)> {
        let (mut lo, mut hi) = (0usize, 0usize);
        unsafe { relax_session::win32::GetCurrentThreadStackLimits(&mut lo, &mut hi) };
        // the reservation's base (the guard region at its bottom included)
        Some((lo + crate::stack::PROGRAM_STACK_GUARD, hi))
    }

    /// Off a domain, mallinfo2 leaves out what the loader's allocator holds in the system heap:
    /// a loader block allocated meanwhile does not show, as the program did not allocate it.
    /// (In a process of its own: the system-held count is the process's, and the suite's other
    /// threads freeing meanwhile took it below the 1 MiB block, one whole-suite run in eight.)
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn mallinfo2_leaves_out_the_loaders_blocks() {
        isolated_test!(mallinfo2_leaves_out_the_loaders_blocks);
        let m = super::Mallinfo2 { uordblks: 1000, hblkhd: 500, fordblks: 7, ..Default::default() };
        let p = super::program_mallinfo2(m, 1200);
        assert_eq!((p.uordblks, p.hblkhd, p.fordblks), (0, 300, 7));
        let before = crate::zone_alloc::system_held::bytes();
        let held: Vec<u8> = std::hint::black_box(vec![1u8; 1 << 20]);
        assert!(crate::zone_alloc::system_held::bytes() >= before + (1 << 20));
        drop(held);
        assert!(crate::zone_alloc::system_held::bytes() < before + (1 << 20));
    }

    /// A domain thread's mallinfo2 is its heap's accounting, the figures DomainInfo shows.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn mallinfo2_of_a_domain_is_its_heap() {
        let heap = crate::zone_alloc::domain::DomainHeap::new().unwrap();
        let outer = crate::zone_alloc::domain::current();
        unsafe { crate::zone_alloc::domain::enter(&*heap) };
        let m = super::mallinfo2();
        let (in_use, committed) = heap.stats();
        unsafe { crate::zone_alloc::domain::restore(outer) };
        assert_eq!((m.uordblks, m.arena, m.fordblks, m.hblkhd), (in_use, committed, committed - in_use, 0));
    }
}
