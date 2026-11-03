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

/// A direction also answers once it has spent as many evaluations again since its farthest
/// point last moved out by more than `tol`, and at least this many: best first finds the
/// extreme early (the demo vase and lattice: within 4k evaluations in every direction), and what
/// follows only proves it. A flat floor's rim or a ring-shaped top leaves a band of cells at one
/// margin that halves only when the cells do, so proving them would take the whole budget for
/// the answer it already has.
const STALL: usize = 4_096;

/// The axis-aligned box, in the frame `m` maps into, of the solid (`field <= 0`) inside the
/// field's box `min..max`: each side within `tol` (a sharp extreme) or exact (a flat or
/// smoothly curved one), or, where proving that stalls ([`STALL`]), the farthest solid point
/// found. `lipschitz` bounds the field's slope (1 for a distance). None when no solid point
/// is found.
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
/// ends when no cell left can beat the farthest point by more than `tol`. A farthest point on
/// the surface ends on its last solid point along `d`, to the field's own precision ([`last_solid`]):
/// the Newton steps leave it up to `tol` / 1000 past the surface (an f32 kernel's 6 mm cube
/// topped out 3.6 nm high, and a layer stack cut up to that ended with a 3.6 nm layer).
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
    // the surface point `best` is at (None: a solid cell's corner)
    let mut best_p: Option<[f64; 3]> = None;
    // evaluations spent when `best` last moved out by more than `tol`
    let gained_at = std::cell::Cell::new(0usize);
    let raise = |best: &mut f64, best_p: &mut Option<[f64; 3]>, v: f64, p: Option<[f64; 3]>, at: usize| {
        if v > *best + tol { gained_at.set(at); }
        if v > *best {
            *best = v;
            *best_p = p;
        }
    };
    let mut heap = BinaryHeap::new();
    let size = std::array::from_fn(|k| max[k] - min[k]);
    heap.push(Cell { reach: reach(min, size), min, size });
    while let Some(cell) = heap.pop() {
        if cell.reach <= best + tol {
            break;
        }
        if evals.get() > BUDGET || best.is_finite() && evals.get() > STALL.max(2 * gained_at.get()) {
            // Out of evaluations, or only proving: the farthest solid point found, never the
            // unproven reach of the cells left (those are the empty margin around the solid,
            // and a part placed by that box would hang above the bed by it).
            if best.is_finite() {
                break;
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
                raise(&mut best, &mut best_p, r_reach, None, evals.get());
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
                raise(&mut best, &mut best_p, dot(p), Some(p), evals.get());
                let cos = n[0] * dn[0] + n[1] * dn[1] + n[2] * dn[2];
                // A point facing d is the extreme of the surface nearest the centre only: more
                // of the solid can rise past it in the same cell (a pillar on a ledge), so the
                // cell splits like any other (what it holds at or below `best` is pruned).
                if cos <= 1.0 - 1e-6 && half.iter().all(|&h| h <= coarse) {
                    // a smooth surface tilted from d by its normal rises this much over the cell
                    if dot(p) + dl * 2.0 * r * (1.0 - cos * cos).max(0.0).sqrt() <= best + tol {
                        continue;
                    }
                    // else from its point along the surface to where that stops rising along d
                    // (in the cell or past it): all it adds
                    let top = slide(&mut f, p, dn, half.iter().fold(0.0f64, |m, &h| m.max(h)), extent, tol);
                    raise(&mut best, &mut best_p, dot(top), Some(top), evals.get());
                    continue;
                }
            }
            heap.push(Cell { reach: r_reach, min: cmin, size: half });
        }
    }
    if !best.is_finite() {
        return None;
    }
    Some(match best_p.and_then(|p| last_solid(&mut f, p, dn, extent, tol)) {
        Some(q) if (dot(q) - best).abs() <= tol => dot(q),
        _ => best,
    })
}

/// The solid's last point along unit `d` from surface point `p` (within `tol` of the surface):
/// in from `p` against the field's gradient to a solid point (`f <= 0`), then from there along
/// `d` to where the solid ends, bisected down to what the field resolves (an f32 kernel: its own
/// grid of points). None when neither is found within `tol`.
fn last_solid(f: &mut impl FnMut([f64; 3]) -> f64, p: [f64; 3], d: [f64; 3], extent: f64, tol: f64) -> Option<[f64; 3]> {
    let along = |q: [f64; 3], dir: [f64; 3], t: f64| -> [f64; 3] { std::array::from_fn(|a| q[a] + t * dir[a]) };
    // the first of `t` = tol / 1000, doubling up to `tol`, where `f` at q + t dir is solid or not (`solid`)
    let reach = |f: &mut dyn FnMut([f64; 3]) -> f64, q: [f64; 3], dir: [f64; 3], solid: bool| {
        let mut t = tol * 1e-3;
        while t <= tol {
            if (f(along(q, dir, t)) <= 0.0) == solid {
                return Some(t);
            }
            t *= 2.0;
        }
        None
    };
    let q = if f(p) <= 0.0 {
        p
    } else {
        let h = extent * 1e-6;
        let g: [f64; 3] = std::array::from_fn(|a| {
            let (mut up, mut down) = (p, p);
            up[a] += h;
            down[a] -= h;
            (f(up) - f(down)) / (2.0 * h)
        });
        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        if l < 1e-12 {
            return None;
        }
        let inward = g.map(|x| -x / l);
        along(p, inward, reach(f, p, inward, true)?)
    };
    // the crossing: solid at t_in, empty at t_out
    let mut t_out = reach(f, q, d, false)?;
    // (the step before it was solid; the first one's is `q`)
    let mut t_in = if t_out > tol * 1.5e-3 { 0.5 * t_out } else { 0.0 };
    for _ in 0..100 {
        let t = 0.5 * (t_in + t_out);
        if t == t_in || t == t_out {
            break;
        }
        if f(along(q, d, t)) <= 0.0 { t_in = t } else { t_out = t }
    }
    Some(along(q, d, t_in))
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

    /// A 6 mm cube as an f32 kernel computes it (points rounded to f32, f32 arithmetic).
    struct F32Cube;
    impl Sdf3 for F32Cube {
        fn distance(&self, p: Vec3d) -> f64 {
            let q = [p.x as f32, p.y as f32, p.z as f32 - 3.0].map(|c| c.abs() - 3.0);
            let out = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
            (out + q[0].max(q[1]).max(q[2]).min(0.0)) as f64
        }
    }

    #[test]
    fn a_flat_top_ends_where_the_solid_does() {
        // the Newton steps stop within tol / 1000 of the surface: on an f32 field that left the
        // top up to nanometres past it, and a layer stack cut to it ended with a sliver
        for tol in [0.005, 0.05] {
            let (lo, hi) = surface_bounds(&F32Cube, [-4.0, -4.0, -1.0], [4.5, 4.5, 7.5], 1.0, &I, tol).expect("a solid");
            assert!(hi[2] <= 6.0 + 2.4e-7 && hi[2] >= 6.0 - 1e-6, "top {} (tol {tol})", hi[2]);
            assert!(lo[2] >= -2.4e-7 && lo[2] <= 1e-6, "bottom {} (tol {tol})", lo[2]);
            assert!(F32Cube.distance(Vec3d::new(0.0, 0.0, hi[2])) <= 0.0, "the top is solid");
        }
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
    fn a_pillar_on_a_ledge_reaches_past_the_ledge_top() {
        // A 10 mm square ledge 1.5 mm tall under a 4 mm square pillar 3 mm tall: the ledge's
        // flat top is the surface nearest the cells around the pillar's foot.
        let at = |y: f64, size: [f64; 3]| SdfNode::Transform { at: [0.0, y, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Box { size, radius: 0.0 }) };
        let step = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![at(0.75, [10.0, 1.5, 10.0]), at(1.5, [4.0, 3.0, 4.0])] };
        let (lo, hi) = bounds(step, [-6.0, -1.0, -6.0], [6.0, 4.0, 6.0], &I);
        assert!(near(lo, [-5.0, 0.0, -5.0], 0.01) && near(hi, [5.0, 3.0, 5.0], 0.01), "{lo:?} {hi:?}");
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
