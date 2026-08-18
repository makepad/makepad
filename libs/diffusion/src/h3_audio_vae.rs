//! MiniMax H3 audio VAE decoder (DAC/BigVGAN lineage), CPU f32.
//!
//! Port of the decode() path of `AutoencoderKLMiniMaxH3Audio`
//! (diffusers autoencoder_kl_minimax_h3_audio.py). The decoder is BigVGAN:
//!
//!   dec_in_proj: Conv1d(latent_channels=32 -> latent_dim=2048, k=1)
//!   decoder.conv_pre: weight-norm Conv1d(2048 -> decoder_dim=1024, k=7, p=3)
//!   7 upsample stages (rates 5,5,2,2,2,2,2 / kernels 9,9,4,4,4,4,4):
//!     ups.i.0: weight-norm ConvTranspose1d(C -> C/2, k, stride=rate,
//!                                          padding=(k-rate)/2)
//!     then the mean of 3 AMP residual blocks (kernels 3/7/11, dilations
//!     1/3/5), every conv preceded by its own anti-aliased SnakeBeta
//!     activation (2x Kaiser-sinc upsample -> snake -> 2x downsample; the
//!     12-tap filters are persistent buffers in the checkpoint)
//!   activation_post: anti-aliased SnakeBeta on the final 8 channels
//!   conv_post: weight-norm Conv1d(8 -> 1, k=7, p=3, no bias)
//!   clamp to [-1, 1]
//!
//! weight_norm follows the old-style torch spelling: `weight_g` / `weight_v`
//! with `w = g * v / ||v||`, the norm taken over all dims except dim 0 — for
//! Conv1d that is per *output* channel (g shape `(out,1,1)`), for
//! ConvTranspose1d per *input* channel (g shape `(in,1,1)`, weight layout
//! `(in, out, k)`).
//!
//! Stereo is carried as two batch items: `decode_latents` takes the
//! denormalized `(2, 32, n_lat)` latents and returns `(2, samples)` planar
//! `[L..., R...]` with `samples = n_lat * 800`. `decode_rows` starts from the
//! raw denoised audio rows (`2*n_lat` rows x 32, all left-channel rows first)
//! exactly like the diffusers decoders.py: reshape(2, n_lat, 32) ->
//! permute(0,2,1) -> `latents * latents_std + latents_mean` -> decode.

use crate::error::{DiffusionError, Result};
use crate::h3::H3ShardedWeights;
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// (channels, length) f32 plane, channel-major rows.
// ---------------------------------------------------------------------------

struct Plane {
    ch: usize,
    len: usize,
    data: Vec<f32>,
}

/// Run `f(row_index, row)` over the `row_len`-strided rows of `data`, split
/// across `threads` std::thread workers (contiguous row chunks per worker).
fn par_rows<F: Fn(usize, &mut [f32]) + Sync>(
    threads: usize,
    data: &mut [f32],
    row_len: usize,
    f: &F,
) {
    debug_assert_eq!(data.len() % row_len, 0);
    let rows = data.len() / row_len;
    let threads = threads.clamp(1, rows.max(1));
    if threads <= 1 {
        for (row_index, row) in data.chunks_mut(row_len).enumerate() {
            f(row_index, row);
        }
        return;
    }
    let rows_per = (rows + threads - 1) / threads;
    std::thread::scope(|scope| {
        let mut rest = data;
        let mut first_row = 0;
        while !rest.is_empty() {
            let take = (rows_per * row_len).min(rest.len());
            let (chunk, tail) = rest.split_at_mut(take);
            rest = tail;
            let start = first_row;
            first_row += take / row_len;
            scope.spawn(move || {
                for (offset, row) in chunk.chunks_mut(row_len).enumerate() {
                    f(start + offset, row);
                }
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Layers.
// ---------------------------------------------------------------------------

/// Conv1d, stride 1, zero padding, optional dilation. Weight `(out, in, k)`.
struct Conv1d {
    in_ch: usize,
    out_ch: usize,
    k: usize,
    dilation: usize,
    padding: usize,
    weight: Vec<f32>,
    /// Empty when the layer has no bias (conv_post).
    bias: Vec<f32>,
}

impl Conv1d {
    fn forward(&self, x: &Plane, threads: usize) -> Plane {
        debug_assert_eq!(x.ch, self.in_ch);
        let span = self.dilation * (self.k - 1);
        let out_len = x.len + 2 * self.padding - span;
        let len = x.len;
        let mut out = vec![0f32; self.out_ch * out_len];
        par_rows(threads, &mut out, out_len, &|o, row| {
            if !self.bias.is_empty() {
                row.fill(self.bias[o]);
            }
            for i in 0..self.in_ch {
                let x_row = &x.data[i * len..(i + 1) * len];
                let w_row = &self.weight[(o * self.in_ch + i) * self.k..][..self.k];
                for (kk, &w) in w_row.iter().enumerate() {
                    // out[t] += w * x[t + kk*dilation - padding]
                    let off = (kk * self.dilation) as isize - self.padding as isize;
                    let t0 = (-off).max(0) as usize;
                    let t1 = (len as isize - off).min(out_len as isize);
                    if (t1 as isize) <= t0 as isize {
                        continue;
                    }
                    let t1 = t1 as usize;
                    let x_slice = &x_row[(t0 as isize + off) as usize..(t1 as isize + off) as usize];
                    for (acc, &xv) in row[t0..t1].iter_mut().zip(x_slice) {
                        *acc += w * xv;
                    }
                }
            }
        });
        Plane { ch: self.out_ch, len: out_len, data: out }
    }
}

/// ConvTranspose1d, no output_padding. Weight `(in, out, k)` (torch layout).
struct ConvTranspose1d {
    in_ch: usize,
    out_ch: usize,
    k: usize,
    stride: usize,
    padding: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl ConvTranspose1d {
    fn forward(&self, x: &Plane, threads: usize) -> Plane {
        debug_assert_eq!(x.ch, self.in_ch);
        let out_len = (x.len - 1) * self.stride + self.k - 2 * self.padding;
        let len = x.len;
        let stride = self.stride;
        let mut out = vec![0f32; self.out_ch * out_len];
        par_rows(threads, &mut out, out_len, &|o, row| {
            row.fill(self.bias[o]);
            for i in 0..self.in_ch {
                let x_row = &x.data[i * len..(i + 1) * len];
                let w_row = &self.weight[(i * self.out_ch + o) * self.k..][..self.k];
                for (kk, &w) in w_row.iter().enumerate() {
                    // out[ti*stride + kk - padding] += w * x[ti]
                    let off = kk as isize - self.padding as isize;
                    let ti0 = if off >= 0 {
                        0
                    } else {
                        ((-off) as usize + stride - 1) / stride
                    };
                    let hi = out_len as isize - 1 - off;
                    if hi < 0 {
                        continue;
                    }
                    let ti1 = (hi as usize / stride + 1).min(len);
                    if ti1 <= ti0 {
                        continue;
                    }
                    let mut t = ((ti0 * stride) as isize + off) as usize;
                    for &xv in &x_row[ti0..ti1] {
                        row[t] += w * xv;
                        t += stride;
                    }
                }
            }
        });
        Plane { ch: self.out_ch, len: out_len, data: out }
    }
}

/// Anti-aliased SnakeBeta activation (MiniMaxH3AudioActivation1d):
/// replicate-pad -> depthwise Kaiser-sinc transposed conv (2x, scaled by the
/// ratio) -> crop -> `x + sin(exp(alpha)*x)^2 / (exp(beta)+1e-9)` -> replicate
/// pad -> depthwise strided Kaiser-sinc conv (back to 1x).
struct AliasFreeSnake {
    /// exp(alpha) per channel (checkpoint stores log space).
    alpha: Vec<f32>,
    /// 1 / (exp(beta) + 1e-9) per channel.
    inv_beta: Vec<f32>,
    /// Upsampler taps with the `ratio` gain already folded in (exact x2).
    up_filter: Vec<f32>,
    down_filter: Vec<f32>,
    ratio: usize,
    up_pad: usize,
    up_crop_left: usize,
    down_pad_left: usize,
    down_pad_right: usize,
}

#[inline]
fn snake_beta_in_place(row: &mut [f32], alpha: f32, inv_beta: f32) {
    for v in row.iter_mut() {
        let s = (alpha * *v).sin();
        *v += inv_beta * s * s;
    }
}

impl AliasFreeSnake {
    /// `alpha_log` / `beta_log` are the raw checkpoint parameters (log
    /// space); `up_filter` / `down_filter` are the raw buffer taps.
    fn new(
        alpha_log: &[f32],
        beta_log: &[f32],
        up_filter: &[f32],
        down_filter: &[f32],
        ratio: usize,
    ) -> Self {
        let k_up = up_filter.len();
        let k_down = down_filter.len();
        // MiniMaxH3AudioUpSample1d arithmetic.
        let up_pad = k_up / ratio - 1;
        let up_crop_left = up_pad * ratio + (k_up - ratio) / 2;
        let up_crop_right = up_pad * ratio + (k_up - ratio + 1) / 2;
        // Cropped upsample must be exactly ratio*len for any len; check the
        // constant part of ((len + 2*up_pad) - 1)*ratio + k_up - crops.
        debug_assert_eq!(
            (2 * up_pad as isize - 1) * ratio as isize + k_up as isize
                - up_crop_left as isize
                - up_crop_right as isize,
            0
        );
        // MiniMaxH3AudioLowPassFilter1d arithmetic.
        let down_pad_left = k_down / 2 - usize::from(k_down % 2 == 0);
        let down_pad_right = k_down / 2;
        Self {
            alpha: alpha_log.iter().map(|a| a.exp()).collect(),
            inv_beta: beta_log.iter().map(|b| 1.0 / (b.exp() + 1e-9)).collect(),
            up_filter: up_filter.iter().map(|f| f * ratio as f32).collect(),
            down_filter: down_filter.to_vec(),
            ratio,
            up_pad,
            up_crop_left,
            down_pad_left,
            down_pad_right,
        }
    }

    fn forward(&self, x: &Plane, threads: usize) -> Plane {
        debug_assert_eq!(x.ch, self.alpha.len());
        let len = x.len;
        let ratio = self.ratio;
        let k_up = self.up_filter.len();
        let k_down = self.down_filter.len();
        let padded_len = len + 2 * self.up_pad;
        let up_full_len = (padded_len - 1) * ratio + k_up;
        let up_len = len * ratio;
        let mut out = vec![0f32; x.ch * len];
        par_rows(threads, &mut out, len, &|c, row| {
            let x_row = &x.data[c * len..(c + 1) * len];
            // Replicate pad, then depthwise transposed conv at `ratio`.
            let mut padded = vec![0f32; padded_len];
            padded[..self.up_pad].fill(x_row[0]);
            padded[self.up_pad..self.up_pad + len].copy_from_slice(x_row);
            padded[self.up_pad + len..].fill(x_row[len - 1]);
            let mut up = vec![0f32; up_full_len];
            for (ti, &v) in padded.iter().enumerate() {
                let base = ti * ratio;
                for (kk, &f) in self.up_filter.iter().enumerate() {
                    up[base + kk] += f * v;
                }
            }
            // Crop to ratio*len, snake in place.
            let cropped = &mut up[self.up_crop_left..self.up_crop_left + up_len];
            snake_beta_in_place(cropped, self.alpha[c], self.inv_beta[c]);
            let cropped = &up[self.up_crop_left..self.up_crop_left + up_len];
            // Replicate pad, depthwise strided conv back down to `len`.
            let down_len = up_len + self.down_pad_left + self.down_pad_right;
            let mut down = vec![0f32; down_len];
            down[..self.down_pad_left].fill(cropped[0]);
            down[self.down_pad_left..self.down_pad_left + up_len].copy_from_slice(cropped);
            down[self.down_pad_left + up_len..].fill(cropped[up_len - 1]);
            for (t, acc) in row.iter_mut().enumerate() {
                let seg = &down[t * ratio..t * ratio + k_down];
                let mut sum = 0f32;
                for (&f, &v) in self.down_filter.iter().zip(seg) {
                    sum += f * v;
                }
                *acc = sum;
            }
        });
        Plane { ch: x.ch, len, data: out }
    }
}

/// BigVGAN AMP residual block: per dilation `x += conv2(act2(conv1(act1(x))))`.
struct AmpBlock {
    convs1: Vec<Conv1d>,
    convs2: Vec<Conv1d>,
    acts: Vec<AliasFreeSnake>,
}

impl AmpBlock {
    fn forward(&self, x: &Plane, threads: usize) -> Plane {
        let mut hidden = Plane { ch: x.ch, len: x.len, data: x.data.clone() };
        for j in 0..self.convs1.len() {
            let t = self.acts[2 * j].forward(&hidden, threads);
            let t = self.convs1[j].forward(&t, threads);
            let t = self.acts[2 * j + 1].forward(&t, threads);
            let t = self.convs2[j].forward(&t, threads);
            for (acc, &v) in hidden.data.iter_mut().zip(t.data.iter()) {
                *acc += v;
            }
        }
        hidden
    }
}

struct UpStage {
    up: ConvTranspose1d,
    blocks: Vec<AmpBlock>,
}

// ---------------------------------------------------------------------------
// Weight loading.
// ---------------------------------------------------------------------------

/// Old-style torch weight_norm: `w = g * v / ||v||`, norm over all dims
/// except dim 0. `v` is `(rows, cols)` flattened, `g` one gain per row.
fn weight_norm_dim0(v: &[f32], g: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    debug_assert_eq!(v.len(), rows * cols);
    debug_assert_eq!(g.len(), rows);
    let mut out = vec![0f32; v.len()];
    for r in 0..rows {
        let src = &v[r * cols..(r + 1) * cols];
        let norm = src.iter().map(|&x| x as f64 * x as f64).sum::<f64>().sqrt() as f32;
        let scale = g[r] / norm;
        for (dst, &x) in out[r * cols..(r + 1) * cols].iter_mut().zip(src) {
            *dst = x * scale;
        }
    }
    out
}

fn tensor_shape3(weights: &H3ShardedWeights, name: &str) -> Result<(usize, usize, usize)> {
    let (_, shape) = weights.tensor_dtype_shape(name)?;
    if shape.len() != 3 {
        return Err(DiffusionError::model(format!(
            "audio vae tensor '{name}': expected rank 3, got {shape:?}"
        )));
    }
    Ok((shape[0] as usize, shape[1] as usize, shape[2] as usize))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvWeightStorage {
    WeightNorm,
    Fused,
}

/// Canonical MiniMax shards carry old-style `weight_g`/`weight_v`; the
/// pinned consumer-tier audio-VAE repack has already folded weight norm into
/// one plain `.weight`. Accept exactly either complete layout and reject
/// partial/ambiguous mixtures before decoding.
fn conv_weight_storage(
    has_g: bool,
    has_v: bool,
    has_fused: bool,
    prefix: &str,
) -> Result<ConvWeightStorage> {
    match (has_g, has_v, has_fused) {
        (true, true, false) => Ok(ConvWeightStorage::WeightNorm),
        (false, false, true) => Ok(ConvWeightStorage::Fused),
        _ => Err(DiffusionError::model(format!(
            "audio vae '{prefix}': expected either weight_g+weight_v or one fused weight; presence g={has_g} v={has_v} fused={has_fused}"
        ))),
    }
}

fn load_wn_conv1d(
    weights: &H3ShardedWeights,
    prefix: &str,
    dilation: usize,
    padding: usize,
    has_bias: bool,
) -> Result<Conv1d> {
    let g_name = format!("{prefix}.weight_g");
    let v_name = format!("{prefix}.weight_v");
    let fused_name = format!("{prefix}.weight");
    let storage = conv_weight_storage(
        weights.has_tensor(&g_name),
        weights.has_tensor(&v_name),
        weights.has_tensor(&fused_name),
        prefix,
    )?;
    let shape_name = match storage {
        ConvWeightStorage::WeightNorm => &v_name,
        ConvWeightStorage::Fused => &fused_name,
    };
    let (out_ch, in_ch, k) = tensor_shape3(weights, shape_name)?;
    let weight = match storage {
        ConvWeightStorage::WeightNorm => {
            let (g0, g1, g2) = tensor_shape3(weights, &g_name)?;
            if (g0, g1, g2) != (out_ch, 1, 1) {
                return Err(DiffusionError::model(format!(
                    "audio vae '{g_name}': expected shape ({out_ch},1,1), got ({g0},{g1},{g2})"
                )));
            }
            let v = weights.tensor_f32(&v_name)?;
            let g = weights.tensor_f32(&g_name)?;
            weight_norm_dim0(&v, &g, out_ch, in_ch * k)
        }
        ConvWeightStorage::Fused => weights.tensor_f32(&fused_name)?,
    };
    let bias = if has_bias {
        weights.tensor_f32(&format!("{prefix}.bias"))?
    } else {
        Vec::new()
    };
    Ok(Conv1d { in_ch, out_ch, k, dilation, padding, weight, bias })
}

fn load_wn_conv_transpose1d(
    weights: &H3ShardedWeights,
    prefix: &str,
    stride: usize,
    padding: usize,
) -> Result<ConvTranspose1d> {
    let g_name = format!("{prefix}.weight_g");
    let v_name = format!("{prefix}.weight_v");
    let fused_name = format!("{prefix}.weight");
    let storage = conv_weight_storage(
        weights.has_tensor(&g_name),
        weights.has_tensor(&v_name),
        weights.has_tensor(&fused_name),
        prefix,
    )?;
    // torch ConvTranspose1d weight layout is (in, out, k); weight_norm dim 0
    // therefore normalizes per input channel.
    let shape_name = match storage {
        ConvWeightStorage::WeightNorm => &v_name,
        ConvWeightStorage::Fused => &fused_name,
    };
    let (in_ch, out_ch, k) = tensor_shape3(weights, shape_name)?;
    let weight = match storage {
        ConvWeightStorage::WeightNorm => {
            let (g0, g1, g2) = tensor_shape3(weights, &g_name)?;
            if (g0, g1, g2) != (in_ch, 1, 1) {
                return Err(DiffusionError::model(format!(
                    "audio vae '{g_name}': expected shape ({in_ch},1,1), got ({g0},{g1},{g2})"
                )));
            }
            let v = weights.tensor_f32(&v_name)?;
            let g = weights.tensor_f32(&g_name)?;
            weight_norm_dim0(&v, &g, in_ch, out_ch * k)
        }
        ConvWeightStorage::Fused => weights.tensor_f32(&fused_name)?,
    };
    let bias = weights.tensor_f32(&format!("{prefix}.bias"))?;
    Ok(ConvTranspose1d { in_ch, out_ch, k, stride, padding, weight, bias })
}

fn load_alias_free(
    weights: &H3ShardedWeights,
    prefix: &str,
    channels: usize,
) -> Result<AliasFreeSnake> {
    let alpha = weights.tensor_f32(&format!("{prefix}.act.alpha"))?;
    let beta = weights.tensor_f32(&format!("{prefix}.act.beta"))?;
    if alpha.len() != channels || beta.len() != channels {
        return Err(DiffusionError::model(format!(
            "audio vae '{prefix}': expected {channels}-channel snake, got alpha {} beta {}",
            alpha.len(),
            beta.len()
        )));
    }
    let up_filter = weights.tensor_f32(&format!("{prefix}.upsample.filter"))?;
    let down_filter = weights.tensor_f32(&format!("{prefix}.downsample.lowpass.filter"))?;
    Ok(AliasFreeSnake::new(&alpha, &beta, &up_filter, &down_filter, 2))
}

// ---------------------------------------------------------------------------
// Config (config.json subset the decoder needs).
// ---------------------------------------------------------------------------

fn json_usize(value: Option<&JsonValue>, path: &str) -> Result<usize> {
    match value {
        Some(JsonValue::U64(n)) => Ok(*n as usize),
        Some(JsonValue::I64(n)) if *n >= 0 => Ok(*n as usize),
        Some(JsonValue::F64(n)) if *n >= 0.0 && n.fract() == 0.0 => Ok(*n as usize),
        _ => Err(DiffusionError::model(format!(
            "audio vae config: expected {path} to be a non-negative integer"
        ))),
    }
}

fn json_f32_item(value: &JsonValue, path: &str) -> Result<f32> {
    match value {
        JsonValue::F64(n) => Ok(*n as f32),
        JsonValue::I64(n) => Ok(*n as f32),
        JsonValue::U64(n) => Ok(*n as f32),
        _ => Err(DiffusionError::model(format!(
            "audio vae config: expected {path} entries to be numbers"
        ))),
    }
}

fn json_array<'a>(value: Option<&'a JsonValue>, path: &str) -> Result<&'a [JsonValue]> {
    match value {
        Some(JsonValue::Array(items)) => Ok(items),
        _ => Err(DiffusionError::model(format!(
            "audio vae config: expected {path} to be an array"
        ))),
    }
}

fn json_usize_vec(value: Option<&JsonValue>, path: &str) -> Result<Vec<usize>> {
    json_array(value, path)?
        .iter()
        .map(|item| json_usize(Some(item), path))
        .collect()
}

fn json_f32_vec(value: Option<&JsonValue>, path: &str) -> Result<Vec<f32>> {
    json_array(value, path)?
        .iter()
        .map(|item| json_f32_item(item, path))
        .collect()
}

// ---------------------------------------------------------------------------
// decode_rows layout (decoders.py): rows.reshape(2, n_lat, lc).permute(0,2,1)
// then per-channel denormalize `latents * std + mean`.
// ---------------------------------------------------------------------------

fn rows_to_latents(
    audio_rows: &[f32],
    n_lat: usize,
    latent_channels: usize,
    mean: &[f32],
    std: &[f32],
) -> Vec<f32> {
    let mut latents = vec![0f32; 2 * latent_channels * n_lat];
    for b in 0..2 {
        for t in 0..n_lat {
            let row = &audio_rows[(b * n_lat + t) * latent_channels..][..latent_channels];
            for (c, &v) in row.iter().enumerate() {
                latents[(b * latent_channels + c) * n_lat + t] = v * std[c] + mean[c];
            }
        }
    }
    latents
}

// ---------------------------------------------------------------------------
// Public model.
// ---------------------------------------------------------------------------

pub struct H3AudioVae {
    sampling_rate: u32,
    hop: usize,
    latent_channels: usize,
    latents_mean: Vec<f32>,
    latents_std: Vec<f32>,
    num_kernels: usize,
    dec_in_proj: Conv1d,
    conv_pre: Conv1d,
    stages: Vec<UpStage>,
    activation_post: AliasFreeSnake,
    conv_post: Conv1d,
    threads: usize,
}

impl H3AudioVae {
    /// Load config.json + the (single-file) safetensors checkpoint from
    /// `audio_vae_dir`. Only decoder-path tensors are read.
    pub fn load(audio_vae_dir: &Path) -> Result<Self> {
        let config_path = audio_vae_dir.join("config.json");
        let text = std::fs::read_to_string(&config_path)
            .map_err(|err| DiffusionError::io(&config_path, err.to_string()))?;
        let config = HashMap::<String, JsonValue>::deserialize_json(&text)
            .map_err(|err| DiffusionError::json(&config_path, format!("{err:?}")))?;

        let latent_dim = json_usize(config.get("latent_dim"), "latent_dim")?;
        let latent_channels = json_usize(config.get("latent_channels"), "latent_channels")?;
        let decoder_dim = json_usize(config.get("decoder_dim"), "decoder_dim")?;
        let decoder_rates = json_usize_vec(config.get("decoder_rates"), "decoder_rates")?;
        let decoder_kernel_sizes =
            json_usize_vec(config.get("decoder_kernel_sizes"), "decoder_kernel_sizes")?;
        let resblock_kernel_sizes =
            json_usize_vec(config.get("resblock_kernel_sizes"), "resblock_kernel_sizes")?;
        let resblock_dilation_sizes = json_array(
            config.get("resblock_dilation_sizes"),
            "resblock_dilation_sizes",
        )?
        .iter()
        .map(|entry| json_usize_vec(Some(entry), "resblock_dilation_sizes[i]"))
        .collect::<Result<Vec<Vec<usize>>>>()?;
        let sampling_rate = json_usize(config.get("sampling_rate"), "sampling_rate")? as u32;
        let latents_mean = json_f32_vec(config.get("latents_mean"), "latents_mean")?;
        let latents_std = json_f32_vec(config.get("latents_std"), "latents_std")?;

        if decoder_rates.len() != decoder_kernel_sizes.len() {
            return Err(DiffusionError::model(
                "audio vae config: decoder_rates / decoder_kernel_sizes length mismatch",
            ));
        }
        if resblock_kernel_sizes.len() != resblock_dilation_sizes.len() {
            return Err(DiffusionError::model(
                "audio vae config: resblock kernel / dilation length mismatch",
            ));
        }
        if latents_mean.len() != latent_channels || latents_std.len() != latent_channels {
            return Err(DiffusionError::model(format!(
                "audio vae config: latents_mean/std must have {latent_channels} entries"
            )));
        }
        let hop: usize = decoder_rates.iter().product();

        let weights = H3ShardedWeights::load(audio_vae_dir)?;

        // dec_in_proj is a plain (not weight-normed) 1x1 conv.
        let dec_in_weight = weights.tensor_f32("dec_in_proj.weight")?;
        let (din_out, din_in, din_k) = tensor_shape3(&weights, "dec_in_proj.weight")?;
        if (din_out, din_in, din_k) != (latent_dim, latent_channels, 1) {
            return Err(DiffusionError::model(format!(
                "audio vae dec_in_proj: expected ({latent_dim},{latent_channels},1), got \
                 ({din_out},{din_in},{din_k})"
            )));
        }
        let dec_in_proj = Conv1d {
            in_ch: din_in,
            out_ch: din_out,
            k: din_k,
            dilation: 1,
            padding: 0,
            weight: dec_in_weight,
            bias: weights.tensor_f32("dec_in_proj.bias")?,
        };

        let conv_pre = load_wn_conv1d(&weights, "decoder.conv_pre", 1, 3, true)?;
        if conv_pre.in_ch != latent_dim || conv_pre.out_ch != decoder_dim {
            return Err(DiffusionError::model(format!(
                "audio vae conv_pre: expected {latent_dim}->{decoder_dim}, got {}->{}",
                conv_pre.in_ch, conv_pre.out_ch
            )));
        }

        let num_kernels = resblock_kernel_sizes.len();
        let mut stages = Vec::with_capacity(decoder_rates.len());
        let mut channels = decoder_dim;
        for (i, (&rate, &kernel)) in decoder_rates
            .iter()
            .zip(decoder_kernel_sizes.iter())
            .enumerate()
        {
            channels /= 2;
            let up = load_wn_conv_transpose1d(
                &weights,
                &format!("decoder.ups.{i}.0"),
                rate,
                (kernel - rate) / 2,
            )?;
            if up.out_ch != channels || up.k != kernel {
                return Err(DiffusionError::model(format!(
                    "audio vae ups.{i}: expected out {channels} k {kernel}, got out {} k {}",
                    up.out_ch, up.k
                )));
            }
            let mut blocks = Vec::with_capacity(num_kernels);
            for j in 0..num_kernels {
                let block_index = i * num_kernels + j;
                let k = resblock_kernel_sizes[j];
                let dilations = &resblock_dilation_sizes[j];
                let mut convs1 = Vec::with_capacity(dilations.len());
                let mut convs2 = Vec::with_capacity(dilations.len());
                let mut acts = Vec::with_capacity(2 * dilations.len());
                for (di, &d) in dilations.iter().enumerate() {
                    convs1.push(load_wn_conv1d(
                        &weights,
                        &format!("decoder.resblocks.{block_index}.convs1.{di}"),
                        d,
                        (k * d - d) / 2,
                        true,
                    )?);
                    convs2.push(load_wn_conv1d(
                        &weights,
                        &format!("decoder.resblocks.{block_index}.convs2.{di}"),
                        1,
                        (k - 1) / 2,
                        true,
                    )?);
                }
                for a in 0..2 * dilations.len() {
                    acts.push(load_alias_free(
                        &weights,
                        &format!("decoder.resblocks.{block_index}.activations.{a}"),
                        channels,
                    )?);
                }
                blocks.push(AmpBlock { convs1, convs2, acts });
            }
            stages.push(UpStage { up, blocks });
        }

        let activation_post = load_alias_free(&weights, "decoder.activation_post", channels)?;
        let conv_post = load_wn_conv1d(&weights, "decoder.conv_post", 1, 3, false)?;
        if conv_post.in_ch != channels || conv_post.out_ch != 1 {
            return Err(DiffusionError::model(format!(
                "audio vae conv_post: expected {channels}->1, got {}->{}",
                conv_post.in_ch, conv_post.out_ch
            )));
        }

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        Ok(Self {
            sampling_rate,
            hop,
            latent_channels,
            latents_mean,
            latents_std,
            num_kernels,
            dec_in_proj,
            conv_pre,
            stages,
            activation_post,
            conv_post,
            threads,
        })
    }

    pub fn sampling_rate(&self) -> u32 {
        self.sampling_rate
    }

    /// One mono batch item: `(latent_channels, n_lat)` -> `n_lat * hop`
    /// samples, clamped to [-1, 1]. `on_stage` fires before each upsample
    /// stage (the decode's per-second-scale units).
    fn decode_mono(
        &self,
        latents: &[f32],
        n_lat: usize,
        on_stage: &mut dyn FnMut(usize),
    ) -> Vec<f32> {
        let threads = self.threads;
        let plane = Plane {
            ch: self.latent_channels,
            len: n_lat,
            data: latents.to_vec(),
        };
        let mut hidden = self.dec_in_proj.forward(&plane, threads);
        hidden = self.conv_pre.forward(&hidden, threads);
        for (stage_index, stage) in self.stages.iter().enumerate() {
            on_stage(stage_index);
            let up = stage.up.forward(&hidden, threads);
            let mut sum: Option<Plane> = None;
            for block in &stage.blocks {
                let out = block.forward(&up, threads);
                match &mut sum {
                    None => sum = Some(out),
                    Some(acc) => {
                        for (a, &b) in acc.data.iter_mut().zip(out.data.iter()) {
                            *a += b;
                        }
                    }
                }
            }
            let mut acc = sum.expect("decoder stage has resblocks");
            let inv = 1.0 / self.num_kernels as f32;
            for v in acc.data.iter_mut() {
                *v *= inv;
            }
            hidden = acc;
        }
        hidden = self.activation_post.forward(&hidden, threads);
        hidden = self.conv_post.forward(&hidden, threads);
        for v in hidden.data.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        hidden.data
    }

    /// Post-denormalize latents `(2, latent_channels, n_lat)` flattened ->
    /// `(2, samples)` stereo, channel-planar `[L..., R...]`,
    /// `samples = n_lat * 800`.
    pub fn decode_latents(&self, latents: &[f32], n_lat: usize) -> Result<Vec<f32>> {
        self.decode_latents_progress(latents, n_lat, None)
    }

    fn decode_latents_progress(
        &self,
        latents: &[f32],
        n_lat: usize,
        mut on_step: Option<&mut dyn FnMut(usize, usize)>,
    ) -> Result<Vec<f32>> {
        let per_item = self.latent_channels * n_lat;
        if n_lat == 0 || latents.len() != 2 * per_item {
            return Err(DiffusionError::model(format!(
                "audio vae decode_latents: expected 2*{}*{n_lat} = {} values, got {}",
                self.latent_channels,
                2 * per_item,
                latents.len()
            )));
        }
        let samples = n_lat * self.hop;
        let stage_count = self.stages.len();
        let total = 2 * stage_count;
        let mut out = Vec::with_capacity(2 * samples);
        for b in 0..2 {
            let mut on_stage = |stage: usize| {
                if let Some(on_step) = on_step.as_deref_mut() {
                    on_step(b * stage_count + stage + 1, total);
                }
            };
            let wave = self.decode_mono(
                &latents[b * per_item..(b + 1) * per_item],
                n_lat,
                &mut on_stage,
            );
            debug_assert_eq!(wave.len(), samples);
            out.extend_from_slice(&wave);
        }
        Ok(out)
    }

    /// Full decoders.py path from raw denoised audio rows: `2*n_lat` rows of
    /// `latent_channels` (all left-channel rows first, then all right) ->
    /// reshape + per-channel denormalize -> decode. Same output layout as
    /// [`Self::decode_latents`].
    pub fn decode_rows(&self, audio_rows: &[f32], num_audio_latents: usize) -> Result<Vec<f32>> {
        self.decode_rows_progress(audio_rows, num_audio_latents, None)
    }

    /// [`Self::decode_rows`] with a per-stage progress callback
    /// `(done, total)` across both stereo channels (total = 2 * stages).
    pub fn decode_rows_progress(
        &self,
        audio_rows: &[f32],
        num_audio_latents: usize,
        on_step: Option<&mut dyn FnMut(usize, usize)>,
    ) -> Result<Vec<f32>> {
        let n_lat = num_audio_latents;
        if n_lat == 0 || audio_rows.len() != 2 * n_lat * self.latent_channels {
            return Err(DiffusionError::model(format!(
                "audio vae decode_rows: expected 2*{n_lat}*{} = {} values, got {}",
                self.latent_channels,
                2 * n_lat * self.latent_channels,
                audio_rows.len()
            )));
        }
        let latents = rows_to_latents(
            audio_rows,
            n_lat,
            self.latent_channels,
            &self.latents_mean,
            &self.latents_std,
        );
        self.decode_latents_progress(&latents, n_lat, on_step)
    }
}

// ---------------------------------------------------------------------------
// Tests (self-contained, no weights needed).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(ch: usize, rows: &[&[f32]]) -> Plane {
        let len = rows[0].len();
        let mut data = Vec::new();
        for row in rows {
            assert_eq!(row.len(), len);
            data.extend_from_slice(row);
        }
        assert_eq!(rows.len(), ch);
        Plane { ch, len, data }
    }

    #[test]
    fn snake_beta_matches_reference_formula() {
        // Reference: x + (exp(beta)+1e-9)^-1 * sin(exp(alpha)*x)^2, with the
        // struct's precomputed-per-channel path vs the direct f64 formula.
        let alpha_log = 0.3f32;
        let beta_log = -0.2f32;
        let xs = [-1.5f32, -0.25, 0.0, 0.7, 3.2];
        for &x in &xs {
            let mut row = [x];
            snake_beta_in_place(
                &mut row,
                alpha_log.exp(),
                1.0 / (beta_log.exp() + 1e-9),
            );
            let expected = x as f64
                + (1.0 / ((beta_log as f64).exp() + 1e-9))
                    * ((alpha_log as f64).exp() * x as f64).sin().powi(2);
            assert!(
                (row[0] as f64 - expected).abs() < 1e-6,
                "snake({x}) = {} vs {expected}",
                row[0]
            );
        }
    }

    #[test]
    fn weight_norm_reconstruction() {
        // Two rows of (in*k)=2: norms 5 and 5, gains 2 and -1.
        let v = [3.0f32, 4.0, 0.0, -5.0];
        let g = [2.0f32, -1.0];
        let w = weight_norm_dim0(&v, &g, 2, 2);
        let expected = [1.2f32, 1.6, 0.0, 1.0];
        for (a, e) in w.iter().zip(expected.iter()) {
            assert!((a - e).abs() < 1e-6, "{w:?} vs {expected:?}");
        }
    }

    #[test]
    fn conv_weight_layout_accepts_only_one_complete_representation() {
        assert_eq!(
            conv_weight_storage(true, true, false, "conv").unwrap(),
            ConvWeightStorage::WeightNorm
        );
        assert_eq!(
            conv_weight_storage(false, false, true, "conv").unwrap(),
            ConvWeightStorage::Fused
        );
        for presence in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
            (true, false, true),
            (false, true, true),
        ] {
            assert!(conv_weight_storage(presence.0, presence.1, presence.2, "conv").is_err());
        }
    }

    #[test]
    fn conv1d_hand_computed() {
        // k=3, padding=1: out[t] = 0.5 + w0*x[t-1] + w1*x[t] + w2*x[t+1].
        let conv = Conv1d {
            in_ch: 1,
            out_ch: 1,
            k: 3,
            dilation: 1,
            padding: 1,
            weight: vec![1.0, 2.0, 3.0],
            bias: vec![0.5],
        };
        let out = conv.forward(&plane(1, &[&[1.0, 2.0, 3.0, 4.0]]), 1);
        assert_eq!(out.ch, 1);
        assert_eq!(out.len, 4);
        assert_eq!(out.data, vec![8.5, 14.5, 20.5, 11.5]);
    }

    #[test]
    fn conv1d_dilated_and_multichannel() {
        // k=3, dilation=2, padding=2: out[t] = x[t-2] + x[t] + x[t+2].
        let conv = Conv1d {
            in_ch: 1,
            out_ch: 1,
            k: 3,
            dilation: 2,
            padding: 2,
            weight: vec![1.0, 1.0, 1.0],
            bias: vec![0.0],
        };
        let out = conv.forward(&plane(1, &[&[1.0, 2.0, 3.0, 4.0, 5.0]]), 1);
        assert_eq!(out.data, vec![4.0, 6.0, 9.0, 6.0, 8.0]);

        // 1x1 conv over two input channels: out = 2*x0 + 3*x1.
        let conv = Conv1d {
            in_ch: 2,
            out_ch: 1,
            k: 1,
            dilation: 1,
            padding: 0,
            weight: vec![2.0, 3.0],
            bias: vec![0.0],
        };
        let out = conv.forward(&plane(2, &[&[1.0, 2.0], &[10.0, 20.0]]), 1);
        assert_eq!(out.data, vec![32.0, 64.0]);
    }

    #[test]
    fn conv_transpose1d_hand_computed() {
        // x=[1,2,3], w=[1,10,100,1000], stride=2, padding=1:
        // out[ti*2 + k - 1] += w[k]*x[ti], out_len = 2*2 - 2 + 4 = 6.
        let conv = ConvTranspose1d {
            in_ch: 1,
            out_ch: 1,
            k: 4,
            stride: 2,
            padding: 1,
            weight: vec![1.0, 10.0, 100.0, 1000.0],
            bias: vec![0.5],
        };
        let out = conv.forward(&plane(1, &[&[1.0, 2.0, 3.0]]), 1);
        assert_eq!(out.len, 6);
        assert_eq!(
            out.data,
            vec![10.5, 102.5, 1020.5, 203.5, 2030.5, 300.5]
        );
    }

    #[test]
    fn alias_free_delta_filters_are_identity() {
        // With 12-tap delta filters at tap 5 (raw up tap 0.5, so the folded
        // x2 gain makes it exactly 1.0) and beta_log so large that
        // 1/(exp(beta)+1e-9) underflows f32 addition, the whole
        // upsample->snake->downsample chain is exactly the identity. This
        // pins the replicate-pad / crop / stride arithmetic.
        let mut up = [0f32; 12];
        up[5] = 0.5;
        let mut down = [0f32; 12];
        down[5] = 1.0;
        let act = AliasFreeSnake::new(&[0.0, 0.0], &[40.0, 40.0], &up, &down, 2);
        let x = plane(
            2,
            &[
                &[0.1, -0.5, 2.0, 3.0, -1.25, 0.0, 7.5],
                &[-3.0, 4.5, 0.25, -0.125, 1.0, 2.0, -2.0],
            ],
        );
        let out = act.forward(&x, 2);
        assert_eq!(out.ch, 2);
        assert_eq!(out.len, 7);
        assert_eq!(out.data, x.data);
    }

    #[test]
    fn alias_free_length_arithmetic() {
        // Non-degenerate filters: output must stay (ch, len) for odd/even len.
        let filt: Vec<f32> = (0..12).map(|i| (i as f32 - 5.5) / 40.0).collect();
        let act = AliasFreeSnake::new(&[0.1], &[-0.3], &filt, &filt, 2);
        for len in [1usize, 2, 5, 8, 33] {
            let x = Plane { ch: 1, len, data: (0..len).map(|i| i as f32 * 0.1).collect() };
            let out = act.forward(&x, 1);
            assert_eq!((out.ch, out.len, out.data.len()), (1, len, len));
        }
    }

    #[test]
    fn rows_to_latents_layout_and_denormalize() {
        // n_lat=2, lc=3; rows are (2*n_lat, lc), left rows first.
        let rows = [
            1.0f32, 2.0, 3.0, // b0 t0
            4.0, 5.0, 6.0, // b0 t1
            7.0, 8.0, 9.0, // b1 t0
            10.0, 11.0, 12.0, // b1 t1
        ];
        let mean = [0.5f32, 0.0, -1.0];
        let std = [2.0f32, 1.0, 3.0];
        let latents = rows_to_latents(&rows, 2, 3, &mean, &std);
        let expected = [
            2.5f32, 8.5, // b0 c0
            2.0, 5.0, // b0 c1
            8.0, 17.0, // b0 c2
            14.5, 20.5, // b1 c0
            8.0, 11.0, // b1 c1
            26.0, 35.0, // b1 c2
        ];
        assert_eq!(latents, expected);
    }

    #[test]
    fn par_rows_covers_all_rows() {
        let mut data = vec![0f32; 7 * 3];
        par_rows(4, &mut data, 3, &|row_index, row| {
            for (i, v) in row.iter_mut().enumerate() {
                *v = (row_index * 10 + i) as f32;
            }
        });
        for r in 0..7 {
            for i in 0..3 {
                assert_eq!(data[r * 3 + i], (r * 10 + i) as f32);
            }
        }
    }
}
