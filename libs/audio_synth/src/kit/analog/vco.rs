//! The voltage-controlled oscillator: one triangle integrator, and what the
//! sheet derives from it.
//!
//! The core charges a timing capacitor up and down between 0 and 5 V; the
//! saw is the triangle while it rises and the triangle flipped over the
//! 5 V line while it falls, so its reset is the triangle's lower peak; the
//! pulse is a comparator of the triangle against the PWM voltage. Levels are
//! the sheet's: triangle 5 V, saw 10 V, pulse 12 V peak to peak (output
//! units: 10 V p-p = 2.0, each centred on its nominal middle).
//!
//! Sync is the sheet's three kinds, not a phase reset for all of them:
//! `hard` reverses the triangle (a rising edge only while it rises, a
//! falling edge only while it falls), `reset` is the conventional
//! discharge, `soft` lowers the upper threshold for the pulse so the
//! period locks to a multiple of the master's. Linear FM adds to the
//! reference current through a coupling capacitor, and cannot go through
//! zero: at -100% the core stops.
//!
//! Every step and slope change goes into [`BandLine`] with its position
//! inside the sample, whatever caused it.

use super::blep::{self, BandLine};
use super::{part, tol};
use crate::dsp::Rng;

pub const SYNCS: &[&str] = &["off", "hard", "hard_rise", "hard_fall", "reset", "soft"];
const S_HARD: u8 = 1;
const S_RISE: u8 = 2;
const S_FALL: u8 = 3;
const S_RESET: u8 = 4;
const S_SOFT: u8 = 5;

/// The sheet's pulse is 12 V where the saw is 10 V p-p (= 2.0 here).
pub const PULSE_PEAK: f32 = 1.2;
const C4: f32 = 261.625_58;
/// The FM input's coupling corner (0.1 uF into 1 M), Hz.
const FM_CORNER: f32 = 1.6;

/// When a master's outputs moved within the last sample, as a fraction of
/// it (negative: not this sample).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edges {
    /// The pulse went up.
    pub rise: f32,
    /// The pulse went down.
    pub fall: f32,
    /// The saw reset.
    pub wrap: f32,
}

pub const NO_EDGES: Edges = Edges { rise: -1.0, fall: -1.0, wrap: -1.0 };

impl Default for Edges {
    fn default() -> Self {
        NO_EDGES
    }
}

/// The part's errors, drawn once for its seed and scaled by `drift`.
#[derive(Clone, Copy, Debug)]
pub struct VcoPart {
    /// Exponential scale error (fraction; sheet 0.2% typ, 1% max).
    scale: f32,
    /// Scale tempco (per degree; sheet 0 typ, 150 ppm max).
    tempco: f32,
    /// Oscillator drift (per degree; sheet 50 ppm typ, 200 max).
    drift: f32,
    /// Comparator delay per cycle, seconds: why the top goes flat (the sheet
    /// names the effect from 5 kHz up and gives no figure; this is inferred
    /// from the scale error it allows at 10 kHz).
    delay: f32,
    /// Fraction of the cycle spent rising (sheet 45..55%).
    sym: f32,
    saw: f32,
    tri: f32,
}

impl VcoPart {
    pub fn new(seed: u32, d: f32) -> VcoPart {
        let e = |i: u32, typ: f32, max: f32| part(seed, i) * tol(d, 0.0, typ, max);
        VcoPart {
            scale: e(0, 0.002, 0.01),
            tempco: e(1, 0.0, 150e-6),
            drift: e(2, 50e-6, 200e-6),
            delay: tol(d, 0.0, 0.5e-6, 2e-6),
            sym: 0.5 + e(3, 0.01, 0.05),
            saw: 1.0 + e(4, 0.01, 0.06),
            tri: 1.0 + e(5, 0.01, 0.03),
        }
    }

    /// The pitch a part at `celsius` from its trim plays for `hz`.
    #[inline]
    pub fn hz(&self, hz: f32, celsius: f32) -> f32 {
        let oct = (hz.max(1e-3) / C4).log2();
        let f = C4 * (oct * (1.0 + self.scale + self.tempco * celsius)).exp2() * (1.0 + self.drift * celsius);
        f / (1.0 + f * self.delay)
    }
}

/// What the core reads per sample (settled a block at a time).
#[derive(Clone, Copy, Debug)]
pub struct VcoBlock {
    /// Cycles a sample.
    pub inc: f32,
    pub saw: f32,
    pub tri: f32,
    pub pulse: f32,
    /// Pulse width 0..1 (0 and 1 are the sheet's 0% and 100%).
    pub width: f32,
    pub sync: u8,
    /// `soft`: how far the master's pulse lowers the threshold, 0..1.
    pub soft: f32,
    /// Linear FM depth (1: the input's peak equals the reference current).
    pub fm: f32,
    /// Exponential FM depth, octaves per unit of input.
    pub fm_exp: f32,
    /// Fraction of the cycle spent rising.
    pub sym: f32,
    /// One sample's step of the FM coupling.
    pub fm_coupling: f32,
}

impl VcoBlock {
    pub fn plain(hz: f32, rate: f32) -> VcoBlock {
        VcoBlock { inc: hz / rate, saw: 1.0, tri: 0.0, pulse: 0.0, width: 0.5, sync: 0, soft: 0.5, fm: 0.0, fm_exp: 0.0, sym: 0.5, fm_coupling: coupling(rate) }
    }

    /// With the part's errors: its pitch at `celsius`, its levels, symmetry.
    #[inline]
    pub fn on_part(mut self, p: &VcoPart, hz: f32, rate: f32, celsius: f32) -> VcoBlock {
        self.inc = p.hz(hz, celsius) / rate;
        self.sym = p.sym;
        self.saw *= p.saw;
        self.tri *= p.tri;
        self
    }
}

#[inline]
pub fn coupling(rate: f32) -> f32 {
    (std::f32::consts::TAU * FM_CORNER / rate).min(1.0)
}

/// One core.
#[derive(Clone, Copy, Debug)]
pub struct Vco {
    /// The triangle, 0..1 (0..5 V); doubles, so a part sweeping 50,000:1
    /// keeps its pitch at the slow end.
    t: f64,
    rising: bool,
    high: bool,
    line: BandLine,
    fm_dc: f32,
    /// What it did in the last sample (a slave's sync input).
    pub edges: Edges,
}

impl Default for Vco {
    fn default() -> Self {
        Vco::new()
    }
}

impl Vco {
    pub fn new() -> Vco {
        blep::ready();
        Vco { t: 0.0, rising: true, high: true, line: BandLine::default(), fm_dc: 0.0, edges: NO_EDGES }
    }

    /// Start at `phase` of a cycle (0: the saw's reset).
    pub fn start(&mut self, phase: f32) {
        let p = phase.rem_euclid(1.0);
        self.rising = p < 0.5;
        self.t = if self.rising { 2.0 * p as f64 } else { 2.0 - 2.0 * p as f64 };
        self.high = self.t < 0.5;
        self.line.clear();
        self.fm_dc = 0.0;
        self.edges = NO_EDGES;
    }

    /// The triangle now, 0..1.
    pub fn triangle(&self) -> f32 {
        self.t as f32
    }

    /// The triangle is on its way up.
    pub fn is_rising(&self) -> bool {
        self.rising
    }

    #[inline]
    fn out(&self, b: &VcoBlock, up: f32, down: f32) -> (f32, f32) {
        let t = self.t as f32;
        let saw = if self.rising { t - 1.0 } else { 1.0 - t };
        let tri = t - 0.5;
        let pulse = if self.high { PULSE_PEAK } else { -PULSE_PEAK };
        let value = b.saw * saw + b.tri * tri + b.pulse * pulse;
        let slope = if self.rising { (b.saw + b.tri) * up } else { (b.saw - b.tri) * down };
        (value, slope)
    }

    /// One sample; `m` is the FM input, `sync` the master's edges.
    #[inline]
    pub fn tick(&mut self, b: &VcoBlock, m: f32, sync: &Edges) -> f32 {
        let mut inc = b.inc;
        if b.fm != 0.0 {
            self.fm_dc += (m - self.fm_dc) * b.fm_coupling;
            // A negative reference current gates the generator off.
            inc *= (1.0 + b.fm * (m - self.fm_dc)).max(0.0);
        }
        if b.fm_exp != 0.0 {
            inc *= (b.fm_exp * m).exp2();
        }
        let inc = inc.min(0.49);
        let up = inc / b.sym;
        let down = inc / (1.0 - b.sym);
        self.edges = NO_EDGES;
        let width = b.width as f64;
        let want = width >= 1.0 || (width > 0.0 && self.t < width);
        if want != self.high {
            // The width moved past the triangle.
            let (y0, s0) = self.out(b, up, down);
            self.set_high(want, 0.0);
            let (y1, s1) = self.out(b, up, down);
            self.line.event(y1 - y0, s1 - s0, 1.0);
        }
        // The sync pulses this sample that can act, in time order.
        let mut pulses = [(2.0f32, 0u8); 2];
        let mut np = 0;
        match b.sync {
            S_HARD | S_RISE if sync.rise >= 0.0 => {
                pulses[0] = (sync.rise, 1);
                np = 1;
            }
            _ => {}
        }
        match b.sync {
            S_HARD | S_FALL if sync.fall >= 0.0 => {
                pulses[np] = (sync.fall, 2);
                np += 1;
            }
            S_RESET if sync.wrap >= 0.0 => {
                pulses[np] = (sync.wrap, 3);
                np += 1;
            }
            S_SOFT if sync.wrap >= 0.0 => {
                pulses[np] = (sync.wrap, 4);
                np += 1;
            }
            _ => {}
        }
        if np == 2 && pulses[1].0 < pulses[0].0 {
            pulses.swap(0, 1);
        }
        let mut next_pulse = 0;
        let mut now = 0.0f64;
        let (rise, fall) = (up as f64, down as f64);
        // Each pass runs to the next thing that happens in this sample.
        for _ in 0..12 {
            let slope = if self.rising { rise } else { -fall };
            let mut dt = 1.0 - now;
            let mut ev = 0u8;
            if slope > 0.0 {
                let d = (1.0 - self.t) / slope;
                if d < dt {
                    dt = d;
                    ev = 1;
                }
                if self.t < width {
                    let d = (width - self.t) / slope;
                    if d < dt {
                        dt = d;
                        ev = 3;
                    }
                }
            } else if slope < 0.0 {
                let d = self.t / -slope;
                if d < dt {
                    dt = d;
                    ev = 2;
                }
                if self.t > width && width > 0.0 {
                    let d = (self.t - width) / -slope;
                    if d < dt {
                        dt = d;
                        ev = 3;
                    }
                }
            }
            if next_pulse < np && pulses[next_pulse].0 as f64 - now < dt {
                dt = (pulses[next_pulse].0 as f64 - now).max(0.0);
                ev = 4;
            }
            now += dt;
            self.t = (self.t + slope * dt).clamp(0.0, 1.0);
            if ev == 0 {
                break;
            }
            let (y0, s0) = self.out(b, up, down);
            match ev {
                1 => {
                    self.t = 1.0;
                    self.rising = false;
                }
                2 => {
                    self.t = 0.0;
                    self.rising = true;
                    if self.edges.wrap < 0.0 {
                        self.edges.wrap = now as f32;
                    }
                }
                3 => {
                    self.t = width;
                    self.set_high(!self.rising, now as f32);
                }
                _ => {
                    match pulses[next_pulse].1 {
                        1 if self.rising => self.rising = false,
                        2 if !self.rising => self.rising = true,
                        3 => {
                            self.t = 0.0;
                            self.rising = true;
                            let high = b.width > 0.0;
                            self.set_high(high, now as f32);
                        }
                        4 if self.rising && self.t >= 1.0 - b.soft as f64 => self.rising = false,
                        _ => {}
                    }
                    next_pulse += 1;
                }
            }
            let (y1, s1) = self.out(b, up, down);
            let (jump, kink) = (y1 - y0, s1 - s0);
            if jump.abs() > 1e-7 || kink.abs() > 1e-10 {
                self.line.event(jump, kink, (1.0 - now) as f32);
            }
        }
        let plain = self.out(b, up, down).0;
        self.line.push(plain)
    }

    #[inline]
    fn set_high(&mut self, high: bool, now: f32) {
        if high != self.high {
            self.high = high;
            if high {
                if self.edges.rise < 0.0 {
                    self.edges.rise = now;
                }
            } else if self.edges.fall < 0.0 {
                self.edges.fall = now;
            }
        }
    }
}

/// A part's temperature and errors alongside its core (one unison voice).
#[derive(Clone, Copy, Debug)]
pub struct VcoUnit {
    pub vco: Vco,
    pub part: VcoPart,
    pub thermal: super::Thermal,
    pub drift_for: f32,
    pub seed: u32,
}

impl VcoUnit {
    pub fn new(seed: u32) -> VcoUnit {
        VcoUnit { vco: Vco::new(), part: VcoPart::new(seed, 0.0), thermal: super::Thermal::new(seed), drift_for: 0.0, seed }
    }

    /// A block's temperature and the part's errors at `drift`.
    #[inline]
    pub fn settle(&mut self, drift: f32, dt: f32) -> f32 {
        if drift != self.drift_for {
            self.drift_for = drift;
            self.part = VcoPart::new(self.seed, drift);
        }
        if drift > 0.0 { self.thermal.step(dt) } else { 0.0 }
    }
}

/// A phase for a new note, from the note's generator (free-running cores).
pub fn free_phase(rng: &mut Rng, phase: f32, rand: f32) -> f32 {
    (phase + rng.unit() * rand).fract()
}
