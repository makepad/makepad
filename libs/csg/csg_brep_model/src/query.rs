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
    let volume = brep_volume(k, b, ACCURACY, &attrs)?;
    let area = brep_area(k, b, ACCURACY)?;
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

/// Every edge of the body, sampled.
pub fn edges(k: &Kernel, b: &mut Body) -> Result<Vec<EdgeInfo>, BrepError> {
    const SAMPLES: usize = 8;
    let ids = makepad_csg_brep::topology::brep::p1::edges(b, None)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let range = edge_parameter_range(b, id)?;
        let Some(curve) = edge::curve(b, id)? else { continue };
        let (lo, hi) = (range.min(), range.max());
        let mut points = Vec::with_capacity(SAMPLES + 1);
        let mut length = 0.0;
        for i in 0..=SAMPLES {
            let p = arr(curve.evaluate_point(k, lo + (hi - lo) * i as f64 / SAMPLES as f64)?);
            if let Some(q) = points.last() {
                length += dist(*q, p);
            }
            points.push(p);
        }
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
