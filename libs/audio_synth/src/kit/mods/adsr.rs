//! The two per-voice ADSR envelopes (Amp EG, Filter EG), spec section 4:
//! digital mode (shaped linear stages, sustain reached by a rate-limited
//! glide) and analog mode (a capacitor charged toward a gate), gated
//! ("freeze") release and the fast ("uber") release used on voice steal.

use super::time::{analog_coef, at_min, stage_rate};
use super::ModCtx;

/// Raw parameter values of one ADSR (log2 s times, 0..1 sustain).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdsrParams {
    pub attack: f32,
    /// 0 Convex, 1 Linear, 2 Concave.
    pub attack_shape: i32,
    pub decay: f32,
    /// 0 Linear, 1 Quadratic, 2 Cubic.
    pub decay_shape: i32,
    pub sustain: f32,
    pub release: f32,
    /// 0 Linear, 1 Quadratic, 2 Cubic.
    pub release_shape: i32,
    /// 0 Digital, 1 Analog.
    pub mode: i32,
    /// Amp EG release's deform flag: hold the note-off level through the
    /// release, drop to 0 when it ends.
    pub gated_release: bool,
    pub attack_sync: bool,
    pub decay_sync: bool,
    pub release_sync: bool,
}

impl Default for AdsrParams {
    /// The init patch's EG.
    fn default() -> Self {
        AdsrParams {
            attack: -8.0,
            attack_shape: 1,
            decay: -2.0,
            decay_shape: 1,
            sustain: 1.0,
            release: -5.0,
            release_shape: 2,
            mode: 0,
            gated_release: false,
            attack_sync: false,
            decay_sync: false,
            release_sync: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Release,
    FastRelease,
    Idle,
}

/// The analog EG's decay target is the sustain value squared; mirrored.
const QUIRK_ANALOG_SUSTAIN_SQUARED: bool = true;
/// The analog EG's discharge flag reads the capacitor of the previous block
/// edge, so the attack overshoots 1 by a block or two; mirrored.
const QUIRK_ANALOG_DISCHARGE_LAGS: bool = true;
/// The digital quadratic decay drops its lower bound to 0 near silence (and
/// to the sustain level for very fast rates) to avoid a slow tail; mirrored.
const QUIRK_QUAD_DECAY_FLOOR: bool = true;

/// The fixed fast-release time (log2 s, ~11 ms).
const FAST_RELEASE_TIME: f32 = -6.5;
/// Analog mode's charge voltage.
const VCC: f32 = 1.5;
/// Mirrors the reference: the analog fast release uses the release
/// coefficient 6, so the capacitor overshoots below 0 and the envelope goes
/// idle in that same block (a one-block fade through the VCA ramp).
const QUIRK_ANALOG_FAST_RELEASE_COEF: f32 = 6.0;

/// One ADSR's state.
#[derive(Clone, Copy, Debug)]
pub struct Adsr {
    stage: Stage,
    analog: bool,
    /// Digital: phase / level; analog: the capacitor.
    phi: f32,
    c_prev: f32,
    discharge: bool,
    hold: f32,
    scale: f32,
    gated: bool,
    out: f32,
    idle_reported: bool,
}

impl Default for Adsr {
    fn default() -> Self {
        Adsr::new()
    }
}

impl Adsr {
    /// An idle envelope at 0.
    pub fn new() -> Adsr {
        Adsr {
            stage: Stage::Idle,
            analog: false,
            phi: 0.0,
            c_prev: 0.0,
            discharge: false,
            hold: 0.0,
            scale: 0.0,
            gated: false,
            out: 0.0,
            idle_reported: true,
        }
    }

    /// Note-on: start the attack from 0, or from the current output when
    /// `from_current` (legato / one-voice-per-key restarts).
    pub fn attack(&mut self, p: &AdsrParams, from_current: bool) {
        let y0 = if from_current { self.out.clamp(0.0, if p.mode == 1 { f32::MAX } else { 1.0 }) } else { 0.0 };
        self.analog = p.mode == 1;
        self.gated = true;
        self.idle_reported = false;
        self.out = y0;
        self.hold = y0;
        if self.analog {
            self.phi = y0;
            self.c_prev = y0;
            self.discharge = false;
            self.stage = Stage::Attack;
            return;
        }
        let y0 = y0.clamp(0.0, 1.0);
        self.phi = match p.attack_shape {
            0 => y0 * y0,
            2 => y0.sqrt(),
            _ => y0,
        };
        self.stage = Stage::Attack;
        if at_min(p.attack) {
            self.stage = Stage::Decay;
            self.phi = 1.0;
            self.out = 1.0;
        }
    }

    /// An EG retrigger request (step-seq bit, MSEG flag, formula): restart
    /// the attack from `from_current`'s level unless already releasing.
    pub fn retrigger(&mut self, p: &AdsrParams, from_current: bool) {
        if self.gated && self.stage != Stage::Idle {
            self.attack(p, from_current);
        }
    }

    /// Note-off.
    pub fn release(&mut self) {
        if !self.gated {
            return;
        }
        self.gated = false;
        if !self.analog {
            if matches!(self.stage, Stage::Attack | Stage::Decay) {
                self.scale = self.out;
                self.phi = 1.0;
                self.stage = Stage::Release;
            }
        }
    }

    /// Voice steal / kill: a fixed ~11 ms release (never gated).
    pub fn uber_release(&mut self) {
        if self.stage == Stage::Idle {
            return;
        }
        self.gated = false;
        if !self.analog {
            self.scale = self.out;
            self.phi = 1.0;
        }
        self.stage = Stage::FastRelease;
    }

    /// The output (0..1 digital; analog may overshoot 1 by one block).
    #[inline]
    pub fn output(&self) -> f32 {
        self.out
    }

    /// Reported one block after the envelope reached idle.
    #[inline]
    pub fn idle(&self) -> bool {
        self.idle_reported
    }

    /// Whether the envelope is in a release (note-off, fast or done).
    #[inline]
    pub fn released(&self) -> bool {
        !self.gated
    }

    /// Advance one block; returns the new output.
    pub fn process_block(&mut self, p: &AdsrParams, ctx: &ModCtx) -> f32 {
        if self.stage == Stage::Idle {
            self.out = 0.0;
            self.idle_reported = true;
            return 0.0;
        }
        if self.analog { self.analog_block(p, ctx) } else { self.digital_block(p, ctx) }
        self.out
    }

    fn digital_block(&mut self, p: &AdsrParams, ctx: &ModCtx) {
        let y = match self.stage {
            Stage::Attack => {
                self.phi += stage_rate(p.attack, p.attack_sync, ctx);
                if self.phi >= 1.0 {
                    self.phi = 1.0;
                    self.stage = Stage::Decay;
                }
                match p.attack_shape {
                    0 => self.phi.max(0.0).sqrt(),
                    2 => self.phi * self.phi,
                    _ => self.phi,
                }
            }
            Stage::Decay => {
                let r = stage_rate(p.decay, p.decay_sync, ctx);
                let s = p.sustain;
                let phi = self.phi;
                let (mut lo, hi) = match p.decay_shape {
                    0 => (phi - r, phi + r),
                    1 => {
                        let u = phi.max(0.0).sqrt();
                        ((u - r) * (u - r), (u + r) * (u + r))
                    }
                    _ => {
                        let u = phi.cbrt();
                        ((u - r).powi(3), (u + r).powi(3))
                    }
                };
                if QUIRK_QUAD_DECAY_FLOOR && p.decay_shape == 1 {
                    if (s < 1e-3 && phi < 1e-4) || (s == 0.0 && p.decay < -7.0) {
                        lo = 0.0;
                    }
                    if r > 1.0 && lo > s {
                        lo = s;
                    }
                }
                self.phi = s.min(hi).max(lo);
                self.phi
            }
            Stage::Release | Stage::FastRelease => {
                let fast = self.stage == Stage::FastRelease;
                let r = if fast { stage_rate(FAST_RELEASE_TIME, false, ctx) } else { stage_rate(p.release, p.release_sync, ctx) };
                self.phi -= r;
                if self.phi < 0.0 {
                    self.stage = Stage::Idle;
                    0.0
                } else if p.gated_release && fast {
                    // Gated release holds its last output through the fast release, then cuts to 0.
                    self.out
                } else if p.gated_release {
                    self.scale
                } else {
                    self.scale * self.phi.powi(p.release_shape.clamp(0, 2) + 1)
                }
            }
            Stage::Idle => 0.0,
        };
        self.out = y.clamp(0.0, 1.0);
    }

    fn analog_block(&mut self, p: &AdsrParams, ctx: &ModCtx) {
        let gate = self.gated;
        let fast = self.stage == Stage::FastRelease;
        let c = self.phi;
        let edge = if QUIRK_ANALOG_DISCHARGE_LAGS { self.c_prev } else { c };
        self.discharge = (edge > 1.0 || self.discharge) && gate;
        self.c_prev = c;
        let s = p.sustain.clamp(0.0, 1.0);
        let s2 = if QUIRK_ANALOG_SUSTAIN_SQUARED { s * s } else { s };
        let va = if gate && !self.discharge { VCC } else { 0.0 };
        let vd = if self.discharge { s2 } else { VCC };
        let vr = if gate { VCC } else { 0.0 };
        let da = (va - c).max(0.0);
        let dd = if self.discharge && gate { vd - c } else { (vd - c).min(0.0) };
        let dr = (vr - c).min(0.0);
        let ka = analog_coef(p.attack, p.attack_sync, ctx);
        let kd = analog_coef(p.decay, p.decay_sync, ctx);
        let kr = if fast { QUIRK_ANALOG_FAST_RELEASE_COEF } else { analog_coef(p.release, p.release_sync, ctx) };
        let mut c = c + da * ka;
        c += dd * kd;
        c += dr * kr;
        self.phi = c;
        if gate {
            self.hold = c;
        }
        if !gate && !self.discharge && c < 1e-6 {
            self.phi = 0.0;
            self.out = 0.0;
            self.stage = Stage::Idle;
            return;
        }
        self.out = if !gate && p.gated_release && !fast { self.hold } else { c };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ModCtx {
        ModCtx::new(48000.0)
    }

    /// Blocks until the output first reaches 1 (digital) from a note-on.
    fn attack_blocks(p: &AdsrParams, ctx: &ModCtx) -> usize {
        let mut e = Adsr::new();
        e.attack(p, false);
        for n in 1..100_000 {
            if e.process_block(p, ctx) >= 1.0 {
                return n;
            }
        }
        panic!("no peak");
    }

    #[test]
    fn attack_peaks_on_time() {
        let ctx = ctx();
        for shape in 0..3 {
            for v in [-6.0f32, -4.5, -2.0, -0.3, 0.0, 1.0, 2.0] {
                let p = AdsrParams { attack: v, attack_shape: shape, ..AdsrParams::default() };
                let want = (v.exp2() * ctx.fs / 32.0).ceil() as i64;
                let got = attack_blocks(&p, &ctx) as i64;
                assert!((got - want).abs() <= 1, "shape {shape} v {v}: {got} vs {want}");
            }
        }
        // Attack at the minimum is skipped.
        let mut e = Adsr::new();
        e.attack(&AdsrParams::default(), false);
        assert_eq!(e.output(), 1.0);
    }

    #[test]
    fn tempo_sync_halves_attack_at_240() {
        let ctx = ctx().with_bpm(240.0);
        let p = AdsrParams { attack: 0.0, attack_sync: true, ..AdsrParams::default() };
        let want = (0.5 * ctx.fs / 32.0).ceil() as i64;
        assert!((attack_blocks(&p, &ctx) as i64 - want).abs() <= 1);
    }

    #[test]
    fn attack_shapes() {
        let ctx = ctx();
        for (shape, f) in [(0, 0.5f32.sqrt()), (1, 0.5), (2, 0.25)] {
            let p = AdsrParams { attack: 0.0, attack_shape: shape, ..AdsrParams::default() };
            let mut e = Adsr::new();
            e.attack(&p, false);
            let half = (0.5 * ctx.fs / 32.0) as usize;
            for _ in 0..half {
                e.process_block(&p, &ctx);
            }
            assert!((e.output() - f).abs() < 2e-3, "{shape} {}", e.output());
        }
    }

    #[test]
    fn decay_reaches_sustain_and_follows_it() {
        let ctx = ctx();
        for shape in 0..3 {
            let p = AdsrParams { decay: -1.0, decay_shape: shape, sustain: 0.3, ..AdsrParams::default() };
            let mut e = Adsr::new();
            e.attack(&p, false);
            let n = (0.5 * ctx.fs / 32.0) as usize;
            for _ in 0..n + 4 {
                e.process_block(&p, &ctx);
            }
            assert!((e.output() - 0.3).abs() < 1e-6, "{shape} {}", e.output());
            // Moving sustain up: the level glides back up.
            let p2 = AdsrParams { sustain: 0.6, ..p };
            e.process_block(&p2, &ctx);
            assert!(e.output() > 0.3 && e.output() < 0.6);
        }
    }

    #[test]
    fn release_times_and_shapes() {
        let ctx = ctx();
        for shape in 0..3 {
            for v in [-5.0f32, -2.0, 0.0, 1.0] {
                let p = AdsrParams { release: v, release_shape: shape, ..AdsrParams::default() };
                let mut e = Adsr::new();
                e.attack(&p, false);
                e.process_block(&p, &ctx);
                e.release();
                let want = (v.exp2() * ctx.fs / 32.0).ceil() as i64;
                let mut n = 0i64;
                let mut prev = 1.0;
                let half = want / 2;
                while e.process_block(&p, &ctx) > 0.0 {
                    n += 1;
                    assert!(e.output() <= prev);
                    prev = e.output();
                    if n == half {
                        let want_mid = 0.5f32.powi(shape + 1);
                        assert!((e.output() - want_mid).abs() < 0.01, "{shape} {v} {}", e.output());
                    }
                }
                assert!((n + 1 - want).abs() <= 1, "shape {shape} v {v}: {} vs {want}", n + 1);
                assert!(!e.idle());
                e.process_block(&p, &ctx);
                assert!(e.idle());
            }
        }
    }

    #[test]
    fn gated_release_holds_then_drops() {
        let ctx = ctx();
        let p = AdsrParams { decay: -3.0, sustain: 0.5, release: -2.0, gated_release: true, ..AdsrParams::default() };
        let mut e = Adsr::new();
        e.attack(&p, false);
        for _ in 0..500 {
            e.process_block(&p, &ctx);
        }
        e.release();
        let want = (0.25 * ctx.fs / 32.0).ceil() as usize;
        for _ in 0..want - 2 {
            assert!((e.process_block(&p, &ctx) - 0.5).abs() < 1e-6);
        }
        for _ in 0..4 {
            e.process_block(&p, &ctx);
        }
        assert_eq!(e.output(), 0.0);
    }

    #[test]
    fn uber_release_is_fast() {
        let ctx = ctx();
        let p = AdsrParams { release: 3.0, ..AdsrParams::default() };
        let mut e = Adsr::new();
        e.attack(&p, false);
        e.process_block(&p, &ctx);
        e.uber_release();
        let want = ((-6.5f32).exp2() * ctx.fs / 32.0).ceil() as i64;
        let mut n = 1;
        while e.process_block(&p, &ctx) > 0.0 {
            n += 1;
        }
        assert!((n - want).abs() <= 1, "{n} {want}");
    }

    #[test]
    fn retrigger_ignored_in_release() {
        let ctx = ctx();
        let p = AdsrParams { attack: -2.0, release: 0.0, ..AdsrParams::default() };
        let mut e = Adsr::new();
        e.attack(&p, false);
        for _ in 0..1000 {
            e.process_block(&p, &ctx);
        }
        e.retrigger(&p, false);
        assert_eq!(e.output(), 0.0);
        e.attack(&p, false);
        for _ in 0..1000 {
            e.process_block(&p, &ctx);
        }
        e.release();
        e.process_block(&p, &ctx);
        let before = e.output();
        e.retrigger(&p, false);
        assert_eq!(e.output(), before);
        // From-current restart keeps the level.
        let mut f = Adsr::new();
        f.attack(&p, false);
        for _ in 0..20 {
            f.process_block(&p, &ctx);
        }
        let lvl = f.output();
        f.retrigger(&p, true);
        assert_eq!(f.output(), lvl);
        f.process_block(&p, &ctx);
        assert!(f.output() > lvl);
    }

    #[test]
    fn analog_curve_is_sane() {
        let ctx = ctx();
        let p = AdsrParams { mode: 1, attack: -3.0, decay: -2.0, sustain: 0.6, release: -2.0, ..AdsrParams::default() };
        let mut e = Adsr::new();
        e.attack(&p, false);
        // Attack: rises with decreasing slope until it crosses 1.
        let mut prev = 0.0f32;
        let mut prev_d = f32::MAX;
        let mut n = 0;
        while e.output() <= 1.0 {
            e.process_block(&p, &ctx);
            let d = e.output() - prev;
            assert!(d > 0.0 && d <= prev_d + 1e-6, "block {n}");
            prev_d = d;
            prev = e.output();
            n += 1;
            assert!(n < 10_000);
        }
        // The discharge flag reads the capacitor two block edges back, so
        // the charge overshoots 1 by at most two blocks' steps.
        e.process_block(&p, &ctx);
        assert!(e.output() < 1.0 + 2.0 * prev_d + 1e-6);
        // Decay settles on sustain squared, monotonic down.
        let mut prev = e.output();
        for _ in 0..3000 {
            e.process_block(&p, &ctx);
            assert!(e.output() <= prev + 1e-6);
            prev = e.output();
        }
        assert!((e.output() - 0.36).abs() < 1e-3, "{}", e.output());
        // Release: monotonic to idle.
        e.release();
        let mut n = 0;
        while !e.idle() {
            e.process_block(&p, &ctx);
            assert!(e.output() <= prev + 1e-7);
            prev = e.output();
            n += 1;
            assert!(n < 100_000);
        }
        assert_eq!(e.output(), 0.0);
    }

    #[test]
    fn analog_gated_release_holds() {
        let ctx = ctx();
        let p = AdsrParams { mode: 1, sustain: 0.8, release: -1.0, gated_release: true, ..AdsrParams::default() };
        let mut e = Adsr::new();
        e.attack(&p, false);
        for _ in 0..3000 {
            e.process_block(&p, &ctx);
        }
        let held = e.output();
        e.release();
        for _ in 0..50 {
            assert_eq!(e.process_block(&p, &ctx), held);
        }
    }
}
