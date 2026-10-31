//! FM2 (type 6): two integer-ratio sine modulators with a detune offset
//! between them and a shared phase offset, plus feedback. Mono.

use super::fm3::{Feedback, Modulator};
use super::sine::feedback_def;
use super::{feedback, omega, wrap_pi, CentredRamp, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::PI;

/// The reference divides the M1/2 offset (Hz) by the HOST rate although the
/// modulators run at 2x, so the real offset is offset/pi Hz; patches were
/// voiced with that.
pub const QUIRK_FM2_OFFSET_HOST_RATE: bool = true;

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef::float("M1 Amount", 0.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef::int("M1 Ratio", 1.0, 32.0, 1.0),
        SlotDef::float("M2 Amount", 0.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef::int("M2 Ratio", 1.0, 32.0, 1.0),
        SlotDef { can_extend: true, about: "M1 up / M2 down detune (extended x100)", ..SlotDef::float("M1/2 Offset", -10.0, 10.0, 0.0, SlotUnit::Hz) },
        SlotDef::float("M1/2 Phase", 0.0, 1.0, 0.0, SlotUnit::Percent),
        feedback_def(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7], retrigger: &mut bool) {
    if rev <= 15 {
        *retrigger = true;
    }
    if rev <= 21 {
        s[6].deform = 0;
    }
    if rev <= 23 {
        s[4].extend = false;
    }
}

#[inline]
fn md(x: f32) -> f32 {
    8.0 * std::f32::consts::PI * x * x * x
}

pub struct Fm2 {
    rng: Rng,
    drift: DriftLfo,
    phase: f64,
    w: CentredRamp,
    m: [Modulator; 2],
    d: [Lag; 2],
    po: Lag,
    fmd: Lag,
    fb: Feedback,
    first: bool,
}

impl Fm2 {
    pub fn new(seed: u32) -> Fm2 {
        Fm2 {
            rng: Rng::new(seed),
            drift: DriftLfo::default(),
            phase: 0.0,
            w: CentredRamp::default(),
            m: [Modulator::default(); 2],
            d: [Lag::default(); 2],
            po: Lag::default(),
            fmd: Lag::default(),
            fb: Feedback::default(),
            first: true,
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.drift.init(&mut self.rng, nonzero_drift);
        let phase0 = if b.retrigger { 0.0 } else { (2.0 * self.rng.unit() as f64 - 1.0) * PI };
        let ph = (b.slots[5].v as f64 + phase0) * 2.0 * PI;
        for m in self.m.iter_mut() {
            *m = Modulator { ph: wrap_pi(ph), ramp: CentredRamp::default() };
        }
        let amt = (md(b.slots[0].v) + md(b.slots[2].v)) as f64;
        self.phase = wrap_pi(-ph.sin() * amt - ph);
        self.w.reset();
        self.d = [Lag::default(); 2];
        self.po = Lag::default();
        self.fmd = Lag::default();
        self.fb.reset();
        self.first = false;
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], _r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        let note = b.pitch + b.drift * self.drift.next(&mut self.rng);
        let wc = omega(note, b.fs_os);
        let off = if b.slots[4].extend { 100.0 * b.slots[4].v } else { b.slots[4].v };
        // QUIRK_FM2_OFFSET_HOST_RATE
        let sh = off as f64 / b.fs() as f64;
        let r1 = b.slots[1].v.round().max(1.0) as f64;
        let r2 = b.slots[3].v.round().max(1.0) as f64;
        let rates = [(wc * r1 + sh).min(PI), (wc * r2 - sh).min(PI)];
        self.d[0].set(md(b.slots[0].v));
        self.d[1].set(md(b.slots[2].v));
        self.po.set(2.0 * std::f32::consts::PI * b.slots[5].v);
        let pi32 = 32.0 * std::f32::consts::PI;
        self.fmd.set(if b.fm { pi32 * b.fmdepth * b.fmdepth * b.fmdepth } else { 0.0 });
        let fbv = feedback(&b.slots[6]);
        self.fb.set(fbv);
        let avg = b.slots[6].deform == 1;
        let (mut w, dw) = self.w.set(wc.min(PI));
        let mut mw = [self.m[0].ramp.set(rates[0]), self.m[1].ramp.set(rates[1])];
        for s in 0..64 {
            let mut x = self.phase;
            for k in 0..2 {
                let d = self.d[k].process() as f64;
                self.m[k].ph += mw[k].0;
                mw[k].0 += mw[k].1;
                if self.m[k].ph > PI {
                    self.m[k].ph = wrap_pi(self.m[k].ph);
                }
                x += d * self.m[k].ph.sin();
            }
            x += self.fb.term(fbv, avg) as f64;
            x += self.po.process() as f64;
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
