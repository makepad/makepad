//! The vocoder's discriminators (training only; they never ship).
//!
//! - Period discriminators (periods 2, 3, 5, 7, 11): the waveform folded by
//!   its period, each phase column its own sequence, through 1-D convolutions
//!   that downsample by 3 (a reshape that stacks 3 steps into channels).
//! - Spectrogram discriminators at three STFT resolutions: log magnitudes,
//!   bins as channels, convolutions over time.
//!
//! Least-squares GAN losses and feature matching.

use crate::dsp::{Rng, Stft};
use crate::layers::{conv, declare_conv};
use crate::nn::{Act, Graph, Id, Params, Tensor};

pub const PERIODS: [usize; 5] = [2, 3, 5, 7, 11];
pub const RES: [Stft; 3] = [
    Stft { n_fft: 512, hop: 128, win: 512 },
    Stft { n_fft: 1024, hop: 256, win: 1024 },
    Stft { n_fft: 2048, hop: 512, win: 2048 },
];
const MPD_CH: [usize; 4] = [32, 64, 128, 256];
const MRD_CH: usize = 128;

pub fn declare(p: &mut Params, rng: &mut Rng) {
    for per in PERIODS {
        let n = format!("disc.p{per}");
        let mut cin = 1;
        for (l, c) in MPD_CH.iter().enumerate() {
            declare_conv(p, rng, &format!("{n}.c{l}"), cin, *c, 5);
            cin = c * 3;
        }
        declare_conv(p, rng, &format!("{n}.post"), MPD_CH[3], 1, 3);
    }
    for (i, st) in RES.iter().enumerate() {
        let n = format!("disc.r{i}");
        declare_conv(p, rng, &format!("{n}.c0"), st.bins(), MRD_CH, 5);
        declare_conv(p, rng, &format!("{n}.c1"), MRD_CH, MRD_CH, 5);
        declare_conv(p, rng, &format!("{n}.c2"), MRD_CH, MRD_CH, 5);
        declare_conv(p, rng, &format!("{n}.post"), MRD_CH, 1, 3);
    }
}

/// Every sub-discriminator's (logits, feature maps) for waveforms [B*len, 1].
pub fn run(g: &mut Graph, wave: Id) -> Vec<(Id, Vec<Id>)> {
    let (rows, len) = {
        let v = g.val(wave);
        (v.rows, v.seg)
    };
    let b = rows / len;
    let mut out = Vec::new();
    for per in PERIODS {
        let n = format!("disc.p{per}");
        let lp = (len / per) / 27 * 27;
        let mut idx = Vec::with_capacity(b * per * lp);
        for item in 0..b {
            for ph in 0..per {
                for r in 0..lp {
                    idx.push(item * len + r * per + ph);
                }
            }
        }
        let mut x = g.gather_rows(wave, &idx, lp, None);
        let mut seg = lp;
        let mut feats = Vec::new();
        for l in 0..MPD_CH.len() {
            x = conv(g, &format!("{n}.c{l}"), x, 5);
            x = g.act(x, Act::LeakyRelu);
            feats.push(x);
            if l + 1 < MPD_CH.len() {
                let (r, c) = g.shape(x);
                seg /= 3;
                x = g.reshape(x, r / 3, c * 3, seg);
            }
        }
        let logit = conv(g, &format!("{n}.post"), x, 3);
        out.push((logit, feats));
    }
    for (i, st) in RES.iter().enumerate() {
        let n = format!("disc.r{i}");
        let m = g.stft_mag(wave, *st);
        let mut x = g.log_eps(m, 1e-5);
        let mut feats = Vec::new();
        for l in 0..3 {
            x = conv(g, &format!("{n}.c{l}"), x, 5);
            x = g.act(x, Act::LeakyRelu);
            feats.push(x);
        }
        let logit = conv(g, &format!("{n}.post"), x, 3);
        out.push((logit, feats));
    }
    out
}

fn constant_like(g: &mut Graph, x: Id, v: f32) -> Id {
    let t = g.val(x);
    let (rows, cols, seg) = (t.rows, t.cols, t.seg);
    g.input(Tensor::batched(rows, cols, vec![v; rows * cols], seg, None))
}

/// The discriminators' loss on real and (detached) generated audio.
pub fn d_loss(g: &mut Graph, real: Id, fake: Id) -> Id {
    let dr = run(g, real);
    let df = run(g, fake);
    let mut terms = Vec::new();
    for ((lr, _), (lf, _)) in dr.iter().zip(&df) {
        let one = constant_like(g, *lr, 1.0);
        let zero = constant_like(g, *lf, 0.0);
        terms.push((g.mse_loss(*lr, one, None, None), 1.0));
        terms.push((g.mse_loss(*lf, zero, None, None), 1.0));
    }
    g.sum_scalars(&terms)
}

/// The generator's adversarial loss and feature-matching loss.
pub fn g_losses(g: &mut Graph, real: Id, fake: Id) -> (Id, Id) {
    let dr = run(g, real);
    let df = run(g, fake);
    let mut adv = Vec::new();
    let mut fm = Vec::new();
    for ((_, fr), (lf, ff)) in dr.iter().zip(&df) {
        let one = constant_like(g, *lf, 1.0);
        adv.push((g.mse_loss(*lf, one, None, None), 1.0));
        for (a, b) in fr.iter().zip(ff) {
            let t = g.detach(*a);
            fm.push((g.l1_loss(*b, t, None, None), 1.0 / fr.len() as f32));
        }
    }
    (g.sum_scalars(&adv), g.sum_scalars(&fm))
}
