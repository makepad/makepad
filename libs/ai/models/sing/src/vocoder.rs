//! The vocoder: log-mel + f0 -> 48 kHz waveform.
//!
//! A harmonic-plus-noise source (DSP: every harmonic of f0, phase-continuous,
//! plus seeded noise) is transformed to the STFT domain; a frame-rate ConvNeXt
//! network predicts, for every bin and frame, a harmonic gain, a noise gain and
//! a harmonic phase rotation; the filtered spectrum is inverted. Every harmonic
//! comes from the source at exactly k * f0, so the output pitch is the f0 curve
//! by construction.

use crate::dsp::{self, Rng, Stft, BINS, F0_REF, N_MEL};
use crate::layers::*;
use crate::nn::{Graph, Id, Params, Tensor};

#[derive(Clone, Debug, PartialEq)]
pub struct VocoderConfig {
    pub d: usize,
    pub layers: usize,
    pub hidden: usize,
}

/// Input columns: normalised mel, ln(f0/ref), voiced.
pub const VOC_IN: usize = N_MEL + 2;

impl VocoderConfig {
    /// The shipping size (~8.8 M parameters).
    pub fn base() -> Self {
        VocoderConfig { d: 384, layers: 8, hidden: 1152 }
    }

    pub fn tiny() -> Self {
        VocoderConfig { d: 32, layers: 1, hidden: 64 }
    }

    pub fn to_kv(&self) -> Vec<(String, String)> {
        vec![("d".into(), self.d.to_string()), ("layers".into(), self.layers.to_string()), ("hidden".into(), self.hidden.to_string())]
    }

    pub fn from_kv(kv: &[(String, String)]) -> Option<Self> {
        let get = |k: &str| kv.iter().find(|(a, _)| a == k).and_then(|(_, v)| v.parse().ok());
        Some(VocoderConfig { d: get("d")?, layers: get("layers")?, hidden: get("hidden")? })
    }

    pub fn declare(&self, p: &mut Params, rng: &mut Rng) {
        declare_conv(p, rng, "voc.in", VOC_IN, self.d, 7);
        declare_norm(p, rng, "voc.n0", self.d);
        for l in 0..self.layers {
            declare_convnext(p, rng, &format!("voc.{l}"), self.d, self.hidden, 7);
        }
        declare_norm(p, rng, "voc.n1", self.d);
        declare_linear(p, rng, "voc.out", self.d, 4 * BINS, 0.1);
        // Start as a gentle pass-through: harmonic gain 1, noise gain e^-3,
        // phase rotation 0 (pa = 1, pb = 0).
        let mut b = vec![0.0; 4 * BINS];
        for i in 0..BINS {
            b[BINS + i] = -3.0;
            b[2 * BINS + i] = 1.0;
        }
        p.insert("voc.out.b", Tensor::new(1, 4 * BINS, b));
    }
}

/// A batch's source: per-sample controls (cheap to make on the host) from
/// which the harmonic and noise waveforms are made where the graph runs.
pub struct SourceCtl {
    pub items: usize,
    pub len: usize,
    pub ph: Vec<f32>,
    pub hz: Vec<f32>,
    pub amp: Vec<f32>,
    pub seed: u64,
    /// Make the waveforms on the host even on a GPU graph (bit-identical
    /// sources for differential tests).
    pub host: bool,
}

impl SourceCtl {
    /// For `t` frames of f0 per item.
    pub fn new(f0: &[f32], t: usize, seed: u64) -> SourceCtl {
        let items = f0.len() / t;
        let len = t * dsp::HOP;
        let (mut ph, mut hz, mut amp) = (Vec::with_capacity(items * len), Vec::with_capacity(items * len), Vec::with_capacity(items * len));
        for b in 0..items {
            let (p, h, a) = dsp::source_controls(&f0[b * t..(b + 1) * t], len);
            ph.extend(p);
            hz.extend(h);
            amp.extend(a);
        }
        SourceCtl { items, len, ph, hz, amp, seed, host: false }
    }

    /// The waveforms on the host: (harmonic, noise), `len` samples per item.
    pub fn host_waves(&self) -> (Tensor, Tensor) {
        let n = self.items * self.len;
        let h: Vec<f32> = (0..n).map(|i| dsp::harmonic_sample(self.ph[i], self.hz[i], self.amp[i])).collect();
        let mut noise = Vec::with_capacity(n);
        for b in 0..self.items {
            noise.extend(dsp::noise_source(self.len, self.seed.wrapping_add(b as u64 * 7919)));
        }
        (Tensor::batched(n, 1, h, self.len, None), Tensor::batched(n, 1, noise, self.len, None))
    }
}

/// The f0 columns of the vocoder input: ln(f0/ref), voiced.
pub fn f0_inputs(f0: &[f32]) -> Vec<f32> {
    let mut x = vec![0.0; f0.len() * 2];
    for (r, f) in f0.iter().enumerate() {
        if *f > 0.0 {
            x[r * 2] = (f / F0_REF).ln();
            x[r * 2 + 1] = 1.0;
        }
    }
    x
}

/// Waveforms [B*len, 1] for a batch of normalised mels `mel` [B*t, N_MEL]
/// and f0 curves [B*t] (`t` frames per item), with source waveforms from
/// `src` (same seed = same noise on the same backend).
pub fn forward(g: &mut Graph, cfg: &VocoderConfig, mel: Id, f0: &[f32], t: usize, src: &SourceCtl) -> Id {
    let rows = f0.len();
    let len = t * dsp::HOP;
    let fi = g.input(Tensor::batched(rows, 2, f0_inputs(f0), t, None));
    let x = g.concat_cols(&[mel, fi]);
    let h = conv(g, "voc.in", x, 7);
    let mut h = norm(g, "voc.n0", h);
    for l in 0..cfg.layers {
        h = convnext(g, &format!("voc.{l}"), h, 7);
    }
    let h = norm(g, "voc.n1", h);
    let o = linear(g, "voc.out", h);
    let gh = g.slice_cols(o, 0, BINS);
    let gn = g.slice_cols(o, BINS, BINS);
    let pa = g.slice_cols(o, 2 * BINS, BINS);
    let pb = g.slice_cols(o, 3 * BINS, BINS);
    let (harm, noise) = g.source_waves(src);
    let (hr, hi) = g.stft_const(harm, Stft::MAIN, t);
    let (nr, ni) = g.stft_const(noise, Stft::MAIN, t);
    let y = g.hn_filter(gh, gn, pa, pb, hr, hi, nr, ni);
    g.istft(y, Stft::MAIN, len)
}
