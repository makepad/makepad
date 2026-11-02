//! Sketches as exact kernel curves: lines and arcs stay lines and arcs.

use crate::guard::BrepError;
use makepad_csg_brep::curve::curve::Curve;
use makepad_csg_brep::geom::axis2_placement::Axis2Placement;
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::math::{Point3, Vec3};
use makepad_csg_brep::nurbs::b_spline_curve::p1::NurbCircleParam;
use makepad_csg_brep::nurbs::b_spline_curve::BSplineCurve;
use makepad_csg_brep::primitive::api_curves;
use makepad_csg_cad::{Seg, Shape, Sketch};

type P2 = [f64; 2];

/// A sketch plane: where sketch (u, v) lands in the model.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: [f64; 3],
    pub u: [f64; 3],
    pub v: [f64; 3],
}

impl Frame {
    pub fn at(&self, p: P2) -> Point3 {
        let o = self.origin;
        Point3::new(
            o[0] + self.u[0] * p[0] + self.v[0] * p[1],
            o[1] + self.u[1] * p[0] + self.v[1] * p[1],
            o[2] + self.u[2] * p[0] + self.v[2] * p[1],
        )
    }
    fn dir(a: [f64; 3]) -> Vec3 {
        Vec3::new(a[0], a[1], a[2])
    }
}

fn input(message: impl Into<String>) -> BrepError {
    BrepError::Input(message.into())
}

fn line(f: &Frame, a: P2, b: P2) -> Result<Curve, BrepError> {
    Ok(Curve::from(api_curves::create_line_segment(f.at(a), f.at(b))?))
}

/// The arc about `c` from angle `a0` to `a1` degrees, counter-clockwise when `a1 > a0`.
fn arc(k: &Kernel, f: &Frame, c: P2, r: f64, a0: f64, a1: f64) -> Result<Curve, BrepError> {
    if r <= 0.0 {
        return Err(input("an arc needs a positive radius"));
    }
    let (lo, hi, reversed) = if a1 >= a0 { (a0, a1, false) } else { (a1, a0, true) };
    let frame = Axis2Placement::new(f.at(c), Frame::dir(f.u), Frame::dir(f.v));
    let curve = BSplineCurve::create_circle_segment(3, &frame, r, lo, hi, NurbCircleParam::Quadratic)?;
    let mut curve = Curve::from(curve);
    if reversed {
        let interval = curve.natural_interval();
        curve.reverse_parameterization(k, &interval)?;
    }
    Ok(curve)
}

fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn len(a: P2) -> f64 {
    a[0].hypot(a[1])
}

fn deg(a: P2) -> f64 {
    a[1].atan2(a[0]).to_degrees()
}

/// The circle through three points.
fn circumcircle(a: P2, b: P2, c: P2) -> Result<(P2, f64), BrepError> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return Err(input("an arc's three points are in line"));
    }
    let s = |p: P2| p[0] * p[0] + p[1] * p[1];
    let x = (s(a) * (b[1] - c[1]) + s(b) * (c[1] - a[1]) + s(c) * (a[1] - b[1])) / d;
    let y = (s(a) * (c[0] - b[0]) + s(b) * (a[0] - c[0]) + s(c) * (b[0] - a[0])) / d;
    Ok(([x, y], len(sub(a, [x, y]))))
}

/// A path's pieces in 2D: corners are rounded here, where both sides are known.
#[derive(Clone, Debug)]
enum Piece {
    Line(P2, P2),
    /// Centre, radius, from and to angle in degrees (to > from: counter-clockwise).
    Arc(P2, f64, f64, f64),
    Spline(Vec<P2>),
}

impl Piece {
    /// The direction arriving at its end.
    fn end_dir(&self) -> P2 {
        match self {
            Piece::Line(a, b) => sub(*b, *a),
            Piece::Arc(_, _, a0, a1) => {
                let t = a1.to_radians();
                let s = if a1 >= a0 { 1.0 } else { -1.0 };
                [-t.sin() * s, t.cos() * s]
            }
            Piece::Spline(p) => sub(p[p.len() - 1], p[p.len() - 2]),
        }
    }
    fn curve(&self, k: &Kernel, f: &Frame) -> Result<Curve, BrepError> {
        match self {
            Piece::Line(a, b) => line(f, *a, *b),
            Piece::Arc(c, r, a0, a1) => arc(k, f, *c, *r, *a0, *a1),
            Piece::Spline(p) => {
                let points: Vec<Point3> = p.iter().map(|q| f.at(*q)).collect();
                Ok(Curve::from(api_curves::create_curve(&points)?))
            }
        }
    }
}

/// The arc from `a` through `m` to `b`.
fn arc_through(a: P2, m: P2, b: P2) -> Result<Piece, BrepError> {
    let (c, r) = circumcircle(a, m, b)?;
    let a0 = deg(sub(a, c));
    let (mut am, mut a1) = (deg(sub(m, c)), deg(sub(b, c)));
    // Counter-clockwise when going a -> m -> b does not wrap past b.
    while am < a0 {
        am += 360.0;
    }
    while a1 < a0 {
        a1 += 360.0;
    }
    if am <= a1 {
        Ok(Piece::Arc(c, r, a0, a1))
    } else {
        Ok(Piece::Arc(c, r, a0, a1 - 360.0))
    }
}

/// An arc to `to` of radius `r` (positive counter-clockwise).
fn arc_radius(a: P2, to: P2, r: f64, large: bool) -> Result<Piece, BrepError> {
    let chord = sub(to, a);
    let d = len(chord);
    let radius = r.abs();
    if d < 1e-12 || d > 2.0 * radius + 1e-9 {
        return Err(input(format!("an arc of radius {radius} cannot span {d}")));
    }
    let h = (radius * radius - d * d * 0.25).max(0.0).sqrt();
    let n = [-chord[1] / d, chord[0] / d];
    let side = if (r > 0.0) != large { 1.0 } else { -1.0 };
    let c = [(a[0] + to[0]) * 0.5 + n[0] * h * side, (a[1] + to[1]) * 0.5 + n[1] * h * side];
    let a0 = deg(sub(a, c));
    let mut a1 = deg(sub(to, c));
    if r > 0.0 {
        while a1 < a0 {
            a1 += 360.0;
        }
    } else {
        while a1 > a0 {
            a1 -= 360.0;
        }
    }
    Ok(Piece::Arc(c, radius, a0, a1))
}

/// An arc from the previous piece's end, tangent to it, to `to`.
fn arc_tangent(a: P2, dir: P2, to: P2) -> Result<Piece, BrepError> {
    let dl = len(dir).max(1e-12);
    let n = [-dir[1] / dl, dir[0] / dl];
    let chord = sub(to, a);
    let denom = 2.0 * (chord[0] * n[0] + chord[1] * n[1]);
    if denom.abs() < 1e-12 {
        return Ok(Piece::Line(a, to));
    }
    let s = (chord[0] * chord[0] + chord[1] * chord[1]) / denom;
    let c = [a[0] + n[0] * s, a[1] + n[1] * s];
    let a0 = deg(sub(a, c));
    let mut a1 = deg(sub(to, c));
    if s > 0.0 {
        while a1 < a0 {
            a1 += 360.0;
        }
    } else {
        while a1 > a0 {
            a1 -= 360.0;
        }
    }
    Ok(Piece::Arc(c, s.abs(), a0, a1))
}

/// A smooth curve starting at `a` through `points` (the kernel interpolates).
fn spline(a: P2, points: &[P2]) -> Piece {
    let mut p = Vec::with_capacity(points.len() + 1);
    p.push(a);
    p.extend_from_slice(points);
    Piece::Spline(p)
}

/// Rounds or cuts the corner between the last line and the next line of a path.
fn corner(pieces: &mut Vec<Piece>, next_to: P2, size: f64, round: bool) -> Result<P2, BrepError> {
    let Some(Piece::Line(a, b)) = pieces.last().cloned() else {
        return Err(input("Fillet and Chamfer in a path round a corner between two lines"));
    };
    let (in_d, out_d) = (sub(b, a), sub(next_to, b));
    let (li, lo) = (len(in_d), len(out_d));
    if li < 1e-12 || lo < 1e-12 {
        return Err(input("a corner's lines have no length"));
    }
    let (ui, uo) = ([in_d[0] / li, in_d[1] / li], [out_d[0] / lo, out_d[1] / lo]);
    let turn = ui[0] * uo[1] - ui[1] * uo[0];
    let cos = ui[0] * uo[0] + ui[1] * uo[1];
    if turn.abs() < 1e-9 {
        return Err(input("a corner's lines are in line"));
    }
    // The tangent length of a round is r tan(turn / 2).
    let back = if round { size * ((1.0 - cos).max(0.0) / (1.0 + cos).max(1e-12)).sqrt() } else { size };
    if back > li + 1e-9 || back > lo + 1e-9 {
        return Err(input(format!("a corner of {size} does not fit between its lines")));
    }
    let p0 = [b[0] - ui[0] * back, b[1] - ui[1] * back];
    let p1 = [b[0] + uo[0] * back, b[1] + uo[1] * back];
    pieces.pop();
    pieces.push(Piece::Line(a, p0));
    if round {
        let n = if turn > 0.0 { [-ui[1], ui[0]] } else { [ui[1], -ui[0]] };
        let c = [p0[0] + n[0] * size, p0[1] + n[1] * size];
        let (a0, mut a1) = (deg(sub(p0, c)), deg(sub(p1, c)));
        if turn > 0.0 {
            while a1 < a0 {
                a1 += 360.0;
            }
        } else {
            while a1 > a0 {
                a1 -= 360.0;
            }
        }
        pieces.push(Piece::Arc(c, size, a0, a1));
    } else {
        pieces.push(Piece::Line(p0, p1));
    }
    Ok(p1)
}

fn path_pieces(start: P2, segs: &[Seg]) -> Result<Vec<Piece>, BrepError> {
    let mut pieces: Vec<Piece> = Vec::new();
    let mut at = start;
    let mut pending: Option<(f64, bool)> = None;
    for (i, seg) in segs.iter().enumerate() {
        let target = match seg {
            Seg::Line(p) => Some(*p),
            Seg::Rel(d) => Some([at[0] + d[0], at[1] + d[1]]),
            Seg::H(dx) => Some([at[0] + dx, at[1]]),
            Seg::V(dy) => Some([at[0], at[1] + dy]),
            Seg::Polar { len, deg } => {
                let a = deg.to_radians();
                Some([at[0] + len * a.cos(), at[1] + len * a.sin()])
            }
            _ => None,
        };
        if let Some((size, round)) = pending.take() {
            let to = match (target, seg) {
                (Some(t), _) => t,
                _ => return Err(input(format!("segment {i}: a Fillet or Chamfer in a path needs a line after it"))),
            };
            at = corner(&mut pieces, to, size, round)?;
        }
        match seg {
            Seg::Line(_) | Seg::Rel(_) | Seg::H(_) | Seg::V(_) | Seg::Polar { .. } => {
                let to = target.unwrap_or(at);
                pieces.push(Piece::Line(at, to));
                at = to;
            }
            Seg::ArcThrough { through, to } => {
                pieces.push(arc_through(at, *through, *to)?);
                at = *to;
            }
            Seg::ArcRadius { to, r, large } => {
                pieces.push(arc_radius(at, *to, *r, *large)?);
                at = *to;
            }
            Seg::Tangent(to) => {
                let dir = pieces.last().map(Piece::end_dir).ok_or_else(|| input("a path can't start with a Tangent arc"))?;
                pieces.push(arc_tangent(at, dir, *to)?);
                at = *to;
            }
            Seg::Spline(points) => {
                if points.is_empty() {
                    return Err(input(format!("segment {i}: a spline needs points")));
                }
                pieces.push(spline(at, points));
                at = points[points.len() - 1];
            }
            Seg::Fillet(r) => pending = Some((*r, true)),
            Seg::Chamfer(d) => pending = Some((*d, false)),
        }
    }
    // A corner at the last point rounds toward the closing line.
    if let Some((size, round)) = pending {
        at = corner(&mut pieces, start, size, round)?;
    }
    if len(sub(at, start)) > 1e-9 {
        pieces.push(Piece::Line(at, start));
    }
    Ok(pieces)
}

fn rect_pieces(center: P2, size: P2, corner_r: f64, chamfer: f64) -> Vec<Piece> {
    let (hx, hy) = (size[0] * 0.5, size[1] * 0.5);
    let (cx, cy) = (center[0], center[1]);
    let d = corner_r.max(chamfer);
    let p = |x: f64, y: f64| [cx + x, cy + y];
    if d <= 0.0 {
        return vec![
            Piece::Line(p(-hx, -hy), p(hx, -hy)),
            Piece::Line(p(hx, -hy), p(hx, hy)),
            Piece::Line(p(hx, hy), p(-hx, hy)),
            Piece::Line(p(-hx, hy), p(-hx, -hy)),
        ];
    }
    let mut out = Vec::with_capacity(8);
    let corners = [(hx - d, -hy + d, -90.0), (hx - d, hy - d, 0.0), (-hx + d, hy - d, 90.0), (-hx + d, -hy + d, 180.0)];
    let mut prev = p(-hx + d, -hy);
    for (x, y, a) in corners {
        let c = p(x, y);
        let (s, e) = (a as f64, a as f64 + 90.0);
        let from = [c[0] + d * s.to_radians().cos(), c[1] + d * s.to_radians().sin()];
        let to = [c[0] + d * e.to_radians().cos(), c[1] + d * e.to_radians().sin()];
        out.push(Piece::Line(prev, from));
        if corner_r > 0.0 {
            out.push(Piece::Arc(c, d, s, e));
        } else {
            out.push(Piece::Line(from, to));
        }
        prev = to;
    }
    out
}

fn polygon_pieces(points: &[P2], fillet: f64) -> Result<Vec<Piece>, BrepError> {
    if points.len() < 3 {
        return Err(input("a polygon needs three points"));
    }
    if fillet <= 0.0 {
        return Ok((0..points.len()).map(|i| Piece::Line(points[i], points[(i + 1) % points.len()])).collect());
    }
    let mut segs: Vec<Seg> = Vec::new();
    for p in &points[1..] {
        segs.push(Seg::Line(*p));
        segs.push(Seg::Fillet(fillet));
    }
    segs.push(Seg::Line(points[0]));
    path_pieces(points[0], &segs)
}

fn shape_pieces(shape: &Shape) -> Result<Vec<Piece>, BrepError> {
    match shape {
        Shape::Rect { center, size, corner, chamfer } => Ok(rect_pieces(*center, *size, *corner, *chamfer)),
        Shape::Circle { center, r } => Ok(vec![Piece::Arc(*center, *r, 0.0, 360.0)]),
        Shape::Ellipse { .. } => Err(input("an Ellipse sketch is not available in a brep profile yet; use a spline Path")),
        Shape::Slot { a, b, r } => {
            let d = sub(*b, *a);
            let l = len(d);
            if l < 1e-12 {
                return Ok(vec![Piece::Arc(*a, *r, 0.0, 360.0)]);
            }
            let n = [-d[1] / l * r, d[0] / l * r];
            let a0 = deg(n);
            Ok(vec![
                Piece::Line([a[0] - n[0], a[1] - n[1]], [b[0] - n[0], b[1] - n[1]]),
                Piece::Arc(*b, *r, a0 - 180.0, a0),
                Piece::Line([b[0] + n[0], b[1] + n[1]], [a[0] + n[0], a[1] + n[1]]),
                Piece::Arc(*a, *r, a0, a0 + 180.0),
            ])
        }
        Shape::Regular { center, r, sides, deg } => {
            let n = (*sides).max(3);
            let at = |i: usize| {
                let a = deg.to_radians() + std::f64::consts::TAU * i as f64 / n as f64;
                [center[0] + r * a.cos(), center[1] + r * a.sin()]
            };
            Ok((0..n).map(|i| Piece::Line(at(i), at((i + 1) % n))).collect())
        }
        Shape::Polygon { points, fillet } => polygon_pieces(points, *fillet),
        Shape::Path { start, segs } => path_pieces(*start, segs),
    }
}

/// Each shape of a sketch as a closed chain of exact curves, in `frame`.
pub fn sketch_curves(k: &Kernel, sketch: &Sketch, frame: &Frame) -> Result<Vec<Vec<Curve>>, BrepError> {
    let mut out = Vec::with_capacity(sketch.shapes.len());
    for (i, shape) in sketch.shapes.iter().enumerate() {
        let pieces = shape_pieces(shape).map_err(|e| input(format!("sketch shape {i}: {e}")))?;
        let curves = pieces.iter().map(|p| p.curve(k, frame)).collect::<Result<Vec<_>, _>>().map_err(|e| input(format!("sketch shape {i}: {e}")))?;
        out.push(curves);
    }
    Ok(out)
}
