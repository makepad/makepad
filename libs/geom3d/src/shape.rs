//! 2D shapes (nonzero fill), SVG paths and documents, cap triangulation,
//! flat shapes, extrusion with bevels, and lathes.
//!
//! Shapes are y up in world units. SVG input is y down; it is flipped once
//! on the way in, so a logo reads the right way up facing +z.

use crate::primitives::revolve;
use crate::math::DEG;
use crate::mesh::*;
use makepad_csg_boolean::cdt::{Point2, CDT};
use makepad_svg::path::{PathCmd, VectorPath};
use std::collections::{HashMap, HashSet};

/// A filled 2D region: closed contours under the nonzero rule, so holes
/// are contours wound against their outline (any orientation works when
/// the hole is inside an opposite-wound ring; same-wound overlaps union).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape2D {
    pub contours: Vec<Vec<[f32; 2]>>,
}

impl Shape2D {
    /// An axis-aligned rectangle centred on the origin.
    pub fn rect(w: f32, h: f32) -> Self {
        let (x, y) = (w * 0.5, h * 0.5);
        Self { contours: vec![vec![[-x, -y], [x, -y], [x, y], [-x, y]]] }
    }

    /// A rectangle with corner radius `r`, `segments` per corner.
    pub fn rounded_rect(w: f32, h: f32, r: f32, segments: u32) -> Self {
        let r = r.max(0.0).min(w.abs() * 0.5).min(h.abs() * 0.5);
        if r <= 0.0 {
            return Self::rect(w, h);
        }
        let seg = segments.clamp(1, 64);
        let (x, y) = (w * 0.5 - r, h * 0.5 - r);
        let mut c = Vec::new();
        for (cx, cy, a0) in [(x, -y, -90.0f32), (x, y, 0.0), (-x, y, 90.0), (-x, -y, 180.0)] {
            for i in 0..=seg {
                let a = (a0 + 90.0 * i as f32 / seg as f32) * DEG;
                c.push([cx + r * a.cos(), cy + r * a.sin()]);
            }
        }
        Self { contours: vec![c] }
    }

    pub fn circle(r: f32, segments: u32) -> Self {
        Self::ellipse(r, r, segments)
    }

    pub fn ellipse(rx: f32, ry: f32, segments: u32) -> Self {
        let n = segments.clamp(3, 4096);
        let c = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [rx * a.cos(), ry * a.sin()]
            })
            .collect();
        Self { contours: vec![c] }
    }

    /// A regular polygon with `n` corners on radius `r`, first corner up.
    pub fn polygon(n: u32, r: f32) -> Self {
        let n = n.clamp(3, 4096);
        let c = (0..n)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        Self { contours: vec![c] }
    }

    /// A star with `points` tips on `outer`, valleys on `inner`, tip up.
    pub fn star(points: u32, inner: f32, outer: f32) -> Self {
        let n = points.clamp(2, 1024) * 2;
        let c = (0..n)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
                let r = if i % 2 == 0 { outer } else { inner };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        Self { contours: vec![c] }
    }

    /// SVG path data (`M 0 0 L 10 0 ...`, every command), curves flattened
    /// to within `tolerance` units, y flipped up.
    pub fn from_svg_path(d: &str, tolerance: f32) -> Self {
        let mut path = VectorPath::new();
        makepad_svg::path_data::parse_path_data(d, &mut path);
        let mut shape = Self { contours: flatten_path(&path, tolerance, &|x, y| (x, y)) };
        shape.flip_y();
        shape
    }

    /// Add another shape's contours (a union under the nonzero rule when
    /// both wind the same way).
    pub fn extend(&mut self, other: &Shape2D) {
        self.contours.extend(other.contours.iter().cloned());
    }

    /// Add `hole` wound against the outline, so it cuts.
    pub fn cut(&mut self, hole: &Shape2D) {
        let outer = self.contours.iter().map(|c| shoelace(c)).sum::<f32>();
        for c in &hole.contours {
            let mut c = c.clone();
            if (shoelace(&c) > 0.0) == (outer > 0.0) {
                c.reverse();
            }
            self.contours.push(c);
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.contours.iter_mut().flatten().for_each(|p| {
            p[0] += dx;
            p[1] += dy;
        });
    }

    pub fn scale(&mut self, sx: f32, sy: f32) {
        self.contours.iter_mut().flatten().for_each(|p| {
            p[0] *= sx;
            p[1] *= sy;
        });
    }

    fn flip_y(&mut self) {
        self.contours.iter_mut().flatten().for_each(|p| p[1] = -p[1]);
    }

    /// Bounds `(min, max)`, `None` when empty.
    pub fn bounds(&self) -> Option<([f32; 2], [f32; 2])> {
        let first = *self.contours.iter().flatten().next()?;
        let (mut min, mut max) = (first, first);
        for p in self.contours.iter().flatten() {
            min = [min[0].min(p[0]), min[1].min(p[1])];
            max = [max[0].max(p[0]), max[1].max(p[1])];
        }
        Some((min, max))
    }

    /// Centre the bounds on the origin.
    pub fn centered(mut self) -> Self {
        if let Some((min, max)) = self.bounds() {
            self.translate(-(min[0] + max[0]) * 0.5, -(min[1] + max[1]) * 0.5);
        }
        self
    }
}

/// Every filled element of an SVG document as a shape with its fill colour
/// (straight rgba 0..1, fill-opacity and opacity applied), in document
/// order: paths, rects (rounded), circles, ellipses, polygons. Group and
/// element transforms are applied; y is flipped up. Unfilled elements are
/// skipped; gradient fills take their first stop.
pub fn svg_document_shapes(svg: &str, tolerance: f32) -> Vec<(String, Shape2D, [f32; 4])> {
    use makepad_svg::*;
    let doc = parse_svg(svg);
    let mut out = Vec::new();
    fn fill_of(style: &SvgStyle, doc: &SvgDocument) -> Option<[f32; 4]> {
        let rgba = match style.fill.as_ref()? {
            SvgPaint::None => return None,
            SvgPaint::Color(r, g, b, a) => [*r, *g, *b, *a],
            SvgPaint::CurrentColor => [style.color.0, style.color.1, style.color.2, style.color.3],
            SvgPaint::GradientRef(id) => doc
                .defs
                .gradients
                .get(id)
                .and_then(|g| g.stops.first())
                .map(|s| {
                    // Stops are premultiplied.
                    let a = s.color[3].max(1e-6);
                    [s.color[0] / a, s.color[1] / a, s.color[2] / a, s.color[3]]
                })
                .unwrap_or([0.5, 0.5, 0.5, 1.0]),
        };
        Some([rgba[0], rgba[1], rgba[2], rgba[3] * style.fill_opacity * style.opacity])
    }
    fn walk(nodes: &[SvgNode], parent: &Transform2d, doc: &SvgDocument, tol: f32, out: &mut Vec<(String, Shape2D, [f32; 4])>) {
        for node in nodes {
            let (id, style, local, path): (&Option<String>, &SvgStyle, &Transform2d, Option<VectorPath>) = match node {
                SvgNode::Group(g) => {
                    let t = g.transform.then(parent);
                    walk(&g.children, &t, doc, tol, out);
                    continue;
                }
                SvgNode::Path(p) => (&p.id, &p.style, &p.transform, Some(p.path.clone())),
                SvgNode::Rect(r) => {
                    let mut v = VectorPath::new();
                    if r.rx > 0.0 || r.ry > 0.0 {
                        v.rounded_rect(r.x, r.y, r.width, r.height, r.rx.max(r.ry));
                    } else {
                        v.rect(r.x, r.y, r.width, r.height);
                    }
                    (&r.id, &r.style, &r.transform, Some(v))
                }
                SvgNode::Circle(c) => {
                    let mut v = VectorPath::new();
                    v.circle(c.cx, c.cy, c.r);
                    (&c.id, &c.style, &c.transform, Some(v))
                }
                SvgNode::Ellipse(e) => {
                    let mut v = VectorPath::new();
                    v.ellipse(e.cx, e.cy, e.rx, e.ry);
                    (&e.id, &e.style, &e.transform, Some(v))
                }
                SvgNode::Polygon(p) => {
                    let mut v = VectorPath::new();
                    for (i, (x, y)) in p.points.iter().enumerate() {
                        if i == 0 {
                            v.move_to(*x, *y)
                        } else {
                            v.line_to(*x, *y)
                        }
                    }
                    v.close();
                    (&p.id, &p.style, &p.transform, Some(v))
                }
                _ => continue,
            };
            let (Some(path), Some(fill)) = (path, fill_of(style, doc)) else { continue };
            let t = local.then(parent);
            let scale = t.scale_factor().max(1e-6);
            let mut shape = Shape2D { contours: flatten_path(&path, tol / scale, &|x, y| t.apply(x, y)) };
            shape.flip_y();
            if shape.contours.is_empty() {
                continue;
            }
            let name = id.clone().unwrap_or_else(|| format!("shape{}", out.len()));
            out.push((name, shape, fill));
        }
    }
    walk(&doc.root, &Transform2d::identity(), &doc, tolerance.max(1e-4), &mut out);
    out
}

/// Flatten a vector path into closed contours (every subpath is closed:
/// fills close implicitly), mapping points through `map`.
fn flatten_path(path: &VectorPath, tol: f32, map: &dyn Fn(f32, f32) -> (f32, f32)) -> Vec<Vec<[f32; 2]>> {
    let tol = tol.max(1e-5);
    let mut out = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut pen = (0.0f32, 0.0f32);
    let mut start = pen;
    let push = |cur: &mut Vec<[f32; 2]>, x: f32, y: f32| {
        let (mx, my) = map(x, y);
        cur.push([mx, my]);
    };
    let finish = |cur: &mut Vec<[f32; 2]>, out: &mut Vec<Vec<[f32; 2]>>| {
        if cur.len() >= 3 {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    for cmd in &path.cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) => {
                finish(&mut cur, &mut out);
                push(&mut cur, x, y);
                pen = (x, y);
                start = pen;
            }
            PathCmd::LineTo(x, y) => {
                if cur.is_empty() {
                    push(&mut cur, pen.0, pen.1);
                }
                push(&mut cur, x, y);
                pen = (x, y);
            }
            PathCmd::BezierTo(c1x, c1y, c2x, c2y, x, y) => {
                if cur.is_empty() {
                    push(&mut cur, pen.0, pen.1);
                }
                let p0 = pen;
                // Segment count from the control polygon length.
                let l = dist(p0, (c1x, c1y)) + dist((c1x, c1y), (c2x, c2y)) + dist((c2x, c2y), (x, y));
                let n = ((l / tol).sqrt() * 0.6).ceil().clamp(1.0, 256.0) as u32;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let bx = u * u * u * p0.0 + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * x;
                    let by = u * u * u * p0.1 + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * y;
                    push(&mut cur, bx, by);
                }
                pen = (x, y);
            }
            PathCmd::Close => {
                finish(&mut cur, &mut out);
                pen = start;
            }
            PathCmd::Winding(_) => {}
        }
    }
    finish(&mut cur, &mut out);
    out
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

// ---------------------------------------------------------------------------
// Triangulation.
// ---------------------------------------------------------------------------

/// A shape prepared for meshing: welded, intersection-split contours as
/// `(start, len)` runs of `pts`, and the cap triangles (counter-clockwise).
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub pts: Vec<[f32; 2]>,
    pub contours: Vec<(usize, usize)>,
    pub tris: Vec<[u32; 3]>,
    /// The caps came from the ear-clip fallback (holes lost).
    pub fallback: bool,
}

/// Triangulate a shape's fill: every contour point, every contour edge a
/// constraint, triangles kept where the winding number is nonzero.
/// Returns the points and counter-clockwise triangles.
pub fn triangulate(shape: &Shape2D) -> (Vec<[f32; 2]>, Vec<[u32; 3]>) {
    let p = prepare(shape);
    (p.pts, p.tris)
}

/// [`triangulate`] with the contour table the extruder needs.
pub fn prepare(shape: &Shape2D) -> Prepared {
    let scale = shape.bounds().map(|(a, b)| (b[0] - a[0]).max(b[1] - a[1])).unwrap_or(1.0).max(1e-6);
    let weld_eps = scale * 1e-5;
    let mut rings: Vec<Vec<[f32; 2]>> = shape.contours.iter().filter_map(|c| weld(c, weld_eps)).collect();
    if rings.is_empty() {
        return Prepared::default();
    }
    if rings.iter().map(|r| r.len()).sum::<usize>() <= 20_000 {
        rings = split_intersections(rings, weld_eps);
        rings = rings.into_iter().filter_map(|c| weld(&c, weld_eps)).collect();
    }
    let mut refinements = 0;
    loop {
        let (pts, contours) = assemble(&rings);
        let first = cdt_caps(&pts, &contours, scale, false);
        let missing = match first {
            Ok(t) if !t.is_empty() => return Prepared { pts, contours, tris: t, fallback: false },
            Ok(_) => Vec::new(),
            Err(m) => m,
        };
        if !missing.is_empty() {
            if let Ok(t) = cdt_caps(&pts, &contours, scale, true) {
                if !t.is_empty() {
                    return Prepared { pts, contours, tris: t, fallback: false };
                }
            }
            if refinements < 3 {
                refinements += 1;
                refine(&mut rings, &missing);
                continue;
            }
        }
        let tris = ear_clip_largest(&pts, &contours);
        return Prepared { pts, contours, tris, fallback: true };
    }
}

fn assemble(rings: &[Vec<[f32; 2]>]) -> (Vec<[f32; 2]>, Vec<(usize, usize)>) {
    let mut pts = Vec::new();
    let mut contours = Vec::new();
    for r in rings {
        contours.push((pts.len(), r.len()));
        pts.extend_from_slice(r);
    }
    (pts, contours)
}

/// Drop consecutive near-duplicates and the closing duplicate; `None` for
/// fewer than three points or no area.
fn weld(c: &[[f32; 2]], eps: f32) -> Option<Vec<[f32; 2]>> {
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(c.len());
    for p in c {
        if !p[0].is_finite() || !p[1].is_finite() {
            continue;
        }
        if out.last().map_or(true, |q| (q[0] - p[0]).abs() > eps || (q[1] - p[1]).abs() > eps) {
            out.push(*p);
        }
    }
    while out.len() > 1 {
        let (f, l) = (out[0], out[out.len() - 1]);
        if (f[0] - l[0]).abs() <= eps && (f[1] - l[1]).abs() <= eps {
            out.pop();
        } else {
            break;
        }
    }
    // Extent, not area: a self-crossing ring can have zero net area.
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &out {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    (out.len() >= 3 && max[0] - min[0] > eps && max[1] - min[1] > eps).then_some(out)
}

fn refine(rings: &mut [Vec<[f32; 2]>], edges: &[(usize, usize)]) {
    let mut sorted = edges.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.dedup();
    for (ci, k) in sorted {
        let Some(r) = rings.get_mut(ci) else { continue };
        let n = r.len();
        if k >= n {
            continue;
        }
        let (a, b) = (r[k], r[(k + 1) % n]);
        r.insert(k + 1, [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]);
    }
}

/// Insert a shared point wherever two contour edges cross properly, so the
/// triangulation's constraints never cross.
fn split_intersections(rings: Vec<Vec<[f32; 2]>>, eps: f32) -> Vec<Vec<[f32; 2]>> {
    // (ring, edge) -> list of (t, point).
    let mut cuts: HashMap<(usize, usize), Vec<(f32, [f32; 2])>> = HashMap::new();
    let mut edges = Vec::new();
    for (ri, r) in rings.iter().enumerate() {
        for k in 0..r.len() {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            edges.push((ri, k, a, b, [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]));
        }
    }
    edges.sort_by(|x, y| x.4[0].total_cmp(&y.4[0]));
    for i in 0..edges.len() {
        let (ri, ki, a, b, bi) = edges[i];
        for j in i + 1..edges.len() {
            let (rj, kj, c, d, bj) = edges[j];
            if bj[0] > bi[2] {
                break;
            }
            if bj[1] > bi[3] || bj[3] < bi[1] {
                continue;
            }
            if ri == rj {
                let n = rings[ri].len();
                if ki == kj || (ki + 1) % n == kj || (kj + 1) % n == ki {
                    continue;
                }
            }
            let r = [b[0] - a[0], b[1] - a[1]];
            let s = [d[0] - c[0], d[1] - c[1]];
            let den = r[0] * s[1] - r[1] * s[0];
            if den.abs() < 1e-20 {
                continue;
            }
            let qp = [c[0] - a[0], c[1] - a[1]];
            let t = (qp[0] * s[1] - qp[1] * s[0]) / den;
            let u = (qp[0] * r[1] - qp[1] * r[0]) / den;
            let lr = (r[0] * r[0] + r[1] * r[1]).sqrt().max(1e-20);
            let ls = (s[0] * s[0] + s[1] * s[1]).sqrt().max(1e-20);
            let (et, eu) = (eps / lr, eps / ls);
            if t > et && t < 1.0 - et && u > eu && u < 1.0 - eu {
                let p = [a[0] + r[0] * t, a[1] + r[1] * t];
                cuts.entry((ri, ki)).or_default().push((t, p));
                cuts.entry((rj, kj)).or_default().push((u, p));
            }
        }
    }
    if cuts.is_empty() {
        return rings;
    }
    rings
        .into_iter()
        .enumerate()
        .map(|(ri, r)| {
            let mut out = Vec::with_capacity(r.len());
            for (k, p) in r.iter().enumerate() {
                out.push(*p);
                if let Some(list) = cuts.get_mut(&(ri, k)) {
                    list.sort_by(|x, y| x.0.total_cmp(&y.0));
                    out.extend(list.iter().map(|x| x.1));
                }
            }
            out
        })
        .collect()
}

pub(crate) fn shoelace(c: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..c.len() {
        let (p, q) = (c[i], c[(i + 1) % c.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Nonzero winding number of `p` against the contours.
pub(crate) fn winding(pts: &[[f32; 2]], contours: &[(usize, usize)], p: [f32; 2]) -> i32 {
    let mut w = 0;
    for &(start, len) in contours {
        for k in 0..len {
            let (a, b) = (pts[start + k], pts[start + (k + 1) % len]);
            let is_left = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] {
                if b[1] > p[1] && is_left > 0.0 {
                    w += 1;
                }
            } else if b[1] <= p[1] && is_left < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

fn cross2(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// The verified constrained triangulation; `Err` names the (contour, edge)
/// constraints missing from the result.
fn cdt_caps(pts: &[[f32; 2]], contours: &[(usize, usize)], scale: f32, reverse: bool) -> Result<Vec<[u32; 3]>, Vec<(usize, usize)>> {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in pts {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    let margin = scale * 0.05;
    let mut cdt = CDT::new(
        Point2 { x: (min[0] - margin) as f64, y: (min[1] - margin) as f64 },
        Point2 { x: (max[0] + margin) as f64, y: (max[1] + margin) as f64 },
    );
    // Weld globally: the CDT drops exact duplicates, which would leave
    // their constraints unenforceable.
    let q = 1e5 / scale;
    let mut key_to_uniq: HashMap<(i64, i64), u32> = HashMap::new();
    let mut uniq_pts: Vec<[f32; 2]> = Vec::new();
    let mut uniq_first: Vec<u32> = Vec::new();
    let mut raw_of_uniq: Vec<u32> = Vec::new();
    let mut uniq_of_pt: Vec<u32> = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        let key = ((p[0] * q).round() as i64, (p[1] * q).round() as i64);
        let u = *key_to_uniq.entry(key).or_insert_with(|| {
            let raw = cdt.insert_point(p[0] as f64, p[1] as f64);
            uniq_pts.push(*p);
            uniq_first.push(i as u32);
            raw_of_uniq.push(raw);
            uniq_pts.len() as u32 - 1
        });
        uniq_of_pt.push(u);
    }
    if uniq_pts.len() < 3 {
        return Ok(Vec::new());
    }
    let mut constraints = Vec::new();
    for (ci, &(start, len)) in contours.iter().enumerate() {
        for k in 0..len {
            let (a, b) = (uniq_of_pt[start + k], uniq_of_pt[start + (k + 1) % len]);
            if a != b {
                constraints.push((a.min(b), a.max(b), ci, k));
            }
        }
    }
    let add = |cdt: &mut CDT, c: &(u32, u32, usize, usize)| cdt.add_constraint(raw_of_uniq[c.0 as usize], raw_of_uniq[c.1 as usize]);
    if reverse {
        constraints.iter().rev().for_each(|c| add(&mut cdt, c));
    } else {
        constraints.iter().for_each(|c| add(&mut cdt, c));
    }
    cdt.finalize();
    let tris = cdt.get_triangles();
    let mut edges: HashSet<(u32, u32)> = HashSet::with_capacity(tris.len() * 3);
    let mut mapped = Vec::with_capacity(tris.len());
    for t in &tris {
        if t.iter().any(|&v| v as usize >= uniq_pts.len()) {
            return Ok(Vec::new());
        }
        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            edges.insert((x.min(y), x.max(y)));
        }
        mapped.push(*t);
    }
    let missing: Vec<(usize, usize)> = constraints.iter().filter(|c| !edges.contains(&(c.0, c.1))).map(|c| (c.2, c.3)).collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    let mut out = Vec::with_capacity(mapped.len());
    for [a, b, c] in mapped {
        let (pa, pb, pc) = (uniq_pts[a as usize], uniq_pts[b as usize], uniq_pts[c as usize]);
        let area2 = cross2(pa, pb, pc);
        if area2.abs() < 1e-12 * scale * scale {
            continue;
        }
        let g = [(pa[0] + pb[0] + pc[0]) / 3.0, (pa[1] + pb[1] + pc[1]) / 3.0];
        if winding(pts, contours, g) == 0 {
            continue;
        }
        let (a, b, c) = (uniq_first[a as usize], uniq_first[b as usize], uniq_first[c as usize]);
        out.push(if area2 > 0.0 { [a, b, c] } else { [a, c, b] });
    }
    Ok(out)
}

/// The fallback: the largest contour ear-clipped alone (holes lost).
fn ear_clip_largest(pts: &[[f32; 2]], contours: &[(usize, usize)]) -> Vec<[u32; 3]> {
    let Some(&(start, len)) = contours.iter().max_by(|a, b| shoelace(&pts[a.0..a.0 + a.1]).abs().total_cmp(&shoelace(&pts[b.0..b.0 + b.1]).abs())) else {
        return Vec::new();
    };
    let mut idx: Vec<usize> = (0..len).collect();
    if shoelace(&pts[start..start + len]) < 0.0 {
        idx.reverse();
    }
    let p = |k: usize| pts[start + k];
    let mut tris = Vec::new();
    let mut guard = len * len + 16;
    while idx.len() > 3 && guard > 0 {
        guard -= 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (a, b, c) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let cr = cross2(p(a), p(b), p(c));
            if cr <= 1e-12 {
                continue;
            }
            let blocked = idx.iter().any(|&k| k != a && k != b && k != c && cross2(p(a), p(b), p(k)) >= 0.0 && cross2(p(b), p(c), p(k)) >= 0.0 && cross2(p(c), p(a), p(k)) >= 0.0);
            if blocked {
                continue;
            }
            tris.push([(start + a) as u32, (start + b) as u32, (start + c) as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Drop the flattest corner and go on.
            let n = idx.len();
            let i = (0..n).min_by(|&i, &j| {
                let f = |i: usize| cross2(p(idx[(i + n - 1) % n]), p(idx[i]), p(idx[(i + 1) % n])).abs();
                f(i).total_cmp(&f(j))
            });
            match i {
                Some(i) => {
                    idx.remove(i);
                }
                None => break,
            }
        }
    }
    if idx.len() == 3 && cross2(p(idx[0]), p(idx[1]), p(idx[2])) > 1e-12 {
        tris.push([(start + idx[0]) as u32, (start + idx[1]) as u32, (start + idx[2]) as u32]);
    }
    tris
}

// ---------------------------------------------------------------------------
// Meshes.
// ---------------------------------------------------------------------------

/// A flat filled shape in xy facing +z (three.js ShapeGeometry); uvs are
/// the xy coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeGeo {
    /// The region to fill.
    pub shape: Shape2D,
}

impl ShapeGeo {
    pub fn build(&self) -> Mesh {
        let p = prepare(&self.shape);
        let mut m = Mesh::new();
        for q in &p.pts {
            m.vertex([q[0], q[1], 0.0], [0.0, 0.0, 1.0], *q);
        }
        for t in &p.tris {
            m.tri(t[0], t[1], t[2]);
        }
        m
    }
}

/// A shape extruded along z with optional rounded bevels (three.js
/// ExtrudeGeometry): the back cap at z = -bevel_thickness, the front at
/// depth + bevel_thickness (or centred on z = 0 with `center`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtrudeGeo {
    /// The region to extrude.
    pub shape: Shape2D,
    /// Wall depth along z (without bevels). Default 1.
    pub depth: f32,
    /// How far the bevel reaches outward from the outline. Default 0 (no bevel).
    pub bevel_size: f32,
    /// How far the bevel reaches along z on each side. Default 0.
    pub bevel_thickness: f32,
    /// Rings per bevel (a quarter-round profile). Default 3.
    pub bevel_segments: u32,
    /// Rings along the wall. Default 1.
    pub steps: u32,
    /// Centre the solid on z = 0. Default true.
    pub center: bool,
    /// Corners sharper than this (degrees between edges) get a hard edge. Default 30.
    pub smooth_angle: f32,
}

impl Default for ExtrudeGeo {
    fn default() -> Self {
        Self { shape: Shape2D::default(), depth: 1.0, bevel_size: 0.0, bevel_thickness: 0.0, bevel_segments: 3, steps: 1, center: true, smooth_angle: 30.0 }
    }
}

impl ExtrudeGeo {
    pub fn build(&self) -> Mesh {
        let prep = prepare(&self.shape);
        let mut m = Mesh::new();
        if prep.pts.is_empty() {
            return m;
        }
        let depth = self.depth.max(0.0);
        let bevel = self.bevel_size > 0.0 && self.bevel_thickness > 0.0;
        let (bs, bt) = if bevel { (self.bevel_size, self.bevel_thickness) } else { (0.0, 0.0) };
        let bseg = if bevel { self.bevel_segments.clamp(1, 32) } else { 0 };
        let steps = self.steps.clamp(1, 1024);
        let z0 = if self.center { -(depth * 0.5 + bt) } else { -bt };
        let zoff = z0 + bt;
        // The side profile: (outward offset, z) from the back cap to the front.
        let mut prof: Vec<[f32; 2]> = Vec::new();
        for b in 0..bseg {
            let t = b as f32 / bseg as f32;
            prof.push([bs * (t * std::f32::consts::FRAC_PI_2).sin(), zoff - bt * (t * std::f32::consts::FRAC_PI_2).cos()]);
        }
        for s in 0..=steps {
            prof.push([bs, zoff + depth * s as f32 / steps as f32]);
        }
        for b in (0..bseg).rev() {
            let t = b as f32 / bseg as f32;
            prof.push([bs * (t * std::f32::consts::FRAC_PI_2).sin(), zoff + depth + bt * (t * std::f32::consts::FRAC_PI_2).cos()]);
        }
        // Profile normals in (outward, z): averaged segment normals (dz, -do).
        let seg_n: Vec<[f32; 2]> = prof
            .windows(2)
            .map(|w| {
                let (d_o, dz) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
                let l = (d_o * d_o + dz * dz).sqrt().max(1e-12);
                [dz / l, -d_o / l]
            })
            .collect();
        let prof_n: Vec<[f32; 2]> = (0..prof.len())
            .map(|i| {
                let a = if i > 0 { seg_n[i - 1] } else { seg_n[0] };
                let b = if i < seg_n.len() { seg_n[i] } else { seg_n[seg_n.len() - 1] };
                let s = [a[0] + b[0], a[1] + b[1]];
                let l = (s[0] * s[0] + s[1] * s[1]).sqrt().max(1e-12);
                [s[0] / l, s[1] / l]
            })
            .collect();
        let z_back = prof[0][1];
        let z_front = prof[prof.len() - 1][1];
        // Caps.
        for (z, nz) in [(z_back, -1.0f32), (z_front, 1.0)] {
            let base = m.positions.len() as u32;
            for q in &prep.pts {
                m.vertex([q[0], q[1], z], [0.0, 0.0, nz], *q);
            }
            for t in &prep.tris {
                if nz > 0.0 {
                    m.tri(base + t[0], base + t[1], base + t[2]);
                } else {
                    m.tri(base + t[0], base + t[2], base + t[1]);
                }
            }
        }
        // Per edge: which side is ink (+1 right-hand normal points out).
        let scale = self.shape.bounds().map(|(a, b)| (b[0] - a[0]).max(b[1] - a[1])).unwrap_or(1.0).max(1e-6);
        let pts = &prep.pts;
        let mut out_sign = vec![0.0f32; pts.len()];
        let mut edge_n = vec![[0.0f32; 2]; pts.len()];
        for &(start, len) in &prep.contours {
            for k in 0..len {
                let (a, b) = (pts[start + k], pts[start + (k + 1) % len]);
                let e = [b[0] - a[0], b[1] - a[1]];
                let l = (e[0] * e[0] + e[1] * e[1]).sqrt().max(1e-12);
                let right = [e[1] / l, -e[0] / l];
                let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let d = (scale * 1e-3).min(0.2 * l);
                let ink_right = winding(pts, &prep.contours, [mid[0] + right[0] * d, mid[1] + right[1] * d]) != 0;
                let ink_left = winding(pts, &prep.contours, [mid[0] - right[0] * d, mid[1] - right[1] * d]) != 0;
                let s = match (ink_left, ink_right) {
                    (true, false) => 1.0,
                    (false, true) => -1.0,
                    _ => 0.0,
                };
                out_sign[start + k] = s;
                edge_n[start + k] = [right[0] * s, right[1] * s];
            }
        }
        let cos_smooth = (self.smooth_angle.clamp(0.0, 180.0) * DEG).cos();
        for &(start, len) in &prep.contours {
            let prev = |k: usize| start + (k + len - 1) % len;
            // Per vertex: the mitred offset direction and the smoothed normal.
            let mut mitre = vec![[0.0f32; 2]; len];
            let mut smooth_n = vec![[0.0f32; 2]; len];
            let mut smooth_ok = vec![false; len];
            for k in 0..len {
                let (np, nn) = (edge_n[prev(k)], edge_n[start + k]);
                let (sp, sn) = (out_sign[prev(k)] != 0.0, out_sign[start + k] != 0.0);
                let avg = match (sp, sn) {
                    (true, true) => [np[0] + nn[0], np[1] + nn[1]],
                    (true, false) => np,
                    (false, true) => nn,
                    _ => [0.0, 0.0],
                };
                let l = (avg[0] * avg[0] + avg[1] * avg[1]).sqrt();
                if l > 1e-9 {
                    let u = [avg[0] / l, avg[1] / l];
                    let c = (u[0] * nn[0] + u[1] * nn[1]).max(0.35);
                    mitre[k] = [u[0] / c, u[1] / c];
                    smooth_n[k] = u;
                }
                smooth_ok[k] = sp && sn && (np[0] * nn[0] + np[1] * nn[1]) >= cos_smooth;
            }
            for k in 0..len {
                let e = start + k;
                if out_sign[e] == 0.0 {
                    continue;
                }
                let (ka, kb) = (k, (k + 1) % len);
                let (a, b) = (pts[start + ka], pts[start + kb]);
                let n_at = |kk: usize| if smooth_ok[kk] { smooth_n[kk] } else { edge_n[e] };
                let (na, nb) = (n_at(ka), n_at(kb));
                let horizontal = (a[1] - b[1]).abs() < (a[0] - b[0]).abs();
                let base = m.positions.len() as u32;
                for (r, pr) in prof.iter().enumerate() {
                    let pn = prof_n[r];
                    for (q, kk, n2) in [(a, ka, na), (b, kb, nb)] {
                        let p = [q[0] + mitre[kk][0] * pr[0], q[1] + mitre[kk][1] * pr[0], pr[1]];
                        let n = normalize_or_up([n2[0] * pn[0], n2[1] * pn[0], pn[1]]);
                        let uv = if horizontal { [p[0], 1.0 - p[2]] } else { [p[1], 1.0 - p[2]] };
                        m.vertex(p, n, uv);
                    }
                }
                for r in 0..prof.len() as u32 - 1 {
                    let (a0, b0, a1, b1) = (base + 2 * r, base + 2 * r + 1, base + 2 * r + 2, base + 2 * r + 3);
                    if out_sign[e] > 0.0 {
                        m.quad(a0, b0, b1, a1);
                    } else {
                        m.quad(a0, a1, b1, b0);
                    }
                }
            }
        }
        m
    }
}

/// A profile revolved around y (three.js LatheGeometry): `points` are
/// (radius, y) from bottom to top.
#[derive(Clone, Debug, PartialEq)]
pub struct LatheGeo {
    /// The profile, (distance from the y axis, height). Default a vase-like curve.
    pub points: Vec<[f32; 2]>,
    /// Segments around. Default 32.
    pub segments: u32,
    /// Start of the sweep, degrees. Default 0.
    pub phi_start: f32,
    /// Sweep, degrees. Default 360.
    pub phi_length: f32,
}

impl Default for LatheGeo {
    fn default() -> Self {
        Self { points: vec![[0.0, -0.5], [0.5, -0.5], [0.35, 0.0], [0.5, 0.5], [0.0, 0.5]], segments: 32, phi_start: 0.0, phi_length: 360.0 }
    }
}

impl LatheGeo {
    pub fn build(&self) -> Mesh {
        let pts: Vec<[f32; 2]> = self.points.iter().map(|p| [p[0].max(0.0), p[1]]).collect();
        if pts.len() < 2 {
            return Mesh::new();
        }
        let seg_n: Vec<[f32; 2]> = pts
            .windows(2)
            .map(|w| {
                let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
                let l = (dx * dx + dy * dy).sqrt().max(1e-12);
                [dy / l, -dx / l]
            })
            .collect();
        let rings: Vec<[f32; 4]> = (0..pts.len())
            .map(|i| {
                let a = seg_n[i.saturating_sub(1).min(seg_n.len() - 1)];
                let b = seg_n[i.min(seg_n.len() - 1)];
                let s = [a[0] + b[0], a[1] + b[1]];
                let l = (s[0] * s[0] + s[1] * s[1]).sqrt().max(1e-12);
                [pts[i][0], pts[i][1], s[0] / l, s[1] / l]
            })
            .collect();
        let vs: Vec<f32> = (0..pts.len()).map(|j| j as f32 / (pts.len() - 1) as f32).collect();
        revolve(&rings, &vs, self.segments.clamp(3, 4096), self.phi_start * DEG, self.phi_length * DEG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[[f32; 2]], tris: &[[u32; 3]]) -> f32 {
        tris.iter().map(|t| cross2(pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]) * 0.5).sum()
    }

    #[test]
    fn triangles_are_ccw_and_cover_the_area() {
        let (pts, tris) = triangulate(&Shape2D::star(5, 0.4, 1.0));
        assert!(tris.iter().all(|t| cross2(pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]) > 0.0));
        let expect = shoelace(&Shape2D::star(5, 0.4, 1.0).contours[0]).abs();
        assert!((area(&pts, &tris) - expect).abs() < 1e-4);
    }

    #[test]
    fn hole_is_kept() {
        let mut s = Shape2D::rect(2.0, 2.0);
        s.cut(&Shape2D::circle(0.5, 32));
        let (pts, tris) = triangulate(&s);
        let expect = 4.0 - shoelace(&Shape2D::circle(0.5, 32).contours[0]).abs();
        assert!((area(&pts, &tris) - expect).abs() < 1e-3, "{}", area(&pts, &tris));
        // No triangle centroid inside the hole.
        for t in &tris {
            let g = [0, 1].map(|k| (pts[t[0] as usize][k] + pts[t[1] as usize][k] + pts[t[2] as usize][k]) / 3.0);
            assert!(g[0] * g[0] + g[1] * g[1] > 0.2);
        }
    }

    #[test]
    fn self_intersecting_bowtie() {
        let s = Shape2D { contours: vec![vec![[-1.0, -1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, 1.0]]] };
        let (pts, tris) = triangulate(&s);
        assert!((area(&pts, &tris) - 2.0).abs() < 1e-3, "{}", area(&pts, &tris));
    }

    #[test]
    fn svg_path_flattens_and_flips() {
        let s = Shape2D::from_svg_path("M0 0 L10 0 C10 5 5 10 0 10 Z", 0.05);
        assert_eq!(s.contours.len(), 1);
        let (min, max) = s.bounds().unwrap();
        assert!((min[1] + 10.0).abs() < 1e-4 && max[1].abs() < 1e-4 && (max[0] - 10.0).abs() < 1e-4);
        assert!(s.contours[0].len() > 8);
        // An "O": two subpaths, the inner one opposite.
        let o = Shape2D::from_svg_path("M0 0 H10 V10 H0 Z M3 3 V7 H7 V3 Z", 0.1);
        let (pts, tris) = triangulate(&o);
        assert!((area(&pts, &tris) - 84.0).abs() < 1e-3);
    }

    #[test]
    fn svg_document_fills_and_transforms() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <g transform="translate(10 20)"><rect id="a" x="0" y="0" width="10" height="5" fill="#ff0000"/></g>
            <circle cx="50" cy="50" r="10" fill="none" stroke="#000"/>
            <path d="M0 0 L5 0 L0 5 Z" fill="#00ff00" fill-opacity="0.5"/>
        </svg>"##;
        let shapes = svg_document_shapes(svg, 0.1);
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].0, "a");
        assert_eq!(shapes[0].2, [1.0, 0.0, 0.0, 1.0]);
        let (min, max) = shapes[0].1.bounds().unwrap();
        assert!((min[0] - 10.0).abs() < 1e-4 && (max[1] + 20.0).abs() < 1e-4, "{min:?} {max:?}");
        assert!((shapes[1].2[3] - 0.5).abs() < 1e-4);
    }

    fn closed_outward(m: &Mesh) {
        m.validate().unwrap();
        // Signed volume positive: faces wound outward overall.
        let mut vol = 0.0;
        for t in m.indices.chunks_exact(3) {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            vol += dot3(a, cross3(b, c)) / 6.0;
        }
        assert!(vol > 0.0, "volume {vol}");
        // Vertex normals agree with the faces they belong to.
        for t in m.indices.chunks_exact(3) {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            let n = tri_normal_unnormalized(a, b, c);
            if len3(n) < 1e-10 {
                continue;
            }
            for &i in t {
                assert!(dot3(n, m.normals[i as usize]) > 0.0, "normal against face at {:?}", m.positions[i as usize]);
            }
        }
    }

    #[test]
    fn extrude_box_volume() {
        let m = ExtrudeGeo { shape: Shape2D::rect(2.0, 1.0), depth: 3.0, ..Default::default() }.build();
        closed_outward(&m);
        let mut vol = 0.0;
        for t in m.indices.chunks_exact(3) {
            vol += dot3(m.positions[t[0] as usize], cross3(m.positions[t[1] as usize], m.positions[t[2] as usize])) / 6.0;
        }
        assert!((vol - 6.0).abs() < 1e-4);
        let (min, max) = m.bounds().unwrap();
        assert!((min[2] + 1.5).abs() < 1e-6 && (max[2] - 1.5).abs() < 1e-6);
    }

    #[test]
    fn extrude_with_hole_and_bevel() {
        let mut s = Shape2D::rect(2.0, 2.0);
        s.cut(&Shape2D::rect(1.0, 1.0));
        // Either winding of the outline works.
        for flip in [false, true] {
            let mut s = s.clone();
            if flip {
                s.contours.iter_mut().for_each(|c| c.reverse());
            }
            let m = ExtrudeGeo { shape: s, depth: 0.5, bevel_size: 0.05, bevel_thickness: 0.05, bevel_segments: 3, ..Default::default() }.build();
            closed_outward(&m);
            // Hole walls face into the hole: a wall vertex on x = +0.5 inside has normal -x.
            // Hole walls face into the hole: the wall of the hole's x = +0.5
            // edge (moved 0.05 into the hole by the bevel) faces -x.
            let near = |p: &[f32; 3]| (p[0] - 0.45).abs() < 1e-4 && p[1].abs() < 0.46;
            let walls: Vec<_> = m.positions.iter().zip(&m.normals).filter(|(p, n)| near(p) && n[2].abs() < 0.2).collect();
            assert!(walls.iter().any(|(_, n)| n[0] < -0.9), "{walls:?}");
            assert!(walls.iter().all(|(_, n)| n[0] < 0.1), "{walls:?}");
            let (min, max) = m.bounds().unwrap();
            assert!((max[0] - 1.05).abs() < 1e-4 && (max[2] - 0.3).abs() < 1e-4 && (min[2] + 0.3).abs() < 1e-4);
        }
    }

    #[test]
    fn extrude_star_hard_corners() {
        let m = ExtrudeGeo { shape: Shape2D::star(5, 0.4, 1.0), depth: 0.2, ..Default::default() }.build();
        closed_outward(&m);
        // A circle wall is smooth: normals are radial.
        let c = ExtrudeGeo { shape: Shape2D::circle(1.0, 48), depth: 0.2, ..Default::default() }.build();
        for (p, n) in c.positions.iter().zip(&c.normals) {
            if n[2].abs() < 0.1 {
                assert!(dot3(*n, normalize_or_up([p[0], p[1], 0.0])) > 0.999);
            }
        }
    }

    #[test]
    fn shape_geo_faces_z() {
        let m = ShapeGeo { shape: Shape2D::rounded_rect(2.0, 1.0, 0.3, 4) }.build();
        m.validate().unwrap();
        for t in m.indices.chunks_exact(3) {
            assert!(tri_normal_unnormalized(m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize])[2] > 0.0);
        }
    }

    #[test]
    fn lathe_outward() {
        let m = LatheGeo { points: vec![[0.0, -1.0], [1.0, -0.5], [1.0, 0.5], [0.0, 1.0]], segments: 16, ..Default::default() }.build();
        closed_outward(&m);
        assert_eq!(m.vertex_count(), 17 * 4);
    }
}
