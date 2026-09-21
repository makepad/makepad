use crate::{Path, Point, Segment};

const EPS: f64 = 1.0e-9;

impl Point {
    pub(crate) fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn cubic(a: Point, c1: Point, c2: Point, b: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let u2 = u * u;
    let t2 = t * t;
    Point::new(
        u2 * u * a.x + 3.0 * u2 * t * c1.x + 3.0 * u * t2 * c2.x + t2 * t * b.x,
        u2 * u * a.y + 3.0 * u2 * t * c1.y + 3.0 * u * t2 * c2.y + t2 * t * b.y,
    )
}

fn point_line_distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len2 = dx * dx + dy * dy;
    if len2 <= EPS {
        return p.distance(a);
    }
    ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / len2.sqrt()
}

fn cubic_flat_enough(a: Point, c1: Point, c2: Point, b: Point, tol: f64) -> bool {
    point_line_distance(c1, a, b).max(point_line_distance(c2, a, b)) <= tol
}

fn flatten_cubic(
    a: Point,
    c1: Point,
    c2: Point,
    b: Point,
    tol: f64,
    depth: u8,
    out: &mut Vec<Point>,
) {
    if depth >= 24 || cubic_flat_enough(a, c1, c2, b, tol) {
        if out.last().map_or(true, |last| last.distance(b) > EPS) {
            out.push(b);
        }
        return;
    }
    let a1 = lerp(a, c1, 0.5);
    let mid_controls = lerp(c1, c2, 0.5);
    let b1 = lerp(c2, b, 0.5);
    let a2 = lerp(a1, mid_controls, 0.5);
    let b2 = lerp(mid_controls, b1, 0.5);
    let mid = lerp(a2, b2, 0.5);
    flatten_cubic(a, a1, a2, mid, tol, depth + 1, out);
    flatten_cubic(mid, b2, b1, b, tol, depth + 1, out);
}

pub(crate) fn flatten(path: &Path, tolerance_mm: f64) -> Vec<Point> {
    let tol = if tolerance_mm.is_finite() && tolerance_mm > 0.0 {
        tolerance_mm
    } else {
        0.1
    };
    let mut out = vec![path.start];
    let mut from = path.start;
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => {
                if out.last().map_or(true, |last| last.distance(to) > EPS) {
                    out.push(to);
                }
                from = to;
            }
            Segment::Curve { c1, c2, to } => {
                flatten_cubic(from, c1, c2, to, tol, 0, &mut out);
                from = to;
            }
        }
    }
    if path.closed && out.last().map_or(true, |last| last.distance(path.start) > EPS) {
        out.push(path.start);
    }
    out
}

fn cubic_length(a: Point, c1: Point, c2: Point, b: Point, depth: u8) -> f64 {
    let chord = a.distance(b);
    let control = a.distance(c1) + c1.distance(c2) + c2.distance(b);
    if depth >= 24 || control - chord < 0.0005 {
        return (control + chord) * 0.5;
    }
    let a1 = lerp(a, c1, 0.5);
    let cm = lerp(c1, c2, 0.5);
    let b1 = lerp(c2, b, 0.5);
    let a2 = lerp(a1, cm, 0.5);
    let b2 = lerp(cm, b1, 0.5);
    let mid = lerp(a2, b2, 0.5);
    cubic_length(a, a1, a2, mid, depth + 1) + cubic_length(mid, b2, b1, b, depth + 1)
}

pub(crate) fn path_length(path: &Path) -> f64 {
    let mut length = 0.0;
    let mut from = path.start;
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => length += from.distance(to),
            Segment::Curve { c1, c2, to } => length += cubic_length(from, c1, c2, to, 0),
        }
        from = match *segment {
            Segment::Line { to } | Segment::Curve { to, .. } => to,
        };
    }
    if path.closed {
        length += from.distance(path.start);
    }
    length
}

pub(crate) fn point_at(path: &Path, t: f64) -> Point {
    let points = flatten(path, 0.02);
    point_at_polyline(&points, t)
}

pub(crate) fn point_at_polyline(points: &[Point], t: f64) -> Point {
    if points.is_empty() {
        return Point::default();
    }
    if points.len() == 1 {
        return points[0];
    }
    let lengths: Vec<f64> = points.windows(2).map(|pair| pair[0].distance(pair[1])).collect();
    let total: f64 = lengths.iter().sum();
    if total <= EPS {
        return points[0];
    }
    let target = t.clamp(0.0, 1.0) * total;
    let mut travelled = 0.0;
    for (index, length) in lengths.iter().enumerate() {
        if travelled + length >= target || index + 2 == points.len() {
            let local = if *length <= EPS { 0.0 } else { (target - travelled) / length };
            return lerp(points[index], points[index + 1], local.clamp(0.0, 1.0));
        }
        travelled += length;
    }
    *points.last().unwrap()
}

fn segment_end(segment: &Segment) -> Point {
    match *segment {
        Segment::Line { to } | Segment::Curve { to, .. } => to,
    }
}

pub(crate) fn reverse(path: &Path) -> Path {
    if path.segments.is_empty() {
        return path.clone();
    }
    let mut starts = Vec::with_capacity(path.segments.len());
    let mut current = path.start;
    for segment in &path.segments {
        starts.push(current);
        current = segment_end(segment);
    }
    let mut reversed = Path { start: current, segments: Vec::with_capacity(path.segments.len()), closed: path.closed };
    for (segment, to) in path.segments.iter().zip(starts).rev() {
        reversed.segments.push(match *segment {
            Segment::Line { .. } => Segment::Line { to },
            Segment::Curve { c1, c2, .. } => Segment::Curve { c1: c2, c2: c1, to },
        });
    }
    reversed
}

fn map_path(path: &Path, mut map: impl FnMut(Point) -> Point) -> Path {
    Path {
        start: map(path.start),
        segments: path
            .segments
            .iter()
            .map(|segment| match *segment {
                Segment::Line { to } => Segment::Line { to: map(to) },
                Segment::Curve { c1, c2, to } => Segment::Curve { c1: map(c1), c2: map(c2), to: map(to) },
            })
            .collect(),
        closed: path.closed,
    }
}

pub(crate) fn translate(path: &Path, dx: f64, dy: f64) -> Path {
    map_path(path, |p| Point::new(p.x + dx, p.y + dy))
}

pub(crate) fn mirror_x(path: &Path, axis_x: f64) -> Path {
    map_path(path, |p| Point::new(2.0 * axis_x - p.x, p.y))
}

fn extrema_roots(p0: f64, p1: f64, p2: f64, p3: f64) -> Vec<f64> {
    let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let b = 2.0 * (p0 - 2.0 * p1 + p2);
    let c = p1 - p0;
    if a.abs() < EPS {
        if b.abs() < EPS {
            return Vec::new();
        }
        let t = -c / b;
        return if (0.0..1.0).contains(&t) { vec![t] } else { Vec::new() };
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Vec::new();
    }
    let root = disc.sqrt();
    [(-b + root) / (2.0 * a), (-b - root) / (2.0 * a)]
        .into_iter()
        .filter(|t| (0.0..1.0).contains(t))
        .collect()
}

pub(crate) fn bounds(path: &Path) -> (Point, Point) {
    let mut min = path.start;
    let mut max = path.start;
    let mut include = |p: Point| {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    };
    let mut from = path.start;
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => include(to),
            Segment::Curve { c1, c2, to } => {
                include(to);
                for t in extrema_roots(from.x, c1.x, c2.x, to.x) {
                    include(cubic(from, c1, c2, to, t));
                }
                for t in extrema_roots(from.y, c1.y, c2.y, to.y) {
                    include(cubic(from, c1, c2, to, t));
                }
            }
        }
        from = segment_end(segment);
    }
    (min, max)
}

fn normalize(p: Point, fallback: Point) -> Point {
    let length = p.x.hypot(p.y);
    if length <= EPS {
        let fallback_length = fallback.x.hypot(fallback.y);
        if fallback_length <= EPS {
            Point::new(1.0, 0.0)
        } else {
            Point::new(fallback.x / fallback_length, fallback.y / fallback_length)
        }
    } else {
        Point::new(p.x / length, p.y / length)
    }
}

/// Build a cubic with endpoint tangent directions and a chord-relative handle length.
pub fn curve_through(a: Point, a_dir: Point, b: Point, b_dir: Point, tension: f64) -> Path {
    let chord = Point::new(b.x - a.x, b.y - a.y);
    let handle = a.distance(b) * tension.max(0.0);
    let ad = normalize(a_dir, chord);
    let bd = normalize(b_dir, chord);
    let c1 = Point::new(a.x + ad.x * handle, a.y + ad.y * handle);
    let c2 = Point::new(b.x - bd.x * handle, b.y - bd.y * handle);
    Path {
        start: a,
        segments: vec![Segment::Curve { c1, c2, to: b }],
        closed: false,
    }
}

/// Arc length between two normalized positions along a path.
pub fn seam_length(path: &Path, from_t: f64, to_t: f64) -> f64 {
    let points = flatten(path, 0.02);
    if points.len() < 2 {
        return 0.0;
    }
    let mut from = from_t.clamp(0.0, 1.0);
    let mut to = to_t.clamp(0.0, 1.0);
    if from > to {
        std::mem::swap(&mut from, &mut to);
    }
    let lengths: Vec<f64> = points.windows(2).map(|pair| pair[0].distance(pair[1])).collect();
    let total: f64 = lengths.iter().sum();
    let lo = from * total;
    let hi = to * total;
    let mut at = 0.0;
    let mut result = 0.0;
    for length in lengths {
        let next = at + length;
        result += (next.min(hi) - at.max(lo)).max(0.0);
        at = next;
    }
    result
}

fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

fn line_intersection(p: Point, r: Point, q: Point, s: Point) -> Option<Point> {
    let denominator = cross(r, s);
    if denominator.abs() <= EPS {
        return None;
    }
    let qp = Point::new(q.x - p.x, q.y - p.y);
    let t = cross(qp, s) / denominator;
    Some(Point::new(p.x + t * r.x, p.y + t * r.y))
}

pub(crate) fn signed_area(points: &[Point]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

pub(crate) fn offset(path: &Path, distance_mm: f64) -> Path {
    if !path.closed || distance_mm.abs() <= EPS {
        return if distance_mm.abs() <= EPS {
            let points = flatten(path, 0.1);
            polyline_path(&points, path.closed)
        } else {
            path.clone()
        };
    }
    let tolerance = (distance_mm.abs() * 0.025).clamp(0.02, 0.25);
    // Keep whether an edge came from a cubic. Smooth convex turns on those
    // edges use short circular joins; true polygon corners retain miters.
    let mut points = vec![path.start];
    let mut incoming_curve = vec![false];
    let mut from = path.start;
    for segment in &path.segments {
        match *segment {
            Segment::Line { to } => {
                if points.last().map_or(true, |point| point.distance(to) > EPS) {
                    points.push(to);
                    incoming_curve.push(false);
                }
                from = to;
            }
            Segment::Curve { c1, c2, to } => {
                let mut curve_points = vec![from];
                flatten_cubic(from, c1, c2, to, tolerance, 0, &mut curve_points);
                for point in curve_points.into_iter().skip(1) {
                    if points.last().map_or(true, |last| last.distance(point) > EPS) {
                        points.push(point);
                        incoming_curve.push(true);
                    }
                }
                from = to;
            }
        }
    }
    if points.len() > 1 && points[0].distance(*points.last().unwrap()) <= EPS {
        points.pop();
        incoming_curve.pop();
    }
    if points.len() < 3 {
        return path.clone();
    }

    let area = signed_area(&points);
    let outward_right = area >= 0.0;
    let mut result = Vec::with_capacity(points.len() * 2);
    for index in 0..points.len() {
        let previous = points[(index + points.len() - 1) % points.len()];
        let point = points[index];
        let next = points[(index + 1) % points.len()];
        let previous_direction = normalize(Point::new(point.x - previous.x, point.y - previous.y), Point::new(1.0, 0.0));
        let next_direction = normalize(Point::new(next.x - point.x, next.y - point.y), previous_direction);
        let previous_normal = if outward_right {
            Point::new(previous_direction.y, -previous_direction.x)
        } else {
            Point::new(-previous_direction.y, previous_direction.x)
        };
        let next_normal = if outward_right {
            Point::new(next_direction.y, -next_direction.x)
        } else {
            Point::new(-next_direction.y, next_direction.x)
        };
        let shifted_previous = Point::new(
            point.x + previous_normal.x * distance_mm,
            point.y + previous_normal.y * distance_mm,
        );
        let shifted_next = Point::new(
            point.x + next_normal.x * distance_mm,
            point.y + next_normal.y * distance_mm,
        );
        let turn = cross(previous_direction, next_direction);
        let convex = if area >= 0.0 { turn > EPS } else { turn < -EPS };
        let next_index = (index + 1) % points.len();
        if distance_mm > 0.0
            && convex
            && incoming_curve[index]
            && incoming_curve[next_index]
        {
            let start_angle = previous_normal.y.atan2(previous_normal.x);
            let mut end_angle = next_normal.y.atan2(next_normal.x);
            if area >= 0.0 {
                while end_angle <= start_angle {
                    end_angle += std::f64::consts::TAU;
                }
            } else {
                while end_angle >= start_angle {
                    end_angle -= std::f64::consts::TAU;
                }
            }
            let delta = end_angle - start_angle;
            let steps = (delta.abs() / (std::f64::consts::PI / 18.0)).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let angle = start_angle + delta * step as f64 / steps as f64;
                result.push(Point::new(
                    point.x + angle.cos() * distance_mm,
                    point.y + angle.sin() * distance_mm,
                ));
            }
            continue;
        }
        let intersection = line_intersection(
            shifted_previous,
            previous_direction,
            shifted_next,
            next_direction,
        );
        let miter_limit = 4.0 * distance_mm.abs();
        if let Some(join) = intersection.filter(|join| join.distance(point) <= miter_limit + EPS) {
            result.push(join);
        } else {
            result.push(shifted_previous);
            result.push(shifted_next);
        }
    }
    polyline_path(&result, true)
}

pub(crate) fn polyline_path(points: &[Point], closed: bool) -> Path {
    let Some(&start) = points.first() else {
        return Path::default();
    };
    let mut path = Path { start, segments: Vec::new(), closed };
    let end = if closed && points.last().map_or(false, |last| last.distance(start) <= EPS) {
        points.len() - 1
    } else {
        points.len()
    };
    for point in &points[1..end] {
        path.segments.push(Segment::Line { to: *point });
    }
    path
}

pub(crate) fn distance_to_polyline(point: Point, points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|pair| point_line_distance_to_segment(point, pair[0], pair[1]))
        .fold(f64::INFINITY, f64::min)
}

fn point_line_distance_to_segment(point: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length2 = dx * dx + dy * dy;
    if length2 <= EPS {
        return point.distance(a);
    }
    let t = (((point.x - a.x) * dx + (point.y - a.y) * dy) / length2).clamp(0.0, 1.0);
    point.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

pub(crate) fn nearest_tangent(path: &Path, point: Point) -> Point {
    let points = flatten(path, 0.1);
    let mut best = f64::INFINITY;
    let mut tangent = Point::new(1.0, 0.0);
    for pair in points.windows(2) {
        let distance = point_line_distance_to_segment(point, pair[0], pair[1]);
        if distance < best {
            best = distance;
            tangent = normalize(Point::new(pair[1].x - pair[0].x, pair[1].y - pair[0].y), tangent);
        }
    }
    tangent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_circle_length() {
        let radius = 100.0;
        let k = 0.552_284_749_830_793_6;
        let path = Path {
            start: Point::new(radius, 0.0),
            segments: vec![Segment::Curve {
                c1: Point::new(radius, k * radius),
                c2: Point::new(k * radius, radius),
                to: Point::new(0.0, radius),
            }],
            closed: false,
        };
        let expected = std::f64::consts::FRAC_PI_2 * radius;
        let length: f64 = flatten(&path, 0.02).windows(2).map(|p| p[0].distance(p[1])).sum();
        assert!((length - expected).abs() / expected < 0.001);
    }

    #[test]
    fn square_offset() {
        let mut square = Path { start: Point::new(0.0, 0.0), ..Path::default() };
        square.line_to(Point::new(100.0, 0.0));
        square.line_to(Point::new(100.0, 100.0));
        square.line_to(Point::new(0.0, 100.0));
        square.close();
        let expanded = offset(&square, 10.0);
        let (min, max) = bounds(&expanded);
        assert!((max.x - min.x - 120.0).abs() < 1.0e-6);
        assert!((max.y - min.y - 120.0).abs() < 1.0e-6);
        let mut points = flatten(&expanded, 0.01);
        points.pop();
        assert!((signed_area(&points).abs() - 14_400.0).abs() < 1.0);

        let reversed = square.reverse();
        let reversed_expanded = offset(&reversed, 10.0);
        let (min, max) = bounds(&reversed_expanded);
        assert!((max.x - min.x - 120.0).abs() < 1.0e-6);
        assert!((max.y - min.y - 120.0).abs() < 1.0e-6);

        let inset = offset(&square, -10.0);
        let (min, max) = bounds(&inset);
        assert!((max.x - min.x - 80.0).abs() < 1.0e-6);
        assert!((max.y - min.y - 80.0).abs() < 1.0e-6);
    }
}
