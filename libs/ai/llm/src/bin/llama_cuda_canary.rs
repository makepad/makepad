//! On-device validation + benchmark harness for the CUDA llama executor.
//!
//! Subcommands:
//!   opcheck                      — per-op numerical gates vs CPU references
//!                                  (runs every kernel through the exact
//!                                  planner/dispatch path the session uses)
//!   opcheck-q4k-mmq              — focused forced packed-Q4_K MMQ oracle
//!   generate <gguf> [...]        — full-model run with truthful metrics
//!   bench <gguf> [...]           — hardened protocol: 1 discarded warm-up,
//!                                  N measured repeats, median/range, exact
//!                                  token counts, VRAM + growth per repeat
//!
//! Correctness ordering: `opcheck` must be green before any full-model
//! number is quoted; `bench` prints greedy token ids so the wrapper can
//! verify stream equivalence against the llama.cpp reference.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use makepad_ai_llm::{
    CudaExecRuntime, ExecBackendKind, ExecRuntime, LlamaModel, LlamaSession, LlamaSessionConfig,
};
use makepad_ai_llm::cuda_exec::{host_split_reset, host_split_snapshot};
use makepad_ggml::{
    quant, BufferUsage, Context, GluOp, Graph, InitParams, TensorId, TensorType, UnaryOp,
    GGML_ROPE_TYPE_IMROPE,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("opcheck") => opcheck(),
        Some("opcheck-q4k-mmq") => opcheck_q4k_mmq(),
        Some("opcheck-q6k-mmq") => opcheck_q6k_mmq(),
        Some("opcheck-q5k-mmq") => opcheck_q5k_mmq(),
        Some("generate") => generate(&args[2..]),
        Some("bench") => bench(&args[2..]),
        _ => {
            eprintln!(
                "usage: llama-cuda-canary <opcheck|opcheck-q4k-mmq|opcheck-q5k-mmq|opcheck-q6k-mmq|generate|bench> ..."
            );
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// Deterministic data
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f32(&mut self) -> f32 {
        // Roughly [-1, 1)
        ((self.next_u64() >> 40) as f32 / (1u64 << 23) as f32) * 2.0 - 1.0
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 32) as u8
    }
}

fn f32s(rng: &mut Rng, n: usize) -> Vec<f32> {
    (0..n).map(|_| rng.f32()).collect()
}

fn as_bytes_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn as_bytes_i32(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn f16_bytes(v: &[f32]) -> Vec<u8> {
    v.iter()
        .flat_map(|&x| quant::f32_to_f16(x).to_le_bytes())
        .collect()
}

fn f16_round(x: f32) -> f32 {
    quant::f16_to_f32(quant::f32_to_f16(x))
}

/// Round-to-nearest-even f32 -> bf16 -> f32 (mirrors the GEMM slab path).
fn bf16_round(x: f32) -> f32 {
    let bits = x.to_bits();
    let rounded = (bits.wrapping_add(0x7FFF + ((bits >> 16) & 1))) & 0xFFFF_0000;
    f32::from_bits(rounded)
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn bytes_f16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|c| quant::f16_to_f32(u16::from_le_bytes(c.try_into().unwrap())))
        .collect()
}

/// IEEE-754 binary32 -> binary16 round-to-nearest, ties-to-even.
///
/// This intentionally lives in the MMQ oracle. The general quant helper is
/// suitable for the existing file encoders, but the CUDA DS4 path uses
/// `__float2half_rn` and the oracle must agree at every halfway case.
fn f32_to_f16_rne_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x007f_ffff;

    if exp == 0xff {
        if mantissa == 0 {
            return sign | 0x7c00;
        }
        return sign | 0x7c00 | ((mantissa >> 13) as u16).max(1);
    }
    // Every binary32 subnormal is too small to survive as binary16.
    if exp == 0 {
        return sign;
    }

    let unbiased = exp - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        let mut half_exp = (unbiased + 15) as u16;
        let mut half_mantissa = (mantissa >> 13) as u16;
        let remainder = mantissa & 0x1fff;
        if remainder > 0x1000 || (remainder == 0x1000 && (half_mantissa & 1) != 0) {
            half_mantissa += 1;
            if half_mantissa == 0x0400 {
                half_mantissa = 0;
                half_exp += 1;
                if half_exp >= 0x1f {
                    return sign | 0x7c00;
                }
            }
        }
        return sign | (half_exp << 10) | half_mantissa;
    }

    // Half subnormal. At exponent -25, exactly 2^-25 is a tie to zero;
    // larger mantissas round to the least binary16 subnormal.
    if unbiased < -25 {
        return sign;
    }
    let significand = 0x0080_0000 | mantissa;
    let shift = (-14 - unbiased + 13) as u32;
    let mut rounded = significand >> shift;
    let remainder_mask = (1u32 << shift) - 1;
    let remainder = significand & remainder_mask;
    let halfway = 1u32 << (shift - 1);
    if remainder > halfway || (remainder == halfway && (rounded & 1) != 0) {
        rounded += 1;
    }
    sign | rounded as u16
}

fn f16_rne(value: f32) -> f32 {
    quant::f16_to_f32(f32_to_f16_rne_bits(value))
}

struct ScopedEnv {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ScopedEnv {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

#[derive(Clone)]
struct Ds4Group {
    q: [i8; 32],
    d: f32,
    sum: f32,
}

/// DS4's stored sum is the lane-0 result of an eight-lane CUDA XOR
/// butterfly (offsets 4, 2, 1), not a linear CPU reduction.
fn ds4_butterfly_sum(values: &[f32]) -> f32 {
    debug_assert_eq!(values.len(), 32);
    let mut partial = [0.0f32; 8];
    for lane in 0..8 {
        let i = lane * 4;
        partial[lane] = ((values[i] + values[i + 1]) + values[i + 2]) + values[i + 3];
    }
    let lane_0_after_4 = partial[0] + partial[4];
    let lane_2_after_4 = partial[2] + partial[6];
    let lane_1_after_4 = partial[1] + partial[5];
    let lane_3_after_4 = partial[3] + partial[7];
    let lane_0_after_2 = lane_0_after_4 + lane_2_after_4;
    let lane_1_after_2 = lane_1_after_4 + lane_3_after_4;
    lane_0_after_2 + lane_1_after_2
}

fn ds4_group(values: &[f32]) -> Ds4Group {
    debug_assert_eq!(values.len(), 32);
    let amax = values.iter().fold(0.0f32, |acc, value| acc.max(value.abs()));
    let d_inv = if amax > 0.0 { 127.0f32 / amax } else { 0.0 };
    let d = if d_inv > 0.0 { 1.0f32 / d_inv } else { 0.0 };
    let mut q = [0i8; 32];
    for (dst, value) in q.iter_mut().zip(values) {
        // CUDA roundf rounds halfway cases away from zero.
        *dst = (value * d_inv).round() as i8;
    }
    Ds4Group {
        q,
        d: f16_rne(d),
        sum: f16_rne(ds4_butterfly_sum(values)),
    }
}

fn q4k_scale_min(block: &[u8], group: usize) -> (u8, u8) {
    debug_assert!(block.len() >= 144);
    debug_assert!(group < 8);
    let scales = &block[4..16];
    if group < 4 {
        (scales[group] & 63, scales[group + 4] & 63)
    } else {
        (
            (scales[group + 4] & 0x0f) | ((scales[group - 4] >> 6) << 4),
            (scales[group + 4] >> 4) | ((scales[group] >> 6) << 4),
        )
    }
}

fn q4k_value(block: &[u8], group: usize, lane: usize) -> i32 {
    debug_assert!(group < 8 && lane < 32);
    let packed = block[16 + (group / 2) * 32 + lane];
    if group & 1 == 0 {
        i32::from(packed & 0x0f)
    } else {
        i32::from(packed >> 4)
    }
}

/// Reference for b10430's Q4_K x Q8_1 MMQ contract. It deliberately does
/// not dequantize the activation: each 32-value group contributes an integer
/// q4*q8 dot scaled by half-rounded DS4/Q4 factors, followed by the exact
/// (half-stored) activation sum times the half-rounded Q4 minimum term.
fn q4k_mmq_reference(weights: &[u8], acts: &[f32], k: usize, n: usize, m: usize) -> Vec<f32> {
    assert_eq!(k % 256, 0);
    assert_eq!(weights.len(), n * (k / 256) * 144);
    assert_eq!(acts.len(), k * m);
    let groups_per_column = k / 32;
    let ds4 = acts
        .chunks_exact(32)
        .map(ds4_group)
        .collect::<Vec<_>>();
    assert_eq!(ds4.len(), m * groups_per_column);

    let row_bytes = (k / 256) * 144;
    let mut out = vec![0.0f32; n * m];
    for col in 0..m {
        for row in 0..n {
            let row_data = &weights[row * row_bytes..(row + 1) * row_bytes];
            let mut acc = 0.0f32;
            for block_index in 0..k / 256 {
                let block = &row_data[block_index * 144..(block_index + 1) * 144];
                let d = quant::f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
                let dmin = quant::f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
                for group in 0..8 {
                    let (scale, min) = q4k_scale_min(block, group);
                    let weight_scale = f16_rne(d * f32::from(scale));
                    let weight_min = f16_rne(-dmin * f32::from(min));
                    let act = &ds4[col * groups_per_column + block_index * 8 + group];
                    let mut dot = 0i32;
                    for lane in 0..32 {
                        dot += q4k_value(block, group, lane) * i32::from(act.q[lane]);
                    }
                    // NVCC -O3 contracts the final multiply/add in each
                    // source-level `+=`; keep the first scale multiply rounded
                    // and preserve the kernel's two contributions per group.
                    let dot_scale = weight_scale * act.d;
                    acc = dot_scale.mul_add(dot as f32, acc);
                    acc = weight_min.mul_add(act.sum, acc);
                }
            }
            out[col * n + row] = acc;
        }
    }
    out
}

fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn q4k_mmq_activations(k: usize, m: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; k * m];
    for col in 0..m {
        for i in 0..k {
            let group = i / 32;
            out[col * k + i] = if col == 0 || (col * 17 + group * 13) % 101 == 0 {
                // Whole zero column plus scattered zero groups exercise d=0.
                0.0
            } else if col == 1 {
                // Pairwise exact cancellation stresses the stored sum path.
                let magnitude = ((i / 2 * 13) % 29 + 1) as f32 * 0.03125;
                if i & 1 == 0 { magnitude } else { -magnitude }
            } else {
                let hash = mix64(((col as u64) << 32) ^ i as u64 ^ 0xa076_1d64_78bd_642f);
                let centered = ((hash >> 16) & 0x1ff) as i32 - 256;
                let scale = match (col + group) & 3 {
                    0 => 1.0 / 512.0,
                    1 => 1.0 / 256.0,
                    2 => 1.0 / 128.0,
                    _ => 1.0 / 64.0,
                };
                centered as f32 * scale
            };
        }
    }
    out
}

fn q4k_mmq_weights(k: usize, n: usize) -> Vec<u8> {
    let mut rng = Rng::new(0x514b_4d4d_51);
    let mut weights = quant_blocks(&mut rng, TensorType::Q4K, k, n);
    for (index, block) in weights.chunks_exact_mut(144).enumerate() {
        let d = 0.002f32 + (index % 19) as f32 * 0.00037;
        let dmin = 0.0007f32 + (index % 13) as f32 * 0.00019;
        block[0..2].copy_from_slice(&f32_to_f16_rne_bits(d).to_le_bytes());
        block[2..4].copy_from_slice(&f32_to_f16_rne_bits(dmin).to_le_bytes());
    }
    weights
}

#[derive(Clone, Copy)]
struct ErrorStats {
    max: f32,
    mean: f64,
    non_finite: usize,
}

fn error_stats(got: &[f32], want: &[f32]) -> ErrorStats {
    assert_eq!(got.len(), want.len());
    let mut max = 0.0f32;
    let mut sum = 0.0f64;
    let mut non_finite = 0usize;
    for (&got, &want) in got.iter().zip(want) {
        let error = (got - want).abs();
        if error.is_finite() {
            max = max.max(error);
            sum += f64::from(error);
        } else {
            non_finite += 1;
        }
    }
    ErrorStats {
        max,
        mean: sum / got.len().max(1) as f64,
        non_finite,
    }
}

/// Random-but-sane K-quant block stream: random payload bytes, controlled
/// finite f16 scale fields.
fn quant_blocks(rng: &mut Rng, ty: TensorType, k: usize, rows: usize) -> Vec<u8> {
    if ty == TensorType::Q8_0 {
        // 32-value/34-byte blocks: sane f16 scale + random int8 payload.
        let mut out = vec![0u8; rows * (k / 32) * 34];
        for block in out.chunks_exact_mut(34) {
            let d = quant::f32_to_f16(0.002 + 0.01 * (rng.byte() as f32 / 255.0));
            block[0..2].copy_from_slice(&d.to_le_bytes());
            for b in block[2..].iter_mut() {
                *b = rng.byte();
            }
        }
        return out;
    }
    let (block_bytes, d_off, dmin_off): (usize, usize, Option<usize>) = match ty {
        TensorType::Q4K => (144, 0, Some(2)),
        TensorType::Q5K => (176, 0, Some(2)),
        TensorType::Q6K => (210, 208, None),
        _ => unreachable!(),
    };
    let blocks_per_row = k / 256;
    let mut out = vec![0u8; rows * blocks_per_row * block_bytes];
    for block in out.chunks_exact_mut(block_bytes) {
        for b in block.iter_mut() {
            *b = rng.byte();
        }
        // Small positive f16 scales: exponent well inside range.
        let d = quant::f32_to_f16(0.002 + 0.01 * (rng.byte() as f32 / 255.0));
        block[d_off..d_off + 2].copy_from_slice(&d.to_le_bytes());
        if let Some(dmin_off) = dmin_off {
            let dmin = quant::f32_to_f16(0.001 + 0.005 * (rng.byte() as f32 / 255.0));
            block[dmin_off..dmin_off + 2].copy_from_slice(&dmin.to_le_bytes());
        }
    }
    out
}

fn dequant_row(ty: TensorType, row: &[u8], k: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(k);
    let mut tmp = [0.0f32; 256];
    match ty {
        TensorType::Q4K => {
            for block in row.chunks_exact(144) {
                quant::dequantize_q4_k(block, &mut tmp);
                out.extend_from_slice(&tmp);
            }
        }
        TensorType::Q6K => {
            for block in row.chunks_exact(210) {
                quant::dequantize_q6_k(block, &mut tmp);
                out.extend_from_slice(&tmp);
            }
        }
        TensorType::Q5K => {
            for block in row.chunks_exact(176) {
                dequantize_q5_k_block(block, &mut tmp);
                out.extend_from_slice(&tmp);
            }
        }
        TensorType::Q8_0 => {
            let mut small = [0.0f32; 32];
            for block in row.chunks_exact(34) {
                quant::dequantize_q8_0(block, &mut small);
                out.extend_from_slice(&small);
            }
        }
        _ => unreachable!(),
    }
    out
}

/// CPU q5_K reference (transcribed from libs/ggml quant.rs / upstream ggml).
fn dequantize_q5_k_block(block: &[u8], out: &mut [f32]) {
    fn scale_min(is: usize, scales: &[u8]) -> (u8, u8) {
        if is < 4 {
            (scales[is] & 63, scales[is + 4] & 63)
        } else {
            (
                (scales[is + 4] & 0x0F) | ((scales[is - 4] >> 6) << 4),
                (scales[is + 4] >> 4) | ((scales[is] >> 6) << 4),
            )
        }
    }
    let d = quant::f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
    let dmin = quant::f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
    let scales = &block[4..16];
    let qh = &block[16..48];
    let qs = &block[48..176];
    let mut is = 0usize;
    let mut u1 = 1u8;
    let mut u2 = 2u8;
    let mut ql_offset = 0usize;
    let mut idx = 0usize;
    for _ in 0..4 {
        let (sc1, m1) = scale_min(is, scales);
        let (sc2, m2) = scale_min(is + 1, scales);
        let d1 = d * sc1 as f32;
        let d2 = d * sc2 as f32;
        let m1 = dmin * m1 as f32;
        let m2 = dmin * m2 as f32;
        let ql = &qs[ql_offset..ql_offset + 32];
        for l in 0..32 {
            out[idx] =
                d1 * ((ql[l] & 0x0F) as f32 + if (qh[l] & u1) != 0 { 16.0 } else { 0.0 }) - m1;
            idx += 1;
        }
        for l in 0..32 {
            out[idx] =
                d2 * ((ql[l] >> 4) as f32 + if (qh[l] & u2) != 0 { 16.0 } else { 0.0 }) - m2;
            idx += 1;
        }
        is += 2;
        u1 <<= 2;
        u2 <<= 2;
        ql_offset += 32;
    }
}

// ---------------------------------------------------------------------------
// opcheck harness
// ---------------------------------------------------------------------------

/// Hybrid tolerance: an element fails when |g-w| > tol_abs + tol_rel*|w|.
/// Absolute-only cases pass tol_rel = 0; GEMM/accumulation cases carry a
/// relative term because bf16/f32 reduction noise scales with magnitude.
fn compare_tol(
    name: &'static str,
    got: Vec<f32>,
    want: Vec<f32>,
    tol_abs: f32,
    tol_rel: f32,
    failures: &mut usize,
) {
    if got.len() != want.len() {
        println!("FAIL {name}: length {} vs expected {}", got.len(), want.len());
        *failures += 1;
        return;
    }
    let mut worst = 0.0f32;
    let mut max_at = 0usize;
    let mut bad = false;
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        let d = (g - w).abs();
        let bound = tol_abs + tol_rel * w.abs();
        let score = d - bound;
        if !d.is_finite() || score > worst {
            worst = score;
            max_at = i;
            if !d.is_finite() || d > bound {
                bad = true;
            }
        }
    }
    if bad {
        println!(
            "FAIL {name}: over tolerance @ {max_at} (got {:.6}, want {:.6}, tol_abs {tol_abs}, tol_rel {tol_rel})",
            got[max_at], want[max_at]
        );
        *failures += 1;
    } else {
        let max_abs = got
            .iter()
            .zip(&want)
            .map(|(g, w)| (g - w).abs())
            .fold(0.0f32, f32::max);
        println!("ok   {name}: max_abs {max_abs:.7}");
    }
}

fn compare(name: &'static str, got: Vec<f32>, want: Vec<f32>, tol: f32, failures: &mut usize) {
    compare_tol(name, got, want, tol, 0.0, failures);
}

struct Bench {
    ctx: Context,
}

impl Bench {
    fn new(bytes: usize) -> Self {
        Self {
            ctx: Context::new(InitParams {
                mem_size: bytes,
                mem_buffer: None,
                no_alloc: false,
            }),
        }
    }

    fn tensor(&mut self, name: &str, ty: TensorType, dims: &[i64], bytes: &[u8]) -> TensorId {
        let id = self
            .ctx
            .new_named_tensor(name.to_string(), ty, dims.len(), dims, BufferUsage::Weights)
            .expect("tensor alloc");
        let dst = self.ctx.tensor_data_mut(id).expect("tensor data");
        dst[..bytes.len()].copy_from_slice(bytes);
        id
    }

    fn run(
        &mut self,
        runtime: &CudaExecRuntime,
        root: TensorId,
        wanted: &[TensorId],
    ) -> BTreeMap<TensorId, Vec<u8>> {
        let mut graph = Graph::new();
        graph
            .build_forward_expand(&self.ctx, root)
            .expect("graph build");
        runtime
            .execute_raw_graph(&self.ctx, &graph, wanted, &[], wanted)
            .expect("raw graph execution")
    }
}

fn run_q4k_mmq_case(
    exec: &CudaExecRuntime,
    weights: &[u8],
    acts: &[f32],
    k: usize,
    n: usize,
    m: usize,
    force: bool,
    profile: bool,
) -> Vec<f32> {
    let mut bench = Bench::new(64 << 20);
    let w = bench.tensor(
        "q4k_mmq_w",
        TensorType::Q4K,
        &[k as i64, n as i64],
        weights,
    );
    let x = bench.tensor(
        "q4k_mmq_x",
        TensorType::F32,
        &[k as i64, m as i64],
        &as_bytes_f32(acts),
    );
    let out = bench
        .ctx
        .mul_mat(w, x, BufferUsage::Activations)
        .expect("q4k MMQ mul_mat");
    let _force = ScopedEnv::set("MKLLM_FORCE_Q4K_MMQ", if force { "1" } else { "0" });
    let _disable = ScopedEnv::set("MKLLM_DISABLE_Q4K_MMQ", if force { "0" } else { "1" });
    let _profile = profile.then(|| ScopedEnv::set("MAKEPAD_LLAMA_CUDA_PROFILE", "1"));
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn q4k_mmq_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    if exec.features().compute_capability.0 < 8 {
        println!("SKIP q4k_mmq_forced_ds4: packed MMQ requires sm80+");
        return;
    }

    // N=257 crosses both the 128-row tile boundary and the second full tile;
    // M=512 is the deliberately narrow test gate and covers four J tiles.
    let (k, n, m) = (512usize, 257usize, 512usize);
    let weights = q4k_mmq_weights(k, n);
    let acts = q4k_mmq_activations(k, m);
    let want = q4k_mmq_reference(&weights, &acts, k, n, m);

    // Quant profiling prints q4k_ds4 + mmq_q4k_j128 when dispatch really
    // enters the candidate. The numerical proof below is machine-checked:
    // the forced result must match DS4 and differ from the known slab route.
    let forced = run_q4k_mmq_case(exec, &weights, &acts, k, n, m, true, true);
    let slab = run_q4k_mmq_case(exec, &weights, &acts, k, n, m, false, false);
    let forced_stats = error_stats(&forced, &want);
    let slab_stats = error_stats(&slab, &want);
    let route_delta = error_stats(&forced, &slab);

    compare_tol(
        "q4k_mmq_forced_ds4",
        forced.clone(),
        want.clone(),
        3e-3,
        2e-4,
        failures,
    );

    let zero_column_ok = forced[..n].iter().all(|value| value.abs() <= f32::EPSILON);
    let has_positive = forced[n..].iter().any(|value| *value > 1e-2);
    let has_negative = forced[n..].iter().any(|value| *value < -1e-2);
    if zero_column_ok && has_positive && has_negative {
        println!("ok   q4k_mmq_zero_sign: zero column and both output signs covered");
    } else {
        println!(
            "FAIL q4k_mmq_zero_sign: zero={zero_column_ok} positive={has_positive} negative={has_negative}"
        );
        *failures += 1;
    }

    let boundary_rows = [0usize, 127, 128, 255, 256];
    let boundary_cols = [0usize, 1, 127, 128, 255, 256, 511];
    let mut boundary_max = 0.0f32;
    let mut boundary_ok = true;
    for col in boundary_cols {
        for row in boundary_rows {
            let index = col * n + row;
            let error = (forced[index] - want[index]).abs();
            boundary_max = boundary_max.max(error);
            boundary_ok &= error <= 3e-3 + 2e-4 * want[index].abs();
        }
    }
    if boundary_ok {
        println!("ok   q4k_mmq_tile_tail: boundary max_abs {boundary_max:.7}");
    } else {
        println!("FAIL q4k_mmq_tile_tail: boundary max_abs {boundary_max:.7}");
        *failures += 1;
    }

    // If the env force were ignored, both executions would be the same slab
    // path. Also require the slab to be measurably farther from the DS4 oracle
    // so a nondeterministic repeated GEMM cannot masquerade as route proof.
    let route_ok = forced_stats.non_finite == 0
        && slab_stats.non_finite == 0
        && route_delta.non_finite == 0
        && route_delta.max > 1e-4
        && slab_stats.mean > forced_stats.mean * 2.0 + 1e-6;
    if route_ok {
        println!(
            "ok   q4k_mmq_dispatch: forced/slab max_delta {:.7}, DS4 mean forced {:.7} slab {:.7}",
            route_delta.max, forced_stats.mean, slab_stats.mean,
        );
    } else {
        println!(
            "FAIL q4k_mmq_dispatch: delta_max {:.7}, DS4 forced max/mean {:.7}/{:.7}, slab max/mean {:.7}/{:.7}, non_finite {}/{}/{}",
            route_delta.max,
            forced_stats.max,
            forced_stats.mean,
            slab_stats.max,
            slab_stats.mean,
            forced_stats.non_finite,
            slab_stats.non_finite,
            route_delta.non_finite,
        );
        *failures += 1;
    }

    // M=129 must remain on the accepted slab implementation even while the
    // force variable is set: this catches accidental widening of the J=128
    // host gate before a tail-capable kernel exists.
    let (gate_n, gate_m) = (129usize, 129usize);
    let row_bytes = (k / 256) * 144;
    let gate_weights = &weights[..gate_n * row_bytes];
    let gate_acts = &acts[..gate_m * k];
    let gate_forced = run_q4k_mmq_case(
        exec,
        gate_weights,
        gate_acts,
        k,
        gate_n,
        gate_m,
        true,
        false,
    );
    let gate_slab = run_q4k_mmq_case(
        exec,
        gate_weights,
        gate_acts,
        k,
        gate_n,
        gate_m,
        false,
        false,
    );
    let gate_delta = error_stats(&gate_forced, &gate_slab);
    if gate_delta.non_finite == 0 && gate_delta.max <= 1e-6 {
        println!("ok   q4k_mmq_m129_gate: forced env stayed on slab");
    } else {
        println!(
            "FAIL q4k_mmq_m129_gate: max_delta {:.7}, non_finite {}",
            gate_delta.max, gate_delta.non_finite,
        );
        *failures += 1;
    }
}

fn opcheck_q4k_mmq() -> i32 {
    let exec = match ExecRuntime::with_backend(ExecBackendKind::Cuda) {
        Ok(ExecRuntime::Cuda(runtime)) => runtime,
        Ok(_) => unreachable!(),
        Err(err) => {
            eprintln!("opcheck-q4k-mmq: CUDA runtime unavailable: {err:?}");
            return 3;
        }
    };
    println!("device: {}", exec.device_description());
    if exec.features().compute_capability.0 < 8 {
        eprintln!("opcheck-q4k-mmq: sm80+ device required");
        return 3;
    }
    let mut failures = 0usize;
    q4k_mmq_canary(&exec, &mut failures);
    println!(
        "\nopcheck-q4k-mmq: {} failures{}",
        failures,
        if failures == 0 { " — ALL GREEN" } else { "" }
    );
    if failures == 0 { 0 } else { 1 }
}

fn q6k_q_and_scale(block: &[u8], l: usize) -> (i32, i32) {
    let n = l >> 7;
    let r = l & 127;
    let ql = &block[n * 64..];
    let qh = &block[128 + n * 32..];
    let sc = &block[192 + n * 8..];
    let group = r >> 5;
    let lo = r & 31;
    let is = lo / 16;
    let q = match group {
        0 => i32::from(((ql[lo] & 0x0F) | ((qh[lo] & 3) << 4)) as i8) - 32,
        1 => i32::from(((ql[lo + 32] & 0x0F) | (((qh[lo] >> 2) & 3) << 4)) as i8) - 32,
        2 => i32::from(((ql[lo] >> 4) | (((qh[lo] >> 4) & 3) << 4)) as i8) - 32,
        _ => i32::from(((ql[lo + 32] >> 4) | (((qh[lo] >> 6) & 3) << 4)) as i8) - 32,
    };
    (q, i32::from(sc[is + 2 * group] as i8))
}

fn d4_group(values: &[f32]) -> (f32, [i8; 32]) {
    let amax = values.iter().fold(0.0f32, |acc, &x| acc.max(x.abs()));
    if amax == 0.0 {
        return (0.0, [0; 32]);
    }
    let d_inv = 127.0 / amax;
    let mut q = [0i8; 32];
    for (slot, &value) in q.iter_mut().zip(values.iter()) {
        *slot = (value * d_inv).round() as i8;
    }
    (1.0 / d_inv, q)
}

fn q6k_mmq_reference(weights: &[u8], acts: &[f32], k: usize, n: usize, m: usize) -> Vec<f32> {
    assert_eq!(k % 256, 0);
    assert_eq!(weights.len(), n * (k / 256) * 210);
    assert_eq!(acts.len(), k * m);
    let groups_per_column = k / 32;
    let d4 = acts
        .chunks_exact(32)
        .map(d4_group)
        .collect::<Vec<_>>();
    assert_eq!(d4.len(), m * groups_per_column);

    let row_bytes = (k / 256) * 210;
    let mut out = vec![0.0f32; n * m];
    for col in 0..m {
        for row in 0..n {
            let row_data = &weights[row * row_bytes..(row + 1) * row_bytes];
            let mut acc = 0.0f32;
            for block_index in 0..k / 256 {
                let block = &row_data[block_index * 210..(block_index + 1) * 210];
                let d = quant::f16_to_f32(u16::from_le_bytes([block[208], block[209]]));
                for group in 0..8 {
                    let (d_act, q_act) = &d4[col * groups_per_column + block_index * 8 + group];
                    let mut acc0 = 0i32;
                    let mut acc1 = 0i32;
                    let mut sc0 = 0i32;
                    let mut sc1 = 0i32;
                    for lane in 0..16 {
                        let (q0, s0) = q6k_q_and_scale(block, group * 32 + lane);
                        let (q1, s1) = q6k_q_and_scale(block, group * 32 + 16 + lane);
                        acc0 += q0 * i32::from(q_act[lane]);
                        acc1 += q1 * i32::from(q_act[16 + lane]);
                        sc0 = s0;
                        sc1 = s1;
                    }
                    acc += (acc0 * sc0 + acc1 * sc1) as f32 * *d_act * d;
                }
            }
            out[col * n + row] = acc;
        }
    }
    out
}

fn run_q6k_mmq_case(
    exec: &CudaExecRuntime,
    weights: &[u8],
    acts: &[f32],
    k: usize,
    n: usize,
    m: usize,
    force: bool,
    profile: bool,
) -> Vec<f32> {
    let mut bench = Bench::new(64 << 20);
    let w = bench.tensor(
        "q6k_mmq_w",
        TensorType::Q6K,
        &[k as i64, n as i64],
        weights,
    );
    let x = bench.tensor(
        "q6k_mmq_x",
        TensorType::F32,
        &[k as i64, m as i64],
        &as_bytes_f32(acts),
    );
    let out = bench
        .ctx
        .mul_mat(w, x, BufferUsage::Activations)
        .expect("q6k MMQ mul_mat");
    let _disable = ScopedEnv::set("MKLLM_DISABLE_Q6K_MMQ", if force { "0" } else { "1" });
    let _profile = profile.then(|| ScopedEnv::set("MAKEPAD_LLAMA_CUDA_PROFILE", "1"));
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn q6k_mmq_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    if exec.features().compute_capability.0 < 8 {
        println!("SKIP q6k_mmq_forced_d4: packed MMQ requires sm80+");
        return;
    }

    let (k, n, m) = (512usize, 257usize, 512usize);
    let mut rng = Rng::new(0x516b_4d4d_51);
    let weights = quant_blocks(&mut rng, TensorType::Q6K, k, n);
    let acts = q4k_mmq_activations(k, m);
    let want = q6k_mmq_reference(&weights, &acts, k, n, m);

    let forced = run_q6k_mmq_case(exec, &weights, &acts, k, n, m, true, true);
    let slab = run_q6k_mmq_case(exec, &weights, &acts, k, n, m, false, false);
    let forced_stats = error_stats(&forced, &want);
    let slab_stats = error_stats(&slab, &want);
    let route_delta = error_stats(&forced, &slab);

    compare_tol(
        "q6k_mmq_forced_d4",
        forced.clone(),
        want.clone(),
        3e-3,
        2e-4,
        failures,
    );

    let zero_column_ok = forced[..n].iter().all(|value| value.abs() <= f32::EPSILON);
    let has_positive = forced[n..].iter().any(|value| *value > 1e-2);
    let has_negative = forced[n..].iter().any(|value| *value < -1e-2);
    if zero_column_ok && has_positive && has_negative {
        println!("ok   q6k_mmq_zero_sign: zero column and both output signs covered");
    } else {
        println!(
            "FAIL q6k_mmq_zero_sign: zero={zero_column_ok} positive={has_positive} negative={has_negative}"
        );
        *failures += 1;
    }

    let boundary_rows = [0usize, 127, 128, 255, 256];
    let boundary_cols = [0usize, 1, 127, 128, 255, 256, 511];
    let mut boundary_max = 0.0f32;
    let mut boundary_ok = true;
    for col in boundary_cols {
        for row in boundary_rows {
            let index = col * n + row;
            let error = (forced[index] - want[index]).abs();
            boundary_max = boundary_max.max(error);
            boundary_ok &= error <= 3e-3 + 2e-4 * want[index].abs();
        }
    }
    if boundary_ok {
        println!("ok   q6k_mmq_tile_tail: boundary max_abs {boundary_max:.7}");
    } else {
        println!("FAIL q6k_mmq_tile_tail: boundary max_abs {boundary_max:.7}");
        *failures += 1;
    }

    let route_ok = forced_stats.non_finite == 0
        && slab_stats.non_finite == 0
        && route_delta.non_finite == 0
        && route_delta.max > 1e-4
        && slab_stats.mean > forced_stats.mean * 2.0 + 1e-6;
    if route_ok {
        println!(
            "ok   q6k_mmq_dispatch: forced/slab max_delta {:.7}, D4 mean forced {:.7} slab {:.7}",
            route_delta.max, forced_stats.mean, slab_stats.mean,
        );
    } else {
        println!(
            "FAIL q6k_mmq_dispatch: delta_max {:.7}, D4 forced max/mean {:.7}/{:.7}, slab max/mean {:.7}/{:.7}, non_finite {}/{}/{}",
            route_delta.max,
            forced_stats.max,
            forced_stats.mean,
            slab_stats.max,
            slab_stats.mean,
            forced_stats.non_finite,
            slab_stats.non_finite,
            route_delta.non_finite,
        );
        *failures += 1;
    }

    let (gate_n, gate_m) = (129usize, 129usize);
    let row_bytes = (k / 256) * 210;
    let gate_weights = &weights[..gate_n * row_bytes];
    let gate_acts = &acts[..gate_m * k];
    let gate_forced = run_q6k_mmq_case(
        exec,
        gate_weights,
        gate_acts,
        k,
        gate_n,
        gate_m,
        true,
        false,
    );
    let gate_slab = run_q6k_mmq_case(
        exec,
        gate_weights,
        gate_acts,
        k,
        gate_n,
        gate_m,
        false,
        false,
    );
    let gate_delta = error_stats(&gate_forced, &gate_slab);
    if gate_delta.non_finite == 0 && gate_delta.max <= 1e-6 {
        println!("ok   q6k_mmq_m129_gate: forced env stayed on slab");
    } else {
        println!(
            "FAIL q6k_mmq_m129_gate: max_delta {:.7}, non_finite {}",
            gate_delta.max, gate_delta.non_finite,
        );
        *failures += 1;
    }
}

fn q5k_value(block: &[u8], group: usize, lane: usize) -> i32 {
    let packed = block[48 + (group / 2) * 32 + lane];
    let nib = if group & 1 == 0 {
        i32::from(packed & 0x0f)
    } else {
        i32::from(packed >> 4)
    };
    let hi = if block[16 + lane] & (1 << group) != 0 {
        16
    } else {
        0
    };
    nib + hi
}

fn q5k_mmq_reference(weights: &[u8], acts: &[f32], k: usize, n: usize, m: usize) -> Vec<f32> {
    assert_eq!(k % 256, 0);
    assert_eq!(weights.len(), n * (k / 256) * 176);
    let groups_per_column = k / 32;
    let ds4 = acts.chunks_exact(32).map(ds4_group).collect::<Vec<_>>();
    let row_bytes = (k / 256) * 176;
    let mut out = vec![0.0f32; n * m];
    for col in 0..m {
        for row in 0..n {
            let row_data = &weights[row * row_bytes..(row + 1) * row_bytes];
            let mut acc = 0.0f32;
            for block_index in 0..k / 256 {
                let block = &row_data[block_index * 176..(block_index + 1) * 176];
                let d = quant::f16_to_f32(u16::from_le_bytes([block[0], block[1]]));
                let dmin = quant::f16_to_f32(u16::from_le_bytes([block[2], block[3]]));
                for group in 0..8 {
                    let (scale, min) = q4k_scale_min(block, group);
                    let weight_scale = f16_rne(d * f32::from(scale));
                    let weight_min = f16_rne(-dmin * f32::from(min));
                    let act = &ds4[col * groups_per_column + block_index * 8 + group];
                    let mut dot = 0i32;
                    for lane in 0..32 {
                        dot += q5k_value(block, group, lane) * i32::from(act.q[lane]);
                    }
                    acc = (weight_scale * act.d).mul_add(dot as f32, acc);
                    acc = weight_min.mul_add(act.sum, acc);
                }
            }
            out[col * n + row] = acc;
        }
    }
    out
}

fn run_q5k_mmq_case(
    exec: &CudaExecRuntime,
    weights: &[u8],
    acts: &[f32],
    k: usize,
    n: usize,
    m: usize,
    force: bool,
    profile: bool,
) -> Vec<f32> {
    let mut bench = Bench::new(64 << 20);
    let w = bench.tensor("q5k_mmq_w", TensorType::Q5K, &[k as i64, n as i64], weights);
    let x = bench.tensor(
        "q5k_mmq_x",
        TensorType::F32,
        &[k as i64, m as i64],
        &as_bytes_f32(acts),
    );
    let out = bench
        .ctx
        .mul_mat(w, x, BufferUsage::Activations)
        .expect("q5k MMQ mul_mat");
    let _disable = ScopedEnv::set("MKLLM_DISABLE_Q5K_MMQ", if force { "0" } else { "1" });
    let _profile = profile.then(|| ScopedEnv::set("MAKEPAD_LLAMA_CUDA_PROFILE", "1"));
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn q5k_mmq_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    if exec.features().compute_capability.0 < 8 {
        println!("SKIP q5k_mmq_forced_ds4: packed MMQ requires sm80+");
        return;
    }
    let (k, n, m) = (512usize, 257usize, 512usize);
    let mut rng = Rng::new(0x515b_4d4d_51);
    let weights = quant_blocks(&mut rng, TensorType::Q5K, k, n);
    let acts = q4k_mmq_activations(k, m);
    let want = q5k_mmq_reference(&weights, &acts, k, n, m);
    let forced = run_q5k_mmq_case(exec, &weights, &acts, k, n, m, true, true);
    let slab = run_q5k_mmq_case(exec, &weights, &acts, k, n, m, false, false);
    let forced_stats = error_stats(&forced, &want);
    let slab_stats = error_stats(&slab, &want);
    let route_delta = error_stats(&forced, &slab);
    compare_tol(
        "q5k_mmq_forced_ds4",
        forced.clone(),
        want,
        3e-3,
        2e-4,
        failures,
    );
    let route_ok = forced_stats.non_finite == 0
        && slab_stats.non_finite == 0
        && route_delta.max > 1e-4
        && slab_stats.mean > forced_stats.mean * 2.0 + 1e-6;
    if route_ok {
        println!(
            "ok   q5k_mmq_dispatch: forced/slab max_delta {:.7}, DS4 mean forced {:.7} slab {:.7}",
            route_delta.max, forced_stats.mean, slab_stats.mean,
        );
    } else {
        println!(
            "FAIL q5k_mmq_dispatch: delta_max {:.7} forced mean {:.7} slab mean {:.7}",
            route_delta.max, forced_stats.mean, slab_stats.mean,
        );
        *failures += 1;
    }
    let row_bytes = (k / 256) * 176;
    let gate_forced = run_q5k_mmq_case(
        exec,
        &weights[..129 * row_bytes],
        &acts[..129 * k],
        k,
        129,
        129,
        true,
        false,
    );
    let gate_slab = run_q5k_mmq_case(
        exec,
        &weights[..129 * row_bytes],
        &acts[..129 * k],
        k,
        129,
        129,
        false,
        false,
    );
    let gate_delta = error_stats(&gate_forced, &gate_slab);
    if gate_delta.non_finite == 0 && gate_delta.max <= 1e-6 {
        println!("ok   q5k_mmq_m129_gate: forced env stayed on slab");
    } else {
        println!("FAIL q5k_mmq_m129_gate: max_delta {:.7}", gate_delta.max);
        *failures += 1;
    }
}

fn opcheck_q5k_mmq() -> i32 {
    let exec = match ExecRuntime::with_backend(ExecBackendKind::Cuda) {
        Ok(ExecRuntime::Cuda(runtime)) => runtime,
        Ok(_) => unreachable!(),
        Err(err) => {
            eprintln!("opcheck-q5k-mmq: CUDA runtime unavailable: {err:?}");
            return 3;
        }
    };
    println!("device: {}", exec.device_description());
    if exec.features().compute_capability.0 < 8 {
        eprintln!("opcheck-q5k-mmq: sm80+ device required");
        return 3;
    }
    let mut failures = 0usize;
    q5k_mmq_canary(&exec, &mut failures);
    println!(
        "\nopcheck-q5k-mmq: {} failures{}",
        failures,
        if failures == 0 { " — ALL GREEN" } else { "" }
    );
    if failures == 0 { 0 } else { 1 }
}

fn opcheck_q6k_mmq() -> i32 {
    let exec = match ExecRuntime::with_backend(ExecBackendKind::Cuda) {
        Ok(ExecRuntime::Cuda(runtime)) => runtime,
        Ok(_) => unreachable!(),
        Err(err) => {
            eprintln!("opcheck-q6k-mmq: CUDA runtime unavailable: {err:?}");
            return 3;
        }
    };
    println!("device: {}", exec.device_description());
    if exec.features().compute_capability.0 < 8 {
        eprintln!("opcheck-q6k-mmq: sm80+ device required");
        return 3;
    }
    let mut failures = 0usize;
    q6k_mmq_canary(&exec, &mut failures);
    println!(
        "\nopcheck-q6k-mmq: {} failures{}",
        failures,
        if failures == 0 { " — ALL GREEN" } else { "" }
    );
    if failures == 0 { 0 } else { 1 }
}

fn run_mmvq_swiglu_case(
    exec: &CudaExecRuntime,
    gate: &[u8],
    up: &[u8],
    acts: &[f32],
    k: usize,
    n: usize,
    fuse: bool,
) -> Vec<f32> {
    let mut bench = Bench::new(32 << 20);
    let gw = bench.tensor("ffn_gate", TensorType::Q4K, &[k as i64, n as i64], gate);
    let uw = bench.tensor("ffn_up", TensorType::Q4K, &[k as i64, n as i64], up);
    let x = bench.tensor(
        "ffn_x",
        TensorType::F32,
        &[k as i64, 1],
        &as_bytes_f32(acts),
    );
    let g = bench
        .ctx
        .mul_mat(gw, x, BufferUsage::Activations)
        .expect("fused mmvq gate");
    let u = bench
        .ctx
        .mul_mat(uw, x, BufferUsage::Activations)
        .expect("fused mmvq up");
    let out = bench
        .ctx
        .glu_split(g, u, GluOp::Swiglu, BufferUsage::Activations)
        .expect("fused mmvq swiglu");
    let _disable = ScopedEnv::set("MKLLM_DISABLE_MMVQ_FUSION", if fuse { "0" } else { "1" });
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn mmvq_swiglu_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    let (k, n) = (512usize, 257usize);
    let mut rng = Rng::new(0xF51E);
    let gate = quant_blocks(&mut rng, TensorType::Q4K, k, n);
    let up = quant_blocks(&mut rng, TensorType::Q4K, k, n);
    let acts = f32s(&mut rng, k);
    let fused = run_mmvq_swiglu_case(exec, &gate, &up, &acts, k, n, true);
    let unfused = run_mmvq_swiglu_case(exec, &gate, &up, &acts, k, n, false);
    let delta = error_stats(&fused, &unfused);
    if delta.non_finite == 0 && delta.max <= 2e-4 {
        println!(
            "ok   mmvq_swiglu_fuse: fused/unfused max_delta {:.7}",
            delta.max
        );
    } else {
        *failures += 1;
        println!(
            "FAIL mmvq_swiglu_fuse: max_delta {:.7} non_finite {}",
            delta.max, delta.non_finite
        );
    }
}

fn run_cpy_set_rows_case(exec: &CudaExecRuntime, fuse: bool) -> Vec<f32> {
    let (prefix, channels) = (3usize, 64usize);
    let width = prefix * channels;
    let mut rng = Rng::new(0xC0F1);
    let cache_data = f32s(&mut rng, width);
    let qkv_data = f32s(&mut rng, channels);
    let idx = [0i32];
    let mut bench = Bench::new(4 << 20);
    let dest = bench.tensor(
        "r_slot",
        TensorType::F32,
        &[width as i64, 1],
        &as_bytes_f32(&vec![0.0f32; width]),
    );
    let cache_rows = bench.tensor(
        "r_cache_rows",
        TensorType::F32,
        &[width as i64, 1],
        &as_bytes_f32(&cache_data),
    );
    let conv_states = bench
        .ctx
        .view_3d(
            cache_rows,
            prefix as i64,
            channels as i64,
            1,
            prefix * size_of::<f32>(),
            width * size_of::<f32>(),
            0,
        )
        .expect("conv states view");
    let qkv = bench.tensor(
        "qkv",
        TensorType::F32,
        &[channels as i64, 1],
        &as_bytes_f32(&qkv_data),
    );
    let qkv_t = bench.ctx.transpose(qkv).expect("qkv transpose");
    let conv_input = bench
        .ctx
        .concat(conv_states, qkv_t, 0, BufferUsage::Activations)
        .expect("conv input concat");
    let conv_input_tensor = bench.ctx.tensor(conv_input).expect("conv input").clone();
    let last_states = bench
        .ctx
        .view_3d(
            conv_input,
            prefix as i64,
            channels as i64,
            1,
            conv_input_tensor.nb[1],
            conv_input_tensor.nb[2],
            size_of::<f32>(),
        )
        .expect("last convolution states view");
    let packed = bench
        .ctx
        .cont_2d(last_states, width as i64, 1)
        .expect("flatten last convolution states");
    let rows = bench.tensor("rows", TensorType::I32, &[1], &as_bytes_i32(&idx));
    let out = bench
        .ctx
        .set_rows(dest, packed, rows, BufferUsage::State)
        .expect("set_rows");
    let _disable = ScopedEnv::set("MKLLM_DISABLE_CPY_FUSION", if fuse { "0" } else { "1" });
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn cpy_set_rows_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    let fused = run_cpy_set_rows_case(exec, true);
    let unfused = run_cpy_set_rows_case(exec, false);
    let delta = error_stats(&fused, &unfused);
    if delta.non_finite == 0 && delta.max == 0.0 {
        println!("ok   cpy_set_rows_fuse: fused/unfused max_delta 0");
    } else {
        *failures += 1;
        println!(
            "FAIL cpy_set_rows_fuse: max_delta {:.7} non_finite {}",
            delta.max, delta.non_finite
        );
    }
}

fn run_cpy_ssm_state_case(exec: &CudaExecRuntime, fuse: bool) -> Vec<f32> {
    // llama.cpp qwen35.cpp:346: CPY new_state (4D view of the GDN pack)
    // into a 1D cache view. We still build SET_ROWS of that view.
    let head = 8usize;
    let n_heads = 4usize;
    let s_width = head * head * n_heads;
    let out_width = head * n_heads;
    let packed = out_width + s_width;
    let mut rng = Rng::new(0x5353_4D43);
    let packed_data = f32s(&mut rng, packed);
    let idx = [0i32];
    let mut bench = Bench::new(4 << 20);
    let dest = bench.tensor(
        "s_slot",
        TensorType::F32,
        &[s_width as i64, 1],
        &as_bytes_f32(&vec![0.0f32; s_width]),
    );
    let gdn = bench.tensor(
        "gdn_pack",
        TensorType::F32,
        &[packed as i64],
        &as_bytes_f32(&packed_data),
    );
    let new_state = bench
        .ctx
        .view_4d(
            gdn,
            head as i64,
            head as i64,
            n_heads as i64,
            1,
            head * size_of::<f32>(),
            head * head * size_of::<f32>(),
            s_width * size_of::<f32>(),
            out_width * size_of::<f32>(),
        )
        .expect("new_state view");
    let new_state_rows = bench
        .ctx
        .view_2d(
            new_state,
            s_width as i64,
            1,
            s_width * size_of::<f32>(),
            0,
        )
        .expect("new_state rows");
    let rows = bench.tensor("rows", TensorType::I32, &[1], &as_bytes_i32(&idx));
    let out = bench
        .ctx
        .set_rows(dest, new_state_rows, rows, BufferUsage::State)
        .expect("set_rows s-state");
    let _disable = ScopedEnv::set("MKLLM_DISABLE_CPY_FUSION", if fuse { "0" } else { "1" });
    let outputs = bench.run(exec, out, &[out]);
    bytes_to_f32(&outputs[&out])
}

fn cpy_ssm_state_canary(exec: &CudaExecRuntime, failures: &mut usize) {
    let fused = run_cpy_ssm_state_case(exec, true);
    let unfused = run_cpy_ssm_state_case(exec, false);
    let delta = error_stats(&fused, &unfused);
    if delta.non_finite == 0 && delta.max == 0.0 {
        println!("ok   cpy_ssm_state_fuse: fused/unfused max_delta 0");
    } else {
        *failures += 1;
        println!(
            "FAIL cpy_ssm_state_fuse: max_delta {:.7} non_finite {}",
            delta.max, delta.non_finite
        );
    }
}

#[allow(clippy::too_many_lines)]
fn opcheck() -> i32 {
    let exec = match ExecRuntime::with_backend(ExecBackendKind::Cuda) {
        Ok(ExecRuntime::Cuda(runtime)) => runtime,
        Ok(_) => unreachable!(),
        Err(err) => {
            eprintln!("opcheck: CUDA runtime unavailable: {err:?}");
            return 3;
        }
    };
    println!("device: {}", exec.device_description());
    let mut failures = 0usize;
    let mut rng = Rng::new(1234);

    // --- quantized mat-vec + GEMM, all three K-quants, small and large M
    for (ty, name_v, name_g) in [
        (TensorType::Q4K, "mmv_q4k", "gemm_q4k"),
        (TensorType::Q5K, "mmv_q5k", "gemm_q5k"),
        (TensorType::Q6K, "mmv_q6k", "gemm_q6k"),
        (TensorType::Q8_0, "mmv_q80", "gemm_q80"),
    ] {
        let (k, n) = (512usize, 33usize);
        let weights = quant_blocks(&mut rng, ty, k, n);
        let row_bytes = weights.len() / n;
        for (case_name, m, tol_abs, tol_rel) in [
            (name_v, 5usize, 1e-3f32, 1e-5f32),
            (name_g, 33usize, 5e-2f32, 1e-2f32),
        ] {
            let acts = f32s(&mut rng, k * m);
            let mut bench = Bench::new(64 << 20);
            let w = bench.tensor("w", ty, &[k as i64, n as i64], &weights);
            let x = bench.tensor("x", TensorType::F32, &[k as i64, m as i64], &as_bytes_f32(&acts));
            let out = bench
                .ctx
                .mul_mat(w, x, BufferUsage::Activations)
                .expect("mul_mat");
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            // The GEMM route (m > MMV_MAX) computes with bf16-rounded weights
            // and activations; model exactly that so cancellation-heavy dots
            // do not read as kernel defects.
            let is_gemm = m > 8;
            let mut want = vec![0.0f32; n * m];
            for row in 0..n {
                let mut wrow = dequant_row(ty, &weights[row * row_bytes..(row + 1) * row_bytes], k);
                if is_gemm {
                    for w in wrow.iter_mut() {
                        *w = bf16_round(*w);
                    }
                }
                for col in 0..m {
                    let mut acc = 0.0f64;
                    for i in 0..k {
                        let a = if is_gemm {
                            bf16_round(acts[col * k + i])
                        } else {
                            acts[col * k + i]
                        };
                        acc += wrow[i] as f64 * a as f64;
                    }
                    want[col * n + row] = acc as f32;
                }
            }
            compare_tol(case_name, got, want, tol_abs, tol_rel, &mut failures);
        }
    }

    q4k_mmq_canary(&exec, &mut failures);
    q5k_mmq_canary(&exec, &mut failures);
    q6k_mmq_canary(&exec, &mut failures);
    mmvq_swiglu_canary(&exec, &mut failures);
    cpy_set_rows_canary(&exec, &mut failures);
    cpy_ssm_state_canary(&exec, &mut failures);

    // --- f32 mat-vec and cublas GEMM
    {
        let (k, n) = (96usize, 17usize);
        let wdata = f32s(&mut rng, k * n);
        for (case_name, m, tol) in [
            ("mmv_f32_m1", 1usize, 1e-4f32),
            ("mmv_f32", 3usize, 1e-4f32),
            ("gemm_f32", 21usize, 1e-3f32),
        ] {
            let acts = f32s(&mut rng, k * m);
            let mut bench = Bench::new(16 << 20);
            let w = bench.tensor(
                "w",
                TensorType::F32,
                &[k as i64, n as i64],
                &as_bytes_f32(&wdata),
            );
            let x = bench.tensor("x", TensorType::F32, &[k as i64, m as i64], &as_bytes_f32(&acts));
            let out = bench
                .ctx
                .mul_mat(w, x, BufferUsage::Activations)
                .expect("mul_mat");
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            let mut want = vec![0.0f32; n * m];
            for row in 0..n {
                for col in 0..m {
                    let mut acc = 0.0f64;
                    for i in 0..k {
                        acc += wdata[row * k + i] as f64 * acts[col * k + i] as f64;
                    }
                    want[col * n + row] = acc as f32;
                }
            }
            compare(case_name, got, want, tol, &mut failures);
        }
    }

    // --- batched strided f16 x f32 with GQA (division) broadcast
    {
        let (k, n, m, a2, b2) = (64usize, 40usize, 3usize, 2usize, 4usize);
        let a_data = f32s(&mut rng, k * n * a2);
        let b_data = f32s(&mut rng, k * m * b2);
        let mut bench = Bench::new(16 << 20);
        let a = bench.tensor(
            "a",
            TensorType::F16,
            &[k as i64, n as i64, a2 as i64],
            &f16_bytes(&a_data),
        );
        let b = bench.tensor(
            "b",
            TensorType::F32,
            &[k as i64, m as i64, b2 as i64],
            &as_bytes_f32(&b_data),
        );
        let out = bench
            .ctx
            .mul_mat(a, b, BufferUsage::Activations)
            .expect("mul_mat");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; n * m * b2];
        for i2 in 0..b2 {
            let a_i2 = i2 / (b2 / a2);
            for col in 0..m {
                for row in 0..n {
                    let mut acc = 0.0f64;
                    for i in 0..k {
                        acc += f16_round(a_data[(a_i2 * n + row) * k + i]) as f64
                            * b_data[(i2 * m + col) * k + i] as f64;
                    }
                    want[(i2 * m + col) * n + row] = acc as f32;
                }
            }
        }
        compare("mul_mat_batched_f16_gqa", got, want, 1e-3, &mut failures);
    }

    // --- get_rows (quant + f32)
    {
        let (k, n) = (512usize, 9usize);
        let table = quant_blocks(&mut rng, TensorType::Q4K, k, n);
        let row_bytes = table.len() / n;
        let idx = [3i32, 0, 8, 8, 5, 1, 2];
        let mut bench = Bench::new(16 << 20);
        let src = bench.tensor("src", TensorType::Q4K, &[k as i64, n as i64], &table);
        let rows = bench.tensor("rows", TensorType::I32, &[idx.len() as i64], &as_bytes_i32(&idx));
        let out = bench
            .ctx
            .get_rows(src, rows, BufferUsage::Activations)
            .expect("get_rows");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = Vec::new();
        for &r in &idx {
            want.extend(dequant_row(
                TensorType::Q4K,
                &table[r as usize * row_bytes..(r as usize + 1) * row_bytes],
                k,
            ));
        }
        compare("get_rows_q4k", got, want, 1e-6, &mut failures);
    }

    // --- set_rows f32 -> f16 (KV write path)
    {
        let (width, cache_rows, n_rows) = (64usize, 32usize, 5usize);
        let src_data = f32s(&mut rng, width * n_rows);
        let idx = [30i32, 2, 17, 8, 31];
        let mut bench = Bench::new(16 << 20);
        let cache = bench.tensor(
            "cache",
            TensorType::F16,
            &[width as i64, cache_rows as i64],
            &vec![0u8; width * cache_rows * 2],
        );
        let src = bench.tensor(
            "src",
            TensorType::F32,
            &[width as i64, n_rows as i64],
            &as_bytes_f32(&src_data),
        );
        let rows = bench.tensor("rows", TensorType::I32, &[n_rows as i64], &as_bytes_i32(&idx));
        let out = bench
            .ctx
            .set_rows(cache, src, rows, BufferUsage::State)
            .expect("set_rows");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_f16_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; width * cache_rows];
        for (r, &dst_row) in idx.iter().enumerate() {
            for i in 0..width {
                want[dst_row as usize * width + i] = f16_round(src_data[r * width + i]);
            }
        }
        // One f16 ULP of slack: the kernel's f32->f16 cast and the CPU
        // reference's converter may tie-break the halfway case differently.
        compare("set_rows_f32_f16", got, want, 2e-3, &mut failures);
    }

    // --- masked softmax
    {
        let (kc, nq, h) = (97usize, 4usize, 3usize);
        let scale = 0.37f32;
        let x_data = f32s(&mut rng, kc * nq * h);
        let mut mask_data = vec![0.0f32; kc * nq];
        for q in 0..nq {
            for c in 0..kc {
                if c > 40 + q {
                    mask_data[q * kc + c] = f32::NEG_INFINITY;
                }
            }
        }
        let mut bench = Bench::new(16 << 20);
        let x = bench.tensor(
            "x",
            TensorType::F32,
            &[kc as i64, nq as i64, h as i64],
            &as_bytes_f32(&x_data),
        );
        let mask = bench.tensor(
            "mask",
            TensorType::F32,
            &[kc as i64, nq as i64],
            &as_bytes_f32(&mask_data),
        );
        let out = bench
            .ctx
            .soft_max_ext(x, Some(mask), scale, 0.0, BufferUsage::Activations)
            .expect("soft_max_ext");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; kc * nq * h];
        for head in 0..h {
            for q in 0..nq {
                let base = (head * nq + q) * kc;
                let mut maxv = f32::NEG_INFINITY;
                for c in 0..kc {
                    maxv = maxv.max(x_data[base + c] * scale + mask_data[q * kc + c]);
                }
                let mut sum = 0.0f32;
                for c in 0..kc {
                    let e = (x_data[base + c] * scale + mask_data[q * kc + c] - maxv).exp();
                    want[base + c] = e;
                    sum += e;
                }
                for c in 0..kc {
                    want[base + c] /= sum;
                }
            }
        }
        compare("softmax_mask", got, want, 1e-5, &mut failures);
    }

    // --- flash decode vs naive attention (through the real builder shapes)
    {
        let (d, kc, nq, h, hkv) = (64usize, 40usize, 2usize, 4usize, 2usize);
        let scale = 1.0 / (d as f32).sqrt();
        let q_data = f32s(&mut rng, d * nq * h);
        let k_data = f32s(&mut rng, d * kc * hkv);
        let v_data = f32s(&mut rng, d * kc * hkv);
        let mut mask_data = vec![0.0f32; kc * nq];
        for q in 0..nq {
            for c in 0..kc {
                if c > 30 + q {
                    mask_data[q * kc + c] = f32::NEG_INFINITY;
                }
            }
        }
        let mut bench = Bench::new(16 << 20);
        // q laid out [d, nq, h]; k/v cache-shaped [d, kc, hkv]; mask f16.
        let q = bench.tensor(
            "q",
            TensorType::F32,
            &[d as i64, nq as i64, h as i64],
            &as_bytes_f32(&q_data),
        );
        let k = bench.tensor(
            "k",
            TensorType::F16,
            &[d as i64, kc as i64, hkv as i64],
            &f16_bytes(&k_data),
        );
        let v = bench.tensor(
            "v",
            TensorType::F16,
            &[d as i64, kc as i64, hkv as i64],
            &f16_bytes(&v_data),
        );
        let mask = bench.tensor(
            "mask",
            TensorType::F16,
            &[kc as i64, nq as i64],
            &f16_bytes(&mask_data),
        );
        let out = bench
            .ctx
            .flash_attn_ext(q, k, v, Some(mask), scale, 0.0, 0.0, BufferUsage::Activations)
            .expect("flash_attn_ext");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        // dst [d, h, nq]
        let mut want = vec![0.0f32; d * h * nq];
        for iq in 0..nq {
            for head in 0..h {
                let kvh = head / (h / hkv);
                let mut scores = vec![0.0f32; kc];
                let mut maxv = f32::NEG_INFINITY;
                for c in 0..kc {
                    let m = f16_round(mask_data[iq * kc + c]);
                    let s = if m == f32::NEG_INFINITY {
                        f32::NEG_INFINITY
                    } else {
                        let mut acc = 0.0f32;
                        for i in 0..d {
                            acc += q_data[(head * nq + iq) * d + i]
                                * f16_round(k_data[(kvh * kc + c) * d + i]);
                        }
                        acc * scale + m
                    };
                    scores[c] = s;
                    maxv = maxv.max(s);
                }
                let mut sum = 0.0f32;
                for s in scores.iter_mut() {
                    if *s == f32::NEG_INFINITY {
                        *s = 0.0;
                    } else {
                        *s = (*s - maxv).exp();
                        sum += *s;
                    }
                }
                for i in 0..d {
                    let mut acc = 0.0f32;
                    for c in 0..kc {
                        acc += scores[c] * f16_round(v_data[(kvh * kc + c) * d + i]);
                    }
                    want[(iq * h + head) * d + i] = acc / sum;
                }
            }
        }
        compare("flash_decode", got, want, 2e-3, &mut failures);
    }

    // --- fattn-mma-f16 D=256 ncols 8x8 (GQA>4, n_q>=20, n_kv%256==0)
    {
        let (d, kc, nq, h, hkv) = (256usize, 256usize, 32usize, 8usize, 1usize);
        let scale = 1.0 / (d as f32).sqrt();
        let q_data = f32s(&mut rng, d * nq * h);
        let k_data = f32s(&mut rng, d * kc * hkv);
        let v_data = f32s(&mut rng, d * kc * hkv);
        let mut mask_data = vec![0.0f32; kc * nq];
        for q in 0..nq {
            for c in 0..kc {
                if c > q {
                    mask_data[q * kc + c] = f32::NEG_INFINITY;
                }
            }
        }
        let mut bench = Bench::new(64 << 20);
        let q = bench.tensor(
            "q256",
            TensorType::F32,
            &[d as i64, nq as i64, h as i64],
            &as_bytes_f32(&q_data),
        );
        let k = bench.tensor(
            "k256",
            TensorType::F16,
            &[d as i64, kc as i64, hkv as i64],
            &f16_bytes(&k_data),
        );
        let v = bench.tensor(
            "v256",
            TensorType::F16,
            &[d as i64, kc as i64, hkv as i64],
            &f16_bytes(&v_data),
        );
        let mask = bench.tensor(
            "m256",
            TensorType::F16,
            &[kc as i64, nq as i64],
            &f16_bytes(&mask_data),
        );
        let out = bench
            .ctx
            .flash_attn_ext(q, k, v, Some(mask), scale, 0.0, 0.0, BufferUsage::Activations)
            .expect("fattn_mma");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; d * h * nq];
        for iq in 0..nq {
            for head in 0..h {
                let kvh = head / (h / hkv);
                let mut scores = vec![0.0f32; kc];
                let mut maxv = f32::NEG_INFINITY;
                for c in 0..kc {
                    let m = f16_round(mask_data[iq * kc + c]);
                    let s = if m == f32::NEG_INFINITY {
                        f32::NEG_INFINITY
                    } else {
                        let mut acc = 0.0f32;
                        for i in 0..d {
                            acc += q_data[(head * nq + iq) * d + i]
                                * f16_round(k_data[(kvh * kc + c) * d + i]);
                        }
                        acc * scale + m
                    };
                    scores[c] = s;
                    maxv = maxv.max(s);
                }
                let mut sum = 0.0f32;
                for s in scores.iter_mut() {
                    if *s == f32::NEG_INFINITY {
                        *s = 0.0;
                    } else {
                        *s = (*s - maxv).exp();
                        sum += *s;
                    }
                }
                for i in 0..d {
                    let mut acc = 0.0f32;
                    for c in 0..kc {
                        acc += scores[c] * f16_round(v_data[(kvh * kc + c) * d + i]);
                    }
                    want[(iq * h + head) * d + i] = acc / sum.max(1e-20);
                }
            }
        }
        compare_tol("fattn_mma_d256", got, want, 5e-2, 2e-2, &mut failures);
    }

    // --- norms
    {
        let (ne0, rows) = (96usize, 6usize);
        let x_data = f32s(&mut rng, ne0 * rows);
        for (case_name, l2) in [("rms_norm", false), ("l2_norm", true)] {
            let eps = 1e-6f32;
            let mut bench = Bench::new(8 << 20);
            let x = bench.tensor(
                "x",
                TensorType::F32,
                &[ne0 as i64, rows as i64],
                &as_bytes_f32(&x_data),
            );
            let out = if l2 {
                bench.ctx.l2_norm_eps(x, eps, BufferUsage::Activations)
            } else {
                bench.ctx.rms_norm_eps(x, eps, BufferUsage::Activations)
            }
            .expect("norm");
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            let mut want = vec![0.0f32; ne0 * rows];
            for r in 0..rows {
                let row = &x_data[r * ne0..(r + 1) * ne0];
                let sum: f32 = row.iter().map(|v| v * v).sum();
                let denom = if l2 {
                    1.0 / sum.max(eps).sqrt()
                } else {
                    1.0 / (sum / ne0 as f32 + eps).sqrt()
                };
                for i in 0..ne0 {
                    want[r * ne0 + i] = row[i] * denom;
                }
            }
            compare(case_name, got, want, 1e-5, &mut failures);
        }
    }

    // --- official RMS_NORM + MUL (+ ADD) fusion (ggml-cuda.cu:3994-4004)
    {
        fn rms_scale_add(
            x: &[f32],
            w: &[f32],
            add: Option<&[f32]>,
            ne0: usize,
            rows: usize,
            eps: f32,
        ) -> Vec<f32> {
            let mut want = vec![0.0f32; ne0 * rows];
            for r in 0..rows {
                let row = &x[r * ne0..(r + 1) * ne0];
                let sum: f32 = row.iter().map(|v| v * v).sum();
                let scale = 1.0 / (sum / ne0 as f32 + eps).sqrt();
                for i in 0..ne0 {
                    let mut v = row[i] * scale * w[i % w.len()];
                    if let Some(a) = add {
                        v += a[i % a.len()];
                    }
                    want[r * ne0 + i] = v;
                }
            }
            want
        }
        for (case_name, ne0, rows, with_add) in [
            ("rms_norm_mul", 96usize, 6usize, false),
            ("rms_norm_mul_1024", 1024usize, 3usize, false),
            ("rms_norm_mul_add", 96usize, 6usize, true),
        ] {
            let x_data = f32s(&mut rng, ne0 * rows);
            let w_data = f32s(&mut rng, ne0);
            let add_data = f32s(&mut rng, ne0);
            let eps = 1e-6f32;
            let mut bench = Bench::new(16 << 20);
            let x = bench.tensor(
                "x",
                TensorType::F32,
                &[ne0 as i64, rows as i64],
                &as_bytes_f32(&x_data),
            );
            let w = bench.tensor("w", TensorType::F32, &[ne0 as i64], &as_bytes_f32(&w_data));
            let norm = bench
                .ctx
                .rms_norm_eps(x, eps, BufferUsage::Activations)
                .expect("rms");
            let scaled = bench
                .ctx
                .binary_like_a(makepad_ggml::Op::Mul, norm, w, BufferUsage::Activations)
                .expect("mul");
            let out = if with_add {
                let a = bench.tensor(
                    "add",
                    TensorType::F32,
                    &[ne0 as i64],
                    &as_bytes_f32(&add_data),
                );
                bench
                    .ctx
                    .binary_like_a(makepad_ggml::Op::Add, scaled, a, BufferUsage::Activations)
                    .expect("add")
            } else {
                scaled
            };
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            let want = rms_scale_add(
                &x_data,
                &w_data,
                with_add.then_some(add_data.as_slice()),
                ne0,
                rows,
                eps,
            );
            compare(case_name, got, want, 1e-5, &mut failures);
        }
        {
            let (ne0, rows) = (96usize, 6usize);
            let x_data = f32s(&mut rng, ne0 * rows);
            let w_data = f32s(&mut rng, ne0);
            let eps = 1e-6f32;
            let run = |fuse: bool| {
                let mut bench = Bench::new(8 << 20);
                let x = bench.tensor(
                    "x",
                    TensorType::F32,
                    &[ne0 as i64, rows as i64],
                    &as_bytes_f32(&x_data),
                );
                let w = bench.tensor("w", TensorType::F32, &[ne0 as i64], &as_bytes_f32(&w_data));
                let norm = bench
                    .ctx
                    .rms_norm_eps(x, eps, BufferUsage::Activations)
                    .expect("rms");
                let out = bench
                    .ctx
                    .binary_like_a(makepad_ggml::Op::Mul, norm, w, BufferUsage::Activations)
                    .expect("mul");
                let _disable = ScopedEnv::set("MKLLM_DISABLE_RMS_FUSION", if fuse { "0" } else { "1" });
                let outputs = bench.run(&exec, out, &[out]);
                bytes_to_f32(&outputs[&out])
            };
            let fused = run(true);
            let unfused = run(false);
            let delta = error_stats(&fused, &unfused);
            if delta.non_finite == 0 && delta.max <= 1e-6 {
                println!(
                    "ok   rms_norm_mul_fuse: fused/unfused max_delta {:.7}",
                    delta.max
                );
            } else {
                failures += 1;
                println!(
                    "FAIL rms_norm_mul_fuse: max_delta {:.7} non_finite {}",
                    delta.max, delta.non_finite
                );
            }
        }
    }

    // --- rope IMROPE
    {
        let (d, heads, tokens) = (64usize, 3usize, 4usize);
        let n_dims = 32i32;
        let sections = [6i32, 5, 5, 0];
        let freq_base = 1_000_000.0f32;
        let x_data = f32s(&mut rng, d * heads * tokens);
        let mut pos = vec![0i32; 4 * tokens];
        for t in 0..tokens {
            pos[t] = 10 + t as i32;
            pos[tokens + t] = 20 + t as i32;
            pos[2 * tokens + t] = 30 + t as i32;
        }
        let mut bench = Bench::new(8 << 20);
        let x = bench.tensor(
            "x",
            TensorType::F32,
            &[d as i64, heads as i64, tokens as i64],
            &as_bytes_f32(&x_data),
        );
        let p = bench.tensor("pos", TensorType::I32, &[(4 * tokens) as i64], &as_bytes_i32(&pos));
        let out = bench
            .ctx
            .rope_multi(
                x,
                p,
                None,
                n_dims,
                sections,
                GGML_ROPE_TYPE_IMROPE,
                0,
                freq_base,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .expect("rope_multi");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = x_data.clone();
        let sect_1 = sections[1];
        let sect_2 = sections[2];
        let sect_0 = sections[0];
        let sect_dims = sections.iter().sum::<i32>();
        for t in 0..tokens {
            for head in 0..heads {
                let base = (t * heads + head) * d;
                for i0 in (0..n_dims as usize).step_by(2) {
                    let ic = i0 / 2;
                    let sector = (ic as i32) % sect_dims;
                    let plane = if sector % 3 == 1 && sector < 3 * sect_1 {
                        1
                    } else if sector % 3 == 2 && sector < 3 * sect_2 {
                        2
                    } else if sector % 3 == 0 && sector < 3 * sect_0 {
                        0
                    } else {
                        3
                    };
                    let theta_base = pos[plane * tokens + t] as f32;
                    let theta = theta_base * freq_base.powf(-(i0 as f32) / n_dims as f32);
                    let (sin_t, cos_t) = theta.sin_cos();
                    let x0 = x_data[base + ic];
                    let x1 = x_data[base + ic + n_dims as usize / 2];
                    want[base + ic] = x0 * cos_t - x1 * sin_t;
                    want[base + ic + n_dims as usize / 2] = x0 * sin_t + x1 * cos_t;
                }
            }
        }
        compare("rope_imrope", got, want, 1e-4, &mut failures);
    }

    // --- binary broadcast, unary, glu
    {
        let (ne0, rows) = (64usize, 7usize);
        let a_data = f32s(&mut rng, ne0 * rows);
        let b_data = f32s(&mut rng, ne0);
        let mut bench = Bench::new(8 << 20);
        let a = bench.tensor(
            "a",
            TensorType::F32,
            &[ne0 as i64, rows as i64],
            &as_bytes_f32(&a_data),
        );
        let b = bench.tensor("b", TensorType::F32, &[ne0 as i64], &as_bytes_f32(&b_data));
        let out = bench
            .ctx
            .binary_like_a(makepad_ggml::Op::Mul, a, b, BufferUsage::Activations)
            .expect("mul");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let want: Vec<f32> = a_data
            .iter()
            .enumerate()
            .map(|(i, v)| v * b_data[i % ne0])
            .collect();
        compare("mul_broadcast_row", got, want, 1e-6, &mut failures);
    }
    {
        let n = 300usize;
        let x_data = f32s(&mut rng, n);
        for (case_name, op, f) in [
            ("unary_silu", UnaryOp::Silu, (|x: f32| x / (1.0 + (-x).exp())) as fn(f32) -> f32),
            ("unary_exp", UnaryOp::Exp, |x: f32| x.exp()),
            ("unary_neg", UnaryOp::Neg, |x: f32| -x),
            ("unary_softplus", UnaryOp::SoftPlus, |x: f32| x.exp().ln_1p()),
            ("unary_sigmoid", UnaryOp::Sigmoid, |x: f32| 1.0 / (1.0 + (-x).exp())),
        ] {
            let mut bench = Bench::new(4 << 20);
            let x = bench.tensor("x", TensorType::F32, &[n as i64], &as_bytes_f32(&x_data));
            let out = bench.ctx.unary(x, op, BufferUsage::Activations).expect("unary");
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            let want: Vec<f32> = x_data.iter().map(|&x| f(x)).collect();
            compare(case_name, got, want, 1e-5, &mut failures);
        }
    }
    {
        let n = 256usize;
        let a_data = f32s(&mut rng, n);
        let b_data = f32s(&mut rng, n);
        let mut bench = Bench::new(4 << 20);
        let a = bench.tensor("a", TensorType::F32, &[n as i64], &as_bytes_f32(&a_data));
        let b = bench.tensor("b", TensorType::F32, &[n as i64], &as_bytes_f32(&b_data));
        let out = bench
            .ctx
            .glu_split(a, b, GluOp::Swiglu, BufferUsage::Activations)
            .expect("glu");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let want: Vec<f32> = a_data
            .iter()
            .zip(&b_data)
            .map(|(&a, &b)| (a / (1.0 + (-a).exp())) * b)
            .collect();
        compare("glu_swiglu", got, want, 1e-5, &mut failures);
    }

    // --- cont of a permuted view + concat
    {
        let (r, c) = (8usize, 6usize);
        let x_data = f32s(&mut rng, r * c);
        let mut bench = Bench::new(4 << 20);
        let x = bench.tensor("x", TensorType::F32, &[r as i64, c as i64], &as_bytes_f32(&x_data));
        let xt = bench.ctx.transpose(x).expect("transpose");
        let out = bench.ctx.cont(xt).expect("cont");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; r * c];
        for i in 0..r {
            for j in 0..c {
                want[i * c + j] = x_data[j * r + i];
            }
        }
        compare("cont_transposed", got, want, 0.0, &mut failures);
    }
    {
        let (ne0, r_a, r_b) = (16usize, 3usize, 5usize);
        let a_data = f32s(&mut rng, ne0 * r_a);
        let b_data = f32s(&mut rng, ne0 * r_b);
        let mut bench = Bench::new(4 << 20);
        let a = bench.tensor("a", TensorType::F32, &[ne0 as i64, r_a as i64], &as_bytes_f32(&a_data));
        let b = bench.tensor("b", TensorType::F32, &[ne0 as i64, r_b as i64], &as_bytes_f32(&b_data));
        let out = bench.ctx.concat(a, b, 1, BufferUsage::Activations).expect("concat");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = a_data.clone();
        want.extend_from_slice(&b_data);
        compare("concat_dim1", got, want, 0.0, &mut failures);
    }
    {
        // Recurrent convolution cache update used by Qwen3.5/3.8:
        // [prefix, channels] + transpose([channels, 1]), slice off the first
        // sample of every channel, then flatten the strided 3-D view to 2-D.
        // This deliberately changes the logical extents across Cont.
        let (prefix, channels) = (3usize, 257usize);
        let r_width = prefix * channels;
        let cache_data = f32s(&mut rng, r_width);
        let qkv_data = f32s(&mut rng, channels);
        let mut bench = Bench::new(4 << 20);
        let cache_rows = bench.tensor(
            "r_cache_rows",
            TensorType::F32,
            &[r_width as i64, 1],
            &as_bytes_f32(&cache_data),
        );
        let conv_states = bench
            .ctx
            .view_3d(
                cache_rows,
                prefix as i64,
                channels as i64,
                1,
                prefix * size_of::<f32>(),
                r_width * size_of::<f32>(),
                0,
            )
            .expect("conv states view");
        let qkv = bench.tensor(
            "qkv",
            TensorType::F32,
            &[channels as i64, 1],
            &as_bytes_f32(&qkv_data),
        );
        let qkv_t = bench.ctx.transpose(qkv).expect("qkv transpose");
        let conv_input = bench
            .ctx
            .concat(conv_states, qkv_t, 0, BufferUsage::Activations)
            .expect("conv input concat");
        let conv_input_tensor = bench.ctx.tensor(conv_input).expect("conv input").clone();
        let last_states = bench
            .ctx
            .view_3d(
                conv_input,
                prefix as i64,
                channels as i64,
                1,
                conv_input_tensor.nb[1],
                conv_input_tensor.nb[2],
                size_of::<f32>(),
            )
            .expect("last convolution states view");
        let out = bench
            .ctx
            .cont_2d(last_states, r_width as i64, 1)
            .expect("flatten last convolution states");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = Vec::with_capacity(r_width);
        for channel in 0..channels {
            let row = &cache_data[channel * prefix..(channel + 1) * prefix];
            want.extend_from_slice(&row[1..]);
            want.push(qkv_data[channel]);
        }
        compare(
            "cont_strided_reshape_recurrent_r",
            got,
            want,
            0.0,
            &mut failures,
        );
    }

    // --- ssm_conv (ggml layout)
    {
        let (d_conv, d_inner, n_t) = (4usize, 16usize, 7usize);
        let span = n_t + d_conv - 1;
        let sx_data = f32s(&mut rng, span * d_inner);
        let c_data = f32s(&mut rng, d_conv * d_inner);
        let mut bench = Bench::new(4 << 20);
        let sx = bench.tensor(
            "sx",
            TensorType::F32,
            &[span as i64, d_inner as i64, 1],
            &as_bytes_f32(&sx_data),
        );
        let c = bench.tensor(
            "c",
            TensorType::F32,
            &[d_conv as i64, d_inner as i64],
            &as_bytes_f32(&c_data),
        );
        let out = bench.ctx.ssm_conv(sx, c, BufferUsage::Activations).expect("ssm_conv");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; d_inner * n_t];
        for t in 0..n_t {
            for i in 0..d_inner {
                let mut acc = 0.0f32;
                for k in 0..d_conv {
                    acc += sx_data[i * span + t + k] * c_data[i * d_conv + k];
                }
                want[t * d_inner + i] = acc;
            }
        }
        compare("ssm_conv", got, want, 1e-5, &mut failures);
    }

    // --- official SSM_CONV + SILU (ggml-cuda.cu:4006) and UNARY + MUL (4012)
    {
        let (d_conv, d_inner, n_t) = (4usize, 16usize, 7usize);
        let span = n_t + d_conv - 1;
        let sx_data = f32s(&mut rng, span * d_inner);
        let c_data = f32s(&mut rng, d_conv * d_inner);
        let silu = |x: f32| x / (1.0 + (-x).exp());
        let mut bench = Bench::new(4 << 20);
        let sx = bench.tensor(
            "sx",
            TensorType::F32,
            &[span as i64, d_inner as i64, 1],
            &as_bytes_f32(&sx_data),
        );
        let c = bench.tensor(
            "c",
            TensorType::F32,
            &[d_conv as i64, d_inner as i64],
            &as_bytes_f32(&c_data),
        );
        let conv = bench.ctx.ssm_conv(sx, c, BufferUsage::Activations).expect("ssm_conv");
        let out = bench
            .ctx
            .unary(conv, UnaryOp::Silu, BufferUsage::Activations)
            .expect("silu");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        let mut want = vec![0.0f32; d_inner * n_t];
        for t in 0..n_t {
            for i in 0..d_inner {
                let mut acc = 0.0f32;
                for k in 0..d_conv {
                    acc += sx_data[i * span + t + k] * c_data[i * d_conv + k];
                }
                want[t * d_inner + i] = silu(acc);
            }
        }
        compare("ssm_conv_silu", got, want, 1e-5, &mut failures);
    }
    {
        let n = 256usize;
        let x_data = f32s(&mut rng, n);
        let g_data = f32s(&mut rng, n);
        for (case_name, op, f) in [
            (
                "unary_mul_silu",
                UnaryOp::Silu,
                (|x: f32| x / (1.0 + (-x).exp())) as fn(f32) -> f32,
            ),
            (
                "unary_mul_sigmoid",
                UnaryOp::Sigmoid,
                |x: f32| 1.0 / (1.0 + (-x).exp()),
            ),
            (
                "unary_mul_softplus",
                UnaryOp::SoftPlus,
                |x: f32| {
                    if x > 20.0 {
                        x
                    } else {
                        x.exp().ln_1p()
                    }
                },
            ),
        ] {
            let mut bench = Bench::new(4 << 20);
            let x = bench.tensor("x", TensorType::F32, &[n as i64], &as_bytes_f32(&x_data));
            let g = bench.tensor("g", TensorType::F32, &[n as i64], &as_bytes_f32(&g_data));
            let u = bench.ctx.unary(x, op, BufferUsage::Activations).expect("unary");
            let out = bench
                .ctx
                .binary_like_a(makepad_ggml::Op::Mul, u, g, BufferUsage::Activations)
                .expect("mul");
            let outputs = bench.run(&exec, out, &[out]);
            let got = bytes_to_f32(&outputs[&out]);
            let want: Vec<f32> = x_data.iter().zip(&g_data).map(|(&x, &g)| f(x) * g).collect();
            compare(case_name, got, want, 1e-5, &mut failures);
        }
    }

    // --- gated delta net (scalar gate, GQA k-heads) vs sequential reference,
    // in isolating variants: core loop (h1), head mapping (t1), full case.
    for (case_name, sv, h, hk, n_t) in [
        ("gdn_h1_core", 32usize, 1usize, 1usize, 3usize),
        ("gdn_map_t1", 32, 6, 3, 1),
        ("gated_delta_net", 32, 6, 3, 3),
    ] {
        let mut rng = Rng::new(4242);
        let q_data = f32s(&mut rng, sv * hk * n_t);
        let k_data = f32s(&mut rng, sv * hk * n_t);
        let v_data = f32s(&mut rng, sv * h * n_t);
        let g_data: Vec<f32> = f32s(&mut rng, h * n_t).iter().map(|x| -x.abs()).collect();
        let beta_data: Vec<f32> = f32s(&mut rng, h * n_t).iter().map(|x| 0.5 + 0.4 * x).collect();
        let state_data = f32s(&mut rng, sv * sv * h);
        let mut bench = Bench::new(8 << 20);
        let q = bench.tensor(
            "q",
            TensorType::F32,
            &[sv as i64, hk as i64, n_t as i64, 1],
            &as_bytes_f32(&q_data),
        );
        let k = bench.tensor(
            "k",
            TensorType::F32,
            &[sv as i64, hk as i64, n_t as i64, 1],
            &as_bytes_f32(&k_data),
        );
        let v = bench.tensor(
            "v",
            TensorType::F32,
            &[sv as i64, h as i64, n_t as i64, 1],
            &as_bytes_f32(&v_data),
        );
        let g = bench.tensor(
            "g",
            TensorType::F32,
            &[1, h as i64, n_t as i64, 1],
            &as_bytes_f32(&g_data),
        );
        let beta = bench.tensor(
            "beta",
            TensorType::F32,
            &[1, h as i64, n_t as i64, 1],
            &as_bytes_f32(&beta_data),
        );
        let state = bench.tensor(
            "state",
            TensorType::F32,
            &[(sv * sv) as i64, h as i64, 1, 1],
            &as_bytes_f32(&state_data),
        );
        let out = bench
            .ctx
            .gated_delta_net(q, k, v, g, beta, state, BufferUsage::Activations)
            .expect("gated_delta_net");
        let outputs = bench.run(&exec, out, &[out]);
        let got = bytes_to_f32(&outputs[&out]);
        // Sequential reference (kernel semantics, kda = 0).
        let scale = 1.0 / (sv as f32).sqrt();
        let mut state_ref = state_data.clone(); // [h][col sv][row sv] as (head*sv + col)*sv + row
        let mut attn_ref = vec![0.0f32; sv * h * n_t];
        for t in 0..n_t {
            for head in 0..h {
                let kv_head = head % hk;
                let qv = &q_data[(t * hk + kv_head) * sv..(t * hk + kv_head + 1) * sv];
                let kv = &k_data[(t * hk + kv_head) * sv..(t * hk + kv_head + 1) * sv];
                let vv = &v_data[(t * h + head) * sv..(t * h + head + 1) * sv];
                let g_scalar = g_data[t * h + head].exp();
                let beta_val = beta_data[t * h + head];
                for col in 0..sv {
                    let scol = &mut state_ref[(head * sv + col) * sv..(head * sv + col + 1) * sv];
                    let mut kv_dot = 0.0f32;
                    for row in 0..sv {
                        kv_dot += scol[row] * kv[row];
                    }
                    let delta = (vv[col] - g_scalar * kv_dot) * beta_val;
                    let mut attn = 0.0f32;
                    for row in 0..sv {
                        scol[row] = g_scalar * scol[row] + kv[row] * delta;
                        attn += scol[row] * qv[row];
                    }
                    attn_ref[(t * h + head) * sv + col] = attn * scale;
                }
            }
        }
        let mut want = attn_ref;
        want.extend_from_slice(&state_ref);
        if case_name == "gdn_map_t1" {
            let attn_len = sv * h * n_t;
            for head in 0..h {
                let mut attn_err = 0.0f32;
                for col in 0..sv {
                    for t in 0..n_t {
                        let i = (t * h + head) * sv + col;
                        attn_err = attn_err.max((got[i] - want[i]).abs());
                    }
                }
                let mut state_err = 0.0f32;
                for i in 0..sv * sv {
                    let idx = attn_len + head * sv * sv + i;
                    state_err = state_err.max((got[idx] - want[idx]).abs());
                }
                println!("  gdn head {head}: attn_err {attn_err:.4} state_err {state_err:.4}");
            }
        }
        compare_tol(case_name, got, want, 5e-4, 1e-5, &mut failures);
    }

    println!(
        "\nopcheck: {} failures{}",
        failures,
        if failures == 0 { " — ALL GREEN" } else { "" }
    );
    if failures == 0 {
        0
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// generate / bench
// ---------------------------------------------------------------------------

fn parse_kv(args: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut iter = args.iter().skip(1);
    while let Some(key) = iter.next() {
        if let Some(name) = key.strip_prefix("--") {
            if let Some(value) = iter.next() {
                out.insert(name.to_string(), value.clone());
            }
        }
    }
    out
}

fn vram_line(tag: &str) {
    if let Ok(ExecRuntime::Cuda(runtime)) = ExecRuntime::with_backend(ExecBackendKind::Cuda) {
        let f = runtime.features();
        println!(
            "vram.{tag}: used_mb={} free_mb={} total_mb={}",
            (f.total_vram_bytes - f.free_vram_bytes) / (1 << 20),
            f.free_vram_bytes / (1 << 20),
            f.total_vram_bytes / (1 << 20)
        );
    }
}

fn generate(args: &[String]) -> i32 {
    let model_path = PathBuf::from(match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("generate: model path required");
            return 2;
        }
    });
    let kv = parse_kv(args);
    let prompt = kv.get("prompt").cloned().unwrap_or_else(|| "The capital of France is".into());
    let max_new: usize = kv.get("max-new-tokens").and_then(|v| v.parse().ok()).unwrap_or(32);
    let prefill_batch: usize = kv.get("prefill-batch-size").and_then(|v| v.parse().ok()).unwrap_or(256);
    let max_context: u32 = kv.get("max-context").and_then(|v| v.parse().ok()).unwrap_or(8192);

    vram_line("before_load");
    let t0 = Instant::now();
    let model = match LlamaModel::load(&model_path) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("model load failed: {err:?}");
            return 1;
        }
    };
    let mut session = match LlamaSession::from_model(
        &model,
        LlamaSessionConfig {
            max_context: Some(max_context),
            prefill_batch_size: prefill_batch,
            ..LlamaSessionConfig::default()
        },
    ) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("session load failed: {err:?}");
            return 1;
        }
    };
    let load_s = t0.elapsed().as_secs_f64();
    vram_line("after_load");

    let tokens = session.vocab().tokenize(&prompt, true, true).expect("tokenize");
    println!("prompt.tokens: {}", tokens.len());
    let t1 = Instant::now();
    if let Err(err) = session.append_tokens(&tokens) {
        eprintln!("prefill failed: {err:?}");
        return 1;
    }
    let prefill_s = t1.elapsed().as_secs_f64();
    vram_line("after_prefill");

    let t2 = Instant::now();
    let mut first_token_s = None;
    let mut generated: Vec<i32> = Vec::new();
    for _ in 0..max_new {
        match session.next_greedy_token() {
            Ok(Some(tok)) => {
                if first_token_s.is_none() {
                    first_token_s = Some(t2.elapsed().as_secs_f64());
                }
                generated.push(tok);
            }
            Ok(None) => break,
            Err(err) => {
                eprintln!("decode failed: {err:?}");
                return 1;
            }
        }
    }
    let decode_s = t2.elapsed().as_secs_f64();
    vram_line("after_decode");

    println!("generated.token_ids: {generated:?}");
    println!("generated.text: {:?}", session.vocab().decode_tokens(&generated).unwrap_or_default());
    println!("load.seconds: {load_s:.3}");
    println!("prefill.seconds: {prefill_s:.3}");
    println!("prefill.tok_s: {:.2}", tokens.len() as f64 / prefill_s);
    println!(
        "ttft.seconds: {:.3}",
        prefill_s + first_token_s.unwrap_or(0.0)
    );
    println!("decode.tokens: {}", generated.len());
    println!("decode.seconds: {decode_s:.3}");
    println!("decode.tok_s: {:.2}", generated.len() as f64 / decode_s.max(1e-9));
    println!("e2e.seconds: {:.3}", t1.elapsed().as_secs_f64() + 0.0);
    0
}

fn graph_key_bucket(cache_tokens: usize, max_context: usize) -> usize {
    cache_tokens.next_multiple_of(256).min(max_context).max(1)
}

fn run_test_gen(
    session: &mut LlamaSession,
    rng: &mut Rng,
    n_vocab: usize,
    bos: Option<i32>,
    n_gen: usize,
    trace_keys: bool,
    max_context: usize,
) {
    let split = std::env::var_os("MAKEPAD_LLAMA_HOST_SPLIT").is_some();
    if split {
        host_split_reset();
    }
    let t_wall = Instant::now();
    for i in 0..n_gen {
        let tok = if i == 0 && session.token_count() == 0 {
            bos.unwrap_or_else(|| (rng.next_u64() % n_vocab as u64) as i32)
        } else {
            (rng.next_u64() % n_vocab as u64) as i32
        };
        let cache_after = session.token_count() + 1;
        if trace_keys && i < 3 {
            println!(
                "bench.n_kv: after_token={} live={} pad256={}",
                i + 1,
                cache_after,
                graph_key_bucket(cache_after, max_context)
            );
        }
        session.append_token(tok).expect("decode");
    }
    if split {
        let wall_ms = t_wall.elapsed().as_secs_f64() * 1e3;
        let snap = host_split_snapshot();
        println!("{}", snap.report_line());
        println!(
            "host.split.canary: wall={:.3} ms/tok llama-bench.cpp:2026 test_gen=decode+sync+rand",
            wall_ms / n_gen.max(1) as f64
        );
    }
}

fn bench(args: &[String]) -> i32 {
    let model_path = PathBuf::from(match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("bench: model path required");
            return 2;
        }
    });
    let kv = parse_kv(args);
    // llama-bench.cpp:2036 test_gen: decode + synchronize + rand.
    // Never llama_get_logits — D2H+parse of vocab 248320 was 0.63 ms/tok.
    std::env::set_var("MAKEPAD_LLAMA_SKIP_LOGITS", "1");
    let prompt_tokens: usize = kv.get("prompt-tokens").and_then(|v| v.parse().ok()).unwrap_or(512);
    let gen_tokens: usize = kv.get("gen-tokens").and_then(|v| v.parse().ok()).unwrap_or(128);
    let repeats: usize = kv.get("repeats").and_then(|v| v.parse().ok()).unwrap_or(5);
    let prefill_batch: usize = kv.get("prefill-batch-size").and_then(|v| v.parse().ok()).unwrap_or(256);
    let max_context: u32 = kv.get("max-context").and_then(|v| v.parse().ok()).unwrap_or(8192);
    let prompt = kv.get("prompt").cloned().unwrap_or_else(|| {
        // Deterministic natural prompt padded by repetition to the target
        // token count during tokenization below.
        "The quick brown fox jumps over the lazy dog while seventeen engineers review \
         the quarterly telemetry report and annotate every anomaly with careful notes. "
            .to_string()
    });

    vram_line("before_load");
    let t0 = Instant::now();
    let model = LlamaModel::load(&model_path).expect("model load");
    let mut session = LlamaSession::from_model(
        &model,
        LlamaSessionConfig {
            max_context: Some(max_context),
            prefill_batch_size: prefill_batch,
            ..LlamaSessionConfig::default()
        },
    )
    .expect("session load");
    println!("load.seconds: {:.3}", t0.elapsed().as_secs_f64());
    vram_line("after_load");

    // Build a prompt with exactly `prompt_tokens` tokens.
    let mut text = String::new();
    let mut tokens: Vec<i32> = Vec::new();
    while tokens.len() < prompt_tokens {
        text.push_str(&prompt);
        tokens = session.vocab().tokenize(&text, true, true).expect("tokenize");
    }
    tokens.truncate(prompt_tokens);
    println!("bench.prompt_tokens: {}", tokens.len());
    println!("bench.gen_tokens_target: {gen_tokens}");
    println!("bench.prefill_batch: {prefill_batch}");
    println!("bench.max_context: {max_context}");
    println!("bench.protocol: llama-bench.cpp:2026 test_gen (random tokens, no argmax)");

    let n_vocab = session.vocab().len();
    let bos = session
        .vocab()
        .bos_token_id()
        .filter(|_| session.vocab().add_bos_token());
    let max_context_usize = max_context as usize;
    // llama-bench.cpp:2033-2042: BOS or rand, then n_gen decode steps with a
    // fresh rand token. No softmax/argmax. n_kv after 512 pads to 512; the
    // next token pads to 768 (GRAPH_KEY_BUCKET=256, llama-kv-cache.cpp:1121).
    let mut rng = Rng::new(0x4c4c_414d_4142_454e);

    // llama-bench warmup: full prompt + 1 gen token, then a lone gen token so
    // the empty-cache tg graph (n_kv pad 256) is compiled before timing.
    session.reset().expect("session reset");
    session.append_tokens(&tokens).expect("warmup prefill");
    println!(
        "bench.n_kv: after_pp512 live={} pad256={}",
        session.token_count(),
        graph_key_bucket(session.token_count(), max_context_usize)
    );
    run_test_gen(
        &mut session,
        &mut rng,
        n_vocab,
        bos,
        1,
        true,
        max_context_usize,
    );
    session.reset().expect("session reset");
    run_test_gen(
        &mut session,
        &mut rng,
        n_vocab,
        bos,
        1,
        true,
        max_context_usize,
    );

    let mut prefill_times = Vec::new();
    let mut tg_empty_times = Vec::new();
    let mut tg_after_pp_times = Vec::new();
    for repeat in 1..=repeats {
        session.reset().expect("session reset");
        let t1 = Instant::now();
        session.append_tokens(&tokens).expect("prefill");
        let prefill_s = t1.elapsed().as_secs_f64();

        // After-pp decode: same n_kv as pp512+tg (512 then 768). Still random
        // tokens — the test_gen elision, not greedy softmax.
        let t2 = Instant::now();
        run_test_gen(
            &mut session,
            &mut rng,
            n_vocab,
            bos,
            gen_tokens,
            repeat == 1,
            max_context_usize,
        );
        let after_pp_s = t2.elapsed().as_secs_f64();

        // Official llama-bench tg128 is a separate test: n_prompt=0, n_gen=128,
        // empty cache, n_kv pad 256 for the whole run.
        session.reset().expect("session reset");
        let t3 = Instant::now();
        run_test_gen(
            &mut session,
            &mut rng,
            n_vocab,
            bos,
            gen_tokens,
            repeat == 1,
            max_context_usize,
        );
        let tg_empty_s = t3.elapsed().as_secs_f64();

        println!(
            "repeat[{repeat}] measured: prefill_s={prefill_s:.3} ({:.1} tok/s) after_pp_s={after_pp_s:.3} ({:.2} tok/s) tg_empty_s={tg_empty_s:.3} ({:.2} tok/s)",
            tokens.len() as f64 / prefill_s,
            gen_tokens as f64 / after_pp_s.max(1e-9),
            gen_tokens as f64 / tg_empty_s.max(1e-9),
        );
        vram_line(&format!("after_repeat_{repeat}"));
        prefill_times.push(prefill_s);
        tg_after_pp_times.push(after_pp_s);
        tg_empty_times.push(tg_empty_s);
    }

    let median = |values: &mut Vec<f64>| -> (f64, f64, f64) {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (
            values[values.len() / 2],
            values[0],
            values[values.len() - 1],
        )
    };
    let (p_med, p_min, p_max) = median(&mut prefill_times);
    let (ap_med, ap_min, ap_max) = median(&mut tg_after_pp_times);
    let (te_med, te_min, te_max) = median(&mut tg_empty_times);
    println!("bench.prefill.median_s: {p_med:.3} (min {p_min:.3}, max {p_max:.3})");
    println!("bench.prefill.median_tok_s: {:.1}", prompt_tokens as f64 / p_med);
    println!("bench.decode.tokens: {gen_tokens}");
    println!("bench.decode.after_pp.median_s: {ap_med:.3} (min {ap_min:.3}, max {ap_max:.3})");
    println!(
        "bench.decode.after_pp.median_tok_s: {:.2}",
        gen_tokens as f64 / ap_med
    );
    println!("bench.decode.tg_empty.median_s: {te_med:.3} (min {te_min:.3}, max {te_max:.3})");
    println!(
        "bench.decode.tg_empty.median_tok_s: {:.2}",
        gen_tokens as f64 / te_med
    );
    // Continuity with the previous official line (was greedy-after-pp).
    println!("bench.decode.median_s: {ap_med:.3} (min {ap_min:.3}, max {ap_max:.3})");
    println!("bench.decode.median_tok_s: {:.2}", gen_tokens as f64 / ap_med);
    0
}

#[cfg(test)]
mod tests {
    use super::{ds4_butterfly_sum, f32_to_f16_rne_bits};

    #[test]
    fn mmq_half_conversion_is_ties_to_even() {
        assert_eq!(f32_to_f16_rne_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(
            f32_to_f16_rne_bits(1.0 + 3.0 * 2f32.powi(-11)),
            0x3c02
        );
        assert_eq!(f32_to_f16_rne_bits(2f32.powi(-25)), 0x0000);
        assert_eq!(f32_to_f16_rne_bits(3.0 * 2f32.powi(-25)), 0x0002);
        assert_eq!(f32_to_f16_rne_bits(65_504.0), 0x7bff);
    }

    #[test]
    fn ds4_sum_uses_cuda_xor_butterfly_order() {
        let partials = [1e20f32, 5.0, 3.0, 7.0, -1e20, 6.0, 4.0, 8.0];
        let mut values = [0.0f32; 32];
        for (lane, value) in partials.into_iter().enumerate() {
            values[lane * 4] = value;
        }
        assert_eq!(ds4_butterfly_sum(&values), 33.0);
    }
}
