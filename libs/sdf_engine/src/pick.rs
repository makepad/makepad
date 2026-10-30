//! Picking on the CPU: one ray against a shape's exact field (f64), the hit
//! point, its normal and the [`SdfNode::Id`] whose surface it is. One ray
//! per click, so the exact `csg_sdf` field is affordable and needs no GPU
//! readback; what it hits is what the march draws (to the march's
//! tolerance).

use makepad_csg_math::Vec3d;
use makepad_csg_sdf::{distance, distance_id, SdfNode};

/// A ray's hit on a shape (its own frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfHit {
    pub t: f64,
    pub point: [f64; 3],
    /// Unit, the field's gradient (pointing out of the solid).
    pub normal: [f64; 3],
    /// The innermost Id node whose surface this is (`None` outside every Id).
    pub id: Option<u32>,
}

/// Sphere traces `ro + t rd` (`rd` unit) over `[t0, t1]` to within `eps`,
/// each step the distance times `step` (a kernel that is no distance: its
/// volume's `kernel_scale`, the inverse of its Lipschitz bound; 1 otherwise).
pub fn pick(root: &SdfNode, ro: [f64; 3], rd: [f64; 3], t0: f64, t1: f64, eps: f64, step: f64) -> Option<SdfHit> {
    let at = |t: f64| Vec3d::new(ro[0] + rd[0] * t, ro[1] + rd[1] * t, ro[2] + rd[2] * t);
    let mut t = t0.max(0.0);
    let step = step.clamp(0.125, 1.0);
    for _ in 0..(1024.0 / step) as usize {
        if t > t1 {
            return None;
        }
        let d = distance(root, at(t));
        if d < eps {
            let p = at(t);
            let h = eps.max(1e-4);
            let g = [
                distance(root, Vec3d::new(p.x + h, p.y, p.z)) - distance(root, Vec3d::new(p.x - h, p.y, p.z)),
                distance(root, Vec3d::new(p.x, p.y + h, p.z)) - distance(root, Vec3d::new(p.x, p.y - h, p.z)),
                distance(root, Vec3d::new(p.x, p.y, p.z + h)) - distance(root, Vec3d::new(p.x, p.y, p.z - h)),
            ];
            let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-30);
            let (_, id) = distance_id(root, p);
            return Some(SdfHit { t, point: [p.x, p.y, p.z], normal: g.map(|c| c / l), id });
        }
        t += d * step;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_sdf::SdfOp;

    #[test]
    fn a_ray_names_the_feature_it_hits() {
        let ball = |r: f64, at: [f64; 3]| SdfNode::Transform { at, rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: r }) };
        let body = SdfNode::Id { id: 1, of: Box::new(ball(5.0, [0.0; 3])) };
        let boss = SdfNode::Id { id: 2, of: Box::new(ball(2.0, [0.0, 0.0, 6.0])) };
        let n = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![body, boss] };
        let down = pick(&n, [0.0, 0.0, 20.0], [0.0, 0.0, -1.0], 0.0, 100.0, 1e-6, 1.0).unwrap();
        assert_eq!(down.id, Some(2));
        assert!((down.t - 12.0).abs() < 1e-5 && (down.normal[2] - 1.0).abs() < 1e-6);
        let side = pick(&n, [20.0, 0.0, 0.0], [-1.0, 0.0, 0.0], 0.0, 100.0, 1e-6, 1.0).unwrap();
        assert_eq!(side.id, Some(1));
        assert!(pick(&n, [20.0, 20.0, 0.0], [0.0, 0.0, -1.0], 0.0, 100.0, 1e-6, 1.0).is_none());
    }
}
