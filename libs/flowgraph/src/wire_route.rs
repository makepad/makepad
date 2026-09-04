//! Deterministic, zoom-independent cable routing in canvas units.
//!
//! Cards passed to [`route_wire`] are already inflated by the caller's card
//! clearance. The router reserves another fillet radius while choosing an
//! orthogonal track, so rounding a corner cannot cut back through that
//! clearance.

use std::cmp::Ordering;

const ENDPOINT_STUB_DISTANCE: f64 = 24.0;
const COLLISION_TOLERANCE: f64 = 4.0;
const CHANNEL_CELL: f64 = 8.0;
const ROUTE_SWITCH_RATIO: f64 = 0.95;
const CALLER_CARD_CLEARANCE: f64 = 12.0;
const NARROW_CLEARANCE: f64 = 6.0;
const NARROW_RADIUS: f64 = 8.0;

/// One visual routing language for every cable on a canvas.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum WireMode {
    Bezier,
    #[default]
    Routed,
}

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

    fn intersects(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }
}

/// Obstacles close enough to affect this edge. Keeping this set local is also
/// what lets a caller cache each cable independently while an unrelated card
/// is moving elsewhere on the canvas.
pub fn obstacles_in_corridor(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    margin: f64,
) -> Vec<Obstacle> {
    let corridor = Obstacle {
        min: Point::new(from.x.min(to.x) - margin, from.y.min(to.y) - margin),
        max: Point::new(from.x.max(to.x) + margin, from.y.max(to.y) + margin),
    };
    obstacles
        .iter()
        .copied()
        .filter(|obstacle| obstacle.intersects(corridor))
        .collect()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteChoice {
    Straight,
    CorridorX,
    Above,
    Below,
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
    choice: RouteChoice,
    tie_x: f64,
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

    /// Shortest distance from a point to the cached, rendered polyline.
    pub fn distance_to_point(&self, point: Point) -> f64 {
        self.samples
            .windows(2)
            .map(|pair| point_segment_distance(point, pair[0], pair[1]))
            .fold(f64::INFINITY, f64::min)
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
    build_route(
        from,
        to,
        RouteKind::Cubic { control_1, control_2 },
        samples,
        RouteChoice::Straight,
        f64::INFINITY,
    )
}

fn cubic_point(from: Point, c1: Point, c2: Point, to: Point, t: f64) -> Point {
    let u = 1.0 - t;
    Point::new(
        u * u * u * from.x + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * to.x,
        u * u * u * from.y + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * to.y,
    )
}

fn build_route(
    from: Point,
    to: Point,
    kind: RouteKind,
    samples: Vec<Point>,
    choice: RouteChoice,
    tie_x: f64,
) -> WireRoute {
    let mut cumulative = Vec::with_capacity(samples.len());
    let mut length = 0.0;
    cumulative.push(0.0);
    for pair in samples.windows(2) {
        length += pair[0].distance(pair[1]);
        cumulative.push(length);
    }
    WireRoute {
        from,
        to,
        kind,
        samples,
        cumulative,
        length,
        choice,
        tie_x,
    }
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
    route_wire_in_mode(
        WireMode::Routed,
        from,
        source_side,
        to,
        target_side,
        obstacles,
        style,
        corridor_offset,
    )
}

/// Route one cable in the canvas's selected visual mode.
pub fn route_wire_in_mode(
    mode: WireMode,
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
) -> WireRoute {
    route_wire_sticky_in_mode(
        mode,
        from,
        source_side,
        to,
        target_side,
        obstacles,
        style,
        corridor_offset,
        None,
    )
}

/// Route a cable while retaining the previous broad route choice unless a
/// different choice is at least five percent shorter.
pub fn route_wire_sticky(
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
    previous: Option<&WireRoute>,
) -> WireRoute {
    route_wire_sticky_in_mode(
        WireMode::Routed,
        from,
        source_side,
        to,
        target_side,
        obstacles,
        style,
        corridor_offset,
        previous,
    )
}

/// Mode-aware sticky routing. Bezier mode deliberately ignores obstacles,
/// offsets, and the previous route: its cubic is a pure function of its ports.
#[allow(clippy::too_many_arguments)]
pub fn route_wire_sticky_in_mode(
    mode: WireMode,
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
    previous: Option<&WireRoute>,
) -> WireRoute {
    if mode == WireMode::Bezier {
        return cubic_route(from, source_side, to, target_side);
    }

    let radius = style.corner_radius.max(16.0);
    let comfortable = orthogonal_candidates(
        from,
        source_side,
        to,
        target_side,
        obstacles,
        style,
        corridor_offset,
        radius,
        radius,
        radius * 2.0,
        false,
    );

    // The caller supplies cards inflated by 12 px. The narrow tier reserves
    // 6 px on each side plus half a cable spacing for the centreline. Thus a
    // raw gap of exactly spacing + 2 * 6 remains usable, with 8 px fillets.
    let narrow_center_clearance = NARROW_CLEARANCE + style.cable_spacing * 0.5;
    let narrow_obstacles: Vec<Obstacle> = obstacles
        .iter()
        .map(|rect| rect.inflate(-(CALLER_CARD_CLEARANCE - narrow_center_clearance)))
        .collect();
    let narrow = orthogonal_candidates(
        from,
        source_side,
        to,
        target_side,
        &narrow_obstacles,
        style,
        corridor_offset,
        NARROW_RADIUS,
        0.0,
        0.0,
        true,
    );

    // Preserve the full-clearance route unless the narrow tier opens a route
    // with fewer bends or one that is meaningfully shorter. Sticky selection
    // below is unchanged and still operates within the winning tier.
    let candidates = match (
        comfortable.iter().min_by(|left, right| compare_routes(left, right)),
        narrow.iter().min_by(|left, right| compare_routes(left, right)),
    ) {
        (None, _) => narrow,
        (_, None) => comfortable,
        (Some(full), Some(tight))
            if uses_narrow_channel(tight, obstacles, style.cable_spacing)
                && (tight.bends() < full.bends()
                    || (tight.bends() == full.bends()
                        && tight.length() < full.length() * ROUTE_SWITCH_RATIO)) =>
        {
            narrow
        }
        _ => comfortable,
    };
    let Some(best) = candidates.iter().min_by(|left, right| compare_routes(left, right)) else {
        return least_bad_orthogonal(
            from,
            source_side,
            to,
            target_side,
            &narrow_obstacles,
            style,
            corridor_offset,
        );
    };
    if let Some(previous) = previous {
        if let Some(sticky) = candidates
            .iter()
            .filter(|candidate| candidate.choice == previous.choice)
            .min_by(|left, right| compare_routes(left, right))
        {
            if best.choice != sticky.choice && best.length() >= sticky.length() * ROUTE_SWITCH_RATIO
            {
                return sticky.clone();
            }
        }
    }
    best.clone()
}

fn uses_narrow_channel(route: &WireRoute, obstacles: &[Obstacle], cable_spacing: f64) -> bool {
    let envelopes: Vec<_> = obstacles
        .iter()
        .map(|obstacle| obstacle.inflate(cable_spacing * 0.5))
        .collect();
    for left in 0..envelopes.len() {
        for right in left + 1..envelopes.len() {
            let overlap = Obstacle {
                min: Point::new(
                    envelopes[left].min.x.max(envelopes[right].min.x),
                    envelopes[left].min.y.max(envelopes[right].min.y),
                ),
                max: Point::new(
                    envelopes[left].max.x.min(envelopes[right].max.x),
                    envelopes[left].max.y.min(envelopes[right].max.y),
                ),
            };
            if overlap.min.x >= overlap.max.x || overlap.min.y >= overlap.max.y {
                continue;
            }
            if route
                .samples
                .windows(2)
                .any(|pair| !segment_clear_rect(pair[0], pair[1], overlap))
            {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn orthogonal_candidates(
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
    radius: f64,
    row_clearance: f64,
    channel_width: f64,
    allow_doglegs: bool,
) -> Vec<WireRoute> {
    let stub = style.port_stub.max(radius * 2.0).max(24.0);
    let source_stub = Point::new(from.x + source_side.sign() * stub, from.y);
    let target_stub = Point::new(to.x + target_side.sign() * stub, to.y);
    let source_owner = endpoint_obstacle(from, obstacles, source_side);
    let target_owner = endpoint_obstacle(to, obstacles, target_side);
    let mut candidates = Vec::with_capacity(5 + obstacles.len() * 2);

    if ((from.x - to.x).abs() < 1e-6 || (from.y - to.y).abs() < 1e-6)
        && route_samples_clear(&[from, to], obstacles, source_owner, target_owner)
    {
        candidates.push(build_route(
            from,
            to,
            RouteKind::Orthogonal {
                points: vec![from, to],
                radius,
            },
            vec![from, to],
            RouteChoice::Straight,
            f64::INFINITY,
        ));
    }

    if !segment_clear_except(from, source_stub, obstacles, source_owner)
        || !segment_clear_except(target_stub, to, obstacles, target_owner)
    {
        return candidates;
    }

    let guide_reserve = if radius > NARROW_RADIUS {
        radius + corridor_offset.abs()
    } else {
        corridor_offset.abs()
    };
    let guides: Vec<Obstacle> = obstacles.iter().map(|rect| rect.inflate(guide_reserve)).collect();
    if source_stub.x <= target_stub.x {
        if let Some(channel) = choose_forward_channel(
            source_stub,
            target_stub,
            &guides,
            corridor_offset,
            channel_width,
        ) {
            if let Some(candidate) = build_orthogonal_candidate(
                from,
                to,
                vec![
                    from,
                    source_stub,
                    Point::new(channel, source_stub.y),
                    Point::new(channel, target_stub.y),
                    target_stub,
                    to,
                ],
                radius,
                obstacles,
                source_owner,
                target_owner,
                RouteChoice::CorridorX,
                channel,
            ) {
                candidates.push(candidate);
            }
        }
    }

    // Boundary rows find both outside detours and lanes between stacked cards.
    // Boundary columns add the dogleg needed to clear a card before entering
    // such a row, or to leave the row before approaching the target.
    let columns = routing_columns(from, to, obstacles, row_clearance, corridor_offset);
    for row in routing_rows(from, to, obstacles, row_clearance, corridor_offset) {
        let choice = if row <= (from.y + to.y) * 0.5 {
            RouteChoice::Above
        } else {
            RouteChoice::Below
        };
        if let Some(candidate) = build_orthogonal_candidate(
            from,
            to,
            vec![
                from,
                source_stub,
                Point::new(source_stub.x, row),
                Point::new(target_stub.x, row),
                target_stub,
                to,
            ],
            radius,
            obstacles,
            source_owner,
            target_owner,
            choice,
            source_stub.x.min(target_stub.x),
        ) {
            candidates.push(candidate);
        }
        for column in columns.iter().filter(|_| allow_doglegs) {
            for points in [
                vec![
                    from,
                    source_stub,
                    Point::new(*column, from.y),
                    Point::new(*column, row),
                    Point::new(target_stub.x, row),
                    target_stub,
                    to,
                ],
                vec![
                    from,
                    source_stub,
                    Point::new(source_stub.x, row),
                    Point::new(*column, row),
                    Point::new(*column, to.y),
                    target_stub,
                    to,
                ],
            ] {
                if let Some(candidate) = build_orthogonal_candidate(
                    from,
                    to,
                    points,
                    radius,
                    obstacles,
                    source_owner,
                    target_owner,
                    choice,
                    *column,
                ) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates
}

fn least_bad_orthogonal(
    from: Point,
    source_side: PortSide,
    to: Point,
    target_side: PortSide,
    obstacles: &[Obstacle],
    style: RouteStyle,
    corridor_offset: f64,
) -> WireRoute {
    let stub = style.port_stub.max(NARROW_RADIUS * 2.0).max(24.0);
    let source_stub = Point::new(from.x + source_side.sign() * stub, from.y);
    let target_stub = Point::new(to.x + target_side.sign() * stub, to.y);
    let mut candidates = Vec::new();
    for row in routing_rows(
        from,
        to,
        obstacles,
        style.cable_spacing * 0.5,
        corridor_offset,
    ) {
        let choice = if row <= (from.y + to.y) * 0.5 {
            RouteChoice::Above
        } else {
            RouteChoice::Below
        };
        candidates.push(build_orthogonal_route(
            from,
            to,
            vec![
                from,
                source_stub,
                Point::new(source_stub.x, row),
                Point::new(target_stub.x, row),
                target_stub,
                to,
            ],
            NARROW_RADIUS,
            choice,
            source_stub.x.min(target_stub.x),
        ));
    }
    if candidates.is_empty() {
        candidates.push(build_orthogonal_route(
            from,
            to,
            vec![
                from,
                source_stub,
                Point::new(source_stub.x, to.y),
                target_stub,
                to,
            ],
            NARROW_RADIUS,
            RouteChoice::CorridorX,
            source_stub.x,
        ));
    }
    let source_owner = endpoint_obstacle(from, obstacles, source_side);
    let target_owner = endpoint_obstacle(to, obstacles, target_side);
    candidates
        .into_iter()
        .min_by(|left, right| {
            collision_count(left, obstacles, source_owner, target_owner)
                .cmp(&collision_count(right, obstacles, source_owner, target_owner))
                .then_with(|| compare_routes(left, right))
        })
        .unwrap()
}

fn build_orthogonal_route(
    from: Point,
    to: Point,
    points: Vec<Point>,
    radius: f64,
    choice: RouteChoice,
    tie_x: f64,
) -> WireRoute {
    let mut points = simplify(points);
    if points.len() < 2 {
        points.push(to);
    }
    let samples = rounded_samples(&points, radius);
    build_route(
        from,
        to,
        RouteKind::Orthogonal { points, radius },
        samples,
        choice,
        tie_x,
    )
}

fn collision_count(
    route: &WireRoute,
    obstacles: &[Obstacle],
    source_owner: Option<usize>,
    target_owner: Option<usize>,
) -> usize {
    let endpoint_exemption = ENDPOINT_STUB_DISTANCE + COLLISION_TOLERANCE;
    route
        .samples
        .windows(2)
        .enumerate()
        .map(|(segment, pair)| {
            obstacles
                .iter()
                .enumerate()
                .filter(|(obstacle, rect)| {
                    if source_owner == Some(*obstacle)
                        && route.cumulative[segment + 1] <= endpoint_exemption
                    {
                        return false;
                    }
                    if target_owner == Some(*obstacle)
                        && route.length - route.cumulative[segment] <= endpoint_exemption
                    {
                        return false;
                    }
                    !segment_clear_rect(pair[0], pair[1], **rect)
                })
                .count()
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn build_orthogonal_candidate(
    from: Point,
    to: Point,
    points: Vec<Point>,
    radius: f64,
    obstacles: &[Obstacle],
    source_owner: Option<usize>,
    target_owner: Option<usize>,
    choice: RouteChoice,
    tie_x: f64,
) -> Option<WireRoute> {
    let points = simplify(points);
    if !valid_orthogonal(&points, radius) {
        return None;
    }
    let samples = rounded_samples(&points, radius);
    route_samples_clear(&samples, obstacles, source_owner, target_owner).then(|| {
        build_route(
            from,
            to,
            RouteKind::Orthogonal { points, radius },
            samples,
            choice,
            tie_x,
        )
    })
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

fn routing_columns(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    clearance: f64,
    offset: f64,
) -> Vec<f64> {
    let mut columns = vec![from.x, to.x];
    for rect in obstacles {
        columns.push(rect.min.x - clearance + offset);
        columns.push(rect.max.x + clearance + offset);
    }
    columns.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    columns.dedup_by(|left, right| (*left - *right).abs() < 1e-6);
    columns
}

/// Find a clear vertical track between the stubs. Forbidden x intervals are
/// accumulated in one pass and merged after sorting. This is O(cards log
/// cards), with route validation remaining O(cards).
fn choose_forward_channel(
    from: Point,
    to: Point,
    obstacles: &[Obstacle],
    offset: f64,
    channel_width: f64,
) -> Option<f64> {
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
        if start >= cursor {
            gaps.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor <= high + 1e-6 {
        gaps.push((cursor, high));
    }
    let desired = (low + high) * 0.5 + offset;
    gaps.into_iter()
        .filter(|(a, b)| b - a + 1e-6 >= channel_width)
        .map(|(a, b)| {
            let low = a + channel_width * 0.5;
            let high = b - channel_width * 0.5;
            let channel = desired.clamp(low, high);
            ((channel / CHANNEL_CELL).round() * CHANNEL_CELL).clamp(low, high)
        })
        .min_by(|a, b| {
            (a - desired)
                .abs()
                .partial_cmp(&(b - desired).abs())
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.partial_cmp(b).unwrap_or(Ordering::Equal))
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
    if points.len() < 2
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
    left.bends()
        .cmp(&right.bends())
        .then_with(|| left.length().partial_cmp(&right.length()).unwrap_or(Ordering::Equal))
        .then_with(|| left.tie_x.partial_cmp(&right.tie_x).unwrap_or(Ordering::Equal))
        .then_with(|| route_choice_rank(left.choice).cmp(&route_choice_rank(right.choice)))
}

fn route_choice_rank(choice: RouteChoice) -> u8 {
    match choice {
        RouteChoice::Straight => 0,
        RouteChoice::CorridorX => 1,
        RouteChoice::Above => 2,
        RouteChoice::Below => 3,
    }
}

fn point_segment_distance(point: Point, from: Point, to: Point) -> f64 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f64::EPSILON {
        return point.distance(from);
    }
    let t = (((point.x - from.x) * dx + (point.y - from.y) * dy) / length_squared)
        .clamp(0.0, 1.0);
    point.distance(Point::new(from.x + dx * t, from.y + dy * t))
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
    fn routed_mode_is_orthogonal_when_clear() {
        let route = route(Point::new(0.0, 20.0), Point::new(240.0, 80.0), &[]);
        assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
    }

    #[test]
    fn first_grab_geometry_stays_orthogonal() {
        let obstacles = facing_endpoint_cards(400.0, 540.0);
        let route = route(Point::new(400.0, 220.0), Point::new(540.0, 180.0), &obstacles);
        assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
        assert_route_misses_cards(&route, &obstacles);
    }

    #[test]
    fn second_grab_geometry_stays_orthogonal() {
        let obstacles = facing_endpoint_cards(160.0, 305.0);
        let route = route(Point::new(160.0, 190.0), Point::new(305.0, 155.0), &obstacles);
        assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
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
            let route = route_wire_in_mode(
                WireMode::Bezier,
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
    fn dense_blocking_stubs_falls_back_to_orthogonal() {
        let obstacles = [
            Obstacle::from_xywh(20.0, -20.0, 60.0, 40.0),
            Obstacle::from_xywh(100.0, -100.0, 60.0, 200.0),
        ];
        let route = route(Point::new(0.0, 0.0), Point::new(220.0, 0.0), &obstacles);
        assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
    }

    #[test]
    fn screenshot_gap_fixture_is_orthogonal_and_avoids_prompt() {
        let raw_prompt = Obstacle::from_xywh(430.0, 650.0, 580.0, 300.0);
        let obstacles = [
            card(100.0, 970.0, 580.0, 250.0),
            raw_prompt.inflate(12.0),
            card(930.0, 150.0, 580.0, 480.0),
        ];
        let route = route(Point::new(680.0, 1020.0), Point::new(930.0, 245.0), &obstacles);
        assert!(matches!(
            route.kind,
            RouteKind::Orthogonal { radius: 8.0, .. }
        ));
        assert!(route_samples_clear(&route.samples, &[raw_prompt], None, None));
        assert!(route.samples.iter().any(|point| point.y >= 950.0 && point.y <= 970.0));
    }

    #[test]
    fn bezier_mode_always_returns_the_port_cubic() {
        let from = Point::new(0.0, 20.0);
        let to = Point::new(240.0, 80.0);
        let obstacles = [Obstacle::from_xywh(40.0, -100.0, 180.0, 300.0)];
        let first = route_wire_in_mode(
            WireMode::Bezier,
            from,
            PortSide::Right,
            to,
            PortSide::Left,
            &obstacles,
            RouteStyle::default(),
            48.0,
        );
        let second = route_wire_sticky_in_mode(
            WireMode::Bezier,
            from,
            PortSide::Right,
            to,
            PortSide::Left,
            &[],
            RouteStyle::default(),
            -48.0,
            Some(&first),
        );
        assert!(matches!(first.kind, RouteKind::Cubic { .. }));
        assert_eq!(first, second);
    }

    #[test]
    fn routed_mode_never_returns_a_cubic() {
        for (from, to, obstacles) in [
            (Point::new(0.0, 20.0), Point::new(240.0, 80.0), vec![]),
            (
                Point::new(0.0, 0.0),
                Point::new(220.0, 0.0),
                vec![
                    Obstacle::from_xywh(20.0, -20.0, 60.0, 40.0),
                    Obstacle::from_xywh(100.0, -100.0, 60.0, 200.0),
                ],
            ),
        ] {
            let route = route_wire_in_mode(
                WireMode::Routed,
                from,
                PortSide::Right,
                to,
                PortSide::Left,
                &obstacles,
                RouteStyle::default(),
                0.0,
            );
            assert!(matches!(route.kind, RouteKind::Orthogonal { .. }));
        }
    }

    fn route_through_stacked_gap(gap: f64) -> WireRoute {
        let y = gap * 0.5;
        let obstacles = [
            card(60.0, -100.0, 180.0, 100.0),
            card(60.0, gap, 180.0, 100.0),
        ];
        route(Point::new(0.0, y), Point::new(300.0, y), &obstacles)
    }

    #[test]
    fn twenty_pixel_gap_is_used_but_a_narrower_one_is_not() {
        let exact = route_through_stacked_gap(20.0);
        assert!(matches!(
            exact.kind,
            RouteKind::Orthogonal { radius: 8.0, .. }
        ));
        assert_eq!(exact.bends(), 0);
        assert!(exact.distance_to_point(Point::new(150.0, 10.0)) < 1e-6);

        let too_narrow = route_through_stacked_gap(19.0);
        assert!(matches!(
            too_narrow.kind,
            RouteKind::Orthogonal { radius: 16.0, .. }
        ));
        assert!(too_narrow.bends() > 0);
        assert!(too_narrow.distance_to_point(Point::new(150.0, 9.5)) > 1.0);
    }

    #[test]
    fn routing_is_deterministic() {
        let obstacles = [Obstacle::from_xywh(90.0, -10.0, 60.0, 80.0)];
        let one = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &obstacles);
        let two = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &obstacles);
        assert_eq!(one, two);
    }

    #[test]
    fn point_to_routed_polyline_distance_uses_nearest_segment() {
        let route = route(Point::new(0.0, 20.0), Point::new(240.0, 20.0), &[]);
        assert!((route.distance_to_point(Point::new(80.0, 25.5)) - 5.5).abs() < 0.01);
        assert!((route.distance_to_point(Point::new(-3.0, 24.0)) - 5.0).abs() < 0.01);
    }

    #[test]
    fn unrelated_card_motion_does_not_change_an_edges_obstacles_or_route() {
        let from = Point::new(0.0, 20.0);
        let to = Point::new(240.0, 20.0);
        let blocker = Obstacle::from_xywh(90.0, -20.0, 60.0, 80.0);
        let first = obstacles_in_corridor(
            from,
            to,
            &[blocker, Obstacle::from_xywh(900.0, 900.0, 100.0, 100.0)],
            64.0,
        );
        let second = obstacles_in_corridor(
            from,
            to,
            &[blocker, Obstacle::from_xywh(901.0, 900.0, 100.0, 100.0)],
            64.0,
        );
        assert_eq!(first, second);
        assert_eq!(route(from, to, &first), route(from, to, &second));
    }

    #[test]
    fn near_tie_keeps_the_previous_route_choice_across_recomputes() {
        let from = Point::new(0.0, 20.0);
        let to = Point::new(240.0, 20.0);
        let style = RouteStyle::default();
        let seed_obstacle = Obstacle::from_xywh(90.0, -25.0, 60.0, 80.0);
        let mut previous = route_wire(
            from,
            PortSide::Right,
            to,
            PortSide::Left,
            &[seed_obstacle],
            style,
            0.0,
        );
        assert_eq!(previous.choice, RouteChoice::Below);
        for step in 0..20 {
            let y = if step % 2 == 0 { -20.5 } else { -19.5 };
            let obstacle = Obstacle::from_xywh(90.0, y, 60.0, 80.0);
            previous = route_wire_sticky(
                from,
                PortSide::Right,
                to,
                PortSide::Left,
                &[obstacle],
                style,
                0.0,
                Some(&previous),
            );
            assert_eq!(previous.choice, RouteChoice::Below, "step {step}");
        }
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
            RouteChoice::Below,
            0.0,
        );
        let up = build_route(
            Point::new(0.0, 100.0),
            Point::new(100.0, 0.0),
            RouteKind::Orthogonal {
                points: vec![Point::new(0.0, 100.0), Point::new(100.0, 0.0)],
                radius: 0.0,
            },
            vec![Point::new(0.0, 100.0), Point::new(100.0, 0.0)],
            RouteChoice::Above,
            0.0,
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
            RouteChoice::Below,
            0.0,
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
            RouteChoice::Straight,
            f64::INFINITY,
        );
        let (_, tangent) = s_route.midpoint_tangent();
        assert!(tangent.x.abs() < 0.05 && tangent.y > 0.99, "{tangent:?}");
    }
}
