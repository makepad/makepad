//! The hybrid oscillators (owner: SYNTHPORT-OSC; twist: SYNTHPORT-TWIST).
//!
//! Twelve types, each rendering one 64-sample block at the oversampled rate
//! (`fs_os` = 2x host) per call: Classic (band-limited steps and ramps),
//! Sine (32 quadrant shapes, two engines), Wavetable (band-limited steps
//! through mip-mapped frames, sample playback), S&H Noise, Audio Input,
//! FM3, FM2, Window (integer granular), Modern (second-order differentiated
//! polynomials), String (two plucked delay-line strings), Twist (lane
//! TWIST) and Alias (8-bit phase games).
//!
//! Shared pieces live here: the per-voice drift walk, the unison spread and
//! pan law, the character filter and the optional low / high cut pair.

use super::core::{midi_hz, Biquad, SlotDef, SlotValue, BLOCK_OS};
use crate::dsp::Rng;
use std::f64::consts::PI;
use std::sync::Arc;

pub mod alias;
pub mod audioin;
pub mod blit;
pub mod classic;
pub mod fm2;
pub mod fm3;
pub mod modern;
pub mod shnoise;
pub mod sine;
pub mod string;
pub mod table;
pub mod wavetable;
pub mod window;
pub mod twist;

#[cfg(test)]
mod tests;

pub use table::RefTable;

/// Display names, index = stored type id.
pub const TYPE_NAMES: [&str; 12] =
    ["Classic", "Sine", "Wavetable", "S&H Noise", "Audio Input", "FM3", "FM2", "Window", "Modern", "String", "Twist", "Alias"];

pub const CLASSIC: u8 = 0;
pub const SINE: u8 = 1;
pub const WAVETABLE: u8 = 2;
pub const SHNOISE: u8 = 3;
pub const AUDIOIN: u8 = 4;
pub const FM3: u8 = 5;
pub const FM2: u8 = 6;
pub const WINDOW: u8 = 7;
pub const MODERN: u8 = 8;
pub const STRING: u8 = 9;
pub const TWIST: u8 = 10;
pub const ALIAS: u8 = 11;

/// What slots param0..6 mean for a type.
pub fn slot_defs(ty: u8) -> [SlotDef; 7] {
    match ty {
        CLASSIC => classic::slot_defs(),
        SINE => sine::slot_defs(),
        WAVETABLE => wavetable::slot_defs(),
        SHNOISE => shnoise::slot_defs(),
        AUDIOIN => audioin::slot_defs(),
        FM3 => fm3::slot_defs(),
        FM2 => fm2::slot_defs(),
        WINDOW => window::slot_defs(),
        MODERN => modern::slot_defs(),
        STRING => string::slot_defs(),
        ALIAS => alias::slot_defs(),
        TWIST => twist::slot_defs(),
        _ => [SlotDef::UNUSED; 7],
    }
}

/// The slot values (with their default flags: deactivated cuts, extended
/// morph, deform options) a newly created oscillator of `ty` gets.
pub fn default_slots(ty: u8) -> [SlotValue; 7] {
    let defs = slot_defs(ty);
    let mut s = [SlotValue::default(); 7];
    for (v, d) in s.iter_mut().zip(defs.iter()) {
        *v = d.default_value();
    }
    match ty {
        TWIST => return twist::default_slots(),
        SINE => {
            s[3].deactivated = true;
            s[4].deactivated = true;
        }
        WAVETABLE => {
            s[0].extend = true;
            s[0].deform = 1;
        }
        WINDOW => {
            s[3].deactivated = true;
            s[4].deactivated = true;
        }
        SHNOISE => {
            s[2].deactivated = true;
            s[3].deactivated = true;
            s[4].deform = 1;
        }
        AUDIOIN => {
            s[5].deactivated = true;
            s[6].deactivated = true;
        }
        STRING => {
            s[1].deform = string::DEFORM_DEFAULT_LEVEL;
            s[6].deform = string::DEFORM_DEFAULT_STIFF;
        }
        _ => {}
    }
    s
}

/// The Alias type's extra config defaults (16 additive partial amplitudes).
pub fn default_extra(ty: u8) -> Vec<f32> {
    if ty == ALIAS {
        (0..16).map(|k| 1.0 / (k as f32 + 1.0)).collect()
    } else {
        Vec::new()
    }
}

/// Per-type stream migrations for a patch of revision `rev` (applied after
/// the raw values are read). The patch-level rules are separate
/// ([`migrate_patch`]) for the loader to call after these.
pub fn migrate(ty: u8, rev: i32, slots: &mut [SlotValue; 7], retrigger: &mut bool, table_is_sample: bool) {
    let _ = table_is_sample;
    match ty {
        SINE => sine::migrate(rev, slots, retrigger),
        WAVETABLE => wavetable::migrate(rev, slots),
        WINDOW => window::migrate(rev, slots, retrigger),
        SHNOISE => shnoise::migrate(rev, slots),
        AUDIOIN => audioin::migrate(rev, slots),
        FM3 => fm3::migrate(rev, slots, retrigger),
        FM2 => fm2::migrate(rev, slots, retrigger),
        MODERN => modern::migrate(rev, slots),
        STRING => string::migrate(rev, slots),
        TWIST => twist::migrate(rev, slots),
        _ => {}
    }
}

/// The patch-level rules that touch only an oscillator's own slots (the
/// loader runs them after [`migrate`]): Sine shape fix (rev <= 27) and the
/// Wavetable sample play-count deform (rev <= 30). Table-flag rules (loop
/// flag at >= 7 voices, Window losing sample flags) stay with the loader.
pub fn migrate_patch(ty: u8, rev: i32, slots: &mut [SlotValue; 7], table_is_sample: bool) {
    match ty {
        SINE => sine::migrate_patch(rev, slots),
        WAVETABLE => wavetable::migrate_patch(rev, slots, table_is_sample),
        _ => {}
    }
}

/// Everything the voice hands an oscillator for one block.
pub struct OscBlock<'a> {
    /// Note number (MIDI convention, 60 = C4).
    pub pitch: f32,
    /// Scene Osc Drift (0..1).
    pub drift: f32,
    /// Wide filter configuration: two independent channels.
    pub stereo: bool,
    pub fm: bool,
    /// Linear 10^(dB/20) of the scene FM depth.
    pub fmdepth: f32,
    /// The modulator (the other oscillator's left output this block).
    pub fm_buf: &'a [f32; 64],
    pub slots: &'a [SlotValue; 7],
    pub retrigger: bool,
    /// 0 Warm, 1 Neutral, 2 Bright.
    pub character: u8,
    pub gate: bool,
    pub table: Option<&'a Arc<RefTable>>,
    /// Alias: the 16 additive partial amplitudes.
    pub extra: &'a [f32],
    /// Host input upsampled to fs_os.
    pub audio_in: Option<(&'a [f32; 64], &'a [f32; 64])>,
    pub fs_os: f32,
    /// Tempo / 120.
    pub tsr: f32,
}

impl OscBlock<'_> {
    #[inline]
    pub fn fs(&self) -> f32 {
        0.5 * self.fs_os
    }
    /// Slot value times 12 when extended (pitch-like slots).
    #[inline]
    pub fn ext12(&self, i: usize) -> f32 {
        let s = &self.slots[i];
        if s.extend { s.v * 12.0 } else { s.v }
    }
    /// Unison voice count (slot 6 for most types), 1..16.
    #[inline]
    pub fn unison(&self) -> usize {
        (self.slots[6].v.round() as i32).clamp(1, 16) as usize
    }
}

/// Hz of a note number (12-TET; note 69 = 440 Hz).
#[inline]
pub fn freq(n: f32) -> f32 {
    midi_hz(n)
}

/// Oversampled samples per cycle (>= 1).
#[inline]
pub fn period(n: f32, fs_os: f32) -> f32 {
    (fs_os / freq(n)).max(1.0)
}

/// rad per oversampled sample.
#[inline]
pub fn omega(n: f32, fs_os: f32) -> f64 {
    2.0 * PI * freq(n) as f64 / fs_os as f64
}

/// cycles per oversampled sample.
#[inline]
pub fn dphase(n: f32, fs_os: f32) -> f64 {
    freq(n) as f64 / fs_os as f64
}

/// The slow random walk each (unison) voice detunes by (x drift).
#[derive(Clone, Copy, Debug, Default)]
pub struct DriftLfo {
    d2: f32,
    pub d: f32,
}

impl DriftLfo {
    pub fn init(&mut self, rng: &mut Rng, nonzero: bool) {
        self.d2 = if nonzero { 0.0005 * rng.unit() } else { 0.0 };
        self.d = self.d2 * 316.227_77;
    }
    /// Once per block.
    pub fn next(&mut self, rng: &mut Rng) -> f32 {
        let r = rng.bipolar();
        self.d2 = self.d2 * (1.0 - 1e-5) + r * 1e-5;
        self.d = self.d2 * 316.227_77;
        self.d
    }
}

/// Unison detune factor (-1..1) of voice `v` of `n`.
#[inline]
pub fn detune_factor(n: usize, v: usize) -> f32 {
    if n <= 1 { 0.0 } else { -1.0 + v as f32 * 2.0 / (n - 1) as f32 }
}

/// Unison pan position d of voice `v` of `n` (L = 1 - d, R = 1 + d).
pub fn unison_pan(n: usize, v: usize) -> f32 {
    if n <= 1 {
        return 0.0;
    }
    let mid = n as f32 * 0.5 - 0.5;
    let mut d = (v as f32 - mid).abs() / mid;
    if n % 2 == 1 && v >= n / 2 {
        d = -d;
    }
    if v % 2 == 1 {
        d = -d;
    }
    d
}

/// Unison (L, R) gains of voice `v` of `n` (before the 1/sqrt(n) gain).
#[inline]
pub fn unison_lr(n: usize, v: usize) -> (f32, f32) {
    let d = unison_pan(n, v);
    (1.0 - d, 1.0 + d)
}

/// The BLIT / Sine / Window absolute-detune conversion: a spread in Hz-ish
/// units to a note offset at `pitch`.
#[inline]
pub fn abs_detune_note(det: f32, pitch: f32) -> f32 {
    det * (-pitch / 12.0).exp2() * 16.0 / 0.9443
}

/// Unison detune spread in semitones (slot `i`): extended x12.
#[inline]
pub fn spread(b: &OscBlock, i: usize) -> f32 {
    b.ext12(i)
}

/// The character filter's coefficient uses the host rate although it runs on
/// the 2x signal (its corner sits an octave higher than designed); mirrored.
pub const QUIRK_CHARACTER_HOST_RATE: bool = true;
/// Hard sync (Classic, S&H, Modern) resets on every second leader cycle (the
/// two-period state cycle); mirrored.
pub const QUIRK_SYNC_EVERY_SECOND_LEADER: bool = true;
/// The legacy Sine engine hands the shapes -cos as the "cosine"; mirrored.
pub const QUIRK_SINE_LEGACY_NEG_COS: bool = true;

/// The patch-wide character filter (Warm one-pole low-pass / Neutral /
/// Bright first-order boost), coefficient from the host rate. A fresh
/// instance seeds its history with the first sample it sees.
#[derive(Clone, Copy, Debug, Default)]
pub struct CharFilter {
    x1: [f32; 2],
    y1: [f32; 2],
    seeded: [bool; 2],
}

impl CharFilter {
    pub fn reset(&mut self) {
        *self = CharFilter::default();
    }
    /// Start from zero state (the inline Classic use).
    pub fn reset_zero(&mut self) {
        *self = CharFilter { seeded: [true; 2], ..CharFilter::default() };
    }
    #[inline]
    pub fn coef(fs: f32) -> f32 {
        // QUIRK_CHARACTER_HOST_RATE: `fs` is the host rate.
        let a = 1.0 - 2.0 * 5000.0 / fs;
        a * a
    }
    /// One channel in place.
    pub fn run(&mut self, ch: usize, mode: u8, fs: f32, x: &mut [f32]) {
        if mode == 1 || x.is_empty() {
            return;
        }
        if !self.seeded[ch] {
            self.x1[ch] = x[0];
            self.y1[ch] = x[0];
            self.seeded[ch] = true;
        }
        let k = Self::coef(fs);
        if mode == 0 {
            let mut y = self.y1[ch];
            for s in x.iter_mut() {
                y = k * y + (1.0 - k) * *s;
                *s = y;
            }
            self.y1[ch] = y;
        } else {
            let ik = 1.0 / (1.0 - k);
            let mut px = self.x1[ch];
            for s in x.iter_mut() {
                let v = *s;
                *s = (v - k * px) * ik;
                px = v;
            }
            self.x1[ch] = px;
        }
    }
    /// L (and R in stereo) in place.
    pub fn process(&mut self, mode: u8, fs: f32, stereo: bool, l: &mut [f32; 64], r: &mut [f32; 64]) {
        self.run(0, mode, fs, l);
        if stereo {
            self.run(1, mode, fs, r);
        }
    }
}

/// The optional low cut (RBJ high-pass) and high cut (LP2B) pair of Sine,
/// Window, S&H Noise and Audio Input; each skipped while deactivated.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cuts {
    lc: Biquad,
    hc: Biquad,
}

impl Cuts {
    pub fn reset(&mut self) {
        self.lc.suspend();
        self.hc.suspend();
    }
    /// Per block: `lo`, `hi` = the cut slots (notes vs A440).
    pub fn process(&mut self, lo: &SlotValue, hi: &SlotValue, fs_os: f32, stereo: bool, l: &mut [f32; 64], r: &mut [f32; 64]) {
        // Coefficients "at fs" with the angle halved = the true angle at fs_os.
        let w = |v: f32| 2.0 * PI * (440.0 * (v as f64 / 12.0).exp2()) / fs_os as f64;
        if !lo.deactivated {
            self.lc.hp(w(lo.v), 0.707);
            Self::run(&mut self.lc, stereo, l, r);
        }
        if !hi.deactivated {
            self.hc.lp2b(w(hi.v), 0.707);
            Self::run(&mut self.hc, stereo, l, r);
        }
    }
    fn run(f: &mut Biquad, stereo: bool, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if stereo {
            f.process(l, r);
        } else {
            f.process_mono(l);
        }
    }
}

/// Bipolar feedback slot to (sign, magnitude) with the x4 extension.
#[inline]
pub fn feedback(s: &SlotValue) -> f32 {
    if s.extend { 4.0 * s.v } else { s.v }
}

/// The common start-of-block value ramp: increment ramped across the block
/// centred on this block's value.
#[derive(Clone, Copy, Debug, Default)]
pub struct CentredRamp {
    prev: f64,
    started: bool,
}

impl CentredRamp {
    pub fn reset(&mut self) {
        self.started = false;
    }
    /// (start, step) so that sample k uses start + k*step.
    pub fn set(&mut self, w: f64) -> (f64, f64) {
        if !self.started {
            self.prev = w;
            self.started = true;
        }
        let step = (w - self.prev) / BLOCK_OS as f64;
        self.prev = w;
        (w - step * 31.5, step)
    }
}

/// Wrap a phase into [-pi, pi).
#[inline]
pub fn wrap_pi(x: f64) -> f64 {
    let t = 2.0 * PI;
    let y = x - t * ((x + PI) / t).floor();
    if y >= PI { y - t } else { y }
}

/// One oscillator's state (any type).
pub enum Osc {
    Classic(Box<classic::Classic>),
    Sine(Box<sine::Sine>),
    Wavetable(Box<wavetable::Wavetable>),
    ShNoise(Box<shnoise::ShNoise>),
    AudioIn(Box<audioin::AudioIn>),
    Fm3(Box<fm3::Fm3>),
    Fm2(Box<fm2::Fm2>),
    Window(Box<window::Window>),
    Modern(Box<modern::Modern>),
    String(Box<string::StringOsc>),
    Twist(Box<twist::Twist>),
    Alias(Box<alias::Alias>),
}

impl Osc {
    /// May allocate (String's delay lines, tables).
    pub fn new(ty: u8, seed: u32) -> Osc {
        match ty {
            CLASSIC => Osc::Classic(Box::new(classic::Classic::new(seed))),
            SINE => Osc::Sine(Box::new(sine::Sine::new(seed))),
            WAVETABLE => Osc::Wavetable(Box::new(wavetable::Wavetable::new(seed))),
            SHNOISE => Osc::ShNoise(Box::new(shnoise::ShNoise::new(seed))),
            AUDIOIN => Osc::AudioIn(Box::default()),
            FM3 => Osc::Fm3(Box::new(fm3::Fm3::new(seed))),
            FM2 => Osc::Fm2(Box::new(fm2::Fm2::new(seed))),
            WINDOW => Osc::Window(Box::new(window::Window::new(seed))),
            MODERN => Osc::Modern(Box::new(modern::Modern::new(seed))),
            STRING => Osc::String(Box::new(string::StringOsc::new(seed))),
            TWIST => Osc::Twist(Box::new(twist::Twist::new(seed))),
            ALIAS => Osc::Alias(Box::new(alias::Alias::new(seed))),
            _ => Osc::Classic(Box::new(classic::Classic::new(seed))),
        }
    }

    pub fn ty(&self) -> u8 {
        match self {
            Osc::Classic(_) => CLASSIC,
            Osc::Sine(_) => SINE,
            Osc::Wavetable(_) => WAVETABLE,
            Osc::ShNoise(_) => SHNOISE,
            Osc::AudioIn(_) => AUDIOIN,
            Osc::Fm3(_) => FM3,
            Osc::Fm2(_) => FM2,
            Osc::Window(_) => WINDOW,
            Osc::Modern(_) => MODERN,
            Osc::String(_) => STRING,
            Osc::Twist(_) => TWIST,
            Osc::Alias(_) => ALIAS,
        }
    }

    /// Note start / type change.
    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        match self {
            Osc::Classic(o) => o.init(b, nonzero_drift),
            Osc::Sine(o) => o.init(b, nonzero_drift),
            Osc::Wavetable(o) => o.init(b, nonzero_drift),
            Osc::ShNoise(o) => o.init(b, nonzero_drift),
            Osc::AudioIn(o) => o.init(),
            Osc::Fm3(o) => o.init(b, nonzero_drift),
            Osc::Fm2(o) => o.init(b, nonzero_drift),
            Osc::Window(o) => o.init(b, nonzero_drift),
            Osc::Modern(o) => o.init(b, nonzero_drift),
            Osc::String(o) => o.init(b, nonzero_drift),
            Osc::Twist(o) => o.init(b, nonzero_drift),
            Osc::Alias(o) => o.init(b, nonzero_drift),
        }
    }

    /// One block of 64 samples at fs_os. In mono the result is in `l` and
    /// copied to `r`.
    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        match self {
            Osc::Classic(o) => o.process(b, l, r),
            Osc::Sine(o) => o.process(b, l, r),
            Osc::Wavetable(o) => o.process(b, l, r),
            Osc::ShNoise(o) => o.process(b, l, r),
            Osc::AudioIn(o) => o.process(b, l, r),
            Osc::Fm3(o) => o.process(b, l, r),
            Osc::Fm2(o) => o.process(b, l, r),
            Osc::Window(o) => o.process(b, l, r),
            Osc::Modern(o) => o.process(b, l, r),
            Osc::String(o) => o.process(b, l, r),
            Osc::Twist(o) => o.process(b, l, r),
            Osc::Alias(o) => o.process(b, l, r),
        }
        if !b.stereo {
            *r = *l;
        }
    }
}
