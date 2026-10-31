//! Organic SDF edits: the reference semantics of the parametric ops the
//! language lowers into the brick engine's edit list.
//!
//! A [`Program`] is an ordered fold of [`Edit`]s over an accumulator
//! distance `d` (starting empty). An edit is either a shape folded in by a
//! union, difference or intersection with an optional blend radius, or a
//! modifier of the accumulator (shell, offset, bounded displacement). Shapes
//! are evaluated through a chain of domain warps (translate, rotate, twist,
//! bend, taper, mirror) and divided by the chain's Lipschitz bound, so every
//! shape term is a safe distance (|grad| <= 1) and the fold of them is too;
//! only displacement raises the bound, and [`Program::lipschitz`] reports it.
//!
//! Every edit has a world box ([`Edit::bounds`]) for a distance band: outside
//! it the edit leaves the accumulator's sign unchanged everywhere and its
//! value unchanged bit for bit wherever |d| <= band (the box is the shape's
//! grown by blend + band, in the shape's frame). The engine's field is the
//! banded fold ([`Program::eval_banded`]: the accumulator clamped to +-band
//! after each edit, as bricks store it), so the contract composes:
//! [`Program::eval_culled`] skips edits whose box misses the point and is
//! bit-identical to the banded fold everywhere.

use crate::math::*;

/// The accumulator's value before any shape: far outside.
pub const FAR: f32 = 1.0e20;

// ---------------------------------------------------------------------------
// Blends and falloff
// ---------------------------------------------------------------------------

/// Polynomial smooth minimum with blend radius `k`. Branches make the
/// no-overlap cases exact (`smin(a, b, k) == a` bit for bit when
/// `b >= a + k`): bounded influence is exact, not approximate, and the bake
/// can skip a sample without changing it.
#[inline]
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 || b - a >= k {
        return a.min(b);
    }
    if a - b >= k {
        return b;
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

#[inline]
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Chamfered minimum: a 45-degree bevel of width `k` along the seam,
/// `min(a, b, (a + b - k) / 2)`. Exactly `min(a, b)` where |a - b| >= k
/// (the bevel term is then above the smaller one), continuous, and
/// 1-Lipschitz like its inputs.
#[inline]
pub fn cmin(a: f32, b: f32, k: f32) -> f32 {
    let m = a.min(b);
    if k <= 0.0 || (a - b).abs() >= k {
        return m;
    }
    m.min((a + b - k) * 0.5)
}

#[inline]
pub fn cmax(a: f32, b: f32, k: f32) -> f32 {
    -cmin(-a, -b, k)
}

/// The brush falloff `(1 - t^2)^3` on 0..1, zero from 1 on: smooth at both
/// ends and compact, so a brush's influence ends exactly at its radius.
#[inline]
pub fn falloff(t: f32) -> f32 {
    if t >= 1.0 {
        return 0.0;
    }
    let u = 1.0 - t * t;
    u * u * u
}

/// max |falloff'(t)| = 96 / (25 sqrt 5), at t = 1/sqrt 5.
pub const FALLOFF_SLOPE: f32 = 1.717_300_6;

// ---------------------------------------------------------------------------
// Noise (the spec the GPU kernel copies: same hash, same fade, f32)
// ---------------------------------------------------------------------------

#[inline]
pub fn hash(x: u32) -> u32 {
    let s = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

#[inline]
fn lattice3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let h = hash(x as u32 ^ hash(y as u32 ^ hash(z as u32 ^ hash(seed))));
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise in -1..1.
pub fn noise3(p: V3, seed: u32) -> f32 {
    let f = [p[0].floor(), p[1].floor(), p[2].floor()];
    let i = [f[0] as i32, f[1] as i32, f[2] as i32];
    let u = [fade(p[0] - f[0]), fade(p[1] - f[1]), fade(p[2] - f[2])];
    let l = |dx: i32, dy: i32, dz: i32| lattice3(i[0] + dx, i[1] + dy, i[2] + dz, seed);
    let x00 = mix(l(0, 0, 0), l(1, 0, 0), u[0]);
    let x10 = mix(l(0, 1, 0), l(1, 1, 0), u[0]);
    let x01 = mix(l(0, 0, 1), l(1, 0, 1), u[0]);
    let x11 = mix(l(0, 1, 1), l(1, 1, 1), u[0]);
    let v = mix(mix(x00, x10, u[1]), mix(x01, x11, u[1]), u[2]);
    v * 2.0 - 1.0
}

/// Fractal value noise in -1..1 (`octaves` 1..8, gain 1/2, lacunarity 2).
pub fn fbm3(p: V3, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut q = p;
    for o in 0..octaves.clamp(1, 8) {
        sum += amp * noise3(q, seed.wrapping_add(o));
        norm += amp;
        amp *= 0.5;
        q = scale(q, 2.0);
    }
    sum / norm
}

/// An upper bound of |grad fbm3| per unit of p.
pub fn fbm3_slope(octaves: u32) -> f32 {
    // per octave: 2 (range) * 1.875 (max fade') * sqrt 3, times its frequency
    let mut s = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut freq = 1.0;
    for _ in 0..octaves.clamp(1, 8) {
        s += amp * freq * 2.0 * 1.875 * 1.732_051;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    s / norm
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// The distance to the round cone between spheres (a, ra) and (b, rb): one
/// tube segment. Exact; degenerates to the larger sphere when one sphere
/// contains the other.
pub fn sd_round_cone(p: V3, a: V3, ra: f32, b: V3, rb: f32) -> f32 {
    let ba = sub(b, a);
    let l2 = dot(ba, ba);
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    if a2 <= 1e-12 {
        let da = len(sub(p, a)) - ra;
        let db = len(sub(p, b)) - rb;
        return da.min(db);
    }
    let il2 = 1.0 / l2;
    let pa = sub(p, a);
    let y = dot(pa, ba);
    let z = y - l2;
    let xv = sub(scale(pa, l2), scale(ba, y));
    let x2 = dot(xv, xv);
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = rr.signum() * rr * rr * x2;
    if z.signum() * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - rb;
    }
    if y.signum() * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - ra;
    }
    ((x2 * a2 * il2).sqrt() + y * rr) * il2 - ra
}

pub fn sd_round_box(p: V3, half: V3, r: f32) -> f32 {
    let q = [p[0].abs() - half[0] + r, p[1].abs() - half[1] + r, p[2].abs() - half[2] + r];
    let o = len([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
    o + q[0].max(q[1].max(q[2])).min(0.0) - r
}

/// A stretched sphere scaled by the smallest radius: 1-Lipschitz and zero on
/// the surface, so it never overestimates the distance and underestimates
/// it at most by rmax / rmin ([`Shape::underestimate`]).
pub fn sd_ellipsoid(p: V3, r: V3) -> f32 {
    let rmin = r[0].min(r[1]).min(r[2]);
    (len([p[0] / r[0], p[1] / r[1], p[2] / r[2]]) - 1.0) * rmin
}

/// A shape in its own frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Sphere { r: f32 },
    /// A safe (under)estimate, off by at most the ratio of its radii.
    Ellipsoid { radii: V3 },
    RoundBox { half: V3, r: f32 },
    /// A chain of round cones through `(x y z r)` points: capsule chains,
    /// limbs, tentacles. [`Shape::tube`] builds one from a spline.
    Tube { pts: Vec<[f32; 4]> },
    /// Metaballs: spheres `(x y z r)` smooth-unioned in order with blend `k`.
    Blobs { balls: Vec<[f32; 4]>, k: f32 },
    /// A nested program (a compound operand: `Union{of: [..], blend}` of
    /// sub-solids, a mirrored half).
    Group(Box<Program>),
}

impl Shape {
    /// A tube through control points `(x y z r)`; `smooth` interpolates a
    /// uniform Catmull-Rom spline (position and radius) and samples it into
    /// round cones about half a radius long.
    pub fn tube(ctrl: &[[f32; 4]], smooth: bool) -> Shape {
        if !smooth || ctrl.len() < 3 {
            return Shape::Tube { pts: ctrl.to_vec() };
        }
        let n = ctrl.len();
        let at = |i: isize| ctrl[i.clamp(0, n as isize - 1) as usize];
        let mut pts = vec![ctrl[0]];
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
            let span = len(sub([p2[0], p2[1], p2[2]], [p1[0], p1[1], p1[2]]));
            let rmin = p1[3].min(p2[3]).max(1e-6);
            let steps = ((span / (0.5 * rmin)).ceil() as usize).clamp(1, 16);
            for s in 1..=steps {
                let t = s as f32 / steps as f32;
                let mut q = [0.0; 4];
                for c in 0..4 {
                    q[c] = catmull_rom(p0[c], p1[c], p2[c], p3[c], t);
                }
                q[3] = q[3].max(0.0);
                pts.push(q);
            }
        }
        Shape::Tube { pts }
    }

    pub fn distance(&self, p: V3) -> f32 {
        match self {
            Shape::Sphere { r } => len(p) - r,
            Shape::Ellipsoid { radii } => sd_ellipsoid(p, *radii),
            Shape::RoundBox { half, r } => sd_round_box(p, *half, *r),
            Shape::Tube { pts } => tube_distance(pts, p),
            Shape::Blobs { balls, k } => {
                let mut d = FAR;
                for b in balls {
                    d = smin(d, len(sub(p, [b[0], b[1], b[2]])) - b[3], *k);
                }
                d
            }
            Shape::Group(prog) => prog.eval(p),
        }
    }

    /// The shape's own Lipschitz bound (1 for exact and safe distances).
    pub fn lipschitz(&self) -> f32 {
        match self {
            Shape::Group(prog) => prog.lipschitz(),
            _ => 1.0,
        }
    }

    /// How much the distance can underestimate the true one (1: exact): a
    /// point `x` outside the box has distance >= x / underestimate.
    pub fn underestimate(&self) -> f32 {
        match self {
            Shape::Ellipsoid { radii } => {
                let mx = radii[0].max(radii[1]).max(radii[2]);
                let mn = radii[0].min(radii[1]).min(radii[2]);
                mx / mn.max(1e-9)
            }
            Shape::Group(prog) => prog
                .edits
                .iter()
                .map(|e| match e {
                    Edit::Shape { shape, warps, .. } => shape.underestimate() * Edit::chain_lipschitz(shape, warps),
                    _ => 1.0,
                })
                .fold(1.0, f32::max),
            _ => 1.0,
        }
    }

    /// The local box outside of which the distance is >= 0 (and grows).
    pub fn bounds(&self) -> Aabb {
        match self {
            Shape::Sphere { r } => Aabb::sphere([0.0; 3], *r),
            Shape::Ellipsoid { radii } => Aabb::new(scale(*radii, -1.0), *radii),
            Shape::RoundBox { half, .. } => Aabb::new(scale(*half, -1.0), *half),
            Shape::Tube { pts } => pts.iter().fold(Aabb::EMPTY, |b, q| b.union(&Aabb::sphere([q[0], q[1], q[2]], q[3]))),
            Shape::Blobs { balls, k } => balls
                .iter()
                .fold(Aabb::EMPTY, |b, q| b.union(&Aabb::sphere([q[0], q[1], q[2]], q[3])))
                .expand(*k),
            Shape::Group(prog) => prog.bounds(),
        }
    }
}

#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

pub fn tube_distance(pts: &[[f32; 4]], p: V3) -> f32 {
    match pts.len() {
        0 => FAR,
        1 => len(sub(p, [pts[0][0], pts[0][1], pts[0][2]])) - pts[0][3],
        _ => {
            let mut d = FAR;
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                d = d.min(sd_round_cone(p, [a[0], a[1], a[2]], a[3], [b[0], b[1], b[2]], b[3]));
            }
            d
        }
    }
}

// ---------------------------------------------------------------------------
// Domain warps
// ---------------------------------------------------------------------------

/// A map from the edit's input space (world, or the previous warp's output)
/// to the next. A chain is applied in order: `shape(wn(..w1(p)))`.
#[derive(Clone, Debug, PartialEq)]
pub enum Warp {
    /// p - at
    Translate(V3),
    /// rows: the rotation from input to output (orthonormal)
    Rotate([V3; 3]),
    /// p[axis] = at + |p[axis] - at|: the shape's half on the + side, mirrored
    Mirror { axis: usize, at: f32 },
    /// a twist about Y, `rate` radians per unit of height
    TwistY { rate: f32 },
    /// a bend of the XY plane, `rate` radians per unit along X
    BendX { rate: f32 },
    /// scale x, z by 1 + k y (clamped at 0.2) about the Y axis
    TaperY { k: f32 },
}

pub const TAPER_MIN: f32 = 0.2;

impl Warp {
    pub fn map(&self, p: V3) -> V3 {
        match *self {
            Warp::Translate(t) => sub(p, t),
            Warp::Rotate(r) => [dot(r[0], p), dot(r[1], p), dot(r[2], p)],
            Warp::Mirror { axis, at } => {
                let mut q = p;
                q[axis] = at + (p[axis] - at).abs();
                q
            }
            Warp::TwistY { rate } => {
                let (s, c) = (p[1] * rate).sin_cos();
                [c * p[0] - s * p[2], p[1], s * p[0] + c * p[2]]
            }
            Warp::BendX { rate } => {
                let (s, c) = (p[0] * rate).sin_cos();
                [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]]
            }
            Warp::TaperY { k } => {
                let s = (1.0 + k * p[1]).max(TAPER_MIN);
                [p[0] / s, p[1], p[2] / s]
            }
        }
    }

    /// An upper bound of |W(p) - W(q)| / |p - q| for p, q in `input`.
    pub fn lipschitz(&self, input: &Aabb) -> f32 {
        let rad = |a: usize, b: usize| {
            input.corners().iter().fold(0.0f32, |m, c| m.max((c[a] * c[a] + c[b] * c[b]).sqrt()))
        };
        match *self {
            Warp::Translate(_) | Warp::Rotate(_) | Warp::Mirror { .. } => 1.0,
            Warp::TwistY { rate } => 1.0 + rate.abs() * rad(0, 2),
            Warp::BendX { rate } => 1.0 + rate.abs() * rad(0, 1),
            Warp::TaperY { k } => {
                let smin = (1.0 + k * input.lo[1]).min(1.0 + k * input.hi[1]).max(TAPER_MIN);
                (1.0f32).max(1.0 / smin) + k.abs() * rad(0, 2) / (smin * smin)
            }
        }
    }

    /// The input-space box that maps into `out` (conservative).
    pub fn preimage(&self, out: &Aabb) -> Aabb {
        if out.is_empty() {
            return *out;
        }
        match *self {
            Warp::Translate(t) => out.translate(t),
            Warp::Rotate(r) => {
                // input = R^T output
                out.corners().iter().fold(Aabb::EMPTY, |b, c| {
                    let q = [
                        r[0][0] * c[0] + r[1][0] * c[1] + r[2][0] * c[2],
                        r[0][1] * c[0] + r[1][1] * c[1] + r[2][1] * c[2],
                        r[0][2] * c[0] + r[1][2] * c[1] + r[2][2] * c[2],
                    ];
                    b.union(&Aabb::point(q))
                })
            }
            Warp::Mirror { axis, at } => {
                let mut half = *out;
                half.lo[axis] = half.lo[axis].max(at);
                if half.is_empty() {
                    return Aabb::EMPTY;
                }
                let mut refl = half;
                refl.lo[axis] = 2.0 * at - half.hi[axis];
                refl.hi[axis] = 2.0 * at - half.lo[axis];
                half.union(&refl)
            }
            Warp::TwistY { .. } => {
                // rotation about Y preserves the radius in XZ
                let r = out.corners().iter().fold(0.0f32, |m, c| m.max((c[0] * c[0] + c[2] * c[2]).sqrt()));
                Aabb::new([-r, out.lo[1], -r], [r, out.hi[1], r])
            }
            Warp::BendX { .. } => {
                // rotation in XY preserves the radius in XY
                let r = out.corners().iter().fold(0.0f32, |m, c| m.max((c[0] * c[0] + c[1] * c[1]).sqrt()));
                Aabb::new([-r, -r, out.lo[2]], [r, r, out.hi[2]])
            }
            Warp::TaperY { k } => {
                let s0 = (1.0 + k * out.lo[1]).max(TAPER_MIN);
                let s1 = (1.0 + k * out.hi[1]).max(TAPER_MIN);
                let smax = s0.max(s1);
                let mut b = *out;
                for a in [0, 2] {
                    b.lo[a] = (out.lo[a] * s0).min(out.lo[a] * s1).min(out.lo[a] * smax);
                    b.hi[a] = (out.hi[a] * s0).max(out.hi[a] * s1).max(out.hi[a] * smax);
                }
                b
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Edits and programs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Union,
    Sub,
    /// keep only what is inside the shape (crop)
    Inter,
    /// the distance is untouched; the edit's material is written inside the
    /// shape (s < 0)
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellSide {
    /// the wall grows inward from the surface (the outside stays put)
    Inside,
    Outside,
    Center,
}

/// A modifier of the accumulator.
#[derive(Clone, Debug, PartialEq)]
pub enum Mod {
    /// a hollow wall `t` thick
    Shell { t: f32, side: ShellSide },
    /// grow (t > 0) or shrink the surface: thicken
    Offset { t: f32 },
    /// d + amp * fbm(p / scale) inside `region`, faded to 0 over `margin`
    Displace { amp: f32, scale: f32, octaves: u32, seed: u32, region: Aabb, margin: f32 },
}

/// An edit of the fold. A shape edit may carry a material (Fab: a filament
/// or colour slot; Sandbox: a material id): a union writes it where the shape
/// wins (its term below the accumulator); differences, intersections and
/// modifiers keep the accumulator's material. No material: a group's own
/// materials, else the accumulator's.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// `chamfer`: the blend is a 45-degree bevel ([`cmin`]) instead of round
    Shape { fold: Fold, blend: f32, shape: Shape, warps: Vec<Warp>, material: Option<u8>, chamfer: bool },
    Mod(Mod),
}

impl Edit {
    pub fn shape(fold: Fold, blend: f32, shape: Shape, warps: Vec<Warp>) -> Edit {
        Edit::Shape { fold, blend, shape, warps, material: None, chamfer: false }
    }

    pub fn chamfered(mut self) -> Edit {
        if let Edit::Shape { chamfer, .. } = &mut self {
            *chamfer = true;
        }
        self
    }

    pub fn with_material(mut self, m: u8) -> Edit {
        if let Edit::Shape { material, .. } = &mut self {
            *material = Some(m);
        }
        self
    }

    /// The shape term: its distance through the warps, divided by the
    /// chain's bound (a safe distance).
    fn term(shape: &Shape, warps: &[Warp], l: f32, p: V3) -> f32 {
        let mut q = p;
        for w in warps {
            q = w.map(q);
        }
        shape.distance(q) / l
    }

    /// The Lipschitz bound of the warp chain times the shape's.
    pub fn chain_lipschitz(shape: &Shape, warps: &[Warp]) -> f32 {
        // the input box of each warp: preimages from the shape backwards
        let mut boxes = vec![shape.bounds()];
        for w in warps.iter().rev() {
            let b = w.preimage(boxes.last().unwrap());
            boxes.push(b);
        }
        boxes.reverse(); // boxes[i] = input of warps[i]
        let mut l = shape.lipschitz();
        for (i, w) in warps.iter().enumerate() {
            l *= w.lipschitz(&boxes[i]);
        }
        l
    }

    /// The world box outside which this edit leaves the accumulator's sign
    /// unchanged and its value too where |d| <= band. `None`: global.
    pub fn bounds(&self, band: f32) -> Option<Aabb> {
        match self {
            Edit::Shape { fold: Fold::Inter, .. } => None,
            Edit::Shape { blend, shape, warps, .. } => {
                // Grow in the shape's frame by (blend + band) L: a local point
                // that far out has term >= blend + band, so for |d| <= band
                // the fold returns d exactly (smin's no-overlap branch); the
                // preimage keeps that (it is a superset).
                let l = Edit::chain_lipschitz(shape, warps) * shape.underestimate();
                let mut b = shape.bounds().expand((blend.max(0.0) + band.max(0.0)) * l);
                for w in warps.iter().rev() {
                    b = w.preimage(&b);
                }
                Some(b)
            }
            Edit::Mod(Mod::Displace { region, .. }) => Some(*region),
            Edit::Mod(_) => None,
        }
    }

    /// The factor this edit multiplies the accumulator's Lipschitz bound by.
    pub fn lipschitz(&self) -> f32 {
        match self {
            Edit::Mod(Mod::Displace { amp, scale, octaves, margin, .. }) => {
                let slope = amp.abs() * fbm3_slope(*octaves) / scale.max(1e-9);
                let mask = if *margin > 0.0 { amp.abs() / margin } else { 0.0 };
                1.0 + slope + mask
            }
            _ => 1.0,
        }
    }

    pub fn apply(&self, d: f32, p: V3) -> f32 {
        self.apply_material(d, 0, p).0
    }

    /// The fold step on distance and material.
    pub fn apply_material(&self, d: f32, m: u8, p: V3) -> (f32, u8) {
        match self {
            Edit::Shape { fold, blend, shape, warps, material, chamfer } => {
                let l = Edit::chain_lipschitz(shape, warps);
                let s = Edit::term(shape, warps, l, p);
                let (bmin, bmax): (fn(f32, f32, f32) -> f32, fn(f32, f32, f32) -> f32) = if *chamfer { (cmin, cmax) } else { (smin, smax) };
                match fold {
                    Fold::Union => {
                        let mat = if s < d {
                            match (material, shape) {
                                (Some(x), _) => *x,
                                (None, Shape::Group(g)) => {
                                    let mut q = p;
                                    for w in warps {
                                        q = w.map(q);
                                    }
                                    g.eval_material(q).1
                                }
                                (None, _) => m,
                            }
                        } else {
                            m
                        };
                        (bmin(d, s, *blend), mat)
                    }
                    Fold::Sub => (bmax(d, -s, *blend), m),
                    Fold::Inter => (bmax(d, s, *blend), m),
                    Fold::Paint => (d, if s < 0.0 { material.unwrap_or(m) } else { m }),
                }
            }
            _ => (self.apply_mod(d, p), m),
        }
    }

    fn apply_mod(&self, d: f32, p: V3) -> f32 {
        match self {
            Edit::Shape { .. } => unreachable!(),
            Edit::Mod(Mod::Shell { t, side }) => match side {
                ShellSide::Inside => d.max(-d - t),
                ShellSide::Outside => (d - t).max(-d),
                ShellSide::Center => d.abs() - t * 0.5,
            },
            Edit::Mod(Mod::Offset { t }) => d - t,
            Edit::Mod(Mod::Displace { amp, scale: sc, octaves, seed, region, margin }) => {
                let inside = -region.distance(p); // 0 on and inside the box
                if inside < 0.0 {
                    return d;
                }
                // fade in over `margin` inward from the box faces
                let edge = (0..3).fold(f32::MAX, |m, a| m.min(p[a] - region.lo[a]).min(region.hi[a] - p[a]));
                let w = if *margin > 0.0 { (edge / margin).clamp(0.0, 1.0) } else { 1.0 };
                if w <= 0.0 {
                    return d;
                }
                d + amp * w * fbm3(scale(p, 1.0 / sc), *octaves, *seed)
            }
        }
    }
}

/// An ordered fold of edits.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Program {
    pub edits: Vec<Edit>,
}

impl Program {
    pub fn new(edits: Vec<Edit>) -> Program {
        Program { edits }
    }

    /// The one fold: `band` clamps the accumulator after every edit (None:
    /// the exact fold), `cull` skips edits whose box misses `p`.
    fn fold(&self, p: V3, band: Option<f32>, cull: bool) -> (f32, u8) {
        let mut d = band.unwrap_or(FAR);
        let mut m = 0u8;
        for e in &self.edits {
            if cull {
                if let Some(b) = e.bounds(band.unwrap_or(0.0)) {
                    if !b.contains(p) {
                        continue;
                    }
                }
            }
            let (nd, nm) = e.apply_material(d, m, p);
            d = match band {
                Some(b) => nd.clamp(-b, b),
                None => nd,
            };
            m = nm;
        }
        (d, m)
    }

    pub fn eval(&self, p: V3) -> f32 {
        self.fold(p, None, false).0
    }

    /// Distance and material (0 where no edit set one).
    pub fn eval_material(&self, p: V3) -> (f32, u8) {
        self.fold(p, None, false)
    }

    /// The engine's field: the fold with the accumulator clamped to
    /// `+-band` after every edit (what distance bricks store). With
    /// band >= 2 x the largest blend it has the exact fold's sign, and its
    /// value wherever |d| < band - 2 blend.
    pub fn eval_banded(&self, p: V3, band: f32) -> f32 {
        self.fold(p, Some(band), false).0
    }

    pub fn eval_banded_material(&self, p: V3, band: f32) -> (f32, u8) {
        self.fold(p, Some(band), false)
    }

    /// The engine's evaluation: edits whose box (for `band`) misses `p` are
    /// skipped. Bit-identical to [`Program::eval_banded`] in distance and
    /// material: a skipped edit would have returned its (banded) input
    /// unchanged, and its term is above the input there, so it writes no
    /// material either.
    pub fn eval_culled(&self, p: V3, band: f32) -> f32 {
        self.fold(p, Some(band), true).0
    }

    pub fn eval_culled_material(&self, p: V3, band: f32) -> (f32, u8) {
        self.fold(p, Some(band), true)
    }

    /// The program's Lipschitz bound (the raymarch divides its step by it).
    pub fn lipschitz(&self) -> f32 {
        self.edits.iter().map(|e| e.lipschitz()).product()
    }

    /// Where the solid can be: the union of the union-fold boxes, grown by
    /// shells and offsets.
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for e in &self.edits {
            match e {
                Edit::Shape { fold: Fold::Union, .. } => b = b.union(&e.bounds(0.0).unwrap()),
                Edit::Mod(Mod::Offset { t }) if *t > 0.0 => b = b.expand(*t),
                Edit::Mod(Mod::Shell { t, side: ShellSide::Outside }) => b = b.expand(*t),
                Edit::Mod(Mod::Shell { t, side: ShellSide::Center }) => b = b.expand(t * 0.5),
                Edit::Mod(Mod::Displace { amp, .. }) => b = b.expand(amp.abs()),
                _ => {}
            }
        }
        b
    }
}
