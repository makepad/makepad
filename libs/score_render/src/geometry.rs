use std::f64::consts::PI;

/// A distance in staff spaces.
pub type Sp = f64;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: Sp,
    pub y: Sp,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: Sp, y: Sp) -> Self {
        Self { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn normalized(self) -> Self {
        let length = self.length();
        if length <= f64::EPSILON {
            Self::new(1.0, 0.0)
        } else {
            self * (1.0 / length)
        }
    }

    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }
}

impl std::ops::Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f64> for Point {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub const EMPTY: Self = Self {
        min: Point {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        max: Point {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };

    pub fn new(min: Point, max: Point) -> Self {
        Self {
            min: Point::new(min.x.min(max.x), min.y.min(max.y)),
            max: Point::new(min.x.max(max.x), min.y.max(max.y)),
        }
    }

    pub fn from_xywh(x: Sp, y: Sp, width: Sp, height: Sp) -> Self {
        Self::new(Point::new(x, y), Point::new(x + width, y + height))
    }

    pub fn from_points(points: impl IntoIterator<Item = Point>) -> Self {
        let mut rect = Self::EMPTY;
        for point in points {
            rect.include(point);
        }
        rect
    }

    pub fn include(&mut self, point: Point) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
    }

    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self::new(
            Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        )
    }

    pub fn expanded(self, amount: Sp) -> Self {
        Self::new(
            Point::new(self.min.x - amount, self.min.y - amount),
            Point::new(self.max.x + amount, self.max.y + amount),
        )
    }

    pub fn intersects(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.max.x >= other.min.x
            && self.min.x <= other.max.x
            && self.max.y >= other.min.y
            && self.min.y <= other.max.y
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }

    pub fn width(self) -> Sp {
        (self.max.x - self.min.x).max(0.0)
    }

    pub fn height(self) -> Sp {
        (self.max.y - self.min.y).max(0.0)
    }

    pub fn center(self) -> Point {
        Point::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    pub fn is_empty(self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    pub fn is_finite(self) -> bool {
        !self.is_empty() && self.min.is_finite() && self.max.is_finite()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Point,
    pub scale: f64,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Point::ZERO,
        scale: 1.0,
    };

    pub fn point(self, point: Point) -> Point {
        self.translation + point * self.scale
    }

    pub fn rect(self, rect: Rect) -> Rect {
        Rect::new(self.point(rect.min), self.point(rect.max))
    }

    pub fn inverse_rect(self, rect: Rect) -> Rect {
        let inverse = 1.0 / self.scale;
        Rect::new(
            (rect.min - self.translation) * inverse,
            (rect.max - self.translation) * inverse,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    pub p0: Point,
    pub p1: Point,
    pub p2: Point,
    pub p3: Point,
}

impl Cubic {
    pub fn point(self, t: f64) -> Point {
        let mt = 1.0 - t;
        self.p0 * (mt * mt * mt)
            + self.p1 * (3.0 * mt * mt * t)
            + self.p2 * (3.0 * mt * t * t)
            + self.p3 * (t * t * t)
    }

    pub fn derivative(self, t: f64) -> Point {
        let mt = 1.0 - t;
        (self.p1 - self.p0) * (3.0 * mt * mt)
            + (self.p2 - self.p1) * (6.0 * mt * t)
            + (self.p3 - self.p2) * (3.0 * t * t)
    }

    pub fn split(self) -> (Self, Self) {
        let p01 = self.p0.lerp(self.p1, 0.5);
        let p12 = self.p1.lerp(self.p2, 0.5);
        let p23 = self.p2.lerp(self.p3, 0.5);
        let p012 = p01.lerp(p12, 0.5);
        let p123 = p12.lerp(p23, 0.5);
        let mid = p012.lerp(p123, 0.5);
        (
            Self {
                p0: self.p0,
                p1: p01,
                p2: p012,
                p3: mid,
            },
            Self {
                p0: mid,
                p1: p123,
                p2: p23,
                p3: self.p3,
            },
        )
    }

    pub fn control_bounds(self) -> Rect {
        Rect::from_points([self.p0, self.p1, self.p2, self.p3])
    }

    pub fn is_finite(self) -> bool {
        self.p0.is_finite()
            && self.p1.is_finite()
            && self.p2.is_finite()
            && self.p3.is_finite()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    pub start: Point,
    pub end: Point,
    pub thickness: Sp,
}

impl Beam {
    /// Exact parallelogram with vertical end cuts, in clockwise page order.
    pub fn vertices(self) -> [Point; 4] {
        let half = self.thickness * 0.5;
        [
            Point::new(self.start.x, self.start.y - half),
            Point::new(self.end.x, self.end.y - half),
            Point::new(self.end.x, self.end.y + half),
            Point::new(self.start.x, self.start.y + half),
        ]
    }

    pub fn bounds(self) -> Rect {
        Rect::from_points(self.vertices())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ribbon {
    pub curve: Cubic,
    pub endpoint_thickness: Sp,
    pub midpoint_thickness: Sp,
}

impl Ribbon {
    pub fn thickness(self, t: f64) -> f64 {
        let swell = (PI * t).sin().max(0.0).powf(0.8);
        self.endpoint_thickness
            + (self.midpoint_thickness - self.endpoint_thickness) * swell
    }

    pub fn bounds(self) -> Rect {
        self.curve
            .control_bounds()
            .expanded(self.midpoint_thickness.max(self.endpoint_thickness) * 0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonVertex {
    pub position: Point,
    /// -1 at the upper edge and +1 at the lower edge; used for analytic AA.
    pub signed_edge: f32,
    pub t: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RibbonMesh {
    pub vertices: Vec<RibbonVertex>,
    pub indices: Vec<u32>,
    pub max_error_px: f64,
}

/// Deterministically tessellates a variable-width cubic ribbon.
///
/// Both centerline flatness and thickness interpolation are bounded by
/// `max_error_px` after projection. Recursion is midpoint-only and ordered
/// left-to-right, making identical curve/zoom inputs byte-stable.
pub fn tessellate_ribbon(ribbon: Ribbon, px_per_sp: f64, max_error_px: f64) -> RibbonMesh {
    assert!(px_per_sp.is_finite() && px_per_sp > 0.0);
    assert!(max_error_px.is_finite() && max_error_px > 0.0);
    let mut samples = vec![(0.0, ribbon.curve.p0)];
    subdivide_ribbon(
        ribbon,
        ribbon.curve,
        0.0,
        1.0,
        px_per_sp,
        max_error_px,
        0,
        &mut samples,
    );

    let mut vertices = Vec::with_capacity(samples.len() * 2);
    for (t, center) in samples.iter().copied() {
        let tangent = ribbon.curve.derivative(t).normalized();
        let normal = tangent.perp();
        let half = ribbon.thickness(t) * 0.5;
        vertices.push(RibbonVertex {
            position: center - normal * half,
            signed_edge: -1.0,
            t: t as f32,
        });
        vertices.push(RibbonVertex {
            position: center + normal * half,
            signed_edge: 1.0,
            t: t as f32,
        });
    }

    let mut indices = Vec::with_capacity(samples.len().saturating_sub(1) * 6);
    for segment in 0..samples.len().saturating_sub(1) {
        let i = (segment * 2) as u32;
        indices.extend_from_slice(&[i, i + 1, i + 2, i + 1, i + 3, i + 2]);
    }
    RibbonMesh {
        vertices,
        indices,
        max_error_px,
    }
}

#[allow(clippy::too_many_arguments)]
fn subdivide_ribbon(
    ribbon: Ribbon,
    curve: Cubic,
    t0: f64,
    t1: f64,
    px_per_sp: f64,
    tolerance: f64,
    depth: u8,
    output: &mut Vec<(f64, Point)>,
) {
    let chord_mid = curve.p0.lerp(curve.p3, 0.5);
    let center_error = (curve.point(0.5) - chord_mid).length();
    let tm = (t0 + t1) * 0.5;
    let width_error = (ribbon.thickness(tm)
        - (ribbon.thickness(t0) + ribbon.thickness(t1)) * 0.5)
        .abs()
        * 0.5;
    let control_error = distance_to_line(curve.p1, curve.p0, curve.p3)
        .max(distance_to_line(curve.p2, curve.p0, curve.p3));
    let projected_error = center_error
        .max(control_error * 0.75)
        .max(width_error)
        * px_per_sp;

    if projected_error <= tolerance || depth >= 20 {
        output.push((t1, curve.p3));
        return;
    }
    let (left, right) = curve.split();
    subdivide_ribbon(
        ribbon,
        left,
        t0,
        tm,
        px_per_sp,
        tolerance,
        depth + 1,
        output,
    );
    subdivide_ribbon(
        ribbon,
        right,
        tm,
        t1,
        px_per_sp,
        tolerance,
        depth + 1,
        output,
    );
}

fn distance_to_line(point: Point, start: Point, end: Point) -> f64 {
    let line = end - start;
    let length = line.length();
    if length <= f64::EPSILON {
        return (point - start).length();
    }
    ((point.x - start.x) * line.y - (point.y - start.y) * line.x).abs() / length
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedRule {
    pub rect_px: Rect,
    pub snapped: bool,
    /// Multiplier for the rule's ink alpha; see [`ink_floor`]. It is `1.0`
    /// whenever the drawn rule is at least as thin as the engraved one.
    pub ink_alpha: f32,
}

/// Physical pixels per unit of the transform's output space.
///
/// Transforms hand back *logical* points; on a 2x display one logical point is
/// two device pixels. The hairline rule has to be expressed on the device grid
/// or a 0.13 sp staff line is clamped to a whole logical point — two physical
/// pixels — and a zoomed-out page turns into a black smear.
pub const LOGICAL_DEVICE_SCALE: f64 = 1.0;

fn sane_device_scale(device_scale: f64) -> f64 {
    if device_scale.is_finite() && device_scale > 0.0 {
        device_scale
    } else {
        LOGICAL_DEVICE_SCALE
    }
}

/// The thinnest mark a raster target can make: one physical pixel.
pub const MIN_INK_DEVICE_PX: f64 = 1.0;

/// A stroke too thin for the pixel grid, and the alpha that keeps its weight.
///
/// Below one physical pixel a mark cannot get thinner, only lighter. Drawing a
/// 0.23 px staff line as a 1 px line at full strength lays down four times the
/// ink the engraving asked for, and a page full of such lines — five per staff,
/// a stem per note, a beam per pair — is why a zoomed-out score turns into a
/// black mass. Draw the line at the floor so it never disappears, and give it
/// the alpha its true width would have covered, and the page keeps exactly its
/// engraved weight at every scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkFloor {
    /// The width to draw, in the same units as the requested width.
    pub width: f64,
    /// Factor to apply to the ink's alpha. `1.0` when nothing was widened.
    pub alpha: f32,
}

/// Applies the one-device-pixel floor to a projected stroke width.
///
/// `width` is in output (logical) units; `device_scale` is physical pixels per
/// output unit.
pub fn ink_floor(width: f64, device_scale: f64) -> InkFloor {
    let floor = MIN_INK_DEVICE_PX / sane_device_scale(device_scale);
    if !width.is_finite() || width <= 0.0 {
        return InkFloor {
            width: floor,
            alpha: 0.0,
        };
    }
    if width >= floor {
        return InkFloor { width, alpha: 1.0 };
    }
    InkFloor {
        width: floor,
        alpha: (width / floor) as f32,
    }
}

/// The alpha that keeps `exact` units of ink after `drawn` units were laid
/// down. Widening is the only thing this compensates; a rule drawn thinner
/// than engraved is left alone, because nothing was added.
fn ink_alpha_for(exact: f64, drawn: f64) -> f32 {
    if !exact.is_finite() || !drawn.is_finite() || drawn <= 0.0 {
        return 1.0;
    }
    ((exact / drawn) as f32).clamp(0.0, 1.0)
}

/// Projects an axis-aligned rule and applies the settled-screen hairline rule.
/// Print/export callers pass `settled = false`, preserving exact engraving.
pub fn project_rule(rect_sp: Rect, transform: Transform, settled: bool) -> ProjectedRule {
    project_rule_with_snap(rect_sp, transform, if settled { 1.0 } else { 0.0 })
}

/// `snap_progress` is zero during pinch/animated zoom and eases to one after
/// zoom settles, so the display-only pixel grid cannot visibly pop.
pub fn project_rule_with_snap(
    rect_sp: Rect,
    transform: Transform,
    snap_progress: f32,
) -> ProjectedRule {
    project_rule_on_grid(rect_sp, transform, snap_progress, LOGICAL_DEVICE_SCALE)
}

/// The hairline rule, snapped to the *device* pixel grid.
///
/// `device_scale` is physical pixels per output unit (`Cx::current_dpi_factor`).
/// A hairline is never allowed to become thinner than one physical pixel, and
/// never thicker than that either when the engraved thickness asks for less.
pub fn project_rule_on_grid(
    rect_sp: Rect,
    transform: Transform,
    snap_progress: f32,
    device_scale: f64,
) -> ProjectedRule {
    let exact = transform.rect(rect_sp);
    let scale = sane_device_scale(device_scale);
    let horizontal = rect_sp.width() >= rect_sp.height();
    let thickness = if horizontal {
        exact.height()
    } else {
        exact.width()
    };
    let progress = snap_progress.clamp(0.0, 1.0) as f64;
    if progress <= 0.0 || thickness * scale >= 1.75 {
        return ProjectedRule {
            rect_px: exact,
            snapped: false,
            ink_alpha: 1.0,
        };
    }

    let display_thickness = (thickness * scale).round().max(1.0) / scale;
    let mut target = exact;
    if horizontal {
        let center = exact.center().y;
        let edge = ((center - display_thickness * 0.5) * scale).round() / scale;
        target.min.y = edge;
        target.max.y = edge + display_thickness;
    } else {
        let center = exact.center().x;
        let edge = ((center - display_thickness * 0.5) * scale).round() / scale;
        target.min.x = edge;
        target.max.x = edge + display_thickness;
    }
    let rect_px = lerp_rect(exact, target, progress);
    let drawn = if horizontal {
        rect_px.height()
    } else {
        rect_px.width()
    };
    ProjectedRule {
        rect_px,
        snapped: progress >= 1.0,
        ink_alpha: ink_alpha_for(thickness, drawn),
    }
}

/// Projects a staff's parallel rules with one shared phase when their screen
/// separation is within 0.05 px of an integer. Otherwise their true centers
/// are retained (only the display-copy thickness is clamped), preventing five
/// independently rounded lines from acquiring visibly uneven gaps.
pub fn project_staff_rules(
    rules_sp: &[Rect],
    transform: Transform,
    settled: bool,
) -> Vec<ProjectedRule> {
    project_staff_rules_with_snap(rules_sp, transform, if settled { 1.0 } else { 0.0 })
}

pub fn project_staff_rules_with_snap(
    rules_sp: &[Rect],
    transform: Transform,
    snap_progress: f32,
) -> Vec<ProjectedRule> {
    project_staff_rules_on_grid(rules_sp, transform, snap_progress, LOGICAL_DEVICE_SCALE)
}

/// Staff-rule projection on the device pixel grid; see [`project_rule_on_grid`].
pub fn project_staff_rules_on_grid(
    rules_sp: &[Rect],
    transform: Transform,
    snap_progress: f32,
    device_scale: f64,
) -> Vec<ProjectedRule> {
    if rules_sp.is_empty() {
        return Vec::new();
    }
    let scale = sane_device_scale(device_scale);
    let true_rects: Vec<_> = rules_sp
        .iter()
        .map(|rect| transform.rect(*rect))
        .collect();
    let progress = snap_progress.clamp(0.0, 1.0) as f64;
    let coherent_phase = progress > 0.0
        && true_rects.windows(2).all(|pair| {
            let a = pair[0].center().y * scale;
            let b = pair[1].center().y * scale;
            let separation = (b - a).abs();
            (separation - separation.round()).abs() <= 0.05
        });
    let phase_delta = if coherent_phase {
        let snapped = project_rule_on_grid(rules_sp[0], transform, 1.0, scale).rect_px;
        snapped.center().y - true_rects[0].center().y
    } else {
        0.0
    };
    true_rects
        .iter()
        .map(|rect| {
            let thickness = rect.height() * scale;
            let mut target = *rect;
            if progress > 0.0 && thickness < 1.75 {
                let display_thickness = thickness.round().max(1.0) / scale;
                let center = rect.center().y + phase_delta;
                target.min.y = center - display_thickness * 0.5;
                target.max.y = center + display_thickness * 0.5;
            }
            let rect_px = lerp_rect(*rect, target, progress);
            ProjectedRule {
                rect_px,
                snapped: coherent_phase && progress >= 1.0,
                ink_alpha: ink_alpha_for(rect.height(), rect_px.height()),
            }
        })
        .collect()
}

fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    Rect {
        min: from.min.lerp(to.min, t),
        max: from.max.lerp(to.max, t),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettledZoomTransition {
    pub started_at_s: f64,
}

impl SettledZoomTransition {
    pub const DURATION_S: f64 = 0.090;

    pub fn snap_progress(self, now_s: f64) -> f32 {
        let t = ((now_s - self.started_at_s) / Self::DURATION_S).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t)) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hairline_minimum_is_one_device_pixel_not_one_logical_point() {
        // 0.13 sp staff line at 3.7 logical px/sp on a 2x display.
        let rule = Rect::from_xywh(0.0, 10.0, 40.0, 0.13);
        let transform = Transform {
            translation: Point::new(0.0, 0.0),
            scale: 3.7,
        };
        let logical = project_rule_on_grid(rule, transform, 1.0, 1.0);
        let retina = project_rule_on_grid(rule, transform, 1.0, 2.0);
        assert_eq!(logical.rect_px.height(), 1.0);
        assert_eq!(retina.rect_px.height(), 0.5);
        // Zoomed far out the hairline still never disappears, and never grows.
        let tiny = Transform {
            translation: Point::new(0.0, 0.0),
            scale: 0.9,
        };
        let far = project_rule_on_grid(rule, tiny, 1.0, 2.0);
        assert_eq!(far.rect_px.height(), 0.5);
        // Edges land on the device grid so the line stays crisp.
        assert_eq!((retina.rect_px.min.y * 2.0).fract(), 0.0);
    }

    #[test]
    fn the_hairline_floor_trades_width_for_alpha() {
        // Above the floor nothing is touched.
        let wide = ink_floor(3.0, 2.0);
        assert_eq!(wide.width, 3.0);
        assert_eq!(wide.alpha, 1.0);
        // Exactly one physical pixel is the floor on a 2x display.
        let exact = ink_floor(0.5, 2.0);
        assert_eq!(exact.width, 0.5);
        assert_eq!(exact.alpha, 1.0);
        // Below it the stroke is widened to the floor and darkened by exactly
        // the shortfall, so the ink it lays down is unchanged.
        for width in [0.4, 0.25, 0.13, 0.02] {
            let ink = ink_floor(width, 2.0);
            assert_eq!(ink.width, 0.5, "the floor is one physical pixel");
            assert!(
                (ink.width * ink.alpha as f64 - width).abs() < 1e-6,
                "{width} sp of ink became {}",
                ink.width * ink.alpha as f64
            );
        }
        // The floor follows the display, not the logical grid.
        assert_eq!(ink_floor(0.3, 1.0).width, 1.0);
        assert_eq!(ink_floor(0.1, 4.0).width, 0.25);
        assert_eq!(ink_floor(0.3, 4.0).width, 0.3);
    }

    /// A staff line at 0.13 sp is under a physical pixel from about 4x zoom
    /// downwards. Snapping it to the grid at full strength is what turned a
    /// zoomed-out page into a black mass; snapped weight has to stay engraved
    /// weight at every scale.
    #[test]
    fn snapped_rules_keep_their_engraved_weight() {
        let engraved = 0.13;
        let line = Rect::from_xywh(0.0, 10.0, 40.0, engraved);
        for scale in [7.0, 3.43, 1.71, 0.86, 0.41] {
            let transform = Transform {
                translation: Point::new(0.0, 0.0),
                scale,
            };
            let rule = project_rule_on_grid(line, transform, 1.0, 2.0);
            let drawn = rule.rect_px.height();
            assert!(
                drawn * 2.0 >= MIN_INK_DEVICE_PX - 1e-9,
                "at {scale} px/sp the line thinned to {drawn} and would drop out"
            );
            let ink = drawn * rule.ink_alpha as f64;
            assert!(
                (ink - engraved * scale).abs() < 1e-6,
                "at {scale} px/sp the line lays down {ink} instead of {}",
                engraved * scale
            );
        }
    }

    /// The same must hold for the five lines that share one snap phase.
    #[test]
    fn phase_locked_staff_rules_keep_their_engraved_weight() {
        let engraved = 0.13;
        let rules: Vec<_> = (0..5)
            .map(|line| Rect::from_xywh(0.0, 10.0 + line as f64, 40.0, engraved))
            .collect();
        for scale in [3.43, 1.71, 0.86] {
            let transform = Transform {
                translation: Point::new(0.0, 0.0),
                scale,
            };
            for rule in project_staff_rules_on_grid(&rules, transform, 1.0, 2.0) {
                let ink = rule.rect_px.height() * rule.ink_alpha as f64;
                assert!(
                    (ink - engraved * scale).abs() < 1e-6,
                    "at {scale} px/sp a staff line lays down {ink} instead of {}",
                    engraved * scale
                );
            }
        }
    }

    /// A rule already wide enough is never dimmed.
    #[test]
    fn rules_above_the_floor_keep_full_ink() {
        let rule = project_rule_on_grid(
            Rect::from_xywh(0.0, 10.0, 40.0, 0.5),
            Transform {
                translation: Point::ZERO,
                scale: 8.0,
            },
            1.0,
            2.0,
        );
        assert_eq!(rule.ink_alpha, 1.0);
        assert_eq!(rule.rect_px.height(), 4.0);
    }

    #[test]
    fn staff_rules_share_one_device_phase_and_keep_even_gaps() {
        let rules: Vec<_> = (0..5)
            .map(|line| Rect::from_xywh(0.0, 10.0 + line as f64, 40.0, 0.13))
            .collect();
        let transform = Transform {
            translation: Point::new(0.0, 0.0),
            scale: 3.5,
        };
        let projected = project_staff_rules_on_grid(&rules, transform, 1.0, 2.0);
        assert_eq!(projected.len(), 5);
        for rule in &projected {
            assert_eq!(rule.rect_px.height(), 0.5);
        }
        let gaps: Vec<_> = projected
            .windows(2)
            .map(|pair| pair[1].rect_px.center().y - pair[0].rect_px.center().y)
            .collect();
        for gap in gaps.windows(2) {
            assert!((gap[0] - gap[1]).abs() < 1e-9, "uneven staff gaps: {gaps:?}");
        }
    }

    #[test]
    fn beam_is_exact_parallelogram_with_vertical_cuts() {
        let beam = Beam {
            start: Point::new(1.0, 2.0),
            end: Point::new(5.0, 3.0),
            thickness: 0.5,
        };
        assert_eq!(
            beam.vertices(),
            [
                Point::new(1.0, 1.75),
                Point::new(5.0, 2.75),
                Point::new(5.0, 3.25),
                Point::new(1.0, 2.25),
            ]
        );
    }

    #[test]
    fn ribbon_subdivision_is_deterministic_and_tightens_with_zoom() {
        let ribbon = Ribbon {
            curve: Cubic {
                p0: Point::new(0.0, 0.0),
                p1: Point::new(2.0, -3.0),
                p2: Point::new(7.0, -3.0),
                p3: Point::new(9.0, 0.0),
            },
            endpoint_thickness: 0.10,
            midpoint_thickness: 0.22,
        };
        let a = tessellate_ribbon(ribbon, 8.0, 0.2);
        let b = tessellate_ribbon(ribbon, 8.0, 0.2);
        let zoomed = tessellate_ribbon(ribbon, 64.0, 0.2);
        assert_eq!(a, b);
        assert!(a.vertices.len() >= 8);
        assert!(zoomed.vertices.len() > a.vertices.len());
        assert_eq!(a.indices.len(), (a.vertices.len() / 2 - 1) * 6);
    }

    #[test]
    fn settled_hairline_has_integer_edges() {
        let projected = project_rule(
            Rect::from_xywh(0.0, 1.13, 10.0, 0.13),
            Transform {
                translation: Point::new(0.0, 0.25),
                scale: 4.0,
            },
            true,
        );
        assert!(projected.snapped);
        assert_eq!(projected.rect_px.height(), 1.0);
        assert_eq!(projected.rect_px.min.y.fract(), 0.0);
        assert_eq!(projected.rect_px.max.y.fract(), 0.0);
    }

    #[test]
    fn staff_rules_share_phase_or_keep_true_spacing() {
        let rules: Vec<_> = (0..5)
            .map(|line| Rect::from_xywh(0.0, line as f64, 20.0, 0.13))
            .collect();
        let coherent = project_staff_rules(
            &rules,
            Transform {
                translation: Point::new(0.0, 0.23),
                scale: 4.0,
            },
            true,
        );
        assert!(coherent.iter().all(|rule| rule.snapped));
        for pair in coherent.windows(2) {
            let gap = pair[1].rect_px.center().y - pair[0].rect_px.center().y;
            assert!((gap - 4.0).abs() < 1e-12);
        }

        let continuous = project_staff_rules(
            &rules,
            Transform {
                translation: Point::ZERO,
                scale: 3.37,
            },
            true,
        );
        assert!(continuous.iter().all(|rule| !rule.snapped));
        let gap = continuous[1].rect_px.center().y - continuous[0].rect_px.center().y;
        assert!((gap - 3.37).abs() < 1e-12);
    }

    #[test]
    fn zoom_grid_settles_with_a_continuous_ease() {
        let rule = Rect::from_xywh(0.0, 1.13, 10.0, 0.13);
        let transform = Transform {
            translation: Point::new(0.0, 0.25),
            scale: 4.0,
        };
        let exact = project_rule_with_snap(rule, transform, 0.0).rect_px;
        let halfway = project_rule_with_snap(rule, transform, 0.5).rect_px;
        let snapped = project_rule_with_snap(rule, transform, 1.0).rect_px;
        assert_eq!(halfway.min.y, (exact.min.y + snapped.min.y) * 0.5);
        let transition = SettledZoomTransition { started_at_s: 1.0 };
        assert_eq!(transition.snap_progress(1.0), 0.0);
        assert_eq!(transition.snap_progress(1.10), 1.0);
    }
}
