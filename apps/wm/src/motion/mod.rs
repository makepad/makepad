//! One motion law for every hotkey and gesture action of the window manager.
//!
//! Everything that moves because the person pressed a chord or made a
//! gesture rides on the two primitives here, so it all feels alike and
//! nothing ever jumps:
//!
//! - [`Spring`] / [`SpringRect`]: a critically damped spring. It follows a
//!   target that may change at any moment; a change of target keeps the
//!   position AND the velocity, so an interrupted move bends into the new one
//!   instead of restarting. Window rects (park, unpark, cycle, tidy, shake,
//!   history scrub, move / resize / tile shortcuts, fullscreen enter and
//!   exit), the space slide and its jumps all use it.
//! - [`Ease`]: a fixed-duration move (the space strip sliding in and out).
//!   Retargeting starts the new move from the current value with the current
//!   velocity (a cubic Hermite with a resting end), so it is as continuous as
//!   the spring.
//!
//! Both integrate in closed form, so any frame time gives the same curve.
//! A pointer drag is not an animation: it moves 1:1 (`snap`).
use crate::layout::LRect;

/// The spring's natural frequency (rad/s). Critically damped, so it reaches
/// 1% of a move after about `6.6 / OMEGA` = 0.39 s without overshoot.
pub const OMEGA: f64 = 17.0;
/// A spring this close to its target (px, or index units * 1e3) and this
/// slow is there.
const REST_POS: f64 = 0.05;
const REST_VEL: f64 = 0.5;
/// Longest step integrated at once (s): a stalled frame does not leap.
pub const MAX_DT: f64 = 0.05;

/// One value chasing a target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub x: f64,
    pub v: f64,
    pub target: f64,
}

impl Spring {
    pub fn at(x: f64) -> Spring {
        Spring { x, v: 0.0, target: x }
    }

    /// Chase a new target from wherever the spring is, at the speed it has.
    pub fn retarget(&mut self, target: f64) {
        self.target = target;
    }

    /// Be at `x` now, at rest (a pointer drag, a layout that must not animate).
    pub fn snap(&mut self, x: f64) {
        *self = Spring::at(x);
    }

    pub fn settled(&self) -> bool {
        self.x == self.target && self.v == 0.0
    }

    /// Advance by `dt` seconds; true while it is still moving.
    pub fn step(&mut self, dt: f64) -> bool {
        self.step_scaled(dt, 1.0)
    }

    /// As `step`, with positions measured in units `unit` times the pixel
    /// (the rest threshold scales with it).
    pub fn step_scaled(&mut self, dt: f64, unit: f64) -> bool {
        if self.settled() {
            return false;
        }
        let dt = dt.clamp(0.0, MAX_DT);
        let d0 = self.x - self.target;
        let b = self.v + OMEGA * d0;
        let e = (-OMEGA * dt).exp();
        let d = (d0 + b * dt) * e;
        let v = (self.v - OMEGA * b * dt) * e;
        if d.abs() < REST_POS * unit && v.abs() < REST_VEL * unit {
            self.x = self.target;
            self.v = 0.0;
            return false;
        }
        self.x = self.target + d;
        self.v = v;
        true
    }
}

/// A rect whose four numbers chase a target together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringRect {
    pub x: Spring,
    pub y: Spring,
    pub w: Spring,
    pub h: Spring,
}

fn max_delta(a: LRect, b: LRect) -> f64 {
    (a.x - b.x).abs().max((a.y - b.y).abs()).max((a.w - b.w).abs()).max((a.h - b.h).abs())
}

impl SpringRect {
    pub fn at(r: LRect) -> SpringRect {
        SpringRect { x: Spring::at(r.x), y: Spring::at(r.y), w: Spring::at(r.w), h: Spring::at(r.h) }
    }

    pub fn rect(&self) -> LRect {
        LRect::new(self.x.x, self.y.x, self.w.x, self.h.x)
    }

    pub fn target(&self) -> LRect {
        LRect::new(self.x.target, self.y.target, self.w.target, self.h.target)
    }

    pub fn retarget(&mut self, r: LRect) {
        self.x.retarget(r.x);
        self.y.retarget(r.y);
        self.w.retarget(r.w);
        self.h.retarget(r.h);
    }

    pub fn snap(&mut self, r: LRect) {
        *self = SpringRect::at(r);
    }

    pub fn settled(&self) -> bool {
        self.x.settled() && self.y.settled() && self.w.settled() && self.h.settled()
    }

    pub fn step(&mut self, dt: f64) -> bool {
        let mut moving = false;
        for s in [&mut self.x, &mut self.y, &mut self.w, &mut self.h] {
            moving |= s.step(dt);
        }
        moving
    }

    /// How far from the target (the largest of the four numbers, px).
    pub fn distance(&self) -> f64 {
        max_delta(self.rect(), self.target())
    }
}

/// A fixed-duration move of one value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ease {
    from: f64,
    to: f64,
    /// Velocity at the start (value units per second).
    v0: f64,
    t: f64,
    dur: f64,
}

impl Ease {
    pub fn at(x: f64, dur: f64) -> Ease {
        Ease { from: x, to: x, v0: 0.0, t: dur, dur }
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    fn s(&self) -> f64 {
        if self.dur <= 0.0 { 1.0 } else { (self.t / self.dur).clamp(0.0, 1.0) }
    }

    pub fn value(&self) -> f64 {
        let s = self.s();
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        h00 * self.from + h10 * self.v0 * self.dur + h01 * self.to
    }

    /// Value units per second now.
    pub fn velocity(&self) -> f64 {
        if self.dur <= 0.0 || self.t >= self.dur {
            return 0.0;
        }
        let s = self.s();
        let s2 = s * s;
        let d00 = 6.0 * s2 - 6.0 * s;
        let d10 = 3.0 * s2 - 4.0 * s + 1.0;
        let d01 = -6.0 * s2 + 6.0 * s;
        (d00 * self.from + d10 * self.v0 * self.dur + d01 * self.to) / self.dur
    }

    /// Move to `to` from here, keeping the speed. The same target again is
    /// no restart.
    pub fn retarget(&mut self, to: f64) {
        if to == self.to {
            return;
        }
        let (x, v) = (self.value(), self.velocity());
        *self = Ease { from: x, to, v0: v, t: 0.0, dur: self.dur };
    }

    pub fn snap(&mut self, x: f64) {
        *self = Ease::at(x, self.dur);
    }

    pub fn settled(&self) -> bool {
        self.t >= self.dur
    }

    pub fn step(&mut self, dt: f64) -> bool {
        if self.settled() {
            return false;
        }
        self.t = (self.t + dt.clamp(0.0, MAX_DT)).min(self.dur);
        !self.settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Spring, secs: f64) {
        let mut t = 0.0;
        while t < secs {
            s.step(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn a_spring_arrives_without_overshoot_in_about_0_4_seconds() {
        let mut s = Spring::at(0.0);
        s.retarget(400.0);
        let mut last = 0.0;
        let mut t = 0.0;
        while s.step(1.0 / 60.0) {
            assert!(s.x >= last && s.x <= 400.0, "{}", s.x);
            last = s.x;
            t += 1.0 / 60.0;
            assert!(t < 1.2);
        }
        assert!(s.settled() && s.x == 400.0);
        assert!(t > 0.3 && t < 0.8, "{t}");
    }

    #[test]
    fn the_curve_does_not_depend_on_the_frame_time() {
        let mut a = Spring::at(10.0);
        let mut b = Spring::at(10.0);
        a.retarget(300.0);
        b.retarget(300.0);
        for _ in 0..12 {
            a.step(1.0 / 60.0);
        }
        for _ in 0..6 {
            b.step(1.0 / 30.0);
        }
        assert!((a.x - b.x).abs() < 1e-9 && (a.v - b.v).abs() < 1e-6);
    }

    #[test]
    fn a_new_target_keeps_position_and_velocity() {
        let mut s = Spring::at(0.0);
        s.retarget(500.0);
        run(&mut s, 0.15);
        let (x, v) = (s.x, s.v);
        assert!(x > 20.0 && v > 100.0);
        s.retarget(-200.0);
        // Nothing jumps at the instant of the change...
        assert_eq!((s.x, s.v), (x, v));
        // ...and the next frame stays on the old heading before turning.
        s.step(1.0 / 60.0);
        assert!((s.x - x).abs() < 15.0, "{} -> {}", x, s.x);
        run(&mut s, 2.0);
        assert!(s.settled() && s.x == -200.0);
    }

    #[test]
    fn a_stalled_frame_does_not_leap() {
        let mut s = Spring::at(0.0);
        s.retarget(1000.0);
        s.step(5.0);
        assert!(s.x < 600.0, "{}", s.x);
    }

    #[test]
    fn a_rect_moves_all_four_numbers_together() {
        let a = LRect::new(10.0, 20.0, 300.0, 200.0);
        let b = LRect::new(400.0, 120.0, 800.0, 600.0);
        let mut r = SpringRect::at(a);
        assert!(r.settled());
        r.retarget(b);
        assert!(!r.settled() && r.distance() > 100.0);
        let mut n = 0;
        while r.step(1.0 / 60.0) {
            n += 1;
            assert!(n < 200);
        }
        assert_eq!(r.rect(), b);
        r.snap(a);
        assert!(r.settled() && r.rect() == a);
    }

    #[test]
    fn an_ease_runs_its_duration_and_a_retarget_is_continuous() {
        let mut e = Ease::at(0.0, 0.3);
        assert!(e.settled());
        e.retarget(1.0);
        assert_eq!(e.value(), 0.0);
        for _ in 0..9 {
            e.step(1.0 / 60.0);
        }
        let (x, v) = (e.value(), e.velocity());
        assert!(x > 0.2 && x < 0.8 && v > 0.0);
        e.retarget(0.0);
        assert!((e.value() - x).abs() < 1e-12 && (e.velocity() - v).abs() < 1e-9);
        let mut n = 0;
        while e.step(1.0 / 60.0) {
            n += 1;
            assert!(n < 100);
        }
        assert_eq!(e.value(), 0.0);
        // The same target again is not a restart.
        let mut e = Ease::at(0.0, 0.3);
        e.retarget(1.0);
        e.step(0.1);
        let x = e.value();
        e.retarget(1.0);
        assert_eq!(e.value(), x);
    }
}
