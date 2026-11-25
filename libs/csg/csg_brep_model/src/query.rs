//! Exact questions about a body.

use crate::guard::BrepError;
use crate::ops::Body;
use makepad_csg_brep::geom::attribute::{AttributeStore, ObjectKey};
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::mass::integrator::properties::{brep_area, brep_mass_properties, brep_volume, PropertyAttributes};
use makepad_csg_brep::math::Point3;
use makepad_csg_brep::query::api_queries::{brep_bounding_box, brep_is_manifold_solid, edge_parameter_range};
use makepad_csg_brep::topology::{edge, EdgeId, FaceId};

/// Volume, area and centroid to this relative accuracy.
const ACCURACY: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    pub volume: f64,
    pub area: f64,
    pub centroid: [f64; 3],
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub faces: usize,
    pub edges: usize,
    pub solid: bool,
}

/// Exact volume, area, centroid and bounds (mm³, mm², mm).
pub fn measure(k: &Kernel, b: &mut Body) -> Result<Measure, BrepError> {
    let store = AttributeStore::default();
    let attrs = PropertyAttributes { store: &store, key_of: &|_| ObjectKey(0) };
    let (volume, area) = volume_area(k, b, ACCURACY)?;
    let mass = brep_mass_properties(k, b, ACCURACY, 1.0, Point3::ZERO, &attrs)?;
    let box3 = brep_bounding_box(k, b, true)?;
    let (lo, hi) = (box3.min(), box3.max());
    Ok(Measure {
        volume,
        area,
        centroid: [mass.centroid.x, mass.centroid.y, mass.centroid.z],
        min: [lo.x, lo.y, lo.z],
        max: [hi.x, hi.y, hi.z],
        faces: makepad_csg_brep::topology::brep::p1::faces(b, None)?.len(),
        edges: makepad_csg_brep::topology::brep::p1::edges(b, None)?.len(),
        solid: brep_is_manifold_solid(b)?,
    })
}

/// Volume and area alone (mm³, mm²), integrated to `accuracy` ([`measure`]'s are to 1e-6),
/// without [`measure`]'s mass properties, bounds and counts.
pub fn volume_area(k: &Kernel, b: &mut Body, accuracy: f64) -> Result<(f64, f64), BrepError> {
    let store = AttributeStore::default();
    let attrs = PropertyAttributes { store: &store, key_of: &|_| ObjectKey(0) };
    Ok((brep_volume(k, b, accuracy, &attrs)?, brep_area(k, b, accuracy)?))
}

/// What an edge's curve is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeShape {
    Line,
    Circle,
    Curve,
}

/// An edge with what selectors and tools ask of it.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeInfo {
    pub id: EdgeId,
    pub shape: EdgeShape,
    pub length: f64,
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub mid: [f64; 3],
    /// Samples along it, ends included.
    pub points: Vec<[f64; 3]>,
    /// The solid's interior angle across it is under 180 degrees.
    pub convex: bool,
    /// Its two faces meet tangent: no visible edge.
    pub smooth: bool,
    /// The radius when `shape` is a circle arc.
    pub radius: Option<f64>,
}

fn arr(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The circle through three points in space: centre and radius.
fn circle3(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<([f64; 3], f64)> {
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let cross = |x: [f64; 3], y: [f64; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let (u, v) = (sub(b, a), sub(c, a));
    let w = cross(u, v);
    let ww = dot(w, w);
    if ww < 1e-18 {
        return None;
    }
    let (uu, vv) = (dot(u, u), dot(v, v));
    let t = cross(w, u);
    let s = cross(v, w);
    let o = [0, 1, 2].map(|i| a[i] + (s[i] * uu + t[i] * vv) / (2.0 * ww));
    Some((o, dist(o, a)))
}

/// Edge `id` at `n` + 1 even steps of its curve's parameter, ends included; None for an edge
/// without a curve.
pub fn sampled(k: &Kernel, b: &Body, id: EdgeId, n: usize) -> Result<Option<Vec<[f64; 3]>>, BrepError> {
    let range = edge_parameter_range(b, id)?;
    let Some(curve) = edge::curve(b, id)? else { return Ok(None) };
    let (lo, hi) = (range.min(), range.max());
    let mut points = Vec::with_capacity(n + 1);
    for i in 0..=n {
        points.push(arr(curve.evaluate_point(k, lo + (hi - lo) * i as f64 / n as f64)?));
    }
    Ok(Some(points))
}

/// Segments an edge's outline takes ([`sampled`]) when it is not a line.
pub const OUTLINE: usize = 32;

/// Every edge of the body, sampled.
pub fn edges(k: &Kernel, b: &mut Body) -> Result<Vec<EdgeInfo>, BrepError> {
    const SAMPLES: usize = 8;
    let ids = makepad_csg_brep::topology::brep::p1::edges(b, None)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(points) = sampled(k, b, id, SAMPLES)? else { continue };
        let length: f64 = points.windows(2).map(|w| dist(w[0], w[1])).sum();
        if length < 1e-9 {
            continue;
        }
        let span = dist(points[0], points[SAMPLES]);
        let straight = points.iter().all(|p| {
            let t = (0..3).map(|i| (p[i] - points[0][i]) * (points[SAMPLES][i] - points[0][i])).sum::<f64>() / span.max(1e-12).powi(2);
            let q = [0, 1, 2].map(|i| points[0][i] + (points[SAMPLES][i] - points[0][i]) * t);
            dist(*p, q) < 1e-6
        }) && span > 1e-9;
        let (shape, radius) = if straight {
            (EdgeShape::Line, None)
        } else {
            match circle3(points[0], points[SAMPLES / 3], points[2 * SAMPLES / 3]) {
                Some((o, r)) if points.iter().all(|p| (dist(*p, o) - r).abs() < 1e-5 * r.max(1.0)) => (EdgeShape::Circle, Some(r)),
                _ => (EdgeShape::Curve, None),
            }
        };
        let (convex, smooth) = crate::tess::edge_is_convex(k, b, id)?;
        out.push(EdgeInfo { id, shape, length, start: points[0], end: points[SAMPLES], mid: points[SAMPLES / 2], points, convex, smooth, radius });
    }
    Ok(out)
}

/// A face with what tools ask of it.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceInfo {
    pub id: FaceId,
    pub kind: &'static str,
    pub area: f64,
    pub centroid: [f64; 3],
    /// The outward normal when the face is flat.
    pub normal: Option<[f64; 3]>,
}

/// Every face of the body.
pub fn faces(k: &Kernel, b: &mut Body) -> Result<Vec<FaceInfo>, BrepError> {
    let ids = makepad_csg_brep::topology::brep::p1::faces(b, None)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let (area, centroid) = makepad_csg_brep::mass::integrator::properties::face_centroid(k, b, id, ACCURACY)?;
        let (kind, normal) = crate::tess::face_kind(k, b, id)?;
        out.push(FaceInfo { id, kind, area, centroid: arr(centroid), normal });
    }
    Ok(out)
}

/// The triangle of `mesh` nearest `p` (`tri_face`: each triangle's face): that face, and how
/// far `p` is from it. O(triangles), most rejected by their box.
pub fn nearest_face(mesh: &makepad_csg_graph::TriMesh, tri_face: &[u32], p: [f64; 3]) -> Option<(u32, f64)> {
    nearest_where(mesh, tri_face, p, |_| true)
}

/// [`nearest_face`] among the triangles of the faces `keep` takes.
pub fn nearest_where(mesh: &makepad_csg_graph::TriMesh, tri_face: &[u32], p: [f64; 3], keep: impl Fn(u32) -> bool) -> Option<(u32, f64)> {
    let v = |i: u32| {
        let q = mesh.vertices[i as usize];
        [q.x, q.y, q.z]
    };
    let mut best: Option<(u32, f64)> = None;
    let mut reach = f64::MAX;
    for (t, tri) in mesh.triangles.iter().enumerate() {
        let Some(&face) = tri_face.get(t) else { continue };
        if !keep(face) {
            continue;
        }
        let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
        if (0..3).any(|i| p[i] < a[i].min(b[i]).min(c[i]) - reach || p[i] > a[i].max(b[i]).max(c[i]) + reach) {
            continue;
        }
        let d = dist(p, on_triangle(p, a, b, c));
        if d < reach {
            reach = d;
            best = Some((face, d));
        }
    }
    best
}

/// The point of segment `ab` nearest `p`.
pub fn on_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let u = if len2 > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    [0, 1, 2].map(|i| a[i] + ab[i] * u)
}

/// The point of triangle `abc` nearest `p` (by its Voronoi regions).
fn on_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let at = |u: f64, w: f64| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * u + (c[i] - a[i]) * w);
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return at(d1 / (d1 - d3), 0.0);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return at(0.0, d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0, 1, 2].map(|i| b[i] + (c[i] - b[i]) * w);
    }
    let denom = 1.0 / (va + vb + vc);
    at(vb * denom, vc * denom)
}
