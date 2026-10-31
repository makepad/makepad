//! Heap identity in every heap ref. Each heap takes a tag `1 + serial % 255` from a process-wide
//! serial; its tables start every slot's generation at `tag << 8`, so a ref carries the tag of the
//! heap that minted it in its generation's high byte, inside the 40-bit payload a `ScriptValue`
//! already has. Resolving a ref in another heap fails the generation compare every access already
//! does, and the panic names both heaps. Tag 0 is skipped: it is the null slot's generation in
//! every heap, so `ScriptObject::ZERO` and friends stay heap-neutral. Two live heaps can share a
//! tag (every 255th heap); that only lowers the odds of catching a wrong access between them.
use std::sync::atomic::{AtomicUsize, Ordering};

static SERIAL: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// A heap's tag.
#[derive(Default)]
pub struct HeapTag(u8);

impl HeapTag {
    pub(crate) fn alloc() -> Self {
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let tag = (1 + serial % 255) as u8;
        let live = LIVE.fetch_add(1, Ordering::Relaxed) + 1;
        if makepad_error_log::trace_enabled("script.heaps") {
            eprintln!("script.heaps: + heap {tag}, {live} live");
            if makepad_error_log::trace_enabled("script.heaps.where") {
                eprintln!("script.heaps.where #{serial}:\n{}", std::backtrace::Backtrace::force_capture());
            }
        }
        HeapTag(tag)
    }

    pub fn get(&self) -> u8 {
        self.0
    }
}

impl Drop for HeapTag {
    fn drop(&mut self) {
        if self.0 != 0 {
            let live = LIVE.fetch_sub(1, Ordering::Relaxed) - 1;
            if makepad_error_log::trace_enabled("script.heaps") {
                eprintln!("script.heaps: - heap {}, {live} live", self.0);
            }
        }
    }
}

/// How many heaps are alive, for the heap census.
#[makepad_relax_api::relax_api]
pub fn live_heaps() -> usize {
    LIVE.load(Ordering::Relaxed)
}
