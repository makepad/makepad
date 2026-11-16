use crate::virtual_points::{determinant3, determinant4, VirtualPoints};
use makepad_csg_exact::Dyadic;
use makepad_csg_math::Vec3d;
use std::cmp::Ordering;

/// One identity per point the cut uses; a point's position never decides identity.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Key {
    /// An input vertex, by point id (a B vertex aliased onto an A vertex takes the A id).
    V(u32),
    /// An edge of operand `.0` (its two point ids, ascending) crossing the plane of the other
    /// operand's triangle `.2` (that operand's triangle index).
    EF(u8, [u32; 2], u32),
    /// An edge of A and an edge of B (point ids, each ascending) meeting in one point.
    EE([u32; 2], [u32; 2]),
}

impl Key {
    pub(crate) fn ef(operand: u8, p: u32, q: u32, triangle: u32) -> Key {
        let mut edge = [p, q];
        edge.sort_unstable();
        Key::EF(operand, edge, triangle)
    }

    pub(crate) fn ee(mut a_edge: [u32; 2], mut b_edge: [u32; 2]) -> Key {
        a_edge.sort_unstable();
        b_edge.sort_unstable();
        Key::EE(a_edge, b_edge)
    }
}

/// A point in homogeneous virtual coordinates, weight positive, with its rounded position.
#[derive(Clone, Debug)]
pub(crate) struct Constructed {
    pub(crate) coordinates: [Dyadic; 4],
    pub(crate) position: Vec3d,
}

impl Constructed {
    fn new(mut coordinates: [Dyadic; 4]) -> Constructed {
        debug_assert!(!coordinates[3].is_zero(), "constructed point must have nonzero weight");
        if coordinates[3].sign() < 0 {
            coordinates = coordinates.map(|coordinate| coordinate.negate());
        }
        let position = Vec3d::new(
            coordinates[0].ratio(&coordinates[3]),
            coordinates[1].ratio(&coordinates[3]),
            coordinates[2].ratio(&coordinates[3]),
        );
        Constructed { coordinates, position }
    }
}

pub(crate) fn edge_plane(points: &VirtualPoints, p: u32, q: u32, plane: [u32; 3]) -> Constructed {
    let [a, b, c] = plane.map(|id| points.homogeneous(id));
    let p = points.homogeneous(p);
    let q = points.homogeneous(q);
    let fp = determinant4([&a, &b, &c, &p]);
    let fq = determinant4([&a, &b, &c, &q]);
    debug_assert!(fp.sign() * fq.sign() < 0, "edge endpoints must lie on opposite sides of the plane");
    Constructed::new(std::array::from_fn(|axis| {
        fp.mul(&q[axis]).sub(&fq.mul(&p[axis]))
    }))
}

/// The crossing of two coplanar edges, projected along `k` first. Nearly parallel edges can give a
/// rounded cross product whose dominant axis lies in their plane, where both project onto one line,
/// so the axis is the first from `k` on which p and q fall on opposite sides of r u.
pub(crate) fn edge_edge(points: &VirtualPoints, p: u32, q: u32, r: u32, u: u32, k: usize) -> Constructed {
    let [p, q, r, u] = [p, q, r, u].map(|id| points.homogeneous(id));
    let side = |axis: usize, s: &[Dyadic; 4]| {
        let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
        determinant3([&r[i], &r[j], &r[3]], [&u[i], &u[j], &u[3]], [&s[i], &s[j], &s[3]])
    };
    let mut sides = (0..3).map(|step| (side((k + step) % 3, &p), side((k + step) % 3, &q)));
    let (gp, gq) = sides.find(|(gp, gq)| gp.sign() != gq.sign())
        .expect("crossing edges must not be parallel");
    Constructed::new(std::array::from_fn(|axis| {
        gp.mul(&q[axis]).sub(&gq.mul(&p[axis]))
    }))
}

pub(crate) fn compare_along(axis: usize, a: &[Dyadic; 4], b: &[Dyadic; 4]) -> Ordering {
    a[axis].mul(&b[3])
        .sub(&b[axis].mul(&a[3]))
        .sign()
        .cmp(&0)
}

pub(crate) fn dominant_axis(v: Vec3d) -> usize {
    v.dominant_axis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructed_points_and_sorted_keys() {
        let mut points = VirtualPoints::new(vec![
            Vec3d::new(-2.0, 0.0, 1.25),
            Vec3d::new(2.0, 0.0, 1.0),
            Vec3d::new(0.0, -1.0, 0.75),
            Vec3d::new(0.0, 1.0, 1.0),
            Vec3d::new(0.0, 0.0, 3.0),
        ]);
        // Non-unit weights on two classed points exercise mixed homogeneous scales.
        let class = points.add_class([
            Vec3d::new(0.0, 0.0, 1.0),
            Vec3d::new(2.0, 0.0, 1.0),
            Vec3d::new(0.0, 3.0, 1.0),
        ]).expect("class plane must have a normal");
        for id in [0, 2] {
            let placed = points.place(id, &[class]).expect("point must lie on the class plane");
            points.set(id, Some(placed));
        }

        let plane = [3, 2, 4];
        let cut = edge_plane(&points, 0, 1, plane);
        let [a, b, c] = plane.map(|id| points.homogeneous(id));
        assert!(determinant4([&a, &b, &c, &cut.coordinates]).is_zero(),
            "edge-plane intersection lies exactly on the plane");
        let p = points.homogeneous(0);
        let q = points.homogeneous(1);
        let axis = dominant_axis(points.approx(1) - points.approx(0));
        assert!(compare_along(axis, &p, &cut.coordinates) == Ordering::Less
            && compare_along(axis, &cut.coordinates, &q) == Ordering::Less,
            "edge-plane intersection lies strictly between the edge endpoints");

        let crossing = edge_edge(&points, 0, 1, 3, 2, 2);
        for edge in [[0, 1], [3, 2]] {
            let [a, b] = edge.map(|id| points.homogeneous(id));
            let c = &crossing.coordinates;
            assert!(determinant3([&a[0], &a[1], &a[3]], [&b[0], &b[1], &b[3]], [&c[0], &c[1], &c[3]]).is_zero(),
                "edge-edge intersection lies exactly on edge {edge:?}");
        }
        assert!(cut.coordinates[3].sign() > 0 && crossing.coordinates[3].sign() > 0,
            "constructed weights are positive after reversed plane and edge winding");
        assert_eq!([cut.position, crossing.position], [Vec3d::new(0.0, 0.0, 1.0); 2],
            "constructed positions are the rounded intersections");
        assert_eq!(Key::ef(1, 7, 2, 5), Key::EF(1, [2, 7], 5),
            "edge-face key sorts its edge and preserves operand and triangle");
        assert_eq!(Key::ee([9, 4], [3, 1]), Key::EE([4, 9], [1, 3]),
            "edge-edge key sorts each edge and preserves operand order");
    }
}
