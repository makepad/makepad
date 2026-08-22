//! IndexTTS-2.5 BigVGAN v2 vocoder (nvidia `bigvgan_v2_22khz_80band_256x`),
//! CPU f32: 80-band mel @ 22.05 kHz -> waveform, 256x upsampling.
//!
//! Port of `indextts/s2mel/modules/bigvgan/bigvgan.py` (+ activations.py and
//! alias_free_activation/torch/{act,resample,filter}.py) at the checkpoint's
//! config.json:
//!
//!   conv_pre: weight-norm Conv1d(80 -> 1536, k7, p3)
//!   6 upsample stages (rates 4,4,2,2,2,2 / kernels 8,8,4,4,4,4):
//!     ups.i.0: weight-norm ConvTranspose1d(C -> C/2, k, stride=rate,
//!                                          padding=(k-rate)/2)
//!     then the MEAN of 3 AMPBlock1 (kernels 3/7/11, dilations 1/3/5; per
//!     dilation `x += conv2(act2(conv1(act1(x))))`, every conv preceded by
//!     its own anti-aliased SnakeBeta activation)
//!   activation_post: anti-aliased SnakeBeta on the final 24 channels
//!   conv_post: weight-norm Conv1d(24 -> 1, k7, p3, bias=False)
//!   use_tanh_at_final=false -> clamp to [-1, 1]
//!
//! Anti-aliased activation (`Activation1d`, ratio 2, 12 taps): replicate-pad
//! -> depthwise Kaiser-sinc transposed conv (gain `ratio`) -> crop -> snakebeta
//! `x + sin(exp(alpha)*x)^2 / (exp(beta)+1e-9)` per channel -> replicate-pad
//! -> depthwise strided Kaiser-sinc conv back to 1x. This is the same
//! alias-free-torch lineage as the MiniMax H3 audio VAE; the `Plane` /
//! `par_rows` / conv / `AliasFreeSnake` building blocks below are local
//! replicas of the (private) ones in `src/h3_audio_vae.rs`.
//!
//! Filter provenance: BigVGAN computes its 12-tap Kaiser-sinc filters
//! procedurally (`kaiser_sinc_filter1d(cutoff=0.25, half_width=0.3, 12)`,
//! Kaiser attenuation A = 2.285*(half_size-1)*pi*4*half_width + 7.95 = 51.02
//! -> beta = 0.1102*(A-8.7) = 4.6638), but `register_buffer` persists them,
//! so the checkpoint carries the exact taps the reference model runs with
//! (all 220 filter buffers in the checkpoint are bit-identical). We LOAD the
//! taps from the checkpoint (guaranteed parity with the reference) and
//! cross-check them at load time against [`kaiser_sinc_filter1d`] computed in
//! Rust (agreement measured at 3e-8).
//!
//! Weights: `bigvgan_generator.pt`, keys under `generator.` with old-style
//! `weight_g`/`weight_v` weight-norm pairs (the reference calls
//! `remove_weight_norm()` after loading; we fold `w = g * v / ||v||` at load,
//! per OUT channel for Conv1d, per IN channel for ConvTranspose1d).

use crate::error::{DiffusionError, Result};
use crate::torch_pth::PthStateDict;
use std::path::Path;

/// Mel bands consumed (config num_mels).
pub const BIGVGAN_MELS: usize = 80;
/// Total upsampling factor: samples per mel frame (4*4*2*2*2*2).
pub const BIGVGAN_HOP: usize = 256;
/// Output sample rate.
pub const BIGVGAN_SAMPLE_RATE: u32 = 22_050;

const UPSAMPLE_RATES: [usize; 6] = [4, 4, 2, 2, 2, 2];
const UPSAMPLE_KERNELS: [usize; 6] = [8, 8, 4, 4, 4, 4];
const INITIAL_CHANNEL: usize = 1536;
const RESBLOCK_KERNELS: [usize; 3] = [3, 7, 11];
const RESBLOCK_DILATIONS: [usize; 3] = [1, 3, 5];
const ALIAS_RATIO: usize = 2;
const ALIAS_TAPS: usize = 12;

// ---------------------------------------------------------------------------
// (channels, length) f32 plane + thread helper (local replica of the private
// h3_audio_vae.rs versions).
// ---------------------------------------------------------------------------

pub(crate) struct Plane {
    pub(crate) ch: usize,
    pub(crate) len: usize,
    pub(crate) data: Vec<f32>,
}

/// Run `f(row_index, row)` over the `row_len`-strided rows of `data`, split
/// across `threads` std::thread workers (contiguous row chunks per worker).
/// (`pub(crate)`: the s2mel CUDA path reuses it for its interim host-side
/// WaveNet gate until the device kernel lands.)
pub(crate) fn par_rows<F: Fn(usize, &mut [f32]) + Sync>(
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
// Layers (local replicas of the private h3_audio_vae.rs versions).
// ---------------------------------------------------------------------------

/// Conv1d, stride 1, zero padding, optional dilation. Weight `(out, in, k)`.
pub(crate) struct Conv1d {
    pub(crate) in_ch: usize,
    pub(crate) out_ch: usize,
    pub(crate) k: usize,
    pub(crate) dilation: usize,
    pub(crate) padding: usize,
    pub(crate) weight: Vec<f32>,
    /// Empty when the layer has no bias (conv_post, use_bias_at_final=false).
    pub(crate) bias: Vec<f32>,
}

impl Conv1d {
    pub(crate) fn forward(&self, x: &Plane, threads: usize) -> Plane {
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
                    if t1 <= t0 as isize {
                        continue;
                    }
                    let t1 = t1 as usize;
                    let x_slice =
                        &x_row[(t0 as isize + off) as usize..(t1 as isize + off) as usize];
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
pub(crate) struct ConvTranspose1d {
    pub(crate) in_ch: usize,
    pub(crate) out_ch: usize,
    pub(crate) k: usize,
    pub(crate) stride: usize,
    pub(crate) padding: usize,
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
}

impl ConvTranspose1d {
    pub(crate) fn forward(&self, x: &Plane, threads: usize) -> Plane {
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

/// Anti-aliased SnakeBeta activation (`Activation1d`): replicate-pad ->
/// depthwise Kaiser-sinc transposed conv (2x, scaled by the ratio) -> crop ->
/// `x + sin(exp(alpha)*x)^2 / (exp(beta)+1e-9)` -> replicate-pad -> depthwise
/// strided Kaiser-sinc conv back down. Padding arithmetic follows
/// resample.py (`pad = k/ratio - 1`, crops `pad*ratio + (k-ratio)/2` /
/// `pad*ratio + (k-ratio+1)/2`) and filter.py (`pad_left = k/2 - even`,
/// `pad_right = k/2`) exactly — identical to the h3_audio_vae.rs replica.
pub(crate) struct AliasFreeSnake {
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
    /// space, snake_logscale=true); `up_filter` / `down_filter` the raw taps.
    fn new(
        alpha_log: &[f32],
        beta_log: &[f32],
        up_filter: &[f32],
        down_filter: &[f32],
        ratio: usize,
    ) -> Self {
        let k_up = up_filter.len();
        let k_down = down_filter.len();
        // UpSample1d arithmetic (resample.py).
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
        // LowPassFilter1d arithmetic (filter.py).
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

    pub(crate) fn forward(&self, x: &Plane, threads: usize) -> Plane {
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

/// AMPBlock1: per dilation `x += conv2(act2(conv1(act1(x))))`.
pub(crate) struct AmpBlock {
    pub(crate) convs1: Vec<Conv1d>,
    pub(crate) convs2: Vec<Conv1d>,
    pub(crate) acts: Vec<AliasFreeSnake>,
}

impl AmpBlock {
    pub(crate) fn forward(&self, x: &Plane, threads: usize) -> Plane {
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

pub(crate) struct UpStage {
    pub(crate) up: ConvTranspose1d,
    pub(crate) blocks: Vec<AmpBlock>,
}

// ---------------------------------------------------------------------------
// Kaiser-sinc filter (port of alias_free_activation/torch/filter.py).
// ---------------------------------------------------------------------------

/// Modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut sum = 1.0f64;
    let mut term = 1.0f64;
    for k in 1..=64 {
        let f = half / k as f64;
        term *= f * f;
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

/// `filter.py::kaiser_sinc_filter1d(cutoff, half_width, kernel_size)`: a
/// Kaiser-windowed sinc low-pass, normalized to sum 1. Kaiser attenuation
/// `A = 2.285*(kernel_size/2 - 1)*pi*(4*half_width) + 7.95`; beta from the
/// standard A-dependent piecewise formula; symmetric (periodic=False)
/// window; even kernels sample time at `arange(-k/2, k/2) + 0.5`.
/// BigVGAN's `Activation1d` uses `cutoff=0.5/ratio`, `half_width=0.6/ratio`,
/// `kernel_size=12` for both the 2x up and 2x down paths.
pub fn kaiser_sinc_filter1d(cutoff: f64, half_width: f64, kernel_size: usize) -> Vec<f32> {
    assert!(cutoff > 0.0 && cutoff <= 0.5, "cutoff must be in (0, 0.5]");
    let even = kernel_size % 2 == 0;
    let half_size = (kernel_size / 2) as f64;
    let delta_f = 4.0 * half_width;
    let a = 2.285 * (half_size - 1.0) * std::f64::consts::PI * delta_f + 7.95;
    let beta = if a > 50.0 {
        0.1102 * (a - 8.7)
    } else if a >= 21.0 {
        0.5842 * (a - 21.0).powf(0.4) + 0.07886 * (a - 21.0)
    } else {
        0.0
    };
    let i0_beta = bessel_i0(beta);
    let m = (kernel_size - 1) as f64;
    let mut taps = vec![0f64; kernel_size];
    let mut sum = 0f64;
    for (n, tap) in taps.iter_mut().enumerate() {
        // Symmetric Kaiser window: I0(beta*sqrt(1 - (2n/(N-1) - 1)^2))/I0(beta).
        let r = 2.0 * n as f64 / m - 1.0;
        let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
        let time = if even {
            n as f64 - half_size + 0.5
        } else {
            n as f64 - half_size
        };
        // sinc(x) = sin(pi x)/(pi x).
        let arg = 2.0 * cutoff * time;
        let sinc = if arg == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
        };
        *tap = 2.0 * cutoff * window * sinc;
        sum += *tap;
    }
    taps.iter().map(|t| (t / sum) as f32).collect()
}

// ---------------------------------------------------------------------------
// Weight loading (weight_g/weight_v folding, PthStateDict flavor).
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

fn shape3(sd: &PthStateDict, name: &str) -> Result<(usize, usize, usize)> {
    let shape = sd.shape(name)?;
    if shape.len() != 3 {
        return Err(DiffusionError::model(format!(
            "bigvgan tensor '{name}': expected rank 3, got {shape:?}"
        )));
    }
    Ok((shape[0], shape[1], shape[2]))
}

fn load_wn_conv1d(
    sd: &mut PthStateDict,
    prefix: &str,
    dilation: usize,
    padding: usize,
    has_bias: bool,
) -> Result<Conv1d> {
    let g_name = format!("{prefix}.weight_g");
    let v_name = format!("{prefix}.weight_v");
    let (out_ch, in_ch, k) = shape3(sd, &v_name)?;
    let g_shape = shape3(sd, &g_name)?;
    if g_shape != (out_ch, 1, 1) {
        return Err(DiffusionError::model(format!(
            "bigvgan '{g_name}': expected shape ({out_ch},1,1), got {g_shape:?}"
        )));
    }
    let v = sd.f32(&v_name)?;
    let g = sd.f32(&g_name)?;
    let weight = weight_norm_dim0(&v, &g, out_ch, in_ch * k);
    let bias = if has_bias {
        sd.f32(&format!("{prefix}.bias"))?
    } else {
        Vec::new()
    };
    Ok(Conv1d { in_ch, out_ch, k, dilation, padding, weight, bias })
}

fn load_wn_conv_transpose1d(
    sd: &mut PthStateDict,
    prefix: &str,
    stride: usize,
    padding: usize,
) -> Result<ConvTranspose1d> {
    let g_name = format!("{prefix}.weight_g");
    let v_name = format!("{prefix}.weight_v");
    // torch ConvTranspose1d weight layout is (in, out, k); weight_norm dim 0
    // therefore normalizes per input channel.
    let (in_ch, out_ch, k) = shape3(sd, &v_name)?;
    let g_shape = shape3(sd, &g_name)?;
    if g_shape != (in_ch, 1, 1) {
        return Err(DiffusionError::model(format!(
            "bigvgan '{g_name}': expected shape ({in_ch},1,1), got {g_shape:?}"
        )));
    }
    let v = sd.f32(&v_name)?;
    let g = sd.f32(&g_name)?;
    let weight = weight_norm_dim0(&v, &g, in_ch, out_ch * k);
    let bias = sd.f32(&format!("{prefix}.bias"))?;
    Ok(ConvTranspose1d { in_ch, out_ch, k, stride, padding, weight, bias })
}

/// Loads one `Activation1d` (snake alpha/beta + the two persistent 12-tap
/// filter buffers) and cross-checks the checkpoint taps against the
/// procedurally computed Kaiser-sinc filter.
fn load_alias_free(
    sd: &mut PthStateDict,
    prefix: &str,
    channels: usize,
    expected_taps: &[f32],
) -> Result<AliasFreeSnake> {
    let alpha = sd.f32(&format!("{prefix}.act.alpha"))?;
    let beta = sd.f32(&format!("{prefix}.act.beta"))?;
    if alpha.len() != channels || beta.len() != channels {
        return Err(DiffusionError::model(format!(
            "bigvgan '{prefix}': expected {channels}-channel snake, got alpha {} beta {}",
            alpha.len(),
            beta.len()
        )));
    }
    let up_filter = sd.f32(&format!("{prefix}.upsample.filter"))?;
    let down_filter = sd.f32(&format!("{prefix}.downsample.lowpass.filter"))?;
    for (which, taps) in [("upsample", &up_filter), ("downsample", &down_filter)] {
        if taps.len() != expected_taps.len() {
            return Err(DiffusionError::model(format!(
                "bigvgan '{prefix}' {which} filter: {} taps, expected {}",
                taps.len(),
                expected_taps.len()
            )));
        }
        let max_diff = taps
            .iter()
            .zip(expected_taps)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        if max_diff > 1e-5 {
            return Err(DiffusionError::model(format!(
                "bigvgan '{prefix}' {which} filter deviates {max_diff:e} from the computed \
                 Kaiser-sinc taps — checkpoint/architecture mismatch"
            )));
        }
    }
    Ok(AliasFreeSnake::new(
        &alpha,
        &beta,
        &up_filter,
        &down_filter,
        ALIAS_RATIO,
    ))
}

// ---------------------------------------------------------------------------
// Public model.
// ---------------------------------------------------------------------------

pub struct IndexTtsBigVgan {
    pub(crate) conv_pre: Conv1d,
    pub(crate) stages: Vec<UpStage>,
    pub(crate) activation_post: AliasFreeSnake,
    pub(crate) conv_post: Conv1d,
    pub(crate) num_kernels: usize,
    pub(crate) threads: usize,
    /// Device path, built at load when CUDA is present. A build failure
    /// fails the load loudly — there is no silent CPU fallback on CUDA
    /// machines.
    pub(crate) cuda_session: Option<cuda::BigVganCudaSession>,
}

impl IndexTtsBigVgan {
    /// Loads `bigvgan_generator.pt` (keys under `generator.`) and folds the
    /// weight norms, mirroring the reference's load + `remove_weight_norm()`.
    pub fn load(checkpoint: &Path) -> Result<Self> {
        let mut sd = PthStateDict::load_nested(checkpoint)?;
        let expected_taps = kaiser_sinc_filter1d(
            0.5 / ALIAS_RATIO as f64,
            0.6 / ALIAS_RATIO as f64,
            ALIAS_TAPS,
        );

        let conv_pre = load_wn_conv1d(&mut sd, "generator.conv_pre", 1, 3, true)?;
        if conv_pre.in_ch != BIGVGAN_MELS || conv_pre.out_ch != INITIAL_CHANNEL || conv_pre.k != 7
        {
            return Err(DiffusionError::model(format!(
                "bigvgan conv_pre: expected {BIGVGAN_MELS}->{INITIAL_CHANNEL} k7, got {}->{} k{}",
                conv_pre.in_ch, conv_pre.out_ch, conv_pre.k
            )));
        }

        let num_kernels = RESBLOCK_KERNELS.len();
        let mut stages = Vec::with_capacity(UPSAMPLE_RATES.len());
        let mut channels = INITIAL_CHANNEL;
        for (i, (&rate, &kernel)) in UPSAMPLE_RATES
            .iter()
            .zip(UPSAMPLE_KERNELS.iter())
            .enumerate()
        {
            channels /= 2;
            let up = load_wn_conv_transpose1d(
                &mut sd,
                &format!("generator.ups.{i}.0"),
                rate,
                (kernel - rate) / 2,
            )?;
            if up.out_ch != channels || up.k != kernel {
                return Err(DiffusionError::model(format!(
                    "bigvgan ups.{i}: expected out {channels} k {kernel}, got out {} k {}",
                    up.out_ch, up.k
                )));
            }
            let mut blocks = Vec::with_capacity(num_kernels);
            for (j, &k) in RESBLOCK_KERNELS.iter().enumerate() {
                let block_index = i * num_kernels + j;
                let mut convs1 = Vec::with_capacity(RESBLOCK_DILATIONS.len());
                let mut convs2 = Vec::with_capacity(RESBLOCK_DILATIONS.len());
                let mut acts = Vec::with_capacity(2 * RESBLOCK_DILATIONS.len());
                for (di, &d) in RESBLOCK_DILATIONS.iter().enumerate() {
                    convs1.push(load_wn_conv1d(
                        &mut sd,
                        &format!("generator.resblocks.{block_index}.convs1.{di}"),
                        d,
                        d * (k - 1) / 2,
                        true,
                    )?);
                    convs2.push(load_wn_conv1d(
                        &mut sd,
                        &format!("generator.resblocks.{block_index}.convs2.{di}"),
                        1,
                        (k - 1) / 2,
                        true,
                    )?);
                }
                for a in 0..2 * RESBLOCK_DILATIONS.len() {
                    acts.push(load_alias_free(
                        &mut sd,
                        &format!("generator.resblocks.{block_index}.activations.{a}"),
                        channels,
                        &expected_taps,
                    )?);
                }
                blocks.push(AmpBlock { convs1, convs2, acts });
            }
            stages.push(UpStage { up, blocks });
        }

        let activation_post =
            load_alias_free(&mut sd, "generator.activation_post", channels, &expected_taps)?;
        // use_bias_at_final=false: conv_post has no bias tensor.
        let conv_post = load_wn_conv1d(&mut sd, "generator.conv_post", 1, 3, false)?;
        if conv_post.in_ch != channels || conv_post.out_ch != 1 {
            return Err(DiffusionError::model(format!(
                "bigvgan conv_post: expected {channels}->1, got {}->{}",
                conv_post.in_ch, conv_post.out_ch
            )));
        }

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let mut model = Self {
            conv_pre,
            stages,
            activation_post,
            conv_post,
            num_kernels,
            threads,
            cuda_session: None,
        };
        if cuda::bigvgan_cuda_available() {
            model.cuda_session = Some(cuda::BigVganCudaSession::new(&model)?);
        }
        Ok(model)
    }

    /// Vocodes a mel plane `(80, frames)` row-major `[mel][t]` (the layout of
    /// the s2mel `vc_target`) into `frames * 256` samples clamped to [-1, 1]
    /// (use_tanh_at_final=false takes the clamp branch of the reference).
    /// Dispatches to the device path when present (parity-gated by
    /// indextts_cuda_validate); CPU reference otherwise.
    pub fn synthesize(&self, mel: &[f32], frames: usize) -> Result<Vec<f32>> {
        if let Some(session) = &self.cuda_session {
            return session.synthesize(self, mel, frames);
        }
        self.synthesize_cpu(mel, frames)
    }

    /// Whether [`Self::synthesize`] dispatches to the device path (used by the
    /// validator to run the explicit CUDA-vs-CPU gate).
    pub fn cuda_active(&self) -> bool {
        self.cuda_session.is_some()
    }

    /// The CPU f32 reference path (also the parity baseline the CUDA gate
    /// compares against).
    pub fn synthesize_cpu(&self, mel: &[f32], frames: usize) -> Result<Vec<f32>> {
        if frames == 0 || mel.len() != BIGVGAN_MELS * frames {
            return Err(DiffusionError::model(format!(
                "bigvgan synthesize: expected {BIGVGAN_MELS}*{frames} = {} values, got {}",
                BIGVGAN_MELS * frames,
                mel.len()
            )));
        }
        let threads = self.threads;
        let plane = Plane {
            ch: BIGVGAN_MELS,
            len: frames,
            data: mel.to_vec(),
        };
        let mut hidden = self.conv_pre.forward(&plane, threads);
        for stage in &self.stages {
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
            let mut acc = sum.expect("bigvgan stage has resblocks");
            // forward(): x = xs / self.num_kernels (mean of the 3 AMP blocks).
            let inv = 1.0 / self.num_kernels as f32;
            for v in acc.data.iter_mut() {
                *v *= inv;
            }
            hidden = acc;
        }
        hidden = self.activation_post.forward(&hidden, threads);
        hidden = self.conv_post.forward(&hidden, threads);
        debug_assert_eq!(hidden.ch, 1);
        debug_assert_eq!(hidden.len, frames * BIGVGAN_HOP);
        for v in hidden.data.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        Ok(hidden.data)
    }
}

// ---------------------------------------------------------------------------
// Tests (synthetic; the waveform oracle lives in indextts-bigvgan-validate).
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

    /// The 12 taps of `kaiser_sinc_filter1d(0.25, 0.3, 12)` as computed by
    /// torch in the reference venv (== every filter buffer in the BigVGAN
    /// checkpoint, all of which are bit-identical).
    const TORCH_TAPS: [f32; 12] = [
        2.028964693e-3,
        9.389466606e-3,
        -2.554345876e-2,
        -5.765738338e-2,
        1.285725683e-1,
        4.432098269e-1,
        4.432098269e-1,
        1.285725683e-1,
        -5.765738338e-2,
        -2.554345876e-2,
        9.389466606e-3,
        2.028964693e-3,
    ];

    #[test]
    fn kaiser_filter_matches_torch_reference() {
        let taps = kaiser_sinc_filter1d(0.25, 0.3, 12);
        assert_eq!(taps.len(), 12);
        for (i, (a, b)) in taps.iter().zip(TORCH_TAPS.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "tap {i}: computed {a} vs torch {b}"
            );
        }
    }

    #[test]
    fn kaiser_filter_symmetry_and_dc_gain() {
        for kernel_size in [12usize, 16, 24] {
            let taps = kaiser_sinc_filter1d(0.25, 0.3, kernel_size);
            let sum: f64 = taps.iter().map(|&t| t as f64).sum();
            assert!((sum - 1.0).abs() < 1e-6, "k={kernel_size}: sum {sum}");
            for i in 0..kernel_size / 2 {
                assert_eq!(
                    taps[i],
                    taps[kernel_size - 1 - i],
                    "k={kernel_size}: tap {i} asymmetric"
                );
            }
        }
    }

    #[test]
    fn snake_beta_matches_reference_formula() {
        // SnakeBeta (logscale): x + (exp(beta)+1e-9)^-1 * sin(exp(alpha)*x)^2.
        let alpha_log = 0.3f32;
        let beta_log = -0.2f32;
        for &x in &[-1.5f32, -0.25, 0.0, 0.7, 3.2] {
            let mut row = [x];
            snake_beta_in_place(&mut row, alpha_log.exp(), 1.0 / (beta_log.exp() + 1e-9));
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
        assert_eq!((out.ch, out.len), (1, 4));
        assert_eq!(out.data, vec![8.5, 14.5, 20.5, 11.5]);

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
        assert_eq!(out.data, vec![10.5, 102.5, 1020.5, 203.5, 2030.5, 300.5]);
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
        assert_eq!((out.ch, out.len), (2, 7));
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

    /// Oracle smoke (skipped when the reference checkout is absent): the
    /// checkpoint's persistent filter buffers must match the Rust-computed
    /// Kaiser-sinc taps. Reads only two 12-tap tensors — fast even in debug.
    #[test]
    fn checkpoint_filter_taps_match_computed() {
        let path = crate::indextts::reference_checkpoints_dir()
            .join("hf_cache/bigvgan/bigvgan_generator.pt");
        if !path.is_file() {
            eprintln!("skipping checkpoint_filter_taps_match_computed: {path:?} missing");
            return;
        }
        let mut sd = PthStateDict::load_nested(&path).unwrap();
        let expected = kaiser_sinc_filter1d(0.25, 0.3, 12);
        for name in [
            "generator.resblocks.0.activations.0.upsample.filter",
            "generator.activation_post.downsample.lowpass.filter",
        ] {
            let taps = sd.f32(name).unwrap();
            assert_eq!(taps.len(), 12, "{name}");
            let max_diff = taps
                .iter()
                .zip(expected.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(max_diff < 1e-6, "{name}: max diff {max_diff:e}");
        }
    }
}

#[path = "indextts_bigvgan_cuda.rs"]
pub mod cuda;
