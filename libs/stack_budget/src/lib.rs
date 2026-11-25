//! Stack bounds and descent budgets for downward-growing thread stacks.
//! Pass the address of a live stack local as the marker.

/// The current thread's OS-reported stack bounds as (low, exclusive high).
/// Returns (0, 0) when the OS cannot supply a range.
pub fn thread_stack_range() -> (usize, usize) {
    os_stack_range()
}

/// Bytes below the marker on the current thread. If a manual stack or web
/// worker has no matching OS descriptor, assume Makepad's smallest thread
/// stack (64 KiB, `MIN_THREAD_STACK_SIZE` in platform/src/thread.rs).
pub fn remaining_stack_bytes(marker: usize) -> usize {
    thread_local! {
        static BOUNDS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    }
    BOUNDS.with(|bounds| {
        let mut range = bounds.get();
        if !(range.0..range.1).contains(&marker) {
            // A host can switch to another stack on the same thread.
            range = thread_stack_range();
            bounds.set(range);
        }
        if (range.0..range.1).contains(&marker) { marker - range.0 } else { 64 * 1024 }
    })
}

/// Bytes available for descent while reserving half the remaining stack
/// and at least 32 KiB for recovery and other calls.
pub fn native_stack_budget(marker: usize) -> usize {
    let remaining = remaining_stack_bytes(marker);
    (remaining / 2).min(remaining.saturating_sub(32 * 1024))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn os_stack_range() -> (usize, usize) {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut u64) -> i32;
        fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attr: *mut u64) -> i32;
    }
    // Opaque, aligned storage for pthread_attr_t on Linux and Android.
    let mut attr = [0u64; 16];
    let (mut low, mut len) = (0, 0);
    unsafe {
        if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
            return (0, 0);
        }
        let ok = pthread_attr_getstack(attr.as_ptr(), &mut low, &mut len) == 0;
        pthread_attr_destroy(attr.as_mut_ptr());
        if ok { (low, low.saturating_add(len)) } else { (0, 0) }
    }
}

#[cfg(target_vendor = "apple")]
fn os_stack_range() -> (usize, usize) {
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(thread: usize) -> usize;
        fn pthread_get_stacksize_np(thread: usize) -> usize;
    }
    // Darwin gives the high address and the size of this thread's stack.
    unsafe {
        let thread = pthread_self();
        let high = pthread_get_stackaddr_np(thread);
        (high.saturating_sub(pthread_get_stacksize_np(thread)), high)
    }
}

#[cfg(windows)]
fn os_stack_range() -> (usize, usize) {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }
    let (mut low, mut high) = (0, 0);
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    (low, high)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
fn os_stack_range() -> (usize, usize) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_uses_the_stack_containing_the_marker() {
        let local = 0u8;
        let marker = std::hint::black_box(&local) as *const u8 as usize;
        let (low, high) = thread_stack_range();
        if (low..high).contains(&marker) {
            assert_eq!(remaining_stack_bytes(marker), marker - low);
        }
        assert!(native_stack_budget(marker) <= remaining_stack_bytes(marker) / 2);
        // A marker outside a cached native range takes the manual-stack fallback.
        assert!(native_stack_budget(0) > 0);
        assert!(native_stack_budget(0) <= remaining_stack_bytes(0) / 2);
    }
}
