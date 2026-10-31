//! Planar regions: closed loops in a sketch plane, nested into outer
//! boundaries with holes, and their triangulation (constrained Delaunay, so
//! glyphs and sketches with holes cap cleanly).

use makepad_csg_boolean::cdt::CDT;
use makepad_csg_boolean::cdt::Point2;

pub type P2 = [f64; 2];

/// One closed loop, without a repeated last point.
pub type Loop = Vec<P2>;

/// An outer boundary (counter-clockwise) and its holes (clockwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub outer: Loop,
    pub holes: Vec<Loop>,
}

/// Twice the signed area: positive for counter-clockwise.
pub fn signed_area2(l: &[P2]) -> f64 {
    let n = l.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = l[i];
        let b = l[(i + 1) % n];
        s += a[0] * b[1] - b[0] * a[1];
    }
    s
}

pub fn area(l: &[P2]) -> f64 {
    signed_area2(l).abs() * 0.5
}

/// The loop turned counter-clockwise (`ccw`) or clockwise.
pub fn oriented(mut l: Loop, ccw: bool) -> Loop {
    if (signed_area2(&l) > 0.0) != ccw {
        l.reverse();
    }
    l
}

/// Even-odd point in loop.
pub fn contains(l: &[P2], p: P2) -> bool {
    let n = l.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (l[i], l[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0];
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// A point strictly inside a loop (for nesting tests): the midpoint of the
/// longest edge, nudged inward.
fn interior_point(l: &[P2]) -> P2 {
    let n = l.len();
    let ccw = signed_area2(l) > 0.0;
    let mut best = (0usize, -1.0);
    for i in 0..n {
        let (a, b) = (l[i], l[(i + 1) % n]);
        let d = (b[0] - a[0]).hypot(b[1] - a[1]);
        if d > best.1 {
            best = (i, d);
        }
    }
    let (a, b) = (l[best.0], l[(best.0 + 1) % n]);
    let (dx, dy) = ((b[0] - a[0]) / best.1.max(1e-12), (b[1] - a[1]) / best.1.max(1e-12));
    // Left normal points inside a counter-clockwise loop.
    let (nx, ny) = if ccw { (-dy, dx) } else { (dy, -dx) };
    let eps = (best.1 * 1e-3).max(1e-6);
    [(a[0] + b[0]) * 0.5 + nx * eps, (a[1] + b[1]) * 0.5 + ny * eps]
}

/// Loops in any orientation and nesting (a glyph, a sketch with cut-outs)
/// into regions: a loop inside an even number of others is an outer
/// boundary, inside an odd number a hole of the loop directly around it.
pub fn regions_from_loops(loops: Vec<Loop>) -> Vec<Region> {
    let loops: Vec<Loop> = loops.into_iter().map(clean_loop).filter(|l| l.len() >= 3 && area(l) > 1e-12).collect();
    let n = loops.len();
    let probes: Vec<P2> = loops.iter().map(|l| interior_point(l)).collect();
    // parent[i]: the smallest loop containing loop i.
    let mut depth = vec![0usize; n];
    let mut parent = vec![usize::MAX; n];
    for i in 0..n {
        let mut best = f64::INFINITY;
        for j in 0..n {
            if i != j && area(&loops[j]) > area(&loops[i]) && contains(&loops[j], probes[i]) {
                depth[i] += 1;
                let a = area(&loops[j]);
                if a < best {
                    best = a;
                    parent[i] = j;
                }
            }
        }
    }
    let mut regions: Vec<(usize, Region)> = Vec::new();
    for i in 0..n {
        if depth[i] % 2 == 0 {
            regions.push((i, Region { outer: oriented(loops[i].clone(), true), holes: Vec::new() }));
        }
    }
    for i in 0..n {
        if depth[i] % 2 == 1 {
            if let Some((_, r)) = regions.iter_mut().find(|(o, _)| *o == parent[i]) {
                r.holes.push(oriented(loops[i].clone(), false));
            }
        }
    }
    regions.into_iter().map(|(_, r)| r).collect()
}

/// Drop repeated and collinear points (and a closing duplicate).
pub fn clean_loop(l: Loop) -> Loop {
    let mut out: Loop = Vec::with_capacity(l.len());
    for p in l {
        if out.last().map_or(true, |q: &P2| (q[0] - p[0]).abs() > 1e-9 || (q[1] - p[1]).abs() > 1e-9) {
            out.push(p);
        }
    }
    while out.len() > 1 {
        let (a, b) = (out[0], out[out.len() - 1]);
        if (a[0] - b[0]).abs() <= 1e-9 && (a[1] - b[1]).abs() <= 1e-9 {
            out.pop();
        } else {
            break;
        }
    }
    let mut changed = true;
    while changed && out.len() > 3 {
        changed = false;
        let n = out.len();
        for i in 0..n {
            let (a, b, c) = (out[(i + n - 1) % n], out[i], out[(i + 1) % n]);
            let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
            let scale = ((b[0] - a[0]).hypot(b[1] - a[1]) * (c[0] - b[0]).hypot(c[1] - b[1])).max(1e-30);
            if cross.abs() / scale < 1e-10 && (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]) > 0.0 {
                out.remove(i);
                changed = true;
                break;
            }
        }
    }
    out
}

impl Region {
    pub fn rect(w: f64, h: f64) -> Region {
        Region { outer: vec![[-w / 2.0, -h / 2.0], [w / 2.0, -h / 2.0], [w / 2.0, h / 2.0], [-w / 2.0, h / 2.0]], holes: Vec::new() }
    }

    pub fn area(&self) -> f64 {
        area(&self.outer) - self.holes.iter().map(|h| area(h)).sum::<f64>()
    }

    pub fn loops(&self) -> impl Iterator<Item = &Loop> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }

    pub fn contains(&self, p: P2) -> bool {
        contains(&self.outer, p) && !self.holes.iter().any(|h| contains(h, p))
    }

    pub fn bounds(&self) -> (P2, P2) {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for p in &self.outer {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// The region's points in loop order (outer, then each hole) and its
    /// triangles over them, counter-clockwise seen from +Z.
    pub fn triangulate(&self) -> (Vec<P2>, Vec<[u32; 3]>) {
        let points: Vec<P2> = self.loops().flat_map(|l| l.iter().copied()).collect();
        if points.len() < 3 {
            return (points, Vec::new());
        }
        // A plain convex outline needs no Delaunay: a fan is exact.
        if self.holes.is_empty() && is_strictly_convex(&self.outer) {
            let tris = (1..self.outer.len() as u32 - 1).map(|i| [0, i, i + 1]).collect();
            return (points, tris);
        }
        let (lo, hi) = self.bounds();
        let mut cdt = CDT::new(Point2 { x: lo[0], y: lo[1] }, Point2 { x: hi[0], y: hi[1] });
        let ids: Vec<u32> = points.iter().map(|p| cdt.insert_point(p[0], p[1])).collect();
        let mut base = 0usize;
        for l in self.loops() {
            let n = l.len();
            for i in 0..n {
                cdt.add_constraint(ids[base + i], ids[base + (i + 1) % n]);
            }
            base += n;
        }
        cdt.finalize();
        let mut tris = Vec::new();
        for t in cdt.get_triangles() {
            let (a, b, c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
            let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            if !self.contains(centroid) {
                continue;
            }
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if cross.abs() < 1e-14 {
                continue;
            }
            tris.push(if cross > 0.0 { t } else { [t[0], t[2], t[1]] });
        }
        (points, tris)
    }

    /// Every loop moved by `d` along its outward normal (negative shrinks),
    /// mitred at corners (limited to 4 d). For drafts and clearances on
    /// moderate offsets; a loop that would turn inside out is dropped.
    pub fn offset(&self, d: f64) -> Option<Region> {
        let outer = offset_loop(&self.outer, d)?;
        let holes = self.holes.iter().filter_map(|h| offset_loop(h, d)).collect();
        Some(Region { outer, holes })
    }

    pub fn transformed(&self, f: impl Fn(P2) -> P2) -> Region {
        let mut r = Region { outer: self.outer.iter().map(|p| f(*p)).collect(), holes: self.holes.iter().map(|h| h.iter().map(|p| f(*p)).collect()).collect() };
        // A mirror flips the winding: keep outer counter-clockwise.
        if signed_area2(&r.outer) < 0.0 {
            r.outer.reverse();
            for h in &mut r.holes {
                h.reverse();
            }
        }
        r
    }
}

/// Convex with no straight-through points (a fan over those would make
/// zero-area triangles).
pub fn is_strictly_convex(l: &[P2]) -> bool {
    let n = l.len();
    let mut sign = 0.0f64;
    for i in 0..n {
        let (a, b, c) = (l[i], l[(i + 1) % n], l[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        let scale = ((b[0] - a[0]).hypot(b[1] - a[1]) * (c[0] - b[0]).hypot(c[1] - b[1])).max(1e-30);
        if cross.abs() / scale < 1e-9 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

pub fn is_convex(l: &[P2]) -> bool {
    let n = l.len();
    let mut sign = 0.0f64;
    for i in 0..n {
        let (a, b, c) = (l[i], l[(i + 1) % n], l[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// One loop moved by `d` along its right-hand normal: away from the
/// material for a region's loops, so a positive `d` grows the region (the
/// outer boundary out, holes in) and a negative one shrinks it.
pub fn offset_loop(l: &[P2], d: f64) -> Option<Loop> {
    let n = l.len();
    if n < 3 || d == 0.0 {
        return Some(l.to_vec());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b, c) = (l[(i + n - 1) % n], l[i], l[(i + 1) % n]);
        let n1 = edge_normal(a, b);
        let n2 = edge_normal(b, c);
        let mut m = [n1[0] + n2[0], n1[1] + n2[1]];
        let ml = m[0].hypot(m[1]);
        if ml < 1e-9 {
            m = n1;
        } else {
            m = [m[0] / ml, m[1] / ml];
        }
        let cos_half = (m[0] * n1[0] + m[1] * n1[1]).max(0.25);
        let k = d / cos_half;
        out.push([b[0] + m[0] * k, b[1] + m[1] * k]);
    }
    // Inside out: the winding flipped or the area went the wrong way.
    let before = signed_area2(l);
    let after = signed_area2(&out);
    if before.signum() != after.signum() || after.abs() < 1e-12 {
        return None;
    }
    Some(out)
}

/// The right-hand normal: away from the material for a region's loops
/// (outer counter-clockwise, holes clockwise).
fn edge_normal(a: P2, b: P2) -> P2 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy).max(1e-30);
    [dy / l, -dx / l]
}

/// The loop resampled to `count` points evenly by arc length, starting at
/// the point nearest `start_near` (for lofting profiles together).
pub fn resample(l: &[P2], count: usize, start_near: Option<P2>) -> Loop {
    let n = l.len();
    let mut start = 0;
    if let Some(s) = start_near {
        let mut best = f64::INFINITY;
        for (i, p) in l.iter().enumerate() {
            let d = (p[0] - s[0]).hypot(p[1] - s[1]);
            if d < best {
                best = d;
                start = i;
            }
        }
    }
    let pts: Vec<P2> = (0..n).map(|i| l[(start + i) % n]).collect();
    let mut cum = vec![0.0];
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        cum.push(cum[i] + (b[0] - a[0]).hypot(b[1] - a[1]));
    }
    let total = cum[n];
    let mut out = Vec::with_capacity(count);
    let mut seg = 0;
    for k in 0..count {
        let t = total * k as f64 / count as f64;
        while seg + 1 < n && cum[seg + 1] < t {
            seg += 1;
        }
        let (a, b) = (pts[seg], pts[(seg + 1) % n]);
        let len = cum[seg + 1] - cum[seg];
        let f = if len > 0.0 { (t - cum[seg]) / len } else { 0.0 };
        out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
    }
    out
}
