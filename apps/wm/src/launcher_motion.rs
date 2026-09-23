//! The Android phone shell's motion, to the Pixel Launcher's numbers
//! (Launcher3 + Quickstep; the parity spec is
//! local/agent_state/wm-android-proc/launcher-parity-spec.md). Units are
//! the shell's points, which are Android's dp (Makepad scales by density);
//! a spec speed in px/ms or px/s is converted with the device density where
//! it is given in pixels.

use makepad_widgets::*;

/// A cubic Bézier easing through (0,0), (x1,y1), (x2,y2), (1,1), evaluated
/// at `t` along x (Android's `PathInterpolator`).
pub fn cubic(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    bezier_segment((0.0, 0.0), (x1, y1), (x2, y2), (1.0, 1.0), t)
}

/// A cubic segment from `p0` to `p3`, y at x = `x`.
fn bezier_segment(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), x: f64) -> f64 {
    let x = x.clamp(p0.0, p3.0);
    let at = |s: f64, a: f64, b: f64, c: f64, d: f64| {
        let u = 1.0 - s;
        u * u * u * a + 3.0 * u * u * s * b + 3.0 * u * s * s * c + s * s * s * d
    };
    // x is monotonic along these curves: bisect for the parameter.
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if at(mid, p0.0, p1.0, p2.0, p3.0) < x { lo = mid } else { hi = mid }
    }
    let s = (lo + hi) * 0.5;
    at(s, p0.1, p1.1, p2.1, p3.1)
}

/// A two-segment path easing (`M0,0 C a b c C d e 1,1`): `mid` is where
/// the first segment ends.
fn path2(c1: [(f64, f64); 3], c2: [(f64, f64); 2], t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let mid = c1[2];
    if t <= mid.0 {
        bezier_segment((0.0, 0.0), c1[0], c1[1], mid, t)
    } else {
        bezier_segment(mid, c2[0], c2[1], (1.0, 1.0), t)
    }
}

/// EMPHASIZED (fast_out_extra_slow_in).
pub fn emphasized(t: f64) -> f64 {
    path2([(0.05, 0.0), (0.133333, 0.06), (0.166666, 0.4)], [(0.208333, 0.82), (0.25, 1.0)], t)
}
/// EMPHASIZED_COMPLEMENT (`app_open_x`): the app-open path's x.
pub fn emphasized_complement(t: f64) -> f64 {
    path2([(0.1217, 0.0462), (0.15, 0.4686), (0.1667, 0.66)], [(0.1834, 0.8878), (0.1667, 1.0)], t)
}
pub fn emphasized_decelerate(t: f64) -> f64 { cubic(0.05, 0.7, 0.1, 1.0, t) }
pub fn touch_response(t: f64) -> f64 { cubic(0.3, 0.0, 0.1, 1.0, t) }
/// The home screen's scaling reveal after a swipe home.
pub fn scaling_reveal(t: f64) -> f64 {
    path2([(0.045, 0.0356), (0.0975, 0.2055), (0.15, 0.3952)], [(0.235, 0.6855), (0.235, 1.0)], t)
}
pub fn decelerate(t: f64) -> f64 { let u = 1.0 - t.clamp(0.0, 1.0); 1.0 - u * u }
/// DECELERATE with factor `n` (1 - (1-t)^(2n)).
pub fn decelerate_n(t: f64, n: f64) -> f64 { 1.0 - (1.0 - t.clamp(0.0, 1.0)).powf(2.0 * n) }
/// ACCELERATE_1_5 as the spec gives it.
pub fn accelerate_1_5(t: f64) -> f64 { t.clamp(0.0, 1.0).powi(3) }
pub fn scroll(t: f64) -> f64 { 1.0 + (t.clamp(0.0, 1.0) - 1.0).powi(5) }
pub fn scroll_cubic(t: f64) -> f64 { 1.0 + (t.clamp(0.0, 1.0) - 1.0).powi(3) }
/// OvershootInterpolator(1.2).
pub fn overshoot_1_2(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    let tension = 1.2;
    t * t * ((tension + 1.0) * t + tension) + 1.0
}
/// A linear ramp from 0 at `a` to 1 at `b`.
pub fn ramp(x: f64, a: f64, b: f64) -> f64 { ((x - a) / (b - a)).clamp(0.0, 1.0) }

/// A damped spring (androidx SpringForce: stiffness, damping ratio).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring {
    pub x: f64,
    pub v: f64,
    pub target: f64,
    pub stiffness: f64,
    pub damping: f64,
}
impl Spring {
    pub fn new(x: f64, v: f64, target: f64, stiffness: f64, damping: f64) -> Self {
        Self { x, v, target, stiffness, damping }
    }
    /// Advance by `dt` in small substeps (semi-implicit Euler).
    pub fn step(&mut self, dt: f64) {
        let steps = (dt / 0.002).ceil().max(1.0) as usize;
        let h = dt / steps as f64;
        let c = 2.0 * self.damping * self.stiffness.sqrt();
        for _ in 0..steps {
            let a = -self.stiffness * (self.x - self.target) - c * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
    }
    /// At rest within `eps` of the target (and barely moving).
    pub fn settled(&self, eps: f64) -> bool {
        (self.x - self.target).abs() < eps && self.v.abs() < eps * 20.0
    }
}

/// Swipe-up release velocity clamp (Quickstep `RectFSpringAnim`, v2):
/// between `min` and `max`, with any excess over `max` compressed as
/// excess^(1/1.4). In the units given.
pub fn clamp_release(v: f64, min: f64, max: f64) -> f64 {
    let a = v.abs();
    let clamped = if a < min { min } else if a <= max { a } else { max + (a - max).powf(1.0 / 1.4) };
    clamped * v.signum()
}

/// App open from an icon: 500 ms (`APP_LAUNCH_DURATION`).
pub const APP_OPEN_SECS: f64 = 0.5;
/// The icon over the growing window fades out from 25 ms to 75 ms.
pub const ICON_FADE: (f64, f64) = (0.025, 0.075);
/// The home screen behind a launch: 1 -> 0.97 over 350 ms.
pub const LAUNCH_HOME_SCALE: f64 = 0.97;
pub const LAUNCH_HOME_SECS: f64 = 0.35;

/// Where the opening window is `elapsed` seconds in, growing out of the
/// icon `icon` (a circle the icon's size) into `app`: size and y on
/// EMPHASIZED, x on EMPHASIZED_COMPLEMENT (the curved path). Returns the
/// rect, its corner radius (from a circle to square) and the window's
/// alpha over the fading icon.
pub fn open_window(icon: Rect, app: Rect, elapsed: f64) -> (Rect, f64, f64) {
    let t = (elapsed / APP_OPEN_SECS).clamp(0.0, 1.0);
    let side = icon.size.x.min(icon.size.y).max(1.0);
    let c0 = icon.pos + icon.size * 0.5;
    let c1 = app.pos + app.size * 0.5;
    let e = emphasized(t);
    let ex = emphasized_complement(t);
    let size = dvec2(side + (app.size.x - side) * e, side + (app.size.y - side) * e);
    let centre = dvec2(c0.x + (c1.x - c0.x) * ex, c0.y + (c1.y - c0.y) * e);
    let radius = side * 0.5 * (1.0 - e);
    let alpha = ramp(elapsed, ICON_FADE.0, ICON_FADE.1);
    (Rect { pos: centre - size * 0.5, size }, radius, alpha)
}

/// The home screen's scale while an app launches over it.
pub fn launch_home_scale(elapsed: f64) -> f64 {
    1.0 + (LAUNCH_HOME_SCALE - 1.0) * decelerate_n(elapsed / LAUNCH_HOME_SECS, 1.5)
}

/// A lifted app released toward Home flies into its icon on three springs
/// (Quickstep v2 `RectFSpringAnim`): centre x (450, 0.965), centre y (400,
/// 0.95) and the size progress (500, 0.99), started with the finger's
/// velocity (clamped as Quickstep does).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomeFlight {
    pub from: Rect,
    pub to: Rect,
    /// The icon's corner radius at the end (a circle for the fallback).
    pub to_radius: f64,
    pub from_radius: f64,
    pub cx: Spring,
    pub cy: Spring,
    pub progress: Spring,
    pub elapsed: f64,
}
impl HomeFlight {
    /// `velocity` in points/s, `density` pixels per point.
    pub fn new(from: Rect, from_radius: f64, to: Rect, to_radius: f64, velocity: Vec2d, density: f64) -> Self {
        let d = density.max(0.5);
        let vx = clamp_release(velocity.x * d, 300.0, 500.0) / d;
        let vy = clamp_release(velocity.y.min(-1.0) * d, 2000.0, 5000.0) / d;
        let c0 = from.pos + from.size * 0.5;
        let c1 = to.pos + to.size * 0.5;
        Self {
            from, to, to_radius, from_radius,
            cx: Spring::new(c0.x, vx, c1.x, 450.0, 0.965),
            cy: Spring::new(c0.y, vy, c1.y, 400.0, 0.95),
            progress: Spring::new(0.0, 0.0, 1.0, 500.0, 0.99),
            elapsed: 0.0,
        }
    }
    pub fn step(&mut self, dt: f64) -> bool {
        self.elapsed += dt;
        self.cx.step(dt);
        self.cy.step(dt);
        self.progress.step(dt);
        !(self.cx.settled(0.25) && self.cy.settled(0.25) && self.progress.settled(0.002))
    }
    pub fn rect(&self) -> Rect {
        let p = self.progress.x;
        let size = self.from.size + (self.to.size - self.from.size) * p;
        Rect { pos: dvec2(self.cx.x, self.cy.x) - size * 0.5, size }
    }
    pub fn radius(&self) -> f64 {
        let p = self.progress.x.clamp(0.0, 1.0);
        self.from_radius + (self.to_radius - self.from_radius) * p
    }
    /// The window fades out over the first 85 % of the size progress.
    pub fn window_alpha(&self) -> f64 {
        1.0 - accelerate_1_5(self.progress.x / 0.85)
    }
}

/// The home screen revealed after a swipe home: scale 0.85 -> 1 over 1 s on
/// the scaling-reveal curve, alpha 0 -> 1 over the first 200 ms.
pub const REVEAL_SECS: f64 = 1.0;
pub const REVEAL_FADE_SECS: f64 = 0.2;
pub const REVEAL_FROM: f64 = 0.85;
pub fn reveal(elapsed: f64) -> (f64, f64) {
    let s = REVEAL_FROM + (1.0 - REVEAL_FROM) * scaling_reveal(elapsed / REVEAL_SECS);
    (s, ramp(elapsed, 0.0, REVEAL_FADE_SECS))
}

/// All Apps on a phone: the panel rises 300 dp while it fades in.
pub const ALL_APPS_SHIFT: f64 = 300.0;
/// Release: a fling faster than 1 dp/ms goes its way; else open past 40 %
/// (from Home) or close under 60 % (from All Apps).
pub const ALL_APPS_FLING: f64 = 1000.0;
pub const ALL_APPS_OPEN_AT: f64 = 0.4;
pub const ALL_APPS_CLOSE_AT: f64 = 0.6;
/// How the home and the All Apps panel look at progress `p` (0 home, 1 All
/// Apps): (home scale, home alpha, panel background alpha, panel content
/// alpha, panel shift in points).
pub fn all_apps_frame(p: f64) -> (f64, f64, f64, f64, f64) {
    let q = p.clamp(0.0, 1.0);
    let home_scale = 1.0 + (LAUNCH_HOME_SCALE - 1.0) * ramp(q, 0.0, 0.4);
    let home_alpha = if q >= 0.4 { 0.0 } else { 1.0 };
    let background = ramp(q, 0.117, 0.4);
    let content = ramp(q, 0.4, 0.8);
    let shift = (1.0 - p.min(1.0)) * ALL_APPS_SHIFT;
    (home_scale, home_alpha, background, content, shift)
}
/// The settle after release (Launcher `BaseSwipeDetector.calculateDuration`):
/// seconds for `remaining` progress at `v` points/s (converted to px/ms with
/// `density`), and whether the fast (quintic) curve applies.
pub fn all_apps_settle(remaining: f64, v: f64, density: f64) -> (f64, bool) {
    let v_px_ms = (v * density / 1000.0).abs();
    let ms = (1200.0 / (0.5 * v_px_ms).max(2.0) * remaining.abs().max(0.2)).max(100.0);
    (ms / 1000.0, v_px_ms > 10.0)
}

/// A timed ease from `from` to `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    pub from: f64,
    pub to: f64,
    pub elapsed: f64,
    pub duration: f64,
    pub curve: Curve,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve { Scroll, ScrollCubic, Overshoot, Decelerate, TouchResponse, Linear }
impl Tween {
    pub fn new(from: f64, to: f64, duration: f64, curve: Curve) -> Self {
        Self { from, to, elapsed: 0.0, duration: duration.max(0.001), curve }
    }
    pub fn step(&mut self, dt: f64) -> (f64, bool) {
        self.elapsed += dt;
        let t = (self.elapsed / self.duration).clamp(0.0, 1.0);
        let e = match self.curve {
            Curve::Scroll => scroll(t),
            Curve::ScrollCubic => scroll_cubic(t),
            Curve::Overshoot => overshoot_1_2(t),
            Curve::Decelerate => decelerate(t),
            Curve::TouchResponse => touch_response(t),
            Curve::Linear => t,
        };
        (self.from + (self.to - self.from) * e, t < 1.0)
    }
}

/// The home screen behind the switcher recedes to the launcher's hint
/// scale (Quickstep HINT state, 0.92) as it fades.
pub const OVERVIEW_HOME_SCALE: f64 = 0.92;
/// The wallpaper behind the switcher dims by the launcher's overview scrim
/// (100/255).
pub const OVERVIEW_SCRIM: f64 = 100.0 / 255.0;

/// Recents: neighbour cards slide in over 300 ms when the lift pauses.
pub const RECENTS_ATTACH_SECS: f64 = 0.3;
/// Swipe-up settle into Recents / back into the app (Quickstep
/// `MAX_SWIPE_DURATION` 350 ms, 3.33 per unit of shift).
pub fn swipe_settle_secs(delta: f64) -> f64 {
    (delta.abs() * 0.350 * 3.33).min(0.350).max(0.05)
}
/// Tapping a Recents card opens it in 336 ms on TOUCH_RESPONSE.
pub const RECENTS_LAUNCH_SECS: f64 = 0.336;

/// Android: the quickstep slop from the gesture bar (8 dp x 1.414).
pub const QUICKSTEP_SLOP: f64 = 11.3;

/// Haptics the shell asks for (Launcher's call sites).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    /// A lift paused: the switcher (EFFECT_CLICK).
    Click,
    /// All Apps starts opening (VIRTUAL_KEY).
    VirtualKey,
    /// A Recents page crossed (low tick) or a card dismissed (tick).
    Tick,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_start_at_zero_end_at_one_and_rise() {
        for f in [emphasized, emphasized_complement, emphasized_decelerate, touch_response, scaling_reveal, decelerate, scroll, scroll_cubic] {
            assert!(f(0.0).abs() < 1e-6 && (f(1.0) - 1.0).abs() < 1e-6);
            let mut last = 0.0;
            for i in 1..=100 {
                let v = f(i as f64 / 100.0);
                assert!(v >= last - 1e-6, "monotonic");
                last = v;
            }
        }
        // EMPHASIZED is 0.4 a sixth of the way in (its path's joint).
        assert!((emphasized(0.166666) - 0.4).abs() < 1e-3);
        assert!(overshoot_1_2(0.8) > 1.0, "overshoots");
    }

    #[test]
    fn app_open_grows_from_the_icon_circle_to_the_app() {
        let icon = Rect { pos: dvec2(40.0, 600.0), size: dvec2(60.0, 60.0) };
        let app = Rect { pos: dvec2(0.0, 42.0), size: dvec2(412.0, 794.0) };
        let (r0, rad0, a0) = open_window(icon, app, 0.0);
        assert!((r0.size.x - 60.0).abs() < 1e-6 && (rad0 - 30.0).abs() < 1e-6 && a0 == 0.0);
        let (r1, rad1, a1) = open_window(icon, app, APP_OPEN_SECS);
        assert!((r1.size.x - app.size.x).abs() < 1e-6 && (r1.pos.y - app.pos.y).abs() < 1e-6 && rad1.abs() < 1e-9 && a1 == 1.0);
        let mut last = 0.0;
        for i in 0..=50 {
            let (r, _, _) = open_window(icon, app, APP_OPEN_SECS * i as f64 / 50.0);
            assert!(r.size.x >= last - 1e-6, "only grows");
            last = r.size.x;
        }
        assert!((open_window(icon, app, 0.05).2 - 0.5).abs() < 1e-9, "the icon is half faded at 50 ms");
    }

    #[test]
    fn a_home_flight_lands_on_the_icon_and_fades_first() {
        let from = Rect { pos: dvec2(60.0, 200.0), size: dvec2(290.0, 600.0) };
        let to = Rect { pos: dvec2(40.0, 600.0), size: dvec2(60.0, 60.0) };
        let mut f = HomeFlight::new(from, 28.0, to, 30.0, dvec2(0.0, -3000.0), 2.4);
        let mut faded_at = None;
        for i in 0..600 {
            if !f.step(1.0 / 120.0) { break; }
            if faded_at.is_none() && f.window_alpha() <= 0.0 { faded_at = Some(i); }
        }
        let r = f.rect();
        assert!((r.pos - to.pos).length() < 1.0 && (r.size - to.size).length() < 1.0);
        assert!(faded_at.is_some(), "the window is gone before the icon lands");
    }

    #[test]
    fn all_apps_frames_follow_the_spec() {
        let (s, a, bg, c, shift) = all_apps_frame(0.0);
        assert_eq!((s, a, bg, c, shift), (1.0, 1.0, 0.0, 0.0, ALL_APPS_SHIFT));
        let (s, a, _, c, _) = all_apps_frame(0.39);
        assert!(s < 1.0 && s > LAUNCH_HOME_SCALE && a == 1.0 && c == 0.0);
        let (_, a, bg, _, _) = all_apps_frame(0.4);
        assert!(a == 0.0 && bg == 1.0, "home gone and the panel ground full at 40 %");
        let (_, _, _, c, shift) = all_apps_frame(1.0);
        assert!(c == 1.0 && shift == 0.0);
        let (fast, quintic) = all_apps_settle(0.5, 6000.0, 2.4);
        assert!(fast >= 0.1 && quintic);
    }

    #[test]
    fn release_velocity_is_clamped_like_quickstep() {
        assert_eq!(clamp_release(100.0, 300.0, 500.0), 300.0);
        assert_eq!(clamp_release(-400.0, 300.0, 500.0), -400.0);
        assert!(clamp_release(-9000.0, 2000.0, 5000.0) > -5500.0);
    }
}
