//! Sine (type 1): 32 quadrant-shaped sines with feedback, phase FM, unison,
//! low / high cut and the character filter; a default phase engine and the
//! legacy rotating-vector engine.

use super::{abs_detune_note, detune_factor, feedback, omega, spread, unison_lr, wrap_pi, CentredRamp, CharFilter, Cuts, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::PI;

pub const SHAPE_NAMES: [&str; 32] = [
    "Wave 1 (TX 1)",
    "Wave 2 (TX 2)",
    "Wave 3 (TX 3)",
    "Wave 4 (TX 4)",
    "Wave 5 (TX 5)",
    "Wave 6 (TX 6)",
    "Wave 7 (TX 7)",
    "Wave 8 (TX 8)",
    "Wave 9",
    "Wave 10",
    "Wave 11",
    "Wave 12",
    "Wave 13",
    "Wave 14",
    "Wave 15",
    "Wave 16",
    "Wave 17",
    "Wave 18",
    "Wave 19",
    "Wave 20",
    "Wave 21",
    "Wave 22",
    "Wave 23",
    "Wave 24",
    "Wave 25",
    "Wave 26",
    "Wave 27",
    "Wave 28",
    "Wave 29 (Legacy TX 2)",
    "Wave 30 (Legacy TX 4)",
    "Wave 31 (Legacy TX 6)",
    "Wave 32 (Legacy TX 8)",
];

pub const FEEDBACK_DEFORM: &[&str] = &["Last Sample", "Average Of Last Two"];

/// A low cut slot (note vs A440, deactivated by default).
pub fn low_cut() -> SlotDef {
    SlotDef { can_deactivate: true, about: "2-pole high-pass", ..SlotDef::float("Low Cut", -60.0, 70.0, -60.0, SlotUnit::NoteA440) }
}
pub fn high_cut() -> SlotDef {
    SlotDef { can_deactivate: true, about: "2-pole low-pass", ..SlotDef::float("High Cut", -60.0, 70.0, 70.0, SlotUnit::NoteA440) }
}
/// Bipolar feedback (extendable x4) with the two history deforms.
pub fn feedback_def() -> SlotDef {
    SlotDef { can_extend: true, deform: FEEDBACK_DEFORM, ..SlotDef::float("Feedback", -1.0, 1.0, 0.0, SlotUnit::Percent) }
}

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef::choice("Shape", &SHAPE_NAMES, 0.0),
        feedback_def(),
        SlotDef::choice("Behavior", &["Legacy", "Default"], 1.0),
        low_cut(),
        high_cut(),
        super::classic::unison_detune(0.1),
        super::classic::unison_voices(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7], retrigger: &mut bool) {
    if rev <= 9 {
        s[0].v = 0.0;
    }
    if rev <= 10 {
        s[1].v = 0.0;
        s[2].v = 0.0;
    }
    if rev <= 12 {
        s[3].v = -60.0;
        s[3].deactivated = true;
        s[4].v = 70.0;
        s[4].deactivated = true;
        const MAP: [i32; 20] = [0, 8, 9, 10, 1, 11, 4, 12, 13, 2, 3, 5, 6, 7, 14, 15, 16, 17, 18, 19];
        let i = s[0].i();
        s[0].v = if (0..20).contains(&i) { MAP[i as usize] as f32 } else { 0.0 };
    }
    if rev <= 15 {
        *retrigger = true;
    }
}

/// Patch-level rule (the loader's, rev <= 27): shapes 1, 3, 5, 7 become the
/// pre-correction TX 2/4/6/8 curves 28..31.
pub fn migrate_patch(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 27 {
        let i = s[0].i();
        if matches!(i, 1 | 3 | 5 | 7) {
            s[0].v = (28 + (i - 1) / 2) as f32;
        }
    }
}

#[inline]
fn quadrant(s: f32, c: f32) -> f32 {
    match (s <= 0.0, c <= 0.0) {
        (false, false) => 1.0,
        (false, true) => 2.0,
        (true, true) => 3.0,
        (true, false) => 4.0,
    }
}

/// The shape functions of the quadrant table (s = sin x, c = cos x).
pub fn shape(mode: u8, s: f32, c: f32) -> f32 {
    let s2 = 2.0 * s * c;
    let c2 = 1.0 - 2.0 * s * s;
    let s4 = 2.0 * s2 * c2;
    let h = s >= 0.0;
    let pm = |b: bool| if b { 1.0 } else { -1.0 };
    match mode {
        0 => s,
        1 => if h { (1.0 - c2) * 0.5 } else { (c2 - 1.0) * 0.5 },
        2 => if h { s } else { 0.0 },
        3 => if h { (1.0 - c2) * 0.5 } else { 0.0 },
        4 => if h { s2 } else { 0.0 },
        5 => if h { shape(1, s2, c2) } else { 0.0 },
        6 => if h { s2.abs() } else { 0.0 },
        7 => shape(5, s, c).abs(),
        8 => 2.0 * (if h { s } else { 0.0 }) - 1.0,
        9 => if s * c <= 0.0 { s } else { 0.0 },
        10 => if s * c >= 0.0 { s } else { 0.0 },
        11 => 2.0 * shape(3, s, c) - 1.0,
        12 => s2 * pm(c >= 0.0),
        13 => if s2 >= 0.0 { s2 * pm(s > 0.0) } else { 0.0 },
        14 => if h { c2.abs() } else { 0.0 },
        15 => if c >= 0.0 { if h { 1.0 - s } else { -1.0 - s } } else { 0.0 },
        16 => if c >= 0.0 { if h { 1.0 - s } else { c - 1.0 } } else { 0.0 },
        17 => pm(h) - s,
        18 => if c <= 0.0 { c } else { pm(h) * s2 },
        19 => if h { if c >= 0.0 { s2 } else { -s4 } } else { s },
        20 => if s * c >= 0.0 { s } else { pm(h) },
        21 => if s * c >= 0.0 { pm(h) } else { s },
        22 => if c >= 0.0 { s } else { 0.0 },
        23 => if c <= 0.0 { s } else { 0.0 },
        24 => if h { 1.0 - s } else { s },
        25 => if h { s2 / quadrant(s, c) } else { 0.0 },
        26 => if s <= 0.0 && c <= 0.0 { 0.0 } else { s },
        27 => s2 / quadrant(s, c),
        28 => pm(h) + pm(s * c < 0.0) * c,
        29 => if h { 1.0 + c * pm(c <= 0.0) } else { 0.0 },
        30 => if h { shape(28, s2, c2) } else { 0.0 },
        31 => shape(30, s, c).abs(),
        _ => s,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    ph: f64,
    w: CentredRamp,
    last: [f32; 2],
    gl: f32,
    gr: f32,
    ramp: f32,
    // legacy rotating vector
    vr: f64,
    vi: f64,
}

pub struct Sine {
    rng: Rng,
    v: [Voice; 16],
    n: usize,
    fm: Lag,
    fb: Lag,
    legacy_fm: Lag,
    cuts: Cuts,
    chr: CharFilter,
    ramp_in: bool,
    first: bool,
}

impl Sine {
    pub fn new(seed: u32) -> Sine {
        Sine {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            n: 1,
            fm: Lag::default(),
            fb: Lag::default(),
            legacy_fm: Lag::default(),
            cuts: Cuts::default(),
            chr: CharFilter::default(),
            ramp_in: false,
            first: true,
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.n = b.unison();
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(self.n, u);
            v.gl = gl;
            v.gr = gr;
            v.ph = if b.retrigger { 0.0 } else { wrap_pi((2.0 * self.rng.unit() as f64 - 1.0) * PI) };
            v.ramp = if u == 0 { 1.0 } else { 0.0 };
            let th = v.ph;
            v.vr = th.sin();
            v.vi = -th.cos();
            self.v[u] = v;
        }
        self.ramp_in = b.retrigger;
        let legacy = b.slots[2].i() == 0;
        if legacy || !b.retrigger {
            // Legacy fades unison voices in itself; default engine only fades on retrigger.
            for u in 0..self.n {
                self.v[u].ramp = if legacy && u > 0 { 0.0 } else { 1.0 };
            }
        }
        self.fm = Lag::default();
        self.fb = Lag::default();
        self.legacy_fm = Lag::default();
        self.cuts.reset();
        self.chr.reset();
        self.first = false;
    }

    fn voice_w(&self, b: &OscBlock, u: usize) -> f64 {
        let det = b.drift * self.v[u].drift.d + spread(b, 5) * detune_factor(self.n, u);
        let det = if b.slots[5].absolute { abs_detune_note(det, b.pitch.min(148.0)) } else { det };
        omega(b.pitch + det, b.fs_os).min(PI)
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        for u in 0..self.n {
            self.v[u].drift.next(&mut self.rng);
        }
        let mode = b.slots[0].i().clamp(0, 31) as u8;
        let mut ol = [0.0f32; 64];
        let mut or = [0.0f32; 64];
        let att = 1.0 / (self.n as f32).sqrt();
        if b.slots[2].i() == 0 {
            self.legacy(b, mode, att, &mut ol, &mut or);
        } else {
            self.default_engine(b, mode, att, &mut ol, &mut or);
        }
        if b.stereo {
            *l = ol;
            *r = or;
        } else {
            for k in 0..64 {
                l[k] = 0.5 * (ol[k] + or[k]);
            }
        }
        self.cuts.process(&b.slots[3], &b.slots[4], b.fs_os, b.stereo, l, r);
        self.chr.process(b.character, b.fs(), b.stereo, l, r);
    }

    fn default_engine(&mut self, b: &OscBlock, mode: u8, att: f32, ol: &mut [f32; 64], or: &mut [f32; 64]) {
        let fmd = b.fmdepth;
        self.fm.set(if b.fm { (32.0 * std::f32::consts::PI * fmd * fmd * fmd).clamp(-1e6, 1e6) } else { 0.0 });
        self.fb.set(feedback(&b.slots[1]));
        let avg = b.slots[1].deform == 1;
        let ramp_step = if self.ramp_in { 1.0 / 64.0 } else { 0.0 };
        let mut fmv = [0.0f32; 64];
        let mut fbv = [0.0f32; 64];
        for k in 0..64 {
            fmv[k] = self.fm.process();
            fbv[k] = self.fb.process();
        }
        for u in 0..self.n {
            let w = self.voice_w(b, u);
            let v = &mut self.v[u];
            let (mut wk, step) = v.w.set(w);
            for k in 0..64 {
                let fb = fbv[k];
                let lv = if avg { 0.5 * (v.last[0] + v.last[1]) } else { v.last[0] };
                let fbt = fb.abs() * if fb < 0.0 { lv * lv } else { lv };
                let x = wrap_pi(v.ph + fbt as f64 + (fmv[k] * b.fm_buf[k]) as f64);
                let (s, c) = x.sin_cos();
                let out = shape(mode, s as f32, c as f32);
                v.last[1] = v.last[0];
                v.last[0] = out;
                if v.ramp < 1.0 {
                    v.ramp = (v.ramp + ramp_step).min(1.0);
                }
                let g = out * att * v.ramp;
                ol[k] += g * v.gl;
                or[k] += g * v.gr;
                v.ph += wk;
                if v.ph > PI {
                    v.ph -= 2.0 * PI;
                }
                wk += step;
            }
        }
        self.ramp_in = false;
    }

    fn legacy(&mut self, b: &OscBlock, mode: u8, att: f32, ol: &mut [f32; 64], or: &mut [f32; 64]) {
        let fade = (1.0 / 50.0) * 44100.0 / b.fs();
        self.legacy_fm.set(if b.fm { b.fmdepth } else { 0.0 });
        let mut fmv = [0.0f32; 64];
        for f in fmv.iter_mut() {
            *f = self.legacy_fm.process();
        }
        for u in 0..self.n {
            let w = self.voice_w(b, u);
            let v = &mut self.v[u];
            if b.fm {
                for k in 0..64 {
                    let (s, c) = v.ph.sin_cos();
                    let out = shape(mode, s as f32, c as f32);
                    let g = out * att * v.ramp;
                    v.ramp = (v.ramp + fade).min(1.0);
                    ol[k] += g * v.gl;
                    or[k] += g * v.gr;
                    v.ph = wrap_pi(v.ph + w + (b.fm_buf[k] * fmv[k]) as f64);
                }
                let th = v.ph;
                v.vr = th.sin();
                v.vi = -th.cos();
            } else {
                // Rotating vector (sin th, -cos th), renormalised per block.
                let (sw, cw) = w.sin_cos();
                let m = (v.vr * v.vr + v.vi * v.vi).sqrt().max(1e-12);
                v.vr /= m;
                v.vi /= m;
                for k in 0..64 {
                    let out = shape(mode, v.vr as f32, v.vi as f32);
                    let g = out * att * v.ramp;
                    v.ramp = (v.ramp + fade).min(1.0);
                    ol[k] += g * v.gl;
                    or[k] += g * v.gr;
                    let (r0, i0) = (v.vr, v.vi);
                    v.vr = r0 * cw - i0 * sw;
                    v.vi = r0 * sw + i0 * cw;
                }
                v.ph = wrap_pi(v.vr.atan2(-v.vi));
            }
        }
    }
}
