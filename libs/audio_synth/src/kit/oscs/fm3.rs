//! FM3 (type 5): three sine modulators (two ratio / fixed, one fixed
//! frequency) phase-modulating a sine carrier, with feedback. Mono.

use super::sine::feedback_def;
use super::{feedback, omega, wrap_pi, CentredRamp, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::PI;

pub fn ratio_def(name: &'static str) -> SlotDef {
    SlotDef {
        can_extend: true,
        can_absolute: true,
        about: "modulator ratio; extended 1/32..32, absolute = fixed note 69 + 69 (v - 16) / 16",
        ..SlotDef::float(name, 0.0, 32.0, 1.0, SlotUnit::None)
    }
}

/// M3 Frequency's minimum (8.18 Hz), and the extended one (0.30 Hz).
pub const M3_MIN: f32 = -60.0;
pub const M3_MIN_EXTENDED: f32 = -117.3763;

/// M3's note offset from middle C: extend only lowers the minimum (the rate
/// formula is unchanged); not extended clamps below -60 up to -60.
pub fn m3_offset(s: &SlotValue) -> f32 {
    s.v.clamp(if s.extend { M3_MIN_EXTENDED } else { M3_MIN }, 70.0)
}

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef::float("M1 Amount", 0.0, 1.0, 0.0, SlotUnit::Percent),
        ratio_def("M1 Ratio"),
        SlotDef::float("M2 Amount", 0.0, 1.0, 0.0, SlotUnit::Percent),
        ratio_def("M2 Ratio"),
        SlotDef::float("M3 Amount", 0.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef {
            can_extend: true,
            about: "semitones vs middle C; extended lowers the minimum to -117.3763 (0.30 Hz)",
            ..SlotDef::float("M3 Frequency", M3_MIN, 70.0, 0.0, SlotUnit::Semis)
        },
        feedback_def(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7], retrigger: &mut bool) {
    if rev <= 13 {
        s[1].absolute = false;
        s[3].absolute = false;
    }
    if rev <= 15 {
        *retrigger = true;
    }
    if rev <= 21 {
        s[5].extend = false;
        s[6].deform = 0;
    }
}

/// A ratio slot's ratio, or the fixed note when absolute.
pub enum Ratio {
    Mul(f32),
    Note(f32),
}

pub fn decode_ratio(s: &SlotValue) -> Ratio {
    let v = s.v;
    if s.absolute {
        return Ratio::Note(69.0 + 69.0 * (v - 16.0) / 16.0);
    }
    if !s.extend {
        return Ratio::Mul(v);
    }
    let m = if v > 16.0 { (v - 16.0) * 31.0 / 16.0 + 1.0 } else { -((16.0 - v) * 31.0 / 16.0 + 1.0) };
    Ratio::Mul(if m < 0.0 { 1.0 / m.abs() } else { m })
}

/// A sine modulator whose rate ramps across the block.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modulator {
    pub ph: f64,
    pub ramp: CentredRamp,
}

#[derive(Clone, Copy, Debug)]
pub struct Feedback {
    mag: Lag,
    last: [f32; 2],
}

impl Default for Feedback {
    fn default() -> Self {
        Feedback { mag: Lag::default(), last: [0.0; 2] }
    }
}

impl Feedback {
    pub fn reset(&mut self) {
        *self = Feedback::default();
    }
    /// Per-sample term, `fb` the signed slot value (x4 when extended).
    #[inline]
    pub fn term(&mut self, fb: f32, avg: bool) -> f32 {
        let m = self.mag.process();
        let lv = if avg { 0.5 * (self.last[0] + self.last[1]) } else { self.last[0] };
        m * if fb < 0.0 { lv * lv } else { lv }
    }
    #[inline]
    pub fn set(&mut self, fb: f32) {
        self.mag.set(fb.abs());
    }
    #[inline]
    pub fn push(&mut self, out: f32) {
        self.last[1] = self.last[0];
        self.last[0] = out;
    }
}

pub struct Fm3 {
    rng: Rng,
    drift: DriftLfo,
    phase: f64,
    w: CentredRamp,
    m: [Modulator; 3],
    d: [Lag; 3],
    fmd: Lag,
    fb: Feedback,
    first: bool,
}

impl Fm3 {
    pub fn new(seed: u32) -> Fm3 {
        Fm3 {
            rng: Rng::new(seed),
            drift: DriftLfo::default(),
            phase: 0.0,
            w: CentredRamp::default(),
            m: [Modulator::default(); 3],
            d: [Lag::default(); 3],
            fmd: Lag::default(),
            fb: Feedback::default(),
            first: true,
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.drift.init(&mut self.rng, nonzero_drift);
        self.phase = if b.retrigger { 0.0 } else { (2.0 * self.rng.unit() as f64 - 1.0) * PI };
        for m in self.m.iter_mut() {
            *m = Modulator { ph: self.phase, ramp: CentredRamp::default() };
        }
        self.w.reset();
        self.d = [Lag::default(); 3];
        self.fmd = Lag::default();
        self.fb.reset();
        self.first = false;
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], _r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        let fs_os = b.fs_os;
        let note = b.pitch + b.drift * self.drift.next(&mut self.rng);
        let wc = omega(note, fs_os).min(PI);
        let mut rates = [0.0f64; 3];
        for (k, slot) in [1usize, 3].iter().enumerate() {
            rates[k] = match decode_ratio(&b.slots[*slot]) {
                Ratio::Mul(r) => (omega(note, fs_os) * r as f64).min(PI),
                Ratio::Note(n) => omega(n, fs_os).min(PI),
            };
        }
        rates[2] = omega(60.0 + m3_offset(&b.slots[5]), fs_os).min(PI);
        let pi32 = 32.0 * std::f32::consts::PI;
        for (k, slot) in [0usize, 2, 4].iter().enumerate() {
            let a = b.slots[*slot].v;
            self.d[k].set(pi32 * a * a * a);
        }
        let fd = if b.fm { pi32 * b.fmdepth * b.fmdepth * b.fmdepth } else { 0.0 };
        self.fmd.set(fd);
        let fbv = feedback(&b.slots[6]);
        self.fb.set(fbv);
        let avg = b.slots[6].deform == 1;
        let (mut w, dw) = self.w.set(wc);
        let mut mw = [(0.0f64, 0.0f64); 3];
        for k in 0..3 {
            mw[k] = self.m[k].ramp.set(rates[k]);
        }
        for s in 0..64 {
            let mut x = self.phase;
            for k in 0..3 {
                let d = self.d[k].process() as f64;
                self.m[k].ph += mw[k].0;
                mw[k].0 += mw[k].1;
                if self.m[k].ph > PI {
                    self.m[k].ph -= 2.0 * PI;
                }
                x += d * self.m[k].ph.sin();
            }
            x += self.fb.term(fbv, avg) as f64;
            x += (self.fmd.process() * b.fm_buf[s]) as f64;
            let out = wrap_pi(x).sin() as f32;
            self.fb.push(out);
            l[s] = out;
            self.phase += w;
            w += dw;
            if self.phase > PI {
                self.phase -= 2.0 * PI;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3_extend_lowers_the_minimum_only() {
        let mut s = SlotValue { v: -100.0, ..SlotValue::default() };
        assert_eq!(m3_offset(&s), -60.0);
        s.extend = true;
        assert_eq!(m3_offset(&s), -100.0);
        s.v = -200.0;
        assert_eq!(m3_offset(&s), M3_MIN_EXTENDED);
        // 0.30 Hz at the extended minimum, 8.18 Hz at the plain one.
        let hz = |off: f32| 440.0 * 2f64.powf((60.0 + off as f64 - 69.0) / 12.0);
        assert!((hz(M3_MIN_EXTENDED) - 0.30).abs() < 0.005, "{}", hz(M3_MIN_EXTENDED));
        assert!((hz(M3_MIN) - 8.18).abs() < 0.01);
        s.v = 12.0;
        assert_eq!(m3_offset(&s), 12.0);
        s.extend = false;
        assert_eq!(m3_offset(&s), 12.0);
        let mut slots = [SlotValue::default(); 7];
        slots[5].extend = true;
        let mut rt = false;
        migrate(21, &mut slots, &mut rt);
        assert!(!slots[5].extend);
    }
}
