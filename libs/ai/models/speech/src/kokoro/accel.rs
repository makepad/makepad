//! Metal offload for the matrix products the model spends its time in.
//!
//! Every hot path in Kokoro reduces to a matmul: `linear` directly, and the
//! convolutions via im2col. This module is the single gateway — callers try it
//! and fall back to the pure-Rust loops in [`super::ops`], which stay the
//! reference implementation. Off Apple platforms every function returns `None`
//! at zero cost.
//!
//! Small products stay on the CPU: a GPU dispatch costs more than computing a
//! `[1, 256] x [256, 242]` projection in place.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Below this many multiply-accumulates the dispatch overhead wins.
const MIN_MACS: usize = 4 * 1024 * 1024;

static FORCE_CPU: AtomicBool = AtomicBool::new(false);

/// Route everything through the pure-Rust reference paths. The parity harness
/// sets this for its strict pass; `MAKEPAD_TTS_CPU=1` does the same for a
/// whole process.
pub fn force_cpu(value: bool) {
    FORCE_CPU.store(value, Ordering::Relaxed);
}

fn enabled() -> bool {
    static ENV_CPU: OnceLock<bool> = OnceLock::new();
    let env_cpu =
        *ENV_CPU.get_or_init(|| std::env::var_os("MAKEPAD_TTS_CPU").is_some_and(|v| v != "0"));
    !env_cpu && !FORCE_CPU.load(Ordering::Relaxed)
}

fn worth_it(m: usize, k: usize, n: usize) -> bool {
    enabled() && m * k * n >= MIN_MACS
}

/// `C[m, n] = A[m, k] * B[k, n]`, both row-major.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn matmul_nn(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    if !worth_it(m, k, n) {
        return None;
    }
    makepad_ggml::backend::metal::try_matmul_nn_f32(a, b, m, k, n)
}

/// `C[m, n] = A[m, k] * B^T`, with `bt` stored `[n, k]` row-major — PyTorch's
/// `nn.Linear` weight layout, used verbatim.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn matmul_nt(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    if !worth_it(m, k, n) {
        return None;
    }
    makepad_ggml::backend::metal::try_matmul_nt_f32(a, bt, m, k, n)
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn matmul_nn(_a: &[f32], _b: &[f32], _m: usize, _k: usize, _n: usize) -> Option<Vec<f32>> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn matmul_nt(_a: &[f32], _bt: &[f32], _m: usize, _k: usize, _n: usize) -> Option<Vec<f32>> {
    None
}
