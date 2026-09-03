//! Deterministic, zoom-independent cable routing in canvas units.
//!
//! Cards passed to [`route_wire`] are already inflated by the caller's card
//! clearance. The router reserves another fillet radius while choosing an
//! orthogonal track, so rounding a corner cannot cut back through that
//! clearance.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn distance(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    fn lerp(self, other: Self, t: f64) -> Self {
        Self::new(self.x + (other.x - self.x) * t, self.y + (other.y - self.y) * t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub min: Point,
    pub max: Point,
}

impl Obstacle {
    pub fn from_xywh(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            min: Point::new(x, y),
            max: Point::new(x + width, y + height),
        }
    }

    pub fn inflate(self, amount: f64) -> Self {
        Self {
            min: Point::new(self.min.x - amount, self.min.y - amount),
            max: Point::new(self.max.x + amount, self.max.y + amount),
        }
    }

    fn contains_strict(self, point: Point) -> bool {
        point.x > self.min.x && point.x < self.max.x && point.y > self.min.y && point.y < self.max.y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteStyle {
    pub port_stub: f64,
    pub corner_radius: f64,
    pub cable_spacing: f64,
}

impl Default for RouteStyle {
    fn default() -> Self {
        Self {
            // Two radii make room for an honest 16 px fillet at the first
            // and last bend while exceeding the requested 24 px approach.
            port_stub: 32.0,
            corner_radius: 16.0,
            cable_spacing: 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RouteKind {
    Cubic { control_1: Point, control_2: Point },
    Orthogonal { points: Vec<Point>, radius: f64 },
}

/// A drawable route plus a dense-enough arc-length table for animation.
#[derive(Clone, Debug, PartialEq)]
pub struct WireRoute {
    pub from: Point,
    pub to: Point,
    pub kind: RouteKind,
    samples: Vec<Point>,
    cumulative: Vec<f64>,
    length: f64,
}

impl WireRoute {
    #[cfg(test)]
    pub fn is_straight_cubic(&self) -> bool {
        matches!(self.kind, RouteKind::Cubic { .. })
    }

    pub fn length(&self) -> f64 {
        self.length
    }

    #[cfg(test)]
    pub fn point_at(&self, fraction: f64) -> Point {
        self.point_at_distance(self.length * fraction.clamp(0.0, 1.0))
    }

    pub fn point_at_distance(&self, distance: f64) -> Point {
        if self.samples.len() < 2 || self.length <= f64::EPSILON {
            return self.from;
        }
        let distance = distance.clamp(0.0, self.length);
        let upper = self.cumulative.partition_point(|value| *value < distance);
        if upper == 0 {
            return self.samples[0];
        }
        if upper >= self.samples.len() {
            return *self.samples.last().unwrap();
        }
        let lower = upper - 1;
        let span = self.cumulative[upper] - self.cumulative[lower];
        let t = if span <= f64::EPSILON {
            0.0
        } else {
            (distance - self.cumulative[lower]) / span
        };
        self.samples[lower].lerp(self.samples[upper], t)
    }

    /// A sampled sub-path, including exact interpolated end points. Rounded
    /// corners use samples no farther than four canvas pixels apart.
    pub fn slice(&self, start: f64, end: f64) -> Vec<Point> {
        if self.samples.is_empty() {
            return Vec::new();
        }
        let start = start.clamp(0.0, self.length);
        let end = end.clamp(start, self.length);
        let mut out = vec![self.point_at_distance(start)];
        for (point, distance) in self.samples.iter().zip(&self.cumulative) {
            if *distance > start && *distance < end {
                out.push(*point);
            }
        }
        let last = self.point_at_distance(end);
        if out.last().copied() != Some(last) {
            out.push(last);
        }
        out
    }
}

pub fn cubic_controls(from: Point, to: Point) -> (Point, Point) {
    let dx = ((to.x - from.x).abs() * 0.5).max(48.0);
    (Point::new(from.x + dx, from.y), Point::new(to.x - dx, to.y))
}

fn cubic_route(from: Point, to: Point) -> WireRoute {
    let (control_1, control_2) = cubic_controls(from, to);
    let samples = (0..=64)
        .map(|step| cubic_point(from, control_1, control_2, to, step as f64 / 64.0))
        .collect();
    build_route(from, to, RouteKind::Cubic { control_1, control_2 }, samples)
}

fn cubic_point(from: Point, c1: Point, c2: Point, to: Point, t: f64) -> Point {
    let u = 1.0 - t;
    Point::new(
        u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
        u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
    )
}

fn build_route(from: Point, to: Point, kind: RouteKind, samples: Vec<Point>) -> WireRoute {
    let mut cumulative = Vec::with_capacity(samples.len());
    let mut length = 0.0;
    cumulative.push(0.0);
    for pair in samples.windows(2) {
        length += pair[0].distance(pair[1]);
        cumulative.push(length);
    }
    WireRoute { from, to, kind, samples, cumulative, length }
}

/// Route one output-to-input cable. `corridor_offset` is stable per parallel
/// cable (normally multiples of `RouteStyle::cable_spacing`).
pub fn route_wire(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
) -> WireRoute {
    let straight = cubic_route(from, to);
    if !cubic_intersects(&straight, obstacles) {
        return straight;
    }

    let radius = style.corner_radius.max(16.0);
    let stub = style.port_stub.max(radius * 2.0).max(24.0);
    let source_stub = Point::new(from.x + stub, from.y);
    let target_stub = Point::new(to.x - stub, to.y);
    if !segment_clear(from, source_stub, obstacles)
        || !segment_clear(target_stub, to, obstacles)
    {
        return straight;
    }

    // Reserving the fillet radius around every card ensures the quadratic
    // corner, which lies inside its orthogonal guide, keeps the caller's full
    // card margin.
    let reserve = radius + corridor_offset.abs();
    let guides: Vec<Obstacle> = obstacles.iter().map(|rect| rect.inflate(reserve)).collect();
    let mut candidates = Vec::with_capacity(3);

    if source_stub.x <= target_stub.x {
        if let Some(channel) = choose_forward_channel(source_stub, target_stub, &guides, corridor_offset)
        {
            candidates.push(vec![
                from,
                source_stub,
                Point::new(channel, source_stub.y),
                Point::new(channel, target_stub.y),
                target_stub,
                to,
            ]);
        }
    }

    if let Some((above, below)) = outside_rows(from, to, &guides, radius, corridor_offset) {
        if source_stub.x <= target_stub.x {
            candidates.push(vec![
                from,
                source_stub,
                Point::new(source_stub.x, above),
                Point::new(target_stub.x, above),
                target_stub,
                to,
            ]);
            candidates.push(vec![
                from,
                source_stub,
                Point::new(source_stub.x, below),
                Point::new(target_stub.x, below),
                target_stub,
                to,
            ]);
        } else {
            let right = guides
                .iter()
                .fold(source_stub.x.max(to.x), |x, rect| x.max(rect.max.x))
                + radius
                + corridor_offset.max(0.0);
            candidates.push(vec![
                from,
                source_stub,
                Point::new(right, source_stub.y),
                Point::new(right, above),
                Point::new(target_stub.x, above),
                target_stub,
                to,
            ]);
            candidates.push(vec![
                from,
                source_stub,
                Point::new(right, source_stub.y),
                Point::new(right, below),
                Point::new(target_stub.x, below),
                target_stub,
                to,
            ]);
        }
    }

    let best = candidates
        .into_iter()
        .map(simplify)
        .filter(|points| valid_orthogonal(points, &guides, radius))
        .min_by(|left, right| compare_routes(left, right));
    let Some(points) = best else {
        return straight;
    };
    let samples = rounded_samples(&points, radius);
    build_route(from, to, RouteKind::Orthogonal { points, radius }, samples)
}

/// Ease used by the 600 ms value pulse. Kept here so animation timing is as
/// deterministic and unit-testable as routing.
pub fn pulse_progress(elapsed_seconds: f64) -> f64 {
    let t = (elapsed_seconds / 0.6).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn cubic_intersects(route: &WireRoute, obstacles: &[Obstacle]) -> bool {
    route
        .samples
        .windows(2)
        .any(|pair| !segment_clear(pair[0], pair[1], obstacles))
}

fn outside_rows(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    radius: f64,
    offset: f64,
) -> Option<(f64, f64)> {
    let mut top = from.y.min(to.y);
    let mut bottom = from.y.max(to.y);
    for rect in obstacles {
        top = top.min(rect.min.y);
        bottom = bottom.max(rect.max.y);
    }
    Some((top - radius + offset, bottom + radius + offset))
}

/// Find a clear vertical track between the stubs. Forbidden x intervals are
/// accumulated in one pass and merged after sorting. This is O(cards log
/// cards), with route validation remaining O(cards).
fn choose_forward_channel(from: Point, to: Point, obstacles: &[Obstacle], offset: f64) -> Option<f64> {
    let low = from.x.min(to.x);
    let high = from.x.max(to.x);
    if high - low < 32.0 {
        return None;
    }
    let y_low = from.y.min(to.y);
    let y_high = from.y.max(to.y);
    let mut forbidden = Vec::new();
    for rect in obstacles {
        if ranges_overlap_open(y_low, y_high, rect.min.y, rect.max.y) {
            forbidden.push((rect.min.x.max(low), rect.max.x.min(high)));
        }
        if from.y > rect.min.y && from.y < rect.max.y {
            if rect.min.x >= from.x {
                forbidden.push((rect.min.x.max(low), high));
            } else if rect.max.x <= from.x {
                forbidden.push((low, rect.max.x.min(high)));
            } else {
                return None;
            }
        }
        if to.y > rect.min.y && to.y < rect.max.y {
            if rect.min.x >= to.x {
                forbidden.push((rect.min.x.max(low), high));
            } else if rect.max.x <= to.x {
                forbidden.push((low, rect.max.x.min(high)));
            } else {
                return None;
            }
        }
    }
    forbidden.retain(|(a, b)| b > a);
    forbidden.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    let mut gaps = Vec::new();
    let mut cursor = low;
    for (start, end) in forbidden {
        if start > cursor {
            gaps.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < high {
        gaps.push((cursor, high));
    }
    let desired = (low + high) * 0.5 + offset;
    gaps.into_iter()
        .filter(|(a, b)| b - a >= 32.0)
        .map(|(a, b)| desired.clamp(a + 16.0, b - 16.0))
        .min_by(|a, b| {
            (a - desired)
                .abs()
                .partial_cmp(&(b - desired).abs())
                .unwrap_or(Ordering::Equal)
        })
}

fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if out.last().copied() == Some(point) {
            continue;
        }
        while out.len() >= 2 {
            let a = out[out.len() - 2];
            let b = out[out.len() - 1];
            if (a.x - b.x).abs() < 1e-6 && (b.x - point.x).abs() < 1e-6
                || (a.y - b.y).abs() < 1e-6 && (b.y - point.y).abs() < 1e-6
            {
                out.pop();
            } else {
                break;
            }
        }
        out.push(point);
    }
    out
}

fn valid_orthogonal(points: &[Point], obstacles: &[Obstacle], radius: f64) -> bool {
    if points.len() < 4
        || points.windows(2).any(|pair| {
            (pair[0].x - pair[1].x).abs() > 1e-6 && (pair[0].y - pair[1].y).abs() > 1e-6
        })
        || points
            .windows(2)
            .any(|pair| !segment_clear(pair[0], pair[1], obstacles))
    {
        return false;
    }
    for index in 0..points.len() - 1 {
        let length = points[index].distance(points[index + 1]);
        let required = if index > 0 && index + 1 < points.len() - 1 {
            radius * 2.0
        } else {
            radius
        };
        if length + 1e-6 < required {
            return false;
        }
    }
    true
}

fn compare_routes(left: &[Point], right: &[Point]) -> Ordering {
    let left_bends = left.len().saturating_sub(2);
    let right_bends = right.len().saturating_sub(2);
    left_bends.cmp(&right_bends).then_with(|| {
        path_length(left)
            .partial_cmp(&path_length(right))
            .unwrap_or(Ordering::Equal)
    })
}

fn path_length(points: &[Point]) -> f64 {
    points.windows(2).map(|pair| pair[0].distance(pair[1])).sum()
}

fn segment_clear(a: Point, b: Point, obstacles: &[Obstacle]) -> bool {
    obstacles.iter().all(|rect| {
        if rect.contains_strict(a) || rect.contains_strict(b) {
            return false;
        }
        if (a.x - b.x).abs() < 1e-6 {
            !(a.x > rect.min.x
                && a.x < rect.max.x
                && ranges_overlap_open(a.y, b.y, rect.min.y, rect.max.y))
        } else if (a.y - b.y).abs() < 1e-6 {
            !(a.y > rect.min.y
                && a.y < rect.max.y
                && ranges_overlap_open(a.x, b.x, rect.min.x, rect.max.x))
        } else {
            !line_intersects_rect(a, b, *rect)
        }
    })
}

fn ranges_overlap_open(a: f64, b: f64, c: f64, d: f64) -> bool {
    a.min(b) < d && a.max(b) > c
}

fn line_intersects_rect(a: Point, b: Point, rect: Obstacle) -> bool {
    // Liang-Barsky against the open interior. Shrinking by a tiny epsilon
    // allows a cable to run exactly on a reserved guide boundary.
    let epsilon = 1e-7;
    let min_x = rect.min.x + epsilon;
    let min_y = rect.min.y + epsilon;
    let max_x = rect.max.x - epsilon;
    let max_y = rect.max.y - epsilon;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (p, q) in [(-dx, a.x - min_x), (dx, max_x - a.x), (-dy, a.y - min_y), (dy, max_y - a.y)] {
        if p.abs() < epsilon {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

fn rounded_samples(points: &[Point], radius: f64) -> Vec<Point> {
    let mut out = vec![points[0]];
    for index in 1..points.len() - 1 {
        let before = points[index - 1];
        let corner = points[index];
        let after = points[index + 1];
        let in_len = before.distance(corner);
        let out_len = corner.distance(after);
        let r = radius.min(in_len * 0.5).min(out_len * 0.5);
        let entry = move_toward(corner, before, r);
        let exit = move_toward(corner, after, r);
        if out.last().copied() != Some(entry) {
            out.push(entry);
        }
        // A real quadratic fillet, sampled at <= ~4 px even at 3x display.
        let steps = ((r * std::f64::consts::FRAC_PI_2 / 4.0).ceil() as usize).max(8);
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let u = 1.0 - t;
            out.push(Point::new(
                u * u * entry.x + 2.0 * u * t * corner.x + t * t * exit.x,
                u * u * entry.y + 2.0 * u * t * corner.y + t * t * exit.y,
            ));
        }
    }
    if out.last().copied() != points.last().copied() {
        out.push(*points.last().unwrap());
    }
    out
}

fn move_toward(from: Point, to: Point, distance: f64) -> Point {
    let length = from.distance(to);
    if length <= f64::EPSILON {
        from
    } else {
        from.lerp(to, distance / length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(from: Point, to: Point, obstacles: &[Obstacle]) -> WireRoute {
        route_wire(from, to, obstacles, RouteStyle::default(), 0.0)
    }

    #[test]
    fn straight_when_clear() {
        let route = route(Point::new(0.0, 20.0), Point::new(240.0, 80.0), &[]);
        assert!(route.is_straight_cubic());
    }

    #[test]
    fn detours_around_card_between_ports() {
        let obstacle = Obstacle::from_xywh(90.0, -10.0, 60.0, 80.0);
        let route = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &[obstacle]);
        let RouteKind::Orthogonal { points, .. } = &route.kind else {
            panic!("expected detour");
        };
        assert!(points.iter().any(|point| point.y < obstacle.min.y || point.y > obstacle.max.y));
        assert!(route.samples.windows(2).all(|pair| segment_clear(pair[0], pair[1], &[obstacle])));
    }

    #[test]
    fn target_left_uses_shorter_outside_row() {
        let obstacle = Obstacle::from_xywh(40.0, 0.0, 100.0, 120.0);
        let route = route(Point::new(220.0, 30.0), Point::new(0.0, 50.0), &[obstacle]);
        let RouteKind::Orthogonal { points, .. } = &route.kind else {
            panic!("expected leftward detour");
        };
        let min_y = points.iter().map(|point| point.y).fold(f64::INFINITY, f64::min);
        let max_y = points.iter().map(|point| point.y).fold(f64::NEG_INFINITY, f64::max);
        assert!(min_y < obstacle.min.y, "shorter route should pass above: {points:?}");
        assert!(max_y < obstacle.max.y + 32.0);
    }

    #[test]
    fn dense_blocking_stubs_falls_back_to_cubic() {
        let obstacles = [
            Obstacle::from_xywh(20.0, -20.0, 60.0, 40.0),
            Obstacle::from_xywh(100.0, -100.0, 60.0, 200.0),
        ];
        let route = route(Point::new(0.0, 0.0), Point::new(220.0, 0.0), &obstacles);
        assert!(route.is_straight_cubic());
    }

    #[test]
    fn routing_is_deterministic() {
        let obstacles = [Obstacle::from_xywh(90.0, -10.0, 60.0, 80.0)];
        let one = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &obstacles);
        let two = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &obstacles);
        assert_eq!(one, two);
    }

    #[test]
    fn arc_length_parameterisation_has_exact_ends_and_midpoint() {
        let route = route(Point::new(0.0, 0.0), Point::new(100.0, 0.0), &[]);
        assert_eq!(route.point_at(0.0), Point::new(0.0, 0.0));
        assert!((route.point_at(0.5).x - 50.0).abs() < 0.01);
        assert_eq!(route.point_at(1.0), Point::new(100.0, 0.0));
    }

    #[test]
    fn pulse_position_is_monotonic() {
        let mut previous = 0.0;
        for step in 0..=60 {
            let current = pulse_progress(step as f64 / 100.0);
            assert!(current >= previous);
            previous = current;
        }
        assert_eq!(previous, 1.0);
    }

    #[test]
    fn corridor_offset_spreads_parallel_cables() {
        let obstacle = Obstacle::from_xywh(90.0, -10.0, 60.0, 80.0);
        let style = RouteStyle::default();
        let one = route_wire(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &[obstacle], style, -4.0);
        let two = route_wire(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &[obstacle], style, 4.0);
        assert_ne!(one.point_at(0.5), two.point_at(0.5));
        assert!((one.point_at(0.5).y - two.point_at(0.5).y).abs() >= style.cable_spacing - 0.1);
    }
}
