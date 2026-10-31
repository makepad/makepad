//! The dual voltage-controlled amplifier: two gain cells whose gain is the
//! product of a linear control and an exponential one (3 mV per dB, 120 dB
//! and more), and whose character is set by the idle current between
//! class B (1 uA) and class A (100 uA and up).
//!
//! What the idle current does here is read off the sheet's tables and its
//! second-harmonic graph, for signal level 1 = 10 dB below clipping at unity
//! gain (95 uA peak):
//! - even harmonics from the cell's mismatch between the two signal halves:
//!   level-independent near class B (-40 dB), falling with idle current
//!   and with level towards class A;
//! - odd harmonics from crossover, which only idle current reduces;
//! - control feedthrough, which grows with idle current (0.2 uA at class B,
//!   7 uA at class A untrimmed; 0.01 and 1 trimmed);
//! - clipping once input plus output peak current passes the cell's limit
//!   (600 uA at class B, 1400 uA at class A).

use super::{part, tol};
use crate::kit::schema::{Builder, Unit::{self, Db, Percent}, Values};

pub const TYPES: &[&str] = &["clean", "analog"];
pub const LAWS: &[&str] = &["linear", "exponential"];

/// Signal level 1 as peak cell current, uA.
const UNIT_UA: f32 = 95.0;
/// Second harmonic asymptote: mismatch `c` makes c * 0.424 relative.
const EVEN_UNTRIMMED: f32 = 0.0236;
/// How far the idle current softens the mismatch, in signal per uA.
const EVEN_SOFT_PER_UA: f32 = 0.047;
/// Trimming lowers the even distortion about 7 times.
const TRIM_EVEN: f32 = 1.0 / 7.0;

/// The gain the control laws make: `control` 0..1 on the law's input.
///
/// Linear: the control itself. Exponential: `control` spans `range_db` to
/// 0 dB in equal dB per unit (the sheet's 3 mV per dB), shut below the first
/// 2% (a negative linear current turns the cell fully off).
#[inline]
pub fn law_gain(law: u8, control: f32, range_db: f32) -> f32 {
    let c = control.clamp(0.0, 1.0);
    if law == 0 {
        return c;
    }
    let g = db_gain(-(1.0 - c) * range_db);
    if c < 0.02 { g * c * 50.0 } else { g }
}

#[inline]
pub fn db_gain(db: f32) -> f32 {
    (db * 0.166_096_4).exp2()
}

#[derive(Clone, Debug, Default)]
pub struct VcaIx {
    pub kind: usize,
    pub law: usize,
    pub range: usize,
    pub gain: usize,
    pub second: usize,
    pub idle: usize,
    pub trim: usize,
    pub drift: usize,
}

/// Declare the voice's amplifier: keys `amp_type` ….
pub fn declare(b: &mut Builder) -> VcaIx {
    b.group("amp", "the voice's amplifier, after the filters. clean multiplies by the amp envelope. analog is the chip's dual gain cell, one per channel: the envelope drives its linear or exponential input, gain the other; idle sets its class");
    VcaIx {
        kind: b.choice("amp_type".into(), "type", TYPES, 0, ""),
        law: b.choice("amp_law".into(), "law", LAWS, 0, "analog: which input the amp envelope drives"),
        range: b.num("amp_range".into(), "range", 20.0, 120.0, 60.0, Db, "analog, exponential: the envelope's span"),
        gain: b.num("amp_gain".into(), "gain", -60.0, 20.0, 0.0, Db, "analog: the other control input"),
        second: b.num("amp_second".into(), "second", -60.0, 20.0, 0.0, Db, "analog: the second (right) cell's own control over the first"),
        idle: b.log("amp_idle".into(), "idle", 1.0, 200.0, 10.0, Unit::None, "analog: idle current in uA, 1 class B (grittier, quietest control bleed) to 100+ class A (cleanest, most control bleed)"),
        trim: b.flag("amp_trim".into(), "trim", false, "analog: the distortion and control feedthrough are trimmed out"),
        drift: b.num("amp_drift".into(), "drift", 0.0, 1.0, 0.5, Percent, "analog: the part's errors: 0 ideal, 0.5 typical, 1 worst"),
    }
}

/// A block's settings for the pair.
#[derive(Clone, Copy, Debug)]
pub struct VcaBlock {
    /// Idle current, uA (1 class B .. 200 class A).
    pub idle: f32,
    /// The distortion and feedthrough trims are done.
    pub trim: bool,
    pub drift: f32,
}

/// What a block's idle current and trim make of the sheet's figures.
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    even: f32,
    soft: f32,
    /// Crossover: depth and width of the gain's dip around zero.
    dip: f32,
    width: f32,
    limit: f32,
    feed: f32,
    gain: f32,
    /// The gain ramp.
    g: f32,
    dg: f32,
}

impl Cell {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        self.g += self.dg;
        let g = self.g;
        // Input plus output peak current against the cell's limit.
        let total = x.abs() * (1.0 + g);
        let x = if total > self.limit * 0.8 {
            let knee = self.limit * 0.8;
            let u = (total - knee) / (self.limit - knee);
            x * (knee + (self.limit - knee) * u / (1.0 + u)) / total
        } else {
            x
        };
        let x2 = x * x;
        let cross = x * (1.0 - self.dip * self.width / (x2 + self.width));
        let even = self.even * ((x2 + self.soft * self.soft).sqrt() - self.soft);
        (cross + even) * g * self.gain + self.feed * (1.0 - g)
    }
}

/// The two cells (one a channel); noise-free, stateless but for the gain.
#[derive(Clone, Copy, Debug)]
pub struct Vca {
    cell: [Cell; 2],
    seed: u32,
    primed: bool,
}

impl VcaBlock {
    pub fn read(v: &(impl Values + ?Sized), x: &VcaIx) -> VcaBlock {
        VcaBlock { idle: v.val(x.idle), trim: v.on(x.trim), drift: v.val(x.drift) }
    }
}

impl Vca {
    pub fn new(seed: u32) -> Vca {
        Vca { cell: [Cell { limit: 6.3, gain: 1.0, ..Cell::default() }; 2], seed, primed: false }
    }

    /// The next note starts from its first block's gain.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Settle a block of `frames`: the gain of each cell by the block's end.
    #[inline]
    pub fn prepare(&mut self, b: &VcaBlock, gains: [f32; 2], frames: usize) {
        let idle = b.idle.clamp(1.0, 200.0);
        // 0 at class B (1 uA), 1 at class A (100 uA), in ratio.
        let class = (idle.ln() / 4.605_17).min(1.15);
        let lerp_ratio = |b: f32, a: f32| b * (a / b).powf(class);
        let d = b.drift;
        let dip = lerp_ratio(CROSS_DEPTH_B, CROSS_DEPTH_A);
        let feed = if b.trim { lerp_ratio(0.01, 1.0) } else { lerp_ratio(0.2, 7.0) } / UNIT_UA;
        let even = EVEN_UNTRIMMED * if b.trim { TRIM_EVEN } else { 1.0 };
        let limit = lerp_ratio(600.0, 1400.0) / UNIT_UA;
        let inv = 1.0 / frames.max(1) as f32;
        for ch in 0..2 {
            let id = self.seed.wrapping_mul(31).wrapping_add(ch as u32);
            let c = &mut self.cell[ch];
            c.even = even * (1.0 + 0.5 * part(id, 0) * tol(d, 0.0, 0.3, 1.0));
            c.soft = EVEN_SOFT_PER_UA * idle;
            c.dip = dip;
            c.width = CROSS_WIDTH * CROSS_WIDTH;
            c.limit = limit;
            c.feed = feed * part(id, 1).signum() * tol(d, 0.0, 1.0, 3.0);
            // Cell current gain 0.83..1.2 at the worst.
            c.gain = 1.0 + part(id, 2) * tol(d, 0.0, 0.03, 0.18);
            if !self.primed {
                c.g = gains[ch];
            }
            c.dg = (gains[ch] - c.g) * inv;
        }
        self.primed = true;
    }

    #[inline]
    pub fn tick(&mut self, x: [f32; 2]) -> [f32; 2] {
        [self.cell[0].tick(x[0]), self.cell[1].tick(x[1])]
    }
}

/// Crossover's gain dip near zero (calibrated in the tests to the sheet's
/// trimmed distortion, which is what is left once the even part is gone:
/// 0.2% at class B, 0.05% at class A, signal 10 dB below clipping).
const CROSS_DEPTH_B: f32 = 0.122;
const CROSS_DEPTH_A: f32 = 0.058;
const CROSS_WIDTH: f32 = 0.03;
