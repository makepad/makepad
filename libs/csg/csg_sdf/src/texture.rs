//! Surface textures: a field displaced by a pattern ([`SdfNode::Texture`]).
//!
//! `f(p) = (g(p) + depth · M(G) · S(p)) / lipschitz`:
//!
//! - `g` is the source's field and `S` the pattern through its mapping, a compiled Splash
//!   kernel with values in [0, 1] (0 leaves the surface, 1 is the full depth; a positive depth
//!   cuts in).
//! - `G` is the source's gradient smoothed over `step`: a tetrahedral difference, taken only
//!   when a mask or the triplanar blend needs it. `M` holds the top and bottom angle masks,
//!   smoothstep ramps on `G.y`.
//! - `lipschitz` bounds the displaced field's slope, derived from the parameters
//!   ([`SdfTexture::new`]), so `f` stays 1-Lipschitz like every other node. Its zero set is
//!   the displaced surface exactly, and everything that steps by the value (a brick's
//!   classification, a march, a layer trace's crossing search) needs nothing more.
//!
//! The SDF engine's program runs the same steps in f32 (its TEX op), bit for bit with its CPU
//! twin.

use crate::shape::{SdfKernel, SdfNode, P3};
use makepad_csg_math::Vec3d;

/// How the pattern kernel is called at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexProject {
    /// One call, the kernel's Y along this axis (0 x, 1 y, 2 z).
    Axis(u8),
    /// One call per axis, blended by the smoothed normal (`G_i² / max(|G|², 1)`).
    Triplanar,
}

/// The tetrahedron the gradient is sampled on (times `step`).
pub const TETRA: [[f64; 3]; 4] = [[1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, 1.0]];

/// A texture over a source field.
#[derive(Clone, Debug)]
pub struct SdfTexture {
    /// `S(q)`: the pattern through its mapping, in [0, 1], at `q` = the swizzled point times
    /// `kscale`.
    pub kernel: SdfKernel,
    pub project: TexProject,
    /// Kernel units per node unit (a kernel reads metres, a part is in millimetres).
    pub kscale: f64,
    /// The displacement at T = 1: positive cuts into the source, negative raises it.
    pub depth: f64,
    /// The bound the node divides by.
    pub lipschitz: f64,
    /// The gradient's step: the normal is smoothed over it, so a masked texture fades out over
    /// about this much at a rim.
    pub step: f64,
    /// The angle masks on `G.y` (`-G.y` for the bottom): the texture is full at or below the
    /// first cosine and gone at or above the second. `None`: off.
    pub top: Option<(f64, f64)>,
    pub bottom: Option<(f64, f64)>,
    /// The narrowest feature the pattern prints (mm in the node's units): what a printer's
    /// line width is checked against. 0: not known.
    pub feature: f64,
}

/// A mask ramp: 1 at or below `lo`, 0 at or above `hi`, a smoothstep between (no kink at
/// either end for a layer's outline to turn a corner at; 1.5 times the straight ramp's slope).
pub fn ramp(m: Option<(f64, f64)>, x: f64) -> f64 {
    match m {
        None => 1.0,
        Some((lo, hi)) => {
            let s = ((hi - x) / (hi - lo)).clamp(0.0, 1.0);
            s * s * (3.0 - 2.0 * s)
        }
    }
}

/// The ramp of a mask that removes the texture where the normal is within `angle` of the
/// axis, with the full texture back `blend` further (radians). `None` for a zero angle.
pub fn angle_mask(angle: f64, blend: f64) -> Option<(f64, f64)> {
    (angle > 0.0).then(|| ((angle + blend.max(1e-3)).min(std::f64::consts::PI).cos(), angle.cos()))
}

impl SdfTexture {
    /// A texture whose pattern through its mapping is at most `pattern_slope` steep (per node
    /// unit, the mapping's stretch and fades included); the bound adds the triplanar blend's
    /// and the masks' terms (both through the smoothed gradient, `1/step` steep at worst).
    #[allow(clippy::too_many_arguments)]
    pub fn new(kernel: SdfKernel, project: TexProject, kscale: f64, depth: f64, step: f64, top: Option<(f64, f64)>, bottom: Option<(f64, f64)>, pattern_slope: f64, feature: f64) -> SdfTexture {
        let step = step.max(1e-6);
        let mut slope = pattern_slope;
        if project == TexProject::Triplanar {
            // sum over the axes of |d w_i / dG| (2 |G_i|, at most 2 sqrt 3) times |dG/dp| (sqrt 3 / step)
            slope += 6.0 / step;
        }
        let mask = |m: Option<(f64, f64)>| m.map_or(0.0, |(lo, hi)| 1.5 / ((hi - lo).max(1e-9) * step));
        // The ramps act on disjoint ranges of G.y unless they overlap (angle + blend past 90).
        let overlap = matches!((top, bottom), (Some(t), Some(b)) if t.0 + b.0 < 0.0);
        slope += if overlap { mask(top) + mask(bottom) } else { mask(top).max(mask(bottom)) };
        let lipschitz = 1.0 + depth.abs() * slope;
        SdfTexture { kernel, project, kscale, depth, lipschitz, step, top, bottom, feature }
    }

    /// Whether the smoothed gradient is taken (four more evaluations of the source).
    pub fn needs_gradient(&self) -> bool {
        self.top.is_some() || self.bottom.is_some() || self.project == TexProject::Triplanar
    }

    /// How far the source's values must be exact around a region for the texture's to be
    /// there: the displacement and the gradient's samples.
    pub fn reach(&self) -> f64 {
        self.depth.abs() + if self.needs_gradient() { 2.0 * 3f64.sqrt() * self.step } else { 0.0 }
    }

    /// The kernel's point for projection `axis` (its Y along that axis).
    pub fn swizzle(axis: u8, p: Vec3d) -> Vec3d {
        match axis {
            0 => Vec3d::new(p.z, p.x, p.y),
            2 => Vec3d::new(p.x, p.z, p.y),
            _ => p,
        }
    }

    /// The smoothed gradient from the source's values at the tetrahedron's corners.
    pub fn gradient(&self, g: [f64; 4]) -> [f64; 3] {
        let k = 0.25 / self.step;
        std::array::from_fn(|i| (TETRA[0][i] * g[0] + TETRA[1][i] * g[1] + TETRA[2][i] * g[2] + TETRA[3][i] * g[3]) * k)
    }

    /// The masks at a smoothed gradient.
    pub fn mask(&self, grad: [f64; 3]) -> f64 {
        ramp(self.top, grad[1]) * ramp(self.bottom, -grad[1])
    }

    /// The triplanar weights at a smoothed gradient.
    pub fn weights(grad: [f64; 3]) -> [f64; 3] {
        let n = (grad[0] * grad[0] + grad[1] * grad[1] + grad[2] * grad[2]).max(1.0);
        grad.map(|g| g * g / n)
    }

    /// The textured value from the source's value `d` at the point, its smoothed gradient
    /// and the pattern's values per projection (`s[0]` for a single axis).
    pub fn combine(&self, d: f64, grad: [f64; 3], s: [f64; 3]) -> f64 {
        let m = self.mask(grad);
        let t = match self.project {
            TexProject::Axis(_) => s[0],
            TexProject::Triplanar => {
                let w = Self::weights(grad);
                w[0] * s[0] + w[1] * s[1] + w[2] * s[2]
            }
        };
        (d + self.depth * m * t) / self.lipschitz
    }

    fn pattern(&self, axis: u8, p: Vec3d) -> f64 {
        self.kernel.field.distance(Self::swizzle(axis, p) * self.kscale)
    }

    /// The textured field at `p` of a source field `g`.
    pub fn eval(&self, p: Vec3d, g: impl Fn(Vec3d) -> f64) -> f64 {
        let d = g(p);
        let grad = if self.needs_gradient() {
            self.gradient(TETRA.map(|k| g(p + Vec3d::new(k[0], k[1], k[2]) * self.step)))
        } else {
            [0.0; 3]
        };
        let s = match self.project {
            TexProject::Axis(a) => [self.pattern(a, p), 0.0, 0.0],
            TexProject::Triplanar => std::array::from_fn(|a| self.pattern(a as u8, p)),
        };
        self.combine(d, grad, s)
    }

    /// [`SdfTexture::eval`] at many points: the source and each projection in one batch.
    pub fn eval_many(&self, ps: &[Vec3d], out: &mut [f64], g: impl Fn(&[Vec3d], &mut [f64])) {
        let n = ps.len();
        let corners = if self.needs_gradient() { 4 } else { 0 };
        let mut qs = Vec::with_capacity(n * (1 + corners));
        qs.extend_from_slice(ps);
        for k in TETRA.iter().take(corners) {
            let o = Vec3d::new(k[0], k[1], k[2]) * self.step;
            qs.extend(ps.iter().map(|p| *p + o));
        }
        let mut gs = vec![0.0; qs.len()];
        g(&qs, &mut gs);
        let axes: Vec<u8> = match self.project {
            TexProject::Axis(a) => vec![a],
            TexProject::Triplanar => vec![0, 1, 2],
        };
        let mut s = vec![0.0; n * axes.len()];
        for (k, a) in axes.iter().enumerate() {
            let q: Vec<Vec3d> = ps.iter().map(|p| Self::swizzle(*a, *p) * self.kscale).collect();
            self.kernel.field.distance_many(&q, &mut s[k * n..(k + 1) * n]);
        }
        for i in 0..n {
            let grad = if corners > 0 { self.gradient(std::array::from_fn(|k| gs[(k + 1) * n + i])) } else { [0.0; 3] };
            let si = std::array::from_fn(|k| if k < axes.len() { s[k * n + i] } else { 0.0 });
            out[i] = self.combine(gs[i], grad, si);
        }
    }

    /// Equal textures (the kernel by its key).
    pub fn same(&self, o: &SdfTexture) -> bool {
        self.kernel.key == o.kernel.key
            && self.project == o.project
            && self.kscale == o.kscale
            && self.depth == o.depth
            && self.lipschitz == o.lipschitz
            && self.step == o.step
            && self.top == o.top
            && self.bottom == o.bottom
    }
}

/// Every texture of a node with its source's box, outermost first: what a printability check
/// reads (the narrowest feature, the depth).
pub fn textures(n: &SdfNode, out: &mut Vec<std::sync::Arc<SdfTexture>>) {
    match n {
        SdfNode::Texture { tex, of } => {
            out.push(tex.clone());
            textures(of, out);
        }
        SdfNode::Combine { of, .. } => of.iter().for_each(|c| textures(c, out)),
        SdfNode::Material { within, of, .. } => {
            if let Some(w) = within {
                textures(w, out);
            }
            textures(of, out);
        }
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } | SdfNode::Sculpt { of, .. } => textures(of, out),
        _ => {}
    }
}

/// The box a texture's solid fits in, from its source's.
pub fn texture_bounds(tex: &SdfTexture, of: (P3, P3)) -> (P3, P3) {
    let r = (-tex.depth).max(0.0);
    (of.0.map(|x| x - r), of.1.map(|x| x + r))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The masks ramp from full to none across the blend, and the bound adds each term.
    #[test]
    fn masks_and_bound() {
        let m = angle_mask(20f64.to_radians(), 15f64.to_radians()).unwrap();
        assert_eq!(m, (35f64.to_radians().cos(), 20f64.to_radians().cos()));
        assert_eq!((ramp(Some(m), 0.0), ramp(Some(m), 1.0), ramp(None, 1.0)), (1.0, 0.0, 1.0));
        assert!((ramp(Some(m), 0.5 * (m.0 + m.1)) - 0.5).abs() < 1e-12);
        assert_eq!(angle_mask(0.0, 0.3), None);
        let k = || SdfKernel { key: 0, field: std::sync::Arc::new(crate::primitives::SdfSphere::new(Vec3d::new(0.0, 0.0, 0.0), 1.0)), splash: None, bounds: None };
        let t = SdfTexture::new(k(), TexProject::Axis(1), 1.0, 0.5, 2.0, None, None, 3.0, 0.0);
        assert_eq!((t.lipschitz, t.needs_gradient(), t.reach()), (2.5, false, 0.5));
        let t = SdfTexture::new(k(), TexProject::Triplanar, 1.0, -0.5, 2.0, Some(m), Some(m), 3.0, 0.0);
        let mask = 1.5 / ((m.1 - m.0) * 2.0);
        assert!((t.lipschitz - (1.0 + 0.5 * (3.0 + 3.0 + mask))).abs() < 1e-12);
        // Ramps that overlap (both reach past the side faces) add up.
        let wide = angle_mask(80f64.to_radians(), 20f64.to_radians()).unwrap();
        let t = SdfTexture::new(k(), TexProject::Axis(1), 1.0, 1.0, 1.0, Some(wide), Some(wide), 0.0, 0.0);
        assert!((t.lipschitz - (1.0 + 3.0 / (wide.1 - wide.0))).abs() < 1e-12);
        assert_eq!(texture_bounds(&t, ([0.0; 3], [1.0; 3])), ([0.0; 3], [1.0; 3]));
    }
}
