//! MOSS continuous-DAC decoder (48 kHz mono), CPU f32, threaded.
//!
//! decode(z): post_quant_conv Conv1d(128->128, k=1) -> Decoder:
//!   WNConv1d(128 -> 2048, k=7, p=3)
//!   5 x DecoderBlock(dim/2^i -> dim/2^(i+1), stride in [8,5,4,3,2]):
//!     Snake1d -> WNConvTranspose1d(k=2s, stride s, p=ceil(s/2), out_pad s%2)
//!     -> ResidualUnit(d,1) -> ResidualUnit(d,3) -> ResidualUnit(d,9)
//!   Snake1d -> WNConv1d(64 -> 1, k=7, p=3) -> tanh
//! ResidualUnit: Snake -> WNConv1d(k=7, dil, p=3*dil) -> Snake -> WNConv1d(k=1),
//! residual add (the k7/p3d convs are length-preserving, so no crop).
//! snake(x) = x + (1/(alpha+1e-9)) * sin^2(alpha*x), per channel.
//! weight_norm w = g * v / ||v||, norm over all dims except dim 0
//! (Conv1d: per OUT channel; ConvTranspose1d layout (in,out,k): per IN channel).

use std::path::Path;

use crate::error::{DiffusionError, Result};
use crate::moss::{MOSS_DAC_DECODER_DIM, MOSS_DAC_RATES, MOSS_HOP, MOSS_LATENT_DIM};
use crate::sa3::par_rows;
use crate::torch_pth::PthStateDict;
use crate::{emit_progress, ProgressHook};

struct Conv {
    w: Vec<f32>, // (out, in, k) materialized
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    dilation: usize,
    padding: usize,
}

struct TConv {
    w: Vec<f32>, // (in, out, k) materialized
    b: Vec<f32>,
    in_ch: usize,
    out_ch: usize,
    k: usize,
    stride: usize,
    padding: usize,
    out_padding: usize,
}

struct ResUnit {
    alpha1: Vec<f32>,
    conv1: Conv,
    alpha2: Vec<f32>,
    conv2: Conv,
}

struct DecBlock {
    alpha: Vec<f32>,
    up: TConv,
    units: [ResUnit; 3],
}

pub struct MossDac {
    post_quant: Conv,
    conv_in: Conv,
    blocks: Vec<DecBlock>,
    alpha_out: Vec<f32>,
    conv_out: Conv,
}

struct Plane {
    ch: usize,
    len: usize,
    data: Vec<f32>,
}

fn norm_weight_conv(g: &[f32], v: &[f32], out_ch: usize, per: usize) -> Vec<f32> {
    // per = in*k elements per output row; ||v|| per out row.
    let mut w = vec![0f32; out_ch * per];
    for o in 0..out_ch {
        let row = &v[o * per..(o + 1) * per];
        let mut sum = 0f64;
        for value in row {
            sum += (*value as f64) * (*value as f64);
        }
        let scale = g[o] as f64 / sum.sqrt().max(1e-30);
        for (i, value) in row.iter().enumerate() {
            w[o * per + i] = (*value as f64 * scale) as f32;
        }
    }
    w
}

impl MossDac {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let mut sd = PthStateDict::load(path)?;
        let conv = |sd: &mut PthStateDict,
                    prefix: &str,
                    out_ch: usize,
                    in_ch: usize,
                    k: usize,
                    dilation: usize,
                    padding: usize|
         -> Result<Conv> {
            let g = sd.f32_shaped(&format!("{prefix}.weight_g"), &[out_ch, 1, 1])?;
            let v = sd.f32_shaped(&format!("{prefix}.weight_v"), &[out_ch, in_ch, k])?;
            let b = sd.f32_shaped(&format!("{prefix}.bias"), &[out_ch])?;
            Ok(Conv {
                w: norm_weight_conv(&g, &v, out_ch, in_ch * k),
                b,
                out_ch,
                in_ch,
                k,
                dilation,
                padding,
            })
        };
        let tconv = |sd: &mut PthStateDict,
                     prefix: &str,
                     in_ch: usize,
                     out_ch: usize,
                     stride: usize|
         -> Result<TConv> {
            let k = 2 * stride;
            let g = sd.f32_shaped(&format!("{prefix}.weight_g"), &[in_ch, 1, 1])?;
            let v = sd.f32_shaped(&format!("{prefix}.weight_v"), &[in_ch, out_ch, k])?;
            let b = sd.f32_shaped(&format!("{prefix}.bias"), &[out_ch])?;
            Ok(TConv {
                w: norm_weight_conv(&g, &v, in_ch, out_ch * k),
                b,
                in_ch,
                out_ch,
                k,
                stride,
                padding: stride.div_ceil(2),
                out_padding: stride % 2,
            })
        };
        let alpha = |sd: &mut PthStateDict, name: &str, ch: usize| -> Result<Vec<f32>> {
            sd.f32_shaped(name, &[1, ch, 1])
        };

        // post_quant_conv is a plain Conv1d (no weight norm).
        let pq_w = sd.f32_shaped("post_quant_conv.weight", &[MOSS_LATENT_DIM, MOSS_LATENT_DIM, 1])?;
        let pq_b = sd.f32_shaped("post_quant_conv.bias", &[MOSS_LATENT_DIM])?;
        let post_quant = Conv {
            w: pq_w,
            b: pq_b,
            out_ch: MOSS_LATENT_DIM,
            in_ch: MOSS_LATENT_DIM,
            k: 1,
            dilation: 1,
            padding: 0,
        };

        let dim = MOSS_DAC_DECODER_DIM;
        let conv_in = conv(&mut sd, "decoder.model.0", dim, MOSS_LATENT_DIM, 7, 1, 3)?;
        let mut blocks = Vec::with_capacity(MOSS_DAC_RATES.len());
        for (i, &stride) in MOSS_DAC_RATES.iter().enumerate() {
            let in_ch = dim >> i;
            let out_ch = dim >> (i + 1);
            let p = format!("decoder.model.{}", i + 1);
            let unit = |sd: &mut PthStateDict, u: usize, dilation: usize| -> Result<ResUnit> {
                let up = format!("{p}.block.{}", u + 2);
                Ok(ResUnit {
                    alpha1: alpha(sd, &format!("{up}.block.0.alpha"), out_ch)?,
                    conv1: conv(sd, &format!("{up}.block.1"), out_ch, out_ch, 7, dilation, 3 * dilation)?,
                    alpha2: alpha(sd, &format!("{up}.block.2.alpha"), out_ch)?,
                    conv2: conv(sd, &format!("{up}.block.3"), out_ch, out_ch, 1, 1, 0)?,
                })
            };
            blocks.push(DecBlock {
                alpha: alpha(&mut sd, &format!("{p}.block.0.alpha"), in_ch)?,
                up: tconv(&mut sd, &format!("{p}.block.1"), in_ch, out_ch, stride)?,
                units: [
                    unit(&mut sd, 0, 1)?,
                    unit(&mut sd, 1, 3)?,
                    unit(&mut sd, 2, 9)?,
                ],
            });
        }
        let last = dim >> MOSS_DAC_RATES.len();
        let idx = MOSS_DAC_RATES.len() + 1;
        Ok(Self {
            post_quant,
            conv_in,
            alpha_out: alpha(&mut sd, &format!("decoder.model.{idx}.alpha"), last)?,
            conv_out: conv(&mut sd, &format!("decoder.model.{}", idx + 1), 1, last, 7, 1, 3)?,
            blocks,
        })
    }

    /// Decodes `frames` latent frames (channel-major 128 x frames) to mono
    /// samples (`frames * MOSS_HOP`).
    pub fn decode(&self, latents: &[f32], frames: usize) -> Result<Vec<f32>> {
        self.decode_with_progress(latents, frames, None)
    }

    /// [`Self::decode`] ticking "dac-decode k/5" per upsample block.
    pub fn decode_with_progress(
        &self,
        latents: &[f32],
        frames: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        if latents.len() != MOSS_LATENT_DIM * frames {
            return Err(DiffusionError::model(format!(
                "moss dac: {} latent values, expected {}",
                latents.len(),
                MOSS_LATENT_DIM * frames
            )));
        }
        let mut plane = Plane {
            ch: MOSS_LATENT_DIM,
            len: frames,
            data: latents.to_vec(),
        };
        plane = conv1d(&plane, &self.post_quant);
        plane = conv1d(&plane, &self.conv_in);
        let block_count = self.blocks.len();
        for (block_index, block) in self.blocks.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("dac-decode {}/{block_count}", block_index + 1),
                    block_index as f64 / block_count as f64,
                )?;
            }
            snake(&mut plane, &block.alpha);
            plane = conv_transpose1d(&plane, &block.up);
            for unit in &block.units {
                let mut y = plane_clone(&plane);
                snake(&mut y, &unit.alpha1);
                let mut y = conv1d(&y, &unit.conv1);
                snake(&mut y, &unit.alpha2);
                let y = conv1d(&y, &unit.conv2);
                if y.len != plane.len {
                    return Err(DiffusionError::model(format!(
                        "moss dac residual length {} vs {}",
                        y.len, plane.len
                    )));
                }
                for (x, yv) in plane.data.iter_mut().zip(y.data.iter()) {
                    *x += *yv;
                }
            }
        }
        snake(&mut plane, &self.alpha_out);
        plane = conv1d(&plane, &self.conv_out);
        debug_assert_eq!(plane.ch, 1);
        let mut out = plane.data;
        for v in out.iter_mut() {
            *v = v.tanh();
        }
        if out.len() != frames * MOSS_HOP {
            return Err(DiffusionError::model(format!(
                "moss dac produced {} samples, expected {}",
                out.len(),
                frames * MOSS_HOP
            )));
        }
        Ok(out)
    }
}

fn plane_clone(p: &Plane) -> Plane {
    Plane {
        ch: p.ch,
        len: p.len,
        data: p.data.clone(),
    }
}

fn snake(p: &mut Plane, alpha: &[f32]) {
    debug_assert_eq!(alpha.len(), p.ch);
    let len = p.len;
    par_rows(&mut p.data, len, &|ch, row| {
        let a = alpha[ch];
        let inv = 1.0 / (a + 1e-9);
        for v in row.iter_mut() {
            let s = (a * *v).sin();
            *v += inv * s * s;
        }
    });
}

fn conv1d(input: &Plane, conv: &Conv) -> Plane {
    debug_assert_eq!(input.ch, conv.in_ch);
    let len = input.len;
    let span = (conv.k - 1) * conv.dilation;
    let out_len = len + 2 * conv.padding - span;
    let mut out = vec![0f32; conv.out_ch * out_len];
    // axpy formulation: per (in_ch, tap), add a shifted contiguous slice of
    // the input row into the output row — sequential loads/stores vectorize.
    par_rows(&mut out, out_len, &|o, out_row| {
        out_row.fill(conv.b[o]);
        let w_base = &conv.w[o * conv.in_ch * conv.k..(o + 1) * conv.in_ch * conv.k];
        for c in 0..conv.in_ch {
            let in_row = &input.data[c * len..(c + 1) * len];
            for tap in 0..conv.k {
                let w = w_base[c * conv.k + tap];
                if w == 0.0 {
                    continue;
                }
                // out[t] += w * in[t - padding + tap*dilation]
                let off = tap as isize * conv.dilation as isize - conv.padding as isize;
                let t_start = (-off).max(0) as usize;
                let t_end = ((len as isize - off).min(out_len as isize)).max(0) as usize;
                if t_start >= t_end {
                    continue;
                }
                let src = &in_row[(t_start as isize + off) as usize
                    ..(t_start as isize + off) as usize + (t_end - t_start)];
                let dst = &mut out_row[t_start..t_end];
                for (d, s) in dst.iter_mut().zip(src.iter()) {
                    *d += w * *s;
                }
            }
        }
    });
    Plane {
        ch: conv.out_ch,
        len: out_len,
        data: out,
    }
}

fn conv_transpose1d(input: &Plane, conv: &TConv) -> Plane {
    debug_assert_eq!(input.ch, conv.in_ch);
    let len = input.len;
    let out_len = (len - 1) * conv.stride + conv.k + conv.out_padding - 2 * conv.padding;
    let mut out = vec![0f32; conv.out_ch * out_len];
    // Scatter formulation per output channel: for each (in_ch, tap), the
    // contributions land at t = src*stride + tap - padding — an arithmetic
    // walk with contiguous input reads.
    par_rows(&mut out, out_len, &|o, out_row| {
        out_row.fill(conv.b[o]);
        for c in 0..conv.in_ch {
            let in_row = &input.data[c * len..(c + 1) * len];
            let w_row = &conv.w[(c * conv.out_ch + o) * conv.k..(c * conv.out_ch + o + 1) * conv.k];
            for (tap, &w) in w_row.iter().enumerate() {
                if w == 0.0 {
                    continue;
                }
                let base = tap as isize - conv.padding as isize;
                // valid src: 0 <= src < len and 0 <= src*stride + base < out_len
                let src_start = if base < 0 {
                    ((-base) as usize).div_ceil(conv.stride)
                } else {
                    0
                };
                let src_end = if base >= out_len as isize {
                    0
                } else {
                    (((out_len as isize - base - 1) / conv.stride as isize) as usize + 1).min(len)
                };
                let mut t = (src_start as isize * conv.stride as isize + base) as usize;
                for src in src_start..src_end {
                    out_row[t] += w * in_row[src];
                    t += conv.stride;
                }
            }
        }
    });
    Plane {
        ch: conv.out_ch,
        len: out_len,
        data: out,
    }
}

// ---------------------------------------------------------------------------
// CUDA device path. Planes are TIME-major on device (rows = samples,
// cols = channels):
// - Conv1d = k shifted gemms: pad rows with zeros, out += slice(x, tap*dil)
//   @ W_tap^T (W_tap = (out_ch x in_ch) tap matrix; bias folded into tap 0).
// - ConvTranspose1d(k=2s, stride s) = TWO stacked gemms (all s low taps /
//   all s high taps as (s*out_ch x in_ch)) + gather_rows_colblock
//   re-interleave: out[t] = y_hi[(t+p)/s][block (t+p)%s]
//                          + y_lo[(t+p)/s - 1][same block].
// - snake via the dedicated gpu_snake_cols kernel (alpha per column).
// - final tanh on the host (single mono row).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_concat_rows, gpu_download, gpu_gather_rows_colblock, gpu_linear_nt_cached,
    gpu_slice_rows, gpu_snake_cols, gpu_upload, gpu_upload_u32, GpuTensor,
};

struct DevConv {
    /// One (out_ch x in_ch) f16 weight per tap.
    taps: Vec<F16Weight>,
    bias: Vec<f32>,
    dilation: usize,
    padding: usize,
    in_ch: usize,
    out_ch: usize,
}

struct DevTConv {
    /// Stacked low taps (tap = r) and high taps (tap = r + s), each
    /// (s*out_ch x in_ch).
    lo: F16Weight,
    hi: F16Weight,
    /// Bias tiled s times (s*out_ch) for the lo gemm.
    bias_tiled: Vec<f32>,
    stride: usize,
    padding: usize,
    out_padding: usize,
    out_ch: usize,
}

struct DevResUnit {
    alpha1: Vec<f32>,
    conv1: DevConv,
    alpha2: Vec<f32>,
    conv2: DevConv,
}

struct DevBlock {
    alpha: Vec<f32>,
    up: DevTConv,
    units: [DevResUnit; 3],
}

pub struct MossDacDevice {
    post_quant: F16Weight,
    post_quant_b: Vec<f32>,
    conv_in: DevConv,
    blocks: Vec<DevBlock>,
    alpha_out: Vec<f32>,
    conv_out: DevConv,
}

fn dev_conv(conv: &Conv, key: &str) -> DevConv {
    let (out_ch, in_ch, k) = (conv.out_ch, conv.in_ch, conv.k);
    let mut taps = Vec::with_capacity(k);
    for tap in 0..k {
        let mut w = vec![0f32; out_ch * in_ch];
        for o in 0..out_ch {
            for c in 0..in_ch {
                w[o * in_ch + c] = conv.w[(o * in_ch + c) * k + tap];
            }
        }
        taps.push(F16Weight::new(format!("{key}.t{tap}"), &w, out_ch, in_ch));
    }
    DevConv {
        taps,
        bias: conv.b.clone(),
        dilation: conv.dilation,
        padding: conv.padding,
        in_ch,
        out_ch,
    }
}

fn dev_tconv(conv: &TConv, key: &str) -> DevTConv {
    let (in_ch, out_ch, k, s) = (conv.in_ch, conv.out_ch, conv.k, conv.stride);
    debug_assert_eq!(k, 2 * s);
    // weight layout (in, out, k); stacked (s*out_ch x in_ch)
    let mut lo = vec![0f32; s * out_ch * in_ch];
    let mut hi = vec![0f32; s * out_ch * in_ch];
    for r in 0..s {
        for o in 0..out_ch {
            for c in 0..in_ch {
                lo[(r * out_ch + o) * in_ch + c] = conv.w[(c * out_ch + o) * k + r];
                hi[(r * out_ch + o) * in_ch + c] = conv.w[(c * out_ch + o) * k + r + s];
            }
        }
    }
    let mut bias_tiled = Vec::with_capacity(s * out_ch);
    for _ in 0..s {
        bias_tiled.extend_from_slice(&conv.b);
    }
    DevTConv {
        lo: F16Weight::new(format!("{key}.lo"), &lo, s * out_ch, in_ch),
        hi: F16Weight::new(format!("{key}.hi"), &hi, s * out_ch, in_ch),
        bias_tiled,
        stride: s,
        padding: conv.padding,
        out_padding: conv.out_padding,
        out_ch,
    }
}

fn conv1d_dev(x: &GpuTensor, len: usize, conv: &DevConv) -> Result<GpuTensor> {
    let span = (conv.taps.len() - 1) * conv.dilation;
    let out_len = len + 2 * conv.padding - span;
    // zero-pad rows
    let padded = if conv.padding > 0 {
        let zeros = gpu_upload(&vec![0f32; conv.padding * conv.in_ch], conv.padding, conv.in_ch)
            .map_err(|e| dev_err("moss dac pad", e))?;
        let top = gpu_concat_rows(&zeros, x).map_err(|e| dev_err("moss dac pad top", e))?;
        gpu_concat_rows(&top, &zeros).map_err(|e| dev_err("moss dac pad bottom", e))?
    } else {
        gpu_slice_rows(x, 0, len).map_err(|e| dev_err("moss dac x copy", e))?
    };
    let mut out: Option<GpuTensor> = None;
    for (tap, weight) in conv.taps.iter().enumerate() {
        let shifted = gpu_slice_rows(&padded, tap * conv.dilation, out_len)
            .map_err(|e| dev_err("moss dac conv slice", e))?;
        let bias: &[f32] = if tap == 0 { &conv.bias } else { &[] };
        let y = gpu_linear_nt_cached(&shifted, "mossdac", &[weight.part()], bias)
            .map_err(|e| dev_err("moss dac conv gemm", e))?;
        out = Some(match out {
            None => y,
            Some(acc) => gpu_add(&acc, &y).map_err(|e| dev_err("moss dac conv add", e))?,
        });
    }
    Ok(out.expect("conv has taps"))
}

fn tconv_dev(x: &GpuTensor, len: usize, conv: &DevTConv) -> Result<GpuTensor> {
    let s = conv.stride;
    let k = 2 * s;
    let out_len = (len - 1) * s + k + conv.out_padding - 2 * conv.padding;
    let y_hi_src = gpu_linear_nt_cached(x, "mossdac", &[conv.lo.part()], &conv.bias_tiled)
        .map_err(|e| dev_err("moss dac tconv lo", e))?;
    let y_lo_src = gpu_linear_nt_cached(x, "mossdac", &[conv.hi.part()], &[])
        .map_err(|e| dev_err("moss dac tconv hi", e))?;
    // out[t] = y_hi_src[(t+p)/s][block (t+p)%s] + y_lo_src[(t+p)/s - 1][block]
    let mut row_hi = vec![0u32; out_len];
    let mut row_lo = vec![0u32; out_len];
    let mut blocks = vec![0u32; out_len];
    for t in 0..out_len {
        let tp = t + conv.padding;
        let src = (tp / s) as isize;
        let r = (tp % s) as u32;
        row_hi[t] = if src < len as isize { src as u32 } else { u32::MAX };
        row_lo[t] = if src - 1 >= 0 && src - 1 < len as isize {
            (src - 1) as u32
        } else {
            u32::MAX
        };
        blocks[t] = r;
    }
    let row_hi = gpu_upload_u32(&row_hi).map_err(|e| dev_err("moss dac idx hi", e))?;
    let row_lo = gpu_upload_u32(&row_lo).map_err(|e| dev_err("moss dac idx lo", e))?;
    let blocks = gpu_upload_u32(&blocks).map_err(|e| dev_err("moss dac idx block", e))?;
    let hi = gpu_gather_rows_colblock(&y_hi_src, &row_hi, Some(&blocks), conv.out_ch)
        .map_err(|e| dev_err("moss dac gather hi", e))?;
    let lo = gpu_gather_rows_colblock(&y_lo_src, &row_lo, Some(&blocks), conv.out_ch)
        .map_err(|e| dev_err("moss dac gather lo", e))?;
    gpu_add(&hi, &lo).map_err(|e| dev_err("moss dac tconv add", e))
}

fn snake_dev(x: &GpuTensor, alpha: &[f32]) -> Result<GpuTensor> {
    let alpha_dev =
        gpu_upload(alpha, 1, alpha.len()).map_err(|e| dev_err("moss dac alpha", e))?;
    gpu_snake_cols(x, &alpha_dev).map_err(|e| dev_err("moss dac snake", e))
}

impl MossDac {
    pub fn prepare_device(&self) -> MossDacDevice {
        MossDacDevice {
            post_quant: F16Weight::new(
                "mossdac.pq",
                &self.post_quant.w,
                self.post_quant.out_ch,
                self.post_quant.in_ch,
            ),
            post_quant_b: self.post_quant.b.clone(),
            conv_in: dev_conv(&self.conv_in, "mossdac.in"),
            blocks: self
                .blocks
                .iter()
                .enumerate()
                .map(|(i, block)| DevBlock {
                    alpha: block.alpha.clone(),
                    up: dev_tconv(&block.up, &format!("mossdac.b{i}.up")),
                    units: [0, 1, 2].map(|u| {
                        let unit = &block.units[u];
                        DevResUnit {
                            alpha1: unit.alpha1.clone(),
                            conv1: dev_conv(&unit.conv1, &format!("mossdac.b{i}.u{u}.c1")),
                            alpha2: unit.alpha2.clone(),
                            conv2: dev_conv(&unit.conv2, &format!("mossdac.b{i}.u{u}.c2")),
                        }
                    }),
                })
                .collect(),
            alpha_out: self.alpha_out.clone(),
            conv_out: dev_conv(&self.conv_out, "mossdac.out"),
        }
    }

    /// Device decode; same contract as [`MossDac::decode`] (latents
    /// channel-major 128 x frames in, mono samples out).
    pub fn decode_device(&self, device: &MossDacDevice, latents: &[f32], frames: usize) -> Result<Vec<f32>> {
        self.decode_device_with_progress(device, latents, frames, None)
    }

    /// [`Self::decode_device`] ticking "dac-decode k/5" per upsample block.
    pub fn decode_device_with_progress(
        &self,
        device: &MossDacDevice,
        latents: &[f32],
        frames: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        if latents.len() != MOSS_LATENT_DIM * frames {
            return Err(DiffusionError::model(format!(
                "moss dac device: {} latent values, expected {}",
                latents.len(),
                MOSS_LATENT_DIM * frames
            )));
        }
        // channel-major -> time-major host transpose (small)
        let c = MOSS_LATENT_DIM;
        let mut x_host = vec![0f32; frames * c];
        for ch in 0..c {
            for f in 0..frames {
                x_host[f * c + ch] = latents[ch * frames + f];
            }
        }
        let x = gpu_upload(&x_host, frames, c).map_err(|e| dev_err("moss dac upload", e))?;
        let x = gpu_linear_nt_cached(&x, "mossdac", &[device.post_quant.part()], &device.post_quant_b)
            .map_err(|e| dev_err("moss dac post quant", e))?;
        let mut plane = conv1d_dev(&x, frames, &device.conv_in)?;
        let mut len = frames;
        let block_count = device.blocks.len();
        for (block_index, block) in device.blocks.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("dac-decode {}/{block_count}", block_index + 1),
                    block_index as f64 / block_count as f64,
                )?;
            }
            plane = snake_dev(&plane, &block.alpha)?;
            plane = tconv_dev(&plane, len, &block.up)?;
            len *= block.up.stride;
            for unit in &block.units {
                let y = snake_dev(&plane, &unit.alpha1)?;
                let y = conv1d_dev(&y, len, &unit.conv1)?;
                let y = snake_dev(&y, &unit.alpha2)?;
                let y = conv1d_dev(&y, len, &unit.conv2)?;
                plane = gpu_add(&plane, &y).map_err(|e| dev_err("moss dac residual", e))?;
            }
        }
        plane = snake_dev(&plane, &device.alpha_out)?;
        plane = conv1d_dev(&plane, len, &device.conv_out)?;
        let mut out = gpu_download(&plane).map_err(|e| dev_err("moss dac download", e))?;
        for v in out.iter_mut() {
            *v = v.tanh();
        }
        if out.len() != frames * MOSS_HOP {
            return Err(DiffusionError::model(format!(
                "moss dac device produced {} samples, expected {}",
                out.len(),
                frames * MOSS_HOP
            )));
        }
        Ok(out)
    }
}
