//! Host-backed tensor handle used by the macOS Metal GpuTensor path.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TENSOR_ID: AtomicU64 = AtomicU64::new(1);

/// A process-unique identity for a tensor's current contents.
pub(crate) fn fresh_tensor_id() -> u64 {
    NEXT_TENSOR_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct GpuTensor {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) data: RefCell<Vec<f32>>,
    pub(crate) u32s: RefCell<Vec<u32>>,
    /// Identity of the current contents: fresh at creation and after every
    /// in-place write, so device-side caches keyed by it can never serve a
    /// stale weight (a pointer can be reused; an id cannot).
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
