//! Deterministic, zoom-independent cable routing in canvas units.
//!
//! Cards passed to [`route_wire`] are already inflated by the caller's card
//! clearance. The router reserves another fillet radius while choosing an
//! orthogonal track, so rounding a corner cannot cut back through that
//! clearance.

use std::cmp::Ordering;

const ENDPOINT_STUB_DISTANCE: f64 = 24.0;
const COLLISION_TOLERANCE: f64 = 4.0;

/// The card edge occupied by a port. For a target this is the side the
/// cable approaches from; for a source it is the side the cable leaves.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum PortSide {
    Left,
    Right,
}

impl PortSide {
    fn sign(self) -> f64 {
        match self {
            Self::Left => -1.0,
            Self::Right => 1.0,
        }
    }
}

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

    pub fn bends(&self) -> usize {
        match &self.kind {
            RouteKind::Cubic { .. } => 0,
            RouteKind::Orthogonal { points, .. } => points.len().saturating_sub(2),
        }
    }

    pub fn is_loop(&self) -> bool {
        self.length > self.from.distance(self.to) * 1.8
    }

    /// Number of proper crossings with another cable. Shared cable endpoints
    /// and collinear runs are not crossings; repeated sampled hits at the same
    /// geometric intersection are collapsed.
    pub fn crossings_with(&self, other: &Self) -> usize {
        let mut intersections = Vec::new();
        for left in self.samples.windows(2) {
            for right in other.samples.windows(2) {
                let Some(point) = segment_intersection(left[0], left[1], right[0], right[1]) else {
                    continue;
                };
                if [self.from, self.to, other.from, other.to]
                    .iter()
                    .any(|endpoint| endpoint.distance(point) < 1e-4)
                {
                    continue;
                }
                if !intersections
                    .iter()
                    .any(|old: &Point| old.distance(point) < 0.25)
                {
                    intersections.push(point);
                }
            }
        }
        intersections.len()
    }

    pub fn intersects_segment(&self, from: Point, to: Point) -> bool {
        self.samples
            .windows(2)
            .any(|pair| segment_intersection(pair[0], pair[1], from, to).is_some())
    }

    /// Arc-length midpoint and its unit tangent in source-to-target order.
    pub fn midpoint_tangent(&self) -> (Point, Point) {
        let point = self.point_at_distance(self.length * 0.5);
        if self.samples.len() < 2 || self.length <= f64::EPSILON {
            return (point, Point::new(1.0, 0.0));
        }
        let upper = self
            .cumulative
            .partition_point(|value| *value < self.length * 0.5)
            .clamp(1, self.samples.len() - 1);
        for radius in 0..self.samples.len() {
            let lower = upper.saturating_sub(1 + radius);
            let upper = (upper + radius).min(self.samples.len() - 1);
            let dx = self.samples[upper].x - self.samples[lower].x;
            let dy = self.samples[upper].y - self.samples[lower].y;
            let length = (dx * dx + dy * dy).sqrt();
            if length > f64::EPSILON {
                return (point, Point::new(dx / length, dy / length));
            }
        }
        (point, Point::new(1.0, 0.0))
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

pub fn directional_cubic_controls(
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
) -> (Point, Point) {
    let dx = ((to.x - from.x).abs() * 0.5).min(60.0);
    (
        Point::new(from.x + source_side.sign() * dx, from.y),
        Point::new(to.x + target_side.sign() * dx, to.y),
    )
}

fn cubic_route(from: Point, source_side: PortSide, to: Point, target_side: PortSide) -> WireRoute {
    let (control_1, control_2) = directional_cubic_controls(from, source_side, to, target_side);
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
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
) -> WireRoute {
    let radius = style.corner_radius.max(16.0);
    let stub = style.port_stub.max(radius * 2.0).max(24.0);
    let source_stub = Point::new(from.x + source_side.sign() * stub, from.y);
    let target_stub = Point::new(to.x + target_side.sign() * stub, to.y);
    let source_owner = endpoint_obstacle(from, obstacles, source_side);
    let target_owner = endpoint_obstacle(to, obstacles, target_side);

    let straight = cubic_route(from, source_side, to, target_side);
    if route_samples_clear(
        &straight.samples,
        obstacles,
        source_owner,
        target_owner,
    ) {
        return straight;
    }

    if !segment_clear_except(from, source_stub, obstacles, source_owner)
        || !segment_clear_except(target_stub, to, obstacles, target_owner)
    {
        return straight;
    }

    // Reserving the fillet radius around every card ensures the quadratic
    // corner, which lies inside its orthogonal guide, keeps the caller's full
    // card margin.
    let reserve = radius + corridor_offset.abs();
    let guides: Vec<Obstacle> = obstacles.iter().map(|rect| rect.inflate(reserve)).collect();
    let mut candidates = Vec::with_capacity(3 + obstacles.len() * 2);

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

    // Backward and vertically stacked connections first follow each port's
    // requested side, cross on the shortest clear row, then approach the
    // target from its requested side. Trying every obstacle boundary also
    // finds the useful row between two stacked cards instead of needlessly
    // going around the entire graph.
    for row in routing_rows(from, to, obstacles, radius, corridor_offset) {
        candidates.push(vec![
            from,
            source_stub,
            Point::new(source_stub.x, row),
            Point::new(target_stub.x, row),
            target_stub,
            to,
        ]);
    }

    let best = candidates
        .into_iter()
        .map(simplify)
        .filter_map(|points| {
            if !valid_orthogonal(&points, radius) {
                return None;
            }
            let samples = rounded_samples(&points, radius);
            route_samples_clear(&samples, obstacles, source_owner, target_owner).then(|| {
                build_route(from, to, RouteKind::Orthogonal { points, radius }, samples)
            })
        })
        .min_by(|left, right| compare_routes(left, right));
    best.unwrap_or(straight)
}

/// Ease used by the 600 ms value pulse. Kept here so animation timing is as
/// deterministic and unit-testable as routing.
pub fn pulse_progress(elapsed_seconds: f64) -> f64 {
    let t = (elapsed_seconds / 0.6).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn endpoint_obstacle(point: Point, obstacles: &[Obstacle], side: PortSide) -> Option<usize> {
    obstacles
        .iter()
        .enumerate()
        .filter_map(|(index, rect)| {
            let contains = point.x >= rect.min.x
                && point.x <= rect.max.x
                && point.y >= rect.min.y
                && point.y <= rect.max.y;
            contains.then_some((
                index,
                match side {
                    PortSide::Right => rect.max.x - point.x,
                    PortSide::Left => point.x - rect.min.x,
                },
            ))
        })
        .min_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        .map(|(index, _)| index)
}

fn routing_rows(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    radius: f64,
    offset: f64,
) -> Vec<f64> {
    let mut rows = vec![from.y, to.y];
    for rect in obstacles {
        rows.push(rect.min.y - radius + offset);
        rows.push(rect.max.y + radius + offset);
    }
    rows.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    rows.dedup_by(|left, right| (*left - *right).abs() < 1e-6);
    rows
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
            let straight_x = (a.x - b.x).abs() < 1e-6 && (b.x - point.x).abs() < 1e-6;
            let straight_y = (a.y - b.y).abs() < 1e-6 && (b.y - point.y).abs() < 1e-6;
            let same_direction = (b.x - a.x) * (point.x - b.x)
                + (b.y - a.y) * (point.y - b.y)
                >= 0.0;
            if (straight_x || straight_y) && same_direction
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

fn valid_orthogonal(
    points: &[Point],
    radius: f64,
) -> bool {
    if points.len() < 4
        || points.windows(2).any(|pair| {
            (pair[0].x - pair[1].x).abs() > 1e-6 && (pair[0].y - pair[1].y).abs() > 1e-6
        })
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

/// Validate the sampled rendered path against the caller's margin-inflated
/// obstacles. An endpoint's owner card is ignored only for the first/last
/// port-stub distance along the route, with one sample's collision tolerance.
fn route_samples_clear(
    samples: &[Point],
    obstacles: &[Obstacle],
    source_owner: Option<usize>,
    target_owner: Option<usize>,
) -> bool {
    let length = path_length(samples);
    let endpoint_exemption = ENDPOINT_STUB_DISTANCE + COLLISION_TOLERANCE;
    let mut segment_start = 0.0;

    samples.windows(2).all(|pair| {
        let segment_end = segment_start + pair[0].distance(pair[1]);
        let segment_length = segment_end - segment_start;
        let clear = obstacles.iter().enumerate().all(|(obstacle_index, rect)| {
            let mut check_start = segment_start;
            let mut check_end = segment_end;
            if source_owner == Some(obstacle_index) {
                check_start = check_start.max(endpoint_exemption);
            }
            if target_owner == Some(obstacle_index) {
                check_end = check_end.min(length - endpoint_exemption);
            }
            if check_start >= check_end || segment_length <= f64::EPSILON {
                return true;
            }
            let a = pair[0].lerp(pair[1], (check_start - segment_start) / segment_length);
            let b = pair[0].lerp(pair[1], (check_end - segment_start) / segment_length);
            segment_clear_rect(a, b, *rect)
        });
        segment_start = segment_end;
        clear
    })
}

fn compare_routes(left: &WireRoute, right: &WireRoute) -> Ordering {
    left.bends().cmp(&right.bends()).then_with(|| {
        left.length()
            .partial_cmp(&right.length())
            .unwrap_or(Ordering::Equal)
    })
}

fn path_length(points: &[Point]) -> f64 {
    points.windows(2).map(|pair| pair[0].distance(pair[1])).sum()
}

fn segment_intersection(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let epsilon = 1e-8;
    if a.x.max(b.x) + epsilon < c.x.min(d.x)
        || c.x.max(d.x) + epsilon < a.x.min(b.x)
        || a.y.max(b.y) + epsilon < c.y.min(d.y)
        || c.y.max(d.y) + epsilon < a.y.min(b.y)
    {
        return None;
    }
    let ab = Point::new(b.x - a.x, b.y - a.y);
    let cd = Point::new(d.x - c.x, d.y - c.y);
    let denominator = ab.x * cd.y - ab.y * cd.x;
    if denominator.abs() < 1e-8 {
        return None;
    }
    let ac = Point::new(c.x - a.x, c.y - a.y);
    let t = (ac.x * cd.y - ac.y * cd.x) / denominator;
    let u = (ac.x * ab.y - ac.y * ab.x) / denominator;
    if t < -epsilon || t > 1.0 + epsilon || u < -epsilon || u > 1.0 + epsilon {
        return None;
    }
    Some(Point::new(a.x + ab.x * t, a.y + ab.y * t))
}

#[cfg(test)]
fn segment_clear(a: Point, b: Point, obstacles: &[Obstacle]) -> bool {
    segment_clear_except(a, b, obstacles, None)
}

fn segment_clear_except(a: Point, b: Point, obstacles: &[Obstacle], ignored: Option<usize>) -> bool {
    obstacles.iter().enumerate().all(|(index, rect)| {
        if ignored == Some(index) {
            return true;
        }
        segment_clear_rect(a, b, *rect)
    })
}

fn segment_clear_rect(a: Point, b: Point, rect: Obstacle) -> bool {
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
        !line_intersects_rect(a, b, rect)
    }
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
        route_wire(
            from,
            PortSide::Right,
            to,
            PortSide::Left,
            obstacles,
            RouteStyle::default(),
            0.0,
        )
    }

    fn card(x: f64, y: f64, width: f64, height: f64) -> Obstacle {
        Obstacle::from_xywh(x, y, width, height).inflate(12.0)
    }

    fn facing_endpoint_cards(source_right: f64, target_left: f64) -> [Obstacle; 2] {
        [
            card(source_right - 300.0, 100.0, 300.0, 200.0),
            card(target_left, 100.0, 300.0, 200.0),
        ]
    }

    fn assert_route_misses_cards(route: &WireRoute, obstacles: &[Obstacle]) {
        assert!(route_samples_clear(
            &route.samples,
            obstacles,
            endpoint_obstacle(route.from, obstacles, PortSide::Right),
            endpoint_obstacle(route.to, obstacles, PortSide::Left),
        ));
    }

    #[test]
    fn straight_when_clear() {
        let route = route(Point::new(0.0, 20.0), Point::new(240.0, 80.0), &[]);
        assert!(route.is_straight_cubic());
    }

    #[test]
    fn first_grab_geometry_prefers_clear_cubic() {
        let obstacles = facing_endpoint_cards(400.0, 540.0);
        let route = route(Point::new(400.0, 220.0), Point::new(540.0, 180.0), &obstacles);
        let RouteKind::Cubic { control_1, control_2 } = route.kind else {
            panic!("clear near-horizontal ports should use a cubic");
        };
        assert_eq!(route.bends(), 0);
        assert_eq!(control_1, Point::new(460.0, 220.0));
        assert_eq!(control_2, Point::new(480.0, 180.0));
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn second_grab_geometry_prefers_clear_cubic() {
        let obstacles = facing_endpoint_cards(160.0, 305.0);
        let route = route(Point::new(160.0, 190.0), Point::new(305.0, 155.0), &obstacles);
        assert!(route.is_straight_cubic());
        assert_eq!(route.bends(), 0);
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn blocking_card_uses_an_orthogonal_route() {
        let mut obstacles = facing_endpoint_cards(400.0, 540.0).to_vec();
        obstacles.push(card(460.0, 160.0, 20.0, 80.0));
        let route = route(Point::new(400.0, 220.0), Point::new(540.0, 180.0), &obstacles);
        assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
        assert!(route.bends() > 0);
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn level_ports_produce_a_straight_segment() {
        let obstacles = facing_endpoint_cards(400.0, 540.0);
        let route = route(Point::new(400.0, 180.0), Point::new(540.0, 180.0), &obstacles);
        assert_eq!(route.bends(), 0);
        assert!(route.samples.iter().all(|point| point.y == 180.0));
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn short_hop_tangents_scale_without_looping() {
        let from = Point::new(100.0, 20.0);
        let to = Point::new(110.0, 30.0);
        let (control_1, control_2) =
            directional_cubic_controls(from, PortSide::Right, to, PortSide::Left);
        assert_eq!(control_1, Point::new(105.0, 20.0));
        assert_eq!(control_2, Point::new(105.0, 30.0));
    }

    #[test]
    fn cubic_tangents_honour_all_four_facing_combinations() {
        let from = Point::new(100.0, 20.0);
        let to = Point::new(300.0, 80.0);
        for (source_side, target_side) in [
            (PortSide::Right, PortSide::Left),
            (PortSide::Right, PortSide::Right),
            (PortSide::Left, PortSide::Left),
            (PortSide::Left, PortSide::Right),
        ] {
            let route = route_wire(
                from,
                source_side,
                to,
                target_side,
                &[],
                RouteStyle::default(),
                0.0,
            );
            let RouteKind::Cubic { control_1, control_2 } = route.kind else {
                panic!("clear route should use cubic");
            };
            assert_eq!((control_1.x - from.x).signum(), source_side.sign());
            assert_eq!((control_2.x - to.x).signum(), target_side.sign());
            assert_eq!(control_1.y, from.y);
            assert_eq!(control_2.y, to.y);
        }
    }

    #[test]
    fn corridor_tangents_honour_all_four_facing_combinations() {
        let from = Point::new(0.0, 20.0);
        let to = Point::new(300.0, 80.0);
        let obstacle = Obstacle::from_xywh(120.0, -100.0, 60.0, 300.0);
        for (source_side, target_side) in [
            (PortSide::Right, PortSide::Left),
            (PortSide::Right, PortSide::Right),
            (PortSide::Left, PortSide::Left),
            (PortSide::Left, PortSide::Right),
        ] {
            let route = route_wire(
                from,
                source_side,
                to,
                target_side,
                &[obstacle],
                RouteStyle::default(),
                0.0,
            );
            let RouteKind::Orthogonal { points, .. } = route.kind else {
                panic!("blocking card should require a corridor");
            };
            assert_eq!((points[1].x - from.x).signum(), source_side.sign());
            assert_eq!(
                (points[points.len() - 2].x - to.x).signum(),
                target_side.sign()
            );
        }
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
    fn stacked_image_to_picture_keeps_every_middle_segment_outside_endpoint_cards() {
        let obstacles = [
            card(40.0, 60.0, 330.0, 165.0),
            card(40.0, 360.0, 330.0, 165.0),
            card(420.0, 60.0, 330.0, 430.0),
            card(420.0, 520.0, 330.0, 165.0),
        ];
        let route = route(Point::new(750.0, 86.0), Point::new(420.0, 546.0), &obstacles);
        let RouteKind::Orthogonal { points, .. } = &route.kind else {
            panic!("stacked endpoint cards require a corridor");
        };
        assert!(points[1].x >= route.from.x + 24.0, "{points:?}");
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn below_left_prompt_to_expand_uses_clear_right_drop_and_cross_row() {
        let obstacles = [
            card(40.0, 60.0, 330.0, 165.0),
            card(40.0, 360.0, 330.0, 165.0),
            card(420.0, 60.0, 330.0, 430.0),
            card(420.0, 520.0, 330.0, 165.0),
        ];
        let route = route(Point::new(370.0, 86.0), Point::new(40.0, 386.0), &obstacles);
        let RouteKind::Orthogonal { points, .. } = &route.kind else {
            panic!("below-left endpoint cards require a corridor");
        };
        assert!(points[1].x >= route.from.x + 24.0, "{points:?}");
        assert!(
            points.iter().any(|point| point.y > 237.0 && point.y < 348.0),
            "route should cross between the stacked cards: {points:?}"
        );
        assert_route_misses_cards(&route, &obstacles);
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
        let one = route_wire(
            Point::new(0.0, 20.0),
            PortSide::Right,
            Point::new(240.0, 20.0),
            PortSide::Left,
            &[obstacle],
            style,
            -4.0,
        );
        let two = route_wire(
            Point::new(0.0, 20.0),
            PortSide::Right,
            Point::new(240.0, 20.0),
            PortSide::Left,
            &[obstacle],
            style,
            4.0,
        );
        assert_ne!(one.point_at(0.5), two.point_at(0.5));
        assert!((one.point_at(0.5).y - two.point_at(0.5).y).abs() >= style.cable_spacing - 0.1);
    }

    #[test]
    fn crossing_count_finds_one_known_intersection() {
        let down = build_route(
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            RouteKind::Orthogonal {
                points: vec![Point::new(0.0, 0.0), Point::new(100.0, 100.0)],
                radius: 0.0,
            },
            vec![Point::new(0.0, 0.0), Point::new(100.0, 100.0)],
        );
        let up = build_route(
            Point::new(0.0, 100.0),
            Point::new(100.0, 0.0),
            RouteKind::Orthogonal {
                points: vec![Point::new(0.0, 100.0), Point::new(100.0, 0.0)],
                radius: 0.0,
            },
            vec![Point::new(0.0, 100.0), Point::new(100.0, 0.0)],
        );
        assert_eq!(down.crossings_with(&up), 1);
    }

    #[test]
    fn loop_detection_uses_straight_distance_ratio() {
        let looped = build_route(
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            RouteKind::Orthogonal {
                points: vec![
                    Point::new(0.0, 0.0),
                    Point::new(0.0, 100.0),
                    Point::new(100.0, 100.0),
                    Point::new(100.0, 0.0),
                ],
                radius: 0.0,
            },
            vec![
                Point::new(0.0, 0.0),
                Point::new(0.0, 100.0),
                Point::new(100.0, 100.0),
                Point::new(100.0, 0.0),
            ],
        );
        assert!(looped.is_loop());
        assert!(!route(Point::new(0.0, 0.0), Point::new(100.0, 0.0), &[]).is_loop());
    }

    #[test]
    fn midpoint_tangent_orients_straight_and_s_routes() {
        let straight = route(Point::new(0.0, 0.0), Point::new(100.0, 0.0), &[]);
        let (_, tangent) = straight.midpoint_tangent();
        assert!(tangent.x > 0.99 && tangent.y.abs() < 0.01);

        let from = Point::new(0.0, 0.0);
        let control_1 = Point::new(100.0, 0.0);
        let control_2 = Point::new(0.0, 100.0);
        let to = Point::new(100.0, 100.0);
        let samples = (0..=64)
            .map(|step| cubic_point(from, control_1, control_2, to, step as f64 / 64.0))
            .collect();
        let s_route = build_route(
            from,
            to,
            RouteKind::Cubic { control_1, control_2 },
            samples,
        );
        let (_, tangent) = s_route.midpoint_tangent();
        assert!(tangent.x.abs() < 0.05 && tangent.y > 0.99, "{tangent:?}");
    }
}
