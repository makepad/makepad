//! Sculpt strokes: the brush record that is the source of a sculpt layer.
//!
//! A stroke is `Stroke{mode radius blend strength path}` in the program,
//! the path a flat list of `x y z w` points (layer-frame millimetres and the
//! pressure 0..1, quantised by the recorder). It is applied to the layer's
//! distance grid ([`crate::layer::Layer`]) in program order. Every mode is a
//! function of the grid state before the stroke and is exactly a no-op
//! outside [`Stroke::bounds`], which is what makes re-baking bounded.
//!
//! | mode    | per grid point q (P = the grid before the stroke)                |
//! |---------|-------------------------------------------------------------------|
//! | add     | smin(P(q), S(q), blend); S = the tube along the path              |
//! | carve   | smax(P(q), -S(q), blend)                                          |
//! | inflate | P(q) - 0.5 radius W(q)                                            |
//! | flatten | mix(P(q), plane(q), W(q))                                         |
//! | smooth  | n Jacobi passes: mix(P, mean of the 6 neighbours, 0.5 W)          |
//! | pinch   | P(c + (q - c) (1 + 0.5 W(q))); c the nearest path point           |
//! | grab    | P pulled back along the drag path: q -= W_k(q) (path[k+1]-path[k]) |
//! | paint   | P unchanged; the material becomes the stroke's where W(q) > 0     |
//!
//! Materials (a u8 per grid sample): add writes the stroke's material where
//! its tube wins (S < P); paint writes it under the brush; pinch and grab
//! carry the material from where they read (the nearest sample); carve,
//! smooth, inflate and flatten keep it.
//!
//! W(q) = max over path segments of strength * w * falloff(dist / r), the
//! point's radius r = radius * (0.2 + 0.8 w): pressure widens the brush and
//! deepens it.

use crate::math::*;
use crate::ops::{cmax, cmin, falloff, smax, smin, FALLOFF_SLOPE};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Add,
    Carve,
    Smooth,
    Inflate,
    Flatten,
    Pinch,
    Grab,
    Paint,
}

impl Mode {
    /// Reads the grid away from the point it writes (needs the dilated
    /// neighbourhood when re-baking), and how far.
    pub fn reads_neighbours(self) -> bool {
        matches!(self, Mode::Smooth | Mode::Pinch | Mode::Grab)
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Add => "add",
            Mode::Carve => "carve",
            Mode::Smooth => "smooth",
            Mode::Inflate => "inflate",
            Mode::Flatten => "flatten",
            Mode::Pinch => "pinch",
            Mode::Grab => "grab",
            Mode::Paint => "paint",
        }
    }
}

pub const MIN_RADIUS_FRAC: f32 = 0.2;
/// Inflate pushes at most this fraction of the radius per stroke.
pub const INFLATE_DEPTH: f32 = 0.5;
pub const PINCH_DEPTH: f32 = 0.5;
pub const SMOOTH_MAX_PASSES: u32 = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub mode: Mode,
    pub radius: f32,
    pub blend: f32,
    pub strength: f32,
    /// x y z w per point (w = pressure 0..1)
    pub path: Vec<[f32; 4]>,
    /// flatten: the plane n.x n.y n.z d (n.q - d = 0), fixed at the stroke's start
    pub plane: [f32; 4],
    /// the material add and paint write
    pub material: u8,
    /// add / carve blend with a 45-degree bevel (cmin / cmax) instead of round
    pub chamfer: bool,
    /// the bone group the stroke belongs to (the lowering bakes one layer per
    /// bone; 0 = the root)
    pub bone: u16,
}

impl Stroke {
    pub fn new(mode: Mode, radius: f32, path: Vec<[f32; 4]>) -> Stroke {
        Stroke { mode, radius, blend: 0.0, strength: 1.0, path, plane: [0.0, 1.0, 0.0, 0.0], material: 0, chamfer: false, bone: 0 }
    }
    pub fn with_material(mut self, m: u8) -> Stroke {
        self.material = m;
        self
    }
    pub fn with_blend(mut self, b: f32) -> Stroke {
        self.blend = b;
        self
    }
    pub fn with_strength(mut self, s: f32) -> Stroke {
        self.strength = s;
        self
    }
    pub fn with_plane(mut self, n: V3, d: f32) -> Stroke {
        let n = normalize(n);
        self.plane = [n[0], n[1], n[2], d];
        self
    }

    #[inline]
    pub fn point_radius(&self, w: f32) -> f32 {
        self.radius * (MIN_RADIUS_FRAC + (1.0 - MIN_RADIUS_FRAC) * w.clamp(0.0, 1.0))
    }
    #[inline]
    fn pos(&self, i: usize) -> V3 {
        let q = self.path[i];
        [q[0], q[1], q[2]]
    }

    /// The tube along the path (add / carve).
    pub fn tube(&self, q: V3) -> f32 {
        let n = self.path.len();
        if n == 0 {
            return crate::ops::FAR;
        }
        if n == 1 {
            return len(sub(q, self.pos(0))) - self.point_radius(self.path[0][3]);
        }
        let mut d = crate::ops::FAR;
        for i in 0..n - 1 {
            let (a, b) = (self.path[i], self.path[i + 1]);
            d = d.min(crate::ops::sd_round_cone(q, self.pos(i), self.point_radius(a[3]), self.pos(i + 1), self.point_radius(b[3])));
        }
        d
    }

    /// The brush weight at q (0 outside the brush) and the nearest centre
    /// line point.
    pub fn weight(&self, q: V3) -> (f32, V3) {
        let n = self.path.len();
        let mut best = (0.0f32, q);
        if n == 0 {
            return best;
        }
        let one = |a: [f32; 4], b: [f32; 4]| -> (f32, V3) {
            let (pa, pb) = ([a[0], a[1], a[2]], [b[0], b[1], b[2]]);
            let ab = sub(pb, pa);
            let l2 = dot(ab, ab);
            let t = if l2 > 0.0 { (dot(sub(q, pa), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let c = add(pa, scale(ab, t));
            let w = mix(a[3], b[3], t);
            let r = self.point_radius(w);
            let wt = self.strength * w * falloff(len(sub(q, c)) / r);
            (wt, c)
        };
        if n == 1 {
            return one(self.path[0], self.path[0]);
        }
        for i in 0..n - 1 {
            let (wt, c) = one(self.path[i], self.path[i + 1]);
            if wt > best.0 {
                best = (wt, c);
            }
        }
        best
    }

    /// The largest point radius.
    pub fn max_radius(&self) -> f32 {
        self.path.iter().fold(0.0f32, |m, q| m.max(self.point_radius(q[3])))
    }

    /// The smooth passes this stroke runs on a grid of `cell`.
    pub fn smooth_passes(&self, cell: f32) -> u32 {
        (((self.radius / cell) * 0.5 * self.strength.clamp(0.0, 1.0)).ceil() as u32).clamp(1, SMOOTH_MAX_PASSES)
    }

    /// The box of grid points this stroke can change, given the layer's
    /// distance band (values are clamped to +-band, so a tube farther than
    /// blend + band from a point cannot reach it).
    pub fn bounds(&self, band: f32) -> Aabb {
        let pts = self.path.iter().fold(Aabb::EMPTY, |b, q| b.union(&Aabb::sphere([q[0], q[1], q[2]], self.point_radius(q[3]))));
        match self.mode {
            Mode::Add | Mode::Carve => pts.expand(self.blend.max(0.0) + band),
            _ => pts,
        }
    }

    /// How far from a written point the stroke reads the grid.
    pub fn reach(&self, cell: f32) -> f32 {
        match self.mode {
            Mode::Smooth => self.smooth_passes(cell) as f32 * cell,
            Mode::Pinch => PINCH_DEPTH * self.strength.max(0.0) * self.max_radius() + cell,
            Mode::Grab => {
                let mut l = 0.0;
                for w in self.path.windows(2) {
                    l += len(sub([w[1][0], w[1][1], w[1][2]], [w[0][0], w[0][1], w[0][2]]));
                }
                l * self.strength.clamp(0.0, 1.0) + cell
            }
            _ => 0.0,
        }
    }

    /// The factor this stroke can raise the grid's Lipschitz bound by.
    pub fn lipschitz(&self) -> f32 {
        let s = self.strength.clamp(0.0, 1.0);
        let r = (self.radius * MIN_RADIUS_FRAC).max(1e-9);
        match self.mode {
            Mode::Add | Mode::Carve | Mode::Smooth | Mode::Paint => 1.0,
            Mode::Inflate => 1.0 + INFLATE_DEPTH * self.radius * s * FALLOFF_SLOPE / r,
            Mode::Flatten => 1.0 + s * FALLOFF_SLOPE,
            Mode::Pinch => (1.0 + PINCH_DEPTH * s) * (1.0 + PINCH_DEPTH * s * FALLOFF_SLOPE),
            Mode::Grab => {
                let mut l = 1.0;
                for w in self.path.windows(2) {
                    let step = len(sub([w[1][0], w[1][1], w[1][2]], [w[0][0], w[0][1], w[0][2]]));
                    l *= 1.0 + s * step * FALLOFF_SLOPE / r;
                }
                l
            }
        }
    }

    /// The mirror image of the stroke across `axis = 0` (layer symmetry).
    pub fn mirrored(&self, axis: usize) -> Stroke {
        let mut s = self.clone();
        for q in &mut s.path {
            q[axis] = -q[axis];
        }
        s.plane[axis] = -s.plane[axis];
        s
    }

    /// Pointwise modes: the new value and material at grid point q from
    /// P(q) and its material.
    pub fn apply_sample(&self, q: V3, p: f32, m: u8) -> (f32, u8) {
        match self.mode {
            Mode::Add => {
                let t = self.tube(q);
                let v = if self.chamfer { cmin(p, t, self.blend) } else { smin(p, t, self.blend) };
                (v, if t < p { self.material } else { m })
            }
            Mode::Paint => (p, if self.weight(q).0 > 0.0 { self.material } else { m }),
            _ => (self.apply_point(q, p), m),
        }
    }

    /// Pointwise modes: the new value at grid point q from P(q).
    pub fn apply_point(&self, q: V3, p: f32) -> f32 {
        match self.mode {
            Mode::Add => if self.chamfer { cmin(p, self.tube(q), self.blend) } else { smin(p, self.tube(q), self.blend) },
            Mode::Paint => p,
            Mode::Carve => if self.chamfer { cmax(p, -self.tube(q), self.blend) } else { smax(p, -self.tube(q), self.blend) },
            Mode::Inflate => {
                let (w, _) = self.weight(q);
                if w <= 0.0 {
                    return p;
                }
                p - INFLATE_DEPTH * self.radius * w
            }
            Mode::Flatten => {
                let (w, _) = self.weight(q);
                if w <= 0.0 {
                    return p;
                }
                let pl = self.plane;
                let dp = pl[0] * q[0] + pl[1] * q[1] + pl[2] * q[2] - pl[3];
                mix(p, dp, w.min(1.0))
            }
            _ => unreachable!("{} reads neighbours", self.mode.name()),
        }
    }

    /// Warp modes: where to read the pre-stroke grid for grid point q
    /// (`None`: q is outside the brush, the value is unchanged).
    pub fn warp_source(&self, q: V3) -> Option<V3> {
        match self.mode {
            Mode::Pinch => {
                let (w, c) = self.weight(q);
                if w <= 0.0 {
                    return None;
                }
                Some(add(c, scale(sub(q, c), 1.0 + PINCH_DEPTH * w.min(1.0))))
            }
            Mode::Grab => {
                // pull q back through the drag, last step first: each step
                // moves space by its delta, weighted around the handle's
                // position after the step
                let s = self.strength.clamp(0.0, 1.0);
                let mut x = q;
                let mut moved = false;
                for k in (0..self.path.len().saturating_sub(1)).rev() {
                    let (a, b) = (self.path[k], self.path[k + 1]);
                    let delta = sub([b[0], b[1], b[2]], [a[0], a[1], a[2]]);
                    let r = self.point_radius(b[3]);
                    let w = falloff(len(sub(x, [b[0], b[1], b[2]])) / r) * s;
                    if w > 0.0 {
                        x = sub(x, scale(delta, w));
                        moved = true;
                    }
                }
                if moved {
                    Some(x)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
