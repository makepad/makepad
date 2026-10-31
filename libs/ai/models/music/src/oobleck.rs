//! Oobleck VAE decoder (stable-audio-tools AutoencoderOobleck), shared by
//! ACE-Step 1.5 and YuE2: weight-norm convs folded at load
//! (`w = g * v / ||v||`), log-scale SnakeBeta, ConvTranspose1d upsampling
//! (k = 2*stride, pad = ceil(stride/2)), three residual units per block
//! (dilation 1, 3, 9). Geometry and tensor naming come from an
//! [`OobleckSpec`].
//!
//! - ACE-Step 1.5: diffusers naming, 128 channels x [1,1,2,4,8,16],
//!   upsampling [10,6,4,4,2].
//! - YuE2-Vae: stable-audio-tools `nn.Sequential` naming, 64 channels x
//!   [1,1,2,4,8,16,32], upsampling [6,5,4,4,2,2], final conv without bias,
//!   no tanh.

use crate::ace::{ace_open_shards, ace_tensor_any};
use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_sfx::sa3::{par_rows, F16Weight};
use crate::{emit_progress, ProgressHook};
use std::path::Path;

/// Tensor naming of the checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OobleckNames {
    /// diffusers `AutoencoderOobleck` (`decoder.block.N.res_unit1.conv1`).
    Diffusers,
    /// stable-audio-tools `nn.Sequential` (`decoder.layers.N.layers.M`).
    StableAudio,
}

#[derive(Clone, Copy, Debug)]
pub struct OobleckSpec {
    pub channels: usize,
    /// Channel multiples including the leading 1 (len = blocks + 1).
    pub mults: &'static [usize],
    /// Upsampling strides in decoder order (len = blocks).
    pub strides: &'static [usize],
    pub latent_dim: usize,
    pub audio_channels: usize,
    pub hop: usize,
    pub out_bias: bool,
    pub names: OobleckNames,
    /// Device weight-cache namespace (distinct per checkpoint family).
    pub key: &'static str,
}

pub const ACE_OOBLECK: OobleckSpec = OobleckSpec {
    channels: crate::ace::ACE_VAE_CHANNELS,
    mults: &crate::ace::ACE_VAE_MULTS,
    strides: &crate::ace::ACE_VAE_STRIDES,
    latent_dim: crate::ace::ACE_LATENT_DIM,
    audio_channels: crate::ace::ACE_AUDIO_CHANNELS,
    hop: crate::ace::ACE_HOP,
    out_bias: false,
    names: OobleckNames::Diffusers,
    key: "acevae",
};

pub const YUE2_OOBLECK: OobleckSpec = OobleckSpec {
    channels: 64,
    mults: &[1, 1, 2, 4, 8, 16, 32],
    strides: &[6, 5, 4, 4, 2, 2],
    latent_dim: 64,
    audio_channels: 2,
    hop: 1920,
    out_bias: false,
    names: OobleckNames::StableAudio,
    key: "yue2vae",
};

impl OobleckSpec {
    fn conv_in(&self) -> String {
        match self.names {
            OobleckNames::Diffusers => "decoder.conv1".into(),
            OobleckNames::StableAudio => "decoder.layers.0".into(),
        }
    }
    fn block(&self, i: usize) -> String {
        match self.names {
            OobleckNames::Diffusers => format!("decoder.block.{i}"),
            OobleckNames::StableAudio => format!("decoder.layers.{}", i + 1),
        }
    }
    fn block_snake(&self, i: usize) -> String {
        match self.names {
            OobleckNames::Diffusers => format!("{}.snake1", self.block(i)),
            OobleckNames::StableAudio => format!("{}.layers.0", self.block(i)),
        }
    }
    fn block_up(&self, i: usize) -> String {
        match self.names {
            OobleckNames::Diffusers => format!("{}.conv_t1", self.block(i)),
            OobleckNames::StableAudio => format!("{}.layers.1", self.block(i)),
        }
    }
    /// (snake1, conv1, snake2, conv2) of residual unit `u` (0..3).
    fn unit(&self, i: usize, u: usize) -> [String; 4] {
        match self.names {
            OobleckNames::Diffusers => {
                let p = format!("{}.res_unit{}", self.block(i), u + 1);
                [format!("{p}.snake1"), format!("{p}.conv1"), format!("{p}.snake2"), format!("{p}.conv2")]
            }
            OobleckNames::StableAudio => {
                let p = format!("{}.layers.{}.layers", self.block(i), u + 2);
                [format!("{p}.0"), format!("{p}.1"), format!("{p}.2"), format!("{p}.3")]
            }
        }
    }
    fn snake_out(&self) -> String {
        match self.names {
            OobleckNames::Diffusers => "decoder.snake1".into(),
            OobleckNames::StableAudio => format!("decoder.layers.{}", self.strides.len() + 1),
        }
    }
    fn conv_out(&self) -> String {
        match self.names {
            OobleckNames::Diffusers => "decoder.conv2".into(),
            OobleckNames::StableAudio => format!("decoder.layers.{}", self.strides.len() + 2),
        }
    }
}

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
    // The existing two-GEMM/stitch layout, packed once at model load.
    // Smaller decode tiles must not repeat this host-side weight conversion.
    device_lo: F16Weight,
    device_hi: F16Weight,
    device_bias: Vec<f32>,
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

pub struct OobleckDecoder {
    spec: OobleckSpec,
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
            "vae {prefix}: g {} v {} expected g {out_ch} v {}",
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
            "vae {prefix}.bias {} expected {out_ch}",
            b.len()
        )));
    }
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
            "vae tconv {prefix}: g {} v {} expected g {in_ch} v {}",
            g.len(),
            v.len(),
            in_ch * out_ch * k
        )));
    }
    let b = ace_tensor_any(weights, &[&format!("{prefix}.bias")])?;
    let w = fold_weight_norm(&g, &v, in_ch, out_ch * k);
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
    let device_lo = F16Weight::new(format!("{prefix}.lo"), &lo, stride * out_ch, in_ch);
    let device_hi = F16Weight::new(format!("{prefix}.hi"), &hi, stride * out_ch, in_ch);
    let device_bias = b.repeat(stride);
    Ok(TConv {
        w,
        b,
        in_ch,
        out_ch,
        k,
        stride,
        padding: stride.div_ceil(2),
        device_lo,
        device_hi,
        device_bias,
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
            "vae snake {prefix}: alpha {} beta {} expected {ch}",
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

fn load_res_unit(weights: &H3ShardedWeights, names: &[String; 4], ch: usize, dilation: usize) -> Result<ResUnit> {
    Ok(ResUnit {
        snake1: load_snake(weights, &names[0], ch)?,
        conv1: load_conv(weights, &names[1], ch, ch, 7, dilation, (7 - 1) * dilation / 2, true)?,
        snake2: load_snake(weights, &names[2], ch)?,
        conv2: load_conv(weights, &names[3], ch, ch, 1, 1, 0, true)?,
    })
}

impl OobleckDecoder {
    pub fn load(dir: impl AsRef<Path>, spec: OobleckSpec) -> Result<Self> {
        Self::load_with_progress(dir, spec, None)
    }

    pub fn load_with_progress(
        dir: impl AsRef<Path>,
        spec: OobleckSpec,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = ace_open_shards(dir)?;
        emit_progress(&mut progress, "load vae", 0.0)?;
        let strides = spec.strides;
        let n = strides.len();
        let ch = spec.channels;
        let conv_in = load_conv(&weights, &spec.conv_in(), ch * spec.mults[n], spec.latent_dim, 7, 1, 3, true)?;
        let mut blocks = Vec::with_capacity(n);
        for (i, &stride) in strides.iter().enumerate() {
            emit_progress(
                &mut progress,
                &format!("load vae {i}/{n}"),
                (i as f64 + 0.2) / (n as f64 + 0.4),
            )?;
            let in_ch = ch * spec.mults[n - i];
            let out_ch = ch * spec.mults[n - i - 1];
            blocks.push(DecBlock {
                snake: load_snake(&weights, &spec.block_snake(i), in_ch)?,
                up: load_tconv(&weights, &spec.block_up(i), in_ch, out_ch, stride)?,
                units: [
                    load_res_unit(&weights, &spec.unit(i, 0), out_ch, 1)?,
                    load_res_unit(&weights, &spec.unit(i, 1), out_ch, 3)?,
                    load_res_unit(&weights, &spec.unit(i, 2), out_ch, 9)?,
                ],
            });
        }
        let snake_out = load_snake(&weights, &spec.snake_out(), ch)?;
        let conv_out = load_conv(&weights, &spec.conv_out(), spec.audio_channels, ch, 7, 1, 3, spec.out_bias)?;
        emit_progress(&mut progress, "load vae", 1.0)?;
        Ok(Self { spec, conv_in, blocks, snake_out, conv_out })
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
        if latents.len() != frames * self.spec.latent_dim {
            return Err(DiffusionError::model(format!(
                "vae: {} latent values, expected {}",
                latents.len(),
                frames * self.spec.latent_dim
            )));
        }
        // Channel-major plane for the conv path.
        let mut plane = Plane {
            ch: self.spec.latent_dim,
            len: frames,
            data: vec![0f32; self.spec.latent_dim * frames],
        };
        for t in 0..frames {
            for c in 0..self.spec.latent_dim {
                plane.data[c * frames + t] = latents[t * self.spec.latent_dim + c];
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
                        "vae residual length {} vs {}",
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
        if plane.ch != self.spec.audio_channels {
            return Err(DiffusionError::model(format!(
                "vae produced {} channels",
                plane.ch
            )));
        }
        let expected = frames * self.spec.hop;
        if plane.len < expected.saturating_sub(self.spec.hop) {
            return Err(DiffusionError::model(format!(
                "vae produced {} samples, expected ~{expected}",
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

    /// Decode on the device; planar stereo cropped to `frames * hop`.
    pub fn decode_device(&self, latents: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let (host, nlen) = self.decode_device_raw(latents, frames)?;
        let expected = frames * self.spec.hop;
        let take = nlen.min(expected);
        let mut left = Vec::with_capacity(take);
        let mut right = Vec::with_capacity(take);
        for t in 0..take {
            left.push(host[t * 2]);
            right.push(host[t * 2 + 1]);
        }
        Ok((left, right))
    }

    /// Output length of a full decode of `frames` latents.
    pub fn natural_output_length(&self, frames: usize) -> usize {
        let mut len = frames;
        for &s in self.spec.strides {
            len = (len - 1) * s + 2 * s - 2 * s.div_ceil(2);
        }
        len
    }

    /// Reference `decode_tiled`: decode `core`-latent tiles with `halo`
    /// latents of context each side and keep exact cores (no blending).
    /// Decode only `output_frames` cores, retaining context from `frames`
    /// input latents. A proper prefix has `output_frames * hop` samples;
    /// the full song retains its natural (possibly shorter) output length.
    pub fn decode_tiled_device(
        &self,
        latents: &[f32],
        frames: usize,
        output_frames: usize,
        core: usize,
        halo: usize,
        progress: &mut dyn FnMut(usize, usize) -> Result<()>,
    ) -> Result<Vec<f32>> {
        let dim = self.spec.latent_dim;
        let ch = self.spec.audio_channels;
        if latents.len() != frames * dim || frames == 0 || core == 0
            || output_frames == 0 || output_frames > frames {
            return Err(DiffusionError::model("vae tiled: shape"));
        }
        let ratio = self.spec.hop;
        let total = self.natural_output_length(frames).min(output_frames * ratio);
        let mut audio = vec![0f32; total * ch];
        let tiles = output_frames.div_ceil(core);
        for (index, start) in (0..output_frames).step_by(core).enumerate() {
            let end = (start + core).min(output_frames);
            let left = start.saturating_sub(halo);
            let right = (end + halo).min(frames);
            let (tile, tile_len) = self.decode_device_raw(&latents[left * dim..right * dim], right - left)?;
            let out_start = start * ratio;
            let out_end = (end * ratio).min(total);
            let crop_start = (start - left) * ratio;
            if crop_start + (out_end - out_start) > tile_len {
                return Err(DiffusionError::model("vae tile did not cover its requested output core"));
            }
            audio[out_start * ch..out_end * ch]
                .copy_from_slice(&tile[crop_start * ch..(crop_start + out_end - out_start) * ch]);
            progress(index + 1, tiles)?;
        }
        Ok(audio)
    }

    /// Device decode: interleaved `[len, audio_channels]` and `len`.
    fn decode_device_raw(&self, latents: &[f32], frames: usize) -> Result<(Vec<f32>, usize)> {
        use makepad_ai_sfx::sa3::dev_err;
        
        use makepad_ai_common::backend::cuda::{
            gpu_add, gpu_download, gpu_upload,
            GpuTensor,
        };

        if latents.len() != frames * self.spec.latent_dim {
            return Err(DiffusionError::model("vae device: shape"));
        }
        // Time-major [T, C]. Conv padding and shifts stay entirely on device.
        let mut x = gpu_upload(latents, frames, self.spec.latent_dim).map_err(|e| dev_err("vae up", e))?;
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
            ns: &str,
        ) -> Result<(GpuTensor, usize)> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_common::backend::cuda::gpu_conv1d_cached;
            if x.rows() != len || x.cols() != in_ch {
                return Err(DiffusionError::model("vae conv input shape"));
            }
            let out = gpu_conv1d_cached(x, w, b, out_ch, k, dilation, padding, ns, key)
                .map_err(|e| dev_err("vae conv", e))?;
            let out_len = out.rows();
            Ok((out, out_len))
        }

        fn snake_tm(x: &GpuTensor, alpha: &[f32], inv_beta: &[f32]) -> Result<GpuTensor> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_common::backend::cuda::gpu_snake;
            gpu_snake(x, alpha, inv_beta).map_err(|e| dev_err("vae snake", e))
        }

        fn tconv_tm(
            x: &GpuTensor,
            len: usize,
            conv: &TConv,
            ns: &str,
        ) -> Result<(GpuTensor, usize)> {
            use makepad_ai_sfx::sa3::dev_err;
            use makepad_ai_common::backend::cuda::gpu_linear_nt_cached;
            if conv.k != 2 * conv.stride {
                return Err(DiffusionError::model("vae tconv k"));
            }
            let y_hi = gpu_linear_nt_cached(x, ns, &[conv.device_lo.part()], &conv.device_bias)
                .map_err(|e| dev_err("vae tlo", e))?;
            let y_lo = gpu_linear_nt_cached(x, ns, &[conv.device_hi.part()], &[])
                .map_err(|e| dev_err("vae thi", e))?;
            let g = makepad_ai_common::backend::cuda::gpu_tconv_stitch(
                &y_hi, &y_lo, len, conv.out_ch, conv.stride, conv.padding, conv.k,
            )
            .map_err(|e| dev_err("vae stitch", e))?;
            let out_len = g.rows();
            Ok((g, out_len))
        }

        let ns = self.spec.key;
        let conv = &self.conv_in;
        let (y, nlen) = conv_tm(
            &x, len, &conv.w, &conv.b, conv.out_ch, conv.in_ch, conv.k, conv.dilation, conv.padding, "in", ns,
        )?;
        x = y;
        len = nlen;

        for (bi, block) in self.blocks.iter().enumerate() {
            x = snake_tm(&x, &block.snake.alpha, &block.snake.inv_beta)?;
            let up = &block.up;
            let (yy, nn) = tconv_tm(&x, len, up, ns)?;
            x = yy;
            len = nn;
            for (ui, unit) in block.units.iter().enumerate() {
                let skip = x;
                let mut h = snake_tm(&skip, &unit.snake1.alpha, &unit.snake1.inv_beta)?;
                let c1 = &unit.conv1;
                let (hh, hlen) = conv_tm(
                    &h, len, &c1.w, &c1.b, c1.out_ch, c1.in_ch, c1.k, c1.dilation, c1.padding,
                    &format!("b{bi}u{ui}c1"), ns,
                )?;
                h = snake_tm(&hh, &unit.snake2.alpha, &unit.snake2.inv_beta)?;
                drop(hh);
                let c2 = &unit.conv2;
                let (hh2, hlen2) = conv_tm(
                    &h, hlen, &c2.w, &c2.b, c2.out_ch, c2.in_ch, c2.k, c2.dilation, c2.padding,
                    &format!("b{bi}u{ui}c2"), ns,
                )?;
                drop(h);
                if hlen2 != len {
                    return Err(DiffusionError::model("vae res len"));
                }
                x = gpu_add(&skip, &hh2).map_err(|e| dev_err("vae res", e))?;
                let _ = hlen2;
            }
        }
        x = snake_tm(&x, &self.snake_out.alpha, &self.snake_out.inv_beta)?;
        let co = &self.conv_out;
        let (yy, nlen) = conv_tm(
            &x, len, &co.w, &co.b, co.out_ch, co.in_ch, co.k, co.dilation, co.padding, "out", ns,
        )?;
        let host = gpu_download(&yy).map_err(|e| dev_err("vae out", e))?;
        Ok((host, nlen))
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
