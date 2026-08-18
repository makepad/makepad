//! IndexTTS-2.5 semantic codec — DECODE path only (semantic codes -> 1024-d
//! feature frames), CPU f32.
//!
//! Reference: `indextts/codec/models.py` `EnhancedCodec.decode` with the
//! config.yaml `semantic_codec` section (codebook 8192x8, hidden 1024,
//! vocos dim 384 / intermediate 2048 / 12 layers, downsample_scale 2):
//!
//!   codes [T] -> quantizer.vq2emb:
//!       codebook Embedding(8192, 8) lookup
//!       -> out_project weight-normed Conv1d 1x1 (8 -> 1024)
//!   -> decoder VocosBackbone:
//!       embed Conv1d k7 p3 (1024 -> 384); LayerNorm eps 1e-6
//!       12 x ConvNeXtBlock(depthwise k7 p3 groups=384, LayerNorm eps 1e-6,
//!                          pwconv1 384->2048, exact-erf GELU, pwconv2,
//!                          gamma layer-scale, residual)
//!       final LayerNorm eps 1e-6
//!   -> Linear 384 -> 1024
//!   -> nearest 2x upsample along time -> up Conv1d k3 p1 (1024 -> 1024)
//!   => [2T, 1024] time-major.
//!
//! Weights: `codec.pth` via [`PthStateDict::load_nested`], keys under
//! `model.` (encoder/down keys are ignored — decode only). The out_project
//! weight-norm pair `weight_g`/`weight_v` is folded at load (norm per OUT
//! channel over in*k, the `moss_dac::norm_weight_conv` convention).
//!
//! Oracle: dumps/semantic_codes.npy (1,92) -> dumps/s_infer.npy (1,184,1024),
//! validated by `indextts-s2mel-validate --stage codec`.

use std::path::Path;

use crate::error::{DiffusionError, Result};
use crate::sa3::{linear, par_rows};
use crate::torch_pth::PthStateDict;
use crate::woosh::gelu_erf;

/// Codebook entries (config `semantic_codec.codebook_size`).
pub const SEMANTIC_CODEC_CODEBOOK: usize = 8192;
/// Factorized codebook dim.
const CODEBOOK_DIM: usize = 8;
/// Output feature dim (config `semantic_codec.hidden_size`).
pub const SEMANTIC_CODEC_DIM: usize = 1024;
/// Vocos backbone width / intermediate / depth.
const VOCOS_DIM: usize = 384;
const VOCOS_INTERMEDIATE: usize = 2048;
const VOCOS_LAYERS: usize = 12;
const LN_EPS: f32 = 1e-6;

// ---------------------------------------------------------------------------
// Small building blocks (single batch; planes are channel-major [ch, len],
// row matrices are time-major [len, ch]).
// ---------------------------------------------------------------------------

/// Plain (non-grouped) Conv1d, weight (out, in, k) materialized, zero padding.
struct Conv1d {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    padding: usize,
}

impl Conv1d {
    /// x: [in_ch, len] -> [out_ch, len] (length-preserving: 2*padding == k-1).
    fn forward(&self, x: &[f32], len: usize) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.in_ch * len);
        debug_assert_eq!(2 * self.padding, self.k - 1);
        let mut out = vec![0f32; self.out_ch * len];
        par_rows(&mut out, len, &|o, row| {
            row.fill(self.b[o]);
            for i in 0..self.in_ch {
                let x_row = &x[i * len..(i + 1) * len];
                let w_row = &self.w[(o * self.in_ch + i) * self.k..][..self.k];
                for (kk, &w) in w_row.iter().enumerate() {
                    let off = kk as isize - self.padding as isize;
                    let t0 = (-off).max(0) as usize;
                    let t1 = ((len as isize - off).min(len as isize)).max(0) as usize;
                    if t1 <= t0 {
                        continue;
                    }
                    let x_slice = &x_row[(t0 as isize + off) as usize..(t1 as isize + off) as usize];
                    for (acc, &xv) in row[t0..t1].iter_mut().zip(x_slice) {
                        *acc += w * xv;
                    }
                }
            }
        });
        out
    }
}

/// LayerNorm over the channel axis of a time-major [len, ch] matrix.
struct LayerNorm {
    gamma: Vec<f32>,
    beta: Vec<f32>,
}

impl LayerNorm {
    fn apply(&self, x: &mut [f32], width: usize) {
        debug_assert_eq!(width, self.gamma.len());
        for row in x.chunks_mut(width) {
            let mut mean = 0f32;
            for v in row.iter() {
                mean += v;
            }
            mean /= width as f32;
            let mut var = 0f32;
            for v in row.iter() {
                let d = v - mean;
                var += d * d;
            }
            var /= width as f32;
            let inv = 1.0 / (var + LN_EPS).sqrt();
            for (i, v) in row.iter_mut().enumerate() {
                *v = (*v - mean) * inv * self.gamma[i] + self.beta[i];
            }
        }
    }
}

struct ConvNeXtBlock {
    /// Depthwise conv weight (ch, 1, 7) flattened to ch*7, plus bias.
    dw_w: Vec<f32>,
    dw_b: Vec<f32>,
    norm: LayerNorm,
    pw1_w: Vec<f32>, // (2048, 384)
    pw1_b: Vec<f32>,
    pw2_w: Vec<f32>, // (384, 2048)
    pw2_b: Vec<f32>,
    gamma: Vec<f32>, // layer scale (384)
}

impl ConvNeXtBlock {
    /// x: plane [VOCOS_DIM, len]; returns residual-added plane.
    fn forward(&self, x: &[f32], len: usize) -> Vec<f32> {
        // Depthwise conv k7 p3 (groups == channels).
        let mut dw = vec![0f32; VOCOS_DIM * len];
        par_rows(&mut dw, len, &|c, row| {
            row.fill(self.dw_b[c]);
            let x_row = &x[c * len..(c + 1) * len];
            let w_row = &self.dw_w[c * 7..(c + 1) * 7];
            for (kk, &w) in w_row.iter().enumerate() {
                let off = kk as isize - 3;
                let t0 = (-off).max(0) as usize;
                let t1 = ((len as isize - off).min(len as isize)).max(0) as usize;
                if t1 <= t0 {
                    continue;
                }
                let x_slice = &x_row[(t0 as isize + off) as usize..(t1 as isize + off) as usize];
                for (acc, &xv) in row[t0..t1].iter_mut().zip(x_slice) {
                    *acc += w * xv;
                }
            }
        });
        // (ch, len) -> (len, ch), LayerNorm, MLP, layer scale.
        let mut rows = transpose(&dw, VOCOS_DIM, len);
        self.norm.apply(&mut rows, VOCOS_DIM);
        let mut mid = linear(&rows, &self.pw1_w, Some(&self.pw1_b), len, VOCOS_DIM, VOCOS_INTERMEDIATE);
        for v in mid.iter_mut() {
            *v = gelu_erf(*v);
        }
        let mut rows = linear(&mid, &self.pw2_w, Some(&self.pw2_b), len, VOCOS_INTERMEDIATE, VOCOS_DIM);
        for row in rows.chunks_mut(VOCOS_DIM) {
            for (v, &g) in row.iter_mut().zip(&self.gamma) {
                *v *= g;
            }
        }
        // (len, ch) -> (ch, len) + residual.
        let mut out = transpose(&rows, len, VOCOS_DIM);
        for (o, &r) in out.iter_mut().zip(x) {
            *o += r;
        }
        out
    }
}

/// [rows, cols] -> [cols, rows].
fn transpose(x: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    debug_assert_eq!(x.len(), rows * cols);
    let mut out = vec![0f32; x.len()];
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = x[r * cols + c];
        }
    }
    out
}

/// torch weight_norm fold: w = g * v / ||v||, norm per row (dim 0) over `per`
/// trailing elements.
fn fold_weight_norm(g: &[f32], v: &[f32], rows: usize, per: usize) -> Vec<f32> {
    debug_assert_eq!(g.len(), rows);
    debug_assert_eq!(v.len(), rows * per);
    let mut w = vec![0f32; rows * per];
    for r in 0..rows {
        let src = &v[r * per..(r + 1) * per];
        let mut sum = 0f64;
        for &x in src {
            sum += x as f64 * x as f64;
        }
        let scale = g[r] as f64 / sum.sqrt().max(1e-30);
        for (dst, &x) in w[r * per..(r + 1) * per].iter_mut().zip(src) {
            *dst = (x as f64 * scale) as f32;
        }
    }
    w
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

pub struct SemanticCodecDecoder {
    /// Embedding (8192, 8).
    codebook: Vec<f32>,
    /// out_project folded weight (1024, 8) + bias.
    out_project_w: Vec<f32>,
    out_project_b: Vec<f32>,
    embed: Conv1d, // k7 p3 1024 -> 384
    norm: LayerNorm,
    blocks: Vec<ConvNeXtBlock>,
    final_norm: LayerNorm,
    out_w: Vec<f32>, // Linear (1024, 384)
    out_b: Vec<f32>,
    up: Conv1d, // k3 p1 1024 -> 1024
    /// Device path, built at load when a CUDA device is present. A build
    /// failure fails the load loudly — there is no silent CPU fallback on
    /// CUDA machines.
    cuda_session: Option<cuda::CodecCudaSession>,
}

impl SemanticCodecDecoder {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let mut sd = PthStateDict::load_nested(path)?;
        let ln = |sd: &mut PthStateDict, prefix: &str, ch: usize| -> Result<LayerNorm> {
            Ok(LayerNorm {
                gamma: sd.f32_shaped(&format!("{prefix}.weight"), &[ch])?,
                beta: sd.f32_shaped(&format!("{prefix}.bias"), &[ch])?,
            })
        };
        let codebook = sd.f32_shaped(
            "model.quantizer.quantizers.0.codebook.weight",
            &[SEMANTIC_CODEC_CODEBOOK, CODEBOOK_DIM],
        )?;
        let g = sd.f32_shaped(
            "model.quantizer.quantizers.0.out_project.weight_g",
            &[SEMANTIC_CODEC_DIM, 1, 1],
        )?;
        let v = sd.f32_shaped(
            "model.quantizer.quantizers.0.out_project.weight_v",
            &[SEMANTIC_CODEC_DIM, CODEBOOK_DIM, 1],
        )?;
        let out_project_w = fold_weight_norm(&g, &v, SEMANTIC_CODEC_DIM, CODEBOOK_DIM);
        let out_project_b =
            sd.f32_shaped("model.quantizer.quantizers.0.out_project.bias", &[SEMANTIC_CODEC_DIM])?;

        let embed = Conv1d {
            w: sd.f32_shaped("model.decoder.0.embed.weight", &[VOCOS_DIM, SEMANTIC_CODEC_DIM, 7])?,
            b: sd.f32_shaped("model.decoder.0.embed.bias", &[VOCOS_DIM])?,
            out_ch: VOCOS_DIM,
            in_ch: SEMANTIC_CODEC_DIM,
            k: 7,
            padding: 3,
        };
        let norm = ln(&mut sd, "model.decoder.0.norm", VOCOS_DIM)?;
        let mut blocks = Vec::with_capacity(VOCOS_LAYERS);
        for i in 0..VOCOS_LAYERS {
            let p = format!("model.decoder.0.convnext.{i}");
            blocks.push(ConvNeXtBlock {
                dw_w: sd.f32_shaped(&format!("{p}.dwconv.weight"), &[VOCOS_DIM, 1, 7])?,
                dw_b: sd.f32_shaped(&format!("{p}.dwconv.bias"), &[VOCOS_DIM])?,
                norm: ln(&mut sd, &format!("{p}.norm"), VOCOS_DIM)?,
                pw1_w: sd.f32_shaped(&format!("{p}.pwconv1.weight"), &[VOCOS_INTERMEDIATE, VOCOS_DIM])?,
                pw1_b: sd.f32_shaped(&format!("{p}.pwconv1.bias"), &[VOCOS_INTERMEDIATE])?,
                pw2_w: sd.f32_shaped(&format!("{p}.pwconv2.weight"), &[VOCOS_DIM, VOCOS_INTERMEDIATE])?,
                pw2_b: sd.f32_shaped(&format!("{p}.pwconv2.bias"), &[VOCOS_DIM])?,
                gamma: sd.f32_shaped(&format!("{p}.gamma"), &[VOCOS_DIM])?,
            });
        }
        let final_norm = ln(&mut sd, "model.decoder.0.final_layer_norm", VOCOS_DIM)?;
        let out_w = sd.f32_shaped("model.decoder.1.weight", &[SEMANTIC_CODEC_DIM, VOCOS_DIM])?;
        let out_b = sd.f32_shaped("model.decoder.1.bias", &[SEMANTIC_CODEC_DIM])?;
        let up = Conv1d {
            w: sd.f32_shaped("model.up.weight", &[SEMANTIC_CODEC_DIM, SEMANTIC_CODEC_DIM, 3])?,
            b: sd.f32_shaped("model.up.bias", &[SEMANTIC_CODEC_DIM])?,
            out_ch: SEMANTIC_CODEC_DIM,
            in_ch: SEMANTIC_CODEC_DIM,
            k: 3,
            padding: 1,
        };
        let mut model = Self {
            codebook,
            out_project_w,
            out_project_b,
            embed,
            norm,
            blocks,
            final_norm,
            out_w,
            out_b,
            up,
            cuda_session: None,
        };
        if cuda::codec_cuda_available() {
            model.cuda_session = Some(cuda::CodecCudaSession::new(&model)?);
        }
        Ok(model)
    }

    /// codes (`[T]`, values < 8192) -> time-major features `[2*T, 1024]`.
    /// Dispatches to the device path when present (parity-gated by
    /// indextts_cuda_validate); CPU reference otherwise.
    pub fn decode(&self, codes: &[u32]) -> Result<Vec<f32>> {
        self.validate_codes(codes)?;
        if let Some(session) = &self.cuda_session {
            return session.decode(self, codes);
        }
        self.decode_cpu(codes)
    }

    /// The CPU f32 reference path (also the parity baseline the CUDA gate
    /// compares against).
    pub fn decode_cpu(&self, codes: &[u32]) -> Result<Vec<f32>> {
        self.decode_traced(codes, &mut |_, _| {})
    }

    /// Whether [`Self::decode`] dispatches to the device path (used by the
    /// validator to run the explicit CUDA-vs-CPU gate).
    pub fn cuda_active(&self) -> bool {
        self.cuda_session.is_some()
    }

    fn validate_codes(&self, codes: &[u32]) -> Result<()> {
        if codes.is_empty() {
            return Err(DiffusionError::model("semantic codec decode: empty codes"));
        }
        for &c in codes {
            if c as usize >= SEMANTIC_CODEC_CODEBOOK {
                return Err(DiffusionError::model(format!(
                    "semantic codec decode: code {c} out of range"
                )));
            }
        }
        Ok(())
    }

    /// [`Self::decode`] emitting intermediates to `trace` (name, values) for
    /// stage-by-stage oracle bisection; layouts match the python dumps
    /// (planes channel-major, `backbone`/`decoded` time-major).
    #[doc(hidden)]
    pub fn decode_traced(
        &self,
        codes: &[u32],
        trace: &mut dyn FnMut(&str, &[f32]),
    ) -> Result<Vec<f32>> {
        let t = codes.len();
        if t == 0 {
            return Err(DiffusionError::model("semantic codec decode: empty codes"));
        }
        for &c in codes {
            if c as usize >= SEMANTIC_CODEC_CODEBOOK {
                return Err(DiffusionError::model(format!(
                    "semantic codec decode: code {c} out of range"
                )));
            }
        }
        // vq2emb: codebook lookup (t, 8) then out_project 1x1 conv == linear.
        let mut emb = vec![0f32; t * CODEBOOK_DIM];
        for (i, &c) in codes.iter().enumerate() {
            let row = &self.codebook[c as usize * CODEBOOK_DIM..(c as usize + 1) * CODEBOOK_DIM];
            emb[i * CODEBOOK_DIM..(i + 1) * CODEBOOK_DIM].copy_from_slice(row);
        }
        let quantized = linear(
            &emb,
            &self.out_project_w,
            Some(&self.out_project_b),
            t,
            CODEBOOK_DIM,
            SEMANTIC_CODEC_DIM,
        ); // (t, 1024) time-major
        let plane = transpose(&quantized, t, SEMANTIC_CODEC_DIM); // (1024, t)
        trace("quantized", &plane);

        // VocosBackbone.
        let plane = self.embed.forward(&plane, t); // (384, t)
        trace("embed", &plane);
        let mut rows = transpose(&plane, VOCOS_DIM, t); // (t, 384)
        self.norm.apply(&mut rows, VOCOS_DIM);
        let mut plane = transpose(&rows, t, VOCOS_DIM);
        trace("norm", &plane);
        for (i, block) in self.blocks.iter().enumerate() {
            plane = block.forward(&plane, t);
            if i == 0 {
                trace("block0", &plane);
            }
        }
        let mut rows = transpose(&plane, VOCOS_DIM, t);
        self.final_norm.apply(&mut rows, VOCOS_DIM);
        trace("backbone", &rows);
        let rows = linear(&rows, &self.out_w, Some(&self.out_b), t, VOCOS_DIM, SEMANTIC_CODEC_DIM);
        trace("decoded", &rows);

        // Nearest 2x upsample along time, then up conv k3 p1.
        let plane = transpose(&rows, t, SEMANTIC_CODEC_DIM); // (1024, t)
        let out_len = 2 * t;
        let mut up_in = vec![0f32; SEMANTIC_CODEC_DIM * out_len];
        for c in 0..SEMANTIC_CODEC_DIM {
            let src = &plane[c * t..(c + 1) * t];
            let dst = &mut up_in[c * out_len..(c + 1) * out_len];
            for (i, v) in dst.iter_mut().enumerate() {
                *v = src[i / 2];
            }
        }
        let plane = self.up.forward(&up_in, out_len); // (1024, 2t)
        Ok(transpose(&plane, SEMANTIC_CODEC_DIM, out_len)) // (2t, 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gelu_is_exact_erf() {
        // torch nn.GELU() (approximate='none') reference values.
        assert!((gelu_erf(1.0) - 0.841_344_7).abs() < 1e-6);
        assert!((gelu_erf(-0.5) - -0.154_268_8).abs() < 1e-6);
        assert!((gelu_erf(0.0)).abs() < 1e-9);
        // Distinct from the tanh approximation at |x| ~ 2.
        assert!((gelu_erf(2.0) - 1.954_499_7).abs() < 1e-6);
    }

    #[test]
    fn nearest_2x_upsample_matches_hand_computed() {
        // The decode() upsample uses src[i/2]; F.interpolate(scale_factor=2,
        // mode='nearest') maps output i -> input floor(i/2).
        let src = [1.0f32, 2.0, 3.0];
        let out: Vec<f32> = (0..6).map(|i| src[i / 2]).collect();
        assert_eq!(out, vec![1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
    }

    #[test]
    fn weight_norm_fold_normalizes_rows() {
        // v row (3, 4): w = g * v/||v||, so ||w_row|| == g_row.
        let v = [3.0f32, 4.0, 0.0, 0.0, 1.0, 1.0];
        let g = [2.0f32, 5.0];
        let w = fold_weight_norm(&g, &v, 2, 3);
        let n0 = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
        let n1 = (w[3] * w[3] + w[4] * w[4] + w[5] * w[5]).sqrt();
        assert!((n0 - 2.0).abs() < 1e-6);
        assert!((n1 - 5.0).abs() < 1e-6);
        assert!((w[0] - 1.2).abs() < 1e-6); // 2 * 3/5
    }

    #[test]
    fn layer_norm_zero_mean_unit_var() {
        let ln = LayerNorm {
            gamma: vec![1.0; 4],
            beta: vec![0.0; 4],
        };
        let mut x = vec![1.0f32, 2.0, 3.0, 4.0];
        ln.apply(&mut x, 4);
        let mean: f32 = x.iter().sum::<f32>() / 4.0;
        let var: f32 = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / 4.0;
        assert!(mean.abs() < 1e-6);
        assert!((var - 1.0).abs() < 1e-3);
    }

    /// Checkpoint smoke: decode a few codes end to end (skipped when the
    /// reference checkout is absent).
    #[test]
    fn decode_smoke_on_reference_checkpoint() {
        let path = crate::indextts::reference_checkpoints_dir().join("codec.pth");
        if !path.is_file() {
            eprintln!("skipping decode_smoke_on_reference_checkpoint: {path:?} missing");
            return;
        }
        let decoder = SemanticCodecDecoder::load(&path).unwrap();
        let out = decoder.decode(&[0, 375, 7956, 42]).unwrap();
        assert_eq!(out.len(), 8 * SEMANTIC_CODEC_DIM);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}

#[path = "indextts_codec_cuda.rs"]
pub mod cuda;
