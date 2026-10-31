//! The box a field's solid fills, without meshing it: per axis of a target frame, the
//! farthest solid point found by a best-first search over the field's cells.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use std::collections::BinaryHeap;

/// Row-major affine `p' = m * p`.
pub type Affine = [[f64; 4]; 3];

/// Field evaluations one direction may spend before it answers with the farthest solid point
/// found (search order is farthest first: what it can miss is a feature smaller than a cell).
const BUDGET: usize = 60_000;

/// The axis-aligned box, in the frame `m` maps into, of the solid (`field <= 0`) inside the
/// field's box `min..max`: each side within `tol` (a sharp extreme) or exact (a flat or
/// smoothly curved one). `lipschitz` bounds the field's slope (1 for a distance). None when
/// no solid point is found.
pub fn surface_bounds(field: &dyn Sdf3, min: [f64; 3], max: [f64; 3], lipschitz: f64, m: &Affine, tol: f64) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let d = std::array::from_fn(|k| sign * m[axis][k]);
            let v = support(field, min, max, lipschitz.max(1.0), d, sign * m[axis][3], tol)?;
            if sign < 0.0 {
                lo[axis] = -v;
            } else {
                hi[axis] = v;
            }
        }
    }
    Some((lo, hi))
}

/// A cell of the search: its box and the most `d · p` reaches in it.
struct Cell {
    reach: f64,
    min: [f64; 3],
    size: [f64; 3],
}

impl PartialEq for Cell {
    fn eq(&self, o: &Self) -> bool {
        self.reach == o.reach
    }
}
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cell {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.reach.total_cmp(&o.reach)
    }
}

/// The most `d · p + c` reaches over the solid: best-first over the field's cells (the cell
/// that may reach farthest first). A cell on the surface takes its centre onto the surface;
/// one whose point faces `d` is resolved (a flat extreme), and so is a cell no bigger than a
/// 64th of the box once its point slid along the surface to where the surface stops rising
/// along `d` (two extremes inside one such cell are what a mesh of that cell misses too). It
/// ends when no cell left can beat the farthest point by more than `tol`.
fn support(field: &dyn Sdf3, min: [f64; 3], max: [f64; 3], lip: f64, d: [f64; 3], c: f64, tol: f64) -> Option<f64> {
    let dot = |p: [f64; 3]| d[0] * p[0] + d[1] * p[1] + d[2] * p[2] + c;
    // The most d·p reaches over a box: its corner farthest along d.
    let reach = |min: [f64; 3], size: [f64; 3]| dot(std::array::from_fn(|k| if d[k] > 0.0 { min[k] + size[k] } else { min[k] }));
    let extent = (0..3).map(|k| max[k] - min[k]).fold(0.0f64, f64::max).max(1e-9);
    let coarse = extent / 64.0;
    let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-30);
    let dn = d.map(|x| x / dl);
    let evals = std::cell::Cell::new(0usize);
    let mut f = |p: [f64; 3]| {
        evals.set(evals.get() + 1);
        field.distance(Vec3d::new(p[0], p[1], p[2]))
    };
    let mut best = f64::NEG_INFINITY;
    let mut heap = BinaryHeap::new();
    let size = std::array::from_fn(|k| max[k] - min[k]);
    heap.push(Cell { reach: reach(min, size), min, size });
    while let Some(cell) = heap.pop() {
        if cell.reach <= best + tol {
            return Some(best);
        }
        if evals.get() > BUDGET {
            // Out of evaluations: the farthest solid point found, never the unproven reach of
            // the cells left (those are the empty margin around the solid, and a part placed
            // by that box would hang above the bed by it).
            if best.is_finite() {
                return Some(best);
            }
            return Some(cell.reach);
        }
        let half = cell.size.map(|s| s * 0.5);
        if half.iter().all(|&h| h < tol) {
            continue;
        }
        for k in 0..8 {
            let cmin: [f64; 3] = std::array::from_fn(|a| cell.min[a] + if k >> a & 1 == 1 { half[a] } else { 0.0 });
            let r_reach = reach(cmin, half);
            if r_reach <= best + tol {
                continue;
            }
            let centre: [f64; 3] = std::array::from_fn(|a| cmin[a] + half[a] * 0.5);
            let r = 0.5 * (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
            let v = f(centre);
            if v > lip * r {
                // nothing of the solid in it
                continue;
            }
            if v < -lip * r {
                // solid throughout: its farthest corner is the solid's
                best = best.max(r_reach);
                continue;
            }
            // A measured slope over 1 (a kernel) leaves the empty margin around the solid unpruned
            // down to small cells: a cell a distance's worth from the surface only splits, the
            // Newton steps are for cells the surface can be in.
            if v.abs() > r {
                heap.push(Cell { reach: r_reach, min: cmin, size: half });
                continue;
            }
            let near = |p: [f64; 3]| (0..3).all(|a| p[a] >= cmin[a] - half[a] && p[a] <= cmin[a] + 2.0 * half[a]);
            if let Some((p, n)) = project(&mut f, centre, extent, tol).filter(|(p, _)| near(*p)) {
                best = best.max(dot(p));
                let cos = n[0] * dn[0] + n[1] * dn[1] + n[2] * dn[2];
                // facing d: a flat (or smooth) extreme, nothing in the cell beyond it
                if cos > 1.0 - 1e-6 {
                    continue;
                }
                if half.iter().all(|&h| h <= coarse) {
                    // a smooth surface tilted from d by its normal rises this much over the cell
                    if dot(p) + dl * 2.0 * r * (1.0 - cos * cos).max(0.0).sqrt() <= best + tol {
                        continue;
                    }
                    // else from its point along the surface to where that stops rising along d
                    // (in the cell or past it): all it adds
                    let top = slide(&mut f, p, dn, half.iter().fold(0.0f64, |m, &h| m.max(h)), extent, tol);
                    best = best.max(dot(top));
                    continue;
                }
            }
            heap.push(Cell { reach: r_reach, min: cmin, size: half });
        }
    }
    best.is_finite().then_some(best)
}

/// From surface point `p`, along the surface towards `d` (unit) while that gains: the
/// surface's local extreme along `d` (a corner, an edge or a smooth top), from steps of
/// `step` halved down to `tol` / 4.
fn slide(f: &mut impl FnMut([f64; 3]) -> f64, mut p: [f64; 3], d: [f64; 3], mut step: f64, extent: f64, tol: f64) -> [f64; 3] {
    let along = |p: [f64; 3]| d[0] * p[0] + d[1] * p[1] + d[2] * p[2];
    let mut tries = 0;
    while step > tol * 0.25 && tries < 32 {
        tries += 1;
        let q = std::array::from_fn(|a| p[a] + step * d[a]);
        match project(f, q, extent, tol) {
            Some((q, _)) if along(q) > along(p) + extent * 1e-9 => p = q,
            _ => step *= 0.5,
        }
    }
    p
}

/// Newton steps from `p` onto the surface (to within a thousandth of `tol`): the point and
/// its outward normal.
fn project(f: &mut impl FnMut([f64; 3]) -> f64, mut p: [f64; 3], extent: f64, tol: f64) -> Option<([f64; 3], [f64; 3])> {
    let h = extent * 1e-6;
    for _ in 0..8 {
        let v = f(p);
        let g: [f64; 3] = std::array::from_fn(|a| {
            let mut q = p;
            q[a] += h;
            let up = f(q);
            q[a] -= 2.0 * h;
            (up - f(q)) / (2.0 * h)
        });
        let g2 = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
        if g2 < 1e-24 {
            return None;
        }
        if v.abs() <= tol * 1e-3 {
            let l = g2.sqrt();
            return Some((p, g.map(|x| x / l)));
        }
        p = std::array::from_fn(|a| p[a] - v * g[a] / g2);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SdfNode, SdfOp, SdfShape};

    fn bounds(root: SdfNode, min: [f64; 3], max: [f64; 3], m: &Affine) -> ([f64; 3], [f64; 3]) {
        let s = SdfShape { root, min, max, materials: Vec::new() };
        surface_bounds(&*s.field(), s.min, s.max, s.lipschitz(), m, 0.005).expect("a solid")
    }

    const I: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

    fn near(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
    }

    #[test]
    fn a_sphere_and_a_box_fill_their_boxes() {
        let (lo, hi) = bounds(SdfNode::Sphere { radius: 10.0 }, [-13.0; 3], [12.0; 3], &I);
        assert!(near(lo, [-10.0; 3], 0.01) && near(hi, [10.0; 3], 0.01), "{lo:?} {hi:?}");
        let (lo, hi) = bounds(SdfNode::Box { size: [20.0, 10.0, 6.0], radius: 0.0 }, [-15.0; 3], [15.0; 3], &I);
        assert!(near(lo, [-10.0, -5.0, -3.0], 0.01) && near(hi, [10.0, 5.0, 3.0], 0.01), "{lo:?} {hi:?}");
    }

    #[test]
    fn a_turned_box_stands_on_its_lowest_corner() {
        // 30 degrees about X: the box's corners reach 5 cos + 3 sin down and up.
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let m: Affine = [[1.0, 0.0, 0.0, 1.0], [0.0, c, -s, 2.0], [0.0, s, c, 3.0]];
        let (lo, hi) = bounds(SdfNode::Box { size: [20.0, 10.0, 6.0], radius: 0.0 }, [-15.0; 3], [15.0; 3], &m);
        let (y, z) = (5.0 * c + 3.0 * s, 5.0 * s + 3.0 * c);
        assert!(near(lo, [-9.0, 2.0 - y, 3.0 - z], 0.02) && near(hi, [11.0, 2.0 + y, 3.0 + z], 0.02), "{lo:?} {hi:?}");
    }

    #[test]
    fn a_hollow_blend_fills_its_outside() {
        // A vase-like skin: an ellipsoid blended with a cylinder, hollowed to a 1.6 mm wall.
        let body = SdfNode::Combine {
            op: SdfOp::Union,
            blend: 4.0,
            of: vec![SdfNode::Ellipsoid { radii: [21.0, 25.0, 21.0] }, SdfNode::Transform { at: [0.0, 20.0, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Cylinder { radius: 9.0, top_radius: 9.0, height: 30.0 }) }],
        };
        let shell = SdfNode::Shell { thickness: 0.8, of: Box::new(body) };
        let (lo, hi) = bounds(shell, [-30.0, -30.0, -30.0], [30.0, 60.0, 30.0], &I);
        assert!(near([lo[0], lo[2], 0.0], [-21.8, -21.8, 0.0], 0.05) && near([hi[0], hi[2], 0.0], [21.8, 21.8, 0.0], 0.05), "{lo:?} {hi:?}");
        assert!((lo[1] + 25.8).abs() < 0.05, "{lo:?}");
    }
}
