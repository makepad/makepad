//! The web page's hashes and noise, ported bit-exactly, so the same seed gives the same layout.
//! Integer hashes wrap exactly as `Math.imul` and `>>> 0` do; everything else is `f64`, as in
//! JavaScript. The city's `hcell` is the trees' `sgTg`, so one function serves both.

/// The page's 2π. Its scripts and shaders use this truncated value, and layouts depend on it.
#[allow(clippy::approx_constant)]
pub const TAU: f64 = 6.2831853;

/// The page's `hu`: one round of a 32-bit integer mix.
#[inline]
pub fn hu(mut x: u32) -> u32 { x ^= x >> 16; x = x.wrapping_mul(0x7feb_352d); x ^= x >> 15; x = x.wrapping_mul(0x846c_a68b); x ^ (x >> 16) }
/// JavaScript's `Math.trunc(x) | 0` (wraps like `ToInt32`).
#[inline]
pub fn i32_of(x: f64) -> i32 { x.trunc() as i64 as i32 }
/// A hash's four bytes as 0..1 (the shader's `hb4`, the page's `pkR`).
pub fn hb4(h: u32) -> [f64; 4] { [0, 8, 16, 24].map(|s| ((h >> s) & 255) as f64 / 255.0) }
#[inline]
pub fn fract(x: f64) -> f64 { x - x.floor() }
#[inline]
pub fn clamp(x: f64, a: f64, b: f64) -> f64 { if x < a { a } else if x > b { b } else { x } }
/// `a + (b - a) t`, the page's `mix` (not GLSL's `a (1 - t) + b t`).
#[inline]
pub fn mix(a: f64, b: f64, t: f64) -> f64 { a + (b - a) * t }
/// GLSL `smoothstep`.
#[inline]
pub fn smooth(e0: f64, e1: f64, x: f64) -> f64 { let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0); t * t * (3.0 - 2.0 * t) }
/// Signed wrapped azimuth difference `a - b` (u units, -0.5 .. 0.5).
#[inline]
pub fn wd(a: f64, b: f64) -> f64 { fract(a - b + 0.5) - 0.5 }
#[inline]
pub fn sgn(x: f64) -> f64 { if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 } }

/// The seeded hashes and noise; `s` is `seed * 0x27d4eb2f`, the seed's share of every hash.
#[derive(Clone, Copy, Debug)]
pub struct Rng { s: u32 }

impl Rng {
    pub fn new(seed: u32) -> Self { Self { s: (seed as i32 as u32).wrapping_mul(0x27d4_eb2f) } }
    /// `hu(a K1 ^ (b K2 + c K3 + seed K4))`, the form `rnd` and `hcell` share.
    #[inline]
    fn hash3(&self, a: u32, b: u32, c: u32) -> u32 {
        hu(a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85eb_ca6b).wrapping_add(c.wrapping_mul(0xc2b2_ae35)).wrapping_add(self.s))
    }
    /// The page's `rnd(id, st, ly)`: 0..1 in steps of 2^-24.
    #[inline]
    pub fn rnd(&self, id: f64, st: f64, ly: f64) -> f64 {
        (self.hash3(i32_of(id).wrapping_add(4096) as u32, i32_of(st) as u32, i32_of(ly) as u32) >> 8) as f64 / 16_777_216.0
    }
    /// The city's lot hash `hcell` (the trees' `sgTg`, the town's `townHcell`) of integer cell (x, y), layer k.
    #[inline]
    pub fn hcell(&self, x: i32, y: i32, k: i32) -> u32 { self.hash3(x.wrapping_add(8192) as u32, y.wrapping_add(8192) as u32, k as u32) }
    /// The props' instance hash `pkHash(a, b)` (a sum, not a xor, of its terms).
    pub fn pk_hash(&self, a: f64, b: f64) -> u32 {
        hu((i32_of(a).wrapping_add(32768) as u32).wrapping_mul(0x9E37_79B1).wrapping_add((i32_of(b) as u32).wrapping_mul(0x85eb_ca6b)).wrapping_add(self.s))
    }
    /// The lattice points around `x` of a ring of `per` and the fraction: `pn` and `pl` share them.
    #[inline]
    fn cell(x: f64, per: f64) -> (f64, f64, f64) { let i = x.floor(); (i, if i + 1.0 >= per { i + 1.0 - per } else { i + 1.0 }, x - i) }
    /// Periodic value noise with a quintic fade (`pn`).
    #[inline]
    pub fn pn(&self, x: f64, per: f64, ly: f64) -> f64 {
        let (i, i1, f) = Self::cell(x, per);
        let a = self.rnd(i, 1.0, ly);
        a + (self.rnd(i1, 1.0, ly) - a) * (f * f * f * (f * (f * 6.0 - 15.0) + 10.0))
    }
    /// The same with a firmer fade (`pl`): 70% of the way from linear to smoothstep.
    #[inline]
    pub fn pl(&self, x: f64, per: f64, ly: f64) -> f64 {
        let (i, i1, f) = Self::cell(x, per);
        let a = self.rnd(i, 1.0, ly);
        a + (self.rnd(i1, 1.0, ly) - a) * (f + (f * f * (3.0 - 2.0 * f) - f) * 0.7)
    }
    /// Periodic fBm in 0..1, five octaves from the lattice period `base`.
    pub fn fbm(&self, u: f64, base: f64, ly: f64) -> f64 {
        let (mut s, mut a, mut per) = (0.0, 0.5, base);
        for k in 0..5 { s += a * self.pn(u * per, per, ly + k as f64 * 3.0); a *= 0.5; per *= 2.0; }
        s / 0.96875
    }
    /// Where on the circle a feature exists (`region`): `cov` of the turn around `c`, its edges
    /// wobbling by up to 0.07. Inside or outside the wobble band no noise is needed.
    pub fn region(&self, u: f64, c: f64, cov: f64, ly: f64) -> f64 {
        let (on, d, h) = (smooth(0.0, 0.02, cov), (fract(u - c + 0.5) - 0.5).abs(), cov * 0.5);
        if on <= 0.0 || (d >= h + 0.105 && cov < 0.999) { return 0.0; }
        if cov >= 0.999 || d <= h - 0.105 { return on; }
        smooth(h + 0.035, h - 0.035, d + (self.fbm(u, 6.0, ly) - 0.5) * 0.14) * on
    }
    /// 2D gradient noise of the tree crowns' rough edge (`sgTgn`), layer k.
    pub fn tgn(&self, x: f64, y: f64, k: i32) -> f64 {
        let (ix, iy) = (x.floor(), y.floor());
        let (fx, fy) = (x - ix, y - iy);
        let (wx, wy) = (fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0), fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0));
        let (ii, jj) = (ix as i64 as i32, iy as i64 as i32);
        const K: f64 = 2.0 / 65535.0;
        let g = |di: i32, dj: i32| {
            let h = self.hcell(ii.wrapping_add(di), jj.wrapping_add(dj), k);
            ((h & 65535) as f64 * K - 1.0) * (fx - di as f64) + ((h >> 16) as f64 * K - 1.0) * (fy - dj as f64)
        };
        let (a, b, c, d) = (g(0, 0), g(1, 0), g(0, 1), g(1, 1));
        let (ab, cd) = (a + (b - a) * wx, c + (d - c) * wx);
        ab + (cd - ab) * wy
    }
}
