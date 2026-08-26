//! CUDA path for the IndexTTS-2.5 semantic codec decoder — the whole Vocos
//! backbone on device, zero mid-network host round trips.
//!
//! Layout: everything stays time-major `[t, ch]` rows (the codec's natural
//! output layout, so no transposes anywhere):
//!
//! - vq2emb (codebook lookup + 1x1 out_project, ~0.1% of the flops) runs on
//!   the host and uploads `[t, 1024]`.
//! - embed / up convs: the shared im2col gather+GEMM plumbing from
//!   `indextts_bigvgan_cuda.rs`.
//! - ConvNeXt depthwise k7 conv: same conv GEMM with a block-diagonal
//!   `[384, 7*384]` weight (only the own-channel tap per block is nonzero —
//!   wasteful in flops but trivial at codec sizes and needs no new op).
//! - LayerNorm: `gpu_layer_norm_pytorch` (exact PyTorch Welford LN, eps
//!   inside the sqrt — matches the CPU `LayerNorm::apply`).
//! - GELU: `gpu_gelu_erf` (exact erf, same as the CPU `gelu_erf`).
//! - Layer-scale gamma folds into the pwconv2 weight rows and bias at
//!   session build, residual is `gpu_add`.
//! - Nearest-2x time upsample: row gather `idx[n] = n/2`.
//!
//! Parity is gated by `indextts_cuda_validate --stage codec`: CUDA vs CPU
//! and CUDA vs the official `s_infer.npy` oracle, both cosine-gated.

use super::*;
use crate::backend::{
    gpu_add, gpu_device_available, gpu_download, gpu_gather_rows_colblock, gpu_gelu_erf,
    gpu_layer_norm_pytorch, gpu_upload, gpu_upload_u32, GpuTensor,
};
use crate::indextts_bigvgan::cuda::{conv_gemm, ensure_weight, gm, lin, tap_indices};
use std::collections::HashMap;
use std::sync::Mutex;

const NS: &str = "indextts_codec";

/// True when the CUDA device path is available in this build.
pub fn codec_cuda_available() -> bool {
    gpu_device_available()
}

/// Codec Conv1d weight `(out, in, k)` packed to GEMM rows `[out, k*in]`
/// kk-major (the codec `Conv1d` struct differs from the bigvgan one).
fn pack_conv(conv: &Conv1d) -> Vec<f32> {
    let (out_ch, in_ch, k) = (conv.out_ch, conv.in_ch, conv.k);
    let mut wk = vec![0f32; out_ch * k * in_ch];
    for o in 0..out_ch {
        for i in 0..in_ch {
            for kk in 0..k {
                wk[o * k * in_ch + kk * in_ch + i] = conv.w[(o * in_ch + i) * k + kk];
            }
        }
    }
    wk
}

/// Per-length device index tensors (tiny at codec sizes; cached by t).
struct CodecRunIdx {
    /// k7 p3 tap gathers at length t (embed and every depthwise conv — tap
    /// row maps are width-independent).
    taps7: Vec<GpuTensor>,
    /// k3 p1 tap gathers at length 2t (up conv).
    taps3: Vec<GpuTensor>,
    /// Nearest-2x upsample row map `[2t]`, `idx[n] = n/2`.
    up_gather: GpuTensor,
    /// Sentinel zero rows for the two operand widths.
    zero_1024: GpuTensor,
    zero_384: GpuTensor,
}

impl CodecRunIdx {
    fn build(t: usize) -> Result<Self> {
        let upload_taps = |maps: Vec<Vec<u32>>| -> Result<Vec<GpuTensor>> {
            maps.into_iter()
                .map(|m| gpu_upload_u32(&m).map_err(gm))
                .collect()
        };
        let up_map: Vec<u32> = (0..2 * t).map(|n| (n / 2) as u32).collect();
        Ok(Self {
            taps7: upload_taps(tap_indices(t, 7, 1, 3))?,
            taps3: upload_taps(tap_indices(2 * t, 3, 1, 1))?,
            up_gather: gpu_upload_u32(&up_map).map_err(gm)?,
            zero_1024: gpu_upload(&vec![0f32; SEMANTIC_CODEC_DIM], 1, SEMANTIC_CODEC_DIM)
                .map_err(gm)?,
            zero_384: gpu_upload(&vec![0f32; VOCOS_DIM], 1, VOCOS_DIM).map_err(gm)?,
        })
    }
}

/// Weight-cache handle + per-length index cache; holds the gamma-folded
/// pwconv2 biases (the only host-side data the device path needs beyond the
/// model itself).
pub struct CodecCudaSession {
    idx: Mutex<HashMap<usize, CodecRunIdx>>,
    pw2_bias_folded: Vec<Vec<f32>>,
}

impl CodecCudaSession {
    pub fn new(model: &SemanticCodecDecoder) -> Result<Self> {
        ensure_weight(
            NS,
            "embed",
            &pack_conv(&model.embed),
            VOCOS_DIM,
            7 * SEMANTIC_CODEC_DIM,
        )?;
        let mut pw2_bias_folded = Vec::with_capacity(model.blocks.len());
        for (i, block) in model.blocks.iter().enumerate() {
            let mut dw = vec![0f32; VOCOS_DIM * 7 * VOCOS_DIM];
            for o in 0..VOCOS_DIM {
                for kk in 0..7 {
                    dw[o * 7 * VOCOS_DIM + kk * VOCOS_DIM + o] = block.dw_w[o * 7 + kk];
                }
            }
            ensure_weight(NS, &format!("blk{i}.dw"), &dw, VOCOS_DIM, 7 * VOCOS_DIM)?;
            ensure_weight(
                NS,
                &format!("blk{i}.pw1"),
                &block.pw1_w,
                VOCOS_INTERMEDIATE,
                VOCOS_DIM,
            )?;
            let mut pw2 = vec![0f32; VOCOS_DIM * VOCOS_INTERMEDIATE];
            for o in 0..VOCOS_DIM {
                let g = block.gamma[o];
                let src = &block.pw2_w[o * VOCOS_INTERMEDIATE..][..VOCOS_INTERMEDIATE];
                let dst = &mut pw2[o * VOCOS_INTERMEDIATE..][..VOCOS_INTERMEDIATE];
                for (d, &s) in dst.iter_mut().zip(src) {
                    *d = s * g;
                }
            }
            ensure_weight(
                NS,
                &format!("blk{i}.pw2"),
                &pw2,
                VOCOS_DIM,
                VOCOS_INTERMEDIATE,
            )?;
            pw2_bias_folded.push(
                block
                    .pw2_b
                    .iter()
                    .zip(&block.gamma)
                    .map(|(&b, &g)| b * g)
                    .collect(),
            );
        }
        ensure_weight(NS, "out", &model.out_w, SEMANTIC_CODEC_DIM, VOCOS_DIM)?;
        ensure_weight(
            NS,
            "up",
            &pack_conv(&model.up),
            SEMANTIC_CODEC_DIM,
            3 * SEMANTIC_CODEC_DIM,
        )?;
        Ok(Self {
            idx: Mutex::new(HashMap::new()),
            pw2_bias_folded,
        })
    }

    /// Device counterpart of [`SemanticCodecDecoder::decode_cpu`]; same
    /// contract (validated codes -> time-major `[2t, 1024]`).
    pub fn decode(&self, model: &SemanticCodecDecoder, codes: &[u32]) -> Result<Vec<f32>> {
        let t = codes.len();
        // vq2emb on host: codebook lookup + out_project 1x1 conv == linear.
        let mut emb = vec![0f32; t * CODEBOOK_DIM];
        for (i, &c) in codes.iter().enumerate() {
            let row = &model.codebook[c as usize * CODEBOOK_DIM..(c as usize + 1) * CODEBOOK_DIM];
            emb[i * CODEBOOK_DIM..(i + 1) * CODEBOOK_DIM].copy_from_slice(row);
        }
        let quantized = makepad_ai_sfx::sa3::linear(
            &emb,
            &model.out_project_w,
            Some(&model.out_project_b),
            t,
            CODEBOOK_DIM,
            SEMANTIC_CODEC_DIM,
        );

        let mut idx_cache = self.idx.lock().map_err(|_| gm("index cache poisoned".into()))?;
        if !idx_cache.contains_key(&t) {
            idx_cache.insert(t, CodecRunIdx::build(t)?);
        }
        let idx = idx_cache.get(&t).expect("just inserted");

        let x = gpu_upload(&quantized, t, SEMANTIC_CODEC_DIM).map_err(gm)?;
        let h = conv_gemm(&x, &idx.taps7, &idx.zero_1024, NS, "embed", VOCOS_DIM, &model.embed.b)?;
        let mut h =
            gpu_layer_norm_pytorch(&h, &model.norm.gamma, &model.norm.beta, LN_EPS).map_err(gm)?;
        for (i, block) in model.blocks.iter().enumerate() {
            let d = conv_gemm(
                &h,
                &idx.taps7,
                &idx.zero_384,
                NS,
                &format!("blk{i}.dw"),
                VOCOS_DIM,
                &block.dw_b,
            )?;
            let n = gpu_layer_norm_pytorch(&d, &block.norm.gamma, &block.norm.beta, LN_EPS)
                .map_err(gm)?;
            let m = lin(&n, NS, &format!("blk{i}.pw1"), VOCOS_INTERMEDIATE, &block.pw1_b)?;
            let g = gpu_gelu_erf(&m).map_err(gm)?;
            let p = lin(&g, NS, &format!("blk{i}.pw2"), VOCOS_DIM, &self.pw2_bias_folded[i])?;
            h = gpu_add(&h, &p).map_err(gm)?;
        }
        let f = gpu_layer_norm_pytorch(&h, &model.final_norm.gamma, &model.final_norm.beta, LN_EPS)
            .map_err(gm)?;
        let o = lin(&f, NS, "out", SEMANTIC_CODEC_DIM, &model.out_b)?;
        let u = gpu_gather_rows_colblock(&o, &idx.up_gather, None, SEMANTIC_CODEC_DIM).map_err(gm)?;
        let r = conv_gemm(
            &u,
            &idx.taps3,
            &idx.zero_1024,
            NS,
            "up",
            SEMANTIC_CODEC_DIM,
            &model.up.b,
        )?;
        gpu_download(&r).map_err(gm)
    }
}
