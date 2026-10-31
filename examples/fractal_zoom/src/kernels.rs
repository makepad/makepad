//! The per-tile fractal kernels: one generic SIMD loop per fractal family,
//! instantiated for `F32x8` (shallow views, 8 pixels per step) and `F64x4`
//! (deep zoom, 4 pixels per step).
//!
//! A tile is a square of fractal space sampled at `res` x `res` texel
//! centres. Each texel gets a u32: the low 24 bits hold the smooth
//! iteration count in 1/64 steps, the top byte the class (0 escaped,
//! 1.. a Newton root, 255 inside the set). The shader turns that into a
//! colour through the palette, so palette edits never re-render a tile.

use crate::simd::{F32x8, F64x4, Wide};

pub const CLASS_INSIDE: u32 = 255;
/// The escape radius squared: large, so the smooth count is smooth.
const BAIL: f64 = 256.0;
/// Fixed-point steps per iteration in the stored value.
pub const VALUE_SCALE: f64 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FractalKind {
    Mandelbrot,
    Julia,
    BurningShip,
    Tricorn,
    Multibrot,
    Newton,
}

impl FractalKind {
    pub const ALL: [FractalKind; 6] = [
        FractalKind::Mandelbrot,
        FractalKind::Julia,
        FractalKind::BurningShip,
        FractalKind::Tricorn,
        FractalKind::Multibrot,
        FractalKind::Newton,
    ];

    /// Whether the power slider applies.
    pub fn uses_power(self) -> bool {
        matches!(self, FractalKind::Multibrot | FractalKind::Newton)
    }

    /// The view that frames the whole fractal: (center x, center y, width).
    pub fn home(self) -> (f64, f64, f64) {
        match self {
            FractalKind::Mandelbrot => (-0.6, 0.0, 3.4),
            FractalKind::Julia => (0.0, 0.0, 3.4),
            FractalKind::BurningShip => (-0.4, -0.5, 3.6),
            FractalKind::Tricorn => (-0.3, 0.0, 3.8),
            FractalKind::Multibrot => (0.0, 0.0, 3.2),
            FractalKind::Newton => (0.0, 0.0, 3.0),
        }
    }
}

/// Everything a tile depends on besides its place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub kind: FractalKind,
    pub power: u32,
    pub julia: (f64, f64),
    pub max_iter: u32,
    /// Distance estimation: the top byte of an escaped texel holds the
    /// distance to the set in texels (log scale, 1..254; 0: none).
    pub de: bool,
    /// Colour by the orbit's closest approach to the axes (a cross trap)
    /// instead of by the escape count.
    pub trap: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { kind: FractalKind::Mandelbrot, power: 3, julia: (-0.7269, 0.1889), max_iter: 500, de: true, trap: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Precision {
    F32,
    F64,
}

impl Precision {
    /// f32 holds while a texel step stays well above f32's resolution at
    /// the coordinates involved.
    pub fn for_step(step: f64, x: f64, y: f64) -> Precision {
        let mag = x.abs().max(y.abs()).max(1.0);
        if step > 4.0e-6 * mag {
            Precision::F32
        } else {
            Precision::F64
        }
    }
}

/// One tile's place in fractal space: its top-left corner and edge.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct TileRect {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// Renders a tile into `out` (`res * res` texels, row-major). `rect` is
/// an offset from `origin`; a deep layer renders against its reference orbit.
pub fn render_tile(
    params: &Params,
    origin: (f64, f64),
    rect: TileRect,
    res: usize,
    precision: Precision,
    deep: Option<&crate::deep::DeepRef>,
    out: &mut [u32],
    stale: &dyn Fn() -> bool,
) -> bool {
    if let Some(deep) = deep {
        return crate::deep::render_deep(params, deep, rect, res, out, stale);
    }
    let rect = TileRect { x: origin.0 + rect.x, y: origin.1 + rect.y, size: rect.size };
    match precision {
        Precision::F32 => render::<F32x8>(params, rect, res, out, stale),
        Precision::F64 => render::<F64x4>(params, rect, res, out, stale),
    }
}

/// Renders row by row; false when `stale` said the tile is no longer wanted
/// (checked per row, so a slow tile of a layer the view left frees its
/// worker at once).
fn render<W: Wide>(params: &Params, rect: TileRect, res: usize, out: &mut [u32], stale: &dyn Fn() -> bool) -> bool {
    let step = rect.size / res as f64;
    let mut lanes = [[0f64; 16]; 5];
    for row in 0..res {
        if stale() {
            return false;
        }
        let ci = rect.y + (row as f64 + 0.5) * step;
        let line = &mut out[row * res..(row + 1) * res];
        let mut col = 0;
        while col < res {
            // each texel centre as the node computes it (x0 + (col + 0.5) * step):
            // a ramp (base + i * step) rounds differently, and chaotic places
            // (Burning Ship) amplify that one ulp into a different texel
            let mut xs = [0f64; 16];
            for (i, x) in xs.iter_mut().enumerate().take(W::LANES) {
                *x = rect.x + ((col + i) as f64 + 0.5) * step;
            }
            let cr = W::load(&xs);
            let ci = W::splat(ci);
            let px = pixel::<W>(params, cr, ci);
            px.n.store(&mut lanes[0]);
            px.mag.store(&mut lanes[1]);
            px.class.store(&mut lanes[2]);
            px.dz.store(&mut lanes[3]);
            px.trap.store(&mut lanes[4]);
            let take = W::LANES.min(res - col);
            for i in 0..take {
                let lane = Lane { n: lanes[0][i], mag: lanes[1][i], class: lanes[2][i], dz: lanes[3][i], trap: lanes[4][i] };
                line[col + i] = encode(params, &lane, step);
            }
            col += W::LANES;
        }
    }
    true
}

/// The per-lane result of one pixel.
pub(crate) struct Px<W> {
    /// Steps taken (to escape, convergence or the cap).
    pub n: W,
    /// |z|^2 at escape (Newton: the last step's size squared).
    pub mag: W,
    /// 1.0 inside the set; Newton: the final angle of z (-10: no root).
    pub class: W,
    /// |dz/dc| at escape (distance estimation), 0 when off.
    pub dz: W,
    /// The orbit's closest approach to the axes (orbit trap).
    pub trap: W,
}

/// One pixel's numbers as scalars, for encoding.
pub(crate) struct Lane {
    pub n: f64,
    pub mag: f64,
    pub class: f64,
    pub dz: f64,
    pub trap: f64,
}

#[inline(always)]
fn pixel<W: Wide>(p: &Params, cr: W, ci: W) -> Px<W> {
    let z0 = W::splat(0.0);
    match p.kind {
        FractalKind::Mandelbrot => {
            // the main cardioid and the period-2 bulb are inside: skip them
            // (an orbit trap wants their orbits, so it iterates them)
            let q = (cr - W::splat(0.25)) * (cr - W::splat(0.25)) + ci * ci;
            let cardioid = (q * (q + (cr - W::splat(0.25)))).lt(W::splat(0.25) * ci * ci);
            let x1 = cr + W::splat(1.0);
            let bulb = (x1 * x1 + ci * ci).lt(W::splat(0.0625));
            let known = if p.trap { z0 } else { cardioid.or(bulb) };
            escape(p, z0, z0, cr, ci, 2, true, known, |zr, zi| (zr * zr - zi * zi, (zr + zr) * zi))
        }
        FractalKind::Julia => {
            let (kr, ki) = (W::splat(p.julia.0), W::splat(p.julia.1));
            escape(p, cr, ci, kr, ki, 2, false, z0, |zr, zi| (zr * zr - zi * zi, (zr + zr) * zi))
        }
        FractalKind::BurningShip => escape(p, z0, z0, cr, ci, 2, true, z0, |zr, zi| {
            let (a, b) = (zr.abs(), zi.abs());
            (a * a - b * b, (a + a) * b)
        }),
        FractalKind::Tricorn => escape(p, z0, z0, cr, ci, 2, true, z0, |zr, zi| (zr * zr - zi * zi, W::splat(-2.0) * zr * zi)),
        FractalKind::Multibrot => {
            let power = p.power.max(2);
            escape(p, z0, z0, cr, ci, power, true, z0, |zr, zi| {
                let (mut wr, mut wi) = (zr, zi);
                for _ in 1..power {
                    let r = wr * zr - wi * zi;
                    wi = wr * zi + wi * zr;
                    wr = r;
                }
                (wr, wi)
            })
        }
        FractalKind::Newton => newton(cr, ci, p.power.max(2), p.max_iter.min(1024)),
    }
}

/// Iterates `z = f(z) + c` until every lane escapes or `max_iter` steps.
/// Escaped lanes freeze their z, so the smooth count sees the escape value.
/// With distance estimation it carries |dz/dc| as the magnitude recurrence
/// |dz'| = p |z|^(p-1) |dz| + 1 (no +1 for Julia, which varies z0, dz0 = 1).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn escape<W: Wide>(
    p: &Params,
    mut zr: W,
    mut zi: W,
    cr: W,
    ci: W,
    power: u32,
    c_varies: bool,
    known_inside: W,
    f: impl Fn(W, W) -> (W, W),
) -> Px<W> {
    let one = W::splat(1.0);
    let bail = W::splat(BAIL);
    let all = W::splat(0.0).lt(one);
    let mut alive = known_inside.and_not(all);
    let mut n = W::splat(0.0);
    let mut mag = zr * zr + zi * zi;
    let mut dz = if c_varies { W::splat(0.0) } else { one };
    let dz_add = if c_varies { one } else { W::splat(0.0) };
    let pf = W::splat(power as f64);
    let mut trap = W::splat(1.0e9);
    for _ in 0..p.max_iter {
        if !alive.any() {
            break;
        }
        if p.de {
            let za = mag.sqrt();
            let mut zp = za;
            for _ in 2..power {
                zp = zp * za;
            }
            dz = W::select(alive, pf * zp * dz + dz_add, dz);
        }
        let (fr, fi) = f(zr, zi);
        zr = W::select(alive, fr + cr, zr);
        zi = W::select(alive, fi + ci, zi);
        mag = zr * zr + zi * zi;
        n = n + alive.and(one);
        alive = alive.and(mag.lt(bail));
        if p.trap {
            let (ar, ai) = (zr.abs(), zi.abs());
            let d = W::select(ar.lt(ai), ar, ai);
            trap = W::select(alive.and(d.lt(trap)), d, trap);
        }
    }
    let class = alive.or(known_inside).and(one);
    Px { n, mag, class, dz, trap }
}

/// Newton's method on z^p - 1: z <- ((p-1) z^p + 1) / (p z^(p-1)).
#[inline(always)]
fn newton<W: Wide>(mut zr: W, mut zi: W, power: u32, max_iter: u32) -> Px<W> {
    let one = W::splat(1.0);
    let pf = W::splat(power as f64);
    let pm1 = W::splat(power as f64 - 1.0);
    let eps = W::splat(1.0e-10);
    let all = W::splat(0.0).lt(one);
    let mut alive = all;
    let mut n = W::splat(0.0);
    let mut d = W::splat(1.0);
    for _ in 0..max_iter {
        if !alive.any() {
            break;
        }
        // w = z^(p-1)
        let (mut wr, mut wi) = (one, W::splat(0.0));
        for _ in 1..power {
            let r = wr * zr - wi * zi;
            wi = wr * zi + wi * zr;
            wr = r;
        }
        let zpr = wr * zr - wi * zi;
        let zpi = wr * zi + wi * zr;
        let (ar, ai) = (pm1 * zpr + one, pm1 * zpi);
        let (br, bi) = (pf * wr, pf * wi);
        let den = br * br + bi * bi;
        let nr = (ar * br + ai * bi) / den;
        let ni = (ai * br - ar * bi) / den;
        let (dr, di) = (nr - zr, ni - zi);
        let step = dr * dr + di * di;
        zr = W::select(alive, nr, zr);
        zi = W::select(alive, ni, zi);
        d = W::select(alive, step, d);
        n = n + alive.and(one);
        // NaN (z hit 0) compares false: such a lane stays alive and is inside
        alive = step.lt(eps).and_not(alive);
    }
    // class: the root's angle, decoded per lane (atan2 is scalar)
    let mut ang = [0f64; 16];
    let mut r = [0f64; 16];
    let mut im = [0f64; 16];
    zr.store(&mut r);
    zi.store(&mut im);
    let mut alive_l = [0f64; 16];
    alive.and(one).store(&mut alive_l);
    for i in 0..W::LANES {
        ang[i] = if alive_l[i] != 0.0 { -10.0 } else { crate::splash_math::atan2(im[i], r[i]) };
    }
    let class = W::load(&ang);
    Px { n, mag: d, class, dz: W::splat(0.0), trap: W::splat(1.0e9) }
}

/// The texel: smooth count (or trap value) in the low 24 bits, the class
/// or the distance code in the top byte. `step` is the texel's size in
/// fractal units (the distance code is in texels).
#[inline(always)]
pub(crate) fn encode(p: &Params, l: &Lane, step: f64) -> u32 {
    use crate::splash_math::{ln, log2};
    // the node kernels' low24: floor(v * 64) clamped to 0..2^24-1
    let low = |v: f64| (v * VALUE_SCALE).clamp(0.0, 16777215.0).floor() as u32;
    if p.kind == FractalKind::Newton {
        if l.class < -5.0 {
            return CLASS_INSIDE << 24;
        }
        let k = ((l.class / std::f64::consts::TAU * p.power as f64 + 0.5).floor() as i64).rem_euclid(p.power as i64) as u32;
        // quadratic convergence: ln(step) doubles per step, so this fraction runs 0..1
        let frac = if l.mag > 0.0 { (log2((ln(l.mag) / ln(1.0e-10)).max(1.0)) as f64).min(1.0) } else { 1.0 };
        return ((k + 1) << 24) | low(l.n - frac);
    }
    // Splash's log2 is f32 (widened back for the f64 arithmetic around it)
    let trap_value = || (1.0 - log2(l.trap.max(1.0e-30)) as f64) * 8.0;
    if l.class > 0.5 {
        return (CLASS_INSIDE << 24) | if p.trap { low(trap_value()) } else { 0 };
    }
    let power = if p.kind == FractalKind::Multibrot { p.power.max(2) as f64 } else { 2.0 };
    let nu = if l.mag > 1.0 { l.n + 1.0 - ln(0.5 * ln(l.mag)) / ln(power) } else { l.n };
    let value = if p.trap { low(trap_value()) } else { low(nu) };
    if p.de && l.dz > 0.0 && l.mag > 1.0 {
        let za = l.mag.sqrt();
        let dist = za * ln(za) / l.dz / step;
        let code = (log2(dist.max(1.0e-9)) as f64 * 20.0 + 100.0).round().clamp(1.0, 254.0) as u32;
        return (code << 24) | value;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without distance codes, so the top byte is the class alone.
    fn base() -> Params {
        Params { de: false, ..Params::default() }
    }

    fn value_at(params: &Params, x: f64, y: f64, precision: Precision) -> u32 {
        let mut out = vec![0u32; 8 * 8];
        render_tile(params, (0.0, 0.0), TileRect { x: x - 0.5e-3, y: y - 0.5e-3, size: 1.0e-3 }, 8, precision, None, &mut out, &|| false);
        out[4 * 8 + 4]
    }

    #[test]
    fn inside_and_outside() {
        let p = base();
        for prec in [Precision::F32, Precision::F64] {
            assert_eq!(value_at(&p, -0.1, 0.0, prec) >> 24, CLASS_INSIDE);
            assert_eq!(value_at(&p, 1.0, 1.0, prec) >> 24, 0);
            // a point outside the cardioid shortcut that is still inside
            assert_eq!(value_at(&p, -1.3, 0.0, prec) >> 24, CLASS_INSIDE);
        }
    }

    #[test]
    fn f32_and_f64_agree() {
        for kind in FractalKind::ALL {
            let p = Params { kind, ..base() };
            let rect = TileRect { x: -1.5, y: -1.0, size: 2.0 };
            let mut a = vec![0u32; 32 * 32];
            let mut b = vec![0u32; 32 * 32];
            render_tile(&p, (0.0, 0.0), rect, 32, Precision::F32, None, &mut a, &|| false);
            render_tile(&p, (0.0, 0.0), rect, 32, Precision::F64, None, &mut b, &|| false);
            let same = a.iter().zip(&b).filter(|(x, y)| (*x >> 24) == (*y >> 24)).count();
            assert!(same > 32 * 32 * 95 / 100, "{kind:?}: {same}");
        }
    }

    #[test]
    fn perturbation_matches_direct() {
        use crate::deep::{DeepRef, Dd};
        for kind in [FractalKind::Mandelbrot, FractalKind::Julia] {
            let p = Params { kind, max_iter: 800, ..base() };
            let origin = (-0.743_643_887_037_151, 0.131_825_904_205_330);
            let rect = TileRect { x: -2.0e-7, y: -2.0e-7, size: 4.0e-7 };
            let mut a = vec![0u32; 32 * 32];
            let mut b = vec![0u32; 32 * 32];
            render_tile(&p, origin, rect, 32, Precision::F64, None, &mut a, &|| false);
            let deep = DeepRef::new(&p, (Dd::new(origin.0), Dd::new(origin.1)), (1.0e-8, -3.0e-8));
            render_tile(&p, origin, rect, 32, Precision::F64, Some(&deep), &mut b, &|| false);
            let close = a
                .iter()
                .zip(&b)
                .filter(|(x, y)| (*x >> 24) == (*y >> 24) && ((**x & 0xff_ffff) as i64 - (**y & 0xff_ffff) as i64).abs() < 64)
                .count();
            assert!(close > 32 * 32 * 97 / 100, "{kind:?}: {close}");
        }
    }

    #[test]
    fn perturbation_matches_double_double_deep() {
        use crate::deep::{DeepRef, Dd};
        let p = Params { max_iter: 2000, ..base() };
        let origin = (Dd::new(0.0), Dd::new(1.0));
        let size = 1.0e-22;
        let rect = TileRect { x: -0.5 * size, y: -0.5 * size, size };
        let res = 16;
        let mut b = vec![0u32; res * res];
        let deep = DeepRef::new(&p, origin, (0.0, 0.0));
        render_tile(&p, (0.0, 1.0), rect, res, Precision::F64, Some(&deep), &mut b, &|| false);
        let mut agree = 0;
        for row in 0..res {
            for col in 0..res {
                let dx = rect.x + (col as f64 + 0.5) * size / res as f64;
                let dy = rect.y + (row as f64 + 0.5) * size / res as f64;
                let (cr, ci) = (origin.0.add_f64(dx), origin.1.add_f64(dy));
                let (mut zr, mut zi) = (Dd::ZERO, Dd::ZERO);
                let mut n = p.max_iter;
                for k in 0..p.max_iter {
                    let r2 = zr.mul(zr);
                    let i2 = zi.mul(zi);
                    let ri = zr.mul(zi);
                    zr = r2.sub(i2).add(cr);
                    zi = ri.add(ri).add(ci);
                    if zr.hi * zr.hi + zi.hi * zi.hi >= 256.0 {
                        n = k + 1;
                        break;
                    }
                }
                let v = b[row * res + col];
                let pn = if v >> 24 == CLASS_INSIDE { p.max_iter } else { ((v & 0xff_ffff) as f64 / VALUE_SCALE).floor() as u32 };
                // the smooth value is n + 1 - frac: within one of the step count
                if (pn as i64 - n as i64).abs() <= 2 {
                    agree += 1;
                } else if agree < 8 {
                    eprintln!("pixel {col},{row}: perturbation {pn} direct {n}");
                }
            }
        }
        assert!(agree >= res * res * 95 / 100, "{agree}/{}", res * res);
    }

    #[test]
    fn newton_basins_have_detail() {
        let p = Params { kind: FractalKind::Newton, power: 3, ..base() };
        let mut out = vec![0u32; 64 * 64];
        render_tile(&p, (0.0, 0.0), TileRect { x: -1.0, y: -1.0, size: 2.0 }, 64, Precision::F64, None, &mut out, &|| false);
        let mut classes = std::collections::BTreeSet::new();
        let mut counts = std::collections::BTreeSet::new();
        for v in &out {
            classes.insert(v >> 24);
            counts.insert((v & 0xff_ffff) / 64);
        }
        assert!(classes.contains(&1) && classes.contains(&2) && classes.contains(&3), "{classes:?}");
        assert!(counts.len() > 8, "{counts:?}");
    }

    #[test]
    fn newton_finds_roots() {
        let p = Params { kind: FractalKind::Newton, power: 3, ..base() };
        // near 1 converges to root 0 (class 1)
        assert_eq!(value_at(&p, 0.9, 0.05, Precision::F64) >> 24, 1);
    }
}
