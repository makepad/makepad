//! The kit's LFO: preset shapes, a drawn shape (points with curves), step
//! and smooth random, two chaos attractors (Lorenz, Rössler) and 2D paths
//! (circle, figure eight, star, rose) that give an x and a y output. Free
//! (on the clock), trig (from each note) or env (one cycle); in Hz or a
//! beat division; delay, rise, phase, smoothing, direction; per voice or
//! one shared (mono). Advanced a block at a time.

use super::env::bend;
use super::schema::{Builder, Unit::{Hz, Percent, Secs}, Values};
use crate::dsp::Rng;
use std::f32::consts::TAU;

pub const SHAPES: &[&str] = &["sine", "triangle", "saw_up", "saw_down", "square", "step", "smooth", "drawn", "lorenz", "rossler", "circle", "eight", "star", "rose"];
pub const MODES: &[&str] = &["free", "trig", "env"];
pub const DIRS: &[&str] = &["forward", "reverse", "pingpong"];
pub const SYNCS: &[&str] = &["off", "b8", "b4", "b2", "b1", "n2", "n2t", "n4", "n4d", "n4t", "n8", "n8d", "n8t", "n16", "n16t", "n32"];
/// Beats a cycle for each of [`SYNCS`] (b: bars of 4/4; n: a note value, t triplet, d dotted).
pub const SYNC_BEATS: [f32; 16] = [0.0, 32.0, 16.0, 8.0, 4.0, 2.0, 4.0 / 3.0, 1.0, 1.5, 2.0 / 3.0, 0.5, 0.75, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];
/// Points a drawn shape holds.
pub const MAX_POINTS: usize = 32;

const S_STEP: u8 = 5;
const S_SMOOTH: u8 = 6;
const S_DRAWN: u8 = 7;
const S_LORENZ: u8 = 8;
const S_ROSSLER: u8 = 9;
const S_CIRCLE: u8 = 10;

#[derive(Clone, Debug, Default)]
pub struct LfoIx {
    pub shape: usize,
    pub mode: usize,
    pub rate: usize,
    pub sync: usize,
    pub delay: usize,
    pub rise: usize,
    pub phase: usize,
    pub smooth: usize,
    pub var: usize,
    pub uni: usize,
    pub dir: usize,
    pub mono: usize,
}

/// Declare LFO `n` (1-based): keys `lfo1_rate` …
pub fn declare(b: &mut Builder, n: usize) -> LfoIx {
    let k = |s: &str| format!("lfo{n}_{s}");
    b.group(&format!("lfo{n}"), "an LFO: shapes, drawn (lfoN_points: [[x y curve] ...], x and y 0..1), chaos (lorenz, rossler: var is speed), 2D paths (circle eight star rose: x here, y as source lfoNy); free runs on the clock, trig restarts per note, env plays once");
    LfoIx {
        shape: b.choice(k("shape"), "shape", SHAPES, 0, ""),
        mode: b.choice(k("mode"), "mode", MODES, 1, ""),
        rate: b.log(k("rate"), "rate", 0.01, 100.0, 2.0, Hz, "cycles a second (sync off)"),
        sync: b.choice(k("sync"), "sync", SYNCS, 0, "cycle on the tempo: b8..b1 bars, n2..n32 note values (t triplet, d dotted)"),
        delay: b.log(k("delay"), "delay", 0.0001, 8.0, 0.0001, Secs, ""),
        rise: b.log(k("rise"), "rise", 0.0001, 8.0, 0.0001, Secs, "fade in after the delay"),
        phase: b.num(k("phase"), "phase", 0.0, 1.0, 0.0, Percent, ""),
        smooth: b.num(k("smooth"), "smooth", 0.0, 1.0, 0.0, Percent, "slews the output"),
        var: b.num(k("var"), "var", 0.0, 1.0, 0.5, Percent, "shape tweak: square width, triangle peak, chaos speed, rose petals"),
        uni: b.flag(k("uni"), "uni", false, "0..1 instead of -1..1"),
        dir: b.choice(k("dir"), "dir", DIRS, 0, ""),
        mono: b.flag(k("mono"), "mono", false, "one LFO shared by every voice (else one per voice)"),
    }
}

/// One LFO's settings for a block.
#[derive(Clone, Copy, Debug)]
pub struct LfoBlock {
    pub shape: u8,
    pub mode: u8,
    pub rate: f32,
    pub sync_beats: f32,
    pub delay: f32,
    pub rise: f32,
    pub phase: f32,
    pub smooth: f32,
    pub var: f32,
    pub uni: bool,
    pub dir: u8,
    pub mono: bool,
}

impl LfoBlock {
    pub fn read(v: &(impl Values + ?Sized), x: &LfoIx) -> LfoBlock {
        LfoBlock {
            shape: v.index(x.shape) as u8,
            mode: v.index(x.mode) as u8,
            rate: v.val(x.rate),
            sync_beats: SYNC_BEATS[v.index(x.sync).min(SYNC_BEATS.len() - 1)],
            delay: v.val(x.delay),
            rise: v.val(x.rise),
            phase: v.val(x.phase),
            smooth: v.val(x.smooth),
            var: v.val(x.var),
            uni: v.on(x.uni),
            dir: v.index(x.dir) as u8,
            mono: v.on(x.mono),
        }
    }

    /// Cycles a second at `beats_per_sec`.
    pub fn hz(&self, beats_per_sec: f32) -> f32 {
        if self.sync_beats > 0.0 { beats_per_sec / self.sync_beats } else { self.rate }
    }
}

/// A drawn shape's value at `x` (0..1): points (x, y, curve) sorted by x,
/// each segment bent by its end point's curve. Returns 0..1.
pub fn drawn(points: &[[f32; 3]], x: f32) -> f32 {
    match points.len() {
        0 => return 0.5,
        1 => return points[0][1],
        _ => {}
    }
    if x <= points[0][0] {
        return points[0][1];
    }
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if x <= b[0] {
            let span = (b[0] - a[0]).max(1e-6);
            let t = bend((x - a[0]) / span, b[2]);
            return a[1] + (b[1] - a[1]) * t;
        }
    }
    points[points.len() - 1][1]
}

/// A star's outline (5 points) at `p`, as (x, y) in -1..1.
fn star(p: f32) -> (f32, f32) {
    let corners = 10.0;
    let x = p * corners;
    let i = x as usize % 10;
    let t = x - x.floor();
    let pt = |k: usize| {
        let r = if k % 2 == 0 { 1.0 } else { 0.4 };
        let a = TAU * k as f32 / corners - TAU / 4.0;
        (r * a.cos(), -r * a.sin())
    };
    let (a, b) = (pt(i), pt((i + 1) % 10));
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    phase: f64,
    age: f32,
    done: bool,
    held: [f32; 2],
    prev: [f32; 2],
    chaos: [f32; 3],
    out: [f32; 2],
    rng: Rng,
    last_cycle: i64,
}

impl Lfo {
    pub fn new(seed: u32) -> Lfo {
        let mut rng = Rng::new(seed | 1);
        let held = [rng.bipolar(), rng.bipolar()];
        Lfo { phase: 0.0, age: 0.0, done: false, held, prev: held, chaos: [0.1, 0.0, 0.0], out: [0.0; 2], rng, last_cycle: 0 }
    }

    /// A note (trig and env restart; free keeps its clock phase).
    pub fn trigger(&mut self, l: &LfoBlock) {
        self.age = 0.0;
        self.done = false;
        if l.mode != 0 {
            self.phase = 0.0;
            self.last_cycle = 0;
        }
        self.chaos = [0.1 + self.rng.unit() * 0.1, self.rng.unit() * 0.1, self.rng.unit() * 0.1];
    }

    /// Outputs x, y (bipolar, or 0..1 when `uni`).
    pub fn out(&self) -> [f32; 2] {
        self.out
    }

    /// Move on `dt` seconds. `clock` is the shared position in cycles of a
    /// free-running LFO at this rate (seconds × hz, or beats ÷ sync), so free
    /// LFOs stay in step with the music.
    pub fn advance(&mut self, l: &LfoBlock, points: &[[f32; 3]], dt: f32, beats_per_sec: f32, clock: f64) {
        let hz = l.hz(beats_per_sec);
        self.age += dt;
        let phase = match l.mode {
            0 => clock,
            _ => {
                self.phase += (hz * dt) as f64;
                self.phase
            }
        };
        let mut p = phase + l.phase as f64;
        if l.mode == 2 && p >= 1.0 {
            self.done = true;
        }
        if self.done {
            p = 0.999_999;
        }
        let cycle = p.floor() as i64;
        let mut x = (p - p.floor()) as f32;
        match l.dir {
            1 => x = 1.0 - x,
            2 => x = 1.0 - (2.0 * x - 1.0).abs(),
            _ => {}
        }
        if cycle != self.last_cycle {
            self.last_cycle = cycle;
            self.prev = self.held;
            self.held = [self.rng.bipolar(), self.rng.bipolar()];
        }
        let (mut a, mut b) = match l.shape {
            0 => ((TAU * x).sin(), (TAU * (x + 0.25)).sin()),
            1 => {
                let peak = l.var.clamp(0.01, 0.99);
                let tri = |x: f32| if x < peak { -1.0 + 2.0 * x / peak } else { 1.0 - 2.0 * (x - peak) / (1.0 - peak) };
                (tri(x), tri((x + 0.25).fract()))
            }
            2 => (2.0 * x - 1.0, 2.0 * (x + 0.25).fract() - 1.0),
            3 => (1.0 - 2.0 * x, 1.0 - 2.0 * (x + 0.25).fract()),
            4 => {
                let w = l.var.clamp(0.02, 0.98);
                (if x < w { 1.0 } else { -1.0 }, if (x + 0.25).fract() < w { 1.0 } else { -1.0 })
            }
            S_STEP => (self.held[0], self.held[1]),
            S_SMOOTH => {
                let t = 0.5 - 0.5 * (std::f32::consts::PI * x).cos();
                (self.prev[0] + (self.held[0] - self.prev[0]) * t, self.prev[1] + (self.held[1] - self.prev[1]) * t)
            }
            S_DRAWN => (drawn(points, x) * 2.0 - 1.0, drawn(points, (x + 0.25).fract()) * 2.0 - 1.0),
            S_LORENZ | S_ROSSLER => {
                // Integrate in small steps; speed follows the rate and var.
                let speed = (hz * (0.25 + l.var * 2.0) * dt).min(0.5);
                let steps = ((speed / 0.005).ceil() as usize).clamp(1, 64);
                let h = speed / steps as f32 * if l.shape == S_LORENZ { 1.0 } else { 6.0 };
                let [mut cx, mut cy, mut cz] = self.chaos;
                for _ in 0..steps {
                    let (dx, dy, dz) = if l.shape == S_LORENZ {
                        (10.0 * (cy - cx), cx * (28.0 - cz) - cy, cx * cy - 8.0 / 3.0 * cz)
                    } else {
                        (-cy - cz, cx + 0.2 * cy, 0.2 + cz * (cx - 5.7))
                    };
                    cx += dx * h;
                    cy += dy * h;
                    cz += dz * h;
                }
                if !(cx.is_finite() && cy.is_finite() && cz.is_finite()) || cx.abs() > 1e3 {
                    cx = 0.1;
                    cy = 0.0;
                    cz = 0.0;
                }
                self.chaos = [cx, cy, cz];
                if l.shape == S_LORENZ {
                    ((cx / 20.0).clamp(-1.0, 1.0), ((cz - 25.0) / 25.0).clamp(-1.0, 1.0))
                } else {
                    ((cx / 11.0).clamp(-1.0, 1.0), (cy / 11.0).clamp(-1.0, 1.0))
                }
            }
            S_CIRCLE => ((TAU * x).cos(), (TAU * x).sin()),
            11 => ((TAU * x).sin(), (2.0 * TAU * x).sin()),
            12 => star(x),
            13 => {
                let petals = 2.0 + (l.var * 6.0).round();
                let r = (petals * TAU * x * 0.5).cos();
                (r * (TAU * x).cos(), r * (TAU * x).sin())
            }
            _ => (0.0, 0.0),
        };
        // Delay, then rise.
        let fade = if self.age < l.delay { 0.0 } else { ((self.age - l.delay) / l.rise.max(1e-4)).min(1.0) };
        a *= fade;
        b *= fade;
        if l.uni {
            a = a * 0.5 + 0.5;
            b = b * 0.5 + 0.5;
        }
        if l.smooth > 0.0 {
            let k = 1.0 - (-dt / (l.smooth * l.smooth * 0.5).max(1e-4)).exp();
            self.out[0] += (a - self.out[0]) * k;
            self.out[1] += (b - self.out[1]) * k;
        } else {
            self.out = [a, b];
        }
    }
}
