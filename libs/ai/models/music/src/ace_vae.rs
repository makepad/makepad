//! ACE-Step 1.5 AutoencoderOobleck decoder (48 kHz stereo).
//!
//! Official geometry: `decoder_input_channels=64`, `decoder_channels=128`,
//! `channel_multiples=[1,2,4,8,16]`, upsampling ratios `[10,6,4,4,2]` (the
//! reverse of the encoder). Snake1d is log-scale (`alpha/beta` stored as
//! logs). Weight-norm convs are folded at load (`w = g * v / ||v||`).

use crate::ace::{
    ace_open_shards, ace_tensor_any, ACE_AUDIO_CHANNELS, ACE_HOP, ACE_LATENT_DIM,
    ACE_VAE_CHANNELS, ACE_VAE_MULTS, ACE_VAE_STRIDES,
};
use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_sfx::sa3::par_rows;
use crate::{emit_progress, ProgressHook};
use std::path::Path;

struct Conv {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    dilation: usize,
    padding: usize,
}

struct TConv {
    w: Vec<f32>, // (in, out, k)
    b: Vec<f32>,
    in_ch: usize,
    out_ch: usize,
    k: usize,
    stride: usize,
    padding: usize,
}

struct Snake {
    alpha: Vec<f32>,
    inv_beta: Vec<f32>,
}

struct ResUnit {
    snake1: Snake,
    conv1: Conv,
    snake2: Snake,
    conv2: Conv,
}

struct DecBlock {
    snake: Snake,
    up: TConv,
    units: [ResUnit; 3],
}

pub struct AceVaeDecoder {
    conv_in: Conv,
    blocks: Vec<DecBlock>,
    snake_out: Snake,
    conv_out: Conv,
}

struct Plane {
    ch: usize,
    len: usize,
    data: Vec<f32>,
}

fn fold_weight_norm(g: &[f32], v: &[f32], rows: usize, per: usize) -> Vec<f32> {
    let mut w = vec![0f32; rows * per];
    for o in 0..rows {
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

fn wn_names(prefix: &str, which: &str) -> Vec<String> {
    vec![
        format!("{prefix}.{which}"),
        format!("{prefix}.parametrizations.weight.original0"),
        format!("{prefix}.weight_g"),
        format!("{prefix}.parametrizations.weight.original1"),
        format!("{prefix}.weight_v"),
        format!("{prefix}.bias"),
    ]
}

fn load_conv(
    weights: &H3ShardedWeights,
    prefix: &str,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    dilation: usize,
    padding: usize,
    with_bias: bool,
) -> Result<Conv> {
    let g = ace_tensor_any(
        weights,
        &[
            &format!("{prefix}.weight_g"),
            &format!("{prefix}.parametrizations.weight.original0"),
        ],
    )?;
    let v = ace_tensor_any(
        weights,
        &[
            &format!("{prefix}.weight_v"),
            &format!("{prefix}.parametrizations.weight.original1"),
        ],
    )?;
    if g.len() != out_ch || v.len() != out_ch * in_ch * k {
        return Err(DiffusionError::model(format!(
            "ace vae {prefix}: g {} v {} expected g {out_ch} v {}",
            g.len(),
            v.len(),
            out_ch * in_ch * k
        )));
    }
    let b = if with_bias {
        ace_tensor_any(weights, &[&format!("{prefix}.bias")])?
    } else {
        vec![0f32; out_ch]
    };
    if b.len() != out_ch {
        return Err(DiffusionError::model(format!(
            "ace vae {prefix}.bias {} expected {out_ch}",
            b.len()
        )));
    }
    let _ = wn_names(prefix, "weight_g");
    Ok(Conv {
        w: fold_weight_norm(&g, &v, out_ch, in_ch * k),
        b,
        out_ch,
        in_ch,
        k,
        dilation,
        padding,
    })
}

fn load_tconv(
    weights: &H3ShardedWeights,
    prefix: &str,
    in_ch: usize,
    out_ch: usize,
    stride: usize,
) -> Result<TConv> {
    let k = 2 * stride;
    let g = ace_tensor_any(
        weights,
        &[
            &format!("{prefix}.weight_g"),
            &format!("{prefix}.parametrizations.weight.original0"),
        ],
    )?;
    let v = ace_tensor_any(
        weights,
        &[
            &format!("{prefix}.weight_v"),
            &format!("{prefix}.parametrizations.weight.original1"),
        ],
    )?;
    if g.len() != in_ch || v.len() != in_ch * out_ch * k {
        return Err(DiffusionError::model(format!(
            "ace vae tconv {prefix}: g {} v {} expected g {in_ch} v {}",
            g.len(),
            v.len(),
            in_ch * out_ch * k
        )));
    }
    let b = ace_tensor_any(weights, &[&format!("{prefix}.bias")])?;
    Ok(TConv {
        w: fold_weight_norm(&g, &v, in_ch, out_ch * k),
        b,
        in_ch,
        out_ch,
        k,
        stride,
        padding: stride.div_ceil(2),
    })
}

fn load_snake(weights: &H3ShardedWeights, prefix: &str, ch: usize) -> Result<Snake> {
    let alpha_log = ace_tensor_any(
        weights,
        &[&format!("{prefix}.alpha"), &format!("{prefix}.alpha")],
    )?;
    let beta_log = ace_tensor_any(weights, &[&format!("{prefix}.beta")])?;
    if alpha_log.len() != ch || beta_log.len() != ch {
        return Err(DiffusionError::model(format!(
            "ace vae snake {prefix}: alpha {} beta {} expected {ch}",
            alpha_log.len(),
            beta_log.len()
        )));
    }
    let alpha: Vec<f32> = alpha_log.iter().map(|v| v.exp()).collect();
    let inv_beta: Vec<f32> = beta_log
        .iter()
        .map(|v| 1.0 / (v.exp() + 1e-9))
        .collect();
    Ok(Snake { alpha, inv_beta })
}

fn load_res_unit(weights: &H3ShardedWeights, prefix: &str, ch: usize, dilation: usize) -> Result<ResUnit> {
    Ok(ResUnit {
        snake1: load_snake(weights, &format!("{prefix}.snake1"), ch)?,
        conv1: load_conv(weights, &format!("{prefix}.conv1"), ch, ch, 7, dilation, (7 - 1) * dilation / 2, true)?,
        snake2: load_snake(weights, &format!("{prefix}.snake2"), ch)?,
        conv2: load_conv(weights, &format!("{prefix}.conv2"), ch, ch, 1, 1, 0, true)?,
    })
}

impl AceVaeDecoder {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_progress(dir, None)
    }

    pub fn load_with_progress(
        dir: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = ace_open_shards(dir)?;
        emit_progress(&mut progress, "load ace vae", 0.0)?;
        let strides = ACE_VAE_STRIDES;
        let ch = ACE_VAE_CHANNELS;
        let conv_in = load_conv(
            &weights,
            "decoder.conv1",
            ch * ACE_VAE_MULTS[strides.len()],
            ACE_LATENT_DIM,
            7,
            1,
            3,
            true,
        )?;
        let mut blocks = Vec::with_capacity(strides.len());
        for (i, &stride) in strides.iter().enumerate() {
            emit_progress(
                &mut progress,
                &format!("load ace vae {i}/{}", strides.len()),
                (i as f64 + 0.2) / (strides.len() as f64 + 0.4),
            )?;
            let in_ch = ch * ACE_VAE_MULTS[strides.len() - i];
            let out_ch = ch * ACE_VAE_MULTS[strides.len() - i - 1];
            let p = format!("decoder.block.{i}");
            blocks.push(DecBlock {
                snake: load_snake(&weights, &format!("{p}.snake1"), in_ch)?,
                up: load_tconv(&weights, &format!("{p}.conv_t1"), in_ch, out_ch, stride)?,
                units: [
                    load_res_unit(&weights, &format!("{p}.res_unit1"), out_ch, 1)?,
                    load_res_unit(&weights, &format!("{p}.res_unit2"), out_ch, 3)?,
                    load_res_unit(&weights, &format!("{p}.res_unit3"), out_ch, 9)?,
                ],
            });
        }
        let snake_out = load_snake(&weights, "decoder.snake1", ch)?;
        let conv_out = load_conv(
            &weights,
            "decoder.conv2",
            ACE_AUDIO_CHANNELS,
            ch,
            7,
            1,
            3,
            false,
        )?;
        emit_progress(&mut progress, "load ace vae", 1.0)?;
        Ok(Self {
            conv_in,
            blocks,
            snake_out,
            conv_out,
        })
    }

    /// Decode token-major `[T, 64]` latents to planar stereo `[left, right]`.
    pub fn decode(&self, latents: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        self.decode_with_progress(latents, frames, None)
    }

    pub fn decode_with_progress(
        &self,
        latents: &[f32],
        frames: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        if latents.len() != frames * ACE_LATENT_DIM {
            return Err(DiffusionError::model(format!(
                "ace vae: {} latent values, expected {}",
                latents.len(),
                frames * ACE_LATENT_DIM
            )));
        }
        // Channel-major plane for the conv path.
        let mut plane = Plane {
            ch: ACE_LATENT_DIM,
            len: frames,
            data: vec![0f32; ACE_LATENT_DIM * frames],
        };
        for t in 0..frames {
            for c in 0..ACE_LATENT_DIM {
                plane.data[c * frames + t] = latents[t * ACE_LATENT_DIM + c];
            }
        }
        plane = conv1d(&plane, &self.conv_in);
        let n = self.blocks.len();
        for (i, block) in self.blocks.iter().enumerate() {
            emit_progress(
                &mut progress,
                &format!("vae-decode {}/{n}", i + 1),
                i as f64 / n as f64,
            )?;
            snake(&mut plane, &block.snake);
            plane = conv_transpose1d(&plane, &block.up);
            for unit in &block.units {
                let mut y = plane_clone(&plane);
                snake(&mut y, &unit.snake1);
                let mut y = conv1d(&y, &unit.conv1);
                snake(&mut y, &unit.snake2);
                let y = conv1d(&y, &unit.conv2);
                if y.len != plane.len {
                    let pad = (plane.len - y.len) / 2;
                    if pad > 0 && y.len + 2 * pad == plane.len {
                        for c in 0..plane.ch {
                            let src = &y.data[c * y.len..(c + 1) * y.len];
                            let dst = &mut plane.data[c * plane.len + pad..c * plane.len + pad + y.len];
                            for (d, s) in dst.iter_mut().zip(src.iter()) {
                                *d += *s;
                            }
                        }
                        continue;
                    }
                    return Err(DiffusionError::model(format!(
                        "ace vae residual length {} vs {}",
                        y.len, plane.len
                    )));
                }
                for (x, yv) in plane.data.iter_mut().zip(y.data.iter()) {
                    *x += *yv;
                }
            }
        }
        snake(&mut plane, &self.snake_out);
        plane = conv1d(&plane, &self.conv_out);
        if plane.ch != ACE_AUDIO_CHANNELS {
            return Err(DiffusionError::model(format!(
                "ace vae produced {} channels",
                plane.ch
            )));
        }
        let expected = frames * ACE_HOP;
        if plane.len < expected.saturating_sub(ACE_HOP) {
            return Err(DiffusionError::model(format!(
                "ace vae produced {} samples, expected ~{expected}",
                plane.len
            )));
        }
        // Channel-major: [left_row | right_row].
        let take = plane.len.min(expected);
        let left = plane.data[..take].to_vec();
        let right = if plane.data.len() >= plane.len + take {
            plane.data[plane.len..plane.len + take].to_vec()
        } else {
            left.clone()
        };
        Ok((left, right))
    }

    pub fn decode_device(&self, latents: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        use makepad_ai_sfx::sa3::dev_err;
        
        use makepad_ai_common::backend::cuda::{
            gpu_add, gpu_download, gpu_upload,
            GpuTensor,
        };

        if latents.len() != frames * ACE_LATENT_DIM {
            return Err(DiffusionError::model("ace vae device: shape"));
        }
        // Time-major [T, C] for linear-as-conv.
        let mut x = gpu_upload(latents, frames, ACE_LATENT_DIM).map_err(|e| dev_err("ace vae up", e))?;
        let mut len = frames;

        fn conv_tm(
            x: &GpuTensor,
            len: usize,
            w: &[f32],
            b: &[f32],
            out_ch: usize,
            in_ch: usize,
            k: usize,
            dilation: usize,
            padding: usize,
            key: &str,
        ) -> Result<(GpuTensor, usize)> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_sfx::sa3::F16Weight;
            use makepad_ai_common::backend::cuda::{
                gpu_add, gpu_concat_rows, gpu_linear_nt_cached, gpu_slice_rows, gpu_upload,
            };
            let span = (k - 1) * dilation;
            let out_len = len + 2 * padding - span;
            let padded = if padding > 0 {
                let zeros = gpu_upload(&vec![0f32; padding * in_ch], padding, in_ch)
                    .map_err(|e| dev_err("ace vae pad", e))?;
                let top = gpu_concat_rows(&zeros, x).map_err(|e| dev_err("ace vae cat0", e))?;
                gpu_concat_rows(&top, &zeros).map_err(|e| dev_err("ace vae cat1", e))?
            } else {
                gpu_slice_rows(x, 0, len).map_err(|e| dev_err("ace vae sl", e))?
            };
            let mut acc: Option<GpuTensor> = None;
            for tap in 0..k {
                let mut wt = vec![0f32; out_ch * in_ch];
                for o in 0..out_ch {
                    for c in 0..in_ch {
                        wt[o * in_ch + c] = w[(o * in_ch + c) * k + tap];
                    }
                }
                let part = F16Weight::new(format!("{key}.t{tap}"), &wt, out_ch, in_ch);
                let shifted = gpu_slice_rows(&padded, tap * dilation, out_len)
                    .map_err(|e| dev_err("ace vae shift", e))?;
                let bias: &[f32] = if tap == 0 { b } else { &[] };
                let y = gpu_linear_nt_cached(&shifted, "acevae", &[part.part()], bias)
                    .map_err(|e| dev_err("ace vae gemm", e))?;
                acc = Some(match acc {
                    None => y,
                    Some(prev) => gpu_add(&prev, &y).map_err(|e| dev_err("ace vae add", e))?,
                });
            }
            Ok((acc.expect("taps"), out_len))
        }

        fn snake_tm(x: &GpuTensor, alpha: &[f32], inv_beta: &[f32]) -> Result<GpuTensor> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_common::backend::cuda::gpu_snake;
            gpu_snake(x, alpha, inv_beta).map_err(|e| dev_err("ace vae snake", e))
        }

        fn tconv_tm(
            x: &GpuTensor,
            len: usize,
            w: &[f32],
            b: &[f32],
            in_ch: usize,
            out_ch: usize,
            k: usize,
            stride: usize,
            padding: usize,
            key: &str,
        ) -> Result<(GpuTensor, usize)> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_sfx::sa3::F16Weight;
            use makepad_ai_common::backend::cuda::gpu_linear_nt_cached;
            // Fallback: download, CPU tconv, upload. Last stages are huge;
            // the GEMM upsample below covers stride convs when k==2*stride.
            if k != 2 * stride {
                return Err(DiffusionError::model("ace vae tconv k"));
            }
            let mut lo = vec![0f32; stride * out_ch * in_ch];
            let mut hi = vec![0f32; stride * out_ch * in_ch];
            for r in 0..stride {
                for o in 0..out_ch {
                    for c in 0..in_ch {
                        lo[(r * out_ch + o) * in_ch + c] = w[(c * out_ch + o) * k + r];
                        hi[(r * out_ch + o) * in_ch + c] = w[(c * out_ch + o) * k + r + stride];
                    }
                }
            }
            let mut bias_tiled = Vec::with_capacity(stride * out_ch);
            for _ in 0..stride {
                bias_tiled.extend_from_slice(b);
            }
            let lo_w = F16Weight::new(format!("{key}.lo"), &lo, stride * out_ch, in_ch);
            let hi_w = F16Weight::new(format!("{key}.hi"), &hi, stride * out_ch, in_ch);
            let y_hi = gpu_linear_nt_cached(x, "acevae", &[lo_w.part()], &bias_tiled)
                .map_err(|e| dev_err("ace vae tlo", e))?;
            let y_lo = gpu_linear_nt_cached(x, "acevae", &[hi_w.part()], &[])
                .map_err(|e| dev_err("ace vae thi", e))?;
            let g = makepad_ai_common::backend::cuda::gpu_tconv_stitch(
                &y_hi, &y_lo, len, out_ch, stride, padding, k,
            )
            .map_err(|e| dev_err("ace vae stitch", e))?;
            let out_len = g.rows();
            Ok((g, out_len))
        }

        let conv = &self.conv_in;
        let (y, nlen) = conv_tm(
            &x, len, &conv.w, &conv.b, conv.out_ch, conv.in_ch, conv.k, conv.dilation, conv.padding, "acevae.in",
        )?;
        x = y;
        len = nlen;

        for (bi, block) in self.blocks.iter().enumerate() {
            x = snake_tm(&x, &block.snake.alpha, &block.snake.inv_beta)?;
            let up = &block.up;
            let (yy, nn) = tconv_tm(
                &x, len, &up.w, &up.b, up.in_ch, up.out_ch, up.k, up.stride, up.padding,
                &format!("acevae.up{bi}"),
            )?;
            x = yy;
            len = nn;
            for (ui, unit) in block.units.iter().enumerate() {
                let skip = x;
                let mut h = snake_tm(&skip, &unit.snake1.alpha, &unit.snake1.inv_beta)?;
                let c1 = &unit.conv1;
                let (hh, hlen) = conv_tm(
                    &h, len, &c1.w, &c1.b, c1.out_ch, c1.in_ch, c1.k, c1.dilation, c1.padding,
                    &format!("acevae.b{bi}u{ui}c1"),
                )?;
                h = snake_tm(&hh, &unit.snake2.alpha, &unit.snake2.inv_beta)?;
                let c2 = &unit.conv2;
                let (hh2, hlen2) = conv_tm(
                    &h, hlen, &c2.w, &c2.b, c2.out_ch, c2.in_ch, c2.k, c2.dilation, c2.padding,
                    &format!("acevae.b{bi}u{ui}c2"),
                )?;
                if hlen2 != len {
                    return Err(DiffusionError::model("ace vae res len"));
                }
                x = gpu_add(&skip, &hh2).map_err(|e| dev_err("ace vae res", e))?;
                let _ = hlen2;
            }
        }
        x = snake_tm(&x, &self.snake_out.alpha, &self.snake_out.inv_beta)?;
        let co = &self.conv_out;
        let (yy, nlen) = conv_tm(
            &x, len, &co.w, &co.b, co.out_ch, co.in_ch, co.k, co.dilation, co.padding, "acevae.out",
        )?;
        let host = gpu_download(&yy).map_err(|e| dev_err("ace vae out", e))?;
        let expected = frames * ACE_HOP;
        let take = nlen.min(expected);
        let mut left = Vec::with_capacity(take);
        let mut right = Vec::with_capacity(take);
        for t in 0..take {
            left.push(host[t * 2]);
            right.push(host[t * 2 + 1]);
        }
        Ok((left, right))
    }
}

fn plane_clone(p: &Plane) -> Plane {
    Plane {
        ch: p.ch,
        len: p.len,
        data: p.data.clone(),
    }
}

fn snake(p: &mut Plane, s: &Snake) {
    let len = p.len;
    par_rows(&mut p.data, len, &|ch, row| {
        let a = s.alpha[ch];
        let inv = s.inv_beta[ch];
        for v in row.iter_mut() {
            let sn = (a * *v).sin();
            *v += inv * sn * sn;
        }
    });
}

fn conv1d(input: &Plane, conv: &Conv) -> Plane {
    let len = input.len;
    let span = (conv.k - 1) * conv.dilation;
    let out_len = (len + 2 * conv.padding).saturating_sub(span);
    let mut out = vec![0f32; conv.out_ch * out_len];
    par_rows(&mut out, out_len, &|o, out_row| {
        out_row.fill(conv.b.get(o).copied().unwrap_or(0.0));
        let w_base = &conv.w[o * conv.in_ch * conv.k..(o + 1) * conv.in_ch * conv.k];
        for c in 0..conv.in_ch {
            let in_row = &input.data[c * len..(c + 1) * len];
            for tap in 0..conv.k {
                let w = w_base[c * conv.k + tap];
                if w == 0.0 {
                    continue;
                }
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
    let len = input.len;
    let out_len = (len - 1) * conv.stride + conv.k - 2 * conv.padding;
    let mut out = vec![0f32; conv.out_ch * out_len];
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
                    if t < out_len {
                        out_row[t] += w * in_row[src];
                    }
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
