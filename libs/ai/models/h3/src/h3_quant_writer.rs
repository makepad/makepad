//! Safetensors -> GGUF Q4_K writer for the H3 DiT family: turns a bf16
//! diffusers transformer shard set (the FastVideo FastH3 4-step distillation,
//! or any checkpoint with the canonical MiniMax-H3 DiT tensor inventory)
//! into the exact pruned-Q4_K GGUF layout `h3_quant.rs` loads for the
//! `*-q4-24g` tiers:
//!
//! - sd.cpp source naming (`blocks.N.*`, `video_patch_proj.*`,
//!   `final_layer.*`), fused `attn.qkv_proj` rows `[q|k|v]`, and the gated
//!   FFN input projection stored `[gate|value]` (the loader exchanges the
//!   halves back on the way in — see `gated_ffn_proj`).
//! - AdaLN rank-8 pruning: the `time_embedder` MLP is evaluated over a
//!   1025-point timestep grid, `silu(temb(t))` is factorized to rank 8, the
//!   basis is folded into every `adaln_proj.linear.weight` (and
//!   `norm_out.linear.weight`), and the rank-8 coefficients ship as the F32
//!   `adaln_t_table` `[8,1025]` the loader turns into [`H3AdalnCurve`].
//! - Q4_K quantization is a faithful port of upstream ggml
//!   `quantize_row_q4_K_ref` (`make_qkx2_quants` weighted scale/min search,
//!   `nearest_int` round-half-even, f16 round-to-nearest-even superblock
//!   scales — never truncation).
//!
//! Block/refiner attention and FFN linears become Q4_K; every model-boundary
//! projection (patch/audio/context in, video/audio out) stays BF16; norms,
//! biases and the curve stay F32. VSA gate tensors
//! (`attn.to_gate_compress`) are never emitted — the dense-attention tiers
//! never read them.

use crate::error::{DiffusionError, Result};
use crate::h3::{
    h3_timestep_embedding, H3ShardedWeights, H3_AUDIO_IN_CHANNELS, H3_DEPTH, H3_FFN_DIM,
    H3_FREQ_DIM, H3_HEAD_COUNT, H3_HEAD_DIM, H3_HIDDEN_SIZE, H3_MODALITY_NUM, H3_REFINER_DEPTH,
    H3_TEXT_DIM, H3_TIME_EMBED_DIM, H3_TIME_EMBED_HIDDEN, H3_VIDEO_PATCH_DIM,
};
use makepad_ai_common::quant::{
    dequantize_q4_k, f16_to_f32, f32_to_f16_rn, GGML_TYPE_BF16, GGML_TYPE_F32, GGML_TYPE_Q4_K,
    QK_K,
};
use makepad_ai_loader::MlxDType;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Rank of the AdaLN timestep curve. Pinned by the loader:
/// `validate_dit_block` requires every folded adaln linear to be `[cols,8]`
/// and `load_adaln_curve` requires the F32 `[8,1025]` table.
pub const ADALN_RANK: usize = 8;
/// Grid points of the curve over `t in [0,1]` (loader-pinned).
pub const ADALN_GRID: usize = 1025;
/// The FastH3 4-step DMD timesteps (fastvideo_inference.json
/// `dmd_denoising_steps` / 1000): where curve fidelity matters most.
pub const FASTH3_DMD_TIMESTEPS: [f32; 4] = [0.999, 0.749, 0.5, 0.25];

const Q4_K_BLOCK_BYTES: usize = 144;
const GGUF_ALIGNMENT: u64 = 32;

fn model_error(message: impl Into<String>) -> DiffusionError {
    DiffusionError::model(message.into())
}

// ---------------------------------------------------------------------------
// ggml reference Q4_K quantization (quantize_row_q4_K_ref port)
// ---------------------------------------------------------------------------

/// Upstream ggml `nearest_int`: round-half-to-even via the f32 magic-number
/// trick. Valid for |v| <= 4194303.
#[inline]
pub fn nearest_int(v: f32) -> i32 {
    debug_assert!(v.abs() <= 4_194_303.0);
    let shifted = v + 12_582_912.0;
    ((shifted.to_bits() & 0x007f_ffff) as i32) - 0x0040_0000
}

/// Upstream ggml `make_qkx2_quants` (rmin=-1, rdelta=0.1, nstep=20,
/// use_mad=false): weighted least-squares search for the best per-group
/// scale/min over 21 candidate step sizes.
fn make_qkx2_quants(
    nmax: i32,
    x: &[f32],
    weights: &[f32],
    l: &mut [u8],
    laux: &mut [u8],
) -> (f32, f32) {
    let n = x.len();
    let mut min = x[0];
    let mut max = x[0];
    let mut sum_w = weights[0];
    let mut sum_x = sum_w * x[0];
    for i in 1..n {
        if x[i] < min {
            min = x[i];
        }
        if x[i] > max {
            max = x[i];
        }
        let w = weights[i];
        sum_w += w;
        sum_x += w * x[i];
    }
    if min > 0.0 {
        min = 0.0;
    }
    if max == min {
        for value in l.iter_mut().take(n) {
            *value = 0;
        }
        return (0.0, -min);
    }
    let mut iscale = nmax as f32 / (max - min);
    let mut scale = 1.0 / iscale;
    let mut best_mad = 0.0f32;
    for i in 0..n {
        let li = nearest_int(iscale * (x[i] - min)).clamp(0, nmax);
        l[i] = li as u8;
        let diff = scale * li as f32 + min - x[i];
        best_mad += weights[i] * diff * diff;
    }
    const RMIN: f32 = -1.0;
    const RDELTA: f32 = 0.1;
    const NSTEP: i32 = 20;
    for is in 0..=NSTEP {
        iscale = (RMIN + RDELTA * is as f32 + nmax as f32) / (max - min);
        let (mut sum_l, mut sum_l2, mut sum_xl) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let li = nearest_int(iscale * (x[i] - min)).clamp(0, nmax);
            laux[i] = li as u8;
            let w = weights[i];
            let lf = li as f32;
            sum_l += w * lf;
            sum_l2 += w * lf * lf;
            sum_xl += w * lf * x[i];
        }
        let d = sum_w * sum_l2 - sum_l * sum_l;
        if d > 0.0 {
            let mut this_scale = (sum_w * sum_xl - sum_x * sum_l) / d;
            let mut this_min = (sum_l2 * sum_x - sum_l * sum_xl) / d;
            if this_min > 0.0 {
                this_min = 0.0;
                this_scale = sum_xl / sum_l2;
            }
            let mut mad = 0.0f32;
            for i in 0..n {
                let diff = this_scale * laux[i] as f32 + this_min - x[i];
                mad += weights[i] * diff * diff;
            }
            if mad < best_mad {
                l[..n].copy_from_slice(&laux[..n]);
                best_mad = mad;
                scale = this_scale;
                min = this_min;
            }
        }
    }
    (scale, -min)
}

/// Quantize one 256-value superblock into the 144-byte Q4_K layout
/// (`d` f16, `dmin` f16, 12 bytes 6-bit scales/mins, 128 bytes nibbles),
/// exactly upstream ggml `quantize_row_q4_K_ref`.
pub fn quantize_q4_k_block(x: &[f32], out: &mut [u8]) {
    assert_eq!(x.len(), QK_K);
    assert!(out.len() >= Q4_K_BLOCK_BYTES);
    let mut l = [0u8; QK_K];
    let mut laux = [0u8; 32];
    let mut weights = [0.0f32; 32];
    let mut scales = [0.0f32; QK_K / 32];
    let mut mins = [0.0f32; QK_K / 32];
    let mut max_scale = 0.0f32;
    let mut max_min = 0.0f32;
    for j in 0..QK_K / 32 {
        let group = &x[32 * j..32 * (j + 1)];
        let mut sum_x2 = 0.0f32;
        for value in group {
            sum_x2 += value * value;
        }
        let av_x = (sum_x2 / 32.0).sqrt();
        for (w, value) in weights.iter_mut().zip(group) {
            *w = av_x + value.abs();
        }
        let (scale, min) =
            make_qkx2_quants(15, group, &weights, &mut l[32 * j..32 * (j + 1)], &mut laux);
        scales[j] = scale;
        mins[j] = min;
        if scale > max_scale {
            max_scale = scale;
        }
        if min > max_min {
            max_min = min;
        }
    }
    let inv_scale = if max_scale > 0.0 { 63.0 / max_scale } else { 0.0 };
    let inv_min = if max_min > 0.0 { 63.0 / max_min } else { 0.0 };
    let mut packed = [0u8; 12];
    for j in 0..QK_K / 32 {
        let ls = (nearest_int(inv_scale * scales[j]).max(0) as u32).min(63) as u8;
        let lm = (nearest_int(inv_min * mins[j]).max(0) as u32).min(63) as u8;
        if j < 4 {
            packed[j] = ls;
            packed[j + 4] = lm;
        } else {
            packed[j + 4] = (ls & 0x0f) | ((lm & 0x0f) << 4);
            packed[j - 4] |= (ls >> 4) << 6;
            packed[j] |= (lm >> 4) << 6;
        }
    }
    let d = f32_to_f16_rn(max_scale / 63.0);
    let dmin = f32_to_f16_rn(max_min / 63.0);
    out[0..2].copy_from_slice(&d.to_le_bytes());
    out[2..4].copy_from_slice(&dmin.to_le_bytes());
    out[4..16].copy_from_slice(&packed);
    // Requantize against the f16-rounded d/dmin actually stored.
    let df = f16_to_f32(d);
    let dminf = f16_to_f32(dmin);
    for j in 0..QK_K / 32 {
        let (sc, m) = unpack_scale_min(j, &packed);
        let dg = df * sc as f32;
        if dg == 0.0 {
            continue;
        }
        let mg = dminf * m as f32;
        for i in 0..32 {
            let li = nearest_int((x[32 * j + i] + mg) / dg).clamp(0, 15);
            l[32 * j + i] = li as u8;
        }
    }
    let qs = &mut out[16..Q4_K_BLOCK_BYTES];
    for j in (0..QK_K).step_by(64) {
        let base = (j / 64) * 32;
        for i in 0..32 {
            qs[base + i] = l[j + i] | (l[j + i + 32] << 4);
        }
    }
}

/// The 6-bit scale/min unpack (`get_scale_min_k4`), mirrored here for the
/// requantization pass and pinned against the decoder by tests.
fn unpack_scale_min(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        (
            (q[j + 4] & 0x0f) | ((q[j - 4] >> 6) << 4),
            (q[j + 4] >> 4) | ((q[j] >> 6) << 4),
        )
    }
}

/// Bytes of one Q4_K row of `k` values (`k % 256 == 0`).
pub fn q4_k_row_bytes(k: usize) -> usize {
    (k / QK_K) * Q4_K_BLOCK_BYTES
}

/// Quantize an `n x k` f32 matrix (row-major) to Q4_K rows, threaded over
/// row ranges. Rows quantize independently, so the split is exact.
pub fn quantize_q4_k_matrix(src: &[f32], n: usize, k: usize, threads: usize) -> Result<Vec<u8>> {
    if k == 0 || k % QK_K != 0 {
        return Err(model_error(format!(
            "q4_k row width {k} is not a multiple of {QK_K}"
        )));
    }
    if src.len() != n * k {
        return Err(model_error(format!(
            "q4_k matrix has {} values, expected {n}x{k}",
            src.len()
        )));
    }
    let row_bytes = q4_k_row_bytes(k);
    let mut out = vec![0u8; n * row_bytes];
    let threads = threads.clamp(1, n.max(1));
    let chunk_rows = n.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut rest = out.as_mut_slice();
        let mut row = 0usize;
        while row < n {
            let take = chunk_rows.min(n - row);
            let (chunk, tail) = rest.split_at_mut(take * row_bytes);
            rest = tail;
            let src_rows = &src[row * k..(row + take) * k];
            scope.spawn(move || {
                for r in 0..take {
                    let x = &src_rows[r * k..(r + 1) * k];
                    let dst = &mut chunk[r * row_bytes..(r + 1) * row_bytes];
                    for (b, xb) in x.chunks_exact(QK_K).enumerate() {
                        quantize_q4_k_block(
                            xb,
                            &mut dst[b * Q4_K_BLOCK_BYTES..(b + 1) * Q4_K_BLOCK_BYTES],
                        );
                    }
                }
            });
            row += take;
        }
    });
    Ok(out)
}

/// Relative RMS error of a Q4_K payload against its f32 source, sampled
/// every `stride` rows (stride 1 = exact).
pub fn q4_k_rel_rmse(payload: &[u8], src: &[f32], n: usize, k: usize, stride: usize) -> f64 {
    let row_bytes = q4_k_row_bytes(k);
    let mut err2 = 0.0f64;
    let mut ref2 = 0.0f64;
    let mut block = [0.0f32; QK_K];
    let mut row = 0usize;
    while row < n {
        let dst = &payload[row * row_bytes..(row + 1) * row_bytes];
        let x = &src[row * k..(row + 1) * k];
        for (b, xb) in x.chunks_exact(QK_K).enumerate() {
            dequantize_q4_k(&dst[b * Q4_K_BLOCK_BYTES..(b + 1) * Q4_K_BLOCK_BYTES], &mut block);
            for (recon, orig) in block.iter().zip(xb) {
                let diff = (*recon - *orig) as f64;
                err2 += diff * diff;
                ref2 += (*orig as f64) * (*orig as f64);
            }
        }
        row += stride.max(1);
    }
    if ref2 == 0.0 {
        return if err2 == 0.0 { 0.0 } else { f64::INFINITY };
    }
    (err2 / ref2).sqrt()
}

/// f32 -> bf16 with IEEE round-to-nearest-even (torch `.to(bfloat16)`
/// parity; the same conversion `h3_quant::f32_payload_to_bf16` applies at
/// load time to F32 payloads).
#[inline]
pub fn f32_to_bf16_rn(value: f32) -> u16 {
    let bits = value.to_bits();
    let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
    (rounded >> 16) as u16
}

// ---------------------------------------------------------------------------
// AdaLN rank-8 timestep curve (the "pruned" transform)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AdalnFit {
    pub rank: usize,
    pub grid: usize,
    pub dim: usize,
    /// `grid x rank` row-major: the curve coefficients (`adaln_t_table`).
    pub curve: Vec<f32>,
    /// `rank x dim` row-major: the basis folded into the adaln weights.
    pub basis: Vec<f64>,
    /// Relative Frobenius reconstruction error over the whole grid.
    pub rel_l2: f64,
    /// Worst per-grid-row relative L2 error.
    pub max_row_rel: f64,
    /// Worst relative row error at the FastH3 DMD timesteps.
    pub dmd_max_rel: f64,
}

fn host_linear_rows(
    x: &[f32],
    rows: usize,
    k: usize,
    w: &[f32],
    n: usize,
    bias: &[f32],
    threads: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; rows * n];
    let threads = threads.clamp(1, rows.max(1));
    let chunk_rows = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut rest = out.as_mut_slice();
        let mut row = 0usize;
        while row < rows {
            let take = chunk_rows.min(rows - row);
            let (chunk, tail) = rest.split_at_mut(take * n);
            rest = tail;
            let x_rows = &x[row * k..(row + take) * k];
            scope.spawn(move || {
                for r in 0..take {
                    let xr = &x_rows[r * k..(r + 1) * k];
                    for j in 0..n {
                        let wr = &w[j * k..(j + 1) * k];
                        let mut sum = bias[j];
                        for i in 0..k {
                            sum += wr[i] * xr[i];
                        }
                        chunk[r * n + j] = sum;
                    }
                }
            });
            row += take;
        }
    });
    out
}

fn host_silu(values: &mut [f32]) {
    for value in values.iter_mut() {
        *value = *value / (1.0 + (-*value).exp());
    }
}

/// Evaluate `silu(time_embedder(t))` over the 1025-point grid with the same
/// f32 formulas the runtime uses (`h3_timestep_embedding` -> linear_1 ->
/// silu -> linear_2 -> silu), one row per grid point.
pub fn silu_temb_grid(
    l1_w: &[f32],
    l1_b: &[f32],
    l2_w: &[f32],
    l2_b: &[f32],
    threads: usize,
) -> Vec<f32> {
    let timesteps: Vec<f32> = (0..ADALN_GRID)
        .map(|i| i as f32 / (ADALN_GRID - 1) as f32)
        .collect();
    let sinusoid = h3_timestep_embedding(&timesteps);
    let mut hidden = host_linear_rows(
        &sinusoid,
        ADALN_GRID,
        H3_FREQ_DIM,
        l1_w,
        H3_TIME_EMBED_HIDDEN,
        l1_b,
        threads,
    );
    host_silu(&mut hidden);
    let mut temb = host_linear_rows(
        &hidden,
        ADALN_GRID,
        H3_TIME_EMBED_HIDDEN,
        l2_w,
        H3_TIME_EMBED_DIM,
        l2_b,
        threads,
    );
    host_silu(&mut temb);
    temb
}

/// Best rank-`rank` factorization `M ~ C * B` of the `grid x dim` matrix
/// (rows = grid points): truncated SVD via subspace iteration on the
/// `grid x grid` Gram matrix in f64. Returns the fit with `C` as the curve
/// and `B` as the basis.
pub fn adaln_low_rank_fit(m: &[f32], grid: usize, dim: usize, rank: usize) -> Result<AdalnFit> {
    if m.len() != grid * dim || rank == 0 || rank > grid.min(dim) {
        return Err(model_error(format!(
            "adaln fit: bad problem {grid}x{dim} rank {rank} (len {})",
            m.len()
        )));
    }
    let mf: Vec<f64> = m.iter().map(|v| *v as f64).collect();
    // Gram matrix A = M * M^T (grid x grid), symmetric PSD.
    let mut a = vec![0.0f64; grid * grid];
    for i in 0..grid {
        let ri = &mf[i * dim..(i + 1) * dim];
        for j in i..grid {
            let rj = &mf[j * dim..(j + 1) * dim];
            let mut sum = 0.0f64;
            for k in 0..dim {
                sum += ri[k] * rj[k];
            }
            a[i * grid + j] = sum;
            a[j * grid + i] = sum;
        }
    }
    // Subspace iteration with head-room, then Rayleigh-Ritz on the block.
    let sub = (rank + 4).min(grid);
    let mut q = vec![0.0f64; grid * sub];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for value in q.iter_mut() {
        // xorshift64* in [-0.5, 0.5)
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let r = seed.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
        *value = (r as f64 / (1u64 << 53) as f64) - 0.5;
    }
    orthonormalize_columns(&mut q, grid, sub);
    let mut y = vec![0.0f64; grid * sub];
    let mut prev_trace = 0.0f64;
    for _iter in 0..500 {
        // y = A * q
        for i in 0..grid {
            let arow = &a[i * grid..(i + 1) * grid];
            for c in 0..sub {
                let mut sum = 0.0f64;
                for j in 0..grid {
                    sum += arow[j] * q[j * sub + c];
                }
                y[i * sub + c] = sum;
            }
        }
        // Convergence on the Rayleigh trace before re-orthonormalizing.
        let mut trace = 0.0f64;
        for c in 0..sub {
            let mut dot = 0.0f64;
            for i in 0..grid {
                dot += q[i * sub + c] * y[i * sub + c];
            }
            trace += dot;
        }
        std::mem::swap(&mut q, &mut y);
        orthonormalize_columns(&mut q, grid, sub);
        if prev_trace > 0.0 && ((trace - prev_trace).abs() / prev_trace) < 1e-14 {
            break;
        }
        prev_trace = trace;
    }
    // Rayleigh-Ritz: S = Q^T A Q, then a Jacobi eigen solve of the small S.
    let mut aq = vec![0.0f64; grid * sub];
    for i in 0..grid {
        let arow = &a[i * grid..(i + 1) * grid];
        for c in 0..sub {
            let mut sum = 0.0f64;
            for j in 0..grid {
                sum += arow[j] * q[j * sub + c];
            }
            aq[i * sub + c] = sum;
        }
    }
    let mut s = vec![0.0f64; sub * sub];
    for r in 0..sub {
        for c in 0..sub {
            let mut sum = 0.0f64;
            for i in 0..grid {
                sum += q[i * sub + r] * aq[i * sub + c];
            }
            s[r * sub + c] = sum;
        }
    }
    let (eigenvalues, vectors) = jacobi_eigen_symmetric(&mut s, sub);
    // Order by eigenvalue descending, take the top `rank`.
    let mut order: Vec<usize> = (0..sub).collect();
    order.sort_by(|x, y| eigenvalues[*y].partial_cmp(&eigenvalues[*x]).unwrap());
    // U = Q * V_top (grid x rank), sigma_r = sqrt(eigenvalue).
    let mut u = vec![0.0f64; grid * rank];
    let mut sigma = vec![0.0f64; rank];
    for (slot, src_col) in order.iter().take(rank).enumerate() {
        sigma[slot] = eigenvalues[*src_col].max(0.0).sqrt();
        for i in 0..grid {
            let mut sum = 0.0f64;
            for c in 0..sub {
                sum += q[i * sub + c] * vectors[c * sub + src_col];
            }
            u[i * rank + slot] = sum;
        }
    }
    // Basis rows b_r = M^T u_r / sigma_r; curve C = M * B^T rows.
    let mut basis = vec![0.0f64; rank * dim];
    for r in 0..rank {
        if sigma[r] <= 0.0 {
            continue;
        }
        for k in 0..dim {
            let mut sum = 0.0f64;
            for i in 0..grid {
                sum += mf[i * dim + k] * u[i * rank + r];
            }
            basis[r * dim + k] = sum / sigma[r];
        }
    }
    let mut curve = vec![0.0f32; grid * rank];
    let mut err2 = 0.0f64;
    let mut ref2 = 0.0f64;
    let mut max_row_rel = 0.0f64;
    let mut dmd_max_rel = 0.0f64;
    let dmd_rows: Vec<usize> = FASTH3_DMD_TIMESTEPS
        .iter()
        .map(|t| ((*t as f64) * (grid - 1) as f64).round() as usize)
        .collect();
    for i in 0..grid {
        let row = &mf[i * dim..(i + 1) * dim];
        let mut coefficients = vec![0.0f64; rank];
        for r in 0..rank {
            let mut sum = 0.0f64;
            for k in 0..dim {
                sum += row[k] * basis[r * dim + k];
            }
            coefficients[r] = sum;
            curve[i * rank + r] = sum as f32;
        }
        let mut row_err2 = 0.0f64;
        let mut row_ref2 = 0.0f64;
        for k in 0..dim {
            let mut recon = 0.0f64;
            for r in 0..rank {
                recon += coefficients[r] * basis[r * dim + k];
            }
            let diff = recon - row[k];
            row_err2 += diff * diff;
            row_ref2 += row[k] * row[k];
        }
        err2 += row_err2;
        ref2 += row_ref2;
        let row_rel = if row_ref2 > 0.0 {
            (row_err2 / row_ref2).sqrt()
        } else {
            0.0
        };
        if row_rel > max_row_rel {
            max_row_rel = row_rel;
        }
        if dmd_rows.contains(&i) && row_rel > dmd_max_rel {
            dmd_max_rel = row_rel;
        }
    }
    Ok(AdalnFit {
        rank,
        grid,
        dim,
        curve,
        basis,
        rel_l2: if ref2 > 0.0 { (err2 / ref2).sqrt() } else { 0.0 },
        max_row_rel,
        dmd_max_rel,
    })
}

fn orthonormalize_columns(q: &mut [f64], rows: usize, cols: usize) {
    for c in 0..cols {
        for prev in 0..c {
            let mut dot = 0.0f64;
            for i in 0..rows {
                dot += q[i * cols + c] * q[i * cols + prev];
            }
            for i in 0..rows {
                q[i * cols + c] -= dot * q[i * cols + prev];
            }
        }
        let mut norm2 = 0.0f64;
        for i in 0..rows {
            norm2 += q[i * cols + c] * q[i * cols + c];
        }
        let norm = norm2.sqrt();
        if norm > 0.0 {
            for i in 0..rows {
                q[i * cols + c] /= norm;
            }
        }
    }
}

/// Cyclic Jacobi eigendecomposition of a small symmetric matrix (in place).
/// Returns (eigenvalues, column eigenvectors `v[row * n + col]`).
fn jacobi_eigen_symmetric(s: &mut [f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0f64;
        for i in 0..n {
            for j in (i + 1)..n {
                off += s[i * n + j] * s[i * n + j];
            }
        }
        if off < 1e-24 {
            break;
        }
        for p in 0..n {
            for q_col in (p + 1)..n {
                let apq = s[p * n + q_col];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let app = s[p * n + p];
                let aqq = s[q_col * n + q_col];
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let sn = t * c;
                for k in 0..n {
                    let skp = s[k * n + p];
                    let skq = s[k * n + q_col];
                    s[k * n + p] = c * skp - sn * skq;
                    s[k * n + q_col] = sn * skp + c * skq;
                }
                for k in 0..n {
                    let spk = s[p * n + k];
                    let sqk = s[q_col * n + k];
                    s[p * n + k] = c * spk - sn * sqk;
                    s[q_col * n + k] = sn * spk + c * sqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q_col];
                    v[k * n + p] = c * vkp - sn * vkq;
                    v[k * n + q_col] = sn * vkp + c * vkq;
                }
            }
        }
    }
    let eigenvalues: Vec<f64> = (0..n).map(|i| s[i * n + i]).collect();
    (eigenvalues, v)
}

/// Fold the rank basis into one adaln linear: `W_fold = W * B^T`
/// (`n x dim` -> `n x rank`, f64 accumulation), threaded over output rows.
pub fn fold_adaln_weight(w: &[f32], n: usize, dim: usize, fit: &AdalnFit, threads: usize) -> Vec<f32> {
    assert_eq!(w.len(), n * dim);
    assert_eq!(fit.dim, dim);
    let rank = fit.rank;
    let mut out = vec![0.0f32; n * rank];
    let threads = threads.clamp(1, n.max(1));
    let chunk_rows = n.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut rest = out.as_mut_slice();
        let mut row = 0usize;
        while row < n {
            let take = chunk_rows.min(n - row);
            let (chunk, tail) = rest.split_at_mut(take * rank);
            rest = tail;
            let w_rows = &w[row * dim..(row + take) * dim];
            let basis = &fit.basis;
            scope.spawn(move || {
                for r in 0..take {
                    let wr = &w_rows[r * dim..(r + 1) * dim];
                    for c in 0..rank {
                        let br = &basis[c * dim..(c + 1) * dim];
                        let mut sum = 0.0f64;
                        for k in 0..dim {
                            sum += wr[k] as f64 * br[k];
                        }
                        chunk[r * rank + c] = sum as f32;
                    }
                }
            });
            row += take;
        }
    });
    out
}

// ---------------------------------------------------------------------------
// GGUF v3 container writer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum GgufWriteValue {
    U32(u32),
    F32(f32),
    Str(String),
}

#[derive(Clone, Debug)]
pub struct GgufTensorDecl {
    pub name: String,
    /// ggml dimension order: `[ne0(=k), ne1(=n), ..]`.
    pub dims: Vec<u64>,
    pub ggml_type: u32,
}

impl GgufTensorDecl {
    pub fn size_bytes(&self) -> Result<u64> {
        let (block_elems, block_bytes) = match self.ggml_type {
            GGML_TYPE_F32 => (1u64, 4u64),
            GGML_TYPE_BF16 => (1, 2),
            GGML_TYPE_Q4_K => (QK_K as u64, Q4_K_BLOCK_BYTES as u64),
            other => {
                return Err(model_error(format!(
                    "gguf writer: unsupported tensor type {other} for '{}'",
                    self.name
                )))
            }
        };
        let ne0 = *self.dims.first().ok_or_else(|| {
            model_error(format!("gguf writer: '{}' has no dimensions", self.name))
        })?;
        if ne0 == 0 || ne0 % block_elems != 0 {
            return Err(model_error(format!(
                "gguf writer: '{}' ne0 {ne0} not divisible by block {block_elems}",
                self.name
            )));
        }
        let mut size = (ne0 / block_elems) * block_bytes;
        for dim in &self.dims[1..] {
            size = size
                .checked_mul(*dim)
                .ok_or_else(|| model_error(format!("gguf writer: '{}' size overflow", self.name)))?;
        }
        Ok(size)
    }
}

/// Streaming GGUF v3 writer: declare metadata and every tensor up front,
/// then append each tensor's payload in declaration order. Offsets follow
/// the 32-byte alignment `GgufFile::open` expects.
pub struct GgufWriter {
    out: BufWriter<File>,
    decls: Vec<GgufTensorDecl>,
    sizes: Vec<u64>,
    offsets: Vec<u64>,
    next: usize,
    data_written: u64,
}

fn write_gguf_string(out: &mut impl Write, value: &str) -> std::io::Result<()> {
    out.write_all(&(value.len() as u64).to_le_bytes())?;
    out.write_all(value.as_bytes())
}

impl GgufWriter {
    pub fn create(
        path: &Path,
        kv: &[(String, GgufWriteValue)],
        decls: Vec<GgufTensorDecl>,
    ) -> Result<Self> {
        let file = File::create(path)
            .map_err(|err| model_error(format!("gguf create {}: {err}", path.display())))?;
        let mut out = BufWriter::new(file);
        let io = |err: std::io::Error| model_error(format!("gguf write {}: {err}", path.display()));

        // The alignment key is always present and always first, so the
        // reader's data_offset math is pinned regardless of caller kv.
        let mut header_kv: Vec<(String, GgufWriteValue)> = vec![(
            "general.alignment".to_string(),
            GgufWriteValue::U32(GGUF_ALIGNMENT as u32),
        )];
        header_kv.extend(kv.iter().cloned());

        out.write_all(b"GGUF").map_err(io)?;
        out.write_all(&3u32.to_le_bytes()).map_err(io)?;
        out.write_all(&(decls.len() as i64).to_le_bytes()).map_err(io)?;
        out.write_all(&(header_kv.len() as i64).to_le_bytes()).map_err(io)?;
        let mut header_len = 4u64 + 4 + 8 + 8;
        for (key, value) in &header_kv {
            write_gguf_string(&mut out, key).map_err(io)?;
            header_len += 8 + key.len() as u64;
            match value {
                GgufWriteValue::U32(v) => {
                    out.write_all(&4i32.to_le_bytes()).map_err(io)?;
                    out.write_all(&v.to_le_bytes()).map_err(io)?;
                    header_len += 4 + 4;
                }
                GgufWriteValue::F32(v) => {
                    out.write_all(&6i32.to_le_bytes()).map_err(io)?;
                    out.write_all(&v.to_le_bytes()).map_err(io)?;
                    header_len += 4 + 4;
                }
                GgufWriteValue::Str(v) => {
                    out.write_all(&8i32.to_le_bytes()).map_err(io)?;
                    write_gguf_string(&mut out, v).map_err(io)?;
                    header_len += 4 + 8 + v.len() as u64;
                }
            }
        }
        let mut sizes = Vec::with_capacity(decls.len());
        let mut offsets = Vec::with_capacity(decls.len());
        let mut offset = 0u64;
        for decl in &decls {
            let size = decl.size_bytes()?;
            debug_assert_eq!(offset % GGUF_ALIGNMENT, 0);
            offsets.push(offset);
            sizes.push(size);
            write_gguf_string(&mut out, &decl.name).map_err(io)?;
            out.write_all(&(decl.dims.len() as u32).to_le_bytes()).map_err(io)?;
            for dim in &decl.dims {
                out.write_all(&dim.to_le_bytes()).map_err(io)?;
            }
            out.write_all(&(decl.ggml_type as i32).to_le_bytes()).map_err(io)?;
            out.write_all(&offset.to_le_bytes()).map_err(io)?;
            header_len += 8 + decl.name.len() as u64 + 4 + 8 * decl.dims.len() as u64 + 4 + 8;
            offset = offset
                .checked_add(size)
                .and_then(|end| end.checked_add(GGUF_ALIGNMENT - 1))
                .map(|end| end / GGUF_ALIGNMENT * GGUF_ALIGNMENT)
                .ok_or_else(|| model_error("gguf writer: data offset overflow"))?;
        }
        // Pad the header up to the aligned data start.
        let data_start = header_len.div_ceil(GGUF_ALIGNMENT) * GGUF_ALIGNMENT;
        let pad = vec![0u8; (data_start - header_len) as usize];
        out.write_all(&pad).map_err(io)?;
        Ok(Self {
            out,
            decls,
            sizes,
            offsets,
            next: 0,
            data_written: 0,
        })
    }

    /// Append the payload of the next declared tensor. Must be called once
    /// per declaration, in order, with exactly the declared byte length.
    pub fn append_tensor(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let index = self.next;
        let decl = self.decls.get(index).ok_or_else(|| {
            model_error(format!("gguf writer: '{name}' appended past the declaration list"))
        })?;
        if decl.name != name {
            return Err(model_error(format!(
                "gguf writer: appended '{name}' but declaration {index} is '{}'",
                decl.name
            )));
        }
        if bytes.len() as u64 != self.sizes[index] {
            return Err(model_error(format!(
                "gguf writer: '{name}' payload {} bytes, declared {}",
                bytes.len(),
                self.sizes[index]
            )));
        }
        debug_assert_eq!(self.data_written, self.offsets[index]);
        let io = |err: std::io::Error| model_error(format!("gguf write '{name}': {err}"));
        self.out.write_all(bytes).map_err(io)?;
        self.data_written += bytes.len() as u64;
        let aligned = self.data_written.div_ceil(GGUF_ALIGNMENT) * GGUF_ALIGNMENT;
        if aligned > self.data_written {
            let pad = vec![0u8; (aligned - self.data_written) as usize];
            self.out.write_all(&pad).map_err(io)?;
            self.data_written = aligned;
        }
        self.next += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if self.next != self.decls.len() {
            return Err(model_error(format!(
                "gguf writer: only {}/{} declared tensors were appended",
                self.next,
                self.decls.len()
            )));
        }
        self.out
            .flush()
            .map_err(|err| model_error(format!("gguf flush: {err}")))
    }
}

// ---------------------------------------------------------------------------
// FastH3 DiT emission plan
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum EmitKind {
    /// 1-D F32 vector read from one canonical tensor. `swap_halves`
    /// exchanges the two equal halves (the gated fc1 bias convention).
    VecF32 { canonical: String, len: usize, swap_halves: bool },
    /// Raw BF16 `n x k` matrix (byte passthrough for bf16 sources).
    MatBf16 { canonical: String, n: usize, k: usize },
    /// Q4_K `n x k` matrix. `swap_halves` exchanges the two row halves on
    /// the way OUT (the loader exchanges them back).
    MatQ4K { canonical: String, n: usize, k: usize, swap_halves: bool },
    /// Fused QKV: three canonical `n x k` matrices concatenated `[q|k|v]`.
    QkvQ4K { parts: [String; 3], n: usize, k: usize },
    /// One adaln linear folded through the rank basis: `n x 2688 -> n x 8`.
    AdalnFoldBf16 { canonical: String, n: usize },
    /// The F32 `[rank, grid]` curve table.
    AdalnTable,
}

#[derive(Clone, Debug)]
struct EmitEntry {
    gguf_name: String,
    kind: EmitKind,
}

fn dit_emission_plan() -> Vec<EmitEntry> {
    const INNER: usize = H3_HEAD_COUNT * H3_HEAD_DIM;
    const ADALN_COLS: usize = H3_MODALITY_NUM * 6 * H3_HIDDEN_SIZE;
    let mut plan = Vec::new();
    let mut push = |gguf_name: &str, kind: EmitKind| {
        plan.push(EmitEntry {
            gguf_name: gguf_name.to_string(),
            kind,
        });
    };
    let vec_f32 = |canonical: &str, len: usize| EmitKind::VecF32 {
        canonical: canonical.to_string(),
        len,
        swap_halves: false,
    };
    let mat_bf16 = |canonical: &str, n: usize, k: usize| EmitKind::MatBf16 {
        canonical: canonical.to_string(),
        n,
        k,
    };

    push("adaln_t_table", EmitKind::AdalnTable);
    push("video_patch_proj.weight", mat_bf16("proj_in.weight", H3_HIDDEN_SIZE, H3_VIDEO_PATCH_DIM));
    push("video_patch_proj.bias", vec_f32("proj_in.bias", H3_HIDDEN_SIZE));
    push(
        "audio_patch_proj.weight",
        mat_bf16("audio_proj_in.weight", H3_HIDDEN_SIZE, H3_AUDIO_IN_CHANNELS),
    );
    push("audio_patch_proj.bias", vec_f32("audio_proj_in.bias", H3_HIDDEN_SIZE));
    push(
        "condition_proj.weight",
        mat_bf16("context_embedder.weight", H3_HIDDEN_SIZE, H3_TEXT_DIM),
    );
    push("condition_proj.bias", vec_f32("context_embedder.bias", H3_HIDDEN_SIZE));
    push("final_layer.norm.weight", vec_f32("norm_out.norm.weight", H3_HIDDEN_SIZE));
    push(
        "final_layer.adaln_proj.linear.weight",
        EmitKind::AdalnFoldBf16 {
            canonical: "norm_out.linear.weight".to_string(),
            n: 2 * H3_HIDDEN_SIZE,
        },
    );
    push(
        "final_layer.adaln_proj.linear.bias",
        vec_f32("norm_out.linear.bias", 2 * H3_HIDDEN_SIZE),
    );
    push(
        "final_layer.video_out.weight",
        mat_bf16("proj_out.weight", H3_VIDEO_PATCH_DIM, H3_HIDDEN_SIZE),
    );
    push("final_layer.video_out.bias", vec_f32("proj_out.bias", H3_VIDEO_PATCH_DIM));
    push(
        "final_layer.audio_out.weight",
        mat_bf16("audio_proj_out.weight", H3_AUDIO_IN_CHANNELS, H3_HIDDEN_SIZE),
    );
    push(
        "final_layer.audio_out.bias",
        vec_f32("audio_proj_out.bias", H3_AUDIO_IN_CHANNELS),
    );
    push(
        "token_refiner.final_norm.weight",
        vec_f32("token_refiner.final_norm.weight", H3_HIDDEN_SIZE),
    );

    let mut block = |gguf_prefix: String, canonical_prefix: String, adaln: bool| {
        let mut push = |gguf_name: String, kind: EmitKind| {
            plan.push(EmitEntry { gguf_name, kind });
        };
        let g = |tail: &str| format!("{gguf_prefix}.{tail}");
        let c = |tail: &str| format!("{canonical_prefix}.{tail}");
        push(
            g("norm1.weight"),
            EmitKind::VecF32 { canonical: c("norm1.weight"), len: H3_HIDDEN_SIZE, swap_halves: false },
        );
        push(
            g("norm2.weight"),
            EmitKind::VecF32 { canonical: c("norm2.weight"), len: H3_HIDDEN_SIZE, swap_halves: false },
        );
        push(
            g("attn.q_norm.weight"),
            EmitKind::VecF32 { canonical: c("attn.norm_q.weight"), len: H3_HEAD_DIM, swap_halves: false },
        );
        push(
            g("attn.k_norm.weight"),
            EmitKind::VecF32 { canonical: c("attn.norm_k.weight"), len: H3_HEAD_DIM, swap_halves: false },
        );
        push(
            g("attn.qkv_proj.weight"),
            EmitKind::QkvQ4K {
                parts: [
                    c("attn.to_q.weight"),
                    c("attn.to_k.weight"),
                    c("attn.to_v.weight"),
                ],
                n: INNER,
                k: H3_HIDDEN_SIZE,
            },
        );
        push(
            g("attn.out_proj.weight"),
            EmitKind::MatQ4K {
                canonical: c("attn.to_out.0.weight"),
                n: H3_HIDDEN_SIZE,
                k: INNER,
                swap_halves: false,
            },
        );
        push(
            g("mlp.fc1.weight"),
            EmitKind::MatQ4K {
                canonical: c("ff.net.0.proj.weight"),
                n: 2 * H3_FFN_DIM,
                k: H3_HIDDEN_SIZE,
                swap_halves: true,
            },
        );
        push(
            g("mlp.fc2.weight"),
            EmitKind::MatQ4K {
                canonical: c("ff.net.2.weight"),
                n: H3_HIDDEN_SIZE,
                k: H3_FFN_DIM,
                swap_halves: false,
            },
        );
        if adaln {
            push(
                g("adaln_proj.linear.weight"),
                EmitKind::AdalnFoldBf16 { canonical: c("adaln_proj.linear.weight"), n: ADALN_COLS },
            );
            push(
                g("adaln_proj.linear.bias"),
                EmitKind::VecF32 { canonical: c("adaln_proj.linear.bias"), len: ADALN_COLS, swap_halves: false },
            );
        }
    };
    for layer in 0..H3_DEPTH {
        block(
            format!("blocks.{layer}"),
            format!("transformer_blocks.{layer}"),
            true,
        );
    }
    for layer in 0..H3_REFINER_DEPTH {
        block(
            format!("token_refiner.blocks.{layer}"),
            format!("token_refiner.refiner_blocks.{layer}"),
            false,
        );
    }
    plan
}

fn emit_decl(entry: &EmitEntry) -> GgufTensorDecl {
    let (dims, ggml_type) = match &entry.kind {
        EmitKind::VecF32 { len, .. } => (vec![*len as u64], GGML_TYPE_F32),
        EmitKind::MatBf16 { n, k, .. } => (vec![*k as u64, *n as u64], GGML_TYPE_BF16),
        EmitKind::MatQ4K { n, k, .. } => (vec![*k as u64, *n as u64], GGML_TYPE_Q4_K),
        EmitKind::QkvQ4K { n, k, .. } => (vec![*k as u64, (3 * *n) as u64], GGML_TYPE_Q4_K),
        EmitKind::AdalnFoldBf16 { n, .. } => (vec![ADALN_RANK as u64, *n as u64], GGML_TYPE_BF16),
        EmitKind::AdalnTable => (vec![ADALN_RANK as u64, ADALN_GRID as u64], GGML_TYPE_F32),
    };
    GgufTensorDecl {
        name: entry.gguf_name.clone(),
        dims,
        ggml_type,
    }
}

fn emit_source_names(entry: &EmitKind) -> Vec<&str> {
    match entry {
        EmitKind::VecF32 { canonical, .. }
        | EmitKind::MatBf16 { canonical, .. }
        | EmitKind::MatQ4K { canonical, .. }
        | EmitKind::AdalnFoldBf16 { canonical, .. } => vec![canonical.as_str()],
        EmitKind::QkvQ4K { parts, .. } => parts.iter().map(|s| s.as_str()).collect(),
        EmitKind::AdalnTable => Vec::new(),
    }
}

fn require_shape(src: &H3ShardedWeights, name: &str, expected: &[u64]) -> Result<()> {
    let (_dtype, shape) = src.tensor_dtype_shape(name)?;
    if shape != expected {
        return Err(model_error(format!(
            "source tensor '{name}' has shape {shape:?}, expected {expected:?}"
        )));
    }
    Ok(())
}

fn source_f32(src: &H3ShardedWeights, name: &str) -> Result<Vec<f32>> {
    src.tensor_f32(name)
}

/// Raw BF16 payload of an `n x k` source matrix: byte passthrough for bf16
/// sources, RNE conversion for f32/f16 sources.
fn source_bf16_bytes(src: &H3ShardedWeights, name: &str, n: usize, k: usize) -> Result<Vec<u8>> {
    let (dtype, _shape) = src.tensor_dtype_shape(name)?;
    match dtype {
        MlxDType::BF16 => {
            let bytes = src.tensor_bytes(name)?;
            if bytes.len() != n * k * 2 {
                return Err(model_error(format!(
                    "source '{name}' bf16 payload {} bytes, expected {}",
                    bytes.len(),
                    n * k * 2
                )));
            }
            Ok(bytes)
        }
        _ => {
            let values = src.tensor_f32(name)?;
            if values.len() != n * k {
                return Err(model_error(format!(
                    "source '{name}' has {} values, expected {}",
                    values.len(),
                    n * k
                )));
            }
            let mut out = Vec::with_capacity(values.len() * 2);
            for value in values {
                out.extend_from_slice(&f32_to_bf16_rn(value).to_le_bytes());
            }
            Ok(out)
        }
    }
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn bf16_bytes_from_f32(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for value in values {
        out.extend_from_slice(&f32_to_bf16_rn(*value).to_le_bytes());
    }
    out
}

#[derive(Clone, Debug)]
pub struct AdalnFitReport {
    pub rank: usize,
    pub grid: usize,
    pub rel_l2: f64,
    pub max_row_rel: f64,
    pub dmd_max_rel: f64,
}

#[derive(Clone, Debug)]
pub struct FastH3QuantReport {
    pub out_path: PathBuf,
    pub tensor_count: usize,
    pub q4k_tensors: usize,
    pub bf16_tensors: usize,
    pub f32_tensors: usize,
    pub total_bytes: u64,
    pub adaln: AdalnFitReport,
    /// Per-Q4_K-tensor sampled relative RMS error, worst first (top 8).
    pub worst_q4k_rel_rmse: Vec<(String, f64)>,
    /// Mean sampled relative RMS error over all Q4_K tensors.
    pub mean_q4k_rel_rmse: f64,
    /// Source tensors deliberately not carried over.
    pub skipped_sources: Vec<String>,
}

pub struct FastH3QuantOptions {
    pub threads: usize,
    /// Free-form provenance string stored under `makepad.h3.source`.
    pub source_label: String,
    /// Sampling stride for the per-tensor quantization error report
    /// (1 = every row; larger = cheaper).
    pub error_sample_stride: usize,
}

impl Default for FastH3QuantOptions {
    fn default() -> Self {
        Self {
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8),
            source_label: String::new(),
            error_sample_stride: 97,
        }
    }
}

/// True for source tensors the quantized tier never carries: the VSA
/// compression gates (dense attention never reads them), the time_embedder
/// MLP (replaced by the curve) and the analytic rope table.
fn skipped_source(name: &str) -> bool {
    name.contains(".attn.to_gate_compress.")
        || name.starts_with("time_embedder.")
        || name == "rope.inv_freq"
}

/// Quantize a canonical bf16/f32 DiT shard set into the pruned-Q4_K GGUF
/// the `h3_quant` loader serves for the 24GB tiers.
pub fn write_fasth3_dit_q4_gguf(
    src: &H3ShardedWeights,
    out_path: &Path,
    options: &FastH3QuantOptions,
    progress: &mut dyn FnMut(&str),
) -> Result<FastH3QuantReport> {
    let threads = options.threads.max(1);
    let plan = dit_emission_plan();

    // ---- Fail-closed inventory check before any compute ----------------
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    for entry in &plan {
        for name in emit_source_names(&entry.kind) {
            consumed.insert(name.to_string());
        }
    }
    for name in [
        "time_embedder.linear_1.weight",
        "time_embedder.linear_1.bias",
        "time_embedder.linear_2.weight",
        "time_embedder.linear_2.bias",
    ] {
        consumed.insert(name.to_string());
        if !src.has_tensor(name) {
            return Err(model_error(format!(
                "source is missing '{name}' — the AdaLN curve needs the full time_embedder MLP"
            )));
        }
    }
    let mut skipped_sources = Vec::new();
    let mut unaccounted = Vec::new();
    for name in src.tensor_names() {
        if consumed.contains(&name) {
            continue;
        }
        if skipped_source(&name) {
            skipped_sources.push(name);
        } else {
            unaccounted.push(name);
        }
    }
    skipped_sources.sort();
    if !unaccounted.is_empty() {
        unaccounted.sort();
        return Err(model_error(format!(
            "source holds {} tensors the emission plan does not account for (refusing to \
             silently drop weights): {}",
            unaccounted.len(),
            unaccounted.join(", ")
        )));
    }
    // Validate every source shape up front.
    for entry in &plan {
        match &entry.kind {
            EmitKind::VecF32 { canonical, len, .. } => {
                require_shape(src, canonical, &[*len as u64])?
            }
            EmitKind::MatBf16 { canonical, n, k } => {
                require_shape(src, canonical, &[*n as u64, *k as u64])?
            }
            EmitKind::MatQ4K { canonical, n, k, .. } => {
                require_shape(src, canonical, &[*n as u64, *k as u64])?
            }
            EmitKind::QkvQ4K { parts, n, k } => {
                for part in parts {
                    require_shape(src, part, &[*n as u64, *k as u64])?;
                }
            }
            EmitKind::AdalnFoldBf16 { canonical, n } => {
                require_shape(src, canonical, &[*n as u64, H3_TIME_EMBED_DIM as u64])?
            }
            EmitKind::AdalnTable => {}
        }
    }
    require_shape(
        src,
        "time_embedder.linear_1.weight",
        &[H3_TIME_EMBED_HIDDEN as u64, H3_FREQ_DIM as u64],
    )?;
    require_shape(src, "time_embedder.linear_1.bias", &[H3_TIME_EMBED_HIDDEN as u64])?;
    require_shape(
        src,
        "time_embedder.linear_2.weight",
        &[H3_TIME_EMBED_DIM as u64, H3_TIME_EMBED_HIDDEN as u64],
    )?;
    require_shape(src, "time_embedder.linear_2.bias", &[H3_TIME_EMBED_DIM as u64])?;

    // ---- AdaLN curve ----------------------------------------------------
    progress("adaln: evaluating silu(time_embedder) over the 1025-point grid");
    let l1_w = source_f32(src, "time_embedder.linear_1.weight")?;
    let l1_b = source_f32(src, "time_embedder.linear_1.bias")?;
    let l2_w = source_f32(src, "time_embedder.linear_2.weight")?;
    let l2_b = source_f32(src, "time_embedder.linear_2.bias")?;
    let grid = silu_temb_grid(&l1_w, &l1_b, &l2_w, &l2_b, threads);
    progress("adaln: rank-8 factorization");
    let fit = adaln_low_rank_fit(&grid, ADALN_GRID, H3_TIME_EMBED_DIM, ADALN_RANK)?;
    progress(&format!(
        "adaln: rel_l2 {:.3e}, max_row_rel {:.3e}, dmd_max_rel {:.3e}",
        fit.rel_l2, fit.max_row_rel, fit.dmd_max_rel
    ));
    // A rank-8 basis reconstructs the base-H3 family's curve to well under
    // a percent; a fit an order worse than that means the checkpoint's
    // time embedder does not fit the pruned tier contract.
    if fit.rel_l2 > 0.05 {
        return Err(model_error(format!(
            "adaln rank-{ADALN_RANK} fit rel_l2 {:.3e} exceeds the 5e-2 gate — refusing to \
             write a curve that misrepresents the time embedder",
            fit.rel_l2
        )));
    }

    // ---- Header ---------------------------------------------------------
    let decls: Vec<GgufTensorDecl> = plan.iter().map(emit_decl).collect();
    let kv = vec![
        (
            "general.architecture".to_string(),
            GgufWriteValue::Str("minimax-h3-fl2va-dit".to_string()),
        ),
        (
            "general.name".to_string(),
            GgufWriteValue::Str(
                "FastH3 DiT, AdaLN rank-8 pruned Q4_K (makepad h3_quant_gguf)".to_string(),
            ),
        ),
        (
            "makepad.h3.source".to_string(),
            GgufWriteValue::Str(options.source_label.clone()),
        ),
        ("makepad.h3.adaln.rank".to_string(), GgufWriteValue::U32(ADALN_RANK as u32)),
        ("makepad.h3.adaln.grid".to_string(), GgufWriteValue::U32(ADALN_GRID as u32)),
        (
            "makepad.h3.adaln.fit_rel_l2".to_string(),
            GgufWriteValue::F32(fit.rel_l2 as f32),
        ),
        (
            "makepad.h3.adaln.fit_dmd_max_rel".to_string(),
            GgufWriteValue::F32(fit.dmd_max_rel as f32),
        ),
    ];
    let mut writer = GgufWriter::create(out_path, &kv, decls)?;

    // ---- Stream tensors -------------------------------------------------
    let mut q4k_tensors = 0usize;
    let mut bf16_tensors = 0usize;
    let mut f32_tensors = 0usize;
    let mut total_bytes = 0u64;
    let mut q4k_errors: Vec<(String, f64)> = Vec::new();
    let total = plan.len();
    for (index, entry) in plan.iter().enumerate() {
        let bytes = match &entry.kind {
            EmitKind::AdalnTable => {
                f32_tensors += 1;
                f32_bytes(&fit.curve)
            }
            EmitKind::VecF32 { canonical, len, swap_halves } => {
                let mut values = source_f32(src, canonical)?;
                if values.len() != *len {
                    return Err(model_error(format!(
                        "'{canonical}' has {} values, expected {len}",
                        values.len()
                    )));
                }
                if *swap_halves {
                    values.rotate_left(len / 2);
                }
                f32_tensors += 1;
                f32_bytes(&values)
            }
            EmitKind::MatBf16 { canonical, n, k } => {
                bf16_tensors += 1;
                source_bf16_bytes(src, canonical, *n, *k)?
            }
            EmitKind::MatQ4K { canonical, n, k, swap_halves } => {
                let mut values = source_f32(src, canonical)?;
                if *swap_halves {
                    // canonical [value|gate] rows -> stored [gate|value].
                    values.rotate_left(*n / 2 * *k);
                }
                let payload = quantize_q4_k_matrix(&values, *n, *k, threads)?;
                let rel =
                    q4_k_rel_rmse(&payload, &values, *n, *k, options.error_sample_stride);
                q4k_errors.push((entry.gguf_name.clone(), rel));
                q4k_tensors += 1;
                payload
            }
            EmitKind::QkvQ4K { parts, n, k } => {
                let row_bytes = q4_k_row_bytes(*k);
                let mut payload = Vec::with_capacity(3 * *n * row_bytes);
                let mut rel_acc = 0.0f64;
                for part in parts {
                    let values = source_f32(src, part)?;
                    let part_payload = quantize_q4_k_matrix(&values, *n, *k, threads)?;
                    rel_acc += q4_k_rel_rmse(
                        &part_payload,
                        &values,
                        *n,
                        *k,
                        options.error_sample_stride,
                    );
                    payload.extend_from_slice(&part_payload);
                }
                q4k_errors.push((entry.gguf_name.clone(), rel_acc / 3.0));
                q4k_tensors += 1;
                payload
            }
            EmitKind::AdalnFoldBf16 { canonical, n } => {
                let values = source_f32(src, canonical)?;
                let folded = fold_adaln_weight(&values, *n, H3_TIME_EMBED_DIM, &fit, threads);
                bf16_tensors += 1;
                bf16_bytes_from_f32(&folded)
            }
        };
        total_bytes += bytes.len() as u64;
        writer.append_tensor(&entry.gguf_name, &bytes)?;
        if index % 25 == 0 || index + 1 == total {
            progress(&format!(
                "write {}/{}: {} ({} bytes)",
                index + 1,
                total,
                entry.gguf_name,
                bytes.len()
            ));
        }
    }
    writer.finish()?;

    let mean_q4k_rel_rmse = if q4k_errors.is_empty() {
        0.0
    } else {
        q4k_errors.iter().map(|(_, e)| e).sum::<f64>() / q4k_errors.len() as f64
    };
    q4k_errors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    q4k_errors.truncate(8);

    Ok(FastH3QuantReport {
        out_path: out_path.to_path_buf(),
        tensor_count: total,
        q4k_tensors,
        bf16_tensors,
        f32_tensors,
        total_bytes,
        adaln: AdalnFitReport {
            rank: fit.rank,
            grid: fit.grid,
            rel_l2: fit.rel_l2,
            max_row_rel: fit.max_row_rel,
            dmd_max_rel: fit.dmd_max_rel,
        },
        worst_q4k_rel_rmse: q4k_errors,
        mean_q4k_rel_rmse,
        skipped_sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_llm::GgufFile;

    fn scratch(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("h3_quant_writer_test_{}_{name}", std::process::id()));
        path
    }

    #[test]
    fn nearest_int_is_round_half_to_even() {
        assert_eq!(nearest_int(0.0), 0);
        assert_eq!(nearest_int(0.4), 0);
        assert_eq!(nearest_int(0.5), 0);
        assert_eq!(nearest_int(1.5), 2);
        assert_eq!(nearest_int(2.5), 2);
        assert_eq!(nearest_int(3.5), 4);
        assert_eq!(nearest_int(-0.5), 0);
        assert_eq!(nearest_int(-1.5), -2);
        assert_eq!(nearest_int(-2.5), -2);
        assert_eq!(nearest_int(7.49), 7);
        assert_eq!(nearest_int(-7.51), -8);
    }

    #[test]
    fn scale_min_packing_round_trips_through_the_decoder_layout() {
        // Every 6-bit (scale, min) pair must survive the 12-byte packing
        // exactly as `get_scale_min_k4` reads it back — pinned by encoding
        // a block engineered to exercise all 8 groups.
        let mut x = [0.0f32; QK_K];
        for j in 0..8 {
            for i in 0..32 {
                x[j * 32 + i] = (j as f32 + 1.0) * (i as f32 - 3.0);
            }
        }
        let mut block = [0u8; 144];
        quantize_q4_k_block(&x, &mut block);
        let packed = &block[4..16];
        for j in 0..8 {
            let (sc, m) = unpack_scale_min(j, packed);
            assert!(sc <= 63, "group {j} scale {sc}");
            assert!(m <= 63, "group {j} min {m}");
        }
    }

    #[test]
    fn q4_k_round_trip_error_is_small_and_zero_blocks_are_exact() {
        // Deterministic pseudo-random block.
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            ((seed.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        };
        let x: Vec<f32> = (0..QK_K).map(|_| next() * 2.0).collect();
        let mut block = [0u8; 144];
        quantize_q4_k_block(&x, &mut block);
        let mut recon = [0.0f32; QK_K];
        dequantize_q4_k(&block, &mut recon);
        let mut err2 = 0.0f64;
        let mut ref2 = 0.0f64;
        for (r, o) in recon.iter().zip(&x) {
            err2 += ((*r - *o) as f64).powi(2);
            ref2 += (*o as f64).powi(2);
        }
        let rel = (err2 / ref2).sqrt();
        assert!(rel < 0.06, "q4_k round-trip rel rmse {rel}");

        let zeros = [0.0f32; QK_K];
        quantize_q4_k_block(&zeros, &mut block);
        dequantize_q4_k(&block, &mut recon);
        assert!(recon.iter().all(|v| *v == 0.0));

        // A constant positive block reconstructs to within one f16 step.
        let constant = [0.75f32; QK_K];
        quantize_q4_k_block(&constant, &mut block);
        dequantize_q4_k(&block, &mut recon);
        for v in recon {
            assert!((v - 0.75).abs() < 1e-3, "constant block recon {v}");
        }
    }

    #[test]
    fn q4_k_matrix_matches_per_block_quantization_and_threads_are_exact() {
        let (n, k) = (7, QK_K * 2);
        let src: Vec<f32> = (0..n * k).map(|i| ((i * 37 % 101) as f32 - 50.0) / 13.0).collect();
        let single = quantize_q4_k_matrix(&src, n, k, 1).unwrap();
        let threaded = quantize_q4_k_matrix(&src, n, k, 4).unwrap();
        assert_eq!(single, threaded);
        assert_eq!(single.len(), n * q4_k_row_bytes(k));
        let mut expected = vec![0u8; Q4_K_BLOCK_BYTES];
        quantize_q4_k_block(&src[k..k + QK_K], &mut expected);
        assert_eq!(&single[q4_k_row_bytes(k)..q4_k_row_bytes(k) + Q4_K_BLOCK_BYTES], &expected[..]);
    }

    #[test]
    fn f32_to_bf16_rounds_to_nearest_even() {
        // 1.0 + 2^-8 is exactly halfway between two bf16 values; RNE keeps
        // the even mantissa (1.0), truncation would too — so also check a
        // value just above the halfway point rounds UP, which truncation
        // gets wrong.
        assert_eq!(f32_to_bf16_rn(1.0), 0x3f80);
        let just_above_half = f32::from_bits(0x3f80_8001);
        assert_eq!(f32_to_bf16_rn(just_above_half), 0x3f81);
        let halfway_odd = f32::from_bits(0x3f81_8000); // between 0x3f81 and 0x3f82
        assert_eq!(f32_to_bf16_rn(halfway_odd), 0x3f82, "ties to even");
        assert_eq!(f32_to_bf16_rn(-2.0), 0xc000);
    }

    #[test]
    fn gguf_container_round_trips_through_the_reader() {
        let path = scratch("container.gguf");
        let decls = vec![
            GgufTensorDecl {
                name: "vec".to_string(),
                dims: vec![7],
                ggml_type: GGML_TYPE_F32,
            },
            GgufTensorDecl {
                name: "mat_q4k".to_string(),
                dims: vec![QK_K as u64, 3],
                ggml_type: GGML_TYPE_Q4_K,
            },
            GgufTensorDecl {
                name: "mat_bf16".to_string(),
                dims: vec![5, 2],
                ggml_type: GGML_TYPE_BF16,
            },
        ];
        let kv = vec![
            ("test.key".to_string(), GgufWriteValue::Str("value".to_string())),
            ("test.num".to_string(), GgufWriteValue::U32(7)),
        ];
        let mut writer = GgufWriter::create(&path, &kv, decls).unwrap();
        let vec_payload = f32_bytes(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        writer.append_tensor("vec", &vec_payload).unwrap();
        let src: Vec<f32> = (0..3 * QK_K).map(|i| (i as f32).sin()).collect();
        let q4k_payload = quantize_q4_k_matrix(&src, 3, QK_K, 2).unwrap();
        writer.append_tensor("mat_q4k", &q4k_payload).unwrap();
        let bf16_payload = bf16_bytes_from_f32(&[0.1f32; 10]);
        writer.append_tensor("mat_bf16", &bf16_payload).unwrap();
        writer.finish().unwrap();

        let file = GgufFile::open(&path).unwrap();
        assert_eq!(file.version, 3);
        assert_eq!(file.alignment, GGUF_ALIGNMENT);
        assert_eq!(file.tensors.len(), 3);
        assert_eq!(file.data_offset % GGUF_ALIGNMENT, 0);
        let vec_info = file.get_tensor("vec").unwrap();
        assert_eq!(vec_info.dimensions, vec![7]);
        assert_eq!(vec_info.size_bytes, 28);
        let q4k_info = file.get_tensor("mat_q4k").unwrap();
        assert_eq!(q4k_info.dimensions, vec![QK_K as u64, 3]);
        assert_eq!(q4k_info.offset % GGUF_ALIGNMENT, 0);
        assert_eq!(q4k_info.size_bytes as usize, q4k_payload.len());
        assert_eq!(file.read_tensor_bytes("vec").unwrap(), vec_payload);
        assert_eq!(file.read_tensor_bytes("mat_q4k").unwrap(), q4k_payload);
        assert_eq!(file.read_tensor_bytes("mat_bf16").unwrap(), bf16_payload);
        let value = file.get_value("test.key").unwrap().as_string().unwrap();
        assert_eq!(value.try_utf8().unwrap(), "value");
        assert_eq!(file.get_value("general.alignment").unwrap().as_u32(), Some(32));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn emission_plan_names_map_back_to_their_canonical_sources() {
        use crate::h3_quant::{dit_prefix_and_tail, gated_ffn_proj, map_dit_tail, top_level_dit_name};
        let plan = dit_emission_plan();
        assert_eq!(
            plan.len(),
            15 + H3_DEPTH * 10 + H3_REFINER_DEPTH * 8,
            "one entry per emitted tensor"
        );
        for entry in &plan {
            match &entry.kind {
                EmitKind::AdalnTable => {
                    assert_eq!(
                        top_level_dit_name(&entry.gguf_name).as_deref(),
                        Some("adaln_t_table")
                    );
                }
                EmitKind::QkvQ4K { parts, .. } => {
                    // The loader splits `attn.qkv_proj.weight` into exactly
                    // these three canonical names, in this row order.
                    let (prefix, tail) = dit_prefix_and_tail(&entry.gguf_name).unwrap();
                    assert_eq!(tail, "attn.qkv_proj.weight");
                    for (part, suffix) in ["to_q", "to_k", "to_v"].iter().enumerate() {
                        assert_eq!(parts[part], format!("{prefix}.attn.{suffix}.weight"));
                    }
                }
                EmitKind::VecF32 { canonical, .. }
                | EmitKind::MatBf16 { canonical, .. }
                | EmitKind::MatQ4K { canonical, .. }
                | EmitKind::AdalnFoldBf16 { canonical, .. } => {
                    let mapped = match dit_prefix_and_tail(&entry.gguf_name) {
                        Some((prefix, tail)) => {
                            // The gated fc1 halves swap exactly when the
                            // loader will swap them back.
                            let expects_swap = gated_ffn_proj(tail);
                            let plan_swaps = matches!(
                                &entry.kind,
                                EmitKind::VecF32 { swap_halves: true, .. }
                                    | EmitKind::MatQ4K { swap_halves: true, .. }
                            );
                            assert_eq!(
                                expects_swap, plan_swaps,
                                "{}: swap flag must mirror the loader",
                                entry.gguf_name
                            );
                            map_dit_tail(&prefix, tail)
                        }
                        None => top_level_dit_name(&entry.gguf_name).unwrap_or_else(|| {
                            panic!("{} does not map to a canonical name", entry.gguf_name)
                        }),
                    };
                    assert_eq!(&mapped, canonical, "{} maps wrong", entry.gguf_name);
                }
            }
        }
    }

    #[test]
    fn adaln_fit_recovers_an_exact_low_rank_curve_and_folds_consistently() {
        // Synthetic silu-temb: exactly rank 5 over a 33-point grid, dim 24.
        let (grid, dim, rank) = (33, 24, 5);
        let mut m = vec![0.0f32; grid * dim];
        for i in 0..grid {
            let t = i as f64 / (grid - 1) as f64;
            for k in 0..dim {
                let mut sum = 0.0f64;
                for r in 0..rank {
                    let coefficient = (t * (r + 1) as f64 * 1.7 + 0.3).sin();
                    let basis = ((k * (r + 2)) as f64 * 0.13).cos();
                    sum += coefficient * basis;
                }
                m[i * dim + k] = sum as f32;
            }
        }
        let fit = adaln_low_rank_fit(&m, grid, dim, 8).unwrap();
        assert!(fit.rel_l2 < 1e-6, "exact low-rank input must fit: {}", fit.rel_l2);
        assert_eq!(fit.curve.len(), grid * 8);

        // Folding: W * silu_temb(t) == W_fold * curve(t) for every grid row.
        let n = 6;
        let w: Vec<f32> = (0..n * dim).map(|i| ((i * 31 % 17) as f32 - 8.0) / 5.0).collect();
        let folded = fold_adaln_weight(&w, n, dim, &fit, 2);
        for i in [0usize, grid / 2, grid - 1] {
            for j in 0..n {
                let mut direct = 0.0f64;
                for k in 0..dim {
                    direct += w[j * dim + k] as f64 * m[i * dim + k] as f64;
                }
                let mut via_curve = 0.0f64;
                for r in 0..8 {
                    via_curve += folded[j * 8 + r] as f64 * fit.curve[i * 8 + r] as f64;
                }
                assert!(
                    (direct - via_curve).abs() < 1e-3 * (1.0 + direct.abs()),
                    "row {i} out {j}: {direct} vs {via_curve}"
                );
            }
        }
    }
}
