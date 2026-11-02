//! The envelope generator: true RC segments between the sheet's voltages,
//! normalised so the attack peak (5 V) is 1.
//!
//! Attack charges towards the 6.5 V asymptote and is cut off at the peak by
//! a comparator; decay and sustain are one segment that approaches the
//! sustain level from either side; release approaches zero. One current
//! sets all three, so one time constant means the same on each.
//!
//! Gate and trigger are separate, as on the sheet's waveforms:
//! - gate up with a trigger: attack from wherever the output is;
//! - a trigger while the gate is up: attack again from where it is;
//! - gate up with no trigger: straight up to the sustain level and hold (or,
//!   with the charge transistor disabled, up at the decay rate);
//! - a trigger with the gate down: nothing (the gate holds the latch reset);
//! - gate down at any point: release from there.

use super::{part, tol};

/// Attack asymptote over the peak (6.5 V / 5 V).
pub const ASYMPTOTE: f32 = 1.3;
/// The sheet's time range and the control law: 0..-5 V at the input is 2 ms
/// to 20 s.
pub const TIME_MIN: f32 = 0.0004;
pub const TIME_MAX: f32 = 20.0;
/// How fast the charge transistor lifts the output to the sustain level on
/// a gate without a trigger (the sheet shows it, without a figure).
const FAST_RISE: f32 = 0.001;
/// Where a release counts as finished, above its final voltage.
const DONE: f32 = 1e-4;

/// The sheet's control law: the time constant for a control input of
/// `volts` (0..5, magnitude; four decades over five volts: 60 mV per decade
/// at the pin behind the input divider).
pub fn time_for_control(volts: f32) -> f32 {
    0.002 * 10f32.powf(volts * 4.0 / 5.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EgStage {
    #[default]
    Idle,
    Attack,
    /// Decay and sustain: the output settling on the sustain level.
    Settle,
    /// Gate without trigger, on the way up to the sustain level.
    Lift,
    Release,
}

/// One block's settings.
#[derive(Clone, Copy, Debug)]
pub struct EgBlock {
    /// Time constants, seconds.
    pub attack: f32,
    pub decay: f32,
    pub release: f32,
    pub sustain: f32,
    /// A gate without a trigger rises at the decay rate.
    pub slow_gate: bool,
    pub drift: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Eg {
    pub stage: EgStage,
    pub level: f32,
    gate: bool,
    pub seed: u32,
}

impl Eg {
    pub fn new(seed: u32) -> Eg {
        Eg { seed, ..Eg::default() }
    }

    pub fn gate_up(&mut self) {
        self.gate = true;
        if !matches!(self.stage, EgStage::Attack | EgStage::Settle | EgStage::Lift) {
            self.stage = EgStage::Lift;
        }
    }

    /// Only a raised gate lets the trigger set the attack latch.
    pub fn trigger(&mut self) {
        if self.gate {
            self.stage = EgStage::Attack;
        }
    }

    pub fn gate_down(&mut self) {
        self.gate = false;
        if self.stage != EgStage::Idle {
            self.stage = EgStage::Release;
        }
    }

    pub fn kill(&mut self) {
        *self = Eg::new(self.seed);
    }

    pub fn active(&self) -> bool {
        self.stage != EgStage::Idle
    }

    /// Move on `dt` seconds exactly (the segments are solved, not stepped);
    /// the level at the end.
    pub fn advance(&mut self, e: &EgBlock, dt: f32) -> f32 {
        let d = e.drift;
        let t = |x: f32| x.clamp(TIME_MIN, TIME_MAX);
        // Charge and discharge current over input current: 0.75..1.3 and
        // 0.83..1.2 at the worst.
        let attack = t(e.attack) * (1.0 + part(self.seed, 0) * tol(d, 0.0, 0.08, 0.28));
        let decay = t(e.decay) * (1.0 + part(self.seed, 1) * tol(d, 0.0, 0.05, 0.19));
        let release = t(e.release) * (1.0 + part(self.seed, 2) * tol(d, 0.0, 0.05, 0.19));
        // Peak 4.7..5.3 V; final voltages +10 mV typ, +23 mV max.
        let peak = 1.0 + part(self.seed, 3) * tol(d, 0.0, 0.02, 0.06);
        let asymptote = peak * ASYMPTOTE;
        let offset = tol(d, 0.0, 0.002, 0.0046);
        let sustain = e.sustain.clamp(0.0, 1.0) * peak + offset;
        let mut left = dt;
        for _ in 0..4 {
            match self.stage {
                EgStage::Idle => {
                    self.level = 0.0;
                    return 0.0;
                }
                EgStage::Attack => {
                    if self.level >= peak {
                        self.stage = EgStage::Settle;
                        continue;
                    }
                    // Time to the comparator on this RC curve.
                    let to_peak = attack * ((asymptote - self.level) / (asymptote - peak)).ln();
                    if to_peak >= left {
                        self.level = asymptote + (self.level - asymptote) * (-left / attack).exp();
                        return self.level;
                    }
                    left -= to_peak;
                    self.level = peak;
                    self.stage = EgStage::Settle;
                }
                EgStage::Settle => {
                    self.level = sustain + (self.level - sustain) * (-left / decay).exp();
                    return self.level;
                }
                EgStage::Lift => {
                    if self.level >= sustain {
                        self.stage = EgStage::Settle;
                        continue;
                    }
                    let tau = if e.slow_gate { decay } else { FAST_RISE };
                    self.level = sustain + (self.level - sustain) * (-left / tau).exp();
                    return self.level;
                }
                EgStage::Release => {
                    self.level = offset + (self.level - offset) * (-left / release).exp();
                    if self.level - offset < DONE {
                        self.stage = EgStage::Idle;
                        self.level = 0.0;
                    }
                    return self.level;
                }
            }
        }
        self.level
    }
}
