//! Shared-buffer tensors on Metal; portable host storage on other platforms.

use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::cell::Cell;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "macos")]
static NEXT_TENSOR_ID: AtomicU64 = AtomicU64::new(1);

/// A process-unique identity for a tensor's current contents.
#[cfg(target_os = "macos")]
pub(crate) fn fresh_tensor_id() -> u64 {
    NEXT_TENSOR_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct GpuTensor {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    #[cfg(not(target_os = "macos"))]
    pub(crate) data: RefCell<Vec<f32>>,
    #[cfg(target_os = "macos")]
    pub(crate) storage: RefCell<TensorStorage>,
    #[cfg(target_os = "macos")]
    pub(crate) writer_batch: Cell<u64>,
    #[cfg(target_os = "macos")]
    pub(crate) u32s: RefCell<Vec<u32>>,
    /// Identity of the current contents: fresh at creation and after every
    /// in-place write, so device-side caches keyed by it can never serve a
    /// stale weight (a pointer can be reused; an id cannot).
    #[cfg(target_os = "macos")]
    pub(crate) id: Cell<u64>,
}

impl GpuTensor {
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn is_half(&self) -> bool {
        false
    }
}

pub struct GpuLinearPart<'a> {
    pub bt_ggml_type: u32,
    pub n: usize,
    pub cache_key: &'a str,
    pub bytes: &'a [u8],
}

#[cfg(target_os = "macos")]
pub(crate) struct TensorStorage {
    pub(crate) buffer: std::sync::Arc<crate::shim::TensorBuffer>,
    pub(crate) offset: usize,
}

// Ambiguous inference if GpuTensor ever becomes Sync. RefCell also guards
// mapped host borrows, while its buffer can be shared immutably across threads.
const _: fn() = || {
    trait AmbiguousIfSync<A> { fn check() {} }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
    let _ = <GpuTensor as AmbiguousIfSync<_>>::check;
    fn is_send<T: Send>() {}
    is_send::<GpuTensor>();
};
