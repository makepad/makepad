//! The world's wind: one breeze field that every swaying thing reads (trees,
//! grass, cloth, flowers), so a gust visibly crosses a meadow and then
//! reaches the trees behind it.
//!
//! A [`WorldWind`] is what a game asked for: the direction and mean speed of
//! the wind and how gusty it is. A [`WindClock`] carries it through the
//! host's presentation time: it eases toward a changed wind (a script that
//! turns the wind does not snap every plant to a new lean), integrates how
//! far the gust pattern has blown, and runs the swaying plants' oscillator
//! clock. [`wind_at`] is the field itself and the CPU twin of the render
//! shaders' `SceneWind` mixin (makepad-render `shaders/mixins.rs`): two
//! layers of value noise, [`WIND_LAYERS`] metres across, carried downwind at
//! a fraction of the wind speed. Their sum scales the speed (gusts and lulls)
//! and the smaller one turns it a little.
//!
//! Everything is continuous in position and time. The noise repeats every
//! [`WIND_CELLS`] lattice cells on both axes and the scroll offsets wrap at
//! that period; the oscillator clock wraps at [`WIND_CLOCK_PERIOD`] seconds,
//! over which every oscillator the shaders run completes whole cycles.

/// The noise lattice's period in cells, on both axes.
pub const WIND_CELLS: f32 = 256.0;
/// The oscillator clock's period in seconds. A shader oscillator runs a
/// whole number of cycles per period, so the wrap never shows.
pub const WIND_CLOCK_PERIOD: f64 = 256.0;
/// The two noise layers: cell size in metres, and the speed they are carried
/// downwind at as a fraction of the wind's.
pub const WIND_LAYERS: [(f32, f32); 2] = [(40.0, 0.6), (9.0, 1.0)];
/// A changed wind is reached to 63 % after this many seconds.
pub const WIND_EASE: f64 = 1.5;

/// The wind a game asked for (`game.wind`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldWind {
    /// Where the wind blows TOWARD on the ground plane (x, z), unit length.
    pub dir: [f32; 2],
    /// Mean speed in m/s; 0 is calm.
    pub speed: f32,
    /// 0 = steady; 1 = the local speed swings between calm and twice the mean.
    pub gust: f32,
}

impl Default for WorldWind {
    /// A light, gusty breeze.
    fn default() -> Self {
        Self { dir: [0.8, 0.6], speed: 4.0, gust: 0.5 }
    }
}

impl WorldWind {
    /// A wind toward `dir` (its length is ignored; a zero or non-finite one
    /// keeps the default direction), `speed` m/s (at least 0) and `gust`
    /// (0..1).
    pub fn new(dir: [f32; 2], speed: f32, gust: f32) -> Self {
        let l = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        let dir = if l.is_finite() && l > 1.0e-6 { [dir[0] / l, dir[1] / l] } else { Self::default().dir };
        let speed = if speed.is_finite() { speed.max(0.0) } else { 0.0 };
        let gust = if gust.is_finite() { gust.clamp(0.0, 1.0) } else { 0.0 };
        Self { dir, speed, gust }
    }

    /// The mean wind's velocity (x, z) in m/s.
    pub fn velocity(&self) -> [f32; 2] {
        [self.dir[0] * self.speed, self.dir[1] * self.speed]
    }
}

/// The wind as the renderer shows it at one presentation time. Default:
/// calm, the state of a host that never runs the clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindClock {
    /// The eased mean velocity (x, z) in m/s and gust.
    velocity: [f64; 2],
    gust: f64,
    /// Per layer, how far its pattern has blown, in cells, wrapped to
    /// [0, WIND_CELLS).
    scroll: [[f64; 2]; 2],
    /// Seconds, wrapped to [0, WIND_CLOCK_PERIOD).
    clock: f64,
    /// The presentation time of the last advance.
    last: Option<f64>,
}

impl WindClock {
    /// Carry the wind to presentation time `time` in seconds: the host's own
    /// clock, so a paused game passes the same time and its wind stands
    /// still. The first call, and a time earlier than the last one (a
    /// reloaded world), take `wind` as it is; later calls ease toward it.
    pub fn advance(&mut self, wind: &WorldWind, time: f64) {
        if !time.is_finite() {
            return;
        }
        let target = wind.velocity();
        let target = [target[0] as f64, target[1] as f64];
        let dt = match self.last {
            Some(last) if time >= last => time - last,
            _ => {
                self.velocity = target;
                self.gust = wind.gust as f64;
                self.last = Some(time);
                return;
            }
        };
        self.last = Some(time);
        let before = self.velocity;
        let k = 1.0 - (-dt / WIND_EASE).exp();
        for axis in 0..2 {
            self.velocity[axis] += (target[axis] - self.velocity[axis]) * k;
        }
        self.gust += (wind.gust as f64 - self.gust) * k;
        // The pattern rides the mean of this step's start and end velocity.
        let moved = [(before[0] + self.velocity[0]) * 0.5 * dt, (before[1] + self.velocity[1]) * 0.5 * dt];
        for (scroll, (cell, rate)) in self.scroll.iter_mut().zip(WIND_LAYERS) {
            for axis in 0..2 {
                scroll[axis] = (scroll[axis] + moved[axis] * rate as f64 / cell as f64).rem_euclid(WIND_CELLS as f64);
            }
        }
        self.clock = (self.clock + dt).rem_euclid(WIND_CLOCK_PERIOD);
    }

    /// Take the next advance's wind as it is, without easing: a host whose
    /// world was replaced (a re-evaluated level) shows that world's wind.
    pub fn restart(&mut self) {
        self.last = None;
    }

    /// `wind` blowing steadily for `time` seconds from the start of its
    /// pattern: the same state whenever it is asked, for a picture that must
    /// not depend on when it is drawn (a review still, a test). A clock that
    /// starts there and is carried through `time` in any steps agrees.
    pub fn at(wind: &WorldWind, time: f64) -> Self {
        let mut clock = Self::default();
        clock.advance(wind, 0.0);
        clock.advance(wind, time.max(0.0));
        clock
    }

    /// The oscillator clock: seconds, wrapped to [0, WIND_CLOCK_PERIOD).
    pub fn time(&self) -> f64 {
        self.clock
    }

    /// The shaders' wind state: mean velocity x, z in m/s, gust, and the
    /// oscillator clock as a fraction of its period.
    pub fn state(&self) -> [f32; 4] {
        [self.velocity[0] as f32, self.velocity[1] as f32, self.gust as f32, (self.clock / WIND_CLOCK_PERIOD) as f32]
    }

    /// The shaders' scroll: each layer's pattern offset (x, z) in cells.
    pub fn scroll(&self) -> [f32; 4] {
        [self.scroll[0][0] as f32, self.scroll[0][1] as f32, self.scroll[1][0] as f32, self.scroll[1][1] as f32]
    }

    /// The wind at ground position `p` (x, z): see [`wind_at`].
    pub fn wind_at(&self, p: [f32; 2]) -> [f32; 3] {
        wind_at(self.state(), self.scroll(), p)
    }
}

fn fract(v: f32) -> f32 {
    v - v.floor()
}

/// The shaders' lattice hash: a hash without sine (identical in f32 on
/// every backend), in [0, 1).
fn wind_hash(x: f32, y: f32) -> f32 {
    let p = [fract(x * 0.1031), fract(y * 0.1031), fract(x * 0.1031)];
    let d = p[0] * (p[1] + 33.33) + p[1] * (p[2] + 33.33) + p[2] * (p[0] + 33.33);
    fract((p[0] + d + p[1] + d) * (p[2] + d))
}

/// Value noise in [0, 1], smoothstepped between lattice corners, repeating
/// every WIND_CELLS cells on both axes.
fn wind_noise(x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let x0 = ix.rem_euclid(WIND_CELLS);
    let y0 = iy.rem_euclid(WIND_CELLS);
    let x1 = (x0 + 1.0).rem_euclid(WIND_CELLS);
    let y1 = (y0 + 1.0).rem_euclid(WIND_CELLS);
    let a = wind_hash(x0, y0);
    let b = wind_hash(x1, y0);
    let c = wind_hash(x0, y1);
    let d = wind_hash(x1, y1);
    let lo = a + (b - a) * ux;
    let hi = c + (d - c) * ux;
    lo + (hi - lo) * uy
}

/// The wind at ground position `p` (x, z) for a [`WindClock::state`] and
/// [`WindClock::scroll`]: its velocity (x, z) in m/s and the gust level
/// there (0..1, 0.5 on average).
pub fn wind_at(state: [f32; 4], scroll: [f32; 4], p: [f32; 2]) -> [f32; 3] {
    let [(c1, _), (c2, _)] = WIND_LAYERS;
    let n1 = wind_noise(p[0] / c1 - scroll[0], p[1] / c1 - scroll[1]);
    let n2 = wind_noise(p[0] / c2 - scroll[2], p[1] / c2 - scroll[3]);
    let g = 0.65 * n1 + 0.35 * n2;
    let m = 1.0 - state[2] + state[2] * 2.0 * g;
    let turn = (n2 - 0.5) * 0.5;
    let (s, c) = turn.sin_cos();
    [(state[0] * c - state[1] * s) * m, (state[0] * s + state[1] * c) * m, g]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A still of a steady wind ([`WindClock::at`]) is the clock carried
    /// there frame by frame from the pattern's start: a review or a test that
    /// asks for a wind time sees what a running host shows at that time.
    #[test]
    fn a_steady_wind_at_a_time_is_the_clock_carried_there() {
        let wind = WorldWind::new([0.6, -0.8], 9.0, 0.7);
        let mut carried = WindClock::default();
        carried.advance(&wind, 0.0);
        for frame in 1..=(10 * 120) {
            carried.advance(&wind, frame as f64 / 120.0);
        }
        let still = WindClock::at(&wind, 10.0);
        let worst = points().iter().map(|&p| {
            let (a, b) = (carried.wind_at(p), still.wind_at(p));
            (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
        }).fold(0.0, f32::max);
        assert!((carried.time() - still.time()).abs() < 1.0e-6, "clock {} carried, {} still", carried.time(), still.time());
        assert!(worst < 1.0e-3, "the still differs from the carried wind by {worst}");
    }

    /// A grid of ground points across both layers' cells.
    fn points() -> Vec<[f32; 2]> {
        let mut out = Vec::new();
        for i in 0..6 {
            for j in 0..6 {
                out.push([-310.0 + i as f32 * 113.7, -170.0 + j as f32 * 71.3]);
            }
        }
        out
    }

    fn max_step(a: &WindClock, b: &WindClock, points: &[[f32; 2]]) -> f32 {
        points.iter().map(|&p| {
            let (u, v) = (a.wind_at(p), b.wind_at(p));
            ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2)).sqrt()
        }).fold(0.0, f32::max)
    }

    /// The field is continuous in time through both layers' scroll wraps,
    /// through a sudden change of the asked wind (a 180 degree turn) and into
    /// calm, and still once calm: every 1/120 s frame step moves the wind
    /// anywhere by at most MAX_STEP. A layer whose noise did not repeat at
    /// the scroll's wrap, or a change taken without easing, jumps by metres
    /// per second there.
    #[test]
    fn the_wind_field_is_continuous_through_wraps_and_changes_and_still_when_calm() {
        // At 12 m/s and gust 0.8 a step moves the second layer 0.011 cells:
        // at most ~0.3 m/s anywhere.
        const MAX_STEP: f32 = 0.5;
        let points = points();
        let strong = WorldWind::new([0.6, -0.8], 12.0, 0.8);
        let mut clock = WindClock::default();
        clock.advance(&strong, 0.0);
        // Both patterns a hair before their wrap on both axes (the wind
        // blows +x, -z: x wraps up through WIND_CELLS, z down through 0).
        let edge = WIND_CELLS as f64 - 0.002;
        clock.scroll = [[edge, 0.0005], [edge, 0.002]];
        let mut wraps = [0; 2];
        let mut worst = 0.0f32;
        // 2 s of the wind, then 12 s turning to the opposite way, then 20 s
        // easing into calm.
        for frame in 1..=(34 * 120) {
            let before = clock;
            let wind = match frame {
                f if f <= 2 * 120 => strong,
                f if f <= 14 * 120 => WorldWind::new([-0.6, 0.8], 12.0, 0.8),
                _ => WorldWind::new([1.0, 0.0], 0.0, 0.8),
            };
            clock.advance(&wind, frame as f64 / 120.0);
            for layer in 0..2 {
                for axis in 0..2 {
                    if (clock.scroll[layer][axis] - before.scroll[layer][axis]).abs() > WIND_CELLS as f64 * 0.5 {
                        wraps[layer] += 1;
                    }
                }
            }
            worst = worst.max(max_step(&before, &clock, &points));
        }
        println!("[wind] wraps {wraps:?}, largest 1/120 s step {worst:.3} m/s");
        assert!(wraps[0] >= 2 && wraps[1] >= 2, "both layers' scrolls wrapped on both axes: {wraps:?}");
        assert!(worst <= MAX_STEP, "the wind moved {worst} m/s in one 1/120 s step (at most {MAX_STEP})");
        // 20 s into calm: the eased speed is gone and nothing blows.
        let calm = points.iter().map(|&p| clock.wind_at(p)).map(|w| (w[0] * w[0] + w[1] * w[1]).sqrt()).fold(0.0, f32::max);
        assert!(calm < 0.01, "a calm wind blows {calm} m/s");
        // Continuous in position: 1 cm apart, the wind differs by little.
        let mut gusty = WindClock::default();
        gusty.advance(&strong, 0.0);
        gusty.advance(&strong, 37.3);
        let near = points.iter().map(|&p| {
            let (u, v) = (gusty.wind_at(p), gusty.wind_at([p[0] + 0.01, p[1] + 0.01]));
            ((u[0] - v[0]).powi(2) + (u[1] - v[1]).powi(2)).sqrt()
        }).fold(0.0, f32::max);
        assert!(near < 0.05, "1 cm apart the wind differs by {near} m/s");
        // The gusts swing the speed: over the points it spans calm-ish to strong.
        let speeds: Vec<f32> = points.iter().map(|&p| gusty.wind_at(p)).map(|w| (w[0] * w[0] + w[1] * w[1]).sqrt()).collect();
        let (lo, hi) = speeds.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        assert!(lo < 12.0 * 0.8 && hi > 12.0 * 1.2, "a gusty 12 m/s wind spans {lo}..{hi} m/s");
    }
}
