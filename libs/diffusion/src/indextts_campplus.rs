//! IndexTTS-2.5 CAMPPlus speaker embedder (192-d style vector), CPU f32.
//!
//! Port of `indextts/s2mel/modules/campplus/{DTDNN,layers}.py` (3D-Speaker
//! CAMPPlus, `campplus_cn_common.bin`) plus its
//! `torchaudio.compliance.kaldi.fbank` front-end:
//!
//! - [`campplus_fbank`]: kaldi fbank on the *raw* 16 kHz waveform (no 2^15
//!   scaling — matching `infer_v2.py`), num_mel_bins 80, dither 0, all other
//!   kaldi defaults (25/10 ms povey window, preemphasis 0.97, snip_edges,
//!   per-frame DC removal, log-mel floored at f32 eps), then mean-subtracted
//!   over time (`feat - feat.mean(dim=0)`).
//! - [`CampPlus`]: FCM 2D-CNN head (BasicResBlocks, freq downsampled 80->10),
//!   TDNN (320->128, k5 s2), three CAMDenseTDNN blocks (12/24/16 layers,
//!   growth 32, bn_size 4, kernel 3, dilations 1/2/2, `batchnorm-relu`) with
//!   CAM attention (global mean + 100-frame segment pooling), TransitLayers,
//!   StatsPool (mean + unbiased std), and a final Linear 1024->192 +
//!   BatchNorm1d(affine=False).
//!
//! All BatchNorms run in eval mode; each is folded at load time into a
//! per-channel `scale`/`shift` pair from `running_mean`/`running_var`
//! (eps 1e-5) and the affine weights when present — applied explicitly where
//! the reference applies BN (no folding into neighboring convs, since most
//! BNs here sit *before* their conv behind a ReLU).

use crate::error::{DiffusionError, Result};
use crate::indextts_w2v::{kaldi_mel_bank_80, kaldi_power_spectra, KALDI_BINS, KALDI_FRAME};
use crate::sa3::{par_rows, sigmoid};
use crate::torch_pth::PthStateDict;
use std::path::Path;

/// Output embedding width.
pub const CAMPPLUS_EMBEDDING: usize = 192;
const MELS: usize = 80;
const BN_EPS: f64 = 1e-5;
/// CAM segment pooling length (`CAMLayer.seg_pooling` default).
const SEG_LEN: usize = 100;

// ---------------------------------------------------------------------------
// kaldi fbank front-end.
// ---------------------------------------------------------------------------

/// `torchaudio.compliance.kaldi.fbank(num_mel_bins=80, dither=0,
/// sample_frequency=16000)` on the raw waveform, then `feat - feat.mean(0)`.
/// Returns `(frames * 80 row-major [t][mel], frames)`.
pub fn campplus_fbank(audio_16k: &[f32]) -> Result<(Vec<f32>, usize)> {
    if audio_16k.len() < KALDI_FRAME {
        return Err(DiffusionError::model(format!(
            "campplus_fbank: need at least {KALDI_FRAME} samples, got {}",
            audio_16k.len()
        )));
    }
    let spec = kaldi_power_spectra(audio_16k, 1.0);
    let frames = spec.len() / KALDI_BINS;
    let bank = kaldi_mel_bank_80();
    let floor = f32::EPSILON as f64; // torch.finfo(torch.float).eps
    let mut mel = vec![0f32; frames * MELS];
    for t in 0..frames {
        let power = &spec[t * KALDI_BINS..(t + 1) * KALDI_BINS];
        for m in 0..MELS {
            let filt = &bank[m * KALDI_BINS..(m + 1) * KALDI_BINS];
            let mut acc = 0f64;
            for (w, p) in filt.iter().zip(power) {
                acc += w * p;
            }
            mel[t * MELS + m] = acc.max(floor).ln() as f32;
        }
    }
    // Column (per-mel-bin) mean subtraction over time.
    for m in 0..MELS {
        let mut sum = 0f64;
        for t in 0..frames {
            sum += mel[t * MELS + m] as f64;
        }
        let mean = (sum / frames as f64) as f32;
        for t in 0..frames {
            mel[t * MELS + m] -= mean;
        }
    }
    Ok((mel, frames))
}

// ---------------------------------------------------------------------------
// Eval-mode BatchNorm folded to per-channel affine.
// ---------------------------------------------------------------------------

struct Bn {
    scale: Vec<f32>,
    shift: Vec<f32>,
}

impl Bn {
    fn load(sd: &mut PthStateDict, prefix: &str, affine: bool) -> Result<Self> {
        let mean = sd.f32(&format!("{prefix}.running_mean"))?;
        let var = sd.f32(&format!("{prefix}.running_var"))?;
        let (w, b) = if affine {
            (
                Some(sd.f32(&format!("{prefix}.weight"))?),
                Some(sd.f32(&format!("{prefix}.bias"))?),
            )
        } else {
            (None, None)
        };
        let mut scale = vec![0f32; mean.len()];
        let mut shift = vec![0f32; mean.len()];
        for c in 0..mean.len() {
            let g = w.as_ref().map_or(1.0, |w| w[c]);
            let s = (g as f64 / (var[c] as f64 + BN_EPS).sqrt()) as f32;
            scale[c] = s;
            shift[c] = b.as_ref().map_or(0.0, |b| b[c]) - mean[c] * s;
        }
        Ok(Self { scale, shift })
    }

    /// Applies BN (+ optional ReLU) in place; `x` is `(channels, plane)`.
    fn forward(&self, x: &mut [f32], plane: usize, relu: bool) {
        debug_assert_eq!(x.len(), self.scale.len() * plane);
        for (c, row) in x.chunks_mut(plane).enumerate() {
            let (s, o) = (self.scale[c], self.shift[c]);
            for v in row.iter_mut() {
                let y = *v * s + o;
                *v = if relu && y < 0.0 { 0.0 } else { y };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conv primitives ((ch, h, w) / (ch, t) channel-major planes).
// ---------------------------------------------------------------------------

/// 2D conv, square kernel, stride `(sh, 1)`, equal padding on both axes.
struct Conv2d {
    w: Vec<f32>, // out_ch x in_ch x k x k
    out_ch: usize,
    in_ch: usize,
    k: usize,
    sh: usize,
    pad: usize,
}

impl Conv2d {
    fn load(
        sd: &mut PthStateDict,
        name: &str,
        out_ch: usize,
        in_ch: usize,
        k: usize,
        sh: usize,
        pad: usize,
    ) -> Result<Self> {
        Ok(Self {
            w: sd.f32_shaped(name, &[out_ch, in_ch, k, k])?,
            out_ch,
            in_ch,
            k,
            sh,
            pad,
        })
    }

    /// `x` is `(in_ch, h, w)`; returns `((out_ch, oh, w'), oh)` where
    /// `w' = w + 2*pad - k + 1` (always `w` for the shapes used here).
    fn forward(&self, x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize, usize) {
        debug_assert_eq!(x.len(), self.in_ch * h * w);
        let oh = (h + 2 * self.pad - self.k) / self.sh + 1;
        let ow = w + 2 * self.pad - self.k + 1;
        let mut out = vec![0f32; self.out_ch * oh * ow];
        par_rows(&mut out, oh * ow, &|o, out_plane| {
            for ci in 0..self.in_ch {
                let in_plane = &x[ci * h * w..(ci + 1) * h * w];
                let w_base = &self.w[(o * self.in_ch + ci) * self.k * self.k
                    ..(o * self.in_ch + ci + 1) * self.k * self.k];
                for oy in 0..oh {
                    for ky in 0..self.k {
                        let iy = (oy * self.sh + ky) as isize - self.pad as isize;
                        if iy < 0 || iy >= h as isize {
                            continue;
                        }
                        let in_row = &in_plane[iy as usize * w..(iy as usize + 1) * w];
                        let out_row = &mut out_plane[oy * ow..(oy + 1) * ow];
                        for kx in 0..self.k {
                            let wt = w_base[ky * self.k + kx];
                            if wt == 0.0 {
                                continue;
                            }
                            let off = kx as isize - self.pad as isize;
                            let x_start = (-off).max(0) as usize;
                            let x_end = ((w as isize - off).min(ow as isize)).max(0) as usize;
                            for ox in x_start..x_end {
                                out_row[ox] += wt * in_row[(ox as isize + off) as usize];
                            }
                        }
                    }
                }
            }
        });
        (out, oh, ow)
    }
}

/// 1D conv over `(ch, t)` planes.
struct Conv1d {
    w: Vec<f32>, // out_ch x in_ch x k
    b: Option<Vec<f32>>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    stride: usize,
    pad: usize,
    dil: usize,
}

impl Conv1d {
    #[allow(clippy::too_many_arguments)]
    fn load(
        sd: &mut PthStateDict,
        name: &str,
        bias: bool,
        out_ch: usize,
        in_ch: usize,
        k: usize,
        stride: usize,
        pad: usize,
        dil: usize,
    ) -> Result<Self> {
        Ok(Self {
            w: sd.f32_shaped(&format!("{name}.weight"), &[out_ch, in_ch, k])?,
            b: if bias {
                Some(sd.f32(&format!("{name}.bias"))?)
            } else {
                None
            },
            out_ch,
            in_ch,
            k,
            stride,
            pad,
            dil,
        })
    }

    fn out_len(&self, t: usize) -> usize {
        (t + 2 * self.pad - self.dil * (self.k - 1) - 1) / self.stride + 1
    }

    fn forward(&self, x: &[f32], t: usize) -> (Vec<f32>, usize) {
        debug_assert_eq!(x.len(), self.in_ch * t);
        let ot = self.out_len(t);
        let mut out = vec![0f32; self.out_ch * ot];
        par_rows(&mut out, ot, &|o, out_row| {
            out_row.fill(self.b.as_ref().map_or(0.0, |b| b[o]));
            for ci in 0..self.in_ch {
                let in_row = &x[ci * t..(ci + 1) * t];
                let w_base = &self.w[(o * self.in_ch + ci) * self.k..(o * self.in_ch + ci + 1) * self.k];
                for (tap, &wt) in w_base.iter().enumerate() {
                    if wt == 0.0 {
                        continue;
                    }
                    let off = (tap * self.dil) as isize - self.pad as isize;
                    for (ot_idx, dst) in out_row.iter_mut().enumerate() {
                        let src = ot_idx as isize * self.stride as isize + off;
                        if src >= 0 && (src as usize) < t {
                            *dst += wt * in_row[src as usize];
                        }
                    }
                }
            }
        });
        (out, ot)
    }
}

// ---------------------------------------------------------------------------
// FCM head.
// ---------------------------------------------------------------------------

struct ResBlock {
    conv1: Conv2d,
    bn1: Bn,
    conv2: Conv2d,
    bn2: Bn,
    shortcut: Option<(Conv2d, Bn)>,
}

impl ResBlock {
    fn load(sd: &mut PthStateDict, prefix: &str, stride: usize) -> Result<Self> {
        let shortcut = if stride != 1 {
            Some((
                Conv2d::load(sd, &format!("{prefix}.shortcut.0.weight"), 32, 32, 1, stride, 0)?,
                Bn::load(sd, &format!("{prefix}.shortcut.1"), true)?,
            ))
        } else {
            None
        };
        Ok(Self {
            conv1: Conv2d::load(sd, &format!("{prefix}.conv1.weight"), 32, 32, 3, stride, 1)?,
            bn1: Bn::load(sd, &format!("{prefix}.bn1"), true)?,
            conv2: Conv2d::load(sd, &format!("{prefix}.conv2.weight"), 32, 32, 3, 1, 1)?,
            bn2: Bn::load(sd, &format!("{prefix}.bn2"), true)?,
            shortcut,
        })
    }

    fn forward(&self, x: &[f32], h: usize, w: usize) -> (Vec<f32>, usize) {
        let (mut y, oh, _) = self.conv1.forward(x, h, w);
        self.bn1.forward(&mut y, oh * w, true);
        let (mut y, oh2, _) = self.conv2.forward(&y, oh, w);
        self.bn2.forward(&mut y, oh2 * w, false);
        let short = match &self.shortcut {
            Some((conv, bn)) => {
                let (mut s, sh, _) = conv.forward(x, h, w);
                debug_assert_eq!(sh, oh2);
                bn.forward(&mut s, sh * w, false);
                s
            }
            None => x.to_vec(),
        };
        for (v, s) in y.iter_mut().zip(&short) {
            *v = (*v + s).max(0.0);
        }
        (y, oh2)
    }
}

struct Fcm {
    conv1: Conv2d,
    bn1: Bn,
    layer1: [ResBlock; 2],
    layer2: [ResBlock; 2],
    conv2: Conv2d,
    bn2: Bn,
}

impl Fcm {
    fn load(sd: &mut PthStateDict) -> Result<Self> {
        Ok(Self {
            conv1: Conv2d::load(sd, "head.conv1.weight", 32, 1, 3, 1, 1)?,
            bn1: Bn::load(sd, "head.bn1", true)?,
            layer1: [
                ResBlock::load(sd, "head.layer1.0", 2)?,
                ResBlock::load(sd, "head.layer1.1", 1)?,
            ],
            layer2: [
                ResBlock::load(sd, "head.layer2.0", 2)?,
                ResBlock::load(sd, "head.layer2.1", 1)?,
            ],
            conv2: Conv2d::load(sd, "head.conv2.weight", 32, 32, 3, 2, 1)?,
            bn2: Bn::load(sd, "head.bn2", true)?,
        })
    }

    /// `(1, 80, t)` mel plane -> `(320, t)` (32 channels x 10 freq rows).
    fn forward(&self, mel_plane: &[f32], t: usize) -> Vec<f32> {
        let (mut x, mut h, _) = self.conv1.forward(mel_plane, MELS, t);
        self.bn1.forward(&mut x, h * t, true);
        for block in &self.layer1 {
            (x, h) = block.forward(&x, h, t);
        }
        for block in &self.layer2 {
            (x, h) = block.forward(&x, h, t);
        }
        let (mut x, h2, _) = self.conv2.forward(&x, h, t);
        self.bn2.forward(&mut x, h2 * t, true);
        debug_assert_eq!(h2, 10);
        // (32, 10, t) reshape -> (320, t): row index = c*10 + freq (C order).
        x
    }
}

// ---------------------------------------------------------------------------
// CAM dense TDNN.
// ---------------------------------------------------------------------------

/// Context vector for the CAM gate: per-channel global mean over time plus
/// 100-frame segment means expanded back to `t` (avg_pool1d ceil_mode; the
/// trailing partial segment averages its actual length).
fn cam_context(x: &[f32], ch: usize, t: usize) -> Vec<f32> {
    let segs = t.div_ceil(SEG_LEN);
    let mut ctx = vec![0f32; ch * t];
    for c in 0..ch {
        let row = &x[c * t..(c + 1) * t];
        let mean = row.iter().map(|&v| v as f64).sum::<f64>() / t as f64;
        let ctx_row = &mut ctx[c * t..(c + 1) * t];
        for s in 0..segs {
            let start = s * SEG_LEN;
            let end = (start + SEG_LEN).min(t);
            let seg_mean = row[start..end].iter().map(|&v| v as f64).sum::<f64>()
                / (end - start) as f64;
            let value = (mean + seg_mean) as f32;
            ctx_row[start..end].fill(value);
        }
    }
    ctx
}

struct CamLayer {
    local: Conv1d, // bn_ch -> out_ch, k3, pad=dil, no bias
    lin1: Conv1d,  // bn_ch -> bn_ch/2, k1, bias
    lin2: Conv1d,  // bn_ch/2 -> out_ch, k1, bias
}

impl CamLayer {
    fn forward(&self, x: &[f32], t: usize) -> Vec<f32> {
        let (mut y, _) = self.local.forward(x, t);
        let ctx = cam_context(x, self.local.in_ch, t);
        let (mut g, _) = self.lin1.forward(&ctx, t);
        for v in &mut g {
            *v = v.max(0.0);
        }
        let (gate, _) = self.lin2.forward(&g, t);
        for (v, m) in y.iter_mut().zip(&gate) {
            *v *= sigmoid(*m);
        }
        y
    }
}

struct DenseTdnnLayer {
    bn1: Bn,
    lin1: Conv1d, // in_ch -> 128, k1, no bias
    bn2: Bn,
    cam: CamLayer,
}

impl DenseTdnnLayer {
    fn load(sd: &mut PthStateDict, prefix: &str, in_ch: usize, dil: usize) -> Result<Self> {
        const GROWTH: usize = 32;
        const BN_CH: usize = 128; // bn_size 4 * growth 32
        Ok(Self {
            bn1: Bn::load(sd, &format!("{prefix}.nonlinear1.batchnorm"), true)?,
            lin1: Conv1d::load(sd, &format!("{prefix}.linear1"), false, BN_CH, in_ch, 1, 1, 0, 1)?,
            bn2: Bn::load(sd, &format!("{prefix}.nonlinear2.batchnorm"), true)?,
            cam: CamLayer {
                local: Conv1d::load(
                    sd,
                    &format!("{prefix}.cam_layer.linear_local"),
                    false,
                    GROWTH,
                    BN_CH,
                    3,
                    1,
                    dil,
                    dil,
                )?,
                lin1: Conv1d::load(
                    sd,
                    &format!("{prefix}.cam_layer.linear1"),
                    true,
                    BN_CH / 2,
                    BN_CH,
                    1,
                    1,
                    0,
                    1,
                )?,
                lin2: Conv1d::load(
                    sd,
                    &format!("{prefix}.cam_layer.linear2"),
                    true,
                    GROWTH,
                    BN_CH / 2,
                    1,
                    1,
                    0,
                    1,
                )?,
            },
        })
    }

    /// `bn-relu -> 1x1 -> bn-relu -> CAM conv`; returns the 32-channel growth.
    fn forward(&self, x: &[f32], t: usize) -> Vec<f32> {
        let mut h = x.to_vec();
        self.bn1.forward(&mut h, t, true);
        let (mut h, _) = self.lin1.forward(&h, t);
        self.bn2.forward(&mut h, t, true);
        self.cam.forward(&h, t)
    }
}

struct Transit {
    bn: Bn,
    linear: Conv1d, // in -> in/2, k1, no bias
}

// ---------------------------------------------------------------------------
// CAMPPlus.
// ---------------------------------------------------------------------------

/// The full DTDNN speaker embedder.
pub struct CampPlus {
    head: Fcm,
    tdnn: Conv1d, // 320 -> 128, k5 s2 p2
    tdnn_bn: Bn,
    blocks: Vec<Vec<DenseTdnnLayer>>, // 12 / 24 / 16 layers
    transits: Vec<Transit>,
    out_bn: Bn,
    dense: Conv1d, // 1024 -> 192, k1, no bias
    dense_bn: Bn,  // affine=False
}

impl CampPlus {
    /// Loads `<checkpoints_dir>/hf_cache/campplus_cn_common.bin`.
    pub fn load(checkpoints_dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_path(checkpoints_dir.as_ref().join("hf_cache/campplus_cn_common.bin"))
    }

    /// Path-explicit load (service cache layout differs from the reference
    /// checkout).
    pub fn load_path(path: impl AsRef<Path>) -> Result<Self> {
        let mut sd = PthStateDict::load_nested(path.as_ref())?;
        let sd = &mut sd;
        let head = Fcm::load(sd)?;
        let tdnn = Conv1d::load(sd, "xvector.tdnn.linear", false, 128, 320, 5, 2, 2, 1)?;
        let tdnn_bn = Bn::load(sd, "xvector.tdnn.nonlinear.batchnorm", true)?;
        let mut blocks = Vec::new();
        let mut transits = Vec::new();
        let mut channels = 128usize;
        for (b, (num_layers, dil)) in [(12usize, 1usize), (24, 2), (16, 2)].iter().enumerate() {
            let mut layers = Vec::with_capacity(*num_layers);
            for i in 0..*num_layers {
                layers.push(DenseTdnnLayer::load(
                    sd,
                    &format!("xvector.block{}.tdnnd{}", b + 1, i + 1),
                    channels + i * 32,
                    *dil,
                )?);
            }
            channels += num_layers * 32;
            blocks.push(layers);
            transits.push(Transit {
                bn: Bn::load(sd, &format!("xvector.transit{}.nonlinear.batchnorm", b + 1), true)?,
                linear: Conv1d::load(
                    sd,
                    &format!("xvector.transit{}.linear", b + 1),
                    false,
                    channels / 2,
                    channels,
                    1,
                    1,
                    0,
                    1,
                )?,
            });
            channels /= 2;
        }
        debug_assert_eq!(channels, 512);
        Ok(Self {
            head,
            tdnn,
            tdnn_bn,
            blocks,
            transits,
            out_bn: Bn::load(sd, "xvector.out_nonlinear.batchnorm", true)?,
            dense: Conv1d::load(sd, "xvector.dense.linear", false, CAMPPLUS_EMBEDDING, 1024, 1, 1, 0, 1)?,
            dense_bn: Bn::load(sd, "xvector.dense.nonlinear.batchnorm", false)?,
        })
    }

    /// Mean-subtracted fbank `(frames * 80 row-major [t][mel])` -> 192-d
    /// style vector.
    pub fn embed(&self, fbank: &[f32], frames: usize) -> Vec<f32> {
        debug_assert_eq!(fbank.len(), frames * MELS);
        // (t, 80) -> the (1, 80, t) input plane.
        let mut plane = vec![0f32; MELS * frames];
        for t in 0..frames {
            for m in 0..MELS {
                plane[m * frames + t] = fbank[t * MELS + m];
            }
        }
        let x = self.head.forward(&plane, frames);
        let (mut x, mut t) = self.tdnn.forward(&x, frames);
        self.tdnn_bn.forward(&mut x, t, true);
        for (layers, transit) in self.blocks.iter().zip(&self.transits) {
            for layer in layers {
                let growth = layer.forward(&x, t);
                x.extend_from_slice(&growth); // channel concat
            }
            transit.bn.forward(&mut x, t, true);
            (x, t) = transit.linear.forward(&x, t);
        }
        self.out_bn.forward(&mut x, t, true);
        // StatsPool: per-channel mean + unbiased std over time.
        let ch = x.len() / t;
        let mut stats = vec![0f32; ch * 2];
        for c in 0..ch {
            let row = &x[c * t..(c + 1) * t];
            let mean = row.iter().map(|&v| v as f64).sum::<f64>() / t as f64;
            let sq: f64 = row.iter().map(|&v| (v as f64 - mean) * (v as f64 - mean)).sum();
            let std = if t > 1 { (sq / (t as f64 - 1.0)).sqrt() } else { 0.0 };
            stats[c] = mean as f32;
            stats[ch + c] = std as f32;
        }
        // DenseLayer: 1x1 conv on the (1024, 1) column + BatchNorm(affine=False).
        let (mut emb, _) = self.dense.forward(&stats, 1);
        self.dense_bn.forward(&mut emb, 1, false);
        emb
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cam_context_segments_and_mean() {
        // One channel, t = 5, seg_len 100 -> single (partial) segment.
        let x = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
        let ctx = cam_context(&x, 1, 5);
        // mean 3.0 + segment mean 3.0 = 6.0 everywhere.
        assert!(ctx.iter().all(|&v| (v - 6.0).abs() < 1e-6), "{ctx:?}");
        // Two channels, t = 150: seg 0 covers 0..100, seg 1 covers 100..150.
        let mut x = vec![0f32; 2 * 150];
        for t in 0..150 {
            x[t] = if t < 100 { 1.0 } else { 3.0 }; // ch0: mean = 1.6667
            x[150 + t] = t as f32;
        }
        let ctx = cam_context(&x, 2, 150);
        let mean0 = (100.0 * 1.0 + 50.0 * 3.0) / 150.0;
        assert!((ctx[0] - (mean0 + 1.0)).abs() < 1e-5);
        assert!((ctx[149] - (mean0 + 3.0)).abs() < 1e-5);
        let mean1 = (0..150).sum::<usize>() as f32 / 150.0;
        assert!((ctx[150] - (mean1 + 49.5)).abs() < 1e-4); // seg 0: mean 0..100
        assert!((ctx[150 + 149] - (mean1 + 124.5)).abs() < 1e-4); // seg 1: 100..150
    }

    #[test]
    fn conv1d_matches_manual_dilated() {
        // 1 -> 1 channel, k3, dilation 2, pad 2, stride 1 on t=6.
        let conv = Conv1d {
            w: vec![1.0, 10.0, 100.0],
            b: None,
            out_ch: 1,
            in_ch: 1,
            k: 3,
            stride: 1,
            pad: 2,
            dil: 2,
        };
        let x = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let (y, ot) = conv.forward(&x, 6);
        assert_eq!(ot, 6);
        // y[t] = x[t-2] + 10*x[t] + 100*x[t+2] (zero outside).
        let expect = [
            10.0 * 1.0 + 100.0 * 3.0,
            10.0 * 2.0 + 100.0 * 4.0,
            1.0 + 10.0 * 3.0 + 100.0 * 5.0,
            2.0 + 10.0 * 4.0 + 100.0 * 6.0,
            3.0 + 10.0 * 5.0,
            4.0 + 10.0 * 6.0,
        ];
        for (a, e) in y.iter().zip(expect) {
            assert!((a - e).abs() < 1e-5, "{y:?}");
        }
    }

    #[test]
    fn conv1d_strided_length() {
        // The tdnn shape: k5 s2 p2 on t=301 -> 151.
        let conv = Conv1d {
            w: vec![0.0; 5],
            b: None,
            out_ch: 1,
            in_ch: 1,
            k: 5,
            stride: 2,
            pad: 2,
            dil: 1,
        };
        assert_eq!(conv.out_len(301), 151);
        assert_eq!(conv.out_len(300), 150);
    }

    #[test]
    fn conv2d_identity_kernel() {
        // 1 -> 1 channel 3x3 with a centered delta kernel reproduces the input.
        let mut w = vec![0f32; 9];
        w[4] = 1.0;
        let conv = Conv2d {
            w,
            out_ch: 1,
            in_ch: 1,
            k: 3,
            sh: 1,
            pad: 1,
        };
        let x: Vec<f32> = (0..12).map(|v| v as f32).collect(); // 3 x 4
        let (y, oh, ow) = conv.forward(&x, 3, 4);
        assert_eq!((oh, ow), (3, 4));
        assert_eq!(x, y);
    }

    #[test]
    fn stats_pool_unbiased() {
        // Mirror of torch.std(unbiased=True) on [1,2,3,5].
        let vals = [1.0f64, 2.0, 3.0, 5.0];
        let mean = vals.iter().sum::<f64>() / 4.0;
        let sq: f64 = vals.iter().map(|v| (v - mean) * (v - mean)).sum();
        let std = (sq / 3.0).sqrt();
        assert!((std - 1.7078).abs() < 1e-4);
    }

    /// Oracle-backed fbank: audio_16k.npy -> campplus_fbank.npy (skips when
    /// the reference checkout is absent).
    #[test]
    fn fbank_matches_oracle_dump() {
        let dir = crate::indextts::reference_dumps_dir();
        let audio_path = dir.join("audio_16k.npy");
        if !audio_path.is_file() {
            eprintln!("skipping fbank_matches_oracle_dump: {audio_path:?} missing");
            return;
        }
        let audio = read_npy_f32(&audio_path);
        let reference = read_npy_f32(&dir.join("campplus_fbank.npy"));
        let (ours, frames) = campplus_fbank(&audio).unwrap();
        assert_eq!(ours.len(), reference.len());
        assert_eq!(frames * MELS, reference.len());
        let mut max_abs = 0f32;
        for (a, b) in ours.iter().zip(&reference) {
            max_abs = max_abs.max((a - b).abs());
        }
        // torchaudio computes this path in f32 (f32 FFT + f32 mel bank); ours
        // runs in f64 — near-floor log-mels differ by up to ~5e-4 between the
        // two precisions. The stage gate is 2e-3.
        assert!(max_abs < 1e-3, "fbank max abs diff {max_abs}");
    }

    /// Oracle-backed full embedder: campplus_fbank.npy -> campplus_style.npy
    /// (weights are 28 MB, quick enough for the unit suite; skips when the
    /// reference checkout is absent).
    #[test]
    fn style_matches_oracle_dump() {
        let ckpt = crate::indextts::reference_checkpoints_dir();
        let dumps = crate::indextts::reference_dumps_dir();
        if !ckpt.join("hf_cache/campplus_cn_common.bin").is_file()
            || !dumps.join("campplus_fbank.npy").is_file()
        {
            eprintln!("skipping style_matches_oracle_dump: reference checkout missing");
            return;
        }
        let fbank = read_npy_f32(&dumps.join("campplus_fbank.npy"));
        let reference = read_npy_f32(&dumps.join("campplus_style.npy"));
        let model = CampPlus::load(&ckpt).unwrap();
        let style = model.embed(&fbank, fbank.len() / MELS);
        assert_eq!(style.len(), reference.len());
        let mut max_abs = 0f32;
        let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
        for (&a, &b) in style.iter().zip(&reference) {
            max_abs = max_abs.max((a - b).abs());
            dot += a as f64 * b as f64;
            na += a as f64 * a as f64;
            nb += b as f64 * b as f64;
        }
        let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
        assert!(cos >= 0.999, "style cosine {cos}");
        assert!(max_abs <= 2e-3, "style max abs diff {max_abs}");
    }

    /// Minimal f32 .npy reader for the oracle tests (little-endian '<f4').
    fn read_npy_f32(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let header = String::from_utf8_lossy(&bytes[10..10 + header_len]).to_string();
        assert!(header.contains("<f4"), "expected f32 npy: {header}");
        assert!(!header.contains("'fortran_order': True"));
        bytes[10 + header_len..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }
}
