//! Woosh-AE VOCOS decoder (VocosAutoEncoder.inverse path), CPU f32.
//!
//! z (128 x 501) -> de-normalize (z * z_std + z_mean) -> Conv1d(128->2048,
//! k7 p3) embed -> LayerNorm -> 8x ConvNeXt blocks (depthwise k7 conv ->
//! LN -> Linear 2048->3072 -> exact-erf GELU -> Linear 3072->2048 ->
//! layer-scale gamma -> residual) -> final LN -> ISTFTCircleHead:
//! Conv1d(2048->1443, k1) -> chunk3 (mag, x, y); mag = softplus(mag),
//! phase circle-normalized by sqrt(clamp(x^2+y^2, 1e-8, 1e3)); S = mag *
//! (x + iy)/pmag -> centered torch.istft (n_fft 960, hop 480, hann window
//! from the checkpoint buffer) -> 240,000 mono 48 kHz samples.
//!
//! Deterministic (no decoder noise). The inverse rFFT is evaluated as a GEMM
//! against a precomputed (960 x 962) basis — the irfft convention ignores
//! the imaginary parts of the DC and Nyquist bins, matching torch.
//!
//! Weights: `checkpoints/Woosh-AE/weights.safetensors` (`autoencoder.decoder.*`
//! + `z_mean`/`z_std`); the encoder half is not ported (only text-to-audio).

use crate::error::{DiffusionError, Result};
use crate::sa3::{linear, par_rows, Sa3Tensors};
use crate::woosh::{
    gelu_erf, softplus, WOOSH_AE_BINS, WOOSH_AE_DIM, WOOSH_AE_HEAD_OUT, WOOSH_AE_HOP,
    WOOSH_AE_INTER, WOOSH_AE_LAYERS, WOOSH_AE_NFFT, WOOSH_AE_SAMPLES, WOOSH_LATENT_DIM,
    WOOSH_LATENT_FRAMES, WOOSH_LN_EPS,
};
use crate::{emit_progress, ProgressHook};

const K: usize = 7;
const PAD: usize = 3;

struct CnxBlock {
    dw_w: Vec<f32>, // (2048 x 7) depthwise taps
    dw_b: Vec<f32>,
    ln_w: Vec<f32>,
    ln_b: Vec<f32>,
    pw1_w: Vec<f32>, // (3072 x 2048)
    pw1_b: Vec<f32>,
    pw2_w: Vec<f32>, // (2048 x 3072)
    pw2_b: Vec<f32>,
    gamma: Vec<f32>,
}

pub struct WooshAe {
    z_mean: Vec<f32>,
    z_std: Vec<f32>,
    /// Embed conv reordered to (2048 x 896): out[o] += w[o][k*128+c] * in-window.
    embed_w: Vec<f32>,
    embed_b: Vec<f32>,
    in_ln_w: Vec<f32>,
    in_ln_b: Vec<f32>,
    blocks: Vec<CnxBlock>,
    final_ln_w: Vec<f32>,
    final_ln_b: Vec<f32>,
    head_w: Vec<f32>, // (1443 x 2048)
    head_b: Vec<f32>,
    window: Vec<f32>, // (960) hann, from the checkpoint buffer
    /// irfft-as-GEMM basis, (960 x 962): sample row n dot [Re bins | Im bins].
    istft_basis: Vec<f32>,
}

/// Validation taps recorded by [`WooshAe::decode_with_taps`]:
/// ("z_denorm"/"embed"/"cnx{N}"/"backbone"/"head_conv", channel-major data).
pub type WooshAeTaps = Vec<(String, Vec<f32>)>;

impl WooshAe {
    pub fn load(weights: &Sa3Tensors) -> Result<Self> {
        let dim = WOOSH_AE_DIM;
        let c = WOOSH_LATENT_DIM;
        let get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let v = weights.f32(name)?;
            if v.len() != len {
                return Err(DiffusionError::model(format!(
                    "woosh ae tensor {name}: {} values, expected {len}",
                    v.len()
                )));
            }
            Ok(v)
        };
        // Reorder the (2048, 128, 7) embed conv to (2048, 7*128) with the
        // window-major layout the shifted-row GEMM consumes.
        let embed_raw = get("autoencoder.decoder.backbone.embed.weight", dim * c * K)?;
        let mut embed_w = vec![0f32; dim * K * c];
        for o in 0..dim {
            for ch in 0..c {
                for k in 0..K {
                    embed_w[o * K * c + k * c + ch] = embed_raw[o * c * K + ch * K + k];
                }
            }
        }
        let mut blocks = Vec::with_capacity(WOOSH_AE_LAYERS);
        for l in 0..WOOSH_AE_LAYERS {
            let p = format!("autoencoder.decoder.backbone.convnext.{l}");
            blocks.push(CnxBlock {
                dw_w: get(&format!("{p}.dwconv.weight"), dim * K)?,
                dw_b: get(&format!("{p}.dwconv.bias"), dim)?,
                ln_w: get(&format!("{p}.norm.weight"), dim)?,
                ln_b: get(&format!("{p}.norm.bias"), dim)?,
                pw1_w: get(&format!("{p}.pwconv1.weight"), WOOSH_AE_INTER * dim)?,
                pw1_b: get(&format!("{p}.pwconv1.bias"), WOOSH_AE_INTER)?,
                pw2_w: get(&format!("{p}.pwconv2.weight"), dim * WOOSH_AE_INTER)?,
                pw2_b: get(&format!("{p}.pwconv2.bias"), dim)?,
                gamma: get(&format!("{p}.gamma"), dim)?,
            });
        }
        let window = get("autoencoder.decoder.head.istft.window", WOOSH_AE_NFFT)?;

        // irfft basis (backward norm, Hermitian half-spectrum, Im of DC and
        // Nyquist ignored): y[n] = 1/N * (Re0 + (-1)^n * Re_Nyq
        //   + sum_{k=1..479} 2*(Re_k cos(2 pi k n / N) - Im_k sin(...))).
        let n_fft = WOOSH_AE_NFFT;
        let bins = WOOSH_AE_BINS;
        let mut istft_basis = vec![0f32; n_fft * 2 * bins];
        for n in 0..n_fft {
            let row = &mut istft_basis[n * 2 * bins..(n + 1) * 2 * bins];
            for k in 0..bins {
                let theta = 2.0 * std::f64::consts::PI * (k * n) as f64 / n_fft as f64;
                let weight = if k == 0 || k == bins - 1 { 1.0 } else { 2.0 };
                row[k] = (weight * theta.cos() / n_fft as f64) as f32;
                if k > 0 && k < bins - 1 {
                    row[bins + k] = (-weight * theta.sin() / n_fft as f64) as f32;
                }
            }
        }

        Ok(Self {
            z_mean: get("z_mean", c)?,
            z_std: get("z_std", c)?,
            embed_w,
            embed_b: get("autoencoder.decoder.backbone.embed.bias", dim)?,
            in_ln_w: get("autoencoder.decoder.backbone.norm.weight", dim)?,
            in_ln_b: get("autoencoder.decoder.backbone.norm.bias", dim)?,
            blocks,
            final_ln_w: get("autoencoder.decoder.backbone.final_layer_norm.weight", dim)?,
            final_ln_b: get("autoencoder.decoder.backbone.final_layer_norm.bias", dim)?,
            head_w: get("autoencoder.decoder.head.out.weight", WOOSH_AE_HEAD_OUT * dim)?,
            head_b: get("autoencoder.decoder.head.out.bias", WOOSH_AE_HEAD_OUT)?,
            window,
            istft_basis,
        })
    }

    /// Decodes channel-major latents (128 x 501) to 240,000 mono samples.
    /// `progress` ticks "ae-decode k/10" through the backbone.
    pub fn decode(
        &self,
        z: &[f32],
        progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        self.decode_with_taps(z, progress, None)
    }

    pub fn decode_with_taps(
        &self,
        z: &[f32],
        mut progress: Option<ProgressHook>,
        mut taps: Option<&mut WooshAeTaps>,
    ) -> Result<Vec<f32>> {
        let c = WOOSH_LATENT_DIM;
        let frames = WOOSH_LATENT_FRAMES;
        let dim = WOOSH_AE_DIM;
        if z.len() != c * frames {
            return Err(DiffusionError::model(format!(
                "woosh ae: {} latent values, expected {}",
                z.len(),
                c * frames
            )));
        }
        let mut tap = |name: &str, token_major: &[f32], width: usize| {
            if let Some(taps) = taps.as_deref_mut() {
                // Store channel-major to match the dumped (1, C, T) tensors.
                let mut out = vec![0f32; token_major.len()];
                for t in 0..frames {
                    for ch in 0..width {
                        out[ch * frames + t] = token_major[t * width + ch];
                    }
                }
                taps.push((name.to_string(), out));
            }
        };

        // De-normalize and go token-major (frames x 128).
        let mut z_rows = vec![0f32; frames * c];
        for ch in 0..c {
            let std = self.z_std[ch];
            let mean = self.z_mean[ch];
            for t in 0..frames {
                z_rows[t * c + ch] = z[ch * frames + t] * std + mean;
            }
        }
        tap("z_denorm", &z_rows, c);

        // Embed conv k7 p3 as a shifted-window GEMM: row t = the 7-frame
        // window (zero padded) flattened (k*128+c).
        emit_progress(&mut progress, "ae-decode 1/10", 0.0)?;
        let mut windows = vec![0f32; frames * K * c];
        for t in 0..frames {
            for k in 0..K {
                let src = t as isize + k as isize - PAD as isize;
                if src < 0 || src >= frames as isize {
                    continue;
                }
                let src = src as usize;
                windows[t * K * c + k * c..t * K * c + (k + 1) * c]
                    .copy_from_slice(&z_rows[src * c..(src + 1) * c]);
            }
        }
        let mut x = linear(&windows, &self.embed_w, Some(&self.embed_b), frames, K * c, dim);
        tap("embed", &x, dim);
        layer_norm_rows(&mut x, dim, &self.in_ln_w, &self.in_ln_b);

        for (index, block) in self.blocks.iter().enumerate() {
            emit_progress(
                &mut progress,
                &format!("ae-decode {}/10", index + 2),
                (index + 1) as f64 / 10.0,
            )?;
            // depthwise k7 conv over time, per channel.
            let mut conv = vec![0f32; frames * dim];
            par_rows(&mut conv, dim, &|t, out| {
                out.copy_from_slice(&block.dw_b);
                for k in 0..K {
                    let src = t as isize + k as isize - PAD as isize;
                    if src < 0 || src >= frames as isize {
                        continue;
                    }
                    let src_row = &x[src as usize * dim..(src as usize + 1) * dim];
                    for ch in 0..dim {
                        out[ch] += block.dw_w[ch * K + k] * src_row[ch];
                    }
                }
            });
            layer_norm_rows(&mut conv, dim, &block.ln_w, &block.ln_b);
            let mut hidden = linear(&conv, &block.pw1_w, Some(&block.pw1_b), frames, dim, WOOSH_AE_INTER);
            par_rows(&mut hidden, WOOSH_AE_INTER, &|_row, slice| {
                for v in slice.iter_mut() {
                    *v = gelu_erf(*v);
                }
            });
            let update = linear(&hidden, &block.pw2_w, Some(&block.pw2_b), frames, WOOSH_AE_INTER, dim);
            par_rows(&mut x, dim, &|t, row| {
                let u = &update[t * dim..(t + 1) * dim];
                for ch in 0..dim {
                    row[ch] += block.gamma[ch] * u[ch];
                }
            });
            if index == 0 || index == 4 {
                tap(&format!("cnx{index}"), &x, dim);
            }
        }
        layer_norm_rows(&mut x, dim, &self.final_ln_w, &self.final_ln_b);
        tap("backbone", &x, dim);

        // Head conv (k1 == linear) then circle-normalized complex spectrum.
        emit_progress(&mut progress, "ae-decode 10/10", 0.9)?;
        let head = linear(&x, &self.head_w, Some(&self.head_b), frames, dim, WOOSH_AE_HEAD_OUT);
        tap("head_conv", &head, WOOSH_AE_HEAD_OUT);
        let bins = WOOSH_AE_BINS;
        let mut spec = vec![0f32; frames * 2 * bins];
        par_rows(&mut spec, 2 * bins, &|t, row| {
            let h = &head[t * WOOSH_AE_HEAD_OUT..(t + 1) * WOOSH_AE_HEAD_OUT];
            for k in 0..bins {
                let mag = softplus(h[k]);
                let px = h[bins + k];
                let py = h[2 * bins + k];
                let p_mag = (px * px + py * py).clamp(1e-8, 1e3).sqrt();
                row[k] = mag * px / p_mag;
                row[bins + k] = mag * py / p_mag;
            }
        });

        // irfft per frame (GEMM against the basis), window, overlap-add,
        // envelope-normalize, center crop.
        let n_fft = WOOSH_AE_NFFT;
        let hop = WOOSH_AE_HOP;
        let frame_time = linear(&spec, &self.istft_basis, None, frames, 2 * bins, n_fft);
        let full = (frames - 1) * hop + n_fft;
        let mut ola = vec![0f32; full];
        let mut envelope = vec![0f32; full];
        for t in 0..frames {
            let base = t * hop;
            let frame = &frame_time[t * n_fft..(t + 1) * n_fft];
            for n in 0..n_fft {
                let w = self.window[n];
                ola[base + n] += frame[n] * w;
                envelope[base + n] += w * w;
            }
        }
        let mut audio = vec![0f32; WOOSH_AE_SAMPLES];
        let pad = n_fft / 2;
        for (i, out) in audio.iter_mut().enumerate() {
            let env = envelope[pad + i];
            if env <= 1e-11 {
                return Err(DiffusionError::model("woosh ae: istft NOLA violation"));
            }
            *out = ola[pad + i] / env;
        }
        Ok(audio)
    }
}

/// Affine LayerNorm over rows, eps 1e-6.
fn layer_norm_rows(x: &mut [f32], d: usize, weight: &[f32], bias: &[f32]) {
    par_rows(x, d, &|_row, row| {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= d as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let dv = *v - mean;
            var += dv * dv;
        }
        var /= d as f32;
        let inv = 1.0 / (var + WOOSH_LN_EPS).sqrt();
        for (i, v) in row.iter_mut().enumerate() {
            *v = (*v - mean) * inv * weight[i] + bias[i];
        }
    });
}
