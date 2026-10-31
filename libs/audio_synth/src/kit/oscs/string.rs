//! String (type 9): two plucked delay-line strings excited by a burst
//! pre-filled into the lines or by a continuous signal, a tone filter in
//! each loop, a soft clip on the output, optional 2x internal oversampling.

use super::blit::kernel;
use super::{freq, CharFilter, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Biquad, HalfBand, Lag, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::PI;

pub const EXCITER_NAMES: [&str; 16] = [
    "Burst Noise",
    "Burst Pink Noise",
    "Burst Sine",
    "Burst Triangle",
    "Burst Ramp",
    "Burst Square",
    "Burst Sweep",
    "Constant Noise",
    "Constant Pink Noise",
    "Constant Sine",
    "Constant Triangle",
    "Constant Ramp",
    "Constant Square",
    "Constant Sweep",
    "Audio In",
    "Scene A",
];

pub const OS_1X: i32 = 2;
pub const OS_2X: i32 = 4;
pub const READ_ZOH: i32 = 8;
pub const READ_LINEAR: i32 = 16;
pub const READ_SINC: i32 = 32;
pub const FILTER_FIXED: i32 = 64;
pub const FILTER_KEYTRACK: i32 = 128;
pub const PITCH_COMPENSATE: i32 = 256;
pub const DEFORM_DEFAULT_LEVEL: i32 = OS_1X | READ_SINC;
pub const DEFORM_DEFAULT_STIFF: i32 = FILTER_FIXED;

/// The reference maps tone cutoffs as 2*pi*110*2^(c/12)/fs * 2*OS, which at
/// 2x string oversampling lands two octaves above the 1x cutoff; mirrored.
pub const QUIRK_STRING_TONE_CUTOFF_OS: bool = true;
/// The pink exciter's low-pass is designed for fs_os even at 2x string
/// oversampling (its cutoff scales with OS); mirrored.
pub const QUIRK_STRING_PINK_CUTOFF_OS: bool = true;

const LINE: usize = 16384;
const MAX_DELAY: f32 = 16284.0;

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef::choice("Exciter", &EXCITER_NAMES, 0.0),
        SlotDef {
            deform: &["1x oversampling", "2x oversampling", "zero-order hold", "linear", "windowed sinc"],
            about: "deform bits: 2 = 1x, 4 = 2x oversampling; 8 ZOH, 16 linear, 32 sinc read",
            ..SlotDef::float("Exciter Level", 0.0, 1.0, 1.0, SlotUnit::Percent)
        },
        SlotDef { can_extend: true, about: "extended: -1..1, negative inverts the loop", ..SlotDef::float("String 1 Decay", 0.0, 1.0, 0.95, SlotUnit::Percent) },
        SlotDef { can_extend: true, ..SlotDef::float("String 2 Decay", 0.0, 1.0, 0.95, SlotUnit::Percent) },
        SlotDef {
            can_extend: true,
            can_absolute: true,
            about: "string 2 offset, 1 = 100 cents (extended x12; absolute = Hz)",
            ..SlotDef::float("String 2 Detune", -1.0, 1.0, 0.1, SlotUnit::Percent)
        },
        SlotDef::float("String Balance", -1.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef {
            deform: &["fixed filter", "keytracked filter", "pitch compensation"],
            about: "deform bits: 64 fixed, 128 keytracked tone filter, 256 pitch compensation",
            ..SlotDef::float("Stiffness", -1.0, 1.0, 0.0, SlotUnit::Percent)
        },
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 18 {
        s[1].deform = DEFORM_DEFAULT_LEVEL;
        s[6].deform = DEFORM_DEFAULT_STIFF;
        s[2].extend = false;
        s[3].extend = false;
    }
}

/// Pitch compensation (semitones) for a stiffness value.
pub fn pitch_comp(st: f32) -> f32 {
    const NEG: [f32; 5] = [0.0591202, 0.122405, 0.225738, 0.406056, 0.7590243];
    const POS: [f32; 5] = [-0.02752047, -0.09026062, -0.31, -0.615, -0.87];
    let t = if st < 0.0 { &NEG } else { &POS };
    let x = (st.abs().min(1.0)) * 4.0;
    let i = (x.floor() as usize).min(3);
    let f = x - i as f32;
    t[i] * (1.0 - f) + t[i + 1] * f
}

fn decay_feedback(s: &SlotValue) -> f32 {
    let d = if s.extend { 2.0 * s.v - 1.0 } else { s.v };
    let m = d.abs().min(1.0);
    let fb = if m < 0.2 { 0.85 + 0.5 * m } else { 0.9375 + 0.0625 * m };
    if d < 0.0 { -fb } else { fb }
}

pub struct StringOsc {
    rng: Rng,
    drift: DriftLfo,
    line: [Vec<f32>; 2],
    w: usize,
    delay: [Lag; 2],
    fb: [Lag; 2],
    bal: Lag,
    level: Lag,
    fmd: Lag,
    lp: Biquad,
    hp: Biquad,
    pink: [Biquad; 2],
    phase: [f64; 2],
    os: usize,
    hb: HalfBand,
    chr: CharFilter,
    first: bool,
}

impl StringOsc {
    pub fn new(seed: u32) -> StringOsc {
        StringOsc {
            rng: Rng::new(seed),
            drift: DriftLfo::default(),
            line: [vec![0.0; LINE], vec![0.0; LINE]],
            w: 0,
            delay: [Lag::default(); 2],
            fb: [Lag::default(); 2],
            bal: Lag::default(),
            level: Lag::default(),
            fmd: Lag::default(),
            lp: Biquad::new(),
            hp: Biquad::new(),
            pink: [Biquad::new(); 2],
            phase: [0.0; 2],
            os: 1,
            hb: HalfBand::new(12, true),
            chr: CharFilter::default(),
            first: true,
        }
    }

    fn periods(&self, b: &OscBlock) -> (f32, f32) {
        let st = b.slots[6];
        let pc = if st.deform & PITCH_COMPENSATE != 0 { pitch_comp(st.v.clamp(-1.0, 1.0)) } else { 0.0 };
        let fs_os = b.fs_os;
        let p1 = (fs_os / freq(b.pitch + b.drift * self.drift.d + pc)).max(7.0);
        let det = &b.slots[4];
        let p2 = if det.absolute {
            let f = freq(b.pitch + pc) + det.v * 16.0 * if det.extend { 12.0 } else { 1.0 };
            fs_os / f.max(10.0)
        } else {
            fs_os / freq(b.pitch + b.ext12(4) + pc)
        };
        (p1, p2.max(7.0))
    }

    fn set_tone(&mut self, b: &OscBlock) {
        let st = b.slots[6];
        let tone = st.v.clamp(-1.0, 1.0);
        let keytrack = st.deform & FILTER_KEYTRACK != 0;
        let os = self.os as f64;
        let w = |c: f32| {
            // QUIRK_STRING_TONE_CUTOFF_OS
            2.0 * PI * 110.0 * (c as f64 / 12.0).exp2() / b.fs() as f64 * 2.0 * os
        };
        let (hp_c, lp_c) = if keytrack {
            if tone > 0.0 { (tone * 80.0 - 70.0 + b.pitch - 60.0, 100.0) } else { (-70.0, b.pitch - 40.0 * tone.abs()) }
        } else if tone > 0.0 {
            (tone * 130.0 - 70.0, 100.0)
        } else {
            (-70.0, 100.0 - 90.0 * tone.abs())
        };
        self.hp.hp(w(hp_c), 0.707);
        self.lp.lp(w(lp_c), 0.707);
    }

    fn excite(&mut self, ex: i32, ch: usize, inc: f64) -> f32 {
        let ph = &mut self.phase[ch];
        let p = *ph;
        *ph += inc;
        if *ph >= 1.0 {
            *ph -= ph.floor();
        }
        match ex % 7 {
            0 => self.rng.bipolar(),
            1 => {
                let n = self.rng.bipolar();
                // Pink: uniform noise through the 2-pole low-pass, x1.7.
                self.pink[ch].tick_mono(n) * 1.7
            }
            2 => 0.707 * (2.0 * PI * p).sin() as f32,
            3 => 0.707 * if p < 0.5 { 4.0 * p as f32 - 1.0 } else { 4.0 * (1.0 - p as f32) - 1.0 },
            4 => 0.707 * (2.0 * p as f32 - 1.0),
            5 => 0.707 * if p > 0.5 { 1.0 } else { -1.0 },
            _ => {
                if p <= 0.0 {
                    1.0
                } else {
                    0.707 * (2.0 * PI / p).sin() as f32
                }
            }
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.drift.init(&mut self.rng, nonzero_drift);
        self.os = if b.slots[1].deform & OS_2X != 0 { 2 } else { 1 };
        for l in self.line.iter_mut() {
            l.fill(0.0);
        }
        self.w = 0;
        self.hb = HalfBand::new(12, true);
        self.chr.reset();
        self.lp.suspend();
        self.hp.suspend();
        for p in self.pink.iter_mut() {
            p.suspend();
        }
        let os = self.os as f32;
        // QUIRK_STRING_PINK_CUTOFF_OS
        let pink_w = 2.0 * PI * 1760.0 / b.fs_os as f64;
        for p in self.pink.iter_mut() {
            p.lp2b(pink_w, 0.9);
        }
        self.set_tone(b);
        self.lp.instantize();
        self.hp.instantize();
        let (p1, p2) = self.periods(b);
        for (k, p) in [p1, p2].iter().enumerate() {
            self.delay[k] = Lag::default();
            self.delay[k].snap((os * p).min(MAX_DELAY));
            self.fb[k] = Lag::default();
            self.fb[k].snap(decay_feedback(&b.slots[2 + k]));
        }
        self.bal = Lag::default();
        self.bal.snap(0.5 * b.slots[5].v + 0.5);
        self.level = Lag::default();
        self.fmd = Lag::default();
        self.phase = if b.retrigger { [0.0; 2] } else { [self.rng.unit() as f64, self.rng.unit() as f64] };
        let ex = b.slots[0].i().clamp(0, 15);
        let lv = b.slots[1].v.clamp(0.0, 1.0);
        let tone = b.slots[6].v.clamp(-1.0, 1.0);
        if ex < 14 {
            let n = ((10.0 * p1.max(p2) * os).floor() as usize).min(LINE - 1);
            let gain = if ex < 7 { 1.0 } else { lv * lv * lv * lv };
            let incs = [super::dphase(b.pitch, b.fs_os) / os as f64, 1.0 / p2 as f64 / os as f64];
            for i in 0..n {
                let mut x = [0.0f32; 2];
                for (ch, xv) in x.iter_mut().enumerate() {
                    *xv = self.excite(ex, ch, incs[ch]) * gain;
                }
                let (fl, fr) = if tone < 0.0 { self.lp.tick(x[0], x[1]) } else { self.hp.tick(x[0], x[1]) };
                self.line[0][i] = fl;
                self.line[1][i] = fr;
            }
            self.w = n;
            self.lp.suspend();
            self.hp.suspend();
            self.set_tone(b);
        }
        self.first = false;
    }

    #[inline]
    fn read(line: &[f32], w: usize, delay: f32, mode: i32) -> f32 {
        let d = delay.floor();
        let frac = delay - d;
        let d = d as usize;
        let at = |o: usize| line[(w + LINE * 2 - o) % LINE];
        if mode & READ_SINC != 0 {
            let k = kernel();
            let j = (((1.0 - frac) * 256.0) as usize).min(256);
            let h = &k.h[j];
            let mut acc = 0.0;
            // taps at w - d - 6 + i, i = 0..11
            for (i, hv) in h.iter().enumerate() {
                acc += hv * line[(w + LINE * 2 + i - d - 6) % LINE];
            }
            acc
        } else if mode & READ_LINEAR != 0 {
            (1.0 - frac) * at(d) + frac * at(d + 1)
        } else {
            at(d + 1)
        }
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        self.drift.next(&mut self.rng);
        self.set_tone(b);
        let os = self.os;
        let (p1, p2) = self.periods(b);
        self.delay[0].set((os as f32 * p1).min(MAX_DELAY));
        self.delay[1].set((os as f32 * p2).min(MAX_DELAY));
        self.fb[0].set(decay_feedback(&b.slots[2]));
        self.fb[1].set(decay_feedback(&b.slots[3]));
        self.bal.set(0.5 * b.slots[5].v.clamp(-1.0, 1.0) + 0.5);
        let ex = b.slots[0].i().clamp(0, 15);
        let lv = b.slots[1].v.clamp(0.0, 1.0);
        let e = if ex >= 7 && ex < 14 {
            lv * lv * lv * lv
        } else if ex < 7 {
            if lv < 0.1 { lv * 5.6234 } else { lv.powf(0.25) }
        } else {
            lv * lv * lv * lv
        };
        self.level.set(e);
        self.fmd.set(if b.fm { b.fmdepth / 16.0 } else { 0.0 });
        let mode = b.slots[1].deform;
        let tone_pos = b.slots[6].v > 0.0;
        let incs = [super::dphase(b.pitch, b.fs_os) / os as f64, 1.0 / p2 as f64 / os as f64];
        let n = 64 * os;
        let mut out = [0.0f32; 128];
        for (s, o) in out.iter_mut().enumerate().take(n) {
            let d1 = self.delay[0].process();
            let d2 = self.delay[1].process();
            let fb1 = self.fb[0].process();
            let fb2 = self.fb[1].process();
            let bal = self.bal.process();
            let e = self.level.process();
            let fmd = self.fmd.process();
            let mut dl = [d1, d2];
            if b.fm {
                let m = (3.0 * fmd * b.fm_buf[s / os]).clamp(-6.0, 4.0).exp();
                dl[0] = (dl[0] * m).clamp(2.0, MAX_DELAY);
                dl[1] = (dl[1] * m).clamp(2.0, MAX_DELAY);
            }
            let mut x = [Self::read(&self.line[0], self.w, dl[0], mode), Self::read(&self.line[1], self.w, dl[1], mode)];
            let mut inject = [0.0f32; 2];
            if ex < 7 {
                x[0] *= e;
                x[1] *= e;
            } else if ex < 14 {
                for ch in 0..2 {
                    x[ch] += e * self.excite(ex, ch, incs[ch]);
                }
            } else if ex == 14 {
                if let Some((il, ir)) = b.audio_in {
                    inject = [e * il[s / os], e * ir[s / os]];
                }
            }
            let f0 = (x[0] + inject[0]).clamp(-1.0, 1.0);
            let f1 = (x[1] + inject[1]).clamp(-1.0, 1.0);
            let (lo, hi) = (self.lp.tick(f0, f1), self.hp.tick(f0, f1));
            let y = if tone_pos { hi } else { lo };
            let mut wv = [y.0 * fb1, y.1 * fb2];
            for v in wv.iter_mut() {
                if v.abs() < 1e-16 {
                    *v = 0.0;
                }
            }
            self.line[0][self.w] = wv[0];
            self.line[1][self.w] = wv[1];
            self.w = (self.w + 1) % LINE;
            let mut o2 = x[0] + bal * (x[1] - x[0]);
            o2 = o2 * (1.5 - 0.5 * o2 * o2);
            *o = o2;
        }
        if os == 2 {
            let mut ol = [0.0f32; 128];
            let mut orr = [0.0f32; 128];
            ol.copy_from_slice(&out);
            orr.copy_from_slice(&out);
            self.hb.decimate(&mut ol, &mut orr);
            l.copy_from_slice(&ol[..64]);
        } else {
            l.copy_from_slice(&out[..64]);
        }
        *r = *l;
        self.chr.process(b.character, b.fs(), b.stereo, l, r);
    }
}
