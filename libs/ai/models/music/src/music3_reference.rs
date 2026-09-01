//! Reference-audio conditioning for MiniMax-Music3 (plan: repo-root
//! `music3.md`).
//!
//! The open Music3 checkpoint is caption + lyrics only; MiniMax never shipped
//! the training-time audio -> RVQ encoder. The one in-family way to let a
//! clip steer a song is the community reverse-distilled encoder
//! (SimpleTuner `open-rvq-encoder-minimax-music3`, v4 169M, distilled from
//! Music3 output): encode the clip to MiniMax-style RVQ codes, then constrain
//! every Nth semantic `c0` draw of the AR loop to the encoder's top-5. The
//! seven acoustic books stay MiniMax's (see `music3_ar::Music3ReferenceMask`).
//!
//! This module is everything before the AR loop:
//!
//! ```text
//! stereo f32 @ any rate
//!   -> resample to 44.1 kHz (DAV native; hop 512 -> 86.13 Hz latents)
//!   -> DAV encoder: the official `dav.pth` encoder half + `mean_proj`;
//!      each channel is encoded MONO through the 1-in DAC-style stack,
//!      2 x 64 projected features -> 128 latent channels (left 0..64,
//!      right 64..128)
//!   -> RVQ encoder v4: conv stem on the latents, mean-pool each 25 Hz
//!      frame's latent span (frame i covers latents [i*441/128,
//!      (i+1)*441/128)), 8-layer muP transformer over <= 128 frames
//!      (5.12 s), semantic head -> per-frame `c0` top-5; causal depth
//!      decoder -> `c1..c7` (validation canary only, never fed to AR)
//! ```
//!
//! Window stitch is the adapter's, not a crossfade: windows start every 128
//! frames, plus one tail window ending at the last frame; a frame keeps the
//! first window that produced it. The encoder has no cross-window state, so
//! long clips seam at 5.12 s — prefer 8-20 s references.
//!
//! Numerics: f32 end to end (the adapter runs the RVQ encoder under bf16
//! autocast on CUDA, so a few near-tie `c0` top-1 ids differ; the top-5 set
//! is the product signal). Weight-norm convs are folded at load. GEMMs go
//! through `sa3::linear` (Metal offload on macOS, threaded CPU elsewhere);
//! the conv stacks are threaded CPU loops.

use crate::{DiffusionError, Result};
use makepad_ai_common::audio::resample_mono;
use makepad_ai_common::json::Json;
use makepad_ai_common::torch_pth::PthStateDict;
use makepad_ai_sfx::sa3::{linear, par_rows};
use std::path::{Path, PathBuf};

/// DAV native rate and hop: 44.1 kHz / 512 = 86.13 Hz latents.
pub const MUSIC3_REFERENCE_SAMPLE_RATE: u32 = 44_100;
pub const MUSIC3_DAV_HOP: usize = 512;
/// Two channels x 64 `mean_proj` features.
pub const MUSIC3_DAV_LATENT: usize = 128;
/// 25 Hz frame i starts at latent `(i * 441) / 128`.
const LATENT_RATE_NUM: usize = 441;
const LATENT_RATE_DEN: usize = 128;
/// Semantic candidates handed to the AR mask per constrained frame.
pub const MUSIC3_REFERENCE_CANDIDATES: usize = 5;
/// `strength` -> `reference_interval` range (adapter contract 1..=10).
pub const MUSIC3_REFERENCE_MAX_INTERVAL: usize = 10;
/// Reference clip bounds after decode (seconds).
pub const MUSIC3_REFERENCE_MIN_SECONDS: f64 = 2.0;
pub const MUSIC3_REFERENCE_MAX_SECONDS: f64 = 60.0;

/// Registry roles + cache-relative paths of the reference weights. They sit
/// next to the bf16 `MiniMax-Music3` tree (the Q4 pack borrows them from
/// there); text-only generation never touches them.
pub const MUSIC3_DAV_ROLE: &str = "dav-pth";
pub const MUSIC3_DAV_FILE: &str = "dav.pth";
pub const MUSIC3_RVQ_ENCODER_ROLE: &str = "rvq-encoder";
pub const MUSIC3_RVQ_ENCODER_FILE: &str =
    "encoders/minimax_music3_rvq_encoder_v4_169m_autoregressive_depth_recommended.safetensors";
pub const MUSIC3_RVQ_ENCODER_CONFIG_ROLE: &str = "rvq-encoder-config";
pub const MUSIC3_RVQ_ENCODER_CONFIG_FILE: &str =
    "encoders/minimax_music3_rvq_encoder_v4_169m_autoregressive_depth_recommended.json";

/// `strength` (0..=1, `None` = default) -> AR constraint interval. The
/// contract points are fixed: 1.0 constrains every frame, 0.0 every tenth,
/// and the default lands on the adapter's tested interval 5. A linear map
/// cannot hit all three (1 + 9*(1-0.8) = 2.8), so the curve is
/// `1 + 9*sqrt(1-strength)`: fine control near "tight", coarse near
/// "loose".
pub fn music3_reference_interval(strength: Option<f32>) -> usize {
    let s = strength.unwrap_or(0.8);
    let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 0.8 };
    let interval = (1.0 + 9.0 * (1.0 - s).sqrt()).round() as usize;
    interval.clamp(1, MUSIC3_REFERENCE_MAX_INTERVAL)
}

/// A decoded reference clip, planar stereo at any rate (the backend decodes
/// and applies the 2..60 s clip policy; this module resamples).
#[derive(Clone, Debug)]
pub struct Music3ReferenceAudio {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub rate: u32,
}

impl Music3ReferenceAudio {
    pub fn seconds(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.left.len() as f64 / self.rate as f64
        }
    }
}

/// Encoder output: per 25 Hz frame the full `[c0, c1..c7]` prediction (the
/// validation canary) and the semantic top-5 the AR mask consumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Music3ReferenceEncoding {
    pub frames: usize,
    pub codes: Vec<[u32; 8]>,
    pub c0_topk: Vec<[u32; MUSIC3_REFERENCE_CANDIDATES]>,
}

/// Resolved reference weight paths. Fails closed naming the registry role
/// that is missing, so a box without the optional pull says so instead of
/// generating text-only behind a wav slot.
#[derive(Clone, Debug)]
pub struct Music3ReferenceWeights {
    pub dav: PathBuf,
    pub encoder: PathBuf,
    pub config: PathBuf,
}

impl Music3ReferenceWeights {
    pub fn resolve(dir: &Path) -> Result<Self> {
        let need = |role: &str, rel: &str| -> Result<PathBuf> {
            let path = dir.join(rel.split('/').collect::<PathBuf>());
            if path.is_file() {
                Ok(path)
            } else {
                Err(DiffusionError::model(format!(
                    "music3 reference audio needs the registry role {role:?} ({rel}) under {}; \
                     pull minimax-music3 with its optional reference weights",
                    dir.display()
                )))
            }
        };
        Ok(Self {
            dav: need(MUSIC3_DAV_ROLE, MUSIC3_DAV_FILE)?,
            encoder: need(MUSIC3_RVQ_ENCODER_ROLE, MUSIC3_RVQ_ENCODER_FILE)?,
            config: need(MUSIC3_RVQ_ENCODER_CONFIG_ROLE, MUSIC3_RVQ_ENCODER_CONFIG_FILE)?,
        })
    }
}

/// Latent index where each 25 Hz frame starts, `frame_count + 1` entries
/// (adapter `frame_latent_starts`).
pub fn frame_latent_starts(frame_count: usize) -> Vec<usize> {
    (0..=frame_count)
        .map(|index| (index * LATENT_RATE_NUM) / LATENT_RATE_DEN)
        .collect()
}

/// Encoder window starts over `frame_count` frames (adapter tiling): every
/// `window` frames, plus the tail window `frame_count - window` so the last
/// frames are covered by a full window. Frames keep the first window that
/// produced them.
pub fn reference_window_starts(frame_count: usize, window: usize) -> Vec<usize> {
    let window = window.max(1);
    if frame_count < window {
        return vec![0];
    }
    let mut starts: Vec<usize> = (0..frame_count + 1 - window).step_by(window).collect();
    let tail = frame_count - window;
    if !starts.contains(&tail) {
        starts.push(tail);
    }
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// Clip -> codes + semantic candidates. `progress(stage, k, n)`; stages are
/// `dav` (per channel block), `rvq` (per window). Encode BEFORE the 8B LM is
/// resident: the two encoders are ~1.5 GB of f32 on the host and nothing
/// here touches the LM's device cache.
pub fn music3_encode_reference(
    weights: &Music3ReferenceWeights,
    audio: &Music3ReferenceAudio,
    progress: &mut dyn FnMut(&str, usize, usize),
    should_cancel: &dyn Fn() -> bool,
) -> Result<Music3ReferenceEncoding> {
    if audio.rate == 0 || audio.left.is_empty() || audio.left.len() != audio.right.len() {
        return Err(DiffusionError::model("music3 reference: planar stereo shape"));
    }
    let cancel = || -> Result<()> {
        if should_cancel() {
            Err(DiffusionError::Cancelled)
        } else {
            Ok(())
        }
    };
    cancel()?;
    let left = resample_mono(&audio.left, audio.rate, MUSIC3_REFERENCE_SAMPLE_RATE);
    let right = resample_mono(&audio.right, audio.rate, MUSIC3_REFERENCE_SAMPLE_RATE);
    let samples = left.len().min(right.len());
    let samples_per_frame = MUSIC3_REFERENCE_SAMPLE_RATE as usize / 25;
    if samples < samples_per_frame {
        return Err(DiffusionError::model(
            "music3 reference: clip shorter than one 25 Hz frame",
        ));
    }
    progress("load dav", 0, 1);
    let dav = DavEncoder::load(&weights.dav)?;
    cancel()?;
    let latents = dav.encode(&left[..samples], &right[..samples], progress, should_cancel)?;
    drop(dav);
    cancel()?;

    // Continuous 25 Hz frames over the ORIGINAL (unpadded) length; drop
    // trailing frames whose latent span runs past what DAV produced.
    let mut frame_count = samples * 25 / MUSIC3_REFERENCE_SAMPLE_RATE as usize;
    let mut bounds = frame_latent_starts(frame_count);
    while frame_count > 0 && bounds[frame_count] > latents.frames {
        frame_count -= 1;
        bounds = frame_latent_starts(frame_count);
    }
    if frame_count == 0 {
        return Err(DiffusionError::model(
            "music3 reference: DAV encoding produced no complete frames",
        ));
    }

    progress("load rvq-encoder", 0, 1);
    let rvq = RvqEncoder::load(&weights.encoder, &weights.config)?;
    let window = rvq.cfg.max_position_embeddings;
    let starts = reference_window_starts(frame_count, window);
    let mut codes = vec![[0u32; 8]; frame_count];
    let mut c0_topk = vec![[0u32; MUSIC3_REFERENCE_CANDIDATES]; frame_count];
    let mut assigned = vec![false; frame_count];
    for (w, &start) in starts.iter().enumerate() {
        cancel()?;
        progress("rvq", w, starts.len());
        let end = (start + window).min(frame_count);
        let local = &bounds[start..=end];
        let lat0 = local[0];
        let lat1 = local[local.len() - 1];
        let slice = &latents.data[lat0 * MUSIC3_DAV_LATENT..lat1 * MUSIC3_DAV_LATENT];
        let out = rvq.encode_window(slice, lat1 - lat0, local)?;
        for (f, frame) in (start..end).enumerate() {
            if assigned[frame] {
                continue;
            }
            assigned[frame] = true;
            codes[frame] = out.codes[f];
            c0_topk[frame] = out.c0_topk[f];
        }
    }
    progress("rvq", starts.len(), starts.len());
    if assigned.iter().any(|a| !a) {
        return Err(DiffusionError::model(
            "music3 reference: window tiling left frames unassigned",
        ));
    }
    Ok(Music3ReferenceEncoding {
        frames: frame_count,
        codes,
        c0_topk,
    })
}

// ---------------------------------------------------------------------------
// Planar conv primitives (CPU, threaded over output channels)
// ---------------------------------------------------------------------------

/// `[ch][len]` planar signal.
struct Plane {
    ch: usize,
    len: usize,
    data: Vec<f32>,
}

/// torch `Conv1d` with weight `[out][in][k]`, optional stride/dilation.
struct Conv {
    out_ch: usize,
    in_ch: usize,
    k: usize,
    stride: usize,
    dilation: usize,
    padding: usize,
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Conv {
    fn check(&self, what: &str) -> Result<()> {
        if self.w.len() != self.out_ch * self.in_ch * self.k
            || self.b.len() != self.out_ch
            || self.stride == 0
            || self.dilation == 0
            || self.k == 0
        {
            return Err(DiffusionError::model(format!(
                "music3 reference {what}: conv geometry out={} in={} k={} w={} b={}",
                self.out_ch,
                self.in_ch,
                self.k,
                self.w.len(),
                self.b.len()
            )));
        }
        Ok(())
    }

    fn out_len(&self, len: usize) -> usize {
        let span = (self.k - 1) * self.dilation;
        let padded = len + 2 * self.padding;
        if padded <= span {
            0
        } else {
            (padded - span - 1) / self.stride + 1
        }
    }
}

fn conv1d(input: &Plane, conv: &Conv) -> Plane {
    debug_assert_eq!(input.ch, conv.in_ch);
    let len = input.len;
    let out_len = conv.out_len(len);
    let mut out = vec![0f32; conv.out_ch * out_len];
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
                // out[t] += w * in[t*stride + tap*dilation - padding]
                let off = tap as isize * conv.dilation as isize - conv.padding as isize;
                let t_start = if off < 0 {
                    ((-off) as usize).div_ceil(conv.stride)
                } else {
                    0
                };
                let t_end = if len as isize <= off {
                    0
                } else {
                    (((len as isize - off - 1) / conv.stride as isize) as usize + 1).min(out_len)
                };
                if t_start >= t_end {
                    continue;
                }
                let src0 = (t_start as isize * conv.stride as isize + off) as usize;
                let n = t_end - t_start;
                let dst = &mut out_row[t_start..t_end];
                if conv.stride == 1 {
                    for (d, s) in dst.iter_mut().zip(&in_row[src0..src0 + n]) {
                        *d += w * *s;
                    }
                } else {
                    for (d, s) in dst
                        .iter_mut()
                        .zip(in_row[src0..].iter().step_by(conv.stride))
                    {
                        *d += w * *s;
                    }
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

/// DAV Snake1d: `x + sin(alpha x)^2 / (alpha + 1e-9)` (plain alpha, not the
/// log-scale ACE variant).
fn snake(p: &mut Plane, alpha: &[f32]) {
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

/// `w = g * v / ||v||`, norm over everything but the output dim
/// (`torch.nn.utils.weight_norm` default `dim=0`).
fn fold_weight_norm(g: &[f32], v: &[f32], out_ch: usize) -> Vec<f32> {
    let per = v.len() / out_ch.max(1);
    let mut w = vec![0f32; v.len()];
    for o in 0..out_ch {
        let src = &v[o * per..(o + 1) * per];
        let norm = src.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
        let scale = if norm > 0.0 { g[o] as f64 / norm } else { 0.0 };
        for (dst, x) in w[o * per..(o + 1) * per].iter_mut().zip(src) {
            *dst = (*x as f64 * scale) as f32;
        }
    }
    w
}

/// Abramowitz-Stegun 7.1.26 erf, |err| < 1.5e-7: torch's exact-erf GELU to
/// f32 working precision without an f64 series per activation.
fn erf_f32(x: f32) -> f32 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * ax);
    let poly = ((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736)
        * t
        + 0.254_829_592;
    sign * (1.0 - poly * t * (-ax * ax).exp())
}

fn gelu_erf(x: f32) -> f32 {
    0.5 * x * (1.0 + erf_f32(x / std::f32::consts::SQRT_2))
}

// ---------------------------------------------------------------------------
// DAV encoder (official dav.pth encoder half)
// ---------------------------------------------------------------------------

struct DavResUnit {
    snake1: Vec<f32>,
    conv1: Conv,
    snake2: Vec<f32>,
    conv2: Conv,
}

struct DavBlock {
    units: Vec<DavResUnit>,
    snake: Vec<f32>,
    down: Conv,
}

/// `DAVEncoderOnly`: conv_in(1->64,k7) -> 4 blocks (3 res units at dim/2 with
/// dilations 1/3/9, snake, strided conv dim/2->dim k=2s) at strides 2/4/8/8
/// -> snake -> conv(1024->1024,k3) -> mean_proj(1024->64,k1). Hop 512.
pub struct DavEncoder {
    conv_in: Conv,
    blocks: Vec<DavBlock>,
    snake_out: Vec<f32>,
    conv_out: Conv,
    mean_proj: Conv,
}

/// DAV latents, time-major `[frames][128]`.
pub struct DavLatents {
    pub frames: usize,
    pub data: Vec<f32>,
}

const DAV_STRIDES: [usize; 4] = [2, 4, 8, 8];
const DAV_DILATIONS: [usize; 3] = [1, 3, 9];

impl DavEncoder {
    pub fn load(path: &Path) -> Result<Self> {
        let mut sd = PthStateDict::load(path).map_err(|e| {
            DiffusionError::model(format!("music3 dav.pth {}: {e}", path.display()))
        })?;
        let read = |sd: &mut PthStateDict, name: &str| -> Result<Vec<f32>> {
            sd.f32(name)
                .map_err(|e| DiffusionError::model(format!("music3 dav.pth {name}: {e}")))
        };
        let wn_conv = |sd: &mut PthStateDict,
                       prefix: &str,
                       out_ch: usize,
                       in_ch: usize,
                       k: usize,
                       stride: usize,
                       dilation: usize,
                       padding: usize|
         -> Result<Conv> {
            let g = read(sd, &format!("{prefix}.weight_g"))?;
            let v = read(sd, &format!("{prefix}.weight_v"))?;
            let b = read(sd, &format!("{prefix}.bias"))?;
            if g.len() != out_ch || v.len() != out_ch * in_ch * k {
                return Err(DiffusionError::model(format!(
                    "music3 dav.pth {prefix}: expected [{out_ch},{in_ch},{k}], got g={} v={}",
                    g.len(),
                    v.len()
                )));
            }
            let conv = Conv {
                out_ch,
                in_ch,
                k,
                stride,
                dilation,
                padding,
                w: fold_weight_norm(&g, &v, out_ch),
                b,
            };
            conv.check(prefix)?;
            Ok(conv)
        };
        let alpha = |sd: &mut PthStateDict, name: &str, ch: usize| -> Result<Vec<f32>> {
            let a = read(sd, name)?;
            if a.len() != ch {
                return Err(DiffusionError::model(format!(
                    "music3 dav.pth {name}: expected {ch} snake alphas, got {}",
                    a.len()
                )));
            }
            Ok(a)
        };

        let conv_in = wn_conv(&mut sd, "encoder.block.0", 64, 1, 7, 1, 1, 3)?;
        let mut blocks = Vec::with_capacity(4);
        let mut dim = 64usize;
        for (i, &stride) in DAV_STRIDES.iter().enumerate() {
            dim *= 2;
            let half = dim / 2;
            let base = format!("encoder.block.{}", i + 1);
            let mut units = Vec::with_capacity(3);
            for (j, &dilation) in DAV_DILATIONS.iter().enumerate() {
                let u = format!("{base}.block.{j}.block");
                units.push(DavResUnit {
                    snake1: alpha(&mut sd, &format!("{u}.0.alpha"), half)?,
                    conv1: wn_conv(&mut sd, &format!("{u}.1"), half, half, 7, 1, dilation, 3 * dilation)?,
                    snake2: alpha(&mut sd, &format!("{u}.2.alpha"), half)?,
                    conv2: wn_conv(&mut sd, &format!("{u}.3"), half, half, 1, 1, 1, 0)?,
                });
            }
            blocks.push(DavBlock {
                units,
                snake: alpha(&mut sd, &format!("{base}.block.3.alpha"), half)?,
                down: wn_conv(
                    &mut sd,
                    &format!("{base}.block.4"),
                    dim,
                    half,
                    2 * stride,
                    stride,
                    1,
                    stride.div_ceil(2),
                )?,
            });
        }
        let snake_out = alpha(&mut sd, "encoder.block.5.alpha", dim)?;
        let conv_out = wn_conv(&mut sd, "encoder.block.6", dim, dim, 3, 1, 1, 1)?;
        let w = read(&mut sd, "mean_proj.weight")?;
        let b = read(&mut sd, "mean_proj.bias")?;
        let mean_proj = Conv {
            out_ch: MUSIC3_DAV_LATENT / 2,
            in_ch: dim,
            k: 1,
            stride: 1,
            dilation: 1,
            padding: 0,
            w,
            b,
        };
        mean_proj.check("mean_proj")?;
        Ok(Self {
            conv_in,
            blocks,
            snake_out,
            conv_out,
            mean_proj,
        })
    }

    /// One channel: right-pad to a hop multiple, run the stack, project to
    /// 64 features. Returns planar `[64][len/512]`.
    fn encode_channel(
        &self,
        samples: &[f32],
        progress: &mut dyn FnMut(&str, usize, usize),
        base: usize,
        total: usize,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<Plane> {
        let rem = samples.len() % MUSIC3_DAV_HOP;
        let mut data = samples.to_vec();
        if rem != 0 {
            data.resize(samples.len() + MUSIC3_DAV_HOP - rem, 0.0);
        }
        let mut x = Plane {
            ch: 1,
            len: data.len(),
            data,
        };
        x = conv1d(&x, &self.conv_in);
        progress("dav", base + 1, total);
        for (i, block) in self.blocks.iter().enumerate() {
            if should_cancel() {
                return Err(DiffusionError::Cancelled);
            }
            for unit in &block.units {
                let mut r = Plane {
                    ch: x.ch,
                    len: x.len,
                    data: x.data.clone(),
                };
                snake(&mut r, &unit.snake1);
                r = conv1d(&r, &unit.conv1);
                snake(&mut r, &unit.snake2);
                r = conv1d(&r, &unit.conv2);
                if r.len != x.len {
                    return Err(DiffusionError::model("music3 dav: residual length"));
                }
                for (a, b) in x.data.iter_mut().zip(&r.data) {
                    *a += *b;
                }
            }
            snake(&mut x, &block.snake);
            x = conv1d(&x, &block.down);
            progress("dav", base + 2 + i, total);
        }
        snake(&mut x, &self.snake_out);
        x = conv1d(&x, &self.conv_out);
        let out = conv1d(&x, &self.mean_proj);
        Ok(out)
    }

    /// Stereo (equal-length planar) -> time-major `[frames][128]` latents,
    /// left features first.
    pub fn encode(
        &self,
        left: &[f32],
        right: &[f32],
        progress: &mut dyn FnMut(&str, usize, usize),
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<DavLatents> {
        if left.len() != right.len() || left.is_empty() {
            return Err(DiffusionError::model("music3 dav: stereo shape"));
        }
        let per = 2 + self.blocks.len();
        let total = 2 * per;
        let l = self.encode_channel(left, progress, 0, total, should_cancel)?;
        let r = self.encode_channel(right, progress, per, total, should_cancel)?;
        if l.len != r.len || l.ch + r.ch != MUSIC3_DAV_LATENT {
            return Err(DiffusionError::model("music3 dav: latent shape"));
        }
        let frames = l.len;
        let mut data = vec![0f32; frames * MUSIC3_DAV_LATENT];
        for t in 0..frames {
            for c in 0..l.ch {
                data[t * MUSIC3_DAV_LATENT + c] = l.data[c * frames + t];
                data[t * MUSIC3_DAV_LATENT + l.ch + c] = r.data[c * frames + t];
            }
        }
        progress("dav", total, total);
        Ok(DavLatents { frames, data })
    }
}

// ---------------------------------------------------------------------------
// RVQ encoder v4 (SimpleTuner open-rvq-encoder-minimax-music3)
// ---------------------------------------------------------------------------

/// The encoder config JSON next to the weights. Only the v4 layout
/// (`mup` transformer, causal depth decoder) is accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct RvqEncoderConfig {
    pub latent_channels: usize,
    pub codebook_vocab_sizes: Vec<usize>,
    pub d_model: usize,
    pub num_layers: usize,
    pub num_heads: usize,
    pub ff_mult: usize,
    pub max_position_embeddings: usize,
    pub conv_dilations: Vec<usize>,
    pub mup_attention_multiplier: f32,
    pub depth_decoder_dim: usize,
    pub depth_decoder_layers: usize,
    pub depth_decoder_heads: usize,
    pub depth_decoder_ff_mult: usize,
}

impl RvqEncoderConfig {
    pub fn parse(text: &str) -> Result<Self> {
        let json = Json::parse(text)
            .map_err(|e| DiffusionError::model(format!("music3 rvq encoder config: {e}")))?;
        let usize_of = |key: &str| -> Result<usize> {
            json.get(key)
                .and_then(|v| v.as_f64())
                .filter(|v| *v >= 0.0 && v.fract() == 0.0)
                .map(|v| v as usize)
                .ok_or_else(|| {
                    DiffusionError::model(format!("music3 rvq encoder config: missing {key}"))
                })
        };
        let list_of = |key: &str| -> Result<Vec<usize>> {
            json.get(key)
                .and_then(|v| v.as_arr())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_f64())
                        .map(|v| v as usize)
                        .collect::<Vec<_>>()
                })
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    DiffusionError::model(format!("music3 rvq encoder config: missing {key}"))
                })
        };
        let flag = |key: &str| json.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        if !flag("mup") || !flag("depth_decoder") {
            return Err(DiffusionError::model(
                "music3 rvq encoder config: only the v4 layout (mup + depth_decoder) is supported",
            ));
        }
        let cfg = Self {
            latent_channels: usize_of("latent_channels")?,
            codebook_vocab_sizes: list_of("codebook_vocab_sizes")?,
            d_model: usize_of("d_model")?,
            num_layers: usize_of("num_layers")?,
            num_heads: usize_of("num_heads")?,
            ff_mult: usize_of("ff_mult")?,
            max_position_embeddings: usize_of("max_position_embeddings")?,
            conv_dilations: list_of("conv_dilations")?,
            mup_attention_multiplier: json
                .get("mup_attention_multiplier")
                .and_then(|v| v.as_f64())
                .unwrap_or(8.0) as f32,
            depth_decoder_dim: usize_of("depth_decoder_dim")?,
            depth_decoder_layers: usize_of("depth_decoder_layers")?,
            depth_decoder_heads: usize_of("depth_decoder_heads")?,
            depth_decoder_ff_mult: usize_of("depth_decoder_ff_mult")?,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let ok = self.latent_channels == MUSIC3_DAV_LATENT
            && self.codebook_vocab_sizes.len() == 8
            && self.d_model > 0
            && self.num_heads > 0
            && self.d_model % self.num_heads == 0
            && self.num_layers > 0
            && self.max_position_embeddings > 0
            && self.depth_decoder_dim > 0
            && self.depth_decoder_heads > 0
            && self.depth_decoder_dim % self.depth_decoder_heads == 0
            && self.codebook_vocab_sizes.iter().all(|v| *v > 0);
        if ok {
            Ok(())
        } else {
            Err(DiffusionError::model(format!(
                "music3 rvq encoder config: bad geometry {self:?}"
            )))
        }
    }
}

struct LinearW {
    w: Vec<f32>,
    b: Vec<f32>,
    n: usize,
    k: usize,
}

impl LinearW {
    fn apply(&self, x: &[f32], m: usize) -> Vec<f32> {
        let bias = if self.b.is_empty() { None } else { Some(&self.b[..]) };
        linear(x, &self.w, bias, m, self.k, self.n)
    }
}

struct Norm {
    g: Vec<f32>,
    b: Vec<f32>,
}

const NORM_EPS: f32 = 1e-5;

/// torch `LayerNorm` over the last dim of `[m][d]`.
fn layer_norm(x: &[f32], d: usize, norm: &Norm) -> Vec<f32> {
    let mut out = vec![0f32; x.len()];
    par_rows(&mut out, d, &|row, dst| {
        let src = &x[row * d..(row + 1) * d];
        let mean = src.iter().map(|v| *v as f64).sum::<f64>() / d as f64;
        let var = src
            .iter()
            .map(|v| (*v as f64 - mean) * (*v as f64 - mean))
            .sum::<f64>()
            / d as f64;
        let inv = 1.0 / (var + NORM_EPS as f64).sqrt();
        for (i, v) in dst.iter_mut().enumerate() {
            *v = ((src[i] as f64 - mean) * inv) as f32 * norm.g[i] + norm.b[i];
        }
    });
    out
}

/// torch `GroupNorm(1, C)` on a planar `[C][T]`: one mean/var over C x T,
/// per-channel affine.
fn group_norm_all(p: &mut Plane, norm: &Norm) {
    let n = p.data.len() as f64;
    let mean = p.data.iter().map(|v| *v as f64).sum::<f64>() / n;
    let var = p
        .data
        .iter()
        .map(|v| (*v as f64 - mean) * (*v as f64 - mean))
        .sum::<f64>()
        / n;
    let inv = (1.0 / (var + NORM_EPS as f64).sqrt()) as f32;
    let mean = mean as f32;
    let len = p.len;
    par_rows(&mut p.data, len, &|ch, row| {
        let g = norm.g[ch] * inv;
        let b = norm.b[ch];
        for v in row.iter_mut() {
            *v = (*v - mean) * g + b;
        }
    });
}

struct AttnLayer {
    norm1: Norm,
    norm2: Norm,
    q: LinearW,
    k: LinearW,
    v: LinearW,
    o: LinearW,
    ff1: LinearW,
    ff2: LinearW,
}

impl AttnLayer {
    /// Pre-LN block over `[n][d]`; `scale` multiplies the raw scores;
    /// `causal` masks keys after the query (depth decoder).
    fn forward(&self, x: &[f32], n: usize, d: usize, heads: usize, scale: f32, causal: bool) -> Vec<f32> {
        let hd = d / heads;
        let normed = layer_norm(x, d, &self.norm1);
        let q = self.q.apply(&normed, n);
        let k = self.k.apply(&normed, n);
        let v = self.v.apply(&normed, n);
        let mut attended = vec![0f32; n * d];
        par_rows(&mut attended, d, &|i, dst| {
            let keys = if causal { i + 1 } else { n };
            let mut scores = vec![0f32; keys];
            for h in 0..heads {
                let qi = &q[i * d + h * hd..i * d + (h + 1) * hd];
                let mut max = f32::NEG_INFINITY;
                for (j, s) in scores.iter_mut().enumerate() {
                    let kj = &k[j * d + h * hd..j * d + (h + 1) * hd];
                    let dot: f32 = qi.iter().zip(kj).map(|(a, b)| a * b).sum();
                    *s = dot * scale;
                    max = max.max(*s);
                }
                let mut sum = 0f32;
                for s in scores.iter_mut() {
                    *s = (*s - max).exp();
                    sum += *s;
                }
                let inv = 1.0 / sum.max(1e-30);
                let out = &mut dst[h * hd..(h + 1) * hd];
                for (j, s) in scores.iter().enumerate() {
                    let p = *s * inv;
                    let vj = &v[j * d + h * hd..j * d + (h + 1) * hd];
                    for (o, val) in out.iter_mut().zip(vj) {
                        *o += p * val;
                    }
                }
            }
        });
        let proj = self.o.apply(&attended, n);
        let mut h: Vec<f32> = x.iter().zip(&proj).map(|(a, b)| a + b).collect();
        let normed = layer_norm(&h, d, &self.norm2);
        let mut ff = self.ff1.apply(&normed, n);
        for v in ff.iter_mut() {
            *v = gelu_erf(*v);
        }
        let ff = self.ff2.apply(&ff, n);
        for (a, b) in h.iter_mut().zip(&ff) {
            *a += *b;
        }
        h
    }
}

struct ResBlock {
    norm: Norm,
    conv1: Conv,
    conv2: Conv,
}

struct DepthDecoder {
    context_projection: LinearW,
    /// `prior_embeddings[i]` = `[vocab_i][dim]`, i in 0..7 (semantic + c1..c6).
    priors: Vec<Vec<f32>>,
    /// `[8][dim]`.
    position: Vec<f32>,
    layers: Vec<AttnLayer>,
    norm: Norm,
    /// Seven acoustic heads `[1024][dim]`.
    heads: Vec<LinearW>,
}

pub struct RvqEncoder {
    pub cfg: RvqEncoderConfig,
    conv_in: Conv,
    blocks: Vec<ResBlock>,
    /// `[max_position_embeddings][d_model]`.
    position: Vec<f32>,
    layers: Vec<AttnLayer>,
    norm_out: Norm,
    semantic_head: LinearW,
    depth: DepthDecoder,
}

/// One window's prediction.
pub struct RvqWindowOutput {
    pub codes: Vec<[u32; 8]>,
    pub c0_topk: Vec<[u32; MUSIC3_REFERENCE_CANDIDATES]>,
    /// Post-`norm_out` hidden `[frames][d_model]`.
    pub hidden: Vec<f32>,
}

impl RvqEncoder {
    pub fn load(weights: &Path, config: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(config).map_err(|e| {
            DiffusionError::model(format!("music3 rvq encoder config {}: {e}", config.display()))
        })?;
        let cfg = RvqEncoderConfig::parse(&text)?;
        let shards = crate::music3_weights::Music3Shards::load(weights)?;
        let read = |name: &str, expect: usize| -> Result<Vec<f32>> {
            let t = shards.tensor_f32(name)?;
            if t.len() != expect {
                return Err(DiffusionError::model(format!(
                    "music3 rvq encoder {name}: {} values, expected {expect}",
                    t.len()
                )));
            }
            Ok(t)
        };
        let lin = |prefix: &str, n: usize, k: usize, bias: bool| -> Result<LinearW> {
            Ok(LinearW {
                w: read(&format!("{prefix}.weight"), n * k)?,
                b: if bias {
                    read(&format!("{prefix}.bias"), n)?
                } else {
                    Vec::new()
                },
                n,
                k,
            })
        };
        let norm = |prefix: &str, d: usize| -> Result<Norm> {
            Ok(Norm {
                g: read(&format!("{prefix}.weight"), d)?,
                b: read(&format!("{prefix}.bias"), d)?,
            })
        };
        let conv = |prefix: &str, out_ch: usize, in_ch: usize, k: usize, dilation: usize, padding: usize| -> Result<Conv> {
            let c = Conv {
                out_ch,
                in_ch,
                k,
                stride: 1,
                dilation,
                padding,
                w: read(&format!("{prefix}.weight"), out_ch * in_ch * k)?,
                b: read(&format!("{prefix}.bias"), out_ch)?,
            };
            c.check(prefix)?;
            Ok(c)
        };
        let layer = |prefix: &str, d: usize, ff: usize| -> Result<AttnLayer> {
            Ok(AttnLayer {
                norm1: norm(&format!("{prefix}.norm1"), d)?,
                norm2: norm(&format!("{prefix}.norm2"), d)?,
                q: lin(&format!("{prefix}.q_proj"), d, d, true)?,
                k: lin(&format!("{prefix}.k_proj"), d, d, true)?,
                v: lin(&format!("{prefix}.v_proj"), d, d, true)?,
                o: lin(&format!("{prefix}.out_proj"), d, d, true)?,
                ff1: lin(&format!("{prefix}.linear1"), ff, d, true)?,
                ff2: lin(&format!("{prefix}.linear2"), d, ff, true)?,
            })
        };
        let d = cfg.d_model;
        let conv_in = conv("conv_in", d, cfg.latent_channels, 7, 1, 3)?;
        let mut blocks = Vec::with_capacity(cfg.conv_dilations.len());
        for (i, &dilation) in cfg.conv_dilations.iter().enumerate() {
            blocks.push(ResBlock {
                norm: norm(&format!("blocks.{i}.norm"), d)?,
                conv1: conv(&format!("blocks.{i}.conv1"), d, d, 3, dilation, dilation)?,
                conv2: conv(&format!("blocks.{i}.conv2"), d, d, 1, 1, 0)?,
            });
        }
        let position = read("position", cfg.max_position_embeddings * d)?;
        let mut layers = Vec::with_capacity(cfg.num_layers);
        for i in 0..cfg.num_layers {
            layers.push(layer(&format!("transformer.{i}"), d, d * cfg.ff_mult)?);
        }
        let norm_out = norm("norm_out", d)?;
        let semantic_head = lin("heads.0", cfg.codebook_vocab_sizes[0], d, true)?;
        let dd = cfg.depth_decoder_dim;
        let mut priors = Vec::with_capacity(7);
        for (i, &vocab) in cfg.codebook_vocab_sizes[..7].iter().enumerate() {
            priors.push(read(&format!("depth_decoder.prior_embeddings.{i}.weight"), vocab * dd)?);
        }
        let mut dlayers = Vec::with_capacity(cfg.depth_decoder_layers);
        for i in 0..cfg.depth_decoder_layers {
            dlayers.push(layer(&format!("depth_decoder.layers.{i}"), dd, dd * cfg.depth_decoder_ff_mult)?);
        }
        let mut heads = Vec::with_capacity(7);
        for (i, &vocab) in cfg.codebook_vocab_sizes[1..].iter().enumerate() {
            heads.push(lin(&format!("depth_decoder.heads.{i}"), vocab, dd, true)?);
        }
        let depth = DepthDecoder {
            context_projection: lin("depth_decoder.context_projection", dd, d, false)?,
            priors,
            position: read("depth_decoder.position", 8 * dd)?,
            layers: dlayers,
            norm: norm("depth_decoder.norm", dd)?,
            heads,
        };
        Ok(Self {
            cfg,
            conv_in,
            blocks,
            position,
            layers,
            norm_out,
            semantic_head,
            depth,
        })
    }

    /// One <= `max_position_embeddings`-frame window. `latents` are
    /// time-major `[t_lat][128]`; `bounds` are the window-local latent
    /// starts of its frames (`frames + 1` entries, first = 0, last = t_lat).
    pub fn encode_window(&self, latents: &[f32], t_lat: usize, bounds: &[usize]) -> Result<RvqWindowOutput> {
        let d = self.cfg.d_model;
        let lc = self.cfg.latent_channels;
        let frames = bounds.len().saturating_sub(1);
        if frames == 0
            || frames > self.cfg.max_position_embeddings
            || latents.len() != t_lat * lc
            || bounds[0] != 0
            || bounds[frames] != t_lat
            || bounds.windows(2).any(|w| w[1] <= w[0])
        {
            return Err(DiffusionError::model(format!(
                "music3 rvq encoder window: frames={frames} t_lat={t_lat} latents={}",
                latents.len()
            )));
        }
        // Planar [128][t_lat] for the conv stem.
        let mut planar = vec![0f32; lc * t_lat];
        for t in 0..t_lat {
            for c in 0..lc {
                planar[c * t_lat + t] = latents[t * lc + c];
            }
        }
        let mut x = conv1d(
            &Plane {
                ch: lc,
                len: t_lat,
                data: planar,
            },
            &self.conv_in,
        );
        for block in &self.blocks {
            let mut r = Plane {
                ch: x.ch,
                len: x.len,
                data: x.data.clone(),
            };
            group_norm_all(&mut r, &block.norm);
            for v in r.data.iter_mut() {
                *v = gelu_erf(*v);
            }
            r = conv1d(&r, &block.conv1);
            for v in r.data.iter_mut() {
                *v = gelu_erf(*v);
            }
            r = conv1d(&r, &block.conv2);
            for (a, b) in x.data.iter_mut().zip(&r.data) {
                *a += *b;
            }
        }
        // Mean-pool each frame's latent span, add the frame position.
        let mut h = vec![0f32; frames * d];
        for f in 0..frames {
            let (s, e) = (bounds[f], bounds[f + 1]);
            let inv = 1.0 / (e - s) as f32;
            let row = &mut h[f * d..(f + 1) * d];
            for (c, v) in row.iter_mut().enumerate() {
                let src = &x.data[c * t_lat + s..c * t_lat + e];
                *v = src.iter().sum::<f32>() * inv + self.position[f * d + c];
            }
        }
        let scale = self.cfg.mup_attention_multiplier / (d / self.cfg.num_heads) as f32;
        for layer in &self.layers {
            h = layer.forward(&h, frames, d, self.cfg.num_heads, scale, false);
        }
        let hidden = layer_norm(&h, d, &self.norm_out);
        let logits = self.semantic_head.apply(&hidden, frames);
        let vocab0 = self.cfg.codebook_vocab_sizes[0];
        let mut c0_topk = Vec::with_capacity(frames);
        let mut semantic = Vec::with_capacity(frames);
        for f in 0..frames {
            let top = top_k_ids(&logits[f * vocab0..(f + 1) * vocab0]);
            semantic.push(top[0]);
            c0_topk.push(top);
        }
        let acoustic = self.decode_depth(&hidden, frames, &semantic);
        let codes = (0..frames)
            .map(|f| {
                let a = &acoustic[f];
                [semantic[f], a[0], a[1], a[2], a[3], a[4], a[5], a[6]]
            })
            .collect();
        Ok(RvqWindowOutput {
            codes,
            c0_topk,
            hidden,
        })
    }

    /// Causal depth decoder, greedy: `[ctx, prior0(c0)] -> c1`, append
    /// `prior1(c1)` -> `c2`, ... All frames of the window are decoded as one
    /// batch (the sequence dim is the RVQ depth, <= 8).
    fn decode_depth(&self, hidden: &[f32], frames: usize, semantic: &[u32]) -> Vec<[u32; 7]> {
        let dd = self.cfg.depth_decoder_dim;
        let dep = &self.depth;
        let ctx = dep.context_projection.apply(hidden, frames);
        // seq[f] = rows [depth][dd]
        let mut seq: Vec<Vec<f32>> = (0..frames)
            .map(|f| {
                let mut s = Vec::with_capacity(8 * dd);
                s.extend_from_slice(&ctx[f * dd..(f + 1) * dd]);
                let c0 = semantic[f] as usize;
                s.extend_from_slice(&dep.priors[0][c0 * dd..(c0 + 1) * dd]);
                s
            })
            .collect();
        let mut out = vec![[0u32; 7]; frames];
        let scale = 1.0 / ((dd / self.cfg.depth_decoder_heads) as f32).sqrt();
        for head in 0..7 {
            let depth = head + 2;
            // Last-position hidden per frame after the causal stack.
            let mut last = vec![0f32; frames * dd];
            for f in 0..frames {
                let mut h: Vec<f32> = seq[f]
                    .iter()
                    .enumerate()
                    .map(|(i, v)| v + dep.position[i])
                    .collect();
                for layer in &dep.layers {
                    h = layer.forward(&h, depth, dd, self.cfg.depth_decoder_heads, scale, true);
                }
                let normed = layer_norm(&h[(depth - 1) * dd..depth * dd], dd, &dep.norm);
                last[f * dd..(f + 1) * dd].copy_from_slice(&normed);
            }
            let vocab = dep.heads[head].n;
            let logits = dep.heads[head].apply(&last, frames);
            for f in 0..frames {
                let row = &logits[f * vocab..(f + 1) * vocab];
                let code = argmax(row);
                out[f][head] = code as u32;
                if head + 1 < 7 {
                    let prior = &dep.priors[head + 1];
                    seq[f].extend_from_slice(&prior[code * dd..(code + 1) * dd]);
                }
            }
        }
        out
    }
}

fn argmax(row: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in row.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best = i;
        }
    }
    best
}

/// Top-5 ids by logit, descending (ties: lower id first).
fn top_k_ids(row: &[f32]) -> [u32; MUSIC3_REFERENCE_CANDIDATES] {
    let mut best: [(f32, usize); MUSIC3_REFERENCE_CANDIDATES] =
        [(f32::NEG_INFINITY, usize::MAX); MUSIC3_REFERENCE_CANDIDATES];
    for (i, &v) in row.iter().enumerate() {
        if v <= best[MUSIC3_REFERENCE_CANDIDATES - 1].0 {
            continue;
        }
        let mut pos = MUSIC3_REFERENCE_CANDIDATES - 1;
        while pos > 0 && best[pos - 1].0 < v {
            best[pos] = best[pos - 1];
            pos -= 1;
        }
        best[pos] = (v, i);
    }
    let mut out = [0u32; MUSIC3_REFERENCE_CANDIDATES];
    for (o, (_, i)) in out.iter_mut().zip(best) {
        *o = if i == usize::MAX { 0 } else { i as u32 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_hits_the_contract_points() {
        assert_eq!(music3_reference_interval(None), 5);
        assert_eq!(music3_reference_interval(Some(0.8)), 5);
        assert_eq!(music3_reference_interval(Some(1.0)), 1);
        assert_eq!(music3_reference_interval(Some(0.0)), 10);
        assert_eq!(music3_reference_interval(Some(f32::NAN)), 5);
        assert_eq!(music3_reference_interval(Some(7.0)), 1);
        // Monotonic: tighter strength never loosens the interval.
        let mut prev = 1;
        for step in 0..=20 {
            let s = 1.0 - step as f32 / 20.0;
            let i = music3_reference_interval(Some(s));
            assert!(i >= prev, "strength {s} -> {i} < {prev}");
            prev = i;
        }
    }

    #[test]
    fn frame_latent_starts_match_the_adapter_formula() {
        let b = frame_latent_starts(4);
        assert_eq!(b, vec![0, 3, 6, 10, 13]);
        // 128 frames span exactly 441 latents (5.12 s at 86.13 Hz).
        let b = frame_latent_starts(128);
        assert_eq!(b.len(), 129);
        assert_eq!(b[128], 441);
        assert!(b.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn window_starts_tile_then_cover_the_tail() {
        assert_eq!(reference_window_starts(50, 128), vec![0]);
        assert_eq!(reference_window_starts(128, 128), vec![0]);
        assert_eq!(reference_window_starts(200, 128), vec![0, 72]);
        assert_eq!(reference_window_starts(256, 128), vec![0, 128]);
        assert_eq!(reference_window_starts(300, 128), vec![0, 128, 172]);
        // Every frame is covered by at least one window.
        for frames in [1usize, 127, 129, 255, 257, 1000] {
            let starts = reference_window_starts(frames, 128);
            let mut covered = vec![false; frames];
            for s in starts {
                for f in s..(s + 128).min(frames) {
                    covered[f] = true;
                }
            }
            assert!(covered.iter().all(|c| *c), "frames {frames}");
        }
    }

    #[test]
    fn conv_geometry_matches_torch() {
        // 'same' conv: k7 p3 keeps the length; dilated k7 d3 p9 too.
        let same = Conv { out_ch: 1, in_ch: 1, k: 7, stride: 1, dilation: 1, padding: 3, w: vec![0.0; 7], b: vec![0.0] };
        assert_eq!(same.out_len(1000), 1000);
        let dil = Conv { out_ch: 1, in_ch: 1, k: 7, stride: 1, dilation: 3, padding: 9, w: vec![0.0; 7], b: vec![0.0] };
        assert_eq!(dil.out_len(1000), 1000);
        // DAV strided downsamplers: k=2s, stride s, pad ceil(s/2) -> L/s.
        for s in DAV_STRIDES {
            let c = Conv { out_ch: 1, in_ch: 1, k: 2 * s, stride: s, dilation: 1, padding: s.div_ceil(2), w: vec![0.0; 2 * s], b: vec![0.0] };
            assert_eq!(c.out_len(512), 512 / s, "stride {s}");
        }
        // Strided conv values: identity tap at the centre picks every s-th sample.
        let s = 2;
        let mut w = vec![0.0; 4];
        w[1] = 1.0; // tap 1 with pad 1 -> src = 2t
        let c = Conv { out_ch: 1, in_ch: 1, k: 4, stride: s, dilation: 1, padding: 1, w, b: vec![0.0] };
        let x = Plane { ch: 1, len: 8, data: (0..8).map(|v| v as f32).collect() };
        let y = conv1d(&x, &c);
        assert_eq!(y.len, 4);
        assert_eq!(y.data, vec![0.0, 2.0, 4.0, 6.0]);
        // Dilated tap reaches back 3 samples with zero padding at the edge.
        let mut w = vec![0.0; 3];
        w[0] = 1.0; // tap 0, dilation 3, pad 3 -> src = t - 3
        let c = Conv { out_ch: 1, in_ch: 1, k: 3, stride: 1, dilation: 3, padding: 3, w, b: vec![1.0] };
        let y = conv1d(&x, &c);
        assert_eq!(y.data, vec![1.0, 1.0, 1.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn weight_norm_fold_and_snake() {
        // g scales v to unit norm times g per output channel.
        let g = vec![2.0, 0.5];
        let v = vec![3.0, 4.0, 0.0, 6.0, 8.0, 0.0];
        let w = fold_weight_norm(&g, &v, 2);
        assert!((w[0] - 1.2).abs() < 1e-6 && (w[1] - 1.6).abs() < 1e-6 && w[2] == 0.0);
        assert!((w[3] - 0.3).abs() < 1e-6 && (w[4] - 0.4).abs() < 1e-6);
        let mut p = Plane { ch: 1, len: 2, data: vec![0.0, 1.0] };
        snake(&mut p, &[1.0]);
        assert!(p.data[0].abs() < 1e-7);
        assert!((p.data[1] - (1.0 + (1.0f32).sin().powi(2))).abs() < 1e-5);
    }

    #[test]
    fn gelu_matches_torch_reference_values() {
        for (x, want) in [(0.0f32, 0.0f32), (1.0, 0.841_345), (-1.0, -0.158_655), (2.0, 1.954_5), (-3.0, -0.004_05)] {
            assert!((gelu_erf(x) - want).abs() < 2e-4, "gelu({x}) = {} want {want}", gelu_erf(x));
        }
    }

    #[test]
    fn norms_and_topk() {
        let x = vec![1.0, 2.0, 3.0, 4.0];
        let n = Norm { g: vec![1.0; 4], b: vec![0.0; 4] };
        let y = layer_norm(&x, 4, &n);
        assert!(y.iter().sum::<f32>().abs() < 1e-5);
        assert!((y[3] - 1.341_64).abs() < 1e-4);
        let mut p = Plane { ch: 2, len: 2, data: vec![1.0, 2.0, 3.0, 4.0] };
        group_norm_all(&mut p, &Norm { g: vec![1.0, 2.0], b: vec![0.0, 1.0] });
        // Channel 1 is scaled by 2 and shifted by 1 after the joint normalise.
        assert!((p.data[2] - (0.447_21 * 2.0 + 1.0)).abs() < 1e-3);
        let top = top_k_ids(&[0.1, 5.0, 3.0, 3.0, 9.0, -1.0, 4.0]);
        assert_eq!(top, [4, 1, 6, 2, 3]);
        assert_eq!(argmax(&[0.0, -1.0, 2.5, 2.5]), 2);
    }

    #[test]
    fn config_accepts_v4_and_rejects_other_layouts() {
        let v4 = r#"{"codebook_vocab_sizes":[16384,1024,1024,1024,1024,1024,1024,1024],"conv_dilations":[1,3,9],"d_model":1088,"depth_decoder":true,"depth_decoder_dim":512,"depth_decoder_dropout":0.1,"depth_decoder_ff_mult":4,"depth_decoder_heads":8,"depth_decoder_layers":2,"dropout":0.1,"ff_mult":4,"latent_channels":128,"max_position_embeddings":128,"mup":true,"mup_attention_multiplier":8.0,"mup_output_mult":1.0,"mup_readout_zero_init":true,"num_heads":17,"num_layers":8}"#;
        let cfg = RvqEncoderConfig::parse(v4).unwrap();
        assert_eq!(cfg.d_model, 1088);
        assert_eq!(cfg.num_heads, 17);
        assert_eq!(cfg.max_position_embeddings, 128);
        assert_eq!(cfg.codebook_vocab_sizes[0], 16_384);
        // muP attention scale for head_dim 64 happens to equal 1/sqrt(64).
        assert!((cfg.mup_attention_multiplier / 64.0 - 0.125).abs() < 1e-7);
        let v1 = v4.replace("\"depth_decoder\":true", "\"depth_decoder\":false");
        assert!(RvqEncoderConfig::parse(&v1).is_err());
        assert!(RvqEncoderConfig::parse("{}").is_err());
    }

    /// Opt-in load smoke on the real files: MAKEPAD_MUSIC3_REFERENCE_DIR must
    /// hold dav.pth + encoders/… (the registry layout). Checks every tensor
    /// is read at the shapes the port expects and that a 3 s tone encodes to
    /// in-range codes.
    #[test]
    fn real_weights_load_and_encode_when_present() {
        let Some(dir) = std::env::var_os("MAKEPAD_MUSIC3_REFERENCE_DIR") else {
            eprintln!("skipping real_weights_load_and_encode_when_present (MAKEPAD_MUSIC3_REFERENCE_DIR unset)");
            return;
        };
        let weights = Music3ReferenceWeights::resolve(Path::new(&dir)).unwrap();
        let rate = 22_050u32;
        let n = rate as usize * 3;
        let left: Vec<f32> = (0..n).map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.3).collect();
        let right: Vec<f32> = (0..n).map(|i| (i as f32 * 660.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.3).collect();
        let audio = Music3ReferenceAudio { left, right, rate };
        let enc = music3_encode_reference(&weights, &audio, &mut |_, _, _| {}, &|| false).unwrap();
        assert_eq!(enc.frames, 75);
        assert_eq!(enc.codes.len(), 75);
        assert!(enc.codes.iter().all(|c| c[0] < 16_384 && c[1..].iter().all(|v| *v < 1024)));
        assert!(enc.c0_topk.iter().all(|t| t[0] < 16_384 && t.iter().all(|v| *v < 16_384)));
        for (c, t) in enc.codes.iter().zip(&enc.c0_topk) {
            assert_eq!(c[0], t[0]);
        }
    }
}
