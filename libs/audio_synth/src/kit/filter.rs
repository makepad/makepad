//! The kit's voice filter SLOT. Its schema (cutoff, res, drive, var, mix,
//! pan, level, keytrack) is fixed; the type family is the shared one the
//! clean-room filter spec defines and is filled in once when it lands, for
//! every layout at once. Until then the types are the ones this crate's own
//! DSP already has (state-variable responses and the ladder).

use super::fast_tanh;
use super::schema::{Builder, Unit::{self, Hz, Percent}, Values};
use crate::dsp::Ladder;
use crate::svf::{Svf, SvfCoeffs};

/// The types built now (the shared filter family extends this list).
pub const TYPES: &[&str] = &["lp12", "lp24", "hp12", "hp24", "bp12", "notch", "peak", "allpass", "ladder"];
pub const ROUTES: &[&str] = &["serial", "parallel"];

#[derive(Clone, Debug, Default)]
pub struct FilterIx {
    pub on: usize,
    pub kind: usize,
    pub cutoff: usize,
    pub res: usize,
    pub drive: usize,
    pub var: usize,
    pub mix: usize,
    pub pan: usize,
    pub level: usize,
    pub key: usize,
}

/// Declare filter `n` (1-based): keys `f1_cutoff` …
pub fn declare(b: &mut Builder, n: usize, on: bool, cutoff: f32) -> FilterIx {
    let k = |s: &str| format!("f{n}_{s}");
    b.group(&format!("filter{n}"), "a filter per voice: cutoff, res, drive, var (peak: gain), mix, pan (spreads the cutoff left/right), keytracked");
    FilterIx {
        on: b.flag(k("on"), "on", on, ""),
        kind: b.choice(k("type"), "type", TYPES, 1, ""),
        cutoff: b.log(k("cutoff"), "cutoff", 20.0, 20000.0, cutoff, Hz, ""),
        res: b.num(k("res"), "res", 0.0, 1.0, 0.2, Percent, ""),
        drive: b.num(k("drive"), "drive", 0.0, 1.0, 0.0, Percent, ""),
        var: b.num(k("var"), "var", 0.0, 1.0, 0.5, Percent, ""),
        mix: b.num(k("mix"), "mix", 0.0, 1.0, 1.0, Percent, ""),
        pan: b.num(k("pan"), "pan", -1.0, 1.0, 0.0, Unit::None, ""),
        level: b.num(k("level"), "level", 0.0, 1.0, 1.0, Percent, ""),
        key: b.num(k("key"), "key", 0.0, 1.0, 0.0, Percent, "cutoff follows the note this much"),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FilterBlock {
    pub on: bool,
    pub kind: u8,
    pub cutoff: f32,
    pub res: f32,
    pub drive: f32,
    pub var: f32,
    pub mix: f32,
    pub pan: f32,
    pub level: f32,
}

impl FilterBlock {
    /// `note_ratio` is the note's frequency over c4's (keytracking).
    pub fn read(v: &(impl Values + ?Sized), x: &FilterIx, note_ratio: f32, cutoff_scale: f32) -> FilterBlock {
        let key = v.val(x.key);
        FilterBlock {
            on: v.on(x.on),
            kind: v.index(x.kind) as u8,
            cutoff: v.val(x.cutoff) * note_ratio.powf(key) * cutoff_scale,
            res: v.val(x.res),
            drive: v.val(x.drive),
            var: v.val(x.var),
            mix: v.val(x.mix),
            pan: v.val(x.pan),
            level: v.val(x.level),
        }
    }
}

/// One filter of one voice, stereo.
#[derive(Clone, Copy, Default)]
pub struct Filter {
    a: [Svf; 2],
    b: [Svf; 2],
    ladder: [Ladder; 2],
    c1: [Option<SvfCoeffs>; 2],
    c2: [Option<SvfCoeffs>; 2],
    kind: u8,
    drive: f32,
    mix: f32,
    gain: f32,
    level: f32,
}

impl Filter {
    pub fn reset(&mut self) {
        *self = Filter::default();
    }

    /// Coefficients for a block.
    pub fn prepare(&mut self, f: &FilterBlock, rate: f32) {
        self.kind = f.kind;
        self.drive = f.drive;
        self.mix = f.mix;
        self.level = f.level;
        // Peak gain from var: 0.5 is flat, the ends ±18 dB.
        self.gain = 10f32.powf((f.var - 0.5) * 36.0 / 20.0);
        let q = 0.5 + f.res * f.res * 24.0;
        for ch in 0..2 {
            let spread = (if ch == 0 { -f.pan } else { f.pan } * 0.5).exp2();
            let hz = (f.cutoff * spread).clamp(20.0, rate * 0.45);
            match f.kind {
                8 => self.ladder[ch].set(hz, f.res, rate),
                1 | 3 => {
                    self.c1[ch] = Some(SvfCoeffs::new(hz, 0.541, rate));
                    self.c2[ch] = Some(SvfCoeffs::new(hz, 1.307 + f.res * f.res * 20.0, rate));
                }
                _ => self.c1[ch] = Some(SvfCoeffs::new(hz, q, rate)),
            }
        }
    }

    #[inline]
    pub fn tick(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        let pre = 1.0 + self.drive * 15.0;
        for ch in 0..2 {
            let dry = x[ch];
            let mut s = dry;
            if self.drive > 0.0 {
                s = fast_tanh(s * pre) / (1.0 + self.drive * 3.0);
            }
            let y = match self.kind {
                8 => self.ladder[ch].process(s),
                k => {
                    let Some(c) = self.c1[ch].as_ref() else { return x };
                    let (lp, bp, hp) = self.a[ch].process(c, s);
                    match k {
                        0 => lp,
                        1 | 3 => {
                            let first = if k == 1 { lp } else { hp };
                            let Some(c2) = self.c2[ch].as_ref() else { return x };
                            let (lp2, _, hp2) = self.b[ch].process(c2, first);
                            if k == 1 { lp2 } else { hp2 }
                        }
                        2 => hp,
                        4 => bp,
                        5 => lp + hp,
                        6 => s + (self.gain - 1.0) * bp,
                        7 => lp - bp + hp,
                        _ => lp,
                    }
                }
            };
            out[ch] = (dry + (y - dry) * self.mix) * self.level;
        }
        out
    }
}
