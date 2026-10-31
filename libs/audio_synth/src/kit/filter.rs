//! The kit's voice filter slot for layouts that run their voices at the
//! host rate in Hz terms (Morph): one schema (type, cutoff, res, drive,
//! var, mix, pan, level, keytrack) over the shared filter family
//! ([`super::flt`], 36 types with subtypes). A slot type names a family
//! type and subtype; `var` picks among a type's subtypes when it has
//! several (its first nine names are the original slot types).

use super::fast_tanh;
use super::flt::FilterUnit;
use super::schema::{Builder, Unit::{self, Hz, Percent}, Values};

/// Slot types: (name, family type id, family subtype; 255 = from `var`).
pub const KINDS: &[(&str, u8, u8)] = &[
    ("lp12", 1, 2),
    ("lp24", 2, 2),
    ("hp12", 4, 2),
    ("hp24", 5, 2),
    ("bp12", 6, 2),
    ("notch", 7, 0),
    ("peak", 34, 4),
    ("allpass", 26, 0),
    ("ladder", 3, 3),
    ("bp24", 23, 2),
    ("notch24", 24, 0),
    ("comb", 8, 255),
    ("comb_neg", 25, 255),
    ("sample_hold", 9, 0),
    ("vintage_ladder", 10, 255),
    ("two_pole_lp", 11, 255),
    ("two_pole_hp", 20, 255),
    ("two_pole_bp", 22, 255),
    ("two_pole_notch", 21, 255),
    ("four_pole_lp", 12, 255),
    ("sallen_key_lp", 13, 255),
    ("sallen_key_hp", 14, 255),
    ("diode_ladder", 15, 255),
    ("warp_lp", 16, 255),
    ("warp_hp", 17, 255),
    ("warp_notch", 18, 255),
    ("warp_bp", 19, 255),
    ("warp_allpass", 27, 255),
    ("res_warp_lp", 28, 255),
    ("res_warp_hp", 29, 255),
    ("res_warp_notch", 30, 255),
    ("res_warp_bp", 31, 255),
    ("res_warp_allpass", 32, 255),
    ("tri_pole", 33, 255),
    ("svf", 34, 255),
    ("cascade", 35, 255),
];

pub const TYPES: &[&str] = &[
    "lp12", "lp24", "hp12", "hp24", "bp12", "notch", "peak", "allpass", "ladder", "bp24", "notch24", "comb", "comb_neg", "sample_hold", "vintage_ladder", "two_pole_lp",
    "two_pole_hp", "two_pole_bp", "two_pole_notch", "four_pole_lp", "sallen_key_lp", "sallen_key_hp", "diode_ladder", "warp_lp", "warp_hp", "warp_notch", "warp_bp",
    "warp_allpass", "res_warp_lp", "res_warp_hp", "res_warp_notch", "res_warp_bp", "res_warp_allpass", "tri_pole", "svf", "cascade",
];
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
    b.group(&format!("filter{n}"), "a filter per voice: 36 types (svf lp/hp/bp/notch, ladders, two/four-pole, sallen-key, diode, combs, warps, tri-pole, cascade), cutoff, res, drive, var (picks a type's variant, e.g. the cascade's 15 mixes), mix, pan (spreads the cutoff left/right), keytracked");
    FilterIx {
        on: b.flag(k("on"), "on", on, ""),
        kind: b.choice(k("type"), "type", TYPES, 1, ""),
        cutoff: b.log(k("cutoff"), "cutoff", 20.0, 20000.0, cutoff, Hz, ""),
        res: b.num(k("res"), "res", 0.0, 1.0, 0.2, Percent, ""),
        drive: b.num(k("drive"), "drive", 0.0, 1.0, 0.0, Percent, ""),
        var: b.num(k("var"), "var", 0.0, 1.0, 0.5, Percent, "the type's variant"),
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

    /// The family type and subtype it plays.
    pub fn family(&self) -> (u8, u8) {
        let (_, ty, sub) = KINDS[(self.kind as usize).min(KINDS.len() - 1)];
        if sub != 255 {
            return (ty, sub);
        }
        let n = super::flt::subtype_count(ty).max(1);
        (ty, ((self.var.clamp(0.0, 1.0) * (n - 1) as f32).round() as u8).min(n - 1))
    }
}

/// One filter of one voice, stereo.
#[derive(Clone, Default)]
pub struct Filter {
    unit: [FilterUnit; 2],
    drive: f32,
    mix: f32,
    level: f32,
}

impl Filter {
    pub fn reset(&mut self) {
        for u in &mut self.unit {
            u.reset();
        }
    }

    /// Coefficients for a block.
    pub fn prepare(&mut self, f: &FilterBlock, rate: f32) {
        self.drive = f.drive;
        self.mix = f.mix;
        self.level = f.level;
        let (ty, sub) = f.family();
        for ch in 0..2 {
            let spread = (if ch == 0 { -f.pan } else { f.pan } * 0.5).exp2();
            let hz = (f.cutoff * spread).clamp(20.0, rate * 0.45);
            let note = 12.0 * (hz / 440.0).log2();
            self.unit[ch].set_block(ty, sub, note, f.res, rate, true);
        }
    }

    #[inline]
    pub fn tick(&mut self, x: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0; 2];
        let pre = 1.0 + self.drive * 15.0;
        for ch in 0..2 {
            let dry = x[ch];
            let s = if self.drive > 0.0 { fast_tanh(dry * pre) / (1.0 + self.drive * 3.0) } else { dry };
            let y = self.unit[ch].tick(s);
            out[ch] = (dry + (y - dry) * self.mix) * self.level;
        }
        out
    }
}
