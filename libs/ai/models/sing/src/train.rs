//! Training: batches, losses, AdamW (host or device), checkpoints, and the
//! synthetic singer that T0 learns (a known answer, so a run proves the whole
//! pipeline learns).

use crate::acoustic::{self, AcousticConfig, MEL_MEAN, MEL_STD};
use crate::data::{Item, Kind};
use crate::dsp::{self, hz_to_midi, mel_filters, Rng, Stft, BINS, HOP, N_MEL, SR};
use crate::layers::linear;
use crate::nn::{GBuf, Graph, Id, Params, RowIndex, Tensor};
use crate::phonemes as ph;
use crate::score::{self, Frames, PitchStyle, FEATS};
use crate::vocoder::{self, VocoderConfig};
use crate::weights::{self, Dtype};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Batches
// ---------------------------------------------------------------------------

/// A vocoder batch: `b` crops of `t` frames.
pub struct VocBatch {
    pub b: usize,
    pub t: usize,
    /// Target audio, t*HOP samples per item.
    pub audio: Vec<f32>,
    pub f0: Vec<f32>,
    /// Content limit per item (Hz).
    pub band: Vec<f32>,
    pub src: vocoder::SourceCtl,
}

/// An acoustic batch: `b` items of up to `n` tokens and `t` frames.
pub struct AcBatch {
    pub b: usize,
    pub n: usize,
    pub t: usize,
    pub tokens: Vec<u8>,
    pub tok_lens: Vec<u32>,
    pub singers: Vec<usize>,
    /// Encoder row of every frame (item-major, padded frames point at a pad token).
    pub frame_idx: Vec<usize>,
    pub frame_lens: Vec<u32>,
    pub note_feats: Vec<f32>,
    pub f0: Vec<f32>,
    pub notes: Vec<f32>,
    /// Frames per token (0 on padding).
    pub dur: Vec<f32>,
    /// Target audio, t*HOP samples per item (zero past the item's frames).
    pub audio: Vec<f32>,
    pub band: Vec<f32>,
    /// No durations (speech): align tokens to frames by MAS each step.
    pub align: bool,
}

/// A frame-aligned training example (tokens + frames per token).
#[derive(Clone)]
pub struct Aligned {
    pub tokens: Vec<u8>,
    pub dur: Vec<usize>,
    pub notes: Vec<f32>,
    pub f0: Vec<f32>,
    pub vel: Vec<f32>,
    pub audio: Vec<f32>,
    pub singer: usize,
    pub band: f32,
}

impl Aligned {
    pub fn frames(&self) -> usize {
        self.f0.len()
    }

    /// A sustained-vowel sung item: SP, the vowel over the voiced span, SP.
    pub fn from_vowel_item(it: &Item) -> Option<Aligned> {
        if it.kind != Kind::Sung || it.tokens.len() != 3 {
            return None;
        }
        let nf = it.frames();
        let first = it.f0.iter().position(|v| *v > 0.0)?;
        let last = nf - 1 - it.f0.iter().rev().position(|v| *v > 0.0)?;
        if last <= first + 5 {
            return None;
        }
        Some(Aligned {
            tokens: it.tokens.clone(),
            dur: vec![first.max(1), last + 1 - first, (nf - 1 - last).max(1)],
            notes: it.notes.clone(),
            f0: it.f0.clone(),
            vel: vec![0.8; nf],
            audio: it.audio_f32(),
            singer: it.speaker as usize,
            band: it.band_hz,
        })
        .map(|mut a| {
            // Keep the durations summing to the frame count.
            let s: usize = a.dur.iter().sum();
            if s > nf {
                a.dur[2] = a.dur[2].saturating_sub(s - nf).max(1);
                let s2: usize = a.dur.iter().sum();
                a.dur[1] -= s2.saturating_sub(nf);
            } else if s < nf {
                a.dur[2] += nf - s;
            }
            a
        })
    }

    /// A speech item: tokens without durations (aligned by MAS in training),
    /// no notes. None when longer than `max_frames` or with too few frames.
    pub fn from_speech_item(it: &Item, max_frames: usize) -> Option<Aligned> {
        if it.kind != Kind::Speech || it.frames() > max_frames || it.frames() < 2 * it.tokens.len() {
            return None;
        }
        let nf = it.frames();
        Some(Aligned {
            tokens: it.tokens.clone(),
            dur: Vec::new(),
            notes: vec![0.0; nf],
            f0: it.f0.clone(),
            vel: vec![0.0; nf],
            audio: it.audio_f32(),
            singer: it.speaker as usize,
            band: it.band_hz,
        })
    }

    /// A window of whole tokens covering at most `max_frames` frames.
    pub fn crop(&self, max_frames: usize, rng: &mut Rng) -> Aligned {
        if self.frames() <= max_frames || self.dur.is_empty() {
            return self.clone();
        }
        let starts: Vec<usize> = self.dur.iter().scan(0, |acc, d| {
            let s = *acc;
            *acc += d;
            Some(s)
        }).collect();
        for _ in 0..16 {
            let a = rng.below(self.tokens.len());
            let mut b = a;
            let mut frames = 0;
            while b < self.tokens.len() && frames + self.dur[b] <= max_frames {
                frames += self.dur[b];
                b += 1;
            }
            if b > a {
                let f0 = starts[a];
                let f1 = f0 + frames;
                return Aligned {
                    tokens: self.tokens[a..b].to_vec(),
                    dur: self.dur[a..b].to_vec(),
                    notes: self.notes[f0..f1].to_vec(),
                    f0: self.f0[f0..f1].to_vec(),
                    vel: self.vel[f0..f1].to_vec(),
                    audio: self.audio[f0 * HOP..(f1 * HOP).min(self.audio.len())].to_vec(),
                    singer: self.singer,
                    band: self.band,
                };
            }
        }
        // A single token longer than the window: cut it.
        let mut c = self.clone();
        c.tokens.truncate(1);
        c.dur = vec![max_frames];
        c.notes.truncate(max_frames);
        c.f0.truncate(max_frames);
        c.vel.truncate(max_frames);
        c.audio.truncate(max_frames * HOP);
        c
    }
}

pub fn ac_batch(items: &[Aligned]) -> AcBatch {
    let b = items.len();
    let n = items.iter().map(|a| a.tokens.len()).max().unwrap_or(1) + 1; // + a pad row
    let t = items.iter().map(|a| a.frames()).max().unwrap_or(1);
    let mut out = AcBatch {
        b,
        n,
        t,
        tokens: vec![ph::PAD; b * n],
        tok_lens: Vec::new(),
        singers: Vec::new(),
        frame_idx: vec![0; b * t],
        frame_lens: Vec::new(),
        note_feats: vec![0.0; b * t * FEATS],
        f0: vec![0.0; b * t],
        notes: vec![0.0; b * t],
        dur: vec![0.0; b * n],
        audio: vec![0.0; b * t * HOP],
        band: Vec::new(),
        align: items.iter().any(|a| a.dur.is_empty()),
    };
    for (i, a) in items.iter().enumerate() {
        out.tok_lens.push(a.tokens.len() as u32);
        out.frame_lens.push(a.frames() as u32);
        out.singers.push(a.singer);
        out.band.push(a.band);
        for (k, tk) in a.tokens.iter().enumerate() {
            out.tokens[i * n + k] = *tk;
            out.dur[i * n + k] = a.dur.get(k).copied().unwrap_or(0) as f32;
        }
        // Frames: rebuild a Frames view for the note features.
        let mut f = Frames { tokens: a.tokens.clone(), token_frames: a.dur.clone(), ..Default::default() };
        if a.dur.is_empty() {
            // Unaligned (speech): positions unknown; every frame points at token 0 for now.
            f.token_of_frame = vec![0; a.frames()];
            f.pos = vec![0.0; a.frames()];
        }
        for (k, d) in a.dur.iter().enumerate() {
            for q in 0..*d {
                f.token_of_frame.push(k);
                f.pos.push(if *d > 1 { q as f32 / (*d - 1) as f32 } else { 0.5 });
            }
        }
        let nf = a.frames().min(f.token_of_frame.len());
        f.token_of_frame.truncate(nf);
        f.pos.truncate(nf);
        f.note = a.notes[..nf].to_vec();
        f.vel = a.vel[..nf].to_vec();
        f.onset = (0..nf).map(|q| if a.notes[q] > 0.0 && (q == 0 || a.notes[q - 1] != a.notes[q]) { 1.0 } else { 0.0 }).collect();
        let nfeat = f.note_feats();
        for q in 0..t {
            out.frame_idx[i * t + q] = if q < nf { i * n + f.token_of_frame[q] } else { i * n + n - 1 };
        }
        out.note_feats[i * t * FEATS..i * t * FEATS + nf * FEATS].copy_from_slice(&nfeat);
        out.f0[i * t..i * t + nf].copy_from_slice(&a.f0[..nf]);
        out.notes[i * t..i * t + nf].copy_from_slice(&a.notes[..nf]);
        let la = a.audio.len().min(t * HOP);
        out.audio[i * t * HOP..i * t * HOP + la].copy_from_slice(&a.audio[..la]);
    }
    out
}

pub fn voc_batch(crops: &[(Vec<f32>, Vec<f32>, f32)], t: usize, seed: u64) -> VocBatch {
    let b = crops.len();
    let mut audio = Vec::with_capacity(b * t * HOP);
    let mut f0 = Vec::with_capacity(b * t);
    let mut band = Vec::new();
    for (a, f, bd) in crops {
        audio.extend_from_slice(&a[..t * HOP]);
        f0.extend_from_slice(&f[..t]);
        band.push(*bd);
    }
    let src = vocoder::SourceCtl::new(&f0, t, seed);
    VocBatch { b, t, audio, f0, band, src }
}

// ---------------------------------------------------------------------------
// Losses
// ---------------------------------------------------------------------------

/// The multi-resolution STFT settings of the vocoder loss.
pub const MR: [Stft; 3] = [
    Stft { n_fft: 512, hop: 120, win: 480 },
    Stft { n_fft: 1024, hop: 240, win: 960 },
    Stft { n_fft: 2048, hop: 480, win: 1920 },
];

fn bins_below(hz: f32, n_fft: usize) -> u32 {
    ((hz / SR as f32 * n_fft as f32) as u32 + 1).min(n_fft as u32 / 2 + 1)
}

fn mel_bins_below(hz: f32) -> u32 {
    // The first mel filter whose top edge passes `hz`.
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let (m0, m1) = (mel(dsp::MEL_FMIN), mel(dsp::MEL_FMAX));
    let x = (mel(hz.min(dsp::MEL_FMAX)) - m0) / (m1 - m0) * (N_MEL + 1) as f32;
    (x.floor() as u32).saturating_sub(1).clamp(1, N_MEL as u32)
}

/// Log-mel [B*frames, N_MEL] of waveforms `wave` [B*len, 1].
pub fn log_mel(g: &mut Graph, wave: Id) -> Id {
    let mag = g.stft_mag(wave, Stft::MAIN);
    let fb = g.input(Tensor::new(N_MEL, BINS, mel_filters()));
    let mel = g.linear(mag, fb, None);
    g.log_eps(mel, dsp::LOG_FLOOR)
}

/// Normalised log-mel of `t` frames per item from target audio (t*HOP
/// samples per item): the last sample is dropped so the STFT has t frames.
pub fn target_mel(g: &mut Graph, audio: &[f32], b: usize, t: usize) -> Id {
    let len = t * HOP - 1;
    let mut w = Vec::with_capacity(b * len);
    for i in 0..b {
        w.extend_from_slice(&audio[i * t * HOP..i * t * HOP + len]);
    }
    let wi = g.input(Tensor::batched(b * len, 1, w, len, None));
    let m = log_mel(g, wi);
    let m = g.detach(m);
    let bias = g.input(Tensor::new(1, N_MEL, vec![-MEL_MEAN / MEL_STD; N_MEL]));
    let m = g.scale(m, 1.0 / MEL_STD);
    g.add_row(m, bias)
}

pub struct VocLosses {
    pub total: Id,
    pub stft: Id,
    pub mel: Id,
    pub wave: Id,
}

pub fn vocoder_loss(g: &mut Graph, cfg: &VocoderConfig, batch: &VocBatch) -> VocLosses {
    let (b, t) = (batch.b, batch.t);
    let len = t * HOP;
    let mel_in = target_mel(g, &batch.audio, b, t);
    let wave = vocoder::forward(g, cfg, mel_in, &batch.f0, t, &batch.src);
    let target = g.input(Tensor::batched(b * len, 1, batch.audio.clone(), len, None));
    let mut terms = Vec::new();
    for st in MR {
        let lim: Vec<u32> = batch.band.iter().map(|hz| bins_below(*hz, st.n_fft)).collect();
        let mo = g.stft_mag(wave, st);
        let mt = g.stft_mag(target, st);
        let lo = g.log_eps(mo, 1e-5);
        let lt = g.log_eps(mt, 1e-5);
        terms.push((g.l1_loss(lo, lt, None, Some(&lim)), 1.0 / MR.len() as f32));
    }
    let stft = g.sum_scalars(&terms);
    let lim: Vec<u32> = batch.band.iter().map(|hz| mel_bins_below(*hz)).collect();
    let mo = log_mel(g, wave);
    let mt = log_mel(g, target);
    let mel = g.l1_loss(mo, mt, None, Some(&lim));
    let total = g.sum_scalars(&[(stft, 1.0), (mel, 1.0)]);
    VocLosses { total, stft, mel, wave }
}

pub struct GanLosses {
    pub total: Id,
    pub stft: Id,
    pub mel: Id,
    pub adv: Id,
    pub fm: Id,
    pub wave: Id,
    pub real: Id,
}

/// The generator's half of a GAN step: reconstruction (weighted) +
/// adversarial + feature matching.
pub fn vocoder_gan_loss(g: &mut Graph, cfg: &VocoderConfig, batch: &VocBatch) -> GanLosses {
    let r = vocoder_loss(g, cfg, batch);
    let len = batch.t * HOP;
    let real = g.input(Tensor::batched(batch.b * len, 1, batch.audio.clone(), len, None));
    let (adv, fm) = crate::disc::g_losses(g, real, r.wave);
    let total = g.sum_scalars(&[(r.stft, 20.0), (r.mel, 20.0), (adv, 1.0), (fm, 2.0)]);
    GanLosses { total, stft: r.stft, mel: r.mel, adv, fm, wave: r.wave, real }
}

/// V2: the vocoder on the acoustic model's own (teacher-forced, refined)
/// mels for aligned sung items, against the real audio. The acoustic model
/// is frozen (its parameters are in the graph; the caller drops their grads).
pub fn vocoder_gta_loss(g: &mut Graph, ac: &AcousticConfig, voc: &VocoderConfig, batch: &AcBatch, rng: &mut Rng) -> GanLosses {
    let (b, n, t) = (batch.b, batch.n, batch.t);
    let rows = b * t;
    let enc = acoustic::encode(g, ac, &batch.tokens, &batch.singers, n, Some(batch.tok_lens.clone()));
    let mut f0f = vec![0.0; rows * FEATS];
    for r in 0..rows {
        if batch.f0[r] > 0.0 {
            f0f[r * FEATS + 5] = (batch.f0[r] / dsp::F0_REF).ln();
            f0f[r * FEATS + 6] = 1.0;
            if batch.notes[r] > 0.0 {
                f0f[r * FEATS + 7] = ((hz_to_midi(batch.f0[r]) - batch.notes[r]) / 2.0).clamp(-3.0, 3.0);
            }
        }
    }
    let idx = RowIndex::Host(batch.frame_idx.clone());
    let dec = acoustic::decode(g, ac, enc.enc, &idx, t, Some(batch.frame_lens.clone()), &batch.note_feats, &f0f);
    // One refiner step from t0 = 0.6 (the render's shallow start, one step).
    let coarse = g.detach(dec.mel);
    let cond = g.detach(dec.cond);
    let z = g.randn(rows, N_MEL, t, None, rng.next_u64());
    let z = g.scale(z, 0.4);
    let c6 = g.scale(coarse, 0.6);
    let x = g.add(z, c6);
    let v = acoustic::velocity(g, ac, x, &vec![0.6; b], cond);
    let v = g.scale(v, 0.4);
    let mel = g.add(x, v);
    let mel = g.detach(mel);
    let len = t * HOP;
    let src = vocoder::SourceCtl::new(&batch.f0, t, rng.next_u64());
    let wave = vocoder::forward(g, voc, mel, &batch.f0, t, &src);
    let real = g.input(Tensor::batched(b * len, 1, batch.audio.clone(), len, None));
    let mut terms = Vec::new();
    for st in MR {
        let mo = g.stft_mag(wave, st);
        let mt = g.stft_mag(real, st);
        let lo = g.log_eps(mo, 1e-5);
        let lt = g.log_eps(mt, 1e-5);
        terms.push((g.l1_loss(lo, lt, None, None), 1.0 / MR.len() as f32));
    }
    let stft = g.sum_scalars(&terms);
    let mo = log_mel(g, wave);
    let mt = log_mel(g, real);
    let mel_l = g.l1_loss(mo, mt, None, None);
    let (adv, fm) = crate::disc::g_losses(g, real, wave);
    let total = g.sum_scalars(&[(stft, 20.0), (mel_l, 20.0), (adv, 1.0), (fm, 2.0)]);
    GanLosses { total, stft, mel: mel_l, adv, fm, wave, real }
}

pub struct AcLosses {
    pub total: Id,
    pub mel: Id,
    pub dur: Id,
    pub f0: Id,
    pub voicing: Id,
    pub flow: Id,
    pub prior: Id,
    pub coarse: Id,
}

pub fn acoustic_loss(g: &mut Graph, cfg: &AcousticConfig, batch: &AcBatch, rng: &mut Rng) -> AcLosses {
    let (b, n, t) = (batch.b, batch.n, batch.t);
    let rows = b * t;
    let enc = acoustic::encode(g, cfg, &batch.tokens, &batch.singers, n, Some(batch.tok_lens.clone()));
    // f0 features from the target curve (teacher forcing).
    let mut f0f = vec![0.0; rows * FEATS];
    for r in 0..rows {
        if batch.f0[r] > 0.0 {
            f0f[r * FEATS + 5] = (batch.f0[r] / dsp::F0_REF).ln();
            f0f[r * FEATS + 6] = 1.0;
            if batch.notes[r] > 0.0 {
                f0f[r * FEATS + 7] = ((hz_to_midi(batch.f0[r]) - batch.notes[r]) / 2.0).clamp(-3.0, 3.0);
            }
        }
    }
    let frame_mask: Vec<f32> = (0..rows).map(|r| if (r % t) < batch.frame_lens[r / t] as usize { 1.0 } else { 0.0 }).collect();
    let tok_mask: Vec<f32> = (0..b * n).map(|r| if (r % n) < batch.tok_lens[r / n] as usize { 1.0 } else { 0.0 }).collect();
    let mel_t = target_mel(g, &batch.audio, b, t);
    // Token means in mel space; with no durations, MAS on them gives the
    // alignment (and the duration targets), and the prior loss trains them.
    let mu = linear(g, "ac.align", enc.enc);
    let (frame_idx, dur_t) = if batch.align {
        g.mas_align(mu, mel_t, &batch.tok_lens, &batch.frame_lens)
    } else {
        let d = g.input(Tensor::batched(b * n, 1, batch.dur.iter().map(|d| (1.0 + d).ln()).collect(), n, None));
        (RowIndex::Host(batch.frame_idx.clone()), d)
    };
    let mu_f = g.gather_rows_idx(mu, &frame_idx, t, Some(batch.frame_lens.clone()));
    let prior = g.mse_loss(mu_f, mel_t, Some(&frame_mask), None);
    let dec = acoustic::decode(g, cfg, enc.enc, &frame_idx, t, Some(batch.frame_lens.clone()), &batch.note_feats, &f0f);
    let mel = g.l1_loss(dec.mel, mel_t, Some(&frame_mask), None);
    // Durations: ln(1 + frames).
    let dur = g.mse_loss(enc.log_dur, dur_t, Some(&tok_mask), None);
    // f0 head: the residual over the note where both exist, and voicing.
    let res_mask: Vec<f32> = (0..rows).map(|r| if frame_mask[r] > 0.0 && batch.f0[r] > 0.0 && batch.notes[r] > 0.0 { 1.0 } else { 0.0 }).collect();
    let res_t: Vec<f32> = (0..rows).map(|r| f0f[r * FEATS + 7]).collect();
    let res_p = g.slice_cols(dec.f0_head, 0, 1);
    let res_t = g.input(Tensor::batched(rows, 1, res_t, t, None));
    let f0 = g.mse_loss(res_p, res_t, Some(&res_mask), None);
    let v_p = g.slice_cols(dec.f0_head, 1, 1);
    let v_t = g.input(Tensor::batched(rows, 1, (0..rows).map(|r| if batch.f0[r] > 0.0 { 1.0 } else { 0.0 }).collect(), t, None));
    let voicing = g.bce_logits(v_p, v_t);
    // Flow refiner on the detached decoder states.
    let times: Vec<f32> = (0..b).map(|_| rng.unit()).collect();
    let zi = g.randn(rows, N_MEL, t, Some(batch.frame_lens.clone()), rng.next_u64());
    let tcol = g.input(Tensor::batched(rows, 1, (0..rows).map(|r| times[r / t]).collect(), t, None));
    let omt = g.input(Tensor::batched(rows, 1, (0..rows).map(|r| 1.0 - times[r / t]).collect(), t, None));
    let a = g.mul_col(zi, omt);
    let c = g.mul_col(mel_t, tcol);
    let xt = g.add(a, c);
    let vt = g.sub(mel_t, zi);
    let cond = g.detach(dec.cond);
    let vp = acoustic::velocity(g, cfg, xt, &times, cond);
    let flow = g.mse_loss(vp, vt, Some(&frame_mask), None);
    let total = g.sum_scalars(&[(mel, 1.0), (dur, 0.3), (f0, 0.5), (voicing, 0.1), (flow, 1.0), (prior, 0.1)]);
    AcLosses { total, mel, dur, f0, voicing, flow, prior, coarse: dec.mel }
}

// ---------------------------------------------------------------------------
// Optimiser
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct OptConfig {
    pub lr: f32,
    pub b1: f32,
    pub b2: f32,
    pub eps: f32,
    pub wd: f32,
    pub clip: f32,
    pub warmup: usize,
    pub total: usize,
    pub ema: f32,
    /// The step this phase started at (the schedule is relative to it).
    pub start: usize,
}

impl Default for OptConfig {
    fn default() -> Self {
        OptConfig { lr: 4e-4, b1: 0.9, b2: 0.98, eps: 1e-8, wd: 0.01, clip: 1.0, warmup: 2000, total: 200_000, ema: 0.999, start: 0 }
    }
}

impl OptConfig {
    /// Warmup then cosine to 5 % of the peak.
    pub fn lr_at(&self, step: usize) -> f32 {
        let step = step.saturating_sub(self.start);
        if step < self.warmup {
            return self.lr * (step + 1) as f32 / self.warmup as f32;
        }
        let p = ((step - self.warmup) as f32 / (self.total.saturating_sub(self.warmup)).max(1) as f32).min(1.0);
        self.lr * (0.05 + 0.95 * 0.5 * (1.0 + (std::f32::consts::PI * p).cos()))
    }
}

/// Parameters, moments and EMA, on the host or the device.
pub struct Optimizer {
    pub cfg: OptConfig,
    pub step: usize,
    pub params: Params,
    pub m: Vec<Vec<f32>>,
    pub v: Vec<Vec<f32>>,
    pub ema: Vec<Vec<f32>>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub dev: Option<DevState>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub struct DevState {
    pub params: Vec<Arc<Tensor>>,
    pub m: Vec<makepad_ai_cuda::train::DevBuf>,
    pub v: Vec<makepad_ai_cuda::train::DevBuf>,
    pub ema: Vec<makepad_ai_cuda::train::DevBuf>,
    pub sumsq: makepad_ai_cuda::train::DevBuf,
    pub scale: makepad_ai_cuda::train::DevBuf,
}

fn decays(name: &str) -> bool {
    name.ends_with(".w") && !name.contains("emb") && !name.ends_with("singer")
}

impl Optimizer {
    pub fn new(params: Params, cfg: OptConfig) -> Optimizer {
        let m = params.vals.iter().map(|t| vec![0.0; t.len()]).collect();
        let v = params.vals.iter().map(|t| vec![0.0; t.len()]).collect();
        let ema = params.vals.iter().map(|t| t.data.clone()).collect();
        Optimizer {
            cfg,
            step: 0,
            params,
            m,
            v,
            ema,
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            dev: None,
        }
    }

    /// Move parameters, moments and EMA to the device (the trainer's mode).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn to_device(&mut self) {
        use makepad_ai_cuda::train::DevBuf;
        let params = self.params.vals.iter().map(|t| Arc::new(crate::nn_gpu::upload((**t).clone()))).collect();
        self.dev = Some(DevState {
            params,
            m: self.m.iter().map(|v| DevBuf::from_host(v)).collect(),
            v: self.v.iter().map(|v| DevBuf::from_host(v)).collect(),
            ema: self.ema.iter().map(|v| DevBuf::from_host(v)).collect(),
            sumsq: DevBuf::zeros(1),
            scale: DevBuf::zeros(1),
        });
    }

    /// Copy the device state back to the host (for checkpoints).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn sync_host(&mut self) {
        if let Some(d) = &self.dev {
            for i in 0..self.params.vals.len() {
                let t = &self.params.vals[i];
                let host = Tensor::new(t.rows, t.cols, d.params[i].host());
                self.params.vals[i] = Arc::new(host);
                self.m[i] = d.m[i].to_host();
                self.v[i] = d.v[i].to_host();
                self.ema[i] = d.ema[i].to_host();
            }
        }
    }

    /// One AdamW step with global-norm clipping. Returns the learning rate.
    pub fn apply(&mut self, grads: &[Option<GBuf>]) -> f32 {
        self.apply_part(grads, true)
    }

    /// AdamW on the parameters that have gradients; `advance` moves the step
    /// (a GAN applies its two halves with one step).
    pub fn apply_part(&mut self, grads: &[Option<GBuf>], advance: bool) -> f32 {
        let c = self.cfg.clone();
        let lr = c.lr_at(self.step);
        let t = self.step + 1;
        if advance {
            self.step += 1;
        }
        let bc1 = 1.0 - c.b1.powi(t as i32);
        let bc2 = 1.0 - c.b2.powi(t as i32);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        if let Some(d) = &self.dev {
            use makepad_ai_cuda::train::*;
            d.sumsq.fill(0.0);
            for gb in grads.iter().flatten() {
                let gd = gb.dev();
                ck("sumsq", unsafe { mkt_sumsq(gd.ptr(), gd.len(), d.sumsq.mptr(), stream()) });
            }
            ck("clip", unsafe { mkt_clip_scale(d.sumsq.ptr(), c.clip, d.scale.mptr(), stream()) });
            for (i, gb) in grads.iter().enumerate() {
                let Some(gb) = gb else { continue };
                let gd = gb.dev();
                let p = d.params[i].dev.as_ref().unwrap();
                let wd = if decays(&self.params.names[i]) { c.wd } else { 0.0 };
                ck("adamw", unsafe { mkt_adamw(p.mptr(), gd.ptr(), d.m[i].mptr(), d.v[i].mptr(), p.len(), lr, c.b1, c.b2, c.eps, wd, bc1, bc2, d.scale.ptr(), stream()) });
                ck("ema", unsafe { mkt_ema(d.ema[i].mptr(), p.ptr(), c.ema, p.len(), stream()) });
            }
            return lr;
        }
        let sumsq: f32 = grads.iter().flatten().map(|g| g.host().iter().map(|v| v * v).sum::<f32>()).sum();
        let nrm = sumsq.sqrt();
        let gs = if nrm > c.clip { c.clip / nrm } else { 1.0 };
        for (i, gb) in grads.iter().enumerate() {
            let Some(gb) = gb else { continue };
            let gv = gb.host();
            let wd = if decays(&self.params.names[i]) { c.wd } else { 0.0 };
            let mut t = (*self.params.vals[i]).clone();
            for k in 0..t.data.len() {
                let gi = gv[k] * gs;
                self.m[i][k] = c.b1 * self.m[i][k] + (1.0 - c.b1) * gi;
                self.v[i][k] = c.b2 * self.v[i][k] + (1.0 - c.b2) * gi * gi;
                let upd = (self.m[i][k] / bc1) / ((self.v[i][k] / bc2).sqrt() + c.eps);
                t.data[k] = t.data[k] * (1.0 - lr * wd) - lr * upd;
                self.ema[i][k] = c.ema * self.ema[i][k] + (1.0 - c.ema) * t.data[k];
            }
            self.params.vals[i] = Arc::new(t);
        }
        lr
    }

    /// The last gradient norm (device: after `apply`; reads one scalar).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn grad_norm(&self) -> f32 {
        self.dev.as_ref().map(|d| d.sumsq.to_host()[0].sqrt()).unwrap_or(0.0)
    }

    pub fn ema_params(&self) -> Params {
        let mut p = self.params.clone();
        for (i, e) in self.ema.iter().enumerate() {
            let t = &self.params.vals[i];
            p.vals[i] = Arc::new(Tensor::new(t.rows, t.cols, e.clone()));
        }
        p
    }

    /// Save params (f32), moments and EMA next to each other: `<stem>.mksing`,
    /// `<stem>.opt.mksing`, `<stem>.ema.mksing`.
    pub fn save(&self, stem: &std::path::Path, config: &[(String, String)]) -> std::io::Result<()> {
        let mut cfg = config.to_vec();
        cfg.push(("step".into(), self.step.to_string()));
        weights::write(&stem.with_extension("mksing"), &cfg, &self.params, Dtype::F32)?;
        let mut opt = Params::new();
        for (i, n) in self.params.names.iter().enumerate() {
            let t = &self.params.vals[i];
            opt.insert(&format!("m.{n}"), Tensor::new(t.rows, t.cols, self.m[i].clone()));
            opt.insert(&format!("v.{n}"), Tensor::new(t.rows, t.cols, self.v[i].clone()));
        }
        weights::write(&stem.with_extension("opt.mksing"), &cfg, &opt, Dtype::F32)?;
        weights::write(&stem.with_extension("ema.mksing"), &cfg, &self.ema_params(), Dtype::F32)
    }

    /// Resume from `save`'s files.
    pub fn load(stem: &std::path::Path, cfg: OptConfig) -> std::io::Result<Optimizer> {
        let w = weights::read(&stem.with_extension("mksing"))?;
        let step = w.config.iter().find(|(k, _)| k == "step").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
        let mut o = Optimizer::new(w.params, cfg);
        o.step = step;
        if let Ok(opt) = weights::read(&stem.with_extension("opt.mksing")) {
            for (i, n) in o.params.names.clone().iter().enumerate() {
                if opt.params.has(&format!("m.{n}")) {
                    o.m[i] = opt.params.get(&format!("m.{n}")).data.clone();
                    o.v[i] = opt.params.get(&format!("v.{n}")).data.clone();
                }
            }
        }
        if let Ok(e) = weights::read(&stem.with_extension("ema.mksing")) {
            for (i, n) in o.params.names.clone().iter().enumerate() {
                if e.params.has(n) {
                    o.ema[i] = e.params.get(n).data.clone();
                }
            }
        }
        Ok(o)
    }
}

// ---------------------------------------------------------------------------
// The synthetic singer (T0)
// ---------------------------------------------------------------------------

/// A fixed, smooth random spectral envelope per phoneme (dB over 64 points),
/// harmonic and noise parts; singer `s` shifts every envelope a little.
pub struct SynthSinger {
    env_h: Vec<[f32; 64]>,
    env_n: Vec<[f32; 64]>,
}

impl SynthSinger {
    pub fn new(seed: u64) -> SynthSinger {
        let mut rng = Rng::new(seed);
        let mut env = |bright: f32| {
            let mut e = [0.0f32; 64];
            // A few random formant bumps over a tilt.
            let bumps: Vec<(f32, f32, f32)> = (0..4).map(|_| (rng.unit() * 50.0 + 4.0, rng.unit() * 18.0 + 6.0, rng.unit() * 4.0 + 1.5)).collect();
            for (i, v) in e.iter_mut().enumerate() {
                let x = i as f32;
                *v = -x * bright;
                for (c, h, w) in &bumps {
                    *v += h * (-((x - c) / w).powi(2)).exp();
                }
            }
            e
        };
        let env_h = (0..ph::TABLE).map(|_| env(0.6)).collect();
        let env_n = (0..ph::TABLE).map(|_| env(0.15)).collect();
        SynthSinger { env_h, env_n }
    }

    fn gain(env: &[f32; 64], bin: usize) -> f32 {
        let x = bin as f32 / BINS as f32 * 63.0;
        let i = (x as usize).min(62);
        let f = x - i as f32;
        let db = env[i] * (1.0 - f) + env[i + 1] * f;
        10f32.powf(db / 20.0)
    }

    /// Audio for aligned frames: per frame, the phoneme's harmonic envelope on
    /// the harmonic source (voiced frames) and its noise envelope on noise
    /// (louder on unvoiced consonants; silent on SP).
    pub fn render(&self, f: &Frames, seed: u64) -> Vec<f32> {
        let t = f.len();
        let len = t * HOP;
        let st = Stft::MAIN;
        let (hr, hi) = st.forward(&dsp::harmonic_source(&f.f0, len));

        let (nr, ni) = st.forward(&dsp::noise_source(len, seed));
        let frames = st.frames(len);
        let mut yr = vec![0.0; frames * BINS];
        let mut yi = vec![0.0; frames * BINS];
        for r in 0..t {
            let tok = f.tokens[f.token_of_frame[r]];
            let (gh, gn) = match tok {
                ph::SP | ph::PAD => (0.0, 0.0),
                ph::AP => (0.0, 0.3),
                p if ph::is_voiced(p) => (1.0, if ph::is_vowel(p) { 0.05 } else { 0.4 }),
                _ => (0.0, 1.0),
            };
            let (eh, en) = (&self.env_h[tok as usize], &self.env_n[tok as usize]);
            for q in 0..BINS {
                let i = r * BINS + q;
                let a = gh * Self::gain(eh, q) * 4.0;
                let b = gn * Self::gain(en, q);
                yr[i] = a * hr[i] + b * nr[i];
                yi[i] = a * hi[i] + b * ni[i];
            }
        }
        st.inverse(&yr, &yi, frames, len)
    }

    /// A random sung line on the synthetic voice, aligned.
    pub fn example(&self, rng: &mut Rng) -> Aligned {
        let vowels: Vec<u8> = (3..=18).collect();
        let cons: Vec<u8> = (19..=44).collect();
        let n = 3 + rng.below(6);
        let mut notes = Vec::new();
        let mut midi = 55.0 + rng.below(14) as f32;
        let mut start = 0.3;
        for _ in 0..n {
            let dur = 0.2 + rng.unit() * 0.5;
            let syl = if rng.unit() < 0.15 && !notes.is_empty() {
                None
            } else {
                let mut onset = Vec::new();
                if rng.unit() < 0.7 {
                    onset.push(cons[rng.below(cons.len())]);
                }
                let mut coda = Vec::new();
                if rng.unit() < 0.4 {
                    coda.push(cons[rng.below(cons.len())]);
                }
                Some(score::Syllable { onset, nucleus: vec![vowels[rng.below(vowels.len())]], coda })
            };
            notes.push(score::Note { start, dur, midi, vel: 0.8, syllable: syl });
            start += dur + if rng.unit() < 0.2 { 0.4 } else { 0.0 };
            midi = (midi + rng.below(7) as f32 - 3.0).clamp(48.0, 76.0);
        }
        let s = score::SingScore { notes, singer: 0 };
        let style = PitchStyle { seed: rng.next_u64(), ..PitchStyle::default() };
        let f = score::align(&s, &score::rule_consonants, &style, 0.3);
        let audio = self.render(&f, rng.next_u64());
        Aligned {
            tokens: f.tokens.clone(),
            dur: f.token_frames.clone(),
            notes: f.note.clone(),
            f0: f.f0.clone(),
            vel: f.vel.clone(),
            audio,
            singer: 0,
            band: 24_000.0,
        }
    }
}

/// Mean |cents| between rendered audio's f0 and the target curve (voiced
/// steady frames), for reports.
pub fn f0_cents(audio: &[f32], f0: &[f32]) -> f32 {
    crate::cantor::f0_error_cents(audio, f0).0
}
