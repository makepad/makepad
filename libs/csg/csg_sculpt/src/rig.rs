//! Instances and bone groups: a sculpted object posed and placed.
//!
//! An object is an ordered list of [`Part`]s, each a field in the bind
//! frame (a sculpt layer, an edit program) assigned to one bone. Posing moves
//! each part rigidly by its bone's transform; parts join by smooth union in
//! order, each with its own joint blend. An instance places the posed object
//! with a rigid transform and a uniform scale. Rigid motion and uniform scale
//! keep distances exact (d_world = scale * d_local), so the posed field stays
//! a safe distance and every part keeps an exact cull box. Per-sample skin
//! weights are a later extension (a weighted warp is not a distance field).
//!
//! The fold, banded like every engine field:
//!
//! ```text
//! d = band, m = 0
//! for part in parts:                      // in order
//!     x = (instance . pose[part.bone]).to_local(p)
//!     (s, ms) = part.field(x / scale)     // s *= scale
//!     m = ms if s < d else m              // the part wins where it is nearer
//!     d = clamp(smin(d, s, part.blend * scale), -band, band)
//! ```

use crate::math::*;
use crate::ops::smin;
use std::sync::Arc;

/// local -> world: `rot * (scale * x) + at`, rot orthonormal (rows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub rot: [V3; 3],
    pub at: V3,
    pub scale: f32,
}

impl Xform {
    pub const IDENT: Xform = Xform { rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], at: [0.0; 3], scale: 1.0 };

    pub fn new(rot: [V3; 3], at: V3, scale: f32) -> Xform {
        Xform { rot, at, scale }
    }

    pub fn rot_z(a: f32, at: V3) -> Xform {
        let (s, c) = a.sin_cos();
        Xform { rot: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], at, scale: 1.0 }
    }

    pub fn to_world(&self, x: V3) -> V3 {
        let r = self.rot;
        let y = scale(x, self.scale);
        add([dot(r[0], y), dot(r[1], y), dot(r[2], y)], self.at)
    }

    pub fn to_local(&self, p: V3) -> V3 {
        let r = self.rot;
        let q = sub(p, self.at);
        // rot^T q / scale
        let y = [
            r[0][0] * q[0] + r[1][0] * q[1] + r[2][0] * q[2],
            r[0][1] * q[0] + r[1][1] * q[1] + r[2][1] * q[2],
            r[0][2] * q[0] + r[1][2] * q[1] + r[2][2] * q[2],
        ];
        scale(y, 1.0 / self.scale)
    }

    /// `self` after `inner`: x -> self(inner(x)).
    pub fn then(&self, inner: &Xform) -> Xform {
        let r = self.rot;
        let ri = inner.rot;
        let mut rot = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                rot[i][j] = r[i][0] * ri[0][j] + r[i][1] * ri[1][j] + r[i][2] * ri[2][j];
            }
        }
        Xform { rot, at: self.to_world(inner.at), scale: self.scale * inner.scale }
    }

    pub fn bounds_to_world(&self, b: &Aabb) -> Aabb {
        if b.is_empty() {
            return *b;
        }
        b.corners().iter().fold(Aabb::EMPTY, |a, c| a.union(&Aabb::point(self.to_world(*c))))
    }
}

pub type Field = Arc<dyn Fn(V3) -> (f32, u8) + Send + Sync>;

/// One bone group of an object.
#[derive(Clone)]
pub struct Part {
    pub bone: u16,
    /// the smooth-union radius joining it to the parts before it, in the
    /// bind frame (it scales with the instance)
    pub blend: f32,
    /// distance and material in the bind frame
    pub field: Field,
    /// where the part's surface can be, in the bind frame
    pub bounds: Aabb,
}

#[derive(Clone, Default)]
pub struct Object {
    pub parts: Vec<Part>,
}

impl Object {
    /// The part's world cull box for this pose: outside it the part is
    /// exactly a no-op for the banded fold.
    pub fn part_bounds(&self, i: usize, pose: &[Xform], instance: &Xform, band: f32) -> Aabb {
        let part = &self.parts[i];
        let xf = instance.then(&pose[part.bone as usize]);
        let grow = part.blend.max(0.0) + band / xf.scale;
        xf.bounds_to_world(&part.bounds.expand(grow))
    }

    fn fold(&self, pose: &[Xform], instance: &Xform, p: V3, band: f32, cull: bool) -> (f32, u8) {
        let mut d = band;
        let mut m = 0u8;
        for (i, part) in self.parts.iter().enumerate() {
            if cull && !self.part_bounds(i, pose, instance, band).contains(p) {
                continue;
            }
            let xf = instance.then(&pose[part.bone as usize]);
            let (s, ms) = (part.field)(xf.to_local(p));
            let s = s * xf.scale;
            if s < d {
                m = ms;
            }
            d = smin(d, s, part.blend * xf.scale).clamp(-band, band);
        }
        (d, m)
    }

    /// The posed, placed field (distance, material).
    pub fn eval(&self, pose: &[Xform], instance: &Xform, p: V3, band: f32) -> (f32, u8) {
        self.fold(pose, instance, p, band, false)
    }

    /// The engine's evaluation: parts whose posed box misses `p` are skipped;
    /// bit-identical to [`Object::eval`].
    pub fn eval_culled(&self, pose: &[Xform], instance: &Xform, p: V3, band: f32) -> (f32, u8) {
        self.fold(pose, instance, p, band, true)
    }
}
