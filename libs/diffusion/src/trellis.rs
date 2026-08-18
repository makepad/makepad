//! TRELLIS 2 shared foundation: per-checkpoint safetensors weights, the
//! rectified-flow Euler/CFG sampler math, 3D rope phase tables, timestep
//! embedding and the sparse-structure coordinate helpers. Every formula
//! mirrors the microsoft/TRELLIS.2 reference (trellis2/pipelines/samplers/
//! flow_euler.py, modules/attention/rope.py, models/sparse_structure_flow.py)
//! exactly — schedule grid in f64 (numpy), tensor math in f32.

use crate::{DiffusionError, Result};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

// DiT trunk (identical across the three flow models).
pub const T2_MODEL_CHANNELS: usize = 1536;
pub const T2_HEAD_COUNT: usize = 12;
pub const T2_HEAD_DIM: usize = 128; // 1536 / 12
pub const T2_DEPTH: usize = 30;
pub const T2_COND_CHANNELS: usize = 1024;
pub const T2_FFN_DIM: usize = 8192; // int(1536 * 5.3334)
pub const T2_NORM_EPS: f32 = 1e-6; // block LayerNorm32 eps
pub const T2_FINAL_NORM_EPS: f32 = 1e-5; // F.layer_norm default at the head
pub const T2_SIGMA_MIN: f32 = 1e-5;

// Sparse-structure flow io.
pub const T2_SS_RESOLUTION: usize = 16;
pub const T2_SS_CHANNELS: usize = 8;
pub const T2_SS_TOKENS: usize = T2_SS_RESOLUTION * T2_SS_RESOLUTION * T2_SS_RESOLUTION;

// SLAT flow io.
pub const T2_SLAT_CHANNELS: usize = 32;
pub const T2_TEX_IN_CHANNELS: usize = 64; // tex noise 32 | shape concat 32

// 3D rope: head_dim/2/3 frequencies per axis, identity-padded to 64 pairs.
pub const T2_ROPE_FREQ_DIM: usize = T2_HEAD_DIM / 2 / 3; // 21
pub const T2_ROPE_PAIRS: usize = T2_HEAD_DIM / 2; // 64
pub const T2_ROPE_THETA: f32 = 10000.0;

// Per-channel SLAT latent normalization (pipeline.json, TRELLIS.2-4B).
pub const T2_SHAPE_SLAT_MEAN: [f32; 32] = [
    0.781296, 0.018091, -0.495192, -0.558457, 1.060530, 0.093252, 1.518149, -0.933218,
    -0.732996, 2.604095, -0.118341, -2.143904, 0.495076, -2.179512, -2.130751, -0.996944,
    0.261421, -2.217463, 1.260067, -0.150213, 3.790713, 1.481266, -1.046058, -1.523667,
    -0.059621, 2.220780, 1.621212, 0.877230, 0.567247, -3.175944, -3.186688, 1.578665,
];
pub const T2_SHAPE_SLAT_STD: [f32; 32] = [
    5.972266, 4.706852, 5.445010, 5.209927, 5.320220, 4.547237, 5.020802, 5.444004,
    5.226681, 5.683095, 4.831436, 5.286469, 5.652043, 5.367606, 5.525084, 4.730578,
    4.805265, 5.124013, 5.530808, 5.619001, 5.103930, 5.417670, 5.269677, 5.547194,
    5.634698, 5.235274, 6.110351, 5.511298, 6.237273, 4.879207, 5.347008, 5.405691,
];
pub const T2_TEX_SLAT_MEAN: [f32; 32] = [
    3.501659, 2.212398, 2.226094, 0.251093, -0.026248, -0.687364, 0.439898, -0.928075,
    0.029398, -0.339596, -0.869527, 1.038479, -0.972385, 0.126042, -1.129303, 0.455149,
    -1.209521, 2.069067, 0.544735, 2.569128, -0.323407, 2.293000, -1.925608, -1.217717,
    1.213905, 0.971588, -0.023631, 0.106750, 2.021786, 0.250524, -0.662387, -0.768862,
];
pub const T2_TEX_SLAT_STD: [f32; 32] = [
    2.665652, 2.743913, 2.765121, 2.595319, 3.037293, 2.291316, 2.144656, 2.911822,
    2.969419, 2.501689, 2.154811, 3.163343, 2.621215, 2.381943, 3.186697, 3.021588,
    2.295916, 3.234985, 3.233086, 2.260140, 2.874801, 2.810596, 3.292720, 2.674999,
    2.680878, 2.372054, 2.451546, 2.353556, 2.995195, 2.379849, 2.786195, 2.775190,
];

// Sampler configs (pipeline.json).
#[derive(Clone, Copy, Debug)]
pub struct T2SamplerConfig {
    pub steps: usize,
    pub guidance_strength: f32,
    pub guidance_rescale: f32,
    pub guidance_interval: (f64, f64),
    pub rescale_t: f64,
}

pub const T2_SS_SAMPLER: T2SamplerConfig = T2SamplerConfig {
    steps: 12,
    guidance_strength: 7.5,
    guidance_rescale: 0.7,
    guidance_interval: (0.6, 1.0),
    rescale_t: 5.0,
};
pub const T2_SHAPE_SAMPLER: T2SamplerConfig = T2SamplerConfig {
    steps: 12,
    guidance_strength: 7.5,
    guidance_rescale: 0.5,
    guidance_interval: (0.6, 1.0),
    rescale_t: 3.0,
};
pub const T2_TEX_SAMPLER: T2SamplerConfig = T2SamplerConfig {
    steps: 12,
    guidance_strength: 1.0,
    guidance_rescale: 0.0,
    guidance_interval: (0.6, 0.9),
    rescale_t: 3.0,
};

// ---------------------------------------------------------------------------
// Single-checkpoint safetensors weights. TRELLIS ships one file per model
// (ss_flow / shape_flow / tex_flow / decoders / dinov3) with overlapping
// tensor names across models, so each file loads independently.
// ---------------------------------------------------------------------------

pub struct TrellisWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl TrellisWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path)
            .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
        Ok(Self { path, header })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.header.tensors.contains_key(name)
    }

    pub fn tensor_names(&self) -> impl Iterator<Item = &String> {
        self.header.tensors.keys()
    }

    pub fn tensor_dtype_shape(&self, name: &str) -> Result<(MlxDType, Vec<u64>)> {
        let entry = self.header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "trellis tensor '{name}' not found in {}",
                self.path.display()
            ))
        })?;
        Ok((entry.dtype, entry.shape.clone()))
    }

    /// Raw on-disk bytes of one tensor (seek + read).
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("trellis tensor '{name}': {err}")))
    }

    /// Whole tensor decoded to f32.
    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let (dtype, _shape) = self.tensor_dtype_shape(name)?;
        let bytes = self.tensor_bytes(name)?;
        match dtype {
            MlxDType::F32 => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            MlxDType::BF16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    f32::from_bits((word as u32) << 16)
                })
                .collect()),
            MlxDType::F16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    crate::h3::f16_word_to_f32(word)
                })
                .collect()),
            other => Err(DiffusionError::model(format!(
                "trellis tensor '{name}': unsupported dtype {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Flow Euler schedule (flow_euler.py): t_seq = linspace(1, 0, steps+1) in
// f64 (numpy), then t' = r*t / (1 + (r-1)*t) in f64. The model is called with
// 1000*t as f32; the Euler/CFG arithmetic runs on f32 tensors with f64
// python-scalar coefficients rounded at use.
// ---------------------------------------------------------------------------

pub fn t2_t_sequence(steps: usize, rescale_t: f64) -> Vec<f64> {
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = 1.0 - i as f64 / steps as f64;
        out.push(rescale_t * t / (1.0 + (rescale_t - 1.0) * t));
    }
    out
}

/// v -> x0 (flow_euler._v_to_xstart_eps, x0 half only):
/// `x0 = (1 - sigma_min) * x_t - (sigma_min + (1 - sigma_min) * t) * v`.
fn v_to_xstart(x_t: &[f32], v: &[f32], t: f32, out: &mut [f32]) {
    let a = 1.0 - T2_SIGMA_MIN;
    let b = T2_SIGMA_MIN + (1.0 - T2_SIGMA_MIN) * t;
    for ((o, x), vi) in out.iter_mut().zip(x_t).zip(v) {
        *o = a * *x - b * *vi;
    }
}

/// x0 -> v (flow_euler._xstart_to_pred):
/// `v = ((1 - sigma_min) * x_t - x0) / (sigma_min + (1 - sigma_min) * t)`.
fn xstart_to_v(x_t: &[f32], x0: &[f32], t: f32, out: &mut [f32]) {
    let a = 1.0 - T2_SIGMA_MIN;
    let b = T2_SIGMA_MIN + (1.0 - T2_SIGMA_MIN) * t;
    for ((o, x), x0i) in out.iter_mut().zip(x_t).zip(x0) {
        *o = (a * *x - *x0i) / b;
    }
}

/// Population or sample standard deviation over the whole slice.
/// Dense tensors go through torch's `.std()` (Bessel-corrected); the sparse
/// VarLenTensor.std is `sqrt(E[x^2] - E[x]^2)` (population).
fn std_all(values: &[f32], bessel: bool) -> f32 {
    let n = values.len() as f64;
    let mut sum = 0.0f64;
    let mut sum2 = 0.0f64;
    for v in values {
        sum += *v as f64;
        sum2 += (*v as f64) * (*v as f64);
    }
    let mean = sum / n;
    if bessel {
        (((sum2 - n * mean * mean) / (n - 1.0)).max(0.0)).sqrt() as f32
    } else {
        ((sum2 / n - mean * mean).max(0.0)).sqrt() as f32
    }
}

/// Classifier-free guidance + optional CFG rescale, mirroring
/// classifier_free_guidance_mixin._inference_model exactly. Returns the
/// blended velocity in `pred_pos` (consumed in place).
pub fn t2_cfg_combine(
    x_t: &[f32],
    pred_pos: &mut Vec<f32>,
    pred_neg: &[f32],
    t: f32,
    guidance_strength: f32,
    guidance_rescale: f32,
    dense_std: bool,
) -> Result<()> {
    if pred_pos.len() != pred_neg.len() || pred_pos.len() != x_t.len() {
        return Err(DiffusionError::workflow("t2 cfg length mismatch"));
    }
    let g = guidance_strength;
    let mut pred: Vec<f32> = pred_pos
        .iter()
        .zip(pred_neg)
        .map(|(p, n)| g * *p + (1.0 - g) * *n)
        .collect();
    if guidance_rescale > 0.0 {
        let mut x0_pos = vec![0.0f32; x_t.len()];
        let mut x0_cfg = vec![0.0f32; x_t.len()];
        v_to_xstart(x_t, pred_pos, t, &mut x0_pos);
        v_to_xstart(x_t, &pred, t, &mut x0_cfg);
        let std_pos = std_all(&x0_pos, dense_std);
        let std_cfg = std_all(&x0_cfg, dense_std);
        let ratio = std_pos / std_cfg;
        let r = guidance_rescale;
        for (cfg, _pos) in x0_cfg.iter_mut().zip(&x0_pos) {
            let rescaled = *cfg * ratio;
            *cfg = r * rescaled + (1.0 - r) * *cfg;
        }
        xstart_to_v(x_t, &x0_cfg, t, &mut pred);
    }
    *pred_pos = pred;
    Ok(())
}

/// One Euler step: `x -= (t - t_prev) * v`, f32 with the f64 dt rounded once.
pub fn t2_euler_step(sample: &mut [f32], velocity: &[f32], t: f64, t_prev: f64) -> Result<()> {
    if sample.len() != velocity.len() {
        return Err(DiffusionError::workflow("t2 euler step length mismatch"));
    }
    let dt = (t - t_prev) as f32;
    for (x, v) in sample.iter_mut().zip(velocity) {
        *x -= dt * *v;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 3D rope phase tables (modules/attention/rope.py + sparse/attention/rope.py):
// 21 f32 frequencies per axis, angle = coord * freq, layout per token
// [x*f0..x*f20 | y*f0..y*f20 | z*f0..z*f20 | identity pad], 64 pairs total,
// applied to INTERLEAVED value pairs (torch.view_as_complex convention =
// gpu_rope_interleaved).
// ---------------------------------------------------------------------------

pub struct T2RopeTables {
    pub rows: usize,
    /// (rows, 64) row-major.
    pub cos: Vec<f32>,
    pub sin: Vec<f32>,
}

pub fn t2_rope_freqs() -> [f32; T2_ROPE_FREQ_DIM] {
    let mut freqs = [0.0f32; T2_ROPE_FREQ_DIM];
    for (j, value) in freqs.iter_mut().enumerate() {
        let f = j as f32 / T2_ROPE_FREQ_DIM as f32;
        *value = 1.0 / T2_ROPE_THETA.powf(f);
    }
    freqs
}

/// Phase tables for explicit integer coordinates (SLAT tokens use the raw
/// voxel coords; the SS model uses the dense 16^3 meshgrid below).
pub fn t2_rope_tables(coords: &[[i32; 3]]) -> T2RopeTables {
    let freqs = t2_rope_freqs();
    let rows = coords.len();
    let mut cos = vec![1.0f32; rows * T2_ROPE_PAIRS];
    let mut sin = vec![0.0f32; rows * T2_ROPE_PAIRS];
    for (row, coord) in coords.iter().enumerate() {
        for axis in 0..3 {
            let pos = coord[axis] as f32;
            for (j, freq) in freqs.iter().enumerate() {
                let angle = pos * freq;
                let col = axis * T2_ROPE_FREQ_DIM + j;
                cos[row * T2_ROPE_PAIRS + col] = angle.cos();
                sin[row * T2_ROPE_PAIRS + col] = angle.sin();
            }
        }
    }
    T2RopeTables { rows, cos, sin }
}

/// The SS model's dense grid coords: meshgrid(arange(16)^3, indexing='ij')
/// flattened — token index = (x*16 + y)*16 + z.
pub fn t2_ss_grid_coords() -> Vec<[i32; 3]> {
    let r = T2_SS_RESOLUTION as i32;
    let mut coords = Vec::with_capacity(T2_SS_TOKENS);
    for x in 0..r {
        for y in 0..r {
            for z in 0..r {
                coords.push([x, y, z]);
            }
        }
    }
    coords
}

// ---------------------------------------------------------------------------
// Sparse-structure decode helpers: threshold + max-pool + argwhere in the
// reference's lexicographic order.
// ---------------------------------------------------------------------------

/// occupancy: `logits > 0` on a dense (res, res, res) grid (row-major x, y, z),
/// optionally max-pooled down to `target_res`, then argwhere in lex (x, y, z)
/// order — exactly `torch.argwhere(decoded)[:, [0, 2, 3, 4]]` for batch 1.
pub fn t2_occupancy_coords(logits: &[f32], res: usize, target_res: usize) -> Result<Vec<[i32; 3]>> {
    if logits.len() != res * res * res || target_res > res || res % target_res != 0 {
        return Err(DiffusionError::workflow("t2 occupancy shape mismatch"));
    }
    let ratio = res / target_res;
    let mut coords = Vec::new();
    for x in 0..target_res {
        for y in 0..target_res {
            for z in 0..target_res {
                let mut occupied = false;
                'scan: for dx in 0..ratio {
                    for dy in 0..ratio {
                        for dz in 0..ratio {
                            let index =
                                ((x * ratio + dx) * res + (y * ratio + dy)) * res + (z * ratio + dz);
                            if logits[index] > 0.0 {
                                occupied = true;
                                break 'scan;
                            }
                        }
                    }
                }
                if occupied {
                    coords.push([x as i32, y as i32, z as i32]);
                }
            }
        }
    }
    Ok(coords)
}

/// Cascade coord quantization: `((hr + 0.5) / lr_res * (hr_res / 16)).int()`
/// then unique(dim=0) — int() truncates toward zero, unique sorts lex.
pub fn t2_quantize_unique_coords(
    hr_coords: &[[i32; 3]],
    lr_resolution: usize,
    hr_resolution: usize,
) -> Vec<[i32; 3]> {
    let scale = (hr_resolution / 16) as f64;
    let lr = lr_resolution as f64;
    let mut out: Vec<[i32; 3]> = hr_coords
        .iter()
        .map(|c| {
            [
                ((c[0] as f64 + 0.5) / lr * scale) as i32,
                ((c[1] as f64 + 0.5) / lr * scale) as i32,
                ((c[2] as f64 + 0.5) / lr * scale) as i32,
            ]
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_sequence_matches_reference() {
        // steps=12, rescale 5: t0 = 1, t12 = 0; t6 (t=0.5) = 5*0.5/(1+4*0.5) = 5/6.
        let seq = t2_t_sequence(12, 5.0);
        assert_eq!(seq.len(), 13);
        assert!((seq[0] - 1.0).abs() < 1e-15);
        assert!((seq[6] - 5.0 / 6.0).abs() < 1e-12);
        assert_eq!(seq[12], 0.0);
        // rescale 3, t = 2/3: 3*(2/3)/(1+2*(2/3)) = 2/(7/3) = 6/7.
        let seq = t2_t_sequence(3, 3.0);
        assert!((seq[1] - 6.0 / 7.0).abs() < 1e-12);
    }

    #[test]
    fn cfg_plain_combine() {
        let x = vec![0.0f32; 4];
        let mut pos = vec![1.0f32, 2.0, 3.0, 4.0];
        let neg = vec![0.0f32; 4];
        t2_cfg_combine(&x, &mut pos, &neg, 0.9, 7.5, 0.0, true).unwrap();
        assert_eq!(pos, vec![7.5, 15.0, 22.5, 30.0]);
    }

    #[test]
    fn cfg_rescale_restores_positive_std() {
        // With x_t = 0, x0 = -b*v, so std(x0) proportional to std(v):
        // rescale=1 forces std(v_cfg) back to std(v_pos).
        let x = vec![0.0f32; 4];
        let mut pos = vec![1.0f32, -1.0, 1.0, -1.0];
        let neg = vec![0.0f32; 4];
        t2_cfg_combine(&x, &mut pos, &neg, 0.9, 7.5, 1.0, true).unwrap();
        let std = super::std_all(&pos, true);
        assert!((std - 1.1547005) < 1e-3, "std {std}"); // std of ±1 with Bessel
    }

    #[test]
    fn euler_step_math() {
        let mut x = vec![1.0f32, 2.0];
        t2_euler_step(&mut x, &[1.0, 1.0], 0.5, 0.25).unwrap();
        assert_eq!(x, vec![0.75, 1.75]);
    }

    #[test]
    fn rope_layout() {
        let coords = vec![[0, 0, 0], [1, 0, 0], [0, 0, 2]];
        let tables = t2_rope_tables(&coords);
        assert_eq!(tables.rows, 3);
        assert_eq!(tables.cos.len(), 3 * 64);
        // Token 0: identity everywhere.
        assert!(tables.cos[..64].iter().all(|v| *v == 1.0));
        assert!(tables.sin[..64].iter().all(|v| *v == 0.0));
        // Token 1 (x=1): pair 0 angle = 1 rad; y/z pairs identity; pad identity.
        assert!((tables.cos[64] - 1.0f32.cos()).abs() < 1e-7);
        assert!((tables.sin[64] - 1.0f32.sin()).abs() < 1e-7);
        assert_eq!(tables.cos[64 + 21], 1.0);
        assert_eq!(tables.cos[64 + 63], 1.0);
        // Token 2 (z=2): z block pair 0 = 2 rad.
        assert!((tables.sin[2 * 64 + 42] - 2.0f32.sin()).abs() < 1e-7);
        // Frequencies decay: freq[20] = 10000^(-20/21).
        let freqs = t2_rope_freqs();
        assert!((freqs[20] - (1.0 / 10000.0f32.powf(20.0 / 21.0))).abs() < 1e-10);
    }

    #[test]
    fn ss_grid_token_order() {
        let coords = t2_ss_grid_coords();
        assert_eq!(coords.len(), 4096);
        assert_eq!(coords[0], [0, 0, 0]);
        assert_eq!(coords[1], [0, 0, 1]); // z fastest
        assert_eq!(coords[16], [0, 1, 0]);
        assert_eq!(coords[256], [1, 0, 0]);
    }

    #[test]
    fn occupancy_and_pool() {
        // 4^3 grid, one positive logit at (2, 1, 3) -> pooled 2^3 coord (1, 0, 1).
        let mut logits = vec![-1.0f32; 64];
        logits[(2 * 4 + 1) * 4 + 3] = 1.0;
        let coords = t2_occupancy_coords(&logits, 4, 4).unwrap();
        assert_eq!(coords, vec![[2, 1, 3]]);
        let pooled = t2_occupancy_coords(&logits, 4, 2).unwrap();
        assert_eq!(pooled, vec![[1, 0, 1]]);
    }

    #[test]
    fn quantize_unique() {
        // lr res 512 coords at 512 grid -> 1024/16 = 64 grid.
        let coords = vec![[511, 0, 8], [508, 1, 8], [0, 0, 0]];
        let out = t2_quantize_unique_coords(&coords, 512, 1024);
        assert_eq!(out, vec![[0, 0, 0], [63, 0, 1]]);
    }
}
