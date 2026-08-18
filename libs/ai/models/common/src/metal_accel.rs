//! Metal offload for the host-path GEMMs and unmasked attention the audio
//! backends spend their time in (SA3 / Woosh / Moss).
//!
//! Same shape as `libs/tts` Kokoro accel: try the existing ggml Metal kernels,
//! fall back to the pure-Rust loops which stay the reference. No new shaders.
//! `MAKEPAD_DIFFUSION_CPU=1` forces the CPU path (oracle / sa3-validate).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Below this many multiply-accumulates a GPU dispatch is not worth it.
const MIN_MACS: usize = 4 * 1024 * 1024;

static FORCE_CPU: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn force_cpu(value: bool) {
    FORCE_CPU.store(value, Ordering::Relaxed);
}

fn enabled() -> bool {
    static ENV_CPU: OnceLock<bool> = OnceLock::new();
    let env_cpu = *ENV_CPU.get_or_init(|| {
        std::env::var_os("MAKEPAD_DIFFUSION_CPU").is_some_and(|v| v != "0")
    });
    !env_cpu && !FORCE_CPU.load(Ordering::Relaxed)
}

fn worth_it(m: usize, k: usize, n: usize) -> bool {
    enabled() && m.saturating_mul(k).saturating_mul(n) >= MIN_MACS
}

/// Whether Metal GEMM offload is allowed (honors `MAKEPAD_DIFFUSION_CPU`).
/// For callers that assemble their own compat-backend calls (vocoder tconv).
pub fn gemm_enabled() -> bool {
    enabled()
}

fn mark_used() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| eprintln!("[diffusion] metal accel on"));
}

/// `C[m, n] = A[m, k] * W[n, k]^T (+ bias[n])` — torch `nn.Linear` layout.
pub fn linear_nt(
    a: &[f32],
    w: &[f32],
    bias: Option<&[f32]>,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !worth_it(m, k, n) {
        return None;
    }
    let mut out = crate::backend::try_matmul_nt_f32(a, w, m, k, n)?;
    if let Some(b) = bias {
        if b.len() != n || out.len() != m * n {
            return None;
        }
        for row in out.chunks_mut(n) {
            for (v, bv) in row.iter_mut().zip(b.iter()) {
                *v += *bv;
            }
        }
    }
    mark_used();
    Some(out)
}

/// Packed `[tokens, heads, d]` flash attention. No mask, no GQA.
pub fn flash_attn_packed(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    if !enabled() {
        return None;
    }
    let macs = n_q
        .saturating_mul(n_kv)
        .saturating_mul(n_head)
        .saturating_mul(d);
    if macs < MIN_MACS {
        return None;
    }
    let out = crate::backend::try_flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale)?;
    mark_used();
    Some(out)
}

/// In-place RMSNorm * gamma over `width`-wide rows.
pub fn rms_norm_mul_inplace(x: &mut [f32], gamma: &[f32], width: usize, eps: f32) -> bool {
    if !enabled() || width == 0 || gamma.len() != width || x.len() % width != 0 {
        return false;
    }
    let rows = x.len() / width;
    if rows.saturating_mul(width) < 64 * 1024 {
        return false;
    }
    let Some(out) = crate::backend::try_rms_norm_mul_f32(
        x,
        &[rows, width],
        gamma,
        &[width],
        eps,
    ) else {
        return false;
    };
    if out.len() != x.len() {
        return false;
    }
    x.copy_from_slice(&out);
    mark_used();
    true
}
