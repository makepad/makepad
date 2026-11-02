//! The kit's envelope: delay, attack, hold, decay, sustain, release, with a
//! curve on each moving segment (0 straight, + slow start, - fast start).
//! Advanced a block at a time (its level is ramped across the block by
//! whoever reads it).

use super::analog::eg::{Eg, EgBlock, EgStage};
use super::schema::{Builder, Unit::{self, Percent, Secs}, Values};

pub const MODELS: &[&str] = &["curve", "analog"];

#[derive(Clone, Debug, Default)]
pub struct EnvIx {
    pub delay: usize,
    pub attack: usize,
    pub hold: usize,
    pub decay: usize,
    pub sustain: usize,
    pub release: usize,
    pub a_curve: usize,
    pub d_curve: usize,
    pub r_curve: usize,
    pub model: usize,
    pub drift: usize,
    pub slow_gate: usize,
}

/// Declare envelope `n` (1-based): keys `env1_attack` …; `amp` makes the
/// amp envelope's defaults.
pub fn declare(b: &mut Builder, n: usize, amp: bool) -> EnvIx {
    let k = |s: &str| format!("env{n}_{s}");
    b.group(&format!("env{n}"), if amp { "the amp envelope (also a mod source); curves: 0 straight, + slow start, - fast start" } else { "a mod envelope (route it with mods)" });
    EnvIx {
        delay: b.log(k("delay"), "delay", 0.0001, 4.0, 0.0001, Secs, ""),
        attack: b.log(k("attack"), "attack", 0.0005, 10.0, if amp { 0.003 } else { 0.01 }, Secs, ""),
        hold: b.log(k("hold"), "hold", 0.0001, 4.0, 0.0001, Secs, ""),
        decay: b.log(k("decay"), "decay", 0.001, 20.0, if amp { 1.0 } else { 0.4 }, Secs, ""),
        sustain: b.num(k("sustain"), "sustain", 0.0, 1.0, if amp { 1.0 } else { 0.0 }, Percent, ""),
        release: b.log(k("release"), "release", 0.001, 20.0, if amp { 0.15 } else { 0.3 }, Secs, ""),
        a_curve: b.num(k("a_curve"), "a curve", -1.0, 1.0, 0.0, Unit::None, ""),
        d_curve: b.num(k("d_curve"), "d curve", -1.0, 1.0, -0.6, Unit::None, ""),
        r_curve: b.num(k("r_curve"), "r curve", -1.0, 1.0, -0.6, Unit::None, ""),
        model: b.choice(k("model"), "model", MODELS, 0, "curve: the times are segment lengths, shaped by the curves. analog: the chip's true RC segments; attack, decay and release are time constants (the attack reaches its peak in 1.47 of its time, charging towards 1.3), delay, hold and curves unused, legato gates rise to the sustain level"),
        drift: b.num(k("drift"), "drift", 0.0, 1.0, 0.5, Percent, "analog: the part's error, 0 ideal, 0.5 typical, 1 worst"),
        slow_gate: b.flag(k("slow_gate"), "slow gate", false, "analog: a gate without a trigger rises at the decay rate, not at once"),
    }
}

/// A 0..1 rise bent by `c` (-1..1): + starts slow, - starts fast.
#[inline]
pub fn bend(x: f32, c: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if c >= 0.0 {
        x.powf(1.0 + 7.0 * c)
    } else {
        1.0 - (1.0 - x).powf(1.0 - 7.0 * c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Idle,
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}

/// One envelope's times and curves for a block.
#[derive(Clone, Copy, Debug)]
pub struct EnvBlock {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub curves: [f32; 3],
    pub analog: bool,
    pub drift: f32,
    pub slow_gate: bool,
}

impl EnvBlock {
    /// `scale` stretches every time (a note's random envelope amount).
    pub fn read(v: &(impl Values + ?Sized), x: &EnvIx, scale: f32) -> EnvBlock {
        EnvBlock {
            delay: v.val(x.delay) * scale,
            attack: v.val(x.attack) * scale,
            hold: v.val(x.hold) * scale,
            decay: v.val(x.decay) * scale,
            sustain: v.val(x.sustain).clamp(0.0, 1.0),
            release: v.val(x.release) * scale,
            curves: [v.val(x.a_curve), v.val(x.d_curve), v.val(x.r_curve)],
            analog: v.index(x.model) == 1,
            drift: v.val(x.drift),
            slow_gate: v.on(x.slow_gate),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Env {
    stage: Stage,
    t: f32,
    level: f32,
    from: f32,
    /// The analog model, gated alongside.
    eg: Eg,
}

impl Env {
    /// `seed`: the analog model's part.
    pub fn new(seed: u32) -> Env {
        Env { eg: Eg::new(seed), ..Env::default() }
    }

    /// Gate on: from the start, or (`legato`, sounding) where it is.
    pub fn gate_on(&mut self, legato: bool) {
        // Legato is a gate without a trigger.
        self.eg.gate_up();
        if !legato {
            self.eg.trigger();
        }
        if legato && self.stage != Stage::Idle && self.stage != Stage::Release {
            return;
        }
        self.from = self.level;
        self.stage = Stage::Delay;
        self.t = 0.0;
    }

    pub fn gate_off(&mut self) {
        self.eg.gate_down();
        if self.stage != Stage::Idle && self.stage != Stage::Release {
            self.from = self.level;
            self.stage = Stage::Release;
            self.t = 0.0;
        }
    }

    pub fn kill(&mut self) {
        *self = Env::new(self.eg.seed);
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn active(&self) -> bool {
        self.stage != Stage::Idle
    }

    pub fn releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    /// Move on `dt` seconds; the level at the end.
    pub fn advance(&mut self, e: &EnvBlock, dt: f32) -> f32 {
        if e.analog {
            let blk = EgBlock { attack: e.attack, decay: e.decay, release: e.release, sustain: e.sustain, slow_gate: e.slow_gate, drift: e.drift };
            self.level = self.eg.advance(&blk, dt);
            self.stage = match self.eg.stage {
                EgStage::Idle => Stage::Idle,
                EgStage::Attack => Stage::Attack,
                EgStage::Settle | EgStage::Lift => Stage::Sustain,
                EgStage::Release => Stage::Release,
            };
            return self.level;
        }
        let mut dt = dt;
        // Walk through as many stages as the step crosses.
        for _ in 0..6 {
            let (len, next) = match self.stage {
                Stage::Idle => {
                    self.level = 0.0;
                    return 0.0;
                }
                Stage::Delay => (e.delay, Stage::Attack),
                Stage::Attack => (e.attack, Stage::Hold),
                Stage::Hold => (e.hold, Stage::Decay),
                Stage::Decay => (e.decay, Stage::Sustain),
                Stage::Sustain => {
                    self.level = e.sustain;
                    return self.level;
                }
                Stage::Release => (e.release, Stage::Idle),
            };
            let len = len.max(1e-5);
            let left = len - self.t;
            let step = dt.min(left);
            self.t += step;
            dt -= step;
            let x = self.t / len;
            self.level = match self.stage {
                Stage::Delay => self.from,
                Stage::Attack => self.from + (1.0 - self.from) * bend(x, e.curves[0]),
                Stage::Hold => 1.0,
                Stage::Decay => 1.0 - (1.0 - e.sustain) * bend(x, e.curves[1]),
                Stage::Release => self.from * (1.0 - bend(x, e.curves[2])),
                _ => self.level,
            };
            if self.t >= len {
                if self.stage == Stage::Attack {
                    self.from = 1.0;
                }
                self.stage = next;
                self.t = 0.0;
                if next == Stage::Idle {
                    self.level = 0.0;
                    return 0.0;
                }
            }
            if dt <= 0.0 {
                break;
            }
        }
        self.level
    }
}
