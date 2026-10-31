//! Combulator (type 21): three parallel feedback combs (a centre pitch and
//! two offsets or absolute pitches), 2x oversampled, with an input-
//! following noise exciter, per-comb level and pan, a soft clip on the sum,
//! an optional tone tilt and a wet/dry mix.

use super::common::*;
use crate::kit::flt::FilterUnit;
use super::FxCtx;
use crate::dsp::Rng;
use crate::kit::core::{calc_omega, Biquad, HalfBand, Lag, SlotDef, SlotUnit, SlotValue};
use std::sync::OnceLock;

/// 0.5 Hz in semitones vs A440.
const F_MIN: f32 = -117.376;
const F_MAX: f32 = 70.0;
/// Comb 1's pan gain on both sides (and the normaliser of every comb).
const CENTER_PAN: f32 = 0.59;

pub fn slot_defs() -> [SlotDef; 12] {
    let gain = |name| fl(name, 0.0, 1.0, 0.7937, SlotUnit::Amp);
    let off = |name, def| {
        about(
            extend(fl(name, -60.0, 60.0, def, SlotUnit::Semis)),
            "not extended: semitone offset from Center; extended: absolute frequency (note vs A440, 0.5 Hz..25 kHz)",
        )
    };
    [
        percent("Extra Noise", 0.0),
        about(
            extend(fl("Center", F_MIN, F_MAX, -9.0, SlotUnit::NoteA440)),
            "named Frequency 1 when both offsets are extended; extended = tuning-aware",
        ),
        off("Offset 2", 0.25),
        off("Offset 3", -0.25),
        bipolar("Feedback", 0.75),
        deact(bipolar("Tone", 0.0)),
        gain("Comb 1"),
        gain("Comb 2"),
        gain("Comb 3"),
        bipolar("Pan 2", 0.25),
        bipolar("Pan 3", -0.25),
        percent("Mix", 1.0),
    ]
}

/// SPEC? the default deactivation state of Tone is not given; the
/// migration (old patches: active) suggests active.
pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[5].deactivated = false;
}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 17 {
        p[5].deactivated = false;
    }
    if rev <= 20 {
        p[2].extend = false;
        p[3].extend = false;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// (L, R) gain of a pan position from the 4096-entry table.
fn pan_gains(pan: f32) -> [f32; 2] {
    static T: OnceLock<Vec<[f32; 2]>> = OnceLock::new();
    let t = T.get_or_init(|| {
        let h = std::f64::consts::FRAC_PI_2;
        (0..4096)
            .map(|i| {
                let a = i as f64 / 4095.0 * h;
                [((h - a) / h * a.cos()).sqrt() as f32, (a * a.sin() / h).sqrt() as f32]
            })
            .collect()
    });
    t[(((pan + 1.0) * 2047.0) as i32 & 4095) as usize]
}

pub struct Combulator {
    sr: f32,
    up: HalfBand,
    down: HalfBand,
    combs: [[FilterUnit; 3]; 2],
    f: [Lag; 3],
    g: [Lag; 3],
    noise: Lag,
    fb: Lag,
    tone: Lag,
    pan: [Lag; 2],
    env: [f32; 2],
    env_c: f32,
    rng: Rng,
    lp: Biquad,
    hp: Biquad,
    mix: Line,
}

impl Combulator {
    pub fn new(sr: f32) -> Self {
        let mut combs: [[FilterUnit; 3]; 2] = Default::default();
        combs.iter_mut().flatten().for_each(|u| u.set_long_comb(true));
        Combulator {
            sr,
            up: HalfBand::new(12, true),
            down: HalfBand::new(12, true),
            combs,
            f: [Lag::new(0.004); 3],
            g: [Lag::new(0.004); 3],
            noise: Lag::new(0.004),
            fb: Lag::new(0.004),
            tone: Lag::new(0.004),
            pan: [Lag::new(0.004); 2],
            env: [0.0; 2],
            env_c: 0.01f32.powf(1.0 / (0.005 * 2.0 * sr)),
            rng: Rng::new(0x5eed_c0b),
            lp: Biquad::new(),
            hp: Biquad::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.up = HalfBand::new(12, true);
        self.down = HalfBand::new(12, true);
        self.combs.iter_mut().flatten().for_each(|u| u.reset());
        self.f = [Lag::new(0.004); 3];
        self.g = [Lag::new(0.004); 3];
        self.noise = Lag::new(0.004);
        self.fb = Lag::new(0.004);
        self.tone = Lag::new(0.004);
        self.pan = [Lag::new(0.004); 2];
        self.env = [0.0; 2];
        self.lp.suspend();
        self.hp.suspend();
        self.mix = Line::new();
    }

    fn targets(&mut self, p: &[SlotValue; 12]) {
        let f1 = p[1].v.clamp(F_MIN, F_MAX);
        self.f[0].set(f1);
        for (i, s) in [(1usize, &p[2]), (2, &p[3])] {
            let f = if s.extend { s.v.clamp(F_MIN, F_MAX) } else { f1 + s.v.clamp(-60.0, 60.0) };
            self.f[i].set(f);
        }
        for i in 0..3 {
            let g = p[6 + i].v.clamp(0.0, 2.0);
            self.g[i].set(g * g * g);
        }
        self.noise.set(clamp01(p[0].v));
        self.fb.set(p[4].v);
        self.tone.set(p[5].v.clamp(-1.0, 1.0));
        self.pan[0].set(p[9].v.clamp(-1.0, 1.0));
        self.pan[1].set(p[10].v.clamp(-1.0, 1.0));
    }

    fn step_lags(&mut self) {
        for i in 0..3 {
            self.f[i].process();
            self.g[i].process();
        }
        self.noise.process();
        self.fb.process();
        self.tone.process();
        self.pan[0].process();
        self.pan[1].process();
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.targets(p);
        let fb = self.fb.v;
        let reso = fb.signum() * fb.abs().sqrt();
        let fs = 2.0 * self.sr;
        for ch in 0..2 {
            for i in 0..3 {
                // 12-TET: the tuning-aware path (Center extended) is identity here.
                self.combs[ch][i].set_block(8, 1, self.f[i].v, reso, fs, true);
            }
        }
        let mut ol = [0f32; 64];
        let mut or = [0f32; 64];
        // SPEC? interpolator gain compensation unspecified (see Resonator).
        self.up.upsample_into(&l[..], &r[..], &mut ol, &mut or, 0.5);
        for k in 0..B {
            self.step_lags();
            let g = [self.g[0].v, self.g[1].v, self.g[2].v];
            let p2 = pan_gains(self.pan[0].v);
            let p3 = pan_gains(self.pan[1].v);
            let amount = self.noise.v;
            for j in [2 * k, 2 * k + 1] {
                for ch in 0..2 {
                    let buf = if ch == 0 { &mut ol } else { &mut or };
                    let v = buf[j];
                    self.env[ch] = self.env_c * (self.env[ch] - v) + v;
                    let x = v + amount * 3.0 * self.env[ch] * self.rng.bipolar();
                    let side = [CENTER_PAN, p2[ch], p3[ch]];
                    let mut mix = 0.0;
                    for i in 0..3 {
                        mix += self.combs[ch][i].tick(x) * g[i] * side[i] / CENTER_PAN;
                    }
                    buf[j] = lookup_waveshape(WS_SOFT, mix);
                }
            }
        }
        let mut wl = [0f32; 32];
        let mut wr = [0f32; 32];
        self.down.decimate_into(&ol, &or, &mut wl, &mut wr);
        if !p[5].deactivated {
            let (lpc, hpc) = tone_cuts(self.tone.v);
            self.lp.lp(calc_omega(lpc / 12.0 - 2.0, self.sr), 0.707);
            self.hp.hp(calc_omega(hpc / 12.0 - 2.0, self.sr), 0.707);
            self.lp.process(&mut wl, &mut wr);
            self.hp.process(&mut wl, &mut wr);
        }
        self.mix.set_target_smoothed(clamp01(p[11].v));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.targets(p);
        for _ in 0..B {
            self.step_lags();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(hz: f64) -> f64 {
        let p = super::super::default_values(21);
        let ctx = FxCtx::new(48000.0);
        let mut fx = Combulator::new(48000.0);
        let (mut sc, mut cc) = (0.0f64, 0.0f64);
        let blocks = 3000;
        for b in 0..blocks {
            let mut l = [0f32; 32];
            for k in 0..32 {
                let n = (b * 32 + k) as f64;
                l[k] = (0.01 * (2.0 * std::f64::consts::PI * hz * n / 48000.0).sin()) as f32;
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            if b >= blocks / 2 {
                for k in 0..32 {
                    assert!(l[k].is_finite());
                    let ph = 2.0 * std::f64::consts::PI * hz * (b * 32 + k) as f64 / 48000.0;
                    sc += l[k] as f64 * ph.sin();
                    cc += l[k] as f64 * ph.cos();
                }
            }
        }
        (sc * sc + cc * cc).sqrt()
    }

    #[test]
    fn resonates_at_center() {
        let f1 = 440.0 * 2f64.powf(-9.0 / 12.0);
        let peak = gain_at(f1);
        let trough = gain_at(1.5 * f1);
        assert!(peak > 3.0 * trough, "{peak} {trough}");
    }

    #[test]
    fn pan_table_ends() {
        let c = pan_gains(0.0);
        assert!((c[0] - c[1]).abs() < 0.01, "{c:?}");
        let l = pan_gains(-1.0);
        assert!(l[0] > 0.99 && l[1] < 1e-6, "{l:?}");
    }
    #[test]
    fn negative_feedback_stays_negative() {
        let mut p = super::super::default_values(21);
        p[4].v = -0.64;
        let ctx = FxCtx::new(48000.0);
        let mut fx = Combulator::new(48000.0);
        for _ in 0..4000 {
            let (mut l, mut r) = ([0f32; 32], [0f32; 32]);
            fx.process(&p, &ctx, &mut l, &mut r);
        }
        // resonance = sign(fb) sqrt(|fb|) = -0.8, not clamped to 0 by the Comb + rule.
        let c = fx.combs[0][0].c[1];
        assert!((c + 0.8).abs() < 1e-3, "{c}");
    }
}
