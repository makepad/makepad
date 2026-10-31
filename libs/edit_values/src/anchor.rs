//! Spatial handles: a value's handle in 3D (an arrow for a length, a ring for a radius, an arc
//! for an angle, points for a position or a profile, + and - for a count) and the drag math the
//! desktop view (a pointer ray) and a headset (a hand point) share.
//!
//! Positions are in the document's world frame (any unit; Fab uses printer millimetres). A
//! handle's `rate` is how far it moves per unit of its value, so a drag maps back to a value
//! whatever formulas sit between the value and the geometry.

use crate::site::{Value, ValueKind};

pub type V3 = [f64; 3];

/// A handle's geometry. `rate`: handle travel (or degrees for an angle) per unit of value.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// An arrow at `to` pointing away from `from`: the handle slides along that line.
    Linear { from: V3, to: V3, rate: f64 },
    /// A ring of `radius` about `axis` through `center`.
    Radius { center: V3, axis: V3, radius: f64, rate: f64 },
    /// An arc about `axis` through `center`, from the direction `from` (angle 0) to `angle`
    /// degrees (right-handed about `axis`).
    Angle { center: V3, axis: V3, from: V3, radius: f64, angle: f64, rate: f64 },
    /// A point at `at`; `axes[i]` is the displacement per unit of coordinate i.
    Point { at: V3, axes: Vec<V3> },
    /// A 2D profile in the plane `origin + x*u + y*v`; a point handle per point.
    Profile { origin: V3, u: V3, v: V3, points: Vec<[f64; 2]> },
    /// + and - buttons at `at`, along `axis`.
    Count { at: V3, axis: V3 },
}

/// A box in the world frame (an object's bounds): handles snap to its faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bounds {
    pub id: String,
    pub min: V3,
    pub max: V3,
}

/// A point through a 4x4 or 3x4 row-major affine.
pub fn xf_point(m: &[[f64; 4]], p: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
}

/// A direction through the linear part.
pub fn xf_dir(m: &[[f64; 4]], d: V3) -> V3 {
    std::array::from_fn(|i| m[i][0] * d[0] + m[i][1] * d[1] + m[i][2] * d[2])
}

impl Anchor {
    /// The same handle through an affine `m` (an object's world matrix, a part's placement).
    /// Lengths and rates scale with `m`; angles do not.
    pub fn transformed(&self, m: &[[f64; 4]]) -> Anchor {
        match self {
            Anchor::Linear { from, to, rate } => {
                let d = unit(sub(*to, *from));
                Anchor::Linear { from: xf_point(m, *from), to: xf_point(m, *to), rate: rate * length(xf_dir(m, d)) }
            }
            Anchor::Radius { center, axis, radius, rate } => {
                let s = length(xf_dir(m, perp(*axis)));
                Anchor::Radius { center: xf_point(m, *center), axis: unit(xf_dir(m, *axis)), radius: radius * s, rate: rate * s }
            }
            Anchor::Angle { center, axis, from, radius, angle, rate } => {
                let s = length(xf_dir(m, unit(*from)));
                Anchor::Angle { center: xf_point(m, *center), axis: unit(xf_dir(m, *axis)), from: unit(xf_dir(m, *from)), radius: radius * s, angle: *angle, rate: *rate }
            }
            Anchor::Point { at, axes } => Anchor::Point { at: xf_point(m, *at), axes: axes.iter().map(|a| xf_dir(m, *a)).collect() },
            Anchor::Profile { origin, u, v, points } => Anchor::Profile { origin: xf_point(m, *origin), u: xf_dir(m, *u), v: xf_dir(m, *v), points: points.clone() },
            Anchor::Count { at, axis } => Anchor::Count { at: xf_point(m, *at), axis: unit(xf_dir(m, *axis)) },
        }
    }

    /// How far along its own measure the handle is (the tip along the arrow, the ring's radius,
    /// the arc's angle): two of these at two values give the handle's rate.
    pub fn measure(&self) -> Option<f64> {
        match self {
            Anchor::Linear { from, to, .. } => Some(length(sub(*to, *from))),
            Anchor::Radius { radius, .. } => Some(*radius),
            Anchor::Angle { angle, .. } => Some(*angle),
            _ => None,
        }
    }

    /// The same handle with its rate measured from `other`, the handle at a value `h` further
    /// on (`None`: the handle does not move with the value).
    pub fn with_rate(self, other: Option<&Anchor>, h: f64) -> Option<Anchor> {
        let dir = match &self {
            Anchor::Linear { from, to, .. } => unit(sub(*to, *from)),
            _ => [0.0; 3],
        };
        let along = |a: &Anchor| match a {
            Anchor::Linear { from, to, .. } => Some(dot(sub(*to, *from), dir)),
            other => other.measure(),
        };
        let rate = match (along(&self), other.and_then(along)) {
            (Some(m0), Some(m1)) => (m1 - m0) / h,
            _ => return Some(self),
        };
        if rate.abs() < 1e-9 {
            return None;
        }
        Some(match self {
            Anchor::Linear { from, to, .. } => Anchor::Linear { from, to, rate },
            Anchor::Radius { center, axis, radius, .. } => Anchor::Radius { center, axis, radius, rate },
            Anchor::Angle { center, axis, from, radius, angle, .. } => Anchor::Angle { center, axis, from, radius, angle, rate },
            other => other,
        })
    }
}

// ===========================================================================
// Snapping (round values, faces) and haptic ticks
// ===========================================================================

/// The round-value increment a drag of a `kind` value snaps to: whole units over 10, halves
/// over 2, tenths below; 15 degrees (5 under 30); whole counts; 5 % for a ratio. `fine` (Shift,
/// a half-pressed trigger) drops to `step`.
pub fn snap_step(kind: ValueKind, step: f64, value: f64, fine: bool) -> f64 {
    if fine {
        return if kind == ValueKind::Count { 1.0 } else { step.max(1e-6) };
    }
    match kind {
        ValueKind::Count => 1.0,
        ValueKind::Angle => {
            if value.abs() < 30.0 {
                5.0
            } else {
                15.0
            }
        }
        ValueKind::Ratio => 0.05,
        _ => {
            if value.abs() >= 10.0 {
                1.0
            } else if value.abs() >= 2.0 {
                0.5
            } else {
                0.1
            }
        }
    }
}

pub fn snap(value: f64, inc: f64) -> f64 {
    if inc > 0.0 {
        (value / inc).round() * inc
    } else {
        value
    }
}

/// The snap points a value passes between `a` and `b` (for haptic ticks).
pub fn ticks_between(a: f64, b: f64, inc: f64) -> usize {
    if inc <= 0.0 {
        return 0;
    }
    ((a / inc).floor() - (b / inc).floor()).abs() as usize
}

// ===========================================================================
// Vector helpers and the drag math (pure; the desktop and the headset share it)
// ===========================================================================

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn unit(a: V3) -> V3 {
    let l = length(a);
    if l > 1e-12 {
        mul(a, 1.0 / l)
    } else {
        [0.0, 0.0, 1.0]
    }
}
pub fn perp(a: V3) -> V3 {
    let t = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    unit(cross(a, t))
}
/// `d` turned by `radians` about the unit `axis` (right-handed).
pub fn rotate(d: V3, axis: V3, radians: f64) -> V3 {
    let k = unit(axis);
    let (s, c) = radians.sin_cos();
    let kd = dot(k, d);
    let x = cross(k, d);
    std::array::from_fn(|i| d[i] * c + x[i] * s + k[i] * kd * (1.0 - c))
}

/// A pointer: a ray (desktop) or a hand point (headset: `dir` zero).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragPointer {
    pub origin: V3,
    pub dir: V3,
}

impl DragPointer {
    pub fn ray(origin: V3, dir: V3) -> DragPointer {
        DragPointer { origin, dir: unit(dir) }
    }
    pub fn point(at: V3) -> DragPointer {
        DragPointer { origin: at, dir: [0.0; 3] }
    }
    fn is_point(&self) -> bool {
        self.dir == [0.0; 3]
    }

    /// The parameter along the line `p + t*d` closest to the pointer.
    pub fn along(&self, p: V3, d: V3) -> Option<f64> {
        let d = unit(d);
        if self.is_point() {
            return Some(dot(sub(self.origin, p), d));
        }
        // Closest points between two lines.
        let w = sub(p, self.origin);
        let (a, b, c) = (dot(d, d), dot(d, self.dir), dot(self.dir, self.dir));
        let (e, f) = (dot(d, w), dot(self.dir, w));
        let den = a * c - b * b;
        if den.abs() < 1e-9 {
            return None;
        }
        Some((b * f - c * e) / den)
    }

    /// Where the pointer meets the plane through `p` with normal `n` (a hand point projects onto it).
    pub fn on_plane(&self, p: V3, n: V3) -> Option<V3> {
        let n = unit(n);
        if self.is_point() {
            return Some(sub(self.origin, mul(n, dot(sub(self.origin, p), n))));
        }
        let den = dot(self.dir, n);
        if den.abs() < 1e-6 {
            return None;
        }
        let t = dot(sub(p, self.origin), n) / den;
        (t > 0.0).then(|| add(self.origin, mul(self.dir, t)))
    }
}

/// The plane normal a point handle drags in: of the planes its axes span, the one that faces
/// `view` (towards the eye) most.
pub fn point_plane(axes: &[V3], view: V3) -> (usize, usize) {
    let n = axes.len().min(3);
    if n < 2 {
        return (0, 0);
    }
    let mut best = (0, 1, -1.0);
    for i in 0..n {
        for j in i + 1..n {
            let f = dot(unit(cross(axes[i], axes[j])), unit(view)).abs();
            if f > best.2 {
                best = (i, j, f);
            }
        }
    }
    (best.0, best.1)
}

/// Solve `d = a*u + b*v` in the least-squares sense.
pub fn solve2(d: V3, u: V3, v: V3) -> Option<(f64, f64)> {
    let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
    let (du, dv) = (dot(d, u), dot(d, v));
    let det = uu * vv - uv * uv;
    if det.abs() < 1e-12 {
        return None;
    }
    Some(((du * vv - dv * uv) / det, (dv * uu - du * uv) / det))
}

/// Which part of a handle a drag holds (a profile point's index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Grip {
    pub point: usize,
}

/// The value a handle drag gives when the pointer is at `ptr`, relative to the handle as last
/// drawn (`now` = the value it was built for). `view` points towards the eye (for point planes).
/// `None` when the pointer is parallel to the drag. Numbers are unsnapped.
pub fn drag_value(handle: &Anchor, now: &Value, grip: Grip, ptr: &DragPointer, view: V3) -> Option<Value> {
    match (handle, now) {
        (Anchor::Linear { from, to, rate }, Value::Num(v)) => {
            let d = sub(*to, *from);
            let s = ptr.along(*to, if length(d) > 1e-9 { d } else { [0.0, 0.0, 1.0] })?;
            (rate.abs() > 1e-12).then(|| Value::Num(v + s / rate))
        }
        (Anchor::Radius { center, axis, radius, rate }, Value::Num(v)) => {
            let hit = ptr.on_plane(*center, *axis)?;
            let r = length(sub(hit, *center));
            (rate.abs() > 1e-12).then(|| Value::Num(v + (r - radius) / rate))
        }
        (Anchor::Angle { center, axis, from, angle, rate, .. }, Value::Num(v)) => {
            let hit = ptr.on_plane(*center, *axis)?;
            let d = sub(hit, *center);
            if length(d) < 1e-9 {
                return None;
            }
            let x = unit(*from);
            let y = cross(unit(*axis), x);
            let a = dot(d, y).atan2(dot(d, x)).to_degrees();
            let mut delta = a - angle;
            while delta > 180.0 {
                delta -= 360.0;
            }
            while delta < -180.0 {
                delta += 360.0;
            }
            (rate.abs() > 1e-12).then(|| Value::Num(v + delta / rate))
        }
        (Anchor::Point { at, axes }, Value::Point(c)) => {
            let mut out = c.clone();
            if axes.len() == 1 {
                let s = ptr.along(*at, axes[0])?;
                if let Some(x) = out.get_mut(0) {
                    *x += s / length(axes[0]).max(1e-12);
                }
                return Some(Value::Point(out));
            }
            let (i, j) = point_plane(axes, view);
            let hit = ptr.on_plane(*at, cross(axes[i], axes[j]))?;
            let (a, b) = solve2(sub(hit, *at), axes[i], axes[j])?;
            if let Some(x) = out.get_mut(i) {
                *x += a;
            }
            if let Some(x) = out.get_mut(j) {
                *x += b;
            }
            Some(Value::Point(out))
        }
        (Anchor::Profile { origin, u, v, .. }, Value::Profile(pts)) => {
            let hit = ptr.on_plane(*origin, cross(*u, *v))?;
            let (a, b) = solve2(sub(hit, *origin), *u, *v)?;
            let mut out = pts.clone();
            *out.get_mut(grip.point)? = [a, b];
            Some(Value::Profile(out))
        }
        _ => None,
    }
}

/// The plate point of profile point `i`.
pub fn profile_point(origin: V3, u: V3, v: V3, p: [f64; 2]) -> V3 {
    add(origin, add(mul(u, p[0]), mul(v, p[1])))
}

/// Where a handle is held: the arrow tip, a ring point facing `view`, the arc end, the point.
pub fn grip_point(handle: &Anchor, grip: Grip, view: V3) -> V3 {
    match handle {
        Anchor::Linear { to, .. } => *to,
        Anchor::Radius { center, axis, radius, .. } => {
            // The ring point nearest the eye's side, so the grip is never behind the part.
            let side = sub(view, mul(unit(*axis), dot(view, unit(*axis))));
            let dir = if length(side) > 1e-6 { unit(side) } else { perp(*axis) };
            add(*center, mul(dir, *radius))
        }
        Anchor::Angle { center, axis, from, radius, angle, .. } => add(*center, mul(rotate(unit(*from), *axis, angle.to_radians()), radius.max(1.0))),
        Anchor::Point { at, .. } | Anchor::Count { at, .. } => *at,
        Anchor::Profile { origin, u, v, points } => points.get(grip.point).map(|p| profile_point(*origin, *u, *v, *p)).unwrap_or(*origin),
    }
}

/// Snap a linear drag's tip to the box edges of other objects: the snapped value when the
/// tip lands within `tol` mm (along the arrow) of a box face plane.
pub fn edge_snap(handle: &Anchor, now: f64, target: f64, boxes: &[Bounds], skip: &[String], tol: f64) -> Option<f64> {
    let Anchor::Linear { from, to, rate } = handle else { return None };
    if rate.abs() < 1e-12 {
        return None;
    }
    let d = unit(sub(*to, *from));
    let tip = add(*to, mul(d, (target - now) * rate));
    let mut best: Option<(f64, f64)> = None;
    for b in boxes.iter().filter(|b| !skip.contains(&b.id)) {
        for k in 0..3 {
            if d[k].abs() < 0.999 {
                continue;
            }
            for face in [b.min[k], b.max[k]] {
                let off = (face - tip[k]) * d[k].signum();
                if off.abs() <= tol && best.is_none_or(|(o, _)| off.abs() < o) {
                    best = Some((off.abs(), target + off / rate));
                }
            }
        }
    }
    best.map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_drag_follows_a_ray_and_a_hand() {
        // An arrow along +Z at the top of a 70 mm part whose tip moves 1 mm per mm of value.
        let h = Anchor::Linear { from: [0.0, 0.0, 35.0], to: [0.0, 0.0, 70.0], rate: 1.0 };
        let now = Value::Num(70.0);
        // A ray from the side at Z 80.
        let ray = DragPointer::ray([100.0, 0.0, 80.0], [-1.0, 0.0, 0.0]);
        assert_eq!(drag_value(&h, &now, Grip::default(), &ray, [1.0, 0.0, 0.0]), Some(Value::Num(80.0)));
        // A hand 4 mm below the tip; a centred size (rate 0.5) needs twice the value change.
        let hand = DragPointer::point([3.0, 2.0, 66.0]);
        assert_eq!(drag_value(&h, &now, Grip::default(), &hand, [1.0, 0.0, 0.0]), Some(Value::Num(66.0)));
        let half = Anchor::Linear { from: [0.0, 0.0, 35.0], to: [0.0, 0.0, 70.0], rate: 0.5 };
        assert_eq!(drag_value(&half, &now, Grip::default(), &hand, [1.0, 0.0, 0.0]), Some(Value::Num(62.0)));
    }

    #[test]
    fn rings_arcs_points_and_profiles() {
        let ring = Anchor::Radius { center: [0.0; 3], axis: [0.0, 0.0, 1.0], radius: 10.0, rate: 1.0 };
        let down = DragPointer::ray([12.0, 0.0, 50.0], [0.0, 0.0, -1.0]);
        assert_eq!(drag_value(&ring, &Value::Num(10.0), Grip::default(), &down, [0.0, 0.0, 1.0]), Some(Value::Num(12.0)));
        let arc = Anchor::Angle { center: [0.0; 3], axis: [0.0, 0.0, 1.0], from: [1.0, 0.0, 0.0], radius: 10.0, angle: 0.0, rate: 1.0 };
        let at90 = DragPointer::ray([0.0, 5.0, 50.0], [0.0, 0.0, -1.0]);
        let Value::Num(a) = drag_value(&arc, &Value::Num(0.0), Grip::default(), &at90, [0.0, 0.0, 1.0]).unwrap() else { panic!() };
        assert!((a - 90.0).abs() < 1e-9);
        let pt = Anchor::Point { at: [1.0, 2.0, 3.0], axes: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };
        // Looking down: the XY plane; Z keeps.
        let Value::Point(c) = drag_value(&pt, &Value::Point(vec![1.0, 2.0, 3.0]), Grip::default(), &DragPointer::ray([4.0, 6.0, 9.0], [0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]).unwrap() else { panic!() };
        assert_eq!(c, vec![4.0, 6.0, 3.0]);
        let prof = Anchor::Profile { origin: [0.0, 0.0, 5.0], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]] };
        let now = Value::Profile(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        let got = drag_value(&prof, &now, Grip { point: 2 }, &DragPointer::ray([2.0, 3.0, 9.0], [0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]);
        assert_eq!(got, Some(Value::Profile(vec![[0.0, 0.0], [1.0, 0.0], [2.0, 3.0]])));
    }

    #[test]
    fn snapping_rounds_and_meets_edges() {
        assert_eq!(snap(13.37, snap_step(ValueKind::Length, 0.1, 13.37, false)), 13.0);
        assert!((snap(13.37, snap_step(ValueKind::Length, 0.1, 13.37, true)) - 13.4).abs() < 1e-9);
        assert_eq!(ticks_between(9.6, 12.2, 1.0), 3);
        let h = Anchor::Linear { from: [0.0, 0.0, 0.0], to: [0.0, 0.0, 10.0], rate: 1.0 };
        let boxes = vec![Bounds { id: "lid".into(), min: [0.0, 0.0, 12.0], max: [5.0, 5.0, 14.0] }];
        assert_eq!(edge_snap(&h, 10.0, 11.7, &boxes, &[], 0.5), Some(12.0));
        assert_eq!(edge_snap(&h, 10.0, 11.0, &boxes, &[], 0.5), None);
        assert_eq!(edge_snap(&h, 10.0, 11.7, &boxes, &["lid".into()], 0.5), None);
    }
}
