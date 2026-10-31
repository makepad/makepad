//! A field's measured Lipschitz bound: how much faster than distance its value changes.
//!
//! A Splash kernel (`m.sdf`) may be any function, not a distance: a gyroid's `|g| / k`
//! climbs up to about 1.9 times as fast as the distance to its sheet. Everything that steps
//! by the value (the mesher's culling, the slicer's coarse search, a march) divides by this
//! bound so it never steps past a surface. Measured like the SDF engine's kernel scale: the
//! steepest forward-difference gradient on a lattice over the bounds, near the surface.

use crate::sdf::Sdf3;
use crate::shape::{SdfNode, SdfShape};
use makepad_csg_math::Vec3d;
use std::sync::Arc;

/// Lattice cells per side.
const N: usize = 24;

/// The steepest gradient of `field` over `min..max` near its surface (within two lattice
/// cells of it), at least 1, with a tenth to spare.
pub fn measure(field: &Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d) -> f64 {
    let size = max - min;
    let step = size.x.max(size.y).max(size.z).max(1e-9) / N as f64;
    let h = step / 16.0;
    let mut ps = Vec::with_capacity((N + 1).pow(3) * 4);
    for k in 0..=N {
        for j in 0..=N {
            for i in 0..=N {
                let p = Vec3d::new(min.x + size.x * i as f64 / N as f64, min.y + size.y * j as f64 / N as f64, min.z + size.z * k as f64 / N as f64);
                ps.extend([p, Vec3d::new(p.x + h, p.y, p.z), Vec3d::new(p.x, p.y + h, p.z), Vec3d::new(p.x, p.y, p.z + h)]);
            }
        }
    }
    let d = crate::surface_mesh::sample(field, ps);
    let mut l = 1.0f64;
    for q in d.chunks_exact(4) {
        if q[0].abs() > 2.0 * step {
            continue;
        }
        let g = ((q[1] - q[0]).powi(2) + (q[2] - q[0]).powi(2) + (q[3] - q[0]).powi(2)).sqrt() / h;
        if g.is_finite() {
            l = l.max(g);
        }
    }
    l * 1.1
}

impl SdfShape {
    /// The shape's Lipschitz bound: 1 for a field built from distances (the engine's own
    /// nodes keep it), measured over the bounds when a kernel is in it.
    pub fn lipschitz(&self) -> f64 {
        if !has_kernel(&self.root) {
            return 1.0;
        }
        let v = |p: [f64; 3]| Vec3d::new(p[0], p[1], p[2]);
        measure(&self.field(), v(self.min), v(self.max))
    }
}

fn has_kernel(n: &SdfNode) -> bool {
    match n {
        SdfNode::Kernel(_) => true,
        SdfNode::Combine { of, .. } => of.iter().any(has_kernel),
        SdfNode::Material { within, of, .. } => has_kernel(of) || within.as_deref().is_some_and(has_kernel),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } => has_kernel(of),
        _ => false,
    }
}
