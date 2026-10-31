//! 2D sketches: closed shapes (rectangles, circles, slots, polygons) and
//! paths built segment by segment (lines, horizontal/vertical/polar moves,
//! arcs through a point or with a radius, tangent arcs, splines), with
//! fillets and chamfers at corners. "Constraints-lite": every segment starts
//! where the last one ended (coincidence), `h`/`v` moves are horizontal or
//! vertical, a tangent arc continues the last direction, and dimensions are
//! plain numbers (formulas over parameters in the program that holds them).
//!
//! Curves are flattened to a chord tolerance; a full circle gets at least
//! `min_segments` points.

use crate::region::{clean_loop, regions_from_loops, Loop, Region, P2};
use std::f64::consts::{PI, TAU};

/// How finely curves are flattened.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Largest distance between the curve and its chords (mm).
    pub chord: f64,
    /// Fewest points on a full circle.
    pub min_segments: usize,
    /// Most points on a full circle.
    pub max_segments: usize,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance { chord: 0.02, min_segments: 24, max_segments: 512 }
    }
}

impl Tolerance {
    /// Segments for an arc of `radius` spanning `angle` radians.
    pub fn segments(&self, radius: f64, angle: f64) -> usize {
        let r = radius.abs().max(1e-9);
        let step = if self.chord >= r { PI / 2.0 } else { 2.0 * (1.0 - self.chord / r).acos() };
        let full = ((TAU / step.max(1e-6)).ceil() as usize).clamp(self.min_segments, self.max_segments);
        ((full as f64 * angle.abs() / TAU).ceil() as usize).max(1)
    }
}

/// One segment of a path, from the current point.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    /// A line to a point.
    Line(P2),
    /// A line by an offset.
    Rel(P2),
    /// Horizontal by dx.
    H(f64),
    /// Vertical by dy.
    V(f64),
    /// A line of `len` at `deg` degrees (0 = +X, counter-clockwise).
    Polar { len: f64, deg: f64 },
    /// An arc to `to` passing through `through`.
    ArcThrough { through: P2, to: P2 },
    /// An arc to `to` of radius `r` (positive: counter-clockwise, the
    /// shorter arc; `large` takes the longer one).
    ArcRadius { to: P2, r: f64, large: bool },
    /// An arc to `to` tangent to the previous segment.
    Tangent(P2),
    /// A smooth curve through these points (Catmull-Rom), ending at the last.
    Spline(Vec<P2>),
    /// Round the corner at the current point with this radius.
    Fillet(f64),
    /// Cut the corner at the current point back by this distance on each side.
    Chamfer(f64),
}

/// A closed shape of a sketch.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect { center: P2, size: P2, corner: f64, chamfer: f64 },
    Circle { center: P2, r: f64 },
    Ellipse { center: P2, rx: f64, ry: f64 },
    /// A stadium: two half circles round the segment a-b.
    Slot { a: P2, b: P2, r: f64 },
    /// A regular polygon with `sides` sides and this circumradius.
    Regular { center: P2, r: f64, sides: usize, deg: f64 },
    /// A polygon with the same fillet radius at every corner (0: sharp).
    Polygon { points: Vec<P2>, fillet: f64 },
    /// A path from `start`; closed back to `start`.
    Path { start: P2, segs: Vec<Seg> },
}

/// A sketch: shapes combined by nesting (a shape inside another is a hole,
/// a shape inside a hole is an island again).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sketch {
    pub shapes: Vec<Shape>,
}

impl Sketch {
    pub fn new(shapes: Vec<Shape>) -> Sketch {
        Sketch { shapes }
    }

    pub fn loops(&self, tol: &Tolerance) -> Result<Vec<Loop>, String> {
        self.shapes.iter().enumerate().map(|(i, s)| shape_loop(s, tol).map_err(|e| format!("shape {i}: {e}"))).collect()
    }

    pub fn regions(&self, tol: &Tolerance) -> Result<Vec<Region>, String> {
        let regions = regions_from_loops(self.loops(tol)?);
        if regions.is_empty() {
            return Err("the sketch encloses no area".into());
        }
        Ok(regions)
    }
}

pub fn circle_loop(center: P2, r: f64, tol: &Tolerance) -> Loop {
    let n = tol.segments(r, TAU).max(3);
    (0..n).map(|i| {
        let a = TAU * i as f64 / n as f64;
        [center[0] + r * a.cos(), center[1] + r * a.sin()]
    }).collect()
}

pub fn shape_loop(shape: &Shape, tol: &Tolerance) -> Result<Loop, String> {
    let l = match shape {
        Shape::Rect { center, size, corner, chamfer } => {
            let (w, h) = (size[0].abs(), size[1].abs());
            if w <= 0.0 || h <= 0.0 {
                return Err("a rect needs a positive size".into());
            }
            let (x0, y0, x1, y1) = (center[0] - w / 2.0, center[1] - h / 2.0, center[0] + w / 2.0, center[1] + h / 2.0);
            let pts = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            if *corner > 0.0 {
                round_corners(&pts, &[*corner; 4], tol)?
            } else if *chamfer > 0.0 {
                chamfer_corners(&pts, &[*chamfer; 4])?
            } else {
                pts
            }
        }
        Shape::Circle { center, r } => {
            if *r <= 0.0 {
                return Err("a circle needs a positive radius".into());
            }
            circle_loop(*center, *r, tol)
        }
        Shape::Ellipse { center, rx, ry } => {
            if *rx <= 0.0 || *ry <= 0.0 {
                return Err("an ellipse needs positive radii".into());
            }
            let n = tol.segments(rx.max(*ry), TAU).max(8);
            (0..n).map(|i| {
                let a = TAU * i as f64 / n as f64;
                [center[0] + rx * a.cos(), center[1] + ry * a.sin()]
            }).collect()
        }
        Shape::Slot { a, b, r } => {
            if *r <= 0.0 {
                return Err("a slot needs a positive radius".into());
            }
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let base = dy.atan2(dx);
            let n = tol.segments(*r, PI).max(4);
            let mut out = Vec::new();
            for i in 0..=n {
                let t = base - PI / 2.0 + PI * i as f64 / n as f64;
                out.push([b[0] + r * t.cos(), b[1] + r * t.sin()]);
            }
            for i in 0..=n {
                let t = base + PI / 2.0 + PI * i as f64 / n as f64;
                out.push([a[0] + r * t.cos(), a[1] + r * t.sin()]);
            }
            out
        }
        Shape::Regular { center, r, sides, deg } => {
            if *sides < 3 || *r <= 0.0 {
                return Err("a regular polygon needs 3 or more sides and a positive radius".into());
            }
            (0..*sides).map(|i| {
                let a = deg.to_radians() + TAU * i as f64 / *sides as f64;
                [center[0] + r * a.cos(), center[1] + r * a.sin()]
            }).collect()
        }
        Shape::Polygon { points, fillet } => {
            if points.len() < 3 {
                return Err("a polygon needs 3 or more points".into());
            }
            if *fillet > 0.0 {
                round_corners(points, &vec![*fillet; points.len()], tol)?
            } else {
                points.clone()
            }
        }
        Shape::Path { start, segs } => path_loop(*start, segs, tol)?,
    };
    let l = clean_loop(l);
    if l.len() < 3 {
        return Err("the shape has no area".into());
    }
    if self_intersects(&l) {
        return Err("the outline crosses itself".into());
    }
    Ok(l)
}

/// The corners of `pts` rounded with `radii` (0 keeps a corner sharp).
pub fn round_corners(pts: &[P2], radii: &[f64], tol: &Tolerance) -> Result<Loop, String> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let r = radii.get(i).copied().unwrap_or(0.0);
        if r <= 0.0 {
            out.push(b);
            continue;
        }
        out.extend(fillet_arc(a, b, c, r, tol).ok_or_else(|| format!("corner {i}: a {r} mm fillet does not fit"))?);
    }
    Ok(out)
}

pub fn chamfer_corners(pts: &[P2], cuts: &[f64]) -> Result<Loop, String> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let d = cuts.get(i).copied().unwrap_or(0.0);
        if d <= 0.0 {
            out.push(b);
            continue;
        }
        let (la, lc) = (dist(a, b), dist(b, c));
        if d >= la * 0.5 + 1e-9 || d >= lc * 0.5 + 1e-9 {
            return Err(format!("corner {i}: a {d} mm chamfer does not fit"));
        }
        out.push(lerp(b, a, d / la));
        out.push(lerp(b, c, d / lc));
    }
    Ok(out)
}

fn dist(a: P2, b: P2) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

fn lerp(a: P2, b: P2, t: f64) -> P2 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The arc rounding corner b (between a-b and b-c) with radius r, as points
/// from the tangent point on a-b to the one on b-c; `None` when it does not
/// fit (each tangent point must stay within half of its edge).
fn fillet_arc(a: P2, b: P2, c: P2, r: f64, tol: &Tolerance) -> Option<Vec<P2>> {
    let (la, lc) = (dist(a, b), dist(b, c));
    if la < 1e-12 || lc < 1e-12 {
        return None;
    }
    let u = [(a[0] - b[0]) / la, (a[1] - b[1]) / la];
    let w = [(c[0] - b[0]) / lc, (c[1] - b[1]) / lc];
    let cos = (u[0] * w[0] + u[1] * w[1]).clamp(-1.0, 1.0);
    let theta = cos.acos(); // interior angle at b
    if theta < 1e-6 || (PI - theta) < 1e-6 {
        return Some(vec![b]); // straight or folded back: nothing to round
    }
    let t = r / (theta / 2.0).tan();
    if t > la * 0.5 + 1e-9 || t > lc * 0.5 + 1e-9 {
        return None;
    }
    let p0 = [b[0] + u[0] * t, b[1] + u[1] * t];
    let p1 = [b[0] + w[0] * t, b[1] + w[1] * t];
    let bis = [u[0] + w[0], u[1] + w[1]];
    let bl = bis[0].hypot(bis[1]);
    let h = r / (theta / 2.0).sin();
    let center = [b[0] + bis[0] / bl * h, b[1] + bis[1] / bl * h];
    let a0 = (p0[1] - center[1]).atan2(p0[0] - center[0]);
    let a1 = (p1[1] - center[1]).atan2(p1[0] - center[0]);
    let mut sweep = a1 - a0;
    while sweep > PI {
        sweep -= TAU;
    }
    while sweep < -PI {
        sweep += TAU;
    }
    let n = tol.segments(r, sweep.abs()).max(1);
    Some((0..=n).map(|k| {
        let ang = a0 + sweep * k as f64 / n as f64;
        [center[0] + r * ang.cos(), center[1] + r * ang.sin()]
    }).collect())
}

/// Points of an arc from p0 around `center` to p1, counter-clockwise when
/// `ccw`; excludes p0, includes p1.
fn arc_points(p0: P2, p1: P2, center: P2, ccw: bool, tol: &Tolerance) -> Vec<P2> {
    let r = dist(p0, center);
    let a0 = (p0[1] - center[1]).atan2(p0[0] - center[0]);
    let a1 = (p1[1] - center[1]).atan2(p1[0] - center[0]);
    let mut sweep = a1 - a0;
    if ccw {
        while sweep <= 0.0 {
            sweep += TAU;
        }
    } else {
        while sweep >= 0.0 {
            sweep -= TAU;
        }
    }
    let n = tol.segments(r, sweep.abs()).max(1);
    let mut out: Vec<P2> = (1..n).map(|k| {
        let ang = a0 + sweep * k as f64 / n as f64;
        [center[0] + r * ang.cos(), center[1] + r * ang.sin()]
    }).collect();
    out.push(p1);
    out
}

fn circumcenter(a: P2, b: P2, c: P2) -> Option<P2> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (a[0] * a[0] + a[1] * a[1], b[0] * b[0] + b[1] * b[1], c[0] * c[0] + c[1] * c[1]);
    Some([(a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d, (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d])
}

/// A path's points, closed. Fillets and chamfers apply to the corner at the
/// point where they stand (also at the start, when they come last).
fn path_loop(start: P2, segs: &[Seg], tol: &Tolerance) -> Result<Loop, String> {
    // Corners: (index in `pts`, radius or chamfer) applied after the path is laid out.
    let mut pts: Vec<P2> = vec![start];
    let mut corners: Vec<(usize, bool, f64)> = Vec::new();
    let mut dir: Option<P2> = None;
    for (i, seg) in segs.iter().enumerate() {
        let cur = *pts.last().unwrap();
        let line = |to: P2, pts: &mut Vec<P2>, dir: &mut Option<P2>| {
            let l = dist(cur, to);
            if l > 1e-12 {
                *dir = Some([(to[0] - cur[0]) / l, (to[1] - cur[1]) / l]);
            }
            pts.push(to);
        };
        match seg {
            Seg::Line(to) => line(*to, &mut pts, &mut dir),
            Seg::Rel(d) => line([cur[0] + d[0], cur[1] + d[1]], &mut pts, &mut dir),
            Seg::H(dx) => line([cur[0] + dx, cur[1]], &mut pts, &mut dir),
            Seg::V(dy) => line([cur[0], cur[1] + dy], &mut pts, &mut dir),
            Seg::Polar { len, deg } => {
                let a = deg.to_radians();
                line([cur[0] + len * a.cos(), cur[1] + len * a.sin()], &mut pts, &mut dir)
            }
            Seg::ArcThrough { through, to } => {
                let c = circumcenter(cur, *through, *to).ok_or_else(|| format!("segment {i}: the arc's points are in a line"))?;
                // cur -> through -> to turning left is a counter-clockwise arc.
                let ccw = (through[0] - cur[0]) * (to[1] - cur[1]) - (through[1] - cur[1]) * (to[0] - cur[0]) > 0.0;
                pts.extend(arc_points(cur, *to, c, ccw, tol));
                dir = Some(arc_end_dir(c, *to, ccw));
            }
            Seg::ArcRadius { to, r, large } => {
                let d = dist(cur, *to);
                let rr = r.abs();
                if d < 1e-12 || rr < d / 2.0 - 1e-9 {
                    return Err(format!("segment {i}: a radius {rr} arc cannot span {d:.3} mm"));
                }
                let ccw = *r > 0.0;
                let m = [(cur[0] + to[0]) / 2.0, (cur[1] + to[1]) / 2.0];
                let h = (rr * rr - d * d / 4.0).max(0.0).sqrt();
                let n = [-(to[1] - cur[1]) / d, (to[0] - cur[0]) / d];
                // The shorter counter-clockwise arc has its centre on the left.
                let side = if ccw != *large { 1.0 } else { -1.0 };
                let c = [m[0] + n[0] * h * side, m[1] + n[1] * h * side];
                pts.extend(arc_points(cur, *to, c, ccw, tol));
                dir = Some(arc_end_dir(c, *to, ccw));
            }
            Seg::Tangent(to) => {
                let t = dir.ok_or_else(|| format!("segment {i}: a tangent arc needs a segment before it"))?;
                // Centre on the normal of t through cur, equidistant to cur and to.
                let n = [-t[1], t[0]];
                let v = [to[0] - cur[0], to[1] - cur[1]];
                let denom = 2.0 * (v[0] * n[0] + v[1] * n[1]);
                if denom.abs() < 1e-12 {
                    line(*to, &mut pts, &mut dir);
                    continue;
                }
                let s = (v[0] * v[0] + v[1] * v[1]) / denom;
                let c = [cur[0] + n[0] * s, cur[1] + n[1] * s];
                let ccw = s > 0.0;
                pts.extend(arc_points(cur, *to, c, ccw, tol));
                dir = Some(arc_end_dir(c, *to, ccw));
            }
            Seg::Spline(points) => {
                if points.is_empty() {
                    continue;
                }
                let mut ctrl = vec![cur];
                ctrl.extend(points.iter().copied());
                let samples = catmull_rom(&ctrl, tol);
                if samples.len() >= 2 {
                    let (a, b) = (samples[samples.len() - 2], samples[samples.len() - 1]);
                    let l = dist(a, b).max(1e-12);
                    dir = Some([(b[0] - a[0]) / l, (b[1] - a[1]) / l]);
                }
                pts.extend(samples.into_iter().skip(1));
            }
            Seg::Fillet(r) => corners.push((pts.len() - 1, true, *r)),
            Seg::Chamfer(d) => corners.push((pts.len() - 1, false, *d)),
        }
    }
    // Back at the start: the closing point is the start itself.
    if pts.len() > 1 && dist(pts[0], *pts.last().unwrap()) < 1e-9 {
        let last = pts.len() - 1;
        for c in &mut corners {
            if c.0 == last {
                c.0 = 0;
            }
        }
        pts.pop();
    }
    if corners.is_empty() {
        return Ok(pts);
    }
    let n = pts.len();
    let mut radii = vec![0.0; n];
    let mut cuts = vec![0.0; n];
    for (at, round, v) in corners {
        if round {
            radii[at % n] = v;
        } else {
            cuts[at % n] = v;
        }
    }
    let rounded = if radii.iter().any(|r| *r > 0.0) { round_corners_keep(&pts, &radii, tol)? } else { pts.iter().map(|p| (vec![*p], 0usize)).collect() };
    // Chamfers on the corners that were not rounded.
    let mut out = Vec::new();
    for (i, (arc, _)) in rounded.iter().enumerate() {
        if cuts[i] > 0.0 && arc.len() == 1 {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let d = cuts[i];
            let (la, lc) = (dist(a, b), dist(b, c));
            if d >= la * 0.5 + 1e-9 || d >= lc * 0.5 + 1e-9 {
                return Err(format!("a {d} mm chamfer does not fit at point {i}"));
            }
            out.push(lerp(b, a, d / la));
            out.push(lerp(b, c, d / lc));
        } else {
            out.extend(arc.iter().copied());
        }
    }
    Ok(out)
}

fn round_corners_keep(pts: &[P2], radii: &[f64], tol: &Tolerance) -> Result<Vec<(Vec<P2>, usize)>, String> {
    let n = pts.len();
    (0..n).map(|i| {
        let r = radii[i];
        if r <= 0.0 {
            return Ok((vec![pts[i]], i));
        }
        let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        fillet_arc(a, b, c, r, tol).map(|arc| (arc, i)).ok_or_else(|| format!("a {r} mm fillet does not fit at point {i}"))
    }).collect()
}

fn arc_end_dir(center: P2, end: P2, ccw: bool) -> P2 {
    let r = [end[0] - center[0], end[1] - center[1]];
    let l = r[0].hypot(r[1]).max(1e-12);
    if ccw {
        [-r[1] / l, r[0] / l]
    } else {
        [r[1] / l, -r[0] / l]
    }
}

/// A Catmull-Rom curve through `pts` (ends clamped), flattened.
fn catmull_rom(pts: &[P2], tol: &Tolerance) -> Vec<P2> {
    let n = pts.len();
    if n < 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    for i in 0..n - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = if i + 2 < n { pts[i + 2] } else { pts[n - 1] };
        let len = dist(p1, p2);
        let steps = ((len / (tol.chord * 40.0).max(0.05)).ceil() as usize).clamp(4, 64);
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f64, b: f64, c: f64, d: f64| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push([f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]);
        }
    }
    out
}

/// Whether two non-adjacent edges of the loop cross.
pub fn self_intersects(l: &[P2]) -> bool {
    let n = l.len();
    if n < 4 {
        return false;
    }
    // Sort edges by min x and sweep: enough for the few thousand points a
    // sketch or a glyph has.
    let mut edges: Vec<(f64, f64, usize)> = (0..n).map(|i| {
        let (a, b) = (l[i], l[(i + 1) % n]);
        (a[0].min(b[0]), a[0].max(b[0]), i)
    }).collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (k, &(_, hi, i)) in edges.iter().enumerate() {
        for &(lo2, _, j) in &edges[k + 1..] {
            if lo2 > hi {
                break;
            }
            let adjacent = i == j || (i + 1) % n == j || (j + 1) % n == i;
            if adjacent {
                continue;
            }
            if segments_cross(l[i], l[(i + 1) % n], l[j], l[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

fn segments_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
    let o = |p: P2, q: P2, r: P2| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}
