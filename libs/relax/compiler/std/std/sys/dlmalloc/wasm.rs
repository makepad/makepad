// Port of dlmalloc 0.2.13, https://github.com/alexcrichton/dlmalloc-rs
// Upstream crate SHA256: 9f5b01c17f85ee988d832c40e549a64bd89ab2c9f8d8a613bdf5122ae507e294
// Copyright Alex Crichton; MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE).
use super::Allocator;
#[cfg(target_arch = "wasm32")]
use core::arch::wasm32 as wasm;
#[cfg(target_arch = "wasm64")]
use core::arch::wasm64 as wasm;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(all(target_os = "unknown", not(relax_web)))]
extern "C" {
    static __heap_base: u8;
    static __heap_end: u8;
}

static PREEXISTING_USED: AtomicBool = AtomicBool::new(false);

#[cfg(all(target_os = "unknown", not(relax_web)))]
fn preexisting_chunk_from_linker(size: usize) -> Option<(usize, usize)> {
    let heap_base = unsafe { &__heap_base as *const u8 as usize };
    let heap_end = unsafe { &__heap_end as *const u8 as usize };

    if heap_base == 0 || heap_end <= heap_base {
        return None;
    }

    let len = heap_end - heap_base;
    if len < size {
        return None;
    }

    Some((heap_base, len))
}

#[cfg(any(not(target_os = "unknown"), relax_web))]
fn preexisting_chunk_from_linker(_size: usize) -> Option<(usize, usize)> {
    None
}

fn try_donate_preexisting(
    state: &AtomicBool,
    chunk: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    if state.swap(true, Ordering::Relaxed) {
        return None;
    }

    chunk
}

fn alloc_via_grow(size: usize, page_size: usize) -> (*mut u8, usize, u32) {
    let pages = size.div_ceil(page_size);
    let prev = wasm::memory_grow::<0>(pages);

    if prev == usize::max_value() {
        return (ptr::null_mut(), 0, 0);
    }

    let prev_page = prev * page_size;
    let base_ptr = unsafe { core::intrinsics_mem::web_memory_ptr(prev_page) };
    let size = pages * page_size;

    // Additionally check to see if we just allocated the final bit of the
    // address space. In such a situation it's not valid in Rust for a
    // pointer to actually wrap around to from the top of the address space
    // to 0, so it's not valid to allocate the entire region. Fake the last
    // few bytes as being un-allocated meaning that the actual size of this
    // allocation won't be page aligned, which should be handled by
    // dlmalloc.
    if prev_page.wrapping_add(size) == 0 {
        return (base_ptr, size - 16, 0);
    }

    (base_ptr, size, 0)
}

/// System setting for Wasm.
pub struct System {
    _priv: (),
}

impl System {
    pub const fn new() -> System {
        System { _priv: () }
    }
}

unsafe impl Allocator for System {
    fn alloc(&self, size: usize) -> (*mut u8, usize, u32) {
        let page_size = self.page_size();

        if size != 0 {
            let chunk = preexisting_chunk_from_linker(size);
            if let Some((base, len)) = try_donate_preexisting(&PREEXISTING_USED, chunk) {
                return (base as *mut u8, len, 0);
            }
        }

        alloc_via_grow(size, page_size)
    }

    fn remap(&self, _ptr: *mut u8, _oldsize: usize, _newsize: usize, _can_move: bool) -> *mut u8 {
        ptr::null_mut()
    }

    fn free_part(&self, _ptr: *mut u8, _oldsize: usize, _newsize: usize) -> bool {
        false
    }

    fn free(&self, _ptr: *mut u8, _size: usize) -> bool {
        false
    }

    fn can_release_part(&self, _flags: u32) -> bool {
        false
    }

    fn allocates_zeros(&self) -> bool {
        true
    }

    fn page_size(&self) -> usize {
        64 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::try_donate_preexisting;
    use core::sync::atomic::{AtomicBool, Ordering};

    fn legacy_grow_only(
        size: usize,
        page_size: usize,
        grow_result: usize,
    ) -> Option<(usize, usize)> {
        let pages = size.div_ceil(page_size);
        if grow_result == usize::MAX {
            return None;
        }
        Some((grow_result * page_size, pages * page_size))
    }

    #[test]
    fn uses_preexisting_memory_when_growth_fails() {
        let page_size = 64 * 1024;
        let chunk = (page_size, page_size * 3);
        let state = AtomicBool::new(false);
        let new_behavior = try_donate_preexisting(&state, Some(chunk));
        let legacy_behavior = legacy_grow_only(16, page_size, usize::MAX);

        assert_eq!(new_behavior, Some((page_size, page_size * 3)));
        assert_eq!(legacy_behavior, None);
        assert!(state.load(Ordering::Relaxed));
    }

    #[test]
    fn one_chunk_or_never_disables_after_failure() {
        let state = AtomicBool::new(false);

        let first = None;
        assert_eq!(first, None);
        assert_eq!(try_donate_preexisting(&state, first), None);
        assert!(state.load(Ordering::Relaxed));

        let second = Some((64 * 1024, 64 * 1024));
        assert_eq!(try_donate_preexisting(&state, second), None);
    }

    #[test]
    fn one_chunk_donates_only_once() {
        let state = AtomicBool::new(false);

        assert_eq!(
            try_donate_preexisting(&state, Some((64 * 1024, 128 * 1024))),
            Some((64 * 1024, 128 * 1024))
        );
        assert!(state.load(Ordering::Relaxed));
        assert_eq!(
            try_donate_preexisting(&state, Some((64 * 1024, 64 * 1024))),
            None
        );
    }
}

#[cfg(feature = "global")]
pub fn acquire_global_lock() {
    // single threaded, no need!
    assert!(!cfg!(target_feature = "atomics"));
}

#[cfg(feature = "global")]
pub fn release_global_lock() {
    // single threaded, no need!
    assert!(!cfg!(target_feature = "atomics"));
}

#[allow(missing_docs)]
#[cfg(feature = "global")]
pub unsafe fn enable_alloc_after_fork() {
    // single threaded, no need!
    assert!(!cfg!(target_feature = "atomics"));
}
