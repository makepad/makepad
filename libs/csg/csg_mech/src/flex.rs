//! Parts bonded to another as they print, and how they flex.
//!
//! A bonded part touches the part it is bonded to with no clearance: that part's shape is cut out
//! of it, so the two print fused where they meet (a TPU spine through a row of rigid segments, an
//! inlay of another colour). Bonded to a rigid part, a part is fixed to it. Bonded to a flexible
//! one, each part bends to the next in the row about the flexible part where it runs free between
//! them: in the sim a soft joint there, as stiff in bending as the flexible part's section across
//! that free run (its modulus times the section's second moment of area, over the run's length).

use crate::geom::*;
use crate::FinalPart;
use makepad_csg_sdf::Sdf3;
use std::f64::consts::PI;

/// A generic 95A TPU's Young's modulus (MPa): a named default, as no printer profile gives one.
pub const FLEX_MODULUS: f64 = 26.0;
/// How much a TPU part damps its own bending, of critical: a named default.
pub const FLEX_DAMPING: f64 = 0.3;
/// A section this many times stiffer one way than the other bends only the soft way.
pub const ONE_WAY: f64 = 4.0;
/// How far out past the flexible part's section a bonded part must reach round it to hold it (mm).
const HOLD: f64 = 0.4;

/// Parts bonded to `to` as they print.
#[derive(Clone, Debug, PartialEq)]
pub struct BondDecl {
    pub to: usize,
    /// In their order along it: a flexible `to` bends each to the next where it runs free between
    /// them, past the point midway between their centres.
    pub parts: Vec<usize>,
    /// `to` is of a flexible material (the host knows its material).
    pub flexible: bool,
}

/// The flexible part's section across its run at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// Its centroid, area (mm²), second moments of area about its principal axes through the
    /// centroid (mm⁴, the smaller first) and the axis of the smaller: the one it bends about most
    /// easily.
    pub centroid: P3,
    pub area: f64,
    pub moments: (f64, f64),
    pub soft_axis: P3,
}

/// The soft joint between two neighbours bonded to a flexible part, as measured on the parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flex {
    pub a: usize,
    pub b: usize,
    /// The middle of the free run (on the section's centroid), and the run's direction (a to b).
    pub pivot: P3,
    pub axis: P3,
    /// The free run's length (mm).
    pub length: f64,
    pub section: Section,
}

impl Flex {
    /// Its bending stiffness about the soft axis and about the stiff one (N mm per radian), for a
    /// modulus of `e` MPa.
    pub fn stiffness(&self, e: f64) -> (f64, f64) {
        (e * self.section.moments.0 / self.length, e * self.section.moments.1 / self.length)
    }

    /// It bends only about its soft axis: its section is [`ONE_WAY`] times stiffer the other way.
    pub fn one_way(&self) -> bool {
        self.section.moments.1 >= ONE_WAY * self.section.moments.0
    }

    /// The ball it bends in: the parts' collision proxies leave it out, so the neighbours do not
    /// collide where they meet the free run.
    pub fn keep_out(&self) -> (P3, f64) {
        (self.pivot, 0.5 * self.length + (self.section.area / PI).sqrt() + 1.0)
    }
}

/// The section of `field` across `axis` through `at` (the piece of it round `at`), or None when
/// `at` is not inside it.
pub fn section(field: &dyn Sdf3, at: P3, axis: P3) -> Option<Section> {
    let axis = norm(axis);
    let (u, v) = basis(axis);
    let inside = |x: f64, y: f64| field.distance(v3(madd(madd(at, u, x), v, y))) < 0.0;
    if !inside(0.0, 0.0) {
        return None;
    }
    // a square round it that the piece does not reach out of
    let mut r = 2.0;
    while r < 64.0 && (0..32).any(|k| {
        let t = k as f64 * PI / 16.0;
        inside(r * t.cos(), r * t.sin())
    }) {
        r *= 2.0;
    }
    const N: usize = 128;
    let res = 2.0 * r / N as f64;
    let at_cell = |i: usize| -r + (i as f64 + 0.5) * res;
    // each cell's share inside, from its centre's distance (exact to the cell for a straight edge)
    let share: Vec<f64> = (0..N * N).map(|k| (0.5 - field.distance(v3(madd(madd(at, u, at_cell(k % N)), v, at_cell(k / N)))) / res).clamp(0.0, 1.0)).collect();
    // the cells joined to the four round `at`: however near its edge `at` lies, the one on its
    // inner side is inside (the middle one alone may be over the edge)
    let mut seen = vec![false; N * N];
    let mut stack: Vec<usize> = [(N / 2 - 1, N / 2 - 1), (N / 2, N / 2 - 1), (N / 2 - 1, N / 2), (N / 2, N / 2)].iter().map(|&(i, j)| j * N + i).collect();
    let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
    while let Some(k) = stack.pop() {
        let w = share[k];
        if seen[k] || w <= 0.0 {
            continue;
        }
        seen[k] = true;
        let (i, j) = (k % N, k / N);
        let (x, y) = (at_cell(i), at_cell(j));
        n += w;
        (sx, sy, sxx, syy, sxy) = (sx + w * x, sy + w * y, sxx + w * x * x, syy + w * y * y, sxy + w * x * y);
        if i > 0 {
            stack.push(k - 1);
        }
        if i + 1 < N {
            stack.push(k + 1);
        }
        if j > 0 {
            stack.push(k - N);
        }
        if j + 1 < N {
            stack.push(k + N);
        }
    }
    if n == 0.0 {
        return None;
    }
    let cell = res * res;
    let (mx, my) = (sx / n, sy / n);
    // about the centroid: bending about u takes y squared, about v x squared (each cell's own too)
    let own = cell * cell / 12.0;
    let (iu, iv, iuv) = (cell * (syy - n * my * my) + n * own, cell * (sxx - n * mx * mx) + n * own, cell * (sxy - n * mx * my));
    // principal moments of [[iu, -iuv], [-iuv, iv]]
    let (mean, half) = (0.5 * (iu + iv), (0.25 * (iu - iv) * (iu - iv) + iuv * iuv).sqrt());
    let (soft, stiff) = (mean - half, mean + half);
    // the eigenvector of the smaller: (iu - soft) a - iuv b = 0
    let (a, b) = if iuv.abs() > 1e-12 * mean { (iuv, iu - soft) } else if iu <= iv { (1.0, 0.0) } else { (0.0, 1.0) };
    let soft_axis = norm(add(scale(u, a), scale(v, b)));
    Some(Section { centroid: madd(madd(at, u, mx), v, my), area: n * cell, moments: (soft, stiff), soft_axis })
}

/// Where a part's vertices' box is centred.
pub(crate) fn centre(p: &FinalPart) -> P3 {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for q in p.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    scale(add(lo, hi), 0.5)
}

/// The point of `field` nearest `at` in the plane through it across `axis`, within `reach`: `at`
/// itself when inside, else the first inside point on rings round it, 0.25 mm apart.
fn nearest_in(field: &dyn Sdf3, at: P3, axis: P3, reach: f64) -> Option<P3> {
    let (u, v) = basis(axis);
    let inside = |p: P3| field.distance(v3(p)) < 0.0;
    if inside(at) {
        return Some(at);
    }
    let mut r = 0.25;
    while r <= reach {
        let n = ((2.0 * PI * r / 0.25).ceil() as usize).max(8);
        for k in 0..n {
            let t = k as f64 * 2.0 * PI / n as f64;
            let p = madd(madd(at, u, r * t.cos()), v, r * t.sin());
            if inside(p) {
                return Some(p);
            }
        }
        r += 0.25;
    }
    None
}

/// The soft joint between the `k`th and the next part bonded to a flexible part: where the
/// flexible part runs free between them (where neither holds it round, about the plane midway
/// between their centres), along its own direction there, and its section in the middle of that
/// run.
pub fn measure(bond: &BondDecl, k: usize, parts: &[FinalPart]) -> Result<Flex, String> {
    let (a, b) = (bond.parts[k], bond.parts[k + 1]);
    let (ca, cb) = (centre(&parts[a]), centre(&parts[b]));
    let chord = norm(sub(cb, ca));
    let at = scale(add(ca, cb), 0.5);
    let (to, na, nb) = (parts[bond.to].name, parts[a].name, parts[b].name);
    let spine = parts[bond.to].field;
    let pt = |p: P3| format!("[{:.1}, {:.1}, {:.1}]", p[0], p[1], p[2]);
    let reach = len(sub(cb, ca));
    let missing = || format!("{to} does not run between {na} and {nb} near {}: run it through the plane midway between them", pt(at));
    let start = nearest_in(spine, at, chord, reach).ok_or_else(missing)?;
    let first = section(spine, start, chord).ok_or_else(missing)?;
    // its own direction there: from its section a millimetre back to one a millimetre on
    let axis = match (section(spine, madd(first.centroid, chord, -1.0), chord), section(spine, madd(first.centroid, chord, 1.0), chord)) {
        (Some(s0), Some(s1)) if dot(sub(s1.centroid, s0.centroid), chord) > 0.0 => norm(sub(s1.centroid, s0.centroid)),
        _ => chord,
    };
    let rim = (first.area / PI).sqrt() + HOLD;
    let (u, v) = basis(axis);
    // the part holds it round at `s` along it from the first section
    let held = |part: usize, s: f64| {
        let c = madd(first.centroid, axis, s);
        (0..8).any(|i| {
            let t = i as f64 * PI / 4.0;
            parts[part].field.distance(v3(madd(madd(c, u, rim * t.cos()), v, rim * t.sin()))) < 0.0
        })
    };
    // from `s` on in steps of `dir` until `until` holds, within `reach`
    let walk = |mut s: f64, dir: f64, until: &dyn Fn(f64) -> bool| -> Option<f64> {
        while s.abs() <= reach {
            if until(s) {
                return Some(s);
            }
            s += 0.1 * dir;
        }
        None
    };
    let not_held = |n: &str| format!("{n} does not hold {to} toward its neighbour: run {to} into it, the part round it");
    let meet = || format!("{na} and {nb} meet round {to}: leave it a free run between them to bend in");
    // the free run: from where a last holds it to where b first holds it
    let (lo, hi) = if held(a, 0.0) {
        let lo = walk(0.0, 1.0, &|s| !held(a, s)).ok_or_else(meet)?;
        (lo, walk(lo, 1.0, &|s| held(b, s)).ok_or_else(|| not_held(nb))?)
    } else if held(b, 0.0) {
        let hi = walk(0.0, -1.0, &|s| !held(b, s)).ok_or_else(meet)?;
        (walk(hi, -1.0, &|s| held(a, s)).ok_or_else(|| not_held(na))?, hi)
    } else {
        (walk(0.0, -1.0, &|s| held(a, s)).ok_or_else(|| not_held(na))?, walk(0.0, 1.0, &|s| held(b, s)).ok_or_else(|| not_held(nb))?)
    };
    if hi - lo < 0.2 {
        return Err(meet());
    }
    let mid = madd(first.centroid, axis, 0.5 * (lo + hi));
    let s = section(spine, mid, axis).ok_or_else(|| format!("{to} does not run between {na} and {nb} at {}", pt(mid)))?;
    Ok(Flex { a, b, pivot: s.centroid, axis, length: hi - lo, section: s })
}

/// Every soft joint of the plan's flexible bonds, measured on the final parts (each bond's in its
/// parts' order); an error names the bond (its index) and why.
pub fn measure_all(bonds: &[BondDecl], parts: &[FinalPart]) -> Result<Vec<Flex>, (usize, String)> {
    let mut out = Vec::new();
    for (i, bond) in bonds.iter().enumerate().filter(|(_, b)| b.flexible) {
        for k in 0..bond.parts.len().saturating_sub(1) {
            out.push(measure(bond, k, parts).map_err(|e| (i, e))?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ribbon {
        half: P3,
    }

    impl Sdf3 for Ribbon {
        fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
            let q = [p.x.abs() - self.half[0], p.y.abs() - self.half[1], p.z.abs() - self.half[2]];
            len([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]) + q[0].max(q[1]).max(q[2]).min(0.0)
        }
    }

    /// A ribbon 1.2 mm thin and 4 mm tall along X: its section's area and second moments match
    /// a rectangle's (b h^3 / 12 each way), and it bends most easily about its tall side.
    #[test]
    fn a_ribbons_section_is_a_rectangles() {
        let r = Ribbon { half: [50.0, 2.0, 0.6] };
        let s = section(&r, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]).unwrap();
        let (soft, stiff) = (4.0 * 1.2f64.powi(3) / 12.0, 1.2 * 4.0f64.powi(3) / 12.0);
        assert!((s.area - 4.8).abs() < 0.15, "area {}", s.area);
        assert!((s.moments.0 - soft).abs() < 0.05 * soft && (s.moments.1 - stiff).abs() < 0.05 * stiff, "{:?} vs {soft} {stiff}", s.moments);
        assert!(s.soft_axis[1].abs() > 0.999, "it bends about its tall side, Y: {:?}", s.soft_axis);
        assert!(section(&r, [0.0, 5.0, 0.0], [1.0, 0.0, 0.0]).is_none());
    }

    /// A square cord turned 45 degrees about its run.
    struct Turned(Ribbon);

    impl Sdf3 for Turned {
        fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
            let h = std::f64::consts::FRAC_1_SQRT_2;
            self.0.distance(makepad_csg_math::Vec3d { x: p.x, y: (p.y - p.z) * h, z: (p.y + p.z) * h })
        }
    }

    /// A square cord measured from a point a hair inside one of its faces, as a search from
    /// beside it lands: the whole section, not none (the cell beside the point is over the face).
    #[test]
    fn a_section_from_its_edge_is_the_whole_section() {
        let r = Turned(Ribbon { half: [50.0, 1.2, 1.2] });
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let e = 1.2 - 1e-9;
        for (a, b) in [(e, 0.0), (-e, 0.0), (0.0, e), (0.0, -e), (e, 0.5), (-0.5, -e)] {
            let at = [0.0, (a + b) * h, (b - a) * h];
            let s = section(&r, at, [1.0, 0.0, 0.0]).unwrap_or_else(|| panic!("no section from {at:?}"));
            assert!((s.area - 5.76).abs() < 0.15, "area {} from {at:?}", s.area);
            assert!(len(s.centroid) < 0.05, "centred on the cord: {:?} from {at:?}", s.centroid);
        }
    }
}
