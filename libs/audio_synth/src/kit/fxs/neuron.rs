//! Neuron (type 15): a gated-recurrent-cell distortion. Each channel's
//! output is a sigmoid-gated blend of its own delayed output and a tanh of
//! the input, where the delayed output comes from a modulated short comb.
//! Runs 4x oversampled. EFFECTS-2.md 6.2.
//!
//! Also home of two small helpers the 4x effects share: `Os4` (two
//! cascaded soft half-band stages each way) and `LinSmooth` (a linear ramp
//! over a fixed number of steps).

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, Biquad, HalfBand, SlotDef, SlotUnit, SlotValue};

/// The Drive dB value is clamped to 0..1 dB before conversion, so Drive
/// barely matters; mirrored so presets sound the same.
const QUIRK_DRIVE_CLAMPED_TO_ONE_DB: bool = true;
const _: () = assert!(QUIRK_DRIVE_CLAMPED_TO_ONE_DB);
/// The comb delay divides by the angular frequency (2 pi f), not f.
const QUIRK_COMB_DIVIDES_BY_ANGULAR: bool = true;
const _: () = assert!(QUIRK_COMB_DIVIDES_BY_ANGULAR);
/// The stored waveform index goes straight into the 7-shape LFO order
/// (0 Sine, 1 Tri, 2 Ramp, 3 Saw, 4 Square, 5 Noise) although the UI labels
/// follow the 6-shape list.
const QUIRK_WAVE_INDEX_7_SHAPE: bool = true;
const _: () = assert!(QUIRK_WAVE_INDEX_7_SHAPE);
/// The LFO's per-block ramp is advanced once per oversampled sample, so it
/// overshoots 4x within a block and snaps back at the next block.
const QUIRK_LFO_RAMP_PER_OS_SAMPLE: bool = true;
const _: () = assert!(QUIRK_LFO_RAMP_PER_OS_SAMPLE);
/// The DC blocker uses omega = 35 / sr (about 5.6 Hz), not 2 pi 35 / sr.
const QUIRK_DC_OMEGA_35_OVER_SR: bool = true;
const _: () = assert!(QUIRK_DC_OMEGA_35_OVER_SR);

const WAVES: &[&str] = &["Sine", "Triangle", "Sawtooth", "Noise", "Sample & Hold", "Square"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        db("Drive", -24.0, 24.0, 0.0),
        percent("Squash", 0.5),
        percent("Stab", 0.5),
        percent("Asymmetry", 1.0),
        percent("Bias", 0.0),
        freq("Frequency", 70.0),
        fl("Separation", -96.0, 96.0, 0.5, SlotUnit::Semis),
        choice("Waveform", WAVES, 0.0),
        deact(lforate("Rate", -2.0)),
        percent("Depth", 0.0),
        width_db("Width"),
        db("Gain", -24.0, 24.0, 0.0),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// 4x oversampling: two cascaded soft (M = 3) half-band stages each way.
/// The upsampler keeps the half-band's default 0.5 scale per stage (the
/// reference's oversampler loses 12 dB at 4x; effects compensate where
/// the spec says so).
pub(super) struct Os4 {
    up1: HalfBand,
    up2: HalfBand,
    dn1: HalfBand,
    dn2: HalfBand,
}

impl Os4 {
    pub fn new() -> Os4 {
        Os4 { up1: HalfBand::new(6, false), up2: HalfBand::new(6, false), dn1: HalfBand::new(6, false), dn2: HalfBand::new(6, false) }
    }
    pub fn reset(&mut self) {
        self.up1.reset();
        self.up2.reset();
        self.dn1.reset();
        self.dn2.reset();
    }
    pub fn up(&mut self, l: &[f32; 32], r: &[f32; 32], ol: &mut [f32; 128], or: &mut [f32; 128]) {
        let mut ml = [0f32; 64];
        let mut mr = [0f32; 64];
        self.up1.upsample_into(l, r, &mut ml, &mut mr, 0.5);
        self.up2.upsample_into(&ml, &mr, ol, or, 0.5);
    }
    pub fn down(&mut self, l: &mut [f32; 128], r: &mut [f32; 128], ol: &mut [f32; 32], or: &mut [f32; 32]) {
        self.dn1.decimate(&mut l[..], &mut r[..]);
        self.dn2.decimate(&mut l[..64], &mut r[..64]);
        ol.copy_from_slice(&l[..32]);
        or.copy_from_slice(&r[..32]);
    }
}

/// A linear ramp to each new target over `n` steps (the first target
/// snaps unless built with `from`).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LinSmooth {
    pub v: f64,
    target: f64,
    step: f64,
    left: u32,
    n: u32,
    started: bool,
}

impl LinSmooth {
    pub fn new(n: u32) -> LinSmooth {
        LinSmooth { n: n.max(1), ..Default::default() }
    }
    /// Starts at `v` and ramps from there on the first target.
    pub fn from(n: u32, v: f64) -> LinSmooth {
        LinSmooth { v, target: v, n: n.max(1), started: true, ..Default::default() }
    }
    pub fn set(&mut self, t: f64) {
        if !self.started {
            self.v = t;
            self.target = t;
            self.started = true;
        } else if t != self.target {
            self.target = t;
            self.step = (t - self.v) / self.n as f64;
            self.left = self.n;
        }
    }
    #[inline]
    pub fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.v += self.step;
            self.left -= 1;
            if self.left == 0 {
                self.v = self.target;
            }
        }
        self.v
    }
    pub fn ramping(&self) -> bool {
        self.left > 0
    }
}

#[inline]
fn sigmoid(u: f32) -> f32 {
    1.0 / (1.0 + (-u).exp())
}

pub struct Neuron {
    sr: f32,
    os: Os4,
    lfo: ModLfo,
    lines: [SincLine; 2],
    y1: [f32; 2],
    wf: LinSmooth,
    wh: LinSmooth,
    uf: LinSmooth,
    uh: LinSmooth,
    bf: LinSmooth,
    d: [LinSmooth; 2],
    dc: Biquad,
    makeup: Line,
    width: Line,
    gain: Line,
}

impl Neuron {
    pub fn new(sr: f32) -> Neuron {
        let mut lfo = ModLfo::new(0x4e55);
        lfo.single_random = true;
        Neuron {
            sr,
            os: Os4::new(),
            lfo,
            lines: [SincLine::new(1 << 18), SincLine::new(1 << 18)],
            y1: [0.0; 2],
            wf: LinSmooth::new(200),
            wh: LinSmooth::new(200),
            uf: LinSmooth::new(200),
            uh: LinSmooth::new(200),
            bf: LinSmooth::new(200),
            d: [LinSmooth::new(200), LinSmooth::new(200)],
            dc: Biquad::new(),
            makeup: Line::new(),
            width: Line::new(),
            gain: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.os.reset();
        self.lfo.reset();
        for l in self.lines.iter_mut() {
            l.clear();
        }
        self.y1 = [0.0; 2];
        self.wf = LinSmooth::new(200);
        self.wh = LinSmooth::new(200);
        self.uf = LinSmooth::new(200);
        self.uh = LinSmooth::new(200);
        self.bf = LinSmooth::new(200);
        self.d = [LinSmooth::new(200), LinSmooth::new(200)];
        self.dc = Biquad::new();
        self.makeup = Line::new();
        self.width = Line::new();
        self.gain = Line::new();
    }

    fn tick_lfo(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        let (inc, offset) = if p[8].deactivated {
            (0.0, ((p[8].v + 7.0) / 16.0).clamp(0.0, 1.0))
        } else {
            (rate_linear(-p[8].v.clamp(-8.0, 10.0), self.sr) * tsr_if(&p[8], ctx.tsr), 0.0)
        };
        let wave = p[7].i().clamp(0, 6);
        self.lfo.tick(wave, inc, p[9].v.clamp(0.0, 2.0), offset, 0.0);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.tick_lfo(p, ctx);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr as f64;
        self.tick_lfo(p, ctx);
        let drive01 = clamp01(p[0].v);
        self.wf.set(20.0 * clamp01(p[1].v) as f64);
        let wh = db_to_linear(drive01);
        self.wh.set(wh as f64);
        self.uf.set(5.0 * clamp01(p[2].v) as f64);
        self.uh.set(0.9 * clamp01(p[3].v) as f64);
        self.bf.set(6.0 * clamp01(p[4].v) as f64 - 1.0);
        let w = |semis: f32| {
            let hz = 440.0 * (semis as f64 / 12.0).exp2();
            0.5 * 4.0 * sr / (2.0 * std::f64::consts::PI * hz)
        };
        self.d[0].set(w(p[5].v));
        self.d[1].set(w(p[5].v + p[6].v));

        let mut ul = [0f32; 128];
        let mut ur = [0f32; 128];
        self.os.up(l, r, &mut ul, &mut ur);
        for k in 0..128 {
            let (wf, wh, uf, uh, bf) = (self.wf.next() as f32, self.wh.next() as f32, self.uf.next() as f32, self.uh.next() as f32, self.bf.next() as f32);
            let lfo = self.lfo.at(0, k);
            let ds = [self.d[0].next() as f32, self.d[1].next() as f32];
            for c in 0..2 {
                let x = if c == 0 { ul[k] } else { ur[k] };
                let y1 = self.y1[c];
                let f = sigmoid(wf * x + uf * y1 + bf);
                let y = f * y1 + (1.0 - f) * (wh * x + uh * f * y1).tanh();
                self.lines[c].write(y);
                self.y1[c] = self.lines[c].read_linear(ds[c] * (1.0 + 0.5 * lfo));
                if c == 0 {
                    ul[k] = y;
                } else {
                    ur[k] = y;
                }
            }
        }
        self.os.down(&mut ul, &mut ur, l, r);

        let omega = 35.0 / sr;
        self.dc.hp(omega, 0.707);
        self.dc.process(l, r);
        let bias = clamp01(p[4].v);
        self.makeup.set_target_smoothed(((-0.11898 * drive01).exp() + 1.0) * (6.0 * bias.powf(7.5) + 0.9));
        self.makeup.multiply_2_blocks(l, r);
        width_db_target(&mut self.width, p[10].v);
        apply_width(l, r, &self.width);
        self.gain.set_target_smoothed(db_to_linear(p[11].v));
        self.gain.multiply_2_blocks(l, r);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    #[test]
    fn finite_and_bounded() {
        let ctx = FxCtx::new(48000.0);
        let mut p = default_values(15);
        p[9].v = 1.0;
        p[5].v = 20.0;
        let mut fx = Neuron::new(48000.0);
        let mut rng = Rng::new(1);
        let mut peak = 0f32;
        for b in 0..400 {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let n = b * 32 + k;
                l[k] = if b < 200 { 0.8 * rng.bipolar() } else { 0.8 * (n as f32 * 0.05).sin() };
                r[k] = l[k];
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            for x in l.iter().chain(r.iter()) {
                assert!(x.is_finite());
                peak = peak.max(x.abs());
            }
        }
        assert!(peak > 1e-3 && peak < 10.0, "{peak}");
    }

    #[test]
    fn lin_smooth_ramps_in_n_steps() {
        let mut s = LinSmooth::new(4);
        s.set(1.0);
        s.set(2.0);
        let v: Vec<f64> = (0..5).map(|_| s.next()).collect();
        assert_eq!(v, vec![1.25, 1.5, 1.75, 2.0, 2.0]);
    }

    #[test]
    fn os4_passes_low_sine_at_quarter_gain() {
        let mut os = Os4::new();
        let mut peak = 0f32;
        for b in 0..100 {
            let mut l = [0f32; 32];
            for k in 0..32 {
                l[k] = (std::f32::consts::TAU * 500.0 * (b * 32 + k) as f32 / 48000.0).sin();
            }
            let r = l;
            let mut ul = [0f32; 128];
            let mut ur = [0f32; 128];
            os.up(&l, &r, &mut ul, &mut ur);
            let mut ol = [0f32; 32];
            let mut or = [0f32; 32];
            os.down(&mut ul, &mut ur, &mut ol, &mut or);
            if b > 20 {
                peak = ol.iter().fold(peak, |m, x| m.max(x.abs()));
            }
        }
        assert!((peak - 0.25).abs() < 0.01, "{peak}");
    }
}
