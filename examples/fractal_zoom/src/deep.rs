//! Deep zoom past f64: perturbation against one reference orbit.
//!
//! The view keeps its origin in double-double (about 32 digits) and every
//! tile in f64 offsets from it. For a deep layer the UI computes one
//! reference orbit Z in double-double at the layer's focus; each pixel then
//! only iterates its small difference dz from that orbit in f64 SIMD:
//!
//! ```text
//! Mandelbrot  dz' = 2 Z dz + dz^2 + dc      (dc: pixel c minus reference c)
//! Julia       dz' = 2 Z dz + dz^2           (dz starts at pixel z0 minus reference z0)
//! ```
//!
//! When |Z + dz| drops below |dz| (or the reference orbit ends) the pixel
//! rebases onto the start of the orbit, dz = Z + dz - Z0, which keeps it
//! accurate without glitch detection. Lanes rebase at different times, so
//! each lane has its own orbit index; Z is gathered per lane.

use crate::kernels::{encode, FractalKind, Lane, Params, TileRect};
use crate::simd::{F64x4, Wide};

/// A double-double: hi + lo with |lo| <= ulp(hi) / 2.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dd {
    pub hi: f64,
    pub lo: f64,
}

impl Dd {
    pub const ZERO: Dd = Dd { hi: 0.0, lo: 0.0 };

    pub fn new(v: f64) -> Dd {
        Dd { hi: v, lo: 0.0 }
    }

    #[inline(always)]
    fn two_sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        let bb = s - a;
        let e = (a - (s - bb)) + (b - bb);
        Dd { hi: s, lo: e }
    }

    #[inline(always)]
    fn quick(a: f64, b: f64) -> Dd {
        let s = a + b;
        Dd { hi: s, lo: b - (s - a) }
    }

    #[inline(always)]
    pub fn add(self, o: Dd) -> Dd {
        let s = Dd::two_sum(self.hi, o.hi);
        let t = Dd::two_sum(self.lo, o.lo);
        let s = Dd::quick(s.hi, s.lo + t.hi);
        Dd::quick(s.hi, s.lo + t.lo)
    }

    #[inline(always)]
    pub fn add_f64(self, o: f64) -> Dd {
        self.add(Dd::new(o))
    }

    #[inline(always)]
    pub fn neg(self) -> Dd {
        Dd { hi: -self.hi, lo: -self.lo }
    }

    #[inline(always)]
    pub fn sub(self, o: Dd) -> Dd {
        self.add(o.neg())
    }

    #[inline(always)]
    pub fn mul(self, o: Dd) -> Dd {
        let p = self.hi * o.hi;
        let e = self.hi.mul_add(o.hi, -p);
        let e = e + (self.hi * o.lo + self.lo * o.hi);
        Dd::quick(p, e)
    }

    pub fn to_f64(self) -> f64 {
        self.hi + self.lo
    }
}

/// The reference orbit of one deep layer.
pub struct DeepRef {
    /// The reference point as an offset from the view origin (f64 is exact
    /// enough: the reference sits inside the layer, near the view).
    pub at: (f64, f64),
    /// Z_0 .. Z_n rounded to f64 (Z_0 = 0 for Mandelbrot, the reference
    /// point for Julia).
    pub orbit: Vec<(f64, f64)>,
}

impl DeepRef {
    /// The orbit at `origin + at`, computed in double-double, until it
    /// escapes or runs `max_iter` steps.
    pub fn new(params: &Params, origin: (Dd, Dd), at: (f64, f64)) -> DeepRef {
        let pr = origin.0.add_f64(at.0);
        let pi = origin.1.add_f64(at.1);
        let (mut zr, mut zi, cr, ci) = match params.kind {
            FractalKind::Julia => (pr, pi, Dd::new(params.julia.0), Dd::new(params.julia.1)),
            _ => (Dd::ZERO, Dd::ZERO, pr, pi),
        };
        let mut orbit = Vec::with_capacity(params.max_iter as usize + 1);
        orbit.push((zr.to_f64(), zi.to_f64()));
        for _ in 0..params.max_iter {
            let r2 = zr.mul(zr);
            let i2 = zi.mul(zi);
            let ri = zr.mul(zi);
            zr = r2.sub(i2).add(cr);
            zi = ri.add(ri).add(ci);
            let (fr, fi) = (zr.to_f64(), zi.to_f64());
            orbit.push((fr, fi));
            if fr * fr + fi * fi > 1.0e8 {
                break;
            }
        }
        DeepRef { at, orbit }
    }

    /// Whether perturbation covers this fractal.
    pub fn supports(kind: FractalKind) -> bool {
        matches!(kind, FractalKind::Mandelbrot | FractalKind::Julia)
    }
}

const BAIL: f64 = 256.0;

/// Renders a tile (offsets from the origin) against the reference.
pub fn render_deep(params: &Params, deep: &DeepRef, rect: TileRect, res: usize, out: &mut [u32], stale: &dyn Fn() -> bool) -> bool {
    type W = F64x4;
    let step = rect.size / res as f64;
    let julia = params.kind == FractalKind::Julia;
    let mut lanes = [[0f64; 16]; 4];
    for row in 0..res {
        if stale() {
            return false;
        }
        let dy = rect.y + (row as f64 + 0.5) * step - deep.at.1;
        let line = &mut out[row * res..(row + 1) * res];
        let mut col = 0;
        while col < res {
            let dx = W::ramp(rect.x + (col as f64 + 0.5) * step - deep.at.0, step);
            let dy = W::splat(dy);
            let z0 = W::splat(0.0);
            let (n, mag, dz, trap) = if julia { perturb(params, deep, dx, dy, z0, z0, false) } else { perturb(params, deep, z0, z0, dx, dy, true) };
            n.store(&mut lanes[0]);
            mag.store(&mut lanes[1]);
            dz.store(&mut lanes[2]);
            trap.store(&mut lanes[3]);
            let take = W::LANES.min(res - col);
            for i in 0..take {
                let class = if lanes[0][i] >= params.max_iter as f64 { 1.0 } else { 0.0 };
                let lane = Lane { n: lanes[0][i], mag: lanes[1][i], class, dz: lanes[2][i], trap: lanes[3][i] };
                line[col + i] = encode(params, &lane, step);
            }
            col += W::LANES;
        }
    }
    true
}

/// Returns per lane the iteration count (max_iter: inside), |z|^2 at
/// escape, |dz/dc| (distance estimation, when on) and the orbit trap value.
#[inline(always)]
fn perturb<W: Wide>(p: &Params, deep: &DeepRef, mut dzr: W, mut dzi: W, dcr: W, dci: W, c_varies: bool) -> (W, W, W, W) {
    let max_iter = p.max_iter;
    let orbit = &deep.orbit;
    let last = orbit.len() - 1;
    let (z0r, z0i) = (W::splat(orbit[0].0), W::splat(orbit[0].1));
    let one = W::splat(1.0);
    let bail = W::splat(BAIL);
    let mut alive = W::splat(0.0).lt(one);
    let mut n = W::splat(0.0);
    let mut mag = W::splat(0.0);
    // |z| of the step before, for the derivative; z starts at Z0 + dz0
    let mut za = {
        let (r, i) = (z0r + dzr, z0i + dzi);
        (r * r + i * i).sqrt()
    };
    let mut deriv = if c_varies { W::splat(0.0) } else { one };
    let deriv_add = if c_varies { one } else { W::splat(0.0) };
    let mut trap = W::splat(1.0e9);
    let mut m = [0usize; 16];
    let mut gr = [0f64; 16];
    let mut gi = [0f64; 16];
    let mut alive_l = [0f64; 16];
    let mut rebase_l = [0f64; 16];
    for _ in 0..max_iter {
        if !alive.any() {
            break;
        }
        for i in 0..W::LANES {
            gr[i] = orbit[m[i]].0;
            gi[i] = orbit[m[i]].1;
        }
        let (zr_ref, zi_ref) = (W::load(&gr), W::load(&gi));
        // dz' = 2 Z dz + dz^2 + dc
        let two = W::splat(2.0);
        let nr = two * (zr_ref * dzr - zi_ref * dzi) + (dzr * dzr - dzi * dzi) + dcr;
        let ni = two * (zr_ref * dzi + zi_ref * dzr) + two * dzr * dzi + dci;
        dzr = W::select(alive, nr, dzr);
        dzi = W::select(alive, ni, dzi);
        if p.de {
            deriv = W::select(alive, two * za * deriv + deriv_add, deriv);
        }
        alive.store(&mut alive_l);
        for i in 0..W::LANES {
            if alive_l[i] != 0.0 {
                m[i] += 1;
            }
            gr[i] = orbit[m[i]].0;
            gi[i] = orbit[m[i]].1;
        }
        let (zr_ref, zi_ref) = (W::load(&gr), W::load(&gi));
        let zr = zr_ref + dzr;
        let zi = zi_ref + dzi;
        let new_mag = zr * zr + zi * zi;
        mag = W::select(alive, new_mag, mag);
        n = n + alive.and(one);
        alive = alive.and(new_mag.lt(bail));
        if p.de {
            za = new_mag.sqrt();
        }
        if p.trap {
            let (ar, ai) = (zr.abs(), zi.abs());
            let d = W::select(ar.lt(ai), ar, ai);
            trap = W::select(alive.and(d.lt(trap)), d, trap);
        }
        // rebase where z came closer to 0 than dz is, or the orbit ran out
        let dmag = dzr * dzr + dzi * dzi;
        let rebase = alive.and(new_mag.lt(dmag));
        rebase.store(&mut rebase_l);
        let mut any = false;
        for i in 0..W::LANES {
            if rebase_l[i] != 0.0 || (m[i] >= last && alive_l[i] != 0.0) {
                rebase_l[i] = f64::from_bits(u64::MAX);
                m[i] = 0;
                any = true;
            }
        }
        if any {
            let rb = W::load(&rebase_l);
            dzr = W::select(rb, zr - z0r, dzr);
            dzi = W::select(rb, zi - z0i, dzi);
        }
    }
    (n, mag, deriv, trap)
}
