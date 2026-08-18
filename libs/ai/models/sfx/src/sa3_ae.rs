//! SA3 SAME-S autoencoder DECODER (taae_v2): softnorm bottleneck ->
//! Linear(256->768) -> one TransformerResamplingBlock (6 DyT/differential
//! attention blocks over [1 latent + 16 new-token] groups, 34-token chunks
//! with midpoint shift) -> WNConv1d(768->512, k3) -> patched unfold to
//! 44.1kHz stereo.
//!
//! CPU f32, mirrors models/autoencoders.py + models/pretransforms.py +
//! models/bottleneck.py. The two inference-time stochastic regularizers of
//! the reference (bottleneck noise 1e-3, decoder new-token mask_noise 0.01)
//! are intentionally OMITTED (deterministic decode); the oracle dumps were
//! made with both zeroed, so parity is exact. Encoder (a2a/inpaint) is not
//! ported yet.

use crate::sa3::{
    dyt_rows, linear, par_rows, silu, Sa3Tensors, SA3_AE_CHUNK_TOKENS, SA3_AE_DEPTH, SA3_AE_DIM,
    SA3_AE_FF_INNER, SA3_AE_GROUP, SA3_AE_PATCH, SA3_AE_PATCH_CHANNELS, SA3_AE_STRIDE,
    SA3_AUDIO_CHANNELS, SA3_HEAD_DIM, SA3_LATENT_DIM,
};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};

const AE_HEADS: usize = SA3_AE_DIM / SA3_HEAD_DIM; // 12
const AE_ROPE_DIM: usize = 32;
const AE_ROPE_BASE: f32 = 10_000.0;

struct Dyt {
    alpha: f32,
    gamma: Vec<f32>,
    beta: Vec<f32>,
}

impl Dyt {
    fn load(t: &Sa3Tensors, prefix: &str, dim: usize) -> Result<Self> {
        let alpha = t.f32_shaped(&format!("{prefix}.alpha"), &[1])?[0];
        Ok(Self {
            alpha,
            gamma: t.f32_shaped(&format!("{prefix}.gamma"), &[dim])?,
            beta: t.f32_shaped(&format!("{prefix}.beta"), &[dim])?,
        })
    }

    fn apply(&self, x: &mut [f32]) {
        dyt_rows(x, self.alpha, &self.gamma, &self.beta, self.gamma.len());
    }
}

struct AeBlock {
    pre_norm: Dyt,
    ff_norm: Dyt,
    q_norm: Dyt,
    k_norm: Dyt,
    qkv: Vec<f32>,
    out: Vec<f32>,
    ff_proj_w: Vec<f32>,
    ff_proj_b: Vec<f32>,
    ff_out_w: Vec<f32>,
    ff_out_b: Vec<f32>,
}

pub struct Sa3AeDecoder {
    running_std: f32,
    latent_w: Vec<f32>,
    latent_b: Vec<f32>,
    new_tokens: Vec<f32>,
    blocks: Vec<AeBlock>,
    /// Weight-normalized mapping conv, resolved to a plain [512, 768, 3] kernel.
    mapping_w: Vec<f32>,
    mapping_b: Vec<f32>,
}

impl Sa3AeDecoder {
    /// Loads from the combined SA3 checkpoint (pretransform.model.* prefix).
    pub fn load(t: &Sa3Tensors) -> Result<Self> {
        let d = SA3_AE_DIM;
        let n = |s: &str| format!("pretransform.model.{s}");
        let running_std = t.f32_shaped(&n("bottleneck.running_std"), &[1])?[0];
        let latent_w = t.f32_shaped(&n("decoder.layers.1.weight"), &[d, SA3_LATENT_DIM])?;
        let latent_b = t.f32_shaped(&n("decoder.layers.1.bias"), &[d])?;
        let new_tokens = t.f32_shaped(&n("decoder.layers.3.new_tokens"), &[1, 1, d])?;
        let mut blocks = Vec::with_capacity(SA3_AE_DEPTH);
        for i in 0..SA3_AE_DEPTH {
            let l = |s: &str| n(&format!("decoder.layers.3.transformers.{i}.{s}"));
            blocks.push(AeBlock {
                pre_norm: Dyt::load(t, &l("pre_norm"), d)?,
                ff_norm: Dyt::load(t, &l("ff_norm"), d)?,
                q_norm: Dyt::load(t, &l("self_attn.q_norm"), SA3_HEAD_DIM)?,
                k_norm: Dyt::load(t, &l("self_attn.k_norm"), SA3_HEAD_DIM)?,
                qkv: t.f32_shaped(&l("self_attn.to_qkv.weight"), &[5 * d, d])?,
                out: t.f32_shaped(&l("self_attn.to_out.weight"), &[d, d])?,
                ff_proj_w: t.f32_shaped(&l("ff.ff.0.proj.weight"), &[2 * SA3_AE_FF_INNER, d])?,
                ff_proj_b: t.f32_shaped(&l("ff.ff.0.proj.bias"), &[2 * SA3_AE_FF_INNER])?,
                ff_out_w: t.f32_shaped(&l("ff.ff.2.weight"), &[d, SA3_AE_FF_INNER])?,
                ff_out_b: t.f32_shaped(&l("ff.ff.2.bias"), &[d])?,
            });
        }
        // Resolve weight norm: w = g * v / ||v||_(in,k) per out channel.
        let g = t.f32_shaped(&n("decoder.layers.3.mapping.weight_g"), &[SA3_AE_PATCH_CHANNELS, 1, 1])?;
        let v = t.f32_shaped(&n("decoder.layers.3.mapping.weight_v"), &[SA3_AE_PATCH_CHANNELS, d, 3])?;
        let mapping_b = t.f32_shaped(&n("decoder.layers.3.mapping.bias"), &[SA3_AE_PATCH_CHANNELS])?;
        let per = d * 3;
        let mut mapping_w = vec![0f32; SA3_AE_PATCH_CHANNELS * per];
        for o in 0..SA3_AE_PATCH_CHANNELS {
            let row = &v[o * per..(o + 1) * per];
            let norm = row.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt() as f32;
            let scale = g[o] / norm;
            for i in 0..per {
                mapping_w[o * per + i] = row[i] * scale;
            }
        }
        Ok(Self {
            running_std,
            latent_w,
            latent_b,
            new_tokens,
            blocks,
            mapping_w,
            mapping_b,
        })
    }

    /// Decodes latents `[latent_len, 256]` (token-major) into interleaved-free
    /// planar stereo `[2][latent_len * 4096]` samples.
    pub fn decode(&self, latents: &[f32], latent_len: usize) -> Result<Vec<Vec<f32>>> {
        self.decode_with_progress(latents, latent_len, None)
    }

    /// [`Self::decode`] ticking "ae-decode k/6" per transformer block.
    pub fn decode_with_progress(
        &self,
        latents: &[f32],
        latent_len: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<Vec<f32>>> {
        let d = SA3_AE_DIM;
        if latents.len() != latent_len * SA3_LATENT_DIM {
            return Err(DiffusionError::model("sa3 ae: latent buffer size mismatch"));
        }
        if latent_len % 2 != 0 {
            // chunk alignment (chunk_size/stride = 2); generate() sizes are
            // always even, unpadded odd lengths are unsupported.
            return Err(DiffusionError::model("sa3 ae: latent length must be even"));
        }

        // Bottleneck decode: x * running_std (stochastic regularizer omitted).
        let scaled: Vec<f32> = latents.iter().map(|v| v * self.running_std).collect();

        // Linear 256 -> 768.
        let projected = linear(&scaled, &self.latent_w, Some(&self.latent_b), latent_len, SA3_LATENT_DIM, d);

        // Build [latent, 16 x new_token] groups: seq = latent_len * 17.
        let group = SA3_AE_GROUP;
        let seq = latent_len * group;
        let mut x = vec![0f32; seq * d];
        for l in 0..latent_len {
            let base = l * group * d;
            x[base..base + d].copy_from_slice(&projected[l * d..(l + 1) * d]);
            for s in 0..SA3_AE_STRIDE {
                let dst = base + (1 + s) * d;
                x[dst..dst + d].copy_from_slice(&self.new_tokens);
            }
        }

        // Chunked transformer: 3 layers on 34-token chunks, midpoint shift by
        // 17 (edge repeat pad), 3 layers, unshift.
        let chunk = SA3_AE_CHUNK_TOKENS;
        debug_assert_eq!(seq % chunk, 0);
        let split = SA3_AE_DEPTH / 2;
        let block_progress = |index: usize, progress: &mut Option<ProgressHook>| {
            if progress.is_none() {
                return Ok(());
            }
            emit_progress(
                progress,
                &format!("ae-decode {}/{SA3_AE_DEPTH}", index + 1),
                index as f64 / SA3_AE_DEPTH as f64,
            )
        };
        for (i, block) in self.blocks[..split].iter().enumerate() {
            block_progress(i, &mut progress)?;
            self.run_block_chunked(block, &mut x, chunk, d);
        }
        let shift = chunk / 2; // 17
        let mut shifted = vec![0f32; (seq + 2 * shift) * d];
        shifted[..shift * d].copy_from_slice(&x[..shift * d]);
        shifted[shift * d..(shift + seq) * d].copy_from_slice(&x);
        shifted[(shift + seq) * d..].copy_from_slice(&x[(seq - shift) * d..]);
        for (i, block) in self.blocks[split..].iter().enumerate() {
            block_progress(split + i, &mut progress)?;
            self.run_block_chunked(block, &mut shifted, chunk, d);
        }
        x.copy_from_slice(&shifted[shift * d..(shift + seq) * d]);

        // Take the last 16 tokens of each 17-token group -> [768, latent*16].
        let out_len = latent_len * SA3_AE_STRIDE;
        let mut planar = vec![0f32; d * out_len];
        for l in 0..latent_len {
            for s in 0..SA3_AE_STRIDE {
                let tok = l * group + 1 + s;
                let col = l * SA3_AE_STRIDE + s;
                for ch in 0..d {
                    planar[ch * out_len + col] = x[tok * d + ch];
                }
            }
        }

        // WNConv1d 768 -> 512, k=3, same padding.
        let out_ch = SA3_AE_PATCH_CHANNELS;
        let mut mapped = vec![0f32; out_ch * out_len];
        par_rows(&mut mapped, out_len, &|o, row| {
            let w = &self.mapping_w[o * d * 3..(o + 1) * d * 3];
            let bias = self.mapping_b[o];
            for (pos, out_v) in row.iter_mut().enumerate() {
                let mut acc = bias;
                for kt in 0..3usize {
                    let src = pos as isize + kt as isize - 1;
                    if src < 0 || src >= out_len as isize {
                        continue;
                    }
                    let src = src as usize;
                    for ci in 0..d {
                        acc += w[ci * 3 + kt] * planar[ci * out_len + src];
                    }
                }
                *out_v = acc;
            }
        });

        // Patched pretransform decode: row (c*256 + h) col l -> audio[c][l*256+h].
        let samples = out_len * SA3_AE_PATCH;
        let mut audio = vec![vec![0f32; samples]; SA3_AUDIO_CHANNELS];
        for c in 0..SA3_AUDIO_CHANNELS {
            for h in 0..SA3_AE_PATCH {
                let row = &mapped[(c * SA3_AE_PATCH + h) * out_len..(c * SA3_AE_PATCH + h + 1) * out_len];
                for l in 0..out_len {
                    audio[c][l * SA3_AE_PATCH + h] = row[l];
                }
            }
        }
        Ok(audio)
    }

    /// Runs one transformer block independently over `chunk`-token windows.
    fn run_block_chunked(&self, block: &AeBlock, x: &mut [f32], chunk: usize, d: usize) {
        debug_assert_eq!(x.len() % (chunk * d), 0);
        // Per-chunk rope tables (positions 0..chunk), partial 32 of 64.
        let half = AE_ROPE_DIM / 2;
        let mut cos = vec![0f32; chunk * AE_ROPE_DIM];
        let mut sin = vec![0f32; chunk * AE_ROPE_DIM];
        for pos in 0..chunk {
            for i in 0..half {
                let inv = 1.0 / AE_ROPE_BASE.powf(2.0 * i as f32 / AE_ROPE_DIM as f32);
                let (s, c) = ((pos as f32) * inv).sin_cos();
                cos[pos * AE_ROPE_DIM + i] = c;
                cos[pos * AE_ROPE_DIM + half + i] = c;
                sin[pos * AE_ROPE_DIM + i] = s;
                sin[pos * AE_ROPE_DIM + half + i] = s;
            }
        }
        let apply_rope_partial = |buf: &mut [f32]| {
            for tok in 0..chunk {
                for h in 0..AE_HEADS {
                    let base = (tok * AE_HEADS + h) * SA3_HEAD_DIM;
                    for i in 0..half {
                        let a = buf[base + i];
                        let b = buf[base + half + i];
                        buf[base + i] = a * cos[tok * AE_ROPE_DIM + i] - b * sin[tok * AE_ROPE_DIM + i];
                        buf[base + half + i] = b * cos[tok * AE_ROPE_DIM + half + i]
                            + a * sin[tok * AE_ROPE_DIM + half + i];
                    }
                }
            }
        };

        let scale = 1.0 / (SA3_HEAD_DIM as f32).sqrt();
        // Each chunk is an independent attention window: parallelize with the
        // shared safe row-splitting helper (row = one chunk).
        crate::sa3::par_rows(x, chunk * d, &|_ci, chunk_x| {
            self.run_block_single(block, chunk_x, chunk, d, scale, &apply_rope_partial);
        });
    }

    fn run_block_single(
        &self,
        block: &AeBlock,
        x: &mut [f32],
        tokens: usize,
        d: usize,
        scale: f32,
        apply_rope_partial: &dyn Fn(&mut [f32]),
    ) {
        // --- differential self-attention, plain pre-norm residual ---
        let mut a = x.to_vec();
        block.pre_norm.apply(&mut a);
        let qkv = linear_st(&a, &block.qkv, None, tokens, d, 5 * d);
        let mut q = vec![0f32; tokens * d];
        let mut k = vec![0f32; tokens * d];
        let mut v = vec![0f32; tokens * d];
        let mut q2 = vec![0f32; tokens * d];
        let mut k2 = vec![0f32; tokens * d];
        for tok in 0..tokens {
            let row = &qkv[tok * 5 * d..(tok + 1) * 5 * d];
            q[tok * d..(tok + 1) * d].copy_from_slice(&row[..d]);
            k[tok * d..(tok + 1) * d].copy_from_slice(&row[d..2 * d]);
            v[tok * d..(tok + 1) * d].copy_from_slice(&row[2 * d..3 * d]);
            q2[tok * d..(tok + 1) * d].copy_from_slice(&row[3 * d..4 * d]);
            k2[tok * d..(tok + 1) * d].copy_from_slice(&row[4 * d..]);
        }
        for buf in [&mut q, &mut q2] {
            dyt_rows(buf, block.q_norm.alpha, &block.q_norm.gamma, &block.q_norm.beta, SA3_HEAD_DIM);
            apply_rope_partial(buf);
        }
        for buf in [&mut k, &mut k2] {
            dyt_rows(buf, block.k_norm.alpha, &block.k_norm.gamma, &block.k_norm.beta, SA3_HEAD_DIM);
            apply_rope_partial(buf);
        }
        let attn_a = attention_st(&q, &k, &v, tokens, scale, d);
        let attn_b = attention_st(&q2, &k2, &v, tokens, scale, d);
        let mut diff = vec![0f32; tokens * d];
        for i in 0..diff.len() {
            diff[i] = attn_a[i] - attn_b[i];
        }
        let out = linear_st(&diff, &block.out, None, tokens, d, d);
        for i in 0..x.len() {
            x[i] += out[i];
        }

        // --- GLU feedforward (mult 3), plain pre-norm residual ---
        let mut f = x.to_vec();
        block.ff_norm.apply(&mut f);
        let proj = linear_st(&f, &block.ff_proj_w, Some(&block.ff_proj_b), tokens, d, 2 * SA3_AE_FF_INNER);
        let mut inner = vec![0f32; tokens * SA3_AE_FF_INNER];
        for tok in 0..tokens {
            let row = &proj[tok * 2 * SA3_AE_FF_INNER..(tok + 1) * 2 * SA3_AE_FF_INNER];
            let out_row = &mut inner[tok * SA3_AE_FF_INNER..(tok + 1) * SA3_AE_FF_INNER];
            for i in 0..SA3_AE_FF_INNER {
                out_row[i] = row[i] * silu(row[SA3_AE_FF_INNER + i]);
            }
        }
        let ff_out = linear_st(&inner, &block.ff_out_w, Some(&block.ff_out_b), tokens, SA3_AE_FF_INNER, d);
        for i in 0..x.len() {
            x[i] += ff_out[i];
        }
    }
}

// ---------------------------------------------------------------------------
// CUDA device path (f16 cached weights, f32 activations).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight};
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_attention_packed, gpu_concat_cols, gpu_concat_rows, gpu_download, gpu_dyt,
    gpu_gated_residual, gpu_gather_rows_colblock, gpu_linear_nt_cached, gpu_rope_half,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_upload_u32, GpuTensor,
};

struct AeDeviceBlock {
    qkv: F16Weight,
    out: F16Weight,
    ff_proj: F16Weight,
    ff_out: F16Weight,
}

/// Prepared f16 device weights for the AE decoder. The bottleneck
/// running_std is folded into the latent projection.
pub struct Sa3AeDevice {
    latent: F16Weight,
    latent_bias: Vec<f32>,
    blocks: Vec<AeDeviceBlock>,
    /// Mapping conv unrolled for the [prev|cur|next] concat trick:
    /// weight'[o][kt*768 + ci] = w[o][ci][kt].
    mapping: F16Weight,
    mapping_bias: Vec<f32>,
}

impl Sa3AeDecoder {
    pub fn prepare_device(&self) -> Sa3AeDevice {
        let d = SA3_AE_DIM;
        // Fold bottleneck decode (x * running_std) into the latent linear.
        let folded: Vec<f32> = self.latent_w.iter().map(|w| w * self.running_std).collect();
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| AeDeviceBlock {
                qkv: F16Weight::new(format!("sa3ae.{i}.qkv"), &block.qkv, 5 * d, d),
                out: F16Weight::new(format!("sa3ae.{i}.out"), &block.out, d, d),
                ff_proj: F16Weight::new(
                    format!("sa3ae.{i}.fp"),
                    &block.ff_proj_w,
                    2 * SA3_AE_FF_INNER,
                    d,
                ),
                ff_out: F16Weight::new(format!("sa3ae.{i}.fo"), &block.ff_out_w, d, SA3_AE_FF_INNER),
            })
            .collect();
        // Reorder the k=3 conv kernel for the concat-cols linear.
        let out_ch = SA3_AE_PATCH_CHANNELS;
        let mut mapping = vec![0f32; out_ch * 3 * d];
        for o in 0..out_ch {
            for ci in 0..d {
                for kt in 0..3usize {
                    mapping[o * 3 * d + kt * d + ci] = self.mapping_w[o * d * 3 + ci * 3 + kt];
                }
            }
        }
        Sa3AeDevice {
            latent: F16Weight::new("sa3ae.latent", &folded, d, SA3_LATENT_DIM),
            latent_bias: self.latent_b.clone(),
            blocks,
            mapping: F16Weight::new("sa3ae.mapping", &mapping, out_ch, 3 * d),
            mapping_bias: self.mapping_b.clone(),
        }
    }

    /// Device decode: same contract as `decode`.
    pub fn decode_device(
        &self,
        device: &Sa3AeDevice,
        latents: &[f32],
        latent_len: usize,
    ) -> Result<Vec<Vec<f32>>> {
        self.decode_device_with_progress(device, latents, latent_len, None)
    }

    /// [`Self::decode_device`] ticking "ae-decode k/6" per transformer block.
    pub fn decode_device_with_progress(
        &self,
        device: &Sa3AeDevice,
        latents: &[f32],
        latent_len: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<Vec<f32>>> {
        let d = SA3_AE_DIM;
        if latents.len() != latent_len * SA3_LATENT_DIM {
            return Err(DiffusionError::model("sa3 ae device: latent size mismatch"));
        }
        if latent_len % 2 != 0 {
            return Err(DiffusionError::model("sa3 ae device: latent length must be even"));
        }
        let group = SA3_AE_GROUP;
        let seq = latent_len * group;
        let chunk = SA3_AE_CHUNK_TOKENS;
        let shift = chunk / 2;

        // Latent projection (+ folded bottleneck), then group assembly via
        // row gather: [latent_i, 16 x new_token] per group.
        let lat = gpu_upload(latents, latent_len, SA3_LATENT_DIM).map_err(|e| dev_err("ae upload", e))?;
        let projected =
            gpu_linear_nt_cached(&lat, "sa3ae", &[device.latent.part()], &device.latent_bias)
                .map_err(|e| dev_err("ae latent linear", e))?;
        let new_token = gpu_upload(&self.new_tokens, 1, d).map_err(|e| dev_err("ae new token", e))?;
        let src = gpu_concat_rows(&projected, &new_token).map_err(|e| dev_err("ae assembly src", e))?;
        let idx: Vec<u32> = (0..seq)
            .map(|i| {
                if i % group == 0 {
                    (i / group) as u32
                } else {
                    latent_len as u32
                }
            })
            .collect();
        let idx = gpu_upload_u32(&idx).map_err(|e| dev_err("ae assembly idx", e))?;
        let mut x = gpu_gather_rows_colblock(&src, &idx, None, d).map_err(|e| dev_err("ae assemble", e))?;

        // Chunk-position rope tables (positions repeat every 34 tokens).
        let rope_pair = |total: usize| -> Result<(GpuTensor, GpuTensor)> {
            let half = AE_ROPE_DIM / 2;
            let mut cos = vec![0f32; total * half];
            let mut sin = vec![0f32; total * half];
            for tok in 0..total {
                let pos = (tok % chunk) as f32;
                for i in 0..half {
                    let inv = 1.0 / AE_ROPE_BASE.powf(2.0 * i as f32 / AE_ROPE_DIM as f32);
                    let (s, c) = (pos * inv).sin_cos();
                    cos[tok * half + i] = c;
                    sin[tok * half + i] = s;
                }
            }
            Ok((
                gpu_upload(&cos, total, half).map_err(|e| dev_err("ae rope cos", e))?,
                gpu_upload(&sin, total, half).map_err(|e| dev_err("ae rope sin", e))?,
            ))
        };
        let rope_plain = rope_pair(seq)?;
        let rope_shifted = rope_pair(seq + 2 * shift)?;

        let split = SA3_AE_DEPTH / 2;
        let block_progress = |index: usize, progress: &mut Option<ProgressHook>| {
            if progress.is_none() {
                return Ok(());
            }
            emit_progress(
                progress,
                &format!("ae-decode {}/{SA3_AE_DEPTH}", index + 1),
                index as f64 / SA3_AE_DEPTH as f64,
            )
        };
        for (i, _) in self.blocks.iter().enumerate().take(split) {
            block_progress(i, &mut progress)?;
            x = self.run_block_device(device, i, &x, chunk, &rope_plain)?;
        }
        // Midpoint shift: repeat-pad 17 tokens on each side.
        let head = gpu_slice_rows(&x, 0, shift).map_err(|e| dev_err("ae shift head", e))?;
        let tail = gpu_slice_rows(&x, seq - shift, shift).map_err(|e| dev_err("ae shift tail", e))?;
        let mut shifted = gpu_concat_rows(&head, &x).map_err(|e| dev_err("ae shift concat 1", e))?;
        shifted = gpu_concat_rows(&shifted, &tail).map_err(|e| dev_err("ae shift concat 2", e))?;
        for (i, _) in self.blocks.iter().enumerate().skip(split) {
            block_progress(i, &mut progress)?;
            shifted = self.run_block_device(device, i, &shifted, chunk, &rope_shifted)?;
        }
        x = gpu_slice_rows(&shifted, shift, seq).map_err(|e| dev_err("ae unshift", e))?;

        // Last 16 tokens of each 17-token group.
        let out_len = latent_len * SA3_AE_STRIDE;
        let take_idx: Vec<u32> = (0..out_len)
            .map(|i| {
                let l = i / SA3_AE_STRIDE;
                let s = i % SA3_AE_STRIDE;
                (l * group + 1 + s) as u32
            })
            .collect();
        let take_idx = gpu_upload_u32(&take_idx).map_err(|e| dev_err("ae take idx", e))?;
        let tokens = gpu_gather_rows_colblock(&x, &take_idx, None, d).map_err(|e| dev_err("ae take", e))?;

        // k=3 mapping conv as [prev|cur|next] concat + one linear.
        let prev_idx: Vec<u32> = (0..out_len)
            .map(|i| if i == 0 { u32::MAX } else { (i - 1) as u32 })
            .collect();
        let next_idx: Vec<u32> = (0..out_len)
            .map(|i| {
                if i + 1 >= out_len {
                    u32::MAX
                } else {
                    (i + 1) as u32
                }
            })
            .collect();
        let prev_idx = gpu_upload_u32(&prev_idx).map_err(|e| dev_err("ae prev idx", e))?;
        let next_idx = gpu_upload_u32(&next_idx).map_err(|e| dev_err("ae next idx", e))?;
        let prev = gpu_gather_rows_colblock(&tokens, &prev_idx, None, d).map_err(|e| dev_err("ae prev", e))?;
        let next = gpu_gather_rows_colblock(&tokens, &next_idx, None, d).map_err(|e| dev_err("ae next", e))?;
        let stacked = gpu_concat_cols(&[&prev, &tokens, &next]).map_err(|e| dev_err("ae stack", e))?;
        let mapped = gpu_linear_nt_cached(
            &stacked,
            "sa3ae",
            &[device.mapping.part()],
            &device.mapping_bias,
        )
        .map_err(|e| dev_err("ae mapping", e))?;
        let host = gpu_download(&mapped).map_err(|e| dev_err("ae download", e))?;

        // Patched unfold: token t, col c*256+h -> audio[c][t*256+h].
        let samples = out_len * SA3_AE_PATCH;
        let mut audio = vec![vec![0f32; samples]; SA3_AUDIO_CHANNELS];
        let cols = SA3_AE_PATCH_CHANNELS;
        for t in 0..out_len {
            let row = &host[t * cols..(t + 1) * cols];
            for c in 0..SA3_AUDIO_CHANNELS {
                let dst = &mut audio[c][t * SA3_AE_PATCH..(t + 1) * SA3_AE_PATCH];
                dst.copy_from_slice(&row[c * SA3_AE_PATCH..(c + 1) * SA3_AE_PATCH]);
            }
        }
        Ok(audio)
    }

    /// One AE transformer block on the device: per-token ops on the full
    /// sequence, differential attention per 34-token chunk with a pairwise
    /// tree re-concat.
    fn run_block_device(
        &self,
        device: &Sa3AeDevice,
        index: usize,
        x: &GpuTensor,
        chunk: usize,
        rope: &(GpuTensor, GpuTensor),
    ) -> Result<GpuTensor> {
        let d = SA3_AE_DIM;
        let block = &self.blocks[index];
        let dev = &device.blocks[index];
        let seq = x.rows();
        debug_assert_eq!(seq % chunk, 0);

        let key = |what: &str| format!("b{index}.{what}");
        let a = gpu_dyt(
            x, d, "sa3ae", &key("pre"), &block.pre_norm.gamma, &block.pre_norm.beta,
            block.pre_norm.alpha,
        )
        .map_err(|e| dev_err("ae pre dyt", e))?;
        let qkv = gpu_linear_nt_cached(&a, "sa3ae", &[dev.qkv.part()], &[])
            .map_err(|e| dev_err("ae qkv", e))?;
        let mut heads_parts = Vec::with_capacity(5);
        for part in 0..5 {
            heads_parts.push(gpu_slice_cols(&qkv, part * d, d).map_err(|e| dev_err("ae qkv slice", e))?);
        }
        let apply_qk = |buf: &GpuTensor, what: &str| -> Result<GpuTensor> {
            let normed = gpu_dyt(
                buf,
                SA3_HEAD_DIM,
                "sa3ae",
                &key(what),
                &block.q_norm.gamma,
                &block.q_norm.beta,
                block.q_norm.alpha,
            )
            .map_err(|e| dev_err("ae qk dyt", e))?;
            gpu_rope_half(&normed, AE_HEADS, AE_ROPE_DIM / 2, &rope.0, &rope.1)
                .map_err(|e| dev_err("ae rope", e))
        };
        let apply_k = |buf: &GpuTensor, what: &str| -> Result<GpuTensor> {
            let normed = gpu_dyt(
                buf,
                SA3_HEAD_DIM,
                "sa3ae",
                &key(what),
                &block.k_norm.gamma,
                &block.k_norm.beta,
                block.k_norm.alpha,
            )
            .map_err(|e| dev_err("ae qk dyt", e))?;
            gpu_rope_half(&normed, AE_HEADS, AE_ROPE_DIM / 2, &rope.0, &rope.1)
                .map_err(|e| dev_err("ae rope", e))
        };
        let q = apply_qk(&heads_parts[0], "qn")?;
        let k = apply_k(&heads_parts[1], "kn")?;
        let v = &heads_parts[2];
        let q2 = apply_qk(&heads_parts[3], "q2n")?;
        let k2 = apply_k(&heads_parts[4], "k2n")?;

        // Differential attention per chunk, pairwise-tree re-concat.
        let scale = 1.0 / (SA3_HEAD_DIM as f32).sqrt();
        let neg_one = vec![-1.0f32; d];
        let mut chunk_outs: Vec<GpuTensor> = Vec::with_capacity(seq / chunk);
        for start in (0..seq).step_by(chunk) {
            let qc = gpu_slice_rows(&q, start, chunk).map_err(|e| dev_err("ae chunk q", e))?;
            let kc = gpu_slice_rows(&k, start, chunk).map_err(|e| dev_err("ae chunk k", e))?;
            let vc = gpu_slice_rows(v, start, chunk).map_err(|e| dev_err("ae chunk v", e))?;
            let q2c = gpu_slice_rows(&q2, start, chunk).map_err(|e| dev_err("ae chunk q2", e))?;
            let k2c = gpu_slice_rows(&k2, start, chunk).map_err(|e| dev_err("ae chunk k2", e))?;
            let attn_a =
                gpu_attention_packed(&qc, &kc, &vc, AE_HEADS, scale).map_err(|e| dev_err("ae attn a", e))?;
            let attn_b = gpu_attention_packed(&q2c, &k2c, &vc, AE_HEADS, scale)
                .map_err(|e| dev_err("ae attn b", e))?;
            // attn_a + (-1) * attn_b
            chunk_outs.push(
                gpu_gated_residual(&attn_a, &attn_b, &neg_one).map_err(|e| dev_err("ae attn sub", e))?,
            );
        }
        while chunk_outs.len() > 1 {
            let mut next = Vec::with_capacity(chunk_outs.len().div_ceil(2));
            let mut iter = chunk_outs.into_iter();
            while let Some(first) = iter.next() {
                match iter.next() {
                    Some(second) => next.push(
                        gpu_concat_rows(&first, &second).map_err(|e| dev_err("ae tree concat", e))?,
                    ),
                    None => next.push(first),
                }
            }
            chunk_outs = next;
        }
        let attn = chunk_outs.pop().ok_or_else(|| {
            DiffusionError::model("sa3 ae device: empty chunk list")
        })?;
        let out = gpu_linear_nt_cached(&attn, "sa3ae", &[dev.out.part()], &[])
            .map_err(|e| dev_err("ae attn out", e))?;
        let x = gpu_add(x, &out).map_err(|e| dev_err("ae attn residual", e))?;

        // GLU feedforward (SwiGLU, value first).
        let f = gpu_dyt(
            &x, d, "sa3ae", &key("ff"), &block.ff_norm.gamma, &block.ff_norm.beta,
            block.ff_norm.alpha,
        )
        .map_err(|e| dev_err("ae ff dyt", e))?;
        let proj = gpu_linear_nt_cached(&f, "sa3ae", &[dev.ff_proj.part()], &block.ff_proj_b)
            .map_err(|e| dev_err("ae ff proj", e))?;
        let inner = gpu_swiglu_value_gate(&proj).map_err(|e| dev_err("ae swiglu", e))?;
        let ff = gpu_linear_nt_cached(&inner, "sa3ae", &[dev.ff_out.part()], &block.ff_out_b)
            .map_err(|e| dev_err("ae ff out", e))?;
        gpu_add(&x, &ff).map_err(|e| dev_err("ae ff residual", e))
    }
}

/// Single-threaded linear (the AE parallelizes over chunks, not rows).
fn linear_st(a: &[f32], w: &[f32], bias: Option<&[f32]>, m: usize, k: usize, n: usize) -> Vec<f32> {
    let mut out = vec![0f32; m * n];
    for row in 0..m {
        let a_row = &a[row * k..(row + 1) * k];
        let out_row = &mut out[row * n..(row + 1) * n];
        for col in 0..n {
            let w_row = &w[col * k..(col + 1) * k];
            let mut acc = 0f32;
            for i in 0..k {
                acc += a_row[i] * w_row[i];
            }
            out_row[col] = acc + bias.map_or(0.0, |b| b[col]);
        }
    }
    out
}

/// Single-threaded softmax attention, q/k/v `[tokens, heads, 64]`, no mask.
fn attention_st(q: &[f32], k: &[f32], v: &[f32], tokens: usize, scale: f32, d: usize) -> Vec<f32> {
    let hd = SA3_HEAD_DIM;
    let heads = d / hd;
    let mut out = vec![0f32; tokens * d];
    let mut scores = vec![0f32; tokens];
    for qt in 0..tokens {
        for h in 0..heads {
            let q_vec = &q[(qt * heads + h) * hd..(qt * heads + h + 1) * hd];
            let mut max_score = f32::NEG_INFINITY;
            for (kt, score) in scores.iter_mut().enumerate() {
                let k_vec = &k[(kt * heads + h) * hd..(kt * heads + h + 1) * hd];
                let mut acc = 0f32;
                for i in 0..hd {
                    acc += q_vec[i] * k_vec[i];
                }
                *score = acc * scale;
                if *score > max_score {
                    max_score = *score;
                }
            }
            let mut denom = 0f32;
            for score in scores.iter_mut() {
                *score = (*score - max_score).exp();
                denom += *score;
            }
            let inv = 1.0 / denom;
            let out_vec = &mut out[(qt * heads + h) * hd..(qt * heads + h + 1) * hd];
            for (kt, &score) in scores.iter().enumerate() {
                let w = score * inv;
                let v_vec = &v[(kt * heads + h) * hd..(kt * heads + h + 1) * hd];
                for i in 0..hd {
                    out_vec[i] += w * v_vec[i];
                }
            }
        }
    }
    out
}
