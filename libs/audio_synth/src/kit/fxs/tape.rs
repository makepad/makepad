//! Tape (type 23): a physically based tape model in three stages:
//! magnetic hysteresis (the published real-time tape-machine model with
//! the Jiles-Atherton equations, solved per oversampled sample with a
//! Runge-Kutta or Newton-Raphson solver), playback-head losses (spacing,
//! thickness and gap losses as an FIR plus a head bump), and degradation
//! (random dropouts with a moving low-pass, hiss and level wobble).
//! EFFECTS-2.md 6.5.

use super::common::*;
use super::neuron::{LinSmooth, Os4};
use super::FxCtx;
use crate::dsp::Rng;
use crate::kit::core::{calc_omega_hz, Biquad, SlotDef, SlotUnit, SlotValue};

/// Only the input-side tone shelf is applied; the reference never runs the
/// matching output tone stage.
const QUIRK_NO_OUTPUT_TONE: bool = true;
/// The hysteresis derivative and solver step use T = 1 / base rate
/// although they run at 4x.
const QUIRK_HYSTERESIS_BASE_RATE_T: bool = true;
const _: () = assert!(QUIRK_HYSTERESIS_BASE_RATE_T);
/// coth is computed as 1 / (Pade tanh) with |Q| clamped to 5.7, as in the
/// reference's vectorised build.
const QUIRK_COTH_PADE_CLAMPED: bool = true;
const _: () = assert!(QUIRK_COTH_PADE_CLAMPED);
/// The DC blocker uses omega = 35 / sr (about 5.6 Hz), not 2 pi 35 / sr.
const QUIRK_DC_OMEGA_35_OVER_SR: bool = true;
const _: () = assert!(QUIRK_DC_OMEGA_35_OVER_SR);
/// Each recompute of the loss FIR ADDS the new impulse response to the
/// FIR's old taps (they are never cleared after the first preparation).
const QUIRK_LOSS_FIR_ACCUMULATES: bool = true;
const _: () = assert!(QUIRK_LOSS_FIR_ACCUMULATES);

const SOLVERS: &[&str] = &["RK2", "RK4", "NR4", "NR8"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        deform(deact(percent("Drive", 0.85)), SOLVERS),
        percent("Saturation", 0.5),
        percent("Bias", 0.5),
        bipolar("Tone", 0.0),
        about(deact(fl("Speed", 1.0, 50.0, 30.0, SlotUnit::None)), "tape speed in inches per second (used up to 30)"),
        about(fl("Gap", 1.0, 50.0, 1.0, SlotUnit::None), "head gap in microns"),
        about(fl("Spacing", 0.1, 20.0, 0.1, SlotUnit::None), "head-to-tape spacing in microns"),
        about(fl("Thickness", 0.1, 50.0, 0.1, SlotUnit::None), "tape thickness in microns"),
        deact(percent("Degrade Depth", 0.0)),
        percent("Degrade Amount", 0.0),
        percent("Degrade Variance", 0.0),
        percent("Mix", 1.0),
    ]
}

// SPEC? the default deactivation of Degrade Depth is not stated: active.
pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[0].deform = 1;
}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

// ------------------------------------------------------------ smoothers --

/// Geometric (multiplicative) ramp over `n` steps; the first target snaps.
#[derive(Clone, Copy, Debug, Default)]
struct GeoSmooth {
    v: f64,
    target: f64,
    mul: f64,
    left: u32,
    n: u32,
    started: bool,
}

impl GeoSmooth {
    fn new(n: u32) -> GeoSmooth {
        GeoSmooth { n: n.max(1), ..Default::default() }
    }
    fn set(&mut self, t: f64) {
        if !self.started {
            self.v = t;
            self.target = t;
            self.started = true;
        } else if t != self.target {
            self.target = t;
            if self.v <= 0.0 || t <= 0.0 {
                self.v = t;
                self.left = 0;
            } else {
                self.mul = (t / self.v).powf(1.0 / self.n as f64);
                self.left = self.n;
            }
        }
    }
    #[inline]
    fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.v *= self.mul;
            self.left -= 1;
            if self.left == 0 {
                self.v = self.target;
            }
        }
        self.v
    }
    fn ramping(&self) -> bool {
        self.left > 0
    }
}

// ------------------------------------------------------ tone (shelf) ----

#[derive(Clone, Copy, Default)]
struct ToneShelf {
    gl: GeoSmooth,
    gh: GeoSmooth,
    b0: f64,
    b1: f64,
    a1: f64,
    z: [f64; 2],
    fresh: bool,
}

impl ToneShelf {
    fn new(sr: f32) -> ToneShelf {
        let n = (0.05 * sr) as u32;
        ToneShelf { gl: GeoSmooth::new(n), gh: GeoSmooth::new(n), fresh: true, ..Default::default() }
    }
    fn design(&mut self, gl: f64, gh: f64, sr: f64) {
        let k = 1.0 / (std::f64::consts::PI * 800.0 / sr).tan();
        let rho = (gh / gl).sqrt();
        let a0 = k / rho + 1.0;
        self.b0 = (gh / rho * k + gl) / a0;
        self.b1 = (-gh / rho * k + gl) / a0;
        self.a1 = (-k / rho + 1.0) / a0;
    }
    fn process(&mut self, tone: f32, sr: f32, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let t = tone.clamp(-1.0, 1.0);
        let (bass, treble) = if t > 0.0 {
            (-0.5 * t.abs(), t.abs())
        } else if t < 0.0 {
            (t.abs(), -0.5 * t.abs())
        } else {
            (0.0, 0.0)
        };
        self.gl.set(10f64.powf(18.0 * bass as f64 / 20.0));
        self.gh.set(10f64.powf(18.0 * treble as f64 / 20.0));
        let srd = sr as f64;
        for k in 0..B {
            let ramp = self.gl.ramping() || self.gh.ramping() || self.fresh;
            let (gl, gh) = (self.gl.next(), self.gh.next());
            if gl == gh && !self.gl.ramping() && !self.gh.ramping() {
                l[k] = (l[k] as f64 * gl) as f32;
                r[k] = (r[k] as f64 * gl) as f32;
                self.z = [0.0; 2];
                self.fresh = true;
                continue;
            }
            if ramp {
                self.design(gl, gh, srd);
                self.fresh = false;
            }
            for (c, s) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                let x = *s as f64;
                let y = self.b0 * x + self.z[c];
                self.z[c] = self.b1 * x - self.a1 * y;
                *s = y as f32;
            }
        }
    }
}

// --------------------------------------------------------- hysteresis --

const HYST_ALPHA: f64 = 1.6e-3;
const HYST_K: f64 = 0.47875;

#[inline]
fn tanh78(x: f64) -> f64 {
    let x2 = x * x;
    x * (2027025.0 + x2 * (270270.0 + x2 * (6930.0 + 36.0 * x2))) / (2027025.0 + x2 * (945945.0 + x2 * (51975.0 + x2 * (630.0 + x2))))
}

#[inline]
fn coth(q: f64) -> f64 {
    1.0 / tanh78(q.clamp(-5.7, 5.7))
}

/// The cooked Jiles-Atherton parameters.
#[derive(Clone, Copy, Debug)]
struct JaParams {
    ms: f64,
    a: f64,
    c: f64,
    nc: f64,
}

impl Default for JaParams {
    fn default() -> Self {
        JaParams { ms: 1.0, a: 0.25, c: 0.17, nc: 0.83 }
    }
}

impl JaParams {
    /// dM/dt and, when asked, its derivative by M.
    fn eval(&self, m: f64, h: f64, hd: f64, deriv: bool) -> (f64, f64) {
        let (ms, a, c, nc, k, al) = (self.ms, self.a, self.c, self.nc, HYST_K, HYST_ALPHA);
        let q = (h + al * m) / a;
        let near = q.abs() < 0.001;
        let ct = if near { 0.0 } else { coth(q) };
        let l = if near { q / 3.0 } else { ct - 1.0 / q };
        let lp = if near { 1.0 / 3.0 } else { 1.0 / (q * q) - ct * ct + 1.0 };
        let mdiff = ms * l - m;
        let delta = if hd >= 0.0 { 1.0 } else { -1.0 };
        let sm = if mdiff >= 0.0 { 1.0 } else { -1.0 };
        let kap = if delta == sm { nc } else { 0.0 };
        let den = nc * delta * k - al * mdiff;
        let f12 = kap * mdiff / den + lp * c * ms / a;
        let f3 = 1.0 - lp * al * c * ms / a;
        let f = hd * f12 / f3;
        if !deriv {
            return (f, 0.0);
        }
        let lpp = if near { -2.0 * q / 15.0 } else { 2.0 * ct * (ct * ct - 1.0) - 2.0 / (q * q * q) };
        let md2 = lp * al * ms / a - 1.0;
        let f1d = kap * (md2 / den + mdiff * al * md2 / (den * den));
        let f2d = lpp * al * c * ms / (a * a);
        let f3d = -lpp * al * al * c * ms / (a * a);
        let fd = (hd * (f1d + f2d) - f * f3d) / f3;
        (f, fd)
    }
}

/// One channel's hysteresis state.
#[derive(Clone, Copy, Debug, Default)]
struct Hyst {
    m1: f64,
    h1: f64,
    hd1: f64,
}

impl Hyst {
    fn step(&mut self, p: &JaParams, h: f64, t: f64, solver: i32) -> f64 {
        let hd = (1.0 + 0.75) / t * (h - self.h1) - 0.75 * self.hd1;
        let (hm, hdm) = (0.5 * (h + self.h1), 0.5 * (hd + self.hd1));
        let f = |m: f64, hh: f64, hdd: f64| p.eval(m, hh, hdd, false).0;
        let mut m = match solver {
            0 => {
                let k1 = t * f(self.m1, self.h1, self.hd1);
                let k2 = t * f(self.m1 + 0.5 * k1, hm, hdm);
                self.m1 + k2
            }
            2 | 3 => {
                let ta = t / 1.9;
                let fprev = f(self.m1, self.h1, self.hd1);
                let mut m = self.m1;
                for _ in 0..(if solver == 2 { 4 } else { 8 }) {
                    let (fv, fd) = p.eval(m, h, hd, true);
                    let g = m - self.m1 - ta * (fv + fprev);
                    let dg = 1.0 - ta * fd;
                    m -= g / dg;
                }
                m
            }
            _ => {
                let k1 = t * f(self.m1, self.h1, self.hd1);
                let k2 = t * f(self.m1 + 0.5 * k1, hm, hdm);
                let k3 = t * f(self.m1 + 0.5 * k2, hm, hdm);
                let k4 = t * f(self.m1 + k3, h, hd);
                self.m1 + k1 / 6.0 + k2 / 3.0 + k3 / 3.0 + k4 / 6.0
            }
        };
        let mut hd = hd;
        if m.is_nan() || m > 20.0 {
            m = 0.0;
            hd = 0.0;
        }
        self.m1 = m;
        self.h1 = h;
        self.hd1 = hd;
        m
    }
}

// ------------------------------------------------------------- loss -----

const FADE: usize = 1024;

struct Loss {
    n: usize,
    h: [Vec<f64>; 2],
    big_h: Vec<f64>,
    hist: [Vec<f32>; 2],
    pos: usize,
    active: usize,
    fade: usize,
    params: [f32; 4],
    prepared: bool,
    bump: Biquad,
}

impl Loss {
    fn new(sr: f32) -> Loss {
        let n = ((48.0 * sr / 44100.0) as usize).max(2);
        Loss {
            n,
            h: [vec![0.0; n], vec![0.0; n]],
            big_h: vec![0.0; n],
            hist: [vec![0.0; n], vec![0.0; n]],
            pos: 0,
            active: 0,
            fade: FADE,
            params: [0.0; 4],
            prepared: false,
            bump: Biquad::new(),
        }
    }

    /// Add (or set) the loss response for `params` = (speed, spacing, gap,
    /// thickness) into FIR `which`.
    fn compute(&mut self, which: usize, params: [f32; 4], sr: f32, clear: bool) {
        let n = self.n;
        let (v, s, g, th) = (params[0] as f64, params[1] as f64, params[2] as f64, params[3] as f64);
        let sr = sr as f64;
        for k in 0..n / 2 {
            let f = k as f64 * sr / n as f64;
            let kw = 2.0 * std::f64::consts::PI * f.max(20.0) / (v * 0.0254);
            let sp = (-kw * s * 1e-6).exp();
            let thk = (1.0 - (-kw * th * 1e-6).exp()) / (kw * th * 1e-6);
            let ga = kw * g * 1e-6 / 2.0;
            let gap = ga.sin() / ga;
            self.big_h[k] = sp * thk * gap;
            self.big_h[n - k - 1] = self.big_h[k];
        }
        let h = &mut self.h[which];
        if clear {
            h.iter_mut().for_each(|x| *x = 0.0);
        }
        for i in 0..n / 2 {
            let mut acc = 0.0;
            for k in 0..n {
                acc += self.big_h[k] * (2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64).cos();
            }
            h[n / 2 + i] += acc / n as f64;
            h[n / 2 - i] = h[n / 2 + i];
        }
    }

    fn process(&mut self, p: &[SlotValue; 12], sr: f32, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let params = [p[4].v.clamp(1.0, 30.0), p[6].v.clamp(0.1, 20.0), p[5].v.clamp(1.0, 50.0), p[7].v.clamp(0.1, 50.0)];
        if !self.prepared {
            self.compute(0, params, sr, true);
            self.compute(1, params, sr, true);
            self.params = params;
            self.prepared = true;
        } else if params != self.params && self.fade >= FADE {
            let next = 1 - self.active;
            self.compute(next, params, sr, false);
            self.params = params;
            self.active = next;
            self.fade = 0;
        }
        let n = self.n;
        for k in 0..B {
            for (c, s) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                self.hist[c][self.pos] = *s;
                let mut y = [0f64; 2];
                for (j, yj) in y.iter_mut().enumerate() {
                    let h = &self.h[j];
                    let mut acc = 0.0;
                    for t in 0..n {
                        acc += h[t] * self.hist[c][(self.pos + n - t) % n] as f64;
                    }
                    *yj = acc;
                }
                let out = if self.fade < FADE {
                    let a = self.fade as f64 / FADE as f64;
                    y[1 - self.active] * (1.0 - a) + y[self.active] * a
                } else {
                    y[self.active]
                };
                *s = out as f32;
            }
            self.pos = (self.pos + 1) % n;
            if self.fade < FADE {
                self.fade += 1;
            }
        }
        let (v, g) = (params[0], params[2]);
        let fb = v * 0.0254 / (g * 1e-6 * 500.0);
        let gain = 20.0 * (1.5 * (1000.0 - (fb - 100.0).abs()) / 100.0).max(1.0).log10();
        self.bump.peak_eq(calc_omega_hz(fb, sr), 0.5, gain as f64);
        self.bump.process(l, r);
    }
}

// ----------------------------------------------------------- degrade ----

/// First-order bilinear low-pass with a geometrically smoothed cutoff.
#[derive(Clone, Copy, Default)]
struct Lp1st {
    fc: GeoSmooth,
    x1: [f64; 2],
    y1: [f64; 2],
}

impl Lp1st {
    fn new(n: u32) -> Lp1st {
        Lp1st { fc: GeoSmooth::new(n), ..Default::default() }
    }
    #[inline]
    fn tick(&mut self, l: f32, r: f32, sr: f64) -> (f32, f32) {
        let fc = self.fc.next();
        let c = 1.0 / (std::f64::consts::PI * fc / sr).tan();
        let b = 1.0 / (c + 1.0);
        let a1 = (1.0 - c) / (c + 1.0);
        let mut out = [0f32; 2];
        for (ch, x) in [l, r].into_iter().enumerate() {
            let x = x as f64;
            let y = b * x + b * self.x1[ch] - a1 * self.y1[ch];
            self.x1[ch] = x;
            self.y1[ch] = y;
            out[ch] = y as f32;
        }
        (out[0], out[1])
    }
}

struct Degrade {
    rng: Rng,
    counter: f32,
    duration: f32,
    crinkled: bool,
    mix: LinSmooth,
    mix_t: f64,
    power: LinSmooth,
    chew_lp: Lp1st,
    noise: Line,
    lp: Lp1st,
    gain: Line,
}

impl Degrade {
    fn new(sr: f32) -> Degrade {
        Degrade {
            rng: Rng::new(0x7a9e),
            counter: 0.0,
            duration: 0.0,
            crinkled: false,
            mix: LinSmooth::new((0.01 * sr) as u32),
            mix_t: 0.0,
            power: LinSmooth::new((0.005 * sr) as u32),
            chew_lp: Lp1st::new((0.02 * sr) as u32),
            noise: Line::new(),
            lp: Lp1st::new(20),
            gain: Line::new(),
        }
    }

    fn u(&mut self, a: f32, b: f32) -> f32 {
        a + self.rng.unit() * (b - a)
    }

    fn dry_time(&mut self, freq: f32, var: f32, sr: f32) -> f32 {
        let ts = freq.powf(0.1);
        let vs = self.u(0.0, 2.0).powf(var);
        let lo = (1.0 - ts) * sr * vs;
        let hi = (2.0 - 1.99 * ts) * sr * vs;
        lo + self.rng.unit() * (hi - lo)
    }

    fn wet_time(&mut self, freq: f32, depth: f32, var: f32, sr: f32) -> f32 {
        let ts = freq.powf(0.1);
        let vs = self.u(0.0, 2.0).powf(var);
        let start = 0.2 + 0.8 * depth;
        let end = start - (0.001 + 0.01 * depth);
        let lo = (1.0 - ts) * sr * vs;
        let hi = ((1.0 - ts) + start - end * ts) * sr * vs;
        lo + self.rng.unit() * (hi - lo)
    }

    fn process(&mut self, p: &[SlotValue; 12], sr: f32, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let (d, a, v) = (clamp01(p[8].v), clamp01(p[9].v), clamp01(p[10].v));
        // chew
        let freq = 0.9 - 0.8 * a;
        let depth = 0.15 * d;
        let high = 22000f32.min(0.49 * sr);
        let span = high - 5000.0;
        if freq <= 0.0 {
            self.mix_t = 0.0;
            self.chew_lp.fc.set(high as f64);
        } else if freq >= 1.0 {
            self.mix_t = 1.0;
            self.power.set(3.0 * depth as f64);
            self.chew_lp.fc.set((high - span * depth) as f64);
        } else {
            self.counter += B as f32;
            if self.counter >= self.duration {
                self.counter = 0.0;
                self.crinkled = !self.crinkled;
                if self.crinkled {
                    self.mix_t = 1.0;
                    let pw = (1.0 + self.u(0.0, 2.0)) * depth;
                    self.power.set(pw as f64);
                    self.chew_lp.fc.set((high - span * depth) as f64);
                    self.duration = self.wet_time(freq, depth, v, sr);
                } else {
                    self.mix_t = 0.0;
                    self.chew_lp.fc.set(high as f64);
                    self.duration = self.dry_time(freq, v, sr);
                }
            } else {
                let pw = (1.0 + self.u(0.0, 2.0)) * depth;
                self.power.set(pw as f64);
                if self.crinkled {
                    self.chew_lp.fc.set((high - span * depth) as f64);
                }
            }
        }
        self.mix.set(self.mix_t);
        let srd = sr as f64;
        for k in 0..B {
            if !(self.mix_t == 0.0 && !self.mix.ramping()) {
                let m = self.mix.next() as f32;
                let pw = self.power.next() as f32;
                for s in [&mut l[k], &mut r[k]] {
                    let x = *s;
                    *s = m * x.signum() * x.abs().powf(1.0 + pw) + (1.0 - m) * x;
                }
            }
            let (a0, b0) = self.chew_lp.tick(l[k], r[k], srd);
            l[k] = a0;
            r[k] = b0;
        }
        // degrade
        self.noise.set_target(0.5 * d * a);
        let fc = 200.0 * 100f32.powf(1.0 - a);
        let lpf = (fc + v * (fc / 0.6) * self.u(-0.5, 0.5)).min(0.49 * sr);
        self.lp.fc.set(lpf.max(1.0) as f64);
        let g = 10f32.powf((v * 36.0 * self.u(-0.5, 0.5)).min(3.0) / 20.0);
        self.gain.set_target_smoothed(g);
        for k in 0..B {
            let nk = self.noise.at(k);
            let nl = self.u(-0.5, 0.5) * nk;
            let nr = self.u(-0.5, 0.5) * nk;
            let (a0, b0) = self.lp.tick(l[k] + nl, r[k] + nr, srd);
            let gk = self.gain.at(k);
            l[k] = a0 * gk;
            r[k] = b0 * gk;
        }
    }
}

// ------------------------------------------------------------ effect ----

pub struct Tape {
    sr: f32,
    tone: ToneShelf,
    os: Os4,
    d: LinSmooth,
    s: LinSmooth,
    w: LinSmooth,
    makeup: GeoSmooth,
    ja: JaParams,
    hyst: [Hyst; 2],
    dc: Biquad,
    loss: Loss,
    degrade: Degrade,
    mix: Line,
}

impl Tape {
    pub fn new(sr: f32) -> Tape {
        Tape {
            sr,
            tone: ToneShelf::new(sr),
            os: Os4::new(),
            // SPEC? the hysteresis control smoothers are taken to start at 0
            // so the first block cooks the parameters.
            d: LinSmooth::from(500, 0.0),
            s: LinSmooth::from(500, 0.0),
            w: LinSmooth::from(500, 0.0),
            makeup: GeoSmooth::new(500),
            ja: JaParams::default(),
            hyst: [Hyst::default(); 2],
            dc: Biquad::new(),
            loss: Loss::new(sr),
            degrade: Degrade::new(sr),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Tape::new(self.sr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    fn hysteresis(&mut self, p: &[SlotValue; 12], l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        self.tone.process(p[3].v, sr, l, r);
        let mut ul = [0f32; 128];
        let mut ur = [0f32; 128];
        self.os.up(l, r, &mut ul, &mut ur);
        let (d, s, w) = (clamp01(p[0].v) as f64, clamp01(p[1].v) as f64, 1.0 - clamp01(p[2].v) as f64);
        self.d.set(d);
        self.s.set(s);
        self.w.set(w);
        self.makeup.set((1.0 + 0.6 * w) / (0.5 + 1.5 * (1.0 - s)));
        let t = 1.0 / sr as f64;
        let solver = p[0].deform.clamp(0, 3);
        for k in 0..128 {
            let cook = self.d.ramping() || self.s.ramping() || self.w.ramping();
            let (dv, sv, wv) = (self.d.next(), self.s.next(), self.w.next());
            if cook {
                let ms = 0.5 + 1.5 * (1.0 - sv);
                let c = (1.0 - wv).max(0.0).sqrt() - 0.01;
                self.ja = JaParams { ms, a: ms / (0.01 + 6.0 * dv), c, nc: 1.0 - c };
            }
            let mk = self.makeup.next();
            ul[k] = (self.hyst[0].step(&self.ja, ul[k] as f64, t, solver) * mk) as f32;
            ur[k] = (self.hyst[1].step(&self.ja, ur[k] as f64, t, solver) * mk) as f32;
        }
        self.os.down(&mut ul, &mut ur, l, r);
        let omega = 35.0 / sr as f64;
        self.dc.hp(omega, 0.707);
        self.dc.process(l, r);
        // No output-side tone stage follows (the reference skips it).
        let _ = QUIRK_NO_OUTPUT_TONE;
        let g = 10f32.powf(9.0 / 20.0);
        for k in 0..B {
            l[k] *= g;
            r[k] *= g;
        }
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let mut wl = *l;
        let mut wr = *r;
        if !p[0].deactivated {
            self.hysteresis(p, &mut wl, &mut wr);
        }
        if !p[4].deactivated {
            self.loss.process(p, self.sr, &mut wl, &mut wr);
        }
        if !p[8].deactivated {
            self.degrade.process(p, self.sr, &mut wl, &mut wr);
        }
        self.mix.set_target_smoothed(clamp01(p[11].v));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;

    fn bin(x: &[f64], cycles: f64) -> f64 {
        let (mut re, mut im) = (0f64, 0f64);
        for (n, v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * cycles * n as f64 / x.len() as f64;
            re += v * a.cos();
            im += v * a.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f64
    }

    /// The hysteresis of one solver on a slow sine (integer cycles).
    fn loop_of(solver: i32) -> Vec<f64> {
        let ms = 0.5 + 1.5 * 0.5;
        let c = 0.5f64.sqrt() - 0.01;
        let ja = JaParams { ms, a: ms / (0.01 + 6.0 * 0.85), c, nc: 1.0 - c };
        let mut h = Hyst::default();
        let t = 1.0 / 48000.0;
        let n = 192000;
        let mut out = Vec::new();
        for i in 0..2 * n {
            let x = 0.5 * (std::f64::consts::TAU * 10.0 * i as f64 / n as f64).sin();
            let m = h.step(&ja, x, t, solver);
            if i >= n {
                out.push(m);
            }
        }
        out
    }

    #[test]
    fn solvers_agree_and_make_odd_harmonics() {
        let refm = loop_of(1);
        assert!(refm.iter().all(|m| m.is_finite()));
        let e: f64 = refm.iter().map(|m| m * m).sum();
        for s in [0, 2, 3] {
            let o = loop_of(s);
            let d: f64 = o.iter().zip(refm.iter()).map(|(a, b)| (a - b) * (a - b)).sum();
            assert!(d < 0.05 * e, "solver {s}: {d} vs {e}");
        }
        let h1 = bin(&refm, 10.0);
        let h2 = bin(&refm, 20.0);
        let h3 = bin(&refm, 30.0);
        assert!(h1 > 1e-3, "{h1}");
        assert!(h3 > 1e-4 * h1, "{h3} {h1}");
        assert!(h3 > 10.0 * h2, "{h3} {h2}");
    }

    #[test]
    fn loss_fir_is_a_lowpass() {
        let mut loss = Loss::new(48000.0);
        loss.compute(0, [7.5, 5.0, 10.0, 10.0], 48000.0, true);
        let resp = |hz: f64| {
            let w = std::f64::consts::TAU * hz / 48000.0;
            let (mut re, mut im) = (0.0, 0.0);
            for (n, h) in loss.h[0].iter().enumerate() {
                re += h * (w * n as f64).cos();
                im -= h * (w * n as f64).sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(resp(15000.0) < 0.5 * resp(500.0), "{} {}", resp(15000.0), resp(500.0));
    }

    #[test]
    fn loss_fir_accumulates_on_change() {
        let mut loss = Loss::new(48000.0);
        loss.compute(0, [30.0, 0.1, 1.0, 0.1], 48000.0, true);
        let once: Vec<f64> = loss.h[0].clone();
        loss.compute(0, [30.0, 0.1, 1.0, 0.1], 48000.0, false);
        let mid = loss.n / 2;
        assert!((loss.h[0][mid] - 2.0 * once[mid]).abs() < 1e-12);
    }

    #[test]
    fn finite_and_bounded_at_defaults() {
        let ctx = FxCtx::new(48000.0);
        for solver in 0..4 {
            let mut p = default_values(23);
            p[0].deform = solver;
            p[9].v = 0.5;
            p[10].v = 0.5;
            p[8].v = 0.5;
            p[3].v = 0.5;
            let mut fx = Tape::new(48000.0);
            let mut rng = Rng::new(5);
            let mut peak = 0f32;
            for b in 0..400 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    let n = b * 32 + k;
                    l[k] = if b < 200 { 0.5 * rng.bipolar() } else { 0.5 * (n as f32 * 0.0576).sin() };
                    r[k] = l[k];
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                for x in l.iter().chain(r.iter()) {
                    assert!(x.is_finite());
                    peak = peak.max(x.abs());
                }
            }
            assert!(peak > 1e-3 && peak < 20.0, "solver {solver}: {peak}");
        }
    }
}
