//! The filter family of the hybrid voice (FILTERS.md sections 1-3, 5, 6, 8,
//! 11): 36 types with their subtypes, per-block coefficient makers with the
//! family's coefficient smoothing (a 0.2-per-block one-pole on the target
//! plus a linear in-block ramp), every type's per-sample algorithm, and the
//! tilt-noise shelving bank.
//!
//! A [`FilterUnit`] is one filter of one voice on one channel. Once per
//! 64-sample oversampled block the voice calls [`FilterUnit::set_block`]
//! with the type, subtype, cutoff (note vs A440, unclamped) and resonance;
//! then [`FilterUnit::tick`] runs per sample. The voice filter-block
//! configurations (serial / dual / stereo / ring / wide) live in the layout.

pub mod approx;
mod biquad;
mod ladder;
mod misc;
mod twopole;
mod tilt;
mod tripole;
mod va;
mod warp;
#[cfg(test)]
mod tests;

pub use tilt::TiltNoise;

use super::core::BLOCK_OS;

// ---- mirrored reference quirks (the mirrored path is the only behaviour) ----

/// The reference runs legacy BP 12 Driven/Clean (subtypes 3, 4) by pushing the
/// 12 dB and then the 24 dB band-pass coefficient sets through the smoother in
/// one block; old patches sound like the blend (~0.44 N12 + 0.56 N24).
pub const QUIRK_BP12_LEGACY_DOUBLE_PUSH: bool = true;
/// Clip registers (the SVF / Driven / Clean k) are ordinary zeroed state, so
/// the first sample after a reset multiplies the integrators by 0.
pub const QUIRK_CLIP_REGISTER_STARTS_ZERO: bool = true;
/// The voice passes no bell/shelf amplitude to the fast SVF: A = 0, so Low
/// Shelf hears the HP mix, High Shelf the LP mix, Bell a wide notch.
pub const QUIRK_FAST_SVF_AMPLITUDE_ZERO: bool = true;
/// The tilt-noise bank's corners are computed at the host rate though it runs
/// on the 2x block, so the real corners sit an octave higher.
pub const QUIRK_TILT_BANK_HOST_RATE: bool = true;
/// The RBJ families take (sin, cos) of the cutoff by interpolating sin and cos
/// separately between integer notes, without re-normalising.
pub const QUIRK_OMEGA_UNNORMALISED: bool = true;

/// Display names, index = stored type id.
pub const TYPE_NAMES: [&str; 36] = [
    "Off",
    "LP 12 dB",
    "LP 24 dB",
    "LP Legacy Ladder",
    "HP 12 dB",
    "HP 24 dB",
    "BP 12 dB",
    "N 12 dB",
    "FX Comb +",
    "FX Sample & Hold",
    "LP Vintage Ladder",
    "LP Two-Pole 12 dB",
    "LP Four-Pole 24 dB",
    "LP Sallen-Key",
    "HP Sallen-Key",
    "LP Diode Ladder",
    "LP Cutoff Warp",
    "HP Cutoff Warp",
    "N Cutoff Warp",
    "BP Cutoff Warp",
    "HP Two-Pole 12 dB",
    "N Two-Pole 12 dB",
    "BP Two-Pole 12 dB",
    "BP 24 dB",
    "N 24 dB",
    "FX Comb -",
    "FX Allpass",
    "FX Cutoff Warp AP",
    "LP Res Warp",
    "HP Res Warp",
    "N Res Warp",
    "BP Res Warp",
    "FX Res Warp AP",
    "MULTI Tri-pole",
    "MULTI Fast SVF",
    "MULTI Cascade",
];

/// Short menu names (group prefix stripped).
pub const SHORT_NAMES: [&str; 36] = [
    "Off",
    "12 dB",
    "24 dB",
    "Legacy Ladder",
    "12 dB",
    "24 dB",
    "12 dB",
    "12 dB",
    "Comb +",
    "Sample & Hold",
    "Vintage Ladder",
    "Two-Pole 12 dB",
    "Four-Pole 24 dB",
    "Sallen-Key",
    "Sallen-Key",
    "Diode Ladder",
    "Cutoff Warp",
    "Cutoff Warp",
    "Cutoff Warp",
    "Cutoff Warp",
    "Two-Pole 12 dB",
    "Two-Pole 12 dB",
    "Two-Pole 12 dB",
    "24 dB",
    "24 dB",
    "Comb -",
    "Allpass",
    "Cutoff Warp Allpass",
    "Resonance Warp",
    "Resonance Warp",
    "Resonance Warp",
    "Resonance Warp",
    "Resonance Warp Allpass",
    "Tri-pole",
    "Fast SVF",
    "Cascade",
];

/// Menu groups in menu order: (group name, stored ids).
pub const MENU: [(&str, &[u8]); 7] = [
    ("", &[0]),
    ("Lowpass", &[1, 2, 3, 10, 13, 15, 11, 12, 16, 28]),
    ("Bandpass", &[6, 23, 22, 19, 31]),
    ("Highpass", &[4, 5, 14, 20, 17, 29]),
    ("Notch", &[7, 24, 21, 18, 30]),
    ("Multi", &[34, 33, 35]),
    ("Effect", &[26, 27, 32, 8, 25, 9]),
];

const SUB_SDC: &[&str] = &["Standard", "Driven", "Clean"];
const SUB_BP12: &[&str] = &["Standard", "Driven", "Clean", "Driven (Legacy)", "Clean (Legacy)"];
const SUB_NOTCH: &[&str] = &["Standard", "Mild"];
const SUB_POLES: &[&str] = &["6 dB", "12 dB", "18 dB", "24 dB"];
const SUB_COMB: &[&str] = &["50% Wet", "100% Wet"];
const SUB_VINTAGE: &[&str] = &["Type 1", "Type 1 Compensated", "Type 2", "Type 2 Compensated", "Type 3", "Type 3 Compensated"];
const SUB_OB2: &[&str] = &["Standard", "Pushed"];
const SUB_OB4: &[&str] = &["6 dB", "12 dB", "18 dB", "24 dB", "24 dB (Legacy)"];
const SUB_SALLEN_KEY: &[&str] = &["No Saturation", "Mild Saturation", "Moderate Saturation", "Heavy Saturation", "Extreme Saturation"];
const SUB_CWARP: &[&str] = &[
    "1 Stage tanh",
    "2 Stages tanh",
    "3 Stages tanh",
    "4 Stages tanh",
    "1 Stage Soft Clip",
    "2 Stages Soft Clip",
    "3 Stages Soft Clip",
    "4 Stages Soft Clip",
    "1 Stage OJD",
    "2 Stages OJD",
    "3 Stages OJD",
    "4 Stages OJD",
];
const SUB_RWARP: &[&str] = &[
    "1 Stage tanh",
    "2 Stages tanh",
    "3 Stages tanh",
    "4 Stages tanh",
    "1 Stage Soft Clip",
    "2 Stages Soft Clip",
    "3 Stages Soft Clip",
    "4 Stages Soft Clip",
];
const SUB_TRIPOLE: &[&str] = &[
    "Low-Low-Low, First",
    "Low-High-Low, First",
    "High-Low-High, First",
    "High-High-High, First",
    "Low-Low-Low, Second",
    "Low-High-Low, Second",
    "High-Low-High, Second",
    "High-High-High, Second",
    "Low-Low-Low, Third",
    "Low-High-Low, Third",
    "High-Low-High, Third",
    "High-High-High, Third",
];
const SUB_FSVF: &[&str] = &["LP", "HP", "BP", "Notch", "Peak", "Allpass", "Bell", "Low Shelf", "High Shelf"];
const SUB_XPANDER: &[&str] =
    &["LP1", "LP2", "LP3", "LP4", "HP1", "HP2", "HP3", "BP2", "BP4", "N2", "PH3", "HP2+LP1", "HP3+LP1", "N2+LP1", "PH3+LP1"];

/// The user-visible subtype names of a type (empty = no subtypes).
pub fn subtype_names(ty: u8) -> &'static [&'static str] {
    match ty {
        1 | 2 | 4 | 5 | 23 => SUB_SDC,
        6 => SUB_BP12,
        7 | 24 => SUB_NOTCH,
        3 | 15 => SUB_POLES,
        8 | 25 => SUB_COMB,
        10 => SUB_VINTAGE,
        11 | 20 | 21 | 22 => SUB_OB2,
        12 => SUB_OB4,
        13 | 14 => SUB_SALLEN_KEY,
        16..=19 | 27 => SUB_CWARP,
        28..=32 => SUB_RWARP,
        33 => SUB_TRIPOLE,
        34 => SUB_FSVF,
        35 => SUB_XPANDER,
        _ => &[],
    }
}

/// Number of user-visible subtypes (0 = none).
pub fn subtype_count(ty: u8) -> u8 {
    subtype_names(ty).len() as u8
}

/// Clamp a loaded subtype to the type's range (count 0 -> 0); unknown type
/// ids (>= 36) are Off.
pub fn clamp_type(ty: i32, sub: i32) -> (u8, u8) {
    if !(0..36).contains(&ty) {
        return (0, 0);
    }
    let n = subtype_count(ty as u8) as i32;
    (ty as u8, sub.clamp(0, (n - 1).max(0)) as u8)
}

/// Response selector shared by several families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resp {
    Lp,
    Hp,
    Bp,
}

/// The per-sample algorithm a (type, subtype) runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Off,
    /// Double-sampled SVF (standard subtypes): response, four-pole.
    Svf(Resp, bool),
    /// Coupled-form biquad (Driven): two sections.
    Cfc(bool),
    /// Normalised lattice biquad (Clean, notch, allpass): two sections.
    Lattice(bool),
    Legacy(u8),
    Snh,
    Comb,
    Vintage1,
    Vintage2,
    Vintage3,
    /// two-pole: mode 0 LP / 1 HP / 2 notch / 3 BP, pushed.
    Ob2(u8, bool),
    /// four-pole and cascade: output weights on y0..y4.
    Ob4([f32; 5]),
    SallenKey(bool),
    Diode(u8),
    /// Cutoff warp: stages (1..4), saturator 0 tanh / 1 soft clip / 2 OJD.
    CWarp(u8, u8),
    /// Resonance warp: stages, saturator 0 tanh / 1 soft clip.
    RWarp(u8, u8),
    /// Tri-pole: mode, output stage.
    Tripole(u8, u8),
    FastSvf,
}

const XPANDER_W: [[f32; 5]; 15] = [
    [0.0, 1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0],
    [1.0, -1.0, 0.0, 0.0, 0.0],
    [1.0, -2.0, 1.0, 0.0, 0.0],
    [1.0, -3.0, 3.0, -1.0, 0.0],
    [0.0, -2.0, 2.0, 0.0, 0.0],
    [0.0, 0.0, 2.0, -4.0, 2.0],
    [1.0, -2.0, 2.0, 0.0, 0.0],
    [1.0, -3.0, 6.0, -4.0, 0.0],
    [0.0, -1.0, 2.0, -1.0, 0.0],
    [0.0, -1.0, 3.0, -3.0, 1.0],
    [0.0, -1.0, 2.0, -2.0, 0.0],
    [0.0, -1.0, 3.0, -6.0, 4.0],
];

const OB4_W: [[f32; 5]; 5] = [
    [0.0, 1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0, 1.0, 1.0],
];

fn resp_of(ty: u8) -> Resp {
    match ty {
        1 | 2 => Resp::Lp,
        4 | 5 => Resp::Hp,
        _ => Resp::Bp,
    }
}

pub(crate) fn kind_of(ty: u8, sub: u8) -> Kind {
    match ty {
        1 | 2 | 4 | 5 | 6 | 23 => {
            let four = matches!(ty, 2 | 5 | 23);
            match sub {
                0 => Kind::Svf(resp_of(ty), four),
                1 | 3 => Kind::Cfc(four),
                _ => Kind::Lattice(four),
            }
        }
        7 => Kind::Lattice(false),
        24 => Kind::Lattice(true),
        26 => Kind::Lattice(false),
        3 => Kind::Legacy(sub.min(3)),
        9 => Kind::Snh,
        8 | 25 => Kind::Comb,
        10 => match sub / 2 {
            0 => Kind::Vintage1,
            1 => Kind::Vintage2,
            _ => Kind::Vintage3,
        },
        11 | 20 | 21 | 22 => {
            let mode = match ty {
                11 => 0,
                20 => 1,
                21 => 2,
                _ => 3,
            };
            Kind::Ob2(mode, sub == 1)
        }
        12 => Kind::Ob4(OB4_W[(sub as usize).min(4)]),
        35 => Kind::Ob4(XPANDER_W[(sub as usize).min(14)]),
        13 => Kind::SallenKey(false),
        14 => Kind::SallenKey(true),
        15 => Kind::Diode(sub.min(3)),
        16..=19 | 27 => Kind::CWarp((sub & 3) + 1, (sub >> 2).min(2)),
        28..=32 => Kind::RWarp((sub & 3) + 1, (sub >> 2).min(1)),
        33 => Kind::Tripole(sub & 3, (sub >> 2).min(2)),
        34 => Kind::FastSvf,
        _ => Kind::Off,
    }
}

/// Coefficient vector length.
pub(crate) const NC: usize = 8;
/// State registers per unit.
pub(crate) const NS: usize = 20;

/// One filter of one voice on one channel.
#[derive(Clone, Debug)]
pub struct FilterUnit {
    pub(crate) ty: u8,
    pub(crate) sub: u8,
    pub(crate) kind: Kind,
    /// Current coefficients, per-sample delta, smoothed target.
    pub(crate) c: [f32; NC],
    pub(crate) dc: [f32; NC],
    pub(crate) t: [f32; NC],
    pub(crate) fresh: bool,
    pub(crate) fs: f32,
    pub(crate) s: [f32; NS],
    pub(crate) comb: Option<Box<misc::CombLine>>,
    /// The comb types run their extended (effect) variant.
    pub(crate) long_comb: bool,
}

impl Default for FilterUnit {
    fn default() -> Self {
        FilterUnit { ty: 0, sub: 0, kind: Kind::Off, c: [0.0; NC], dc: [0.0; NC], t: [0.0; NC], fresh: true, fs: 96000.0, s: [0.0; NS], comb: None, long_comb: false }
    }
}

impl FilterUnit {
    pub fn new() -> FilterUnit {
        FilterUnit::default()
    }

    /// Zero the state registers; the next block's coefficients snap.
    pub fn reset(&mut self) {
        self.s = [0.0; NS];
        self.dc = [0.0; NC];
        self.fresh = true;
        if !QUIRK_CLIP_REGISTER_STARTS_ZERO && matches!(self.kind, Kind::Svf(..) | Kind::Cfc(_) | Kind::Lattice(_)) {
            self.s[4] = 1.0;
        }
        if let Some(c) = &mut self.comb {
            c.clear();
        }
    }

    pub fn active(&self) -> bool {
        self.kind != Kind::Off
    }

    pub fn ty(&self) -> u8 {
        self.ty
    }

    pub fn subtype(&self) -> u8 {
        self.sub
    }

    /// Once per block: type/subtype (a change resets), cutoff in notes vs
    /// A440 (unclamped), raw resonance, `fs` = the rate the unit runs at,
    /// Allocate what type `ty` needs (the comb's line) ahead of the render
    /// path; `set_block` then never allocates for it.
    pub fn prepare(&mut self, ty: u8) {
        if matches!(ty, 8 | 25) && self.comb.is_none() {
            self.comb = Some(Box::default());
        }
    }

    /// The comb types' extended variant (the Combulator's, EFFECTS 5.16): a
    /// 2^17-sample line (allocates), delay clamp [12, 2^17 - 12] and the
    /// feedback ("resonance") clamped to [-2, 2] keeping its sign instead
    /// of the Comb +/- sign rule. Off = the voice combs (FILTERS 5.6).
    pub fn set_long_comb(&mut self, on: bool) {
        self.long_comb = on;
        let len = if on { misc::COMB_LONG } else { misc::COMB_LEN };
        if self.comb.as_ref().map(|c| c.len()) != Some(len) {
            self.comb = Some(Box::new(misc::CombLine::new(len)));
        }
    }

    /// `comb_correct` = the patch's correctly-tuned-comb flag.
    pub fn set_block(&mut self, ty: u8, sub: u8, cutoff: f32, reso: f32, fs: f32, comb_correct: bool) {
        let (ty, sub) = clamp_type(ty as i32, sub as i32);
        if ty != self.ty || sub != self.sub {
            self.ty = ty;
            self.sub = sub;
            self.kind = kind_of(ty, sub);
            if self.kind == Kind::Comb && self.comb.is_none() {
                self.comb = Some(Box::default());
            }
            self.reset();
        }
        if self.kind == Kind::Off {
            return;
        }
        self.fs = fs;
        if QUIRK_BP12_LEGACY_DOUBLE_PUSH && ty == 6 && sub >= 3 {
            // Legacy BP 12: the 12 dB and then the 24 dB band-pass set go
            // through the smoother in the same block.
            let variant = sub - 2;
            let n12 = biquad::make(6, variant, cutoff, reso, fs);
            let n24 = biquad::make(23, variant, cutoff, reso, fs);
            self.push(&n12);
            self.push(&n24);
        } else {
            let n = make_coefs(self.kind, ty, sub, cutoff, reso, fs, comb_correct, self.long_comb);
            self.push(&n);
        }
        for k in 0..NC {
            self.dc[k] = (self.t[k] - self.c[k]) / BLOCK_OS as f32;
        }
    }

    fn push(&mut self, n: &[f32; NC]) {
        if self.fresh {
            self.c = *n;
            self.t = *n;
            self.dc = [0.0; NC];
            self.fresh = false;
        } else {
            for k in 0..NC {
                self.t[k] = 0.8 * self.t[k] + 0.2 * n[k];
            }
        }
    }

    /// Advance the coefficients by `frac` of a sample's delta.
    #[inline]
    pub(crate) fn step(&mut self, frac: f32) {
        for k in 0..NC {
            self.c[k] += frac * self.dc[k];
        }
    }

    /// One sample.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        match self.kind {
            Kind::Off => x,
            Kind::Svf(r, four) => self.tick_svf(x, r, four),
            Kind::Cfc(two) => self.tick_cfc(x, two),
            Kind::Lattice(two) => self.tick_lattice(x, two),
            Kind::Legacy(o) => self.tick_legacy(x, o),
            Kind::Snh => self.tick_snh(x),
            Kind::Comb => self.tick_comb(x),
            Kind::Vintage1 => self.tick_vintage1(x, self.fs),
            Kind::Vintage2 => self.tick_vintage2(x),
            Kind::Vintage3 => self.tick_vintage3(x),
            Kind::Ob2(m, p) => self.tick_ob2(x, m, p),
            Kind::Ob4(w) => self.tick_ob4(x, &w),
            Kind::SallenKey(hp) => self.tick_sallen_key(x, hp),
            Kind::Diode(o) => self.tick_diode(x, o),
            Kind::CWarp(n, sat) => self.tick_cwarp(x, n, sat),
            Kind::RWarp(n, sat) => self.tick_rwarp(x, n, sat),
            Kind::Tripole(m, o) => self.tick_tripole(x, m, o),
            Kind::FastSvf => self.tick_fastsvf(x),
        }
    }

    /// A block in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            *v = self.tick(*v);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn make_coefs(kind: Kind, ty: u8, sub: u8, n: f32, r: f32, fs: f32, comb_correct: bool, long_comb: bool) -> [f32; NC] {
    match kind {
        Kind::Off => [0.0; NC],
        Kind::Svf(..) => biquad::make_svf(ty, n, r, fs),
        Kind::Cfc(_) | Kind::Lattice(_) => biquad::make(ty, sub, n, r, fs),
        Kind::Legacy(_) => ladder::make_legacy(n, r, fs),
        Kind::Snh => misc::make_snh(n, r, fs),
        Kind::Comb => misc::make_comb(ty, sub, n, r, fs, comb_correct, long_comb),
        Kind::Vintage1 => ladder::make_vintage1(sub & 1 == 1, n, r, fs),
        Kind::Vintage2 => ladder::make_vintage2(sub & 1 == 1, n, r, fs),
        Kind::Vintage3 => ladder::make_vintage3(sub & 1 == 1, n, r, fs),
        Kind::Ob2(..) => twopole::make_ob2(n, r, fs),
        Kind::Ob4(_) => twopole::make_ob4(n, r, fs),
        Kind::SallenKey(hp) => va::make_sallen_key(hp, sub, n, r, fs),
        Kind::Diode(_) => va::make_diode(n, r, fs),
        Kind::CWarp(..) => warp::make_cwarp(ty, sub, n, r, fs),
        Kind::RWarp(..) => warp::make_rwarp(ty, n, r, fs),
        Kind::Tripole(..) => tripole::make(n, r, fs),
        Kind::FastSvf => va::make_fastsvf(sub, n, r, fs),
    }
}

/// Cutoff in Hz of a note vs A440, clamped to [5, 0.3 fs] (the common
/// clamp of the newer families).
#[inline]
pub(crate) fn hz_clamped(n: f32, fs: f32) -> f32 {
    super::core::note_hz(n).clamp(5.0, 0.3 * fs)
}
