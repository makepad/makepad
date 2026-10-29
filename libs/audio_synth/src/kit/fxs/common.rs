//! Shared primitives of the effects (EFFECTS.md section 4 and the helpers
//! several effects repeat): block-linear lines, tri-pan and width, the
//! windowed-sinc fractional delay lines, the clippers, the legacy
//! waveshaper table lookup, the effects' modulation LFO, the simple LFO,
//! quadrature phasors, the trapezoidal SVF, tone cuts and the slot-def
//! builders. Nothing here allocates after construction.

use crate::dsp::Rng;
use crate::kit::core::{db_to_linear, SlotDef, SlotUnit, SlotValue, BLOCK};
use std::f64::consts::PI;
use std::sync::OnceLock;

/// The effects' block (host-rate samples).
pub const B: usize = BLOCK;
pub type Buf = [f32; B];

// ---------------------------------------------------------------- lines ---

/// A per-block value with a linear ramp across the block (section 4.1):
/// sample k sees `cur + (target - cur) (k + 1) / B`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Line {
    pub cur: f32,
    pub target: f32,
    started: bool,
}

impl Line {
    pub fn new() -> Line {
        Line::default()
    }
    pub fn set_target(&mut self, x: f32) {
        if !self.started {
            self.cur = x;
            self.target = x;
            self.started = true;
        } else {
            self.cur = self.target;
            self.target = x;
        }
    }
    /// The new target is `0.25 x + 0.75 previous target`, then ramped.
    pub fn set_target_smoothed(&mut self, x: f32) {
        if !self.started {
            self.set_target(x);
        } else {
            self.cur = self.target;
            self.target = 0.25 * x + 0.75 * self.target;
        }
    }
    pub fn instantize(&mut self) {
        self.cur = self.target;
    }
    /// Snap both ends (and mark started).
    pub fn snap(&mut self, x: f32) {
        self.cur = x;
        self.target = x;
        self.started = true;
    }
    pub fn started(&self) -> bool {
        self.started
    }
    #[inline]
    pub fn at(&self, k: usize) -> f32 {
        self.cur + (self.target - self.cur) * (k + 1) as f32 * (1.0 / B as f32)
    }
    pub fn multiply_block(&self, x: &mut Buf) {
        for (k, s) in x.iter_mut().enumerate() {
            *s *= self.at(k);
        }
    }
    pub fn multiply_2_blocks(&self, l: &mut Buf, r: &mut Buf) {
        for k in 0..B {
            let g = self.at(k);
            l[k] *= g;
            r[k] *= g;
        }
    }
    /// `dst += line * src`.
    pub fn mac_block(&self, src: &Buf, dst: &mut Buf) {
        for k in 0..B {
            dst[k] += self.at(k) * src[k];
        }
    }
    /// `dry = dry (1 - line) + wet line` (wet/dry mix, in place on dry).
    pub fn fade_2_blocks(&self, dry_l: &mut Buf, dry_r: &mut Buf, wet_l: &Buf, wet_r: &Buf) {
        for k in 0..B {
            let m = self.at(k);
            dry_l[k] = dry_l[k] * (1.0 - m) + wet_l[k] * m;
            dry_r[k] = dry_r[k] * (1.0 - m) + wet_r[k] * m;
        }
    }
}

/// Tri-pan of a stereo pair by a bipolar line value p (section 4.1).
pub fn tri_pan(l: &mut Buf, r: &mut Buf, p: &Line) {
    for k in 0..B {
        let v = p.at(k);
        let a = v.max(0.0);
        let b = v.min(0.0);
        let (x, y) = (l[k], r[k]);
        l[k] = (1.0 - a) * x - b * y;
        r[k] = a * x + (1.0 + b) * y;
    }
}

/// Mid/side width (section 4.3): S *= line (a linear side gain).
pub fn apply_width(l: &mut Buf, r: &mut Buf, side_gain: &Line) {
    for k in 0..B {
        let m = 0.5 * (l[k] + r[k]);
        let s = 0.5 * (l[k] - r[k]) * side_gain.at(k);
        l[k] = m + s;
        r[k] = m - s;
    }
}

/// Set a width line from a width-in-dB slot (smoothed target).
pub fn width_db_target(line: &mut Line, db: f32) {
    line.set_target_smoothed(db_to_linear(db));
}

// ------------------------------------------------------- tempo / rates ---

/// Per-block increment of a phase taking 2^x s per cycle: B / (sr 2^x).
#[inline]
pub fn rate_linear(x: f32, sr: f32) -> f32 {
    B as f32 / (sr * x.exp2())
}

/// tsr when the slot is synced, else 1.
#[inline]
pub fn tsr_if(p: &SlotValue, tsr: f32) -> f32 {
    if p.temposync {
        tsr
    } else {
        1.0
    }
}

/// 1/tsr when the slot is synced, else 1.
#[inline]
pub fn tsr_inv_if(p: &SlotValue, tsr: f32) -> f32 {
    if p.temposync {
        1.0 / tsr.max(1e-6)
    } else {
        1.0
    }
}

/// Seconds of a log2-seconds slot (x 1/tsr when synced).
#[inline]
pub fn time_secs(p: &SlotValue, tsr: f32) -> f32 {
    p.v.exp2() * tsr_inv_if(p, tsr)
}

/// Cycles per block of a log2-Hz rate slot (x tsr when synced).
#[inline]
pub fn rate_per_block(p: &SlotValue, tsr: f32, sr: f32) -> f32 {
    rate_linear(-p.v, sr) * tsr_if(p, tsr)
}

/// An "audible frequency" slot (semitones vs A440) in octaves for
/// `core::calc_omega`.
#[inline]
pub fn oct(semis: f32) -> f32 {
    semis / 12.0
}

/// Value of a slot that may be extended (`f` applied when extended).
#[inline]
pub fn ext(p: &SlotValue, f: impl Fn(f32) -> f32) -> f32 {
    if p.extend {
        f(p.v)
    } else {
        p.v
    }
}

#[inline]
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

// ------------------------------------------------------------ clippers ---

/// x clamped to +-1.139, y = x (1 - x^2/3 + 2x^4/15 - 17x^6/315).
#[inline]
pub fn tanh7(x: f32) -> f32 {
    let x = x.clamp(-1.139, 1.139);
    let x2 = x * x;
    x * (1.0 - x2 / 3.0 + x2 * x2 * (2.0 / 15.0) - x2 * x2 * x2 * (17.0 / 315.0))
}

#[inline]
pub fn hardclip(x: f32) -> f32 {
    x.clamp(-1.0, 1.0)
}

#[inline]
pub fn hardclip8(x: f32) -> f32 {
    x.clamp(-8.0, 8.0)
}

pub use crate::kit::core::softclip;

/// 1.5 x - 0.5 x^3 (the cubic used after several mixers).
#[inline]
pub fn cubic_sat(x: f32) -> f32 {
    1.5 * x - 0.5 * x * x * x
}

/// Pade tanh with |x| clamped to 5 (FILTERS.md section 8).
#[inline]
pub fn pade_tanh(x: f32) -> f32 {
    let x = x.clamp(-5.0, 5.0);
    let x2 = x * x;
    x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2))) / (135135.0 + x2 * (62370.0 + x2 * (3150.0 + 28.0 * x2)))
}

/// Pade tan on (-pi/2, pi/2) (FILTERS.md section 8).
#[inline]
pub fn pade_tan(x: f32) -> f32 {
    let x2 = x * x;
    x * (-135135.0 + x2 * (17325.0 + x2 * (-378.0 + x2))) / (-135135.0 + x2 * (62370.0 + x2 * (-3150.0 + 28.0 * x2)))
}

// ------------------------------------------- legacy waveshaper tables ----

struct LegacyTables {
    tanh: [f32; 1024],
    hard: [f32; 1024],
    asym: [f32; 1024],
    sine: [f32; 1024],
}

fn legacy() -> &'static LegacyTables {
    static T: OnceLock<LegacyTables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = LegacyTables { tanh: [0.0; 1024], hard: [0.0; 1024], asym: [0.0; 1024], sine: [0.0; 1024] };
        let h = |v: f64| (v.exp() - (-1.2 * v).exp()) / (v.exp() + (-v).exp());
        for i in 0..1024 {
            let u = (i as f64 - 512.0) / 32.0;
            t.tanh[i] = u.tanh() as f32;
            t.hard[i] = (u.signum() * u.abs().powi(5).tanh().powf(0.2)) as f32;
            t.asym[i] = (h(u + 0.5) - h(0.5)) as f32;
            t.sine[i] = ((i as f64 - 512.0) * PI / 512.0).sin() as f32;
        }
        t
    })
}

/// The 1024-entry table of a waveshaper id with a legacy table (1 Soft =
/// tanh, 2 Hard, 3 Asymmetric, 4 Sine, 5 Digital = tanh).
pub fn legacy_table(ws: u8) -> &'static [f32; 1024] {
    let t = legacy();
    match ws {
        2 => &t.hard,
        3 => &t.asym,
        4 => &t.sine,
        _ => &t.tanh,
    }
}

/// Waveshaper id numbers used by the effects.
pub const WS_SOFT: u8 = 1;
pub const WS_HARD: u8 = 2;
pub const WS_ASYM: u8 = 3;

/// `lookup_waveshape(type, x)` (section 4.7): e = trunc(32x + 512), linear
/// interpolation; -1 below index 1, +1 above 1021.
#[inline]
pub fn lookup_waveshape(ws: u8, x: f32) -> f32 {
    let t = legacy_table(ws);
    let p = 32.0 * x + 512.0;
    if !(p >= 1.0) {
        return -1.0;
    }
    let e = p as usize;
    if e > 1021 {
        return 1.0;
    }
    let a = p - e as f32;
    (1.0 - a) * t[e] + a * t[e + 1]
}

/// `lookup_waveshape_warp`: 256x + 512, index wraps.
#[inline]
pub fn lookup_waveshape_warp(ws: u8, x: f32) -> f32 {
    let t = legacy_table(ws);
    let p = 256.0 * x + 512.0;
    let e = p.floor();
    let a = p - e;
    let e = (e as i64).rem_euclid(1024) as usize;
    (1.0 - a) * t[e] + a * t[(e + 1) & 1023]
}

// ---------------------------------------------- windowed-sinc delays -----

pub const FIR_M: usize = 256;
pub const FIR_N: usize = 12;
pub type SincTable = [[f32; FIR_N]; FIR_M + 1];

fn build_sinc(cut: f64) -> Box<SincTable> {
    let mut t = Box::new([[0f32; FIR_N]; FIR_M + 1]);
    let n = FIR_N as f64;
    for j in 0..=FIR_M {
        for i in 0..FIR_N {
            let x = -(i as f64) + n / 2.0 + j as f64 / FIR_M as f64 - 1.0;
            let w = 0.42 - 0.5 * (2.0 * PI * (x - n / 2.0) / n).cos() + 0.08 * (4.0 * PI * (x - n / 2.0) / n).cos();
            let u = cut * x;
            let sinc = if u.abs() < 1e-12 { 1.0 } else { (PI * u).sin() / (PI * u) };
            t[j][i] = (w * cut * sinc) as f32;
        }
    }
    t
}

/// The "1X" table (0.85 cutoff) of the delay-style effects.
pub fn sinc_1x() -> &'static SincTable {
    static T: OnceLock<Box<SincTable>> = OnceLock::new();
    T.get_or_init(|| build_sinc(0.85))
}

/// The "2X" table (0.455 cutoff) of the generic sinc delay line.
pub fn sinc_2x() -> &'static SincTable {
    static T: OnceLock<Box<SincTable>> = OnceLock::new();
    T.get_or_init(|| build_sinc(0.455))
}

/// A block-written delay line read with the "1X" kernel (section 4.5):
/// power-of-two length with N guard samples mirrored past the end. Read a
/// block's taps first, then write the block.
pub struct BlockDelay {
    buf: Vec<f32>,
    len: usize,
    wp: usize,
}

impl BlockDelay {
    /// `len` a power of two.
    pub fn new(len: usize) -> BlockDelay {
        debug_assert!(len.is_power_of_two());
        BlockDelay { buf: vec![0.0; len + FIR_N], len, wp: 0 }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.wp = 0;
    }
    /// The sample at block offset k read `d` samples behind (the caller
    /// has subtracted the kernel's 6-sample centre when it wants that).
    #[inline]
    pub fn read(&self, d: f32, k: usize) -> f32 {
        let tab = sinc_1x();
        let id = (d.floor() as i64).clamp(B as i64, (self.len - FIR_N - 1) as i64);
        let rp = (self.wp as i64 - id + k as i64 - FIR_N as i64).rem_euclid(self.len as i64) as usize;
        let ph = ((FIR_M as f32 * (id as f32 + 1.0 - d)).floor() as i64).clamp(0, FIR_M as i64 - 1) as usize;
        let row = &tab[ph];
        let mut y = 0.0;
        for i in 0..FIR_N {
            y += row[i] * self.buf[rp + i];
        }
        y
    }
    /// Write one sample at block offset k (call for k = 0..B, then `advance`).
    #[inline]
    pub fn write_at(&mut self, k: usize, x: f32) {
        let i = (self.wp + k) & (self.len - 1);
        self.buf[i] = x;
        if i < FIR_N {
            self.buf[self.len + i] = x;
        }
    }
    /// Move the write head on by one block.
    pub fn advance(&mut self) {
        self.wp = (self.wp + B) & (self.len - 1);
    }
}

/// The generic per-sample sinc delay line ("2X" kernel, section 4.5):
/// `read(d)` before this sample's `write` gives the input `d` samples ago
/// (after the write: d - 1 ago).
pub struct SincLine {
    buf: Vec<f32>,
    len: usize,
    wp: usize,
}

impl SincLine {
    pub fn new(len: usize) -> SincLine {
        debug_assert!(len.is_power_of_two());
        SincLine { buf: vec![0.0; len + FIR_N], len, wp: 0 }
    }
    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.wp = 0;
    }
    pub fn len(&self) -> usize {
        self.len
    }
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.wp] = x;
        if self.wp < FIR_N {
            self.buf[self.len + self.wp] = x;
        }
        self.wp = (self.wp + 1) & (self.len - 1);
    }
    /// Read `d` samples behind the next write slot (clamped to [6, len - 13]).
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let d = d.clamp(6.0, (self.len - FIR_N - 1) as f32);
        let i = d.floor();
        let f = d - i;
        let row = (((1.0 - f) * FIR_M as f32) as usize).min(FIR_M);
        let start = (self.wp as i64 - i as i64 - 6).rem_euclid(self.len as i64) as usize;
        let tab = &sinc_2x()[row];
        let mut y = 0.0;
        for n in 0..FIR_N {
            y += tab[n] * self.buf[start + n];
        }
        y
    }
    /// Linear interpolation between the samples `floor(d)` and `floor(d)+1`
    /// writes ago (d = 1 = the last written sample).
    #[inline]
    pub fn read_linear(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, (self.len - 2) as f32);
        let i = d.floor();
        let f = d - i;
        let a = self.buf[(self.wp as i64 - i as i64).rem_euclid(self.len as i64) as usize];
        let b = self.buf[(self.wp as i64 - i as i64 - 1).rem_euclid(self.len as i64) as usize];
        a * (1.0 - f) + b * f
    }
}

// --------------------------------------------------------------- LFOs ----

fn sine_table() -> &'static [f32; 8193] {
    static T: OnceLock<Box<[f32; 8193]>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0f32; 8193]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (2.0 * PI * i as f64 / 8192.0).sin() as f32;
        }
        t
    })
}

/// sin(2 pi p) from the 8192-entry table, linear interpolation.
#[inline]
pub fn sine_turns(p: f32) -> f32 {
    let x = (p - p.floor()) * 8192.0;
    let e = (x as usize).min(8191);
    let a = x - e as f32;
    let t = sine_table();
    (1.0 - a) * t[e] + a * t[e + 1]
}

/// The FX mod LFO's deterministic waves (section 5.0 order): 0 Sine,
/// 1 Triangle, 2 Ramp, 3 Saw, 4 Square. (5 Noise and 6 S&H are stateful.)
#[inline]
pub fn lfo_wave(w: i32, p: f32) -> f32 {
    match w {
        0 => sine_turns(p),
        1 => 2.0 * (2.0 * p - 1.0).abs() - 1.0,
        2 => ramp(p),
        3 => -ramp(p),
        4 => {
            if p < 0.49 {
                1.0
            } else if p <= 0.5 {
                50.0 - 100.0 * p
            } else if p < 0.99 {
                -1.0
            } else {
                100.0 * p - 99.0
            }
        }
        _ => 0.0,
    }
}

/// Rising ramp: 2p/0.98 - 1 below 0.98, then a fall from +1 to -1.
#[inline]
pub fn ramp(p: f32) -> f32 {
    if p < 0.98 {
        2.0 * p / 0.98 - 1.0
    } else {
        1.0 - 2.0 * (p - 0.98) / 0.02
    }
}

pub const LFO_NOISE: i32 = 5;
pub const LFO_SH: i32 = 6;

/// The 4-output FX modulation LFO of section 5.0. Call `tick` once per
/// control tick; `at(i, k)` is output i ramped across the block (depth
/// applied), `value(i)` its end value.
pub struct ModLfo {
    ph: f32,
    rng: Rng,
    target: [f32; 4],
    glide: [f32; 4],
    out: [Line; 4],
    depth: Line,
    last_bucket: i32,
    first: bool,
    /// One random value for all outputs (the Neuron variant).
    pub single_random: bool,
}

impl ModLfo {
    pub fn new(seed: u32) -> ModLfo {
        ModLfo {
            ph: 0.0,
            rng: Rng::new(seed),
            target: [0.0; 4],
            glide: [0.0; 4],
            out: [Line::default(); 4],
            depth: Line::default(),
            last_bucket: -1,
            first: true,
            single_random: false,
        }
    }
    pub fn reset(&mut self) {
        let s = self.single_random;
        let seed = self.rng.next_u32();
        *self = ModLfo::new(seed);
        self.single_random = s;
    }
    pub fn phase(&self) -> f32 {
        self.ph
    }
    pub fn set_phase(&mut self, p: f32) {
        self.ph = p - p.floor();
    }

    fn draw(&mut self, spread: f32) {
        let mut t = [0f32; 4];
        if self.single_random {
            let v = self.rng.bipolar();
            t = [v; 4];
        } else {
            for v in t.iter_mut() {
                *v = self.rng.bipolar();
            }
        }
        let w3 = spread * spread * spread;
        let h = (1.0 - w3) * 0.5;
        for pair in [0usize, 2] {
            let (a, b) = (t[pair], t[pair + 1]);
            self.target[pair] = a + h * (b - a);
            self.target[pair + 1] = b - h * (b - a);
        }
    }

    /// One control tick: wave index (section 5.0 order), phase increment
    /// in cycles per tick (0 = frozen at `offset`), depth, phase offset,
    /// stereo spread 0..1.
    pub fn tick(&mut self, wave: i32, inc: f32, depth: f32, offset: f32, spread: f32) {
        let phase0 = if inc > 0.0 {
            self.ph += inc;
            self.ph -= self.ph.floor();
            let p = self.ph + offset;
            p - p.floor()
        } else {
            offset - offset.floor()
        };
        let reset = if inc > 0.0 {
            phase0 - inc <= 0.0
        } else {
            let b = (16.0 * phase0).floor() as i32;
            let r = b != self.last_bucket;
            self.last_bucket = b;
            r
        };
        if wave == LFO_NOISE || wave == LFO_SH {
            if reset || self.first {
                self.draw(spread);
            }
        }
        let mut v = [0f32; 4];
        for (i, vi) in v.iter_mut().enumerate() {
            *vi = match wave {
                LFO_SH => self.target[i],
                LFO_NOISE => {
                    let g = if inc == 0.0 { 0.98 * phase0 } else { 2.0 * inc };
                    if inc >= 0.98 || self.first {
                        self.glide[i] = self.target[i];
                    } else {
                        self.glide[i] = (self.glide[i] + (self.target[i] - self.glide[i]) * g).clamp(-1.0, 1.0);
                    }
                    self.glide[i]
                }
                w => {
                    let p = phase0 + i as f32 * spread * 0.25;
                    lfo_wave(w, p - p.floor())
                }
            };
        }
        for i in 0..4 {
            self.out[i].set_target(v[i]);
        }
        self.depth.set_target(depth);
        self.first = false;
    }

    #[inline]
    pub fn at(&self, i: usize, k: usize) -> f32 {
        self.out[i].at(k) * self.depth.at(k)
    }
    #[inline]
    pub fn value(&self, i: usize) -> f32 {
        self.out[i].target * self.depth.target
    }
}

/// The "simple LFO" of Floaty Delay (section 5.17): phase advancing per
/// block; a sine and a smooth (cubic-interpolated) random.
pub struct SimpleLfo {
    pub ph: f32,
    hist: [f32; 4],
    rng: Rng,
}

impl SimpleLfo {
    pub fn new(seed: u32) -> SimpleLfo {
        let mut rng = Rng::new(seed);
        let hist = [rng.bipolar(), rng.bipolar(), rng.bipolar(), rng.bipolar()];
        SimpleLfo { ph: 0.0, hist, rng }
    }
    /// Advance by one block at log2-Hz `rate`; returns true on a wrap.
    pub fn advance(&mut self, rate: f32, sr: f32) -> bool {
        self.ph += rate_linear(-rate, sr);
        if self.ph >= 1.0 {
            self.ph -= self.ph.floor();
            self.hist = [self.hist[1], self.hist[2], self.hist[3], self.rng.bipolar()];
            true
        } else {
            false
        }
    }
    pub fn sine(&self) -> f32 {
        -sine_turns(self.ph - 0.5)
    }
    pub fn smooth_noise(&self) -> f32 {
        let [y0, y1, y2, y3] = self.hist;
        let mu = self.ph;
        let a0 = y3 - y2 - y0 + y1;
        let a1 = y0 - y1 - a0;
        let a2 = y2 - y0;
        let a3 = y1;
        ((a0 * mu + a1) * mu + a2) * mu + a3
    }
}

/// A unit phasor (r, i) starting at (0, -1), rotating by `w` per step:
/// r = sin(theta), i = -cos(theta).
#[derive(Clone, Copy, Debug)]
pub struct Quadrature {
    pub r: f64,
    pub i: f64,
    cw: f64,
    sw: f64,
    n: u32,
}

impl Default for Quadrature {
    fn default() -> Self {
        Quadrature::new(0.0)
    }
}

impl Quadrature {
    pub fn new(theta: f64) -> Quadrature {
        Quadrature { r: theta.sin(), i: -theta.cos(), cw: 1.0, sw: 0.0, n: 0 }
    }
    pub fn set_rate(&mut self, w: f64) {
        self.cw = w.cos();
        self.sw = w.sin();
    }
    #[inline]
    pub fn step(&mut self) {
        let r = self.r * self.cw - self.i * self.sw;
        let i = self.i * self.cw + self.r * self.sw;
        self.r = r;
        self.i = i;
        self.n += 1;
        if self.n >= 1024 {
            self.n = 0;
            let m = 1.0 / (r * r + i * i).sqrt();
            self.r *= m;
            self.i *= m;
        }
    }
}

// ----------------------------------------------------------- filters -----

/// The trapezoidal SVF ("Fast SVF", FILTERS.md 5.15) with per-block
/// coefficients ramped across the block. Modes: LP, HP, BP, and the
/// shelves/bell with an amplitude A.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    a: [f32; 3],
    m: [f32; 3],
    da: [f32; 3],
    dm: [f32; 3],
    s: [[f32; 2]; 2],
    first: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvfMode {
    Lp,
    Hp,
    Bp,
    Notch,
    Peak,
    Allpass,
    Bell,
    LowShelf,
    HighShelf,
}

impl Svf {
    pub fn new() -> Svf {
        Svf { first: true, ..Default::default() }
    }
    pub fn reset(&mut self) {
        self.s = [[0.0; 2]; 2];
        self.first = true;
    }
    /// Coefficients for frequency `hz`, resonance `res` (0..0.99), amplitude
    /// `amp` (shelves/bell), at `fs`; ramped over the next `n` ticks.
    pub fn set(&mut self, mode: SvfMode, hz: f32, res: f32, amp: f32, fs: f32, n: usize) {
        let g = pade_tan(std::f32::consts::PI * (hz / fs).clamp(0.0, 0.499));
        let rho = res.clamp(0.0, 0.99);
        let mut k = 2.0 - 2.0 * rho;
        let a = amp;
        if mode == SvfMode::Bell {
            k /= a.max(0.001);
        }
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let m = match mode {
            SvfMode::Lp => [0.0, 0.0, 1.0],
            SvfMode::Hp => [1.0, -k, -1.0],
            SvfMode::Bp => [0.0, 1.0, 0.0],
            SvfMode::Notch => [1.0, -k, 0.0],
            SvfMode::Peak => [1.0, -k, -2.0],
            SvfMode::Allpass => [1.0, -2.0 * k, 0.0],
            SvfMode::Bell => [1.0, k * (a * a - 1.0), 0.0],
            SvfMode::LowShelf => [1.0, k * (a - 1.0), a * a - 1.0],
            SvfMode::HighShelf => [a * a, a * k * (1.0 - a), 1.0 - a * a],
        };
        let na = [a1, a2, a3];
        if self.first || n == 0 {
            self.a = na;
            self.m = m;
            self.da = [0.0; 3];
            self.dm = [0.0; 3];
            self.first = false;
        } else {
            let inv = 1.0 / n as f32;
            for j in 0..3 {
                self.da[j] = (na[j] - self.a[j]) * inv;
                self.dm[j] = (m[j] - self.m[j]) * inv;
            }
        }
    }
    /// One stereo tick (coefficients step after use).
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32) -> (f32, f32) {
        let yl = self.run(0, l);
        let yr = self.run(1, r);
        for j in 0..3 {
            self.a[j] += self.da[j];
            self.m[j] += self.dm[j];
        }
        (yl, yr)
    }
    /// One mono tick on channel 0 (coefficients step after use).
    #[inline]
    pub fn tick_mono(&mut self, x: f32) -> f32 {
        let y = self.run(0, x);
        for j in 0..3 {
            self.a[j] += self.da[j];
            self.m[j] += self.dm[j];
        }
        y
    }
    #[inline]
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        let [a1, a2, a3] = self.a;
        let s = &mut self.s[ch];
        let v3 = x - s[1];
        let v1 = a1 * s[0] + a2 * v3;
        let v2 = s[1] + a2 * s[0] + a3 * v3;
        s[0] = 2.0 * v1 - s[0];
        s[1] = 2.0 * v2 - s[1];
        self.m[0] * x + self.m[1] * v1 + self.m[2] * v2
    }
    /// Stop any ramp at its end value.
    pub fn settle(&mut self) {
        self.da = [0.0; 3];
        self.dm = [0.0; 3];
    }
}

/// The Phaser/Combulator "Tone" cuts in semitones vs A440 (before the -2
/// octave offset): (lp, hp).
pub fn tone_cuts(tone: f32) -> (f32, f32) {
    let t = tone.clamp(-1.0, 1.0);
    let mut lp = 67.0;
    let mut hp = -33.0;
    if t > 0.0 {
        hp = t * (67.0 + 33.0) - 33.0;
    }
    if t < 0.0 {
        lp = t.abs() * (-12.0 - 67.0) + 67.0;
    }
    (lp, hp)
}

// ------------------------------------------------------ slot builders ----

pub const fn unused() -> SlotDef {
    SlotDef::UNUSED
}
pub const fn fl(name: &'static str, min: f32, max: f32, default: f32, unit: SlotUnit) -> SlotDef {
    SlotDef::float(name, min, max, default, unit)
}
pub const fn percent(name: &'static str, default: f32) -> SlotDef {
    SlotDef::float(name, 0.0, 1.0, default, SlotUnit::Percent)
}
pub const fn bipolar(name: &'static str, default: f32) -> SlotDef {
    SlotDef::float(name, -1.0, 1.0, default, SlotUnit::Percent)
}
pub const fn db(name: &'static str, min: f32, max: f32, default: f32) -> SlotDef {
    SlotDef::float(name, min, max, default, SlotUnit::Db)
}
/// "audible frequency": -60..70 semitones vs A440.
pub const fn freq(name: &'static str, default: f32) -> SlotDef {
    SlotDef::float(name, -60.0, 70.0, default, SlotUnit::NoteA440)
}
/// A deactivatable audible frequency (low cut / high cut).
pub const fn freq_d(name: &'static str, default: f32) -> SlotDef {
    deact(freq(name, default))
}
/// log2 seconds with tempo sync.
pub const fn envtime(name: &'static str, min: f32, max: f32, default: f32) -> SlotDef {
    sync(SlotDef::float(name, min, max, default, SlotUnit::Log2Secs))
}
/// log2 Hz -7..9 with tempo sync.
pub const fn lforate(name: &'static str, default: f32) -> SlotDef {
    sync(SlotDef::float(name, -7.0, 9.0, default, SlotUnit::Log2Hz))
}
/// Width in dB, -24..24.
pub const fn width_db(name: &'static str) -> SlotDef {
    db(name, -24.0, 24.0, 0.0)
}
pub const fn choice(name: &'static str, choices: &'static [&'static str], default: f32) -> SlotDef {
    SlotDef::choice(name, choices, default)
}
pub const fn int(name: &'static str, min: f32, max: f32, default: f32) -> SlotDef {
    SlotDef::int(name, min, max, default)
}
pub const fn deact(s: SlotDef) -> SlotDef {
    SlotDef { can_deactivate: true, ..s }
}
pub const fn extend(s: SlotDef) -> SlotDef {
    SlotDef { can_extend: true, ..s }
}
pub const fn sync(s: SlotDef) -> SlotDef {
    SlotDef { can_temposync: true, ..s }
}
pub const fn absolute(s: SlotDef) -> SlotDef {
    SlotDef { can_absolute: true, ..s }
}
pub const fn deform(s: SlotDef, names: &'static [&'static str]) -> SlotDef {
    SlotDef { deform: names, ..s }
}
pub const fn about(s: SlotDef, text: &'static str) -> SlotDef {
    SlotDef { about: text, ..s }
}

/// The waveshaper model sub-list of Distortion and Rotary (WAVESHAPERS.md 4).
pub const MODEL_NAMES: [&str; 8] = ["Soft", "Hard", "Asymmetric", "Sine", "Digital", "OJD", "Full Wave", "Fuzz Soft Clip"];
