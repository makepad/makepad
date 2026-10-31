//! Accelerator gateway for matrix products and resident Vulkan blocks.
//!
//! Every hot path in Kokoro reduces to a matmul: `linear` directly, and the
//! convolutions via im2col. This module is the single gateway — callers try it
//! and fall back to the pure-Rust loops in [`super::ops`], which stay the
//! reference implementation. Android selects Vulkan; Linux selects it with
//! `MAKEPAD_KOKORO_BACKEND=vulkan`. Other targets keep their existing path.
//!
//! Metal declines small standalone products. Vulkan records small operations
//! with their blocks so intermediate activations stay on the device.

use super::ops::{LstmLayer, Mat};
use std::sync::atomic::{AtomicBool, Ordering};

/// Below this many multiply-accumulates the dispatch overhead wins.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const MIN_MACS: usize = 4 * 1024 * 1024;

static FORCE_CPU: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CPU: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn cpu_forced() -> bool {
    #[cfg(test)]
    if TEST_CPU.with(|v| v.get()) { return true; }
    FORCE_CPU.load(Ordering::Relaxed)
}

/// The active backend, including initialization failure and forced CPU mode.
pub fn backend_name() -> &'static str {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if super::vulkan::available() { return "vulkan"; }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if enabled() { return "metal"; }
    "cpu"
}

/// Route everything through the pure-Rust reference paths.
pub fn force_cpu(value: bool) {
    FORCE_CPU.store(value, Ordering::Relaxed);
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn enabled() -> bool {
    !FORCE_CPU.load(Ordering::Relaxed)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn worth_it(m: usize, k: usize, n: usize) -> bool {
    enabled() && m * k * n >= MIN_MACS
}

/// `C[m, n] = A[m, k] * B[k, n]`, both row-major.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn matmul_nn(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    if !worth_it(m, k, n) {
        return None;
    }
    makepad_ai_common::backend::metal::try_matmul_nn_f32(a, b, m, k, n)
}

/// `C[m, n] = A[m, k] * B^T`, with `bt` stored `[n, k]` row-major — PyTorch's
/// `nn.Linear` weight layout, used verbatim.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn matmul_nt(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    if !worth_it(m, k, n) {
        return None;
    }
    makepad_ai_common::backend::metal::try_matmul_nt_f32(a, bt, m, k, n)
}

// ---------------------------------------------------------------- CUDA
//
// The same gateway on Linux/Windows with the CUDA kernels built in: cuBLAS
// f32 GEMMs with the weight operand kept device-resident (keyed by a content
// fingerprint, bounded), the activations uploaded per call. The pure-Rust loops
// stay the reference and the fallback when no device answers.

#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
mod cuda {
    use super::{LstmLayer, Mat};
    use super::FORCE_CPU;
    use makepad_ai_cuda::launch::{
        gpu_device_available, gpu_download, gpu_linear_f32_resident, gpu_pool_trim, gpu_upload,
        kokoro_conv1d, kokoro_lstm, KokoroWeightCache,
    };
    use std::cell::RefCell;
    use std::sync::atomic::Ordering;

    const MIN_MACS: usize = 1024 * 1024;
    const MIN_CONV_MACS: usize = 64 * 1024;
    static TRIM_EVERY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn trim_every() -> usize {
        static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *N.get_or_init(|| std::env::var("KOKORO_TRIM_EVERY").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(16))
    }

    thread_local! {
        static WEIGHTS: RefCell<KokoroWeightCache> = RefCell::new(KokoroWeightCache::default());
        static DEGRADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static CONV_DEGRADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static LSTM_DEGRADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn available() -> bool {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(gpu_device_available)
    }

    fn fingerprint(values: &[f32]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        mix(values.len() as u64);
        let step = (values.len() / 256).max(1);
        let mut i = 0;
        while i < values.len() {
            mix(values[i].to_bits() as u64);
            i += step;
        }
        if let Some(last) = values.last() {
            mix(last.to_bits() as u64);
        }
        h
    }

    fn enabled() -> bool {
        !FORCE_CPU.load(Ordering::Relaxed) && !DEGRADED.with(|d| d.get()) && available()
    }

    pub fn worth_it(m: usize, k: usize, n: usize) -> bool {
        !FORCE_CPU.load(Ordering::Relaxed) && !DEGRADED.with(|d| d.get()) && m * k * n >= MIN_MACS && available()
    }

    /// Largest working set of one device product (activations in + out), bytes.
    /// The big generator convolutions are `[m, k] x [n, k]^T` with m in the tens of
    /// thousands: done whole they ask the card for gigabytes; in row blocks of this
    /// size the weights stay resident and the peak stays small and flat.
    const BLOCK_BYTES: usize = 48 << 20;

    /// `x[m, k] * w[n, k]^T` with `w` cached on the device, in row blocks of `x`.
    pub fn linear(x: &[f32], w: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
        let rows_per_block = (BLOCK_BYTES / (4 * (k + n)).max(1)).clamp(1, m.max(1));
        let result = (|| -> Result<Vec<f32>, String> {
            WEIGHTS.with(|cache| {
                let weight = cache.borrow_mut().weight(w, n, k, fingerprint(w))?;
                let mut out = Vec::with_capacity(m * n);
                let mut row = 0;
                while row < m {
                    let rows = rows_per_block.min(m - row);
                    let xd = gpu_upload(&x[row * k..(row + rows) * k], rows, k)?;
                    let yd = gpu_linear_f32_resident(&xd, &weight, None)?;
                    out.extend_from_slice(&gpu_download(&yd)?);
                    row += rows;
                }
                Ok(out)
            })
        })();
        finish(result, "matmul", &DEGRADED)
    }

    fn finish(
        result: Result<Vec<f32>, String>,
        operation: &str,
        degraded: &'static std::thread::LocalKey<std::cell::Cell<bool>>,
    ) -> Option<Vec<f32>> {
        if TRIM_EVERY.fetch_add(1, Ordering::Relaxed) % trim_every() == trim_every() - 1 {
            gpu_pool_trim();
        }
        match result {
            Ok(v) => Some(v),
            Err(why) => {
                eprintln!("[kokoro] CUDA {operation} failed ({why}); using the fallback path");
                degraded.with(|d| d.set(true));
                None
            }
        }
    }

    pub fn conv1d(
        x: &Mat,
        weight: &[f32],
        bias: &[f32],
        out_channels: usize,
        pad: usize,
        stride: usize,
        dilation: usize,
    ) -> Option<Mat> {
        let kernel = weight.len().checked_div(out_channels.checked_mul(x.rows)?)?;
        let span = dilation.checked_mul(kernel.checked_sub(1)?)?.checked_add(1)?;
        let out_time = x.cols.checked_add(pad.checked_mul(2)?)?
            .saturating_sub(span).checked_div(stride)?.checked_add(1)?;
        let macs = out_channels.saturating_mul(x.rows).saturating_mul(kernel).saturating_mul(out_time);
        if macs < MIN_CONV_MACS || CONV_DEGRADED.with(|d| d.get()) || !enabled() {
            return None;
        }
        let result = WEIGHTS.with(|cache| {
            kokoro_conv1d(
                &x.data, x.rows, x.cols, weight, bias, out_channels, pad, stride, dilation,
                [fingerprint(weight), fingerprint(bias)], &mut cache.borrow_mut(),
            )
        });
        // A direct-kernel failure must still allow conv1d_general's existing
        // im2col/cuBLAS fallback to try; do not degrade the matmul gateway.
        finish(result, "conv1d", &CONV_DEGRADED)
            .map(|data| Mat::from_vec(out_channels, out_time, data))
    }

    pub fn lstm(layer: &LstmLayer, x: &Mat, reverse: bool) -> Option<Mat> {
        if x.rows == 0 || x.cols != layer.input || LSTM_DEGRADED.with(|d| d.get()) || !enabled() {
            return None;
        }
        let result = WEIGHTS.with(|cache| {
            kokoro_lstm(
                &x.data, x.rows, layer.input, layer.hidden,
                &layer.weight_ih, &layer.weight_hh, &layer.bias_ih, &layer.bias_hh, reverse,
                [
                    fingerprint(&layer.weight_ih), fingerprint(&layer.weight_hh),
                    fingerprint(&layer.bias_ih), fingerprint(&layer.bias_hh),
                ],
                &mut cache.borrow_mut(),
            )
        });
        finish(result, "LSTM", &LSTM_DEGRADED)
            .map(|data| Mat::from_vec(x.rows, layer.hidden, data))
    }

    /// Free this thread's device-resident Kokoro weights.
    pub fn release() {
        WEIGHTS.with(|cache| cache.borrow_mut().clear());
        gpu_pool_trim();
    }
}

/// CUDA direct convolution; other backends retain the existing matmul/CPU paths.
pub fn conv1d(
    x: &Mat,
    weight: &[f32],
    bias: &[f32],
    out_channels: usize,
    pad: usize,
    stride: usize,
    dilation: usize,
) -> Option<Mat> {
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    {
        return cuda::conv1d(x, weight, bias, out_channels, pad, stride, dilation);
    }
    #[allow(unreachable_code)]
    {
        let _ = (x, weight, bias, out_channels, pad, stride, dilation);
        None
    }
}

/// CUDA LSTM; the unmodified Rust recurrence remains the fallback/reference.
pub fn lstm(layer: &LstmLayer, x: &Mat, reverse: bool) -> Option<Mat> {
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    {
        return cuda::lstm(layer, x, reverse);
    }
    #[allow(unreachable_code)]
    {
        let _ = (layer, x, reverse);
        None
    }
}

/// Free the device memory this thread holds for Kokoro (cached weights).
pub fn release_device_state() {
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    cuda::release();
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn matmul_nn(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(c) = super::vulkan::matmul(a, b, m, k, n, false) {
        return Some(c);
    }
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    {
        if !cuda::worth_it(m, k, n) || a.len() != m * k || b.len() != k * n {
            return None;
        }
        // C = A * B: the weight operand (A, [m, k]) stays resident; the
        // activations B [k, n] are transposed so the GEMM is `x * w^T` with
        // x = B^T [n, k], w = A [m, k]: that gives C^T [n, m].
        let mut bt = vec![0f32; k * n];
        for row in 0..k {
            for col in 0..n {
                bt[col * k + row] = b[row * n + col];
            }
        }
        let ct = cuda::linear(&bt, a, n, k, m)?;
        let mut c = vec![0f32; m * n];
        for row in 0..n {
            for col in 0..m {
                c[col * n + row] = ct[row * m + col];
            }
        }
        return Some(c);
    }
    #[allow(unreachable_code)]
    {
        let _ = (a, b, m, k, n);
        None
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn matmul_nt(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(c) = super::vulkan::matmul(a, bt, m, k, n, true) {
        return Some(c);
    }
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    {
        if !cuda::worth_it(m, k, n) || a.len() != m * k || bt.len() != n * k {
            return None;
        }
        // Keep the smaller operand (the weight) resident: when `bt` is the big
        // one (im2col patches) compute the transpose, `bt * a^T`, instead.
        if n > m {
            let ct = cuda::linear(bt, a, n, k, m)?;
            let mut c = vec![0f32; m * n];
            for row in 0..n {
                for col in 0..m {
                    c[col * n + row] = ct[row * m + col];
                }
            }
            return Some(c);
        }
        return cuda::linear(a, bt, m, k, n);
    }
    #[allow(unreachable_code)]
    {
        let _ = (a, bt, m, k, n);
        None
    }
}
