//! Flanger (type 12): up to four modulated comb taps per channel on one
//! delay line, tuned to a base pitch plus a spacing; in the arpeggio modes a
//! slow long phase walks the four combs in turn. Damped feedback, a cubic
//! output saturation, width and a bipolar mix.

use super::common::*;
use super::FxCtx;
use crate::dsp::Rng;
use crate::kit::core::{db_to_linear, Lag, SlotDef, SlotUnit, SlotValue};

const LEN: usize = 32768;
const MASK: usize = LEN - 1;

/// The reference's noise wave draws its target as U(0,1) - 1 (always in
/// [-1, 0]); mirrored so patches using it sound the same.
const QUIRK_NOISE_NEGATIVE_ONLY: bool = true;
const _: () = assert!(QUIRK_NOISE_NEGATIVE_ONLY);
/// The reference's feedback scaling for mode 2 falls through into mode 3's
/// case, so mode 2 gets +0.3 and then +0.5.
const QUIRK_MODE2_FALLS_THROUGH: bool = true;

const MODES: [&str; 4] = ["Dry + Combs", "Combs Only", "Dry + Arp Combs", "Arp Combs Only"];
const WAVES: [&str; 6] = ["Sine", "Triangle", "Sawtooth", "Noise", "Sample & Hold", "Square"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        choice("Mode", &MODES, 0.0),
        choice("Waveform", &WAVES, 0.0),
        lforate("Rate", -2.0),
        percent("Depth", 1.0),
        fl("Count", 1.0, 4.0, 4.0, SlotUnit::None),
        fl("Base Pitch", 0.0, 127.0, 60.0, SlotUnit::MidiNote),
        fl("Spacing", 0.0, 12.0, 0.0, SlotUnit::Semis),
        percent("Feedback", 0.0),
        percent("HF Damping", 0.1),
        width_db("Width"),
        bipolar("Mix", 0.8),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(p: &[SlotValue; 12], sr: f32) -> i32 {
    if p[7].v > 0.0 {
        (32.0 * sr) as i32
    } else {
        1024
    }
}

pub struct Flanger {
    sr: f32,
    line: [Vec<f32>; 2],
    w: usize,
    ph: [[f32; 4]; 2],
    long: [f32; 2],
    v0: Lag,
    first: bool,
    lfo: [[Line; 4]; 2],
    noise_t: [[f32; 4]; 2],
    noise_v: [[f32; 4]; 2],
    dbase: [Line; 4],
    weights: [[f32; 4]; 2],
    depth: f32,
    count: f32,
    mode: i32,
    fb: Line,
    damp: Line,
    hp: [f32; 2],
    mix: Line,
    width: Line,
    rng: Rng,
}

impl Flanger {
    pub fn new(sr: f32) -> Self {
        let mut ph = [[0f32; 4]; 2];
        for (c, row) in ph.iter_mut().enumerate() {
            for (i, v) in row.iter_mut().enumerate() {
                *v = (i as f32 + 0.5 * c as f32) / 4.0;
            }
        }
        Flanger {
            sr,
            line: [vec![0.0; LEN], vec![0.0; LEN]],
            w: 0,
            ph,
            long: [0.0, 0.5],
            v0: Lag::new(0.004),
            first: true,
            lfo: [[Line::new(); 4]; 2],
            noise_t: [[0.0; 4]; 2],
            noise_v: [[0.0; 4]; 2],
            dbase: [Line::new(); 4],
            weights: [[0.0; 4]; 2],
            depth: 0.0,
            count: 4.0,
            mode: 0,
            fb: Line::new(),
            damp: Line::new(),
            hp: [0.0; 2],
            mix: Line::new(),
            width: Line::new(),
            rng: Rng::new(0xf1a9e),
        }
    }

    pub fn reset(&mut self) {
        for l in &mut self.line {
            l.iter_mut().for_each(|s| *s = 0.0);
        }
        self.w = 0;
        for (c, row) in self.ph.iter_mut().enumerate() {
            for (i, v) in row.iter_mut().enumerate() {
                *v = (i as f32 + 0.5 * c as f32) / 4.0;
            }
        }
        self.long = [0.0, 0.5];
        self.v0 = Lag::new(0.004);
        self.first = true;
        self.lfo = [[Line::new(); 4]; 2];
        self.noise_t = [[0.0; 4]; 2];
        self.noise_v = [[0.0; 4]; 2];
        self.dbase = [Line::new(); 4];
        self.fb = Line::new();
        self.damp = Line::new();
        self.hp = [0.0; 2];
        self.mix = Line::new();
        self.width = Line::new();
    }

    fn control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        let sr = self.sr;
        let r = rate_linear(-p[2].v.clamp(-8.0, 10.0), sr) * tsr_if(&p[2], ctx.tsr);
        for lp in self.long.iter_mut() {
            *lp += r;
            *lp -= 4.0 * (*lp / 4.0).floor();
        }
        if self.first {
            self.v0.snap(p[5].v);
            self.first = false;
        } else {
            self.v0.set(p[5].v);
        }
        self.v0.process();
        self.v0.process();
        self.mode = p[0].i().clamp(0, 3);
        let arp = self.mode >= 2;
        let wave = p[1].i().clamp(0, 5);
        for c in 0..2 {
            for i in 0..4 {
                let mut reset = false;
                self.ph[c][i] += r;
                if self.ph[c][i] > 1.0 {
                    self.ph[c][i] -= 1.0;
                    reset = true;
                }
                let lp = if arp { self.long[c] - self.long[c].floor() } else { self.ph[c][i] };
                if reset && (wave == 3 || wave == 4) {
                    self.noise_t[c][i] = self.rng.unit() - 1.0;
                }
                let v = match wave {
                    0 => sine_turns(lp),
                    1 => 2.0 * (2.0 * lp - 1.0).abs() - 1.0,
                    2 => ramp(lp),
                    3 => {
                        self.noise_v[c][i] += (self.noise_t[c][i] - self.noise_v[c][i]) * 2.0 * r;
                        self.noise_v[c][i]
                    }
                    4 => self.noise_t[c][i],
                    _ => {
                        // SPEC? p = 0.49 exactly falls in no listed range; it is
                        // taken into the falling edge here.
                        if lp < 0.49 {
                            1.0
                        } else if lp < 0.51 {
                            -100.0 * lp + 50.0
                        } else if lp <= 0.98 {
                            -1.0
                        } else {
                            100.0 * lp - 99.0
                        }
                    }
                };
                self.lfo[c][i].set_target(v);
            }
        }
        self.depth = p[3].v;
        self.count = p[4].v.clamp(1.0, 4.0);
        let mut mean = 0.0;
        for i in 0..4 {
            // 12-TET: the reference uses the active tuning here.
            let pitch = self.v0.v + p[6].v * i as f32;
            let mut d = sr / (8.1758 * (pitch / 12.0).exp2());
            let top = d * (1.0 + self.depth) + 1.0;
            if top >= LEN as f32 {
                d *= 0.999 * LEN as f32 / top;
            }
            self.dbase[i].set_target(d);
            mean += 0.25 * d;
        }
        for c in 0..2 {
            let w = &mut self.weights[c];
            *w = [0.0; 4];
            if arp {
                let k = (self.long[c].floor() as usize) & 3;
                let f = self.long[c] - self.long[c].floor();
                if f > 0.9 {
                    let dt = (f - 0.9) * 10.0;
                    w[k] = (1.0 - dt).sqrt();
                    w[(k + 1) & 3] = dt.sqrt();
                } else {
                    w[k] = 1.0;
                }
            } else {
                let whole = self.count.floor() as usize;
                for v in w.iter_mut().take(whole.min(4)) {
                    *v = 1.0;
                }
                if whole < 4 {
                    w[whole] = self.count - whole as f32;
                }
            }
        }
        let da = r * sr / B as f32 * mean * self.depth;
        let mut fs = 0.4 * ((da.clamp(2.0, 60.0) + 30.0) / 100.0).sqrt();
        match self.mode {
            0 | 1 => fs += (3.0 - (self.count - 1.0)) * 0.45 / 3.0,
            2 => {
                fs += 0.3;
                if QUIRK_MODE2_FALLS_THROUGH {
                    fs += 0.5;
                }
            }
            _ => fs += 0.5,
        }
        if wave == 2 || wave == 4 {
            fs *= 0.7;
        }
        let mut fb = p[7].v;
        if fb > 0.0 && fb < 1.0 {
            fb = fb.sqrt();
        }
        self.fb.set_target(fs * fb);
        self.damp.set_target(0.4 * p[8].v);
        self.mix.set_target_smoothed(p[10].v);
        self.width.set_target_smoothed(db_to_linear(p[9].v) / 3.0);
    }

    #[inline]
    fn read(&self, c: usize, d: f32) -> f32 {
        let t = d.floor();
        let f = d - t;
        let t = t as usize;
        let a = self.line[c][(self.w + LEN - t) & MASK];
        let b = self.line[c][(self.w + LEN - t - 1) & MASK];
        b * f + a * (1.0 - f)
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.control(p, ctx);
        let dryw = if self.mode == 1 || self.mode == 3 { 0.0 } else { 1.0 };
        let g = match self.mode {
            0 => -1.0 / (7.0 - self.count).sqrt(),
            1 => -1.0 / (8.0 - self.count).sqrt(),
            2 => -1.0 / 6f32.sqrt(),
            _ => -1.0 / 7f32.sqrt(),
        };
        let maxd = (LEN - 2) as f32;
        for k in 0..B {
            let mix = self.mix.at(k);
            let gk = g - 0.07 * mix;
            let fbk = self.fb.at(k);
            let dmp = self.damp.at(k).clamp(0.01, 0.99);
            for c in 0..2 {
                let x = if c == 0 { l[k] } else { r[k] };
                let mut comb = 0.0;
                for i in 0..4 {
                    let wi = self.weights[c][i];
                    if wi != 0.0 {
                        let d = (self.dbase[i].at(k) * (1.0 + self.lfo[c][i].at(k) * self.depth) + 1.0).clamp(1.0, maxd);
                        comb += wi * self.read(c, d);
                    }
                }
                let f = if fbk > 0.0 {
                    let f = cubic_sat((fbk * comb).clamp(-1.0, 1.0));
                    self.hp[c] = self.hp[c] * (1.0 - dmp) + f * dmp;
                    f - self.hp[c]
                } else {
                    0.0
                };
                self.line[c][self.w] = x - f;
                let out = ((1.0 + gk) * (dryw * x + mix * comb)).clamp(-1.0, 1.0);
                let out = cubic_sat(out);
                if c == 0 {
                    l[k] = out;
                } else {
                    r[k] = out;
                }
            }
            self.w = (self.w + 1) & MASK;
        }
        apply_width(l, r, &self.width);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.control(p, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comb_delay_at_base_pitch() {
        let mut p = super::super::default_values(12);
        p[3].v = 0.0; // depth
        p[4].v = 1.0; // one comb
        p[10].v = 1.0;
        let ctx = FxCtx::new(48000.0);
        let mut fx = Flanger::new(48000.0);
        let mut out = Vec::new();
        for b in 0..20 {
            let mut l = [0f32; 32];
            if b == 0 {
                l[0] = 0.01;
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
            out.extend_from_slice(&l);
        }
        let (best, _) = out.iter().enumerate().skip(1).fold((0, 0.0f32), |a, (i, v)| if v.abs() > a.1 { (i, v.abs()) } else { a });
        let d = 48000.0 / (8.1758 * 2f32.powf(60.0 / 12.0)) + 1.0;
        assert!((best as f32 - d).abs() <= 1.0, "{best} vs {d}");
    }

    #[test]
    fn defaults_stay_finite_and_bounded() {
        let p = super::super::default_values(12);
        let ctx = FxCtx::new(48000.0);
        let mut fx = Flanger::new(48000.0);
        let mut rng = Rng::new(3);
        for _ in 0..2000 {
            let mut l = [0f32; 32];
            for v in l.iter_mut() {
                *v = 0.5 * rng.bipolar();
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite() && v.abs() <= 1.5));
        }
    }
}
