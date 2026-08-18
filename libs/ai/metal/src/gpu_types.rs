//! Host-backed tensor handle used by the macOS Metal GpuTensor path.

use std::cell::RefCell;

pub struct GpuTensor {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) data: RefCell<Vec<f32>>,
    pub(crate) u32s: RefCell<Vec<u32>>,
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
