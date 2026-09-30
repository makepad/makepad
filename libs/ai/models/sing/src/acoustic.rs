//! The acoustic model: phonemes + notes + f0 -> 128-bin log-mel.
//!
//! A phoneme encoder (transformer) with a duration head; tokens expanded to
//! frames; note features added; an f0 head (a residual over the note, and a
//! voicing logit); the f0 features added; a conformer decoder to a coarse mel;
//! and a rectified-flow refiner that adds the detail a regression smooths out.

use crate::dsp::{Rng, N_MEL};
use crate::layers::*;
use crate::nn::{Act, Graph, Id, Init, Params, RowIndex, Tensor};
use crate::phonemes;
use crate::score::FEATS;

/// Log-mel values are normalised as (x - MEL_MEAN) / MEL_STD for the model.
pub const MEL_MEAN: f32 = -5.0;
pub const MEL_STD: f32 = 2.5;

#[derive(Clone, Debug, PartialEq)]
pub struct AcousticConfig {
    pub d: usize,
    pub heads: usize,
    pub enc_layers: usize,
    pub dec_layers: usize,
    pub ffn: usize,
    pub conv_k: usize,
    pub refine_layers: usize,
    pub refine_hidden: usize,
    pub singers: usize,
}

impl AcousticConfig {
    /// The shipping size (~12.7 M parameters).
    pub fn base() -> Self {
        AcousticConfig { d: 256, heads: 4, enc_layers: 4, dec_layers: 6, ffn: 1024, conv_k: 31, refine_layers: 8, refine_hidden: 768, singers: 64 }
    }

    /// A tiny model for tests and first runs.
    pub fn tiny() -> Self {
        AcousticConfig { d: 32, heads: 2, enc_layers: 1, dec_layers: 1, ffn: 64, conv_k: 7, refine_layers: 1, refine_hidden: 64, singers: 4 }
    }

    pub fn to_kv(&self) -> Vec<(String, String)> {
        [
            ("d", self.d),
            ("heads", self.heads),
            ("enc_layers", self.enc_layers),
            ("dec_layers", self.dec_layers),
            ("ffn", self.ffn),
            ("conv_k", self.conv_k),
            ("refine_layers", self.refine_layers),
            ("refine_hidden", self.refine_hidden),
            ("singers", self.singers),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    pub fn from_kv(kv: &[(String, String)]) -> Option<Self> {
        let get = |k: &str| kv.iter().find(|(a, _)| a == k).and_then(|(_, v)| v.parse().ok());
        Some(AcousticConfig {
            d: get("d")?,
            heads: get("heads")?,
            enc_layers: get("enc_layers")?,
            dec_layers: get("dec_layers")?,
            ffn: get("ffn")?,
            conv_k: get("conv_k")?,
            refine_layers: get("refine_layers")?,
            refine_hidden: get("refine_hidden")?,
            singers: get("singers")?,
        })
    }

    pub fn declare(&self, p: &mut Params, rng: &mut Rng) {
        let d = self.d;
        p.declare("ac.ph_emb", phonemes::TABLE, d, Init::Normal(0.3), rng);
        p.declare("ac.singer", self.singers, d, Init::Normal(0.1), rng);
        for l in 0..self.enc_layers {
            declare_transformer(p, rng, &format!("ac.enc.{l}"), d, self.ffn);
        }
        declare_norm(p, rng, "ac.enc_out", d);
        declare_conv(p, rng, "ac.dur.c1", d, d, 3);
        declare_norm(p, rng, "ac.dur.n1", d);
        declare_conv(p, rng, "ac.dur.c2", d, d, 3);
        declare_norm(p, rng, "ac.dur.n2", d);
        declare_linear(p, rng, "ac.dur.out", d, 1, 0.1);
        // Token means in mel space: the aligner for speech without durations.
        declare_linear(p, rng, "ac.align", d, N_MEL, 1.0);
        declare_linear(p, rng, "ac.note_in", FEATS, d, 1.0);
        declare_conv(p, rng, "ac.f0.c1", d, d, 5);
        declare_norm(p, rng, "ac.f0.n1", d);
        declare_linear(p, rng, "ac.f0.out", d, 2, 0.1);
        declare_linear(p, rng, "ac.f0_in", FEATS, d, 1.0);
        for l in 0..self.dec_layers {
            declare_conformer(p, rng, &format!("ac.dec.{l}"), d, self.ffn, self.conv_k);
        }
        declare_linear(p, rng, "ac.mel", d, N_MEL, 0.5);
        // Refiner.
        declare_linear(p, rng, "ac.rf.in", N_MEL + d, d, 1.0);
        declare_linear(p, rng, "ac.rf.t1", 64, d, 1.0);
        declare_linear(p, rng, "ac.rf.t2", d, d, 1.0);
        for l in 0..self.refine_layers {
            declare_convnext(p, rng, &format!("ac.rf.{l}"), d, self.refine_hidden, 7);
        }
        declare_norm(p, rng, "ac.rf.n", d);
        declare_linear(p, rng, "ac.rf.out", d, N_MEL, 0.5);
    }
}

pub struct Encoded {
    pub enc: Id,
    /// ln(1 + frames) per token, predicted.
    pub log_dur: Id,
}

pub struct Decoded {
    /// Normalised coarse mel [T, N_MEL].
    pub mel: Id,
    /// Decoder states [T, d] (the refiner's condition).
    pub cond: Id,
    /// [T, 2]: predicted f0 residual (semitones / 2 over the note) and voicing logit.
    pub f0_head: Id,
}

/// Encode a batch of token sequences: `tokens` holds `singers.len()` items of
/// `n` tokens each (padded), `lens` the real token counts.
pub fn encode(g: &mut Graph, cfg: &AcousticConfig, tokens: &[u8], singers: &[usize], n: usize, lens: Option<Vec<u32>>) -> Encoded {
    let emb = g.p("ac.ph_emb");
    let x = g.gather_rows(emb, &tokens.iter().map(|t| *t as usize).collect::<Vec<_>>(), n, lens.clone());
    let sg = g.p("ac.singer");
    let sidx: Vec<usize> = singers.iter().flat_map(|s| std::iter::repeat((*s).min(cfg.singers - 1)).take(n)).collect();
    let s = g.gather_rows(sg, &sidx, n, lens);
    let mut x = g.add(x, s);
    for l in 0..cfg.enc_layers {
        x = transformer(g, &format!("ac.enc.{l}"), x, cfg.heads);
    }
    let enc = norm(g, "ac.enc_out", x);
    let h = conv(g, "ac.dur.c1", enc, 3);
    let h = g.act(h, Act::Relu);
    let h = norm(g, "ac.dur.n1", h);
    let h = conv(g, "ac.dur.c2", h, 3);
    let h = g.act(h, Act::Relu);
    let h = norm(g, "ac.dur.n2", h);
    let log_dur = linear(g, "ac.dur.out", h);
    Encoded { enc, log_dur }
}

/// Frames: expand the encoding (`frame_idx` = the encoder row of every frame,
/// `t` frames per item, `lens` the real frame counts), add the note features,
/// run the f0 head, add the f0 features (given: the rule curve, the
/// recording's f0, or the head's own prediction), and decode.
#[allow(clippy::too_many_arguments)]
pub fn decode(g: &mut Graph, cfg: &AcousticConfig, enc: Id, frame_idx: &RowIndex, t: usize, lens: Option<Vec<u32>>, note_feats: &[f32], f0_feats: &[f32]) -> Decoded {
    let rows = frame_idx.len();
    let h = g.gather_rows_idx(enc, frame_idx, t, lens.clone());
    let nf = g.input(Tensor::batched(rows, FEATS, note_feats.to_vec(), t, lens.clone()));
    let nf = linear(g, "ac.note_in", nf);
    let h = g.add(h, nf);
    let f = conv(g, "ac.f0.c1", h, 5);
    let f = g.act(f, Act::Gelu);
    let f = norm(g, "ac.f0.n1", f);
    let f0_head = linear(g, "ac.f0.out", f);
    let ff = g.input(Tensor::batched(rows, FEATS, f0_feats.to_vec(), t, lens));
    let ff = linear(g, "ac.f0_in", ff);
    let mut x = g.add(h, ff);
    for l in 0..cfg.dec_layers {
        x = conformer(g, &format!("ac.dec.{l}"), x, cfg.heads, cfg.conv_k);
    }
    let mel = linear(g, "ac.mel", x);
    Decoded { mel, cond: x, f0_head }
}

pub fn time_embedding(t: f32) -> Vec<f32> {
    (0..64)
        .map(|i| {
            let f = (1000f32).powf(-((i % 32) as f32) / 32.0) * t * 1000.0;
            if i < 32 { f.sin() } else { f.cos() }
        })
        .collect()
}

/// The refiner's velocity field v(x_t, t | cond); `t_items` = the flow time
/// of each item (x_t's rows are items * seg).
pub fn velocity(g: &mut Graph, cfg: &AcousticConfig, x_t: Id, t_items: &[f32], cond: Id) -> Id {
    let (rows, seg, lens) = {
        let v = g.val(x_t);
        (v.rows, v.seg, v.lens.as_ref().map(|l| l.to_vec()))
    };
    let x = g.concat_cols(&[x_t, cond]);
    let mut h = linear(g, "ac.rf.in", x);
    let mut te = Vec::with_capacity(rows * 64);
    for r in 0..rows {
        te.extend(time_embedding(t_items[r / seg]));
    }
    let te = g.input(Tensor::batched(rows, 64, te, seg, lens));
    let te = linear(g, "ac.rf.t1", te);
    let te = g.act(te, Act::Silu);
    let te = linear(g, "ac.rf.t2", te);
    h = g.add(h, te);
    for l in 0..cfg.refine_layers {
        h = convnext(g, &format!("ac.rf.{l}"), h, 7);
    }
    let h = norm(g, "ac.rf.n", h);
    linear(g, "ac.rf.out", h)
}

/// Shallow flow refine: start at `t0` between noise and the coarse mel and
/// take `steps` Euler steps to t = 1. Normalised mel in and out.
pub fn refine(params: &Params, cfg: &AcousticConfig, coarse: &Tensor, cond: &Tensor, t0: f32, steps: usize, seed: u64) -> Tensor {
    if steps == 0 {
        return coarse.clone();
    }
    let mut rng = Rng::new(seed);
    let mut x: Vec<f32> = coarse.data.iter().map(|c| (1.0 - t0) * rng.normal() + t0 * c).collect();
    let dt = (1.0 - t0) / steps as f32;
    for s in 0..steps {
        let t = t0 + s as f32 * dt;
        let mut g = Graph::new(params, false);
        let xi = g.input(Tensor::new(coarse.rows, coarse.cols, x.clone()));
        let ci = g.input(cond.clone());
        let v = velocity(&mut g, cfg, xi, &[t], ci);
        for (a, b) in x.iter_mut().zip(&g.host(v)) {
            *a += dt * b;
        }
    }
    Tensor::new(coarse.rows, coarse.cols, x)
}

pub fn normalise_mel(logmel: &[f32]) -> Vec<f32> {
    logmel.iter().map(|v| (v - MEL_MEAN) / MEL_STD).collect()
}

pub fn denormalise_mel(m: &[f32]) -> Vec<f32> {
    m.iter().map(|v| v * MEL_STD + MEL_MEAN).collect()
}
