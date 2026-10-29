//! The waveshaper family (WAVESHAPERS.md): 43 static and stateful curves
//! (saturators, harmonic and additive Chebyshev shapers, rectifiers with
//! first-order antiderivative anti-aliasing, wavefolders, fuzz and
//! trigonometric tables), one per voice in the filter block and reused by
//! effects.
//!
//! A [`Shaper`] owns four registers and the `init` flag; the voice calls
//! [`Shaper::begin_block`] at the top of every 64-sample block (which makes
//! the anti-aliased types output their plain curve on that block's first
//! sample, as the reference does), then [`Shaper::tick`] per sample with
//! the linear drive.

use std::sync::{Mutex, OnceLock};

// ---- mirrored reference quirks (the mirrored path is the only behaviour) ----

/// The voice sets every shaper's init flag at the start of every block, so ADAA
/// types output F(x) instead of the antiderivative difference on sample 0.
pub const QUIRK_INIT_EVERY_BLOCK: bool = true;
/// Fuzz (24) and Fuzz Soft Clip (25) share one random stream (one generator per
/// range value): whichever table is built second continues the first's draws.
pub const QUIRK_FUZZ_SHARED_STREAM: bool = true;
/// The legacy table lookup's "next" entry wraps to entry 0 at the top edge.
pub const QUIRK_LEGACY_TABLE_WRAP: bool = true;
/// Table interpolation takes its fraction from the unclamped position.
pub const QUIRK_TABLE_FRACTION_UNCLAMPED: bool = true;
/// The folders output 0 (and AD 0) outside their outermost points (+-10).
pub const QUIRK_FOLDER_ZERO_OUTSIDE: bool = true;

/// Names, index = stored `ws_type`.
pub const SHAPER_NAMES: [&str; 43] = [
    "Off",
    "Soft",
    "Hard",
    "Asymmetric",
    "Sine",
    "Digital",
    "Soft Harmonic 2",
    "Soft Harmonic 3",
    "Soft Harmonic 4",
    "Soft Harmonic 5",
    "Full Wave",
    "Half Wave Positive",
    "Half Wave Negative",
    "Soft Rectifier",
    "Single Fold",
    "Double Fold",
    "West Coast Fold",
    "Additive 1+2",
    "Additive 1+3",
    "Additive 1+4",
    "Additive 1+5",
    "Additive 12345",
    "Additive Saw 3",
    "Additive Square 3",
    "Fuzz",
    "Fuzz Soft Clip",
    "Heavy Fuzz",
    "Fuzz Center",
    "Fuzz Soft Edge",
    "Sin+x",
    "Sin 2x + x",
    "Sin 3x + x",
    "Sin 7x + x",
    "Sin 10x + x",
    "2 Cycle",
    "7 Cycle",
    "10 Cycle",
    "2 Cycle Bound",
    "7 Cycle Bound",
    "10 Cycle Bound",
    "Medium",
    "OJD",
    "Soft Single Fold",
];

/// Short UI names.
pub const SHAPER_SHORT: [&str; 43] = [
    "Off", "Soft", "Hard", "Asym", "Sine", "Digital", "Harm 2", "Harm 3", "Harm 4", "Harm 5", "FullRect", "HalfPos", "HalfNeg",
    "SoftRect", "1Fold", "2Fold", "WCFold", "Add12", "Add13", "Add14", "Add15", "Add1-5", "AddSaw3", "AddSqr3", "Fuzz", "SoftFz",
    "HeavyFz", "CenterFz", "EdgeFz", "Sin+x", "Sin2x+x", "Sin3x+x", "Sin7x+x", "Sin10x+x", "2Cycle", "7Cycle", "10Cycle", "2CycleB",
    "7CycleB", "10CycleB", "Medium", "OJD", "Sft1Fld",
];

/// Menu groups in menu order: (group, ids).
pub const SHAPER_MENU: [(&str, &[u8]); 8] = [
    ("", &[0]),
    ("Saturator", &[1, 40, 2, 3, 41]),
    ("Effect", &[4, 5]),
    ("Harmonic", &[6, 7, 8, 9, 17, 18, 19, 20, 21, 22, 23]),
    ("Rectifiers", &[10, 11, 12, 13]),
    ("Wavefolder", &[42, 14, 15, 16]),
    ("Fuzz", &[24, 26, 27, 25, 28]),
    ("Trigonometric", &[29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39]),
];

/// The 8-entry shaper list of the Distortion and Rotary effects (index ->
/// shaper id): Soft, Hard, Asymmetric, Sine, Digital, OJD, Full Wave, Fuzz
/// Soft Clip.
pub const FX_SHAPER_SUBLIST: [u8; 8] = [1, 2, 3, 4, 5, 41, 10, 25];

#[inline]
fn clip(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

/// The rational tanh saturator T(x, D).
#[inline]
pub fn soft(x: f32, d: f32) -> f32 {
    let v = x * d;
    clip(v * (27.0 + v * v) / (27.0 + 9.0 * v * v))
}

/// The OJD curve (no drive).
#[inline]
pub fn ojd(v: f32) -> f32 {
    if v <= -1.7 {
        -1.0
    } else if v < -0.3 {
        let a = v + 0.3;
        a + a * a / (4.0 * 0.7) - 0.3
    } else if v <= 0.9 {
        v
    } else if v < 1.1 {
        let a = v - 0.9;
        a - a * a / (4.0 * 0.1) + 0.9
    } else {
        1.0
    }
}

// ---- legacy 1024-point tables ----

/// The legacy 1024-entry tables (abscissa u = (i - 512) / 32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyTable {
    Tanh,
    Hard,
    Asym,
    Sine,
    Digital,
}

/// One legacy table.
pub fn legacy_table(which: LegacyTable) -> &'static [f32; 1024] {
    static T: OnceLock<[[f32; 1024]; 5]> = OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [[0.0f32; 1024]; 5];
        let h = |v: f64| (v.exp() - (-1.2 * v).exp()) / (v.exp() + (-v).exp());
        for i in 0..1024 {
            let u = (i as f64 - 512.0) / 32.0;
            t[0][i] = u.tanh() as f32;
            t[1][i] = (u.signum() * u.abs().powi(5).tanh().powf(0.2)) as f32;
            t[2][i] = (h(u + 0.5) - h(0.5)) as f32;
            t[3][i] = ((i as f64 - 512.0) * std::f64::consts::PI / 512.0).sin() as f32;
            t[4][i] = u.tanh() as f32;
        }
        t[1][512] = 0.0;
        t
    });
    &t[which as usize]
}

/// The voice shapers' legacy lookup at v (= x D): round-to-nearest index,
/// fraction in -0.5..0.5, next entry wrapping at the top edge.
#[inline]
pub fn legacy_lookup(tab: &[f32; 1024], v: f32) -> f32 {
    let p = v * 32.0 + 512.0;
    let ef = p.round_ties_even();
    let t = p - ef;
    let e = ef.clamp(0.0, 1023.0) as usize;
    let next = if QUIRK_LEGACY_TABLE_WRAP { (e + 1) % 1024 } else { (e + 1).min(1023) };
    (1.0 - t) * tab[e] + t * tab[next]
}

/// The effects' scalar legacy lookup: truncated index, +-1 past the edges.
#[inline]
pub fn legacy_lookup_scalar(tab: &[f32; 1024], x: f32) -> f32 {
    let p = 32.0 * x + 512.0;
    let e = p.trunc();
    let t = p - e;
    if e > 1021.0 {
        1.0
    } else if e < 1.0 {
        -1.0
    } else {
        let e = e as usize;
        (1.0 - t) * tab[e] + t * tab[e + 1]
    }
}

// ---- [-1, 1] tables ----

/// A table of N intervals on [-1, 1] (N + 1 points).
pub struct Pm1Table {
    pub data: Vec<f32>,
}

impl Pm1Table {
    fn build(n: usize, mut g: impl FnMut(f32) -> f32) -> Pm1Table {
        let step = 2.0 / n as f32;
        Pm1Table { data: (0..=n).map(|i| g(i as f32 * step - 1.0)).collect() }
    }

    pub fn intervals(&self) -> usize {
        self.data.len() - 1
    }

    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let n = self.intervals() as f32;
        let p = x * n / 2.0 + n / 2.0;
        let e = p.clamp(0.0, n - 1.0).round_ties_even();
        let t = if QUIRK_TABLE_FRACTION_UNCLAMPED { p - e } else { p.clamp(0.0, n) - e };
        let e = e as usize;
        (1.0 - t) * self.data[e] + t * self.data[e + 1]
    }
}

/// The table generator: the "minimal standard" multiplicative LCG.
#[derive(Clone, Copy, Debug)]
pub struct MinStd(pub u32);

impl MinStd {
    pub const SEED: u32 = 2112;
    #[inline]
    pub fn next(&mut self) -> u32 {
        self.0 = ((self.0 as u64 * 48271) % 2147483647) as u32;
        self.0
    }
    /// One canonical single-precision draw mapped to [a, b).
    #[inline]
    pub fn uniform(&mut self, a: f32, b: f32) -> f32 {
        let mut u = (self.next() - 1) as f32 / 2147483646u32 as f32;
        if u >= 1.0 {
            u = 1.0 - f32::EPSILON / 2.0;
        }
        a + u * (b - a)
    }
}

fn fuzz_table(id: u8) -> &'static Pm1Table {
    static SHARED_01: Mutex<MinStd> = Mutex::new(MinStd(MinStd::SEED));
    static T: [OnceLock<Pm1Table>; 5] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = (id - 24) as usize;
    T[slot].get_or_init(|| match id {
        24 | 25 => {
            let mut fresh = MinStd(MinStd::SEED);
            let mut guard = SHARED_01.lock().unwrap_or_else(|e| e.into_inner());
            let g: &mut MinStd = if QUIRK_FUZZ_SHARED_STREAM { &mut guard } else { &mut fresh };
            Pm1Table::build(1024, |v| v * 0.9 + g.uniform(-0.1, 0.1))
        }
        26 => {
            let mut g = MinStd(MinStd::SEED);
            Pm1Table::build(1024, |v| v * 0.7 + g.uniform(-0.3, 0.3))
        }
        27 => {
            let mut g = MinStd(MinStd::SEED);
            Pm1Table::build(2048, |v| v + (-20.0 * v * v).exp() * g.uniform(-1.0, 1.0))
        }
        _ => {
            let mut g = MinStd(MinStd::SEED);
            Pm1Table::build(2048, |v| 0.85 * v + 0.15 * v * v * v * v * g.uniform(-1.0, 1.0))
        }
    })
}

/// The definition of a trigonometric table type (29-39) at v.
pub fn trig_curve(id: u8, v: f64) -> f64 {
    use std::f64::consts::PI;
    let n = match id {
        30 | 34 | 37 => 2.0,
        31 => 3.0,
        32 | 35 | 38 => 7.0,
        _ => 10.0,
    };
    match id {
        29 => v - (PI * v).sin(),
        30..=33 => (1.0 - v.abs()) * (PI * n * v).sin() + v,
        34..=36 => (PI * n * v).sin(),
        _ => (1.0 - v.abs()) * (PI * n * v).sin(),
    }
}

fn trig_table(id: u8) -> &'static Pm1Table {
    static T: [OnceLock<Pm1Table>; 11] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    T[(id - 29) as usize].get_or_init(|| {
        let n = if id == 29 { 1024 } else { 2048 };
        Pm1Table::build(n, |v| trig_curve(id, v as f64) as f32)
    })
}

/// A table type's table (24-39), building it on first use.
pub fn table_of(id: u8) -> Option<&'static Pm1Table> {
    match id {
        24..=28 => Some(fuzz_table(id)),
        29..=39 => Some(trig_table(id)),
        _ => None,
    }
}

// ---- folders ----

/// A piecewise-linear folder with its antiderivative constants.
pub struct Folder {
    pub xs: &'static [f32],
    pub ys: &'static [f32],
    c: Vec<f32>,
}

const FOLD1_X: [f32; 4] = [-10.0, -0.7, 0.7, 10.0];
const FOLD1_Y: [f32; 4] = [-1.0, 1.0, -1.0, 1.0];
const FOLD2_X: [f32; 8] = [-10.0, -3.0, -1.0, -0.3, 0.3, 1.0, 3.0, 10.0];
const FOLD2_Y: [f32; 8] = [-1.0, -0.9, 1.0, -1.0, 1.0, -1.0, 0.9, 1.0];
const WCFOLD_X: [f32; 14] = [
    -10.0,
    -2.0,
    -1.0919091909190919,
    -0.815881588158816,
    -0.5986598659865987,
    -0.3598359835983597,
    -0.11981198119811971,
    0.11981198119811971,
    0.3598359835983597,
    0.5986598659865987,
    0.8158815881588157,
    1.0919091909190919,
    2.0,
    10.0,
];
const WCFOLD_Y: [f32; 14] = [
    1.0,
    0.9,
    -0.679765619488133,
    0.5309659972270625,
    -0.6255506631744251,
    0.5991799179917987,
    -0.5990599059905986,
    0.5990599059905986,
    -0.5991799179917987,
    0.6255506631744251,
    -0.5309659972270642,
    0.679765619488133,
    -0.9,
    -1.0,
];

impl Folder {
    fn new(xs: &'static [f32], ys: &'static [f32]) -> Folder {
        let mut c = vec![-xs[0] * ys[0]];
        for i in 0..xs.len() - 1 {
            let d = xs[i + 1] - xs[i];
            let m = (ys[i + 1] - ys[i]) / d;
            let ci = c[i];
            c.push(m * d * d / 2.0 + ys[i] * xs[i + 1] + ci - ys[i + 1] * xs[i + 1]);
        }
        Folder { xs, ys, c }
    }

    /// (F, AD) at v; both 0 outside the points.
    #[inline]
    pub fn eval(&self, v: f32) -> (f32, f32) {
        for i in 0..self.xs.len() - 1 {
            if self.xs[i] <= v && v < self.xs[i + 1] {
                let m = (self.ys[i + 1] - self.ys[i]) / (self.xs[i + 1] - self.xs[i]);
                let dx = v - self.xs[i];
                return (m * dx + self.ys[i], m / 2.0 * dx * dx + self.ys[i] * v + self.c[i]);
            }
        }
        if QUIRK_FOLDER_ZERO_OUTSIDE {
            (0.0, 0.0)
        } else {
            (v.signum(), 0.0)
        }
    }
}

/// The folder of type 14, 15 or 16.
pub fn folder(id: u8) -> &'static Folder {
    static F: OnceLock<[Folder; 3]> = OnceLock::new();
    let f = F.get_or_init(|| [Folder::new(&FOLD1_X, &FOLD1_Y), Folder::new(&FOLD2_X, &FOLD2_Y), Folder::new(&WCFOLD_X, &WCFOLD_Y)]);
    &f[(id - 14) as usize]
}

// ---- Chebyshev ----

#[inline]
fn cheb_soft(n: u8, u: f32) -> f32 {
    match n {
        2 => 2.0 * u * u - 1.0,
        3 => 4.0 * u * u * u - 3.0 * u,
        4 => 8.0 * u * u * u * u - 8.0 * u * u + 1.0,
        _ => 16.0 * u * u * u * u * u - 20.0 * u * u * u + 5.0 * u,
    }
}

/// Additive types (17-23): (pre-gain s, weights w0.., DC block).
pub fn additive(id: u8) -> (f32, &'static [f32], bool) {
    const PHI: f32 = 0.9 / 1.75;
    const PSI: f32 = 0.9 / (1.0 - 0.25 + 1.0 / 16.0);
    match id {
        17 => (0.66, &[0.0, 0.5, 0.5], true),
        18 => (0.66, &[0.0, 0.5, 0.0, 0.5], false),
        19 => (0.66, &[0.0, 0.5, 0.0, 0.0, 0.5], true),
        20 => (0.66, &[0.0, 0.5, 0.0, 0.0, 0.0, 0.5], false),
        21 => (0.66, &[0.0, 0.2, 0.2, 0.2, 0.2, 0.2], true),
        22 => (-0.66, &[0.0, -PHI, PHI / 2.0, -PHI / 4.0], true),
        _ => (0.66, &[0.0, PSI, 0.0, -PSI / 4.0, 0.0, PSI / 16.0], false),
    }
}

#[inline]
fn cheb_series(w: &[f32], u: f32) -> f32 {
    let (mut t0, mut t1) = (1.0f32, u);
    let mut y = w[0] * t0;
    if w.len() > 1 {
        y += w[1] * t1;
    }
    for &wk in &w[2..] {
        let t2 = 2.0 * u * t1 - t0;
        y += wk * t2;
        t0 = t1;
        t1 = t2;
    }
    y
}

// ---- the stateful shaper ----

/// One shaper: four registers and the init flag.
#[derive(Clone, Copy, Debug)]
pub struct Shaper {
    pub r: [f32; 4],
    pub init: bool,
}

impl Default for Shaper {
    fn default() -> Self {
        Shaper { r: [0.0; 4], init: true }
    }
}

impl Shaper {
    pub fn new() -> Shaper {
        Shaper::default()
    }

    pub fn reset(&mut self) {
        *self = Shaper::default();
    }

    /// Top of every voice block.
    #[inline]
    pub fn begin_block(&mut self) {
        if QUIRK_INIT_EVERY_BLOCK {
            self.init = true;
        }
    }

    #[inline]
    fn dc(&mut self, x: f32, a: usize) -> f32 {
        let y = x - self.r[a] + 0.9999 * self.r[a + 1];
        self.r[a] = x;
        self.r[a + 1] = y;
        self.init = false;
        y
    }

    #[inline]
    fn adaa(&mut self, x: f32, f: f32, ad: f32, clear: bool) -> f32 {
        let y = if self.init || (x - self.r[0]).abs() < 1e-4 { f } else { (ad - self.r[1]) / (x - self.r[0]) };
        self.r[0] = x;
        self.r[1] = ad;
        if clear {
            self.init = false;
        }
        y
    }

    /// One sample of shaper `ty` at linear drive `d` (ids >= 43 are Off).
    #[inline]
    pub fn tick(&mut self, ty: u8, x: f32, d: f32) -> f32 {
        match ty {
            1 => soft(x, d),
            2 => clip(x * d),
            3 => legacy_lookup(legacy_table(LegacyTable::Asym), x * d),
            4 => {
                let tab = legacy_table(LegacyTable::Sine);
                let p = x * d * 256.0 + 512.0;
                let ef = p.round_ties_even();
                let t = p - ef;
                let e = ef.clamp(0.0, 1022.0) as usize;
                (1.0 - t) * tab[e] + t * tab[e + 1]
            }
            5 => {
                let q = (0.5 + 16.0 * x / d).round_ties_even();
                d * (q - 0.5) / 16.0
            }
            6..=9 => {
                let h = cheb_soft(ty - 4, clip(x));
                let h = self.dc(h, 0);
                soft(h, d)
            }
            10..=12 => {
                let v = clip(x * d);
                let (f, ad) = match ty {
                    10 => (v.abs(), v.signum() * v * v / 2.0),
                    11 => {
                        let f = v.max(0.0);
                        (f, f * f / 2.0)
                    }
                    _ => {
                        let f = v.min(0.0);
                        (f, f * f / 2.0)
                    }
                };
                self.adaa(v, f, ad, true)
            }
            13 => {
                let y = self.adaa(x, 2.0 * x.abs() - 1.0, x.signum() * x * x - x, false);
                let y = self.dc(y, 2);
                soft(y, d)
            }
            14..=16 => {
                let v = x * d;
                let (f, ad) = folder(ty).eval(v);
                self.adaa(v, f, ad, true)
            }
            17..=23 => {
                let (s, w, dc) = additive(ty);
                let u = soft(s * x, d);
                let y = cheb_series(w, u);
                if dc {
                    self.dc(y, 0)
                } else {
                    y
                }
            }
            24..=28 => {
                let u = if matches!(ty, 24 | 26) { clip(x * d) } else { soft(x, d) };
                let y = fuzz_table(ty).eval(u);
                self.dc(y, 0)
            }
            29..=39 => trig_table(ty).eval(clip(x * d)),
            40 => {
                let v = clip(x * d);
                let sg = if v >= 0.0 { 1.0 } else { -1.0 };
                2.0 * v - sg * v * v
            }
            41 => ojd(x * d),
            42 => {
                let v = x * d;
                v / (0.4 + 0.7 * v * v)
            }
            _ => x,
        }
    }
}

/// The stateless curve of a shaper: the first sample of a fresh shaper
/// (ADAA types give F, DC blockers pass their first input).
pub fn shape_static(ty: u8, x: f32, drive: f32) -> f32 {
    Shaper::default().tick(ty, x, drive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xs() -> impl Iterator<Item = f32> {
        (0..4096).map(|i| -2.0 + 4.0 * i as f32 / 4095.0)
    }

    fn soft64(x: f64, d: f64) -> f64 {
        let v = x * d;
        (v * (27.0 + v * v) / (27.0 + 9.0 * v * v)).clamp(-1.0, 1.0)
    }

    fn close(a: f32, b: f64, tol: f64, what: &str) {
        assert!((a as f64 - b).abs() <= tol, "{what}: {a} vs {b}");
    }

    #[test]
    fn closed_form_curves() {
        for d in [1.0f32, 0.5, 3.0] {
            let dd = d as f64;
            for x in xs() {
                let xx = x as f64;
                let v = (xx * dd).clamp(-1.0, 1.0);
                close(shape_static(0, x, d), xx, 0.0, "off");
                close(shape_static(1, x, d), soft64(xx, dd), 1e-6, "soft");
                close(shape_static(2, x, d), v, 1e-6, "hard");
                for n in 2..=5u8 {
                    let u = xx.clamp(-1.0, 1.0);
                    let t = (n as f64 * u.acos()).cos();
                    // f32 evaluation of the degree-n polynomial: ~16 ulp of its largest term.
                    close(shape_static(4 + n, x, d), soft64(t, dd), 1e-5, "harm");
                }
                close(shape_static(10, x, d), v.abs(), 1e-6, "full");
                close(shape_static(11, x, d), v.max(0.0), 1e-6, "halfpos");
                close(shape_static(12, x, d), v.min(0.0), 1e-6, "halfneg");
                close(shape_static(13, x, d), soft64(2.0 * xx.abs() - 1.0, dd), 2e-6, "softrect");
                close(shape_static(40, x, d), 2.0 * v - v.signum() * v * v, 1e-6, "medium");
                let w = xx * dd;
                close(shape_static(42, x, d), w / (0.4 + 0.7 * w * w), 1e-6, "sft1fld");
                let o = if w <= -1.7 {
                    -1.0
                } else if w < -0.3 {
                    (w + 0.3) + (w + 0.3).powi(2) / 2.8 - 0.3
                } else if w <= 0.9 {
                    w
                } else if w < 1.1 {
                    (w - 0.9) - (w - 0.9).powi(2) / 0.4 + 0.9
                } else {
                    1.0
                };
                close(shape_static(41, x, d), o, 1e-6, "ojd");
                let q = (0.5 + 16.0 * xx / dd).round_ties_even();
                close(shape_static(5, x, d), dd * (q - 0.5) / 16.0, 1e-6, "digital");
                for id in 17..=23u8 {
                    let (s, w, _) = additive(id);
                    let u = soft64(s as f64 * xx, dd).clamp(-1.0, 1.0);
                    let y: f64 = w.iter().enumerate().map(|(k, wk)| *wk as f64 * (k as f64 * u.acos()).cos()).sum();
                    close(shape_static(id, x, d), y, 1e-5, "additive");
                }
            }
        }
    }

    #[test]
    fn folders_match_points() {
        for id in 14..=16u8 {
            let f = folder(id);
            for x in xs() {
                let v = 5.5 * x;
                let i = f.xs.windows(2).position(|w| w[0] <= v && v < w[1]);
                let want = match i {
                    Some(i) => f.ys[i] as f64 + (f.ys[i + 1] - f.ys[i]) as f64 / (f.xs[i + 1] - f.xs[i]) as f64 * (v - f.xs[i]) as f64,
                    None => 0.0,
                };
                close(shape_static(id, x, 5.5), want, 2e-6, "fold");
            }
            // AD continuous and its slope is F.
            for k in 1..f.xs.len() - 1 {
                let xk = f.xs[k];
                let (_, a) = f.eval(xk - 1e-4);
                let (_, b) = f.eval(xk);
                assert!((a - b).abs() < 1e-3, "AD jump at {xk}");
            }
            let (_, a0) = f.eval(0.2);
            let (_, a1) = f.eval(0.2001);
            let (fv, _) = f.eval(0.20005);
            assert!(((a1 - a0) / 0.0001 - fv).abs() < 2e-2);
        }
        assert_eq!(folder(14).eval(10.5), (0.0, 0.0));
    }

    #[test]
    fn tables_interpolate_their_definition() {
        for id in 29..=39u8 {
            let t = table_of(id).unwrap();
            let n = t.intervals() as f64;
            let h = 2.0 / n;
            let freq = match id {
                29 => 1.0,
                31 => 3.0,
                30 | 34 | 37 => 2.0,
                32 | 35 | 38 => 7.0,
                _ => 10.0,
            };
            // Round-to-nearest indexing: up to half a step of extrapolation.
            let tol = (std::f64::consts::PI * freq).powi(2) * h * h * 0.375 + 2e-6;
            for x in xs() {
                let u = (x as f64).clamp(-1.0, 1.0);
                close(shape_static(id, x, 1.0), trig_curve(id, u), tol, "trig");
            }
        }
    }

    #[test]
    fn fuzz_tables_from_the_generator() {
        let mut g = MinStd(MinStd::SEED);
        assert_eq!(g.next(), 101948352);
        // Heavy Fuzz has its own stream: rebuild and compare nodes and lerp.
        let mut g = MinStd(MinStd::SEED);
        let nodes: Vec<f32> = (0..=1024).map(|i| {
            let v = i as f32 * (2.0 / 1024.0) - 1.0;
            v * 0.7 + g.uniform(-0.3, 0.3)
        }).collect();
        let t = table_of(26).unwrap();
        assert_eq!(t.data, nodes);
        for x in xs() {
            let u = (x as f32).clamp(-1.0, 1.0);
            let p = (u as f64 + 1.0) * 512.0;
            let e = p.clamp(0.0, 1023.0).round_ties_even();
            let a = p - e;
            let e = e as usize;
            let want = (1.0 - a) * nodes[e] as f64 + a * nodes[e + 1] as f64;
            close(t.eval(u), want, 1e-6, "heavy fuzz");
        }
        // 24 and 25: one shared stream, the second table continues it.
        let a = table_of(24).unwrap();
        let b = table_of(25).unwrap();
        let mut g = MinStd(MinStd::SEED);
        let mut first: Vec<f32> = Vec::new();
        for i in 0..=1024 {
            first.push(i as f32 * (2.0 / 1024.0) - 1.0);
            let k = first.len() - 1;
            first[k] = first[k] * 0.9 + g.uniform(-0.1, 0.1);
        }
        assert!(a.data == first || b.data == first);
        assert!(a.data != b.data);
    }

    #[test]
    fn adaa_and_init_quirk() {
        let mut s = Shaper::new();
        s.begin_block();
        let x0 = 0.3;
        assert_eq!(s.tick(10, x0, 1.0), x0);
        let x1 = -0.5f32;
        let y1 = s.tick(10, x1, 1.0);
        let ad = |v: f32| v.signum() * v * v / 2.0;
        assert!((y1 - (ad(x1) - ad(x0)) / (x1 - x0)).abs() < 1e-6);
        s.begin_block();
        assert_eq!(s.tick(10, 0.7, 1.0), 0.7);
        // The DC blocker removes a constant.
        let mut s = Shaper::new();
        let mut y = 0.0;
        for _ in 0..200000 {
            y = s.tick(6, 0.0, 1.0);
        }
        assert!(y.abs() < 1e-3, "{y}");
    }

    #[test]
    fn legacy_tables() {
        let asym = legacy_table(LegacyTable::Asym);
        assert!(asym[512].abs() < 1e-7);
        for x in xs() {
            let p = x as f64 * 32.0 + 512.0;
            let e = p.round_ties_even();
            let t = p - e;
            let e = e as usize;
            let want = (1.0 - t) * asym[e] as f64 + t * asym[e + 1] as f64;
            close(shape_static(3, x, 1.0), want, 1e-6, "asym");
            let p = x as f64 * 256.0 + 512.0;
            let e = p.round_ties_even();
            let t = p - e;
            let e = (e.clamp(0.0, 1022.0)) as usize;
            let sine = legacy_table(LegacyTable::Sine);
            close(shape_static(4, x, 1.0), (1.0 - t) * sine[e] as f64 + t * sine[e + 1] as f64, 1e-6, "sine");
        }
        assert!((shape_static(4, 1.0, 1.0) - 1.0).abs() < 1e-6);
        // The wrap at the top edge mixes in the bottom entry.
        let top = legacy_lookup(asym, 15.99);
        let t = 15.99f32 * 32.0 + 512.0 - 1024.0;
        assert!((top - ((1.0 - t) * asym[1023] + t * asym[0])).abs() < 1e-5);
    }

    #[test]
    fn effect_sublist_names() {
        let names: Vec<&str> = FX_SHAPER_SUBLIST.iter().map(|&i| SHAPER_NAMES[i as usize]).collect();
        assert_eq!(names, ["Soft", "Hard", "Asymmetric", "Sine", "Digital", "OJD", "Full Wave", "Fuzz Soft Clip"]);
    }
}
