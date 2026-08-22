//! Per-op-class GPU accel profiling counters, opt-in via MAKEPAD_GPU_PROF=1.
//!
//! Recording happens at the backend entry points (accel.rs dispatchers, the
//! metal/compat.rs shims, and inside the CUDA dense-linear cached path where
//! the upload/gemm/download phases are split with explicit stream syncs when
//! profiling is enabled). All counters are plain atomics so this module
//! compiles on every platform; when the env flag is absent the only cost is a
//! branch at each site.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

pub const CAT_DENSE_TOTAL: usize = 0;
pub const CAT_DENSE_WEIGHT_UPLOAD: usize = 1;
pub const CAT_DENSE_UPLOAD: usize = 2;
pub const CAT_DENSE_GEMM: usize = 3;
pub const CAT_DENSE_DOWNLOAD: usize = 4;
pub const CAT_FLASH_ATTN: usize = 5;
pub const CAT_ATTN_SOFTMAX_WS: usize = 6;
pub const CAT_LAYER_NORM: usize = 7;
pub const CAT_RMS_NORM: usize = 8;
pub const CAT_ELEMENTWISE: usize = 9;
pub const CAT_MATMUL_UNCACHED: usize = 10;
pub const CAT_CONV2D: usize = 11;
pub const CAT_GROUP_NORM: usize = 12;
pub const CAT_COUNT: usize = 13;

pub const CAT_NAMES: [&str; CAT_COUNT] = [
    "dense_total",
    "dense_weight_upload",
    "dense_upload",
    "dense_gemm",
    "dense_download",
    "flash_attn",
    "attn_softmax_ws",
    "layer_norm",
    "rms_norm",
    "elementwise",
    "matmul_uncached",
    "conv2d",
    "group_norm",
];

struct Counters {
    ns: [AtomicU64; CAT_COUNT],
    calls: [AtomicU64; CAT_COUNT],
    bytes: [AtomicU64; CAT_COUNT],
}

impl Counters {
    const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            ns: [ZERO; CAT_COUNT],
            calls: [ZERO; CAT_COUNT],
            bytes: [ZERO; CAT_COUNT],
        }
    }
}

static COUNTERS: Counters = Counters::new();

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_GPU_PROF").is_some())
}

/// Record `elapsed` (and optionally an approximate host<->device byte count)
/// against a category. No-op when profiling is disabled.
pub fn record(cat: usize, start: Instant, bytes: u64) {
    if !enabled() || cat >= CAT_COUNT {
        return;
    }
    let ns = start.elapsed().as_nanos() as u64;
    COUNTERS.ns[cat].fetch_add(ns, Ordering::Relaxed);
    COUNTERS.calls[cat].fetch_add(1, Ordering::Relaxed);
    if bytes > 0 {
        COUNTERS.bytes[cat].fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Format all non-zero categories as `name calls total_ms mb` lines and reset
/// the counters, so callers can report per-phase deltas.
pub fn report_and_reset(prefix: &str) -> String {
    let mut out = String::new();
    for cat in 0..CAT_COUNT {
        let ns = COUNTERS.ns[cat].swap(0, Ordering::Relaxed);
        let calls = COUNTERS.calls[cat].swap(0, Ordering::Relaxed);
        let bytes = COUNTERS.bytes[cat].swap(0, Ordering::Relaxed);
        if calls == 0 {
            continue;
        }
        out.push_str(&format!(
            "{prefix}{}.ms={:.1} calls={} mb={:.1}\n",
            CAT_NAMES[cat],
            ns as f64 / 1.0e6,
            calls,
            bytes as f64 / 1.0e6,
        ));
    }
    out
}
