//! Slur and tie curve fitting.
//!
//! A slur is not a stroked centerline — constant thickness looks mechanical
//! and exposed caps betray attachment errors. The output here is a cubic
//! Bezier *centerline* plus a variable thickness profile; the renderer
//! sweeps the ribbon `B(u) +/- 0.5 * t(u) * normal(B'(u))`.
//!
//! Fitting is candidate scoring, not optimization: a small grid of
//! control-arm fractions, height multipliers and endpoint offsets generates
//! a few dozen cubics; each is scored against the supplied obstacle shapes
//! and aesthetic targets; the cheapest wins deterministically. The score is
//! built so that collision area dominates every aesthetic term by orders of
//! magnitude — the essential, tested property is categorical: a legal curve
//! always beats a colliding pretty one.
//!
//! The kernel does not render and does not know fonts: obstacles arrive as
//! plain rectangles with a weight class, staff lines as y coordinates.

use crate::sp::{Sp, SpPoint, SpRect};
use crate::style::CurveStyle;

/// What kind of curve is being fitted (ties are flatter than slurs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CurveKind {
    /// A phrase/legato slur.
    Slur,
    /// A tie between two noteheads of the same pitch.
    Tie,
}

/// Which side of the chord the curve arches toward. Y grows downward, so
/// `Above` arches toward smaller y.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CurveSide {
    /// Arch upward (smaller y).
    Above,
    /// Arch downward (larger y).
    Below,
}

/// Collision weight class of an obstacle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObstacleClass {
    /// Noteheads, stems, accidentals: the heaviest class.
    NoteCore,
    /// Articulations, fingerings, text.
    Marking,
    /// Staff lines and other light ink.
    Line,
}

/// One obstacle the curve should clear.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CurveObstacle {
    /// The obstacle's bounding shape.
    pub rect: SpRect,
    /// Its collision weight class.
    pub class: ObstacleClass,
}

/// The fitting request.
#[derive(Clone, PartialEq, Debug)]
pub struct CurveSpec {
    /// Requested start attachment point.
    pub start: SpPoint,
    /// Requested end attachment point.
    pub end: SpPoint,
    /// Which side to arch toward.
    pub side: CurveSide,
    /// Slur or tie (selects the height formula).
    pub kind: CurveKind,
    /// Y coordinates of nearby staff lines, for the line-hugging penalty.
    /// May be empty.
    pub staff_lines: Vec<Sp>,
}

/// The parabolic thickness profile of the ribbon:
/// `t(u) = end + (mid - end) * 4u(1-u)`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ThicknessProfile {
    /// Thickness at both endpoints.
    pub end: Sp,
    /// Thickness at the midpoint.
    pub mid: Sp,
}

impl ThicknessProfile {
    /// Thickness at parameter `u` in `[0, 1]`.
    pub fn at(&self, u: f64) -> Sp {
        self.end + (self.mid - self.end) * (4.0 * u * (1.0 - u))
    }
}

/// Score breakdown of one candidate (all terms already weighted).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CurveScore {
    /// Weighted approximate area of ribbon/obstacle overlap.
    pub collision_area: f64,
    /// Weighted squared penetration into the obstacle clearance zones.
    pub penetration: f64,
    /// Weighted squared endpoint motion away from the requested points.
    pub end_motion: f64,
    /// Weighted squared deviation of apex height from the preferred height.
    pub height_dev: f64,
    /// Weighted squared deviation of the arm fraction from its preferred
    /// value.
    pub arm_dev: f64,
    /// Weighted squared excess of the endpoint tangents beyond the limit.
    pub tangent: f64,
    /// Weighted staff-line hugging measure.
    pub line_nearness: f64,
    /// Weighted squared endpoint-offset asymmetry.
    pub asymmetry: f64,
    /// Sum of all terms — the quantity minimized.
    pub total: f64,
    /// Unweighted collision area, for categorical assertions.
    pub raw_collision_area: f64,
}

/// One scored candidate.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CurveCandidate {
    /// Cubic control points `[P0, C0, C1, P1]` of the centerline.
    pub points: [SpPoint; 4],
    /// Control-arm fraction used.
    pub arm: f64,
    /// Measured apex height above the (offset) chord.
    pub apex_height: Sp,
    /// Endpoint offsets applied along the placement normal.
    pub offsets: (Sp, Sp),
    /// The score breakdown.
    pub score: CurveScore,
}

/// The chosen curve.
#[derive(Clone, PartialEq, Debug)]
pub struct CurveFit {
    /// Cubic control points `[P0, C0, C1, P1]` of the centerline.
    pub points: [SpPoint; 4],
    /// Thickness profile for the ribbon sweep.
    pub thickness: ThicknessProfile,
    /// The preferred height the search aimed for.
    pub preferred_height: Sp,
    /// Score breakdown of the winner.
    pub score: CurveScore,
}

/// Evaluate a cubic Bezier at `u`.
pub fn eval_cubic(p: &[SpPoint; 4], u: f64) -> SpPoint {
    let v = 1.0 - u;
    p[0].scale(v * v * v)
        .add(p[1].scale(3.0 * v * v * u))
        .add(p[2].scale(3.0 * v * u * u))
        .add(p[3].scale(u * u * u))
}

/// Derivative of a cubic Bezier at `u`.
pub fn cubic_derivative(p: &[SpPoint; 4], u: f64) -> SpPoint {
    let v = 1.0 - u;
    p[1].sub(p[0]).scale(3.0 * v * v)
        .add(p[2].sub(p[1]).scale(6.0 * v * u))
        .add(p[3].sub(p[2]).scale(3.0 * u * u))
}

/// The preferred arch height for a curve of the given kind spanning
/// `length`: `clamp(base + slope * L, min, max)`.
pub fn preferred_height(kind: CurveKind, length: Sp, style: &CurveStyle) -> Sp {
    match kind {
        CurveKind::Slur => Sp(style.slur_height_base + style.slur_height_per_len * length.0)
            .clamp(style.slur_height_min, style.slur_height_max),
        CurveKind::Tie => Sp(style.tie_height_base + style.tie_height_per_len * length.0)
            .clamp(style.tie_height_min, style.tie_height_max),
    }
}

fn obstacle_weight(class: ObstacleClass, style: &CurveStyle) -> f64 {
    match class {
        ObstacleClass::NoteCore => style.obstacle_weight_note,
        ObstacleClass::Marking => style.obstacle_weight_marking,
        ObstacleClass::Line => style.obstacle_weight_line,
    }
}

/// Generate and score every candidate for the spec, in deterministic
/// generation order (offsets outer, then arm, then height multiplier).
/// Candidates that backtrack along the chord (self-intersection risk) are
/// dropped. Empty result only for a degenerate zero-length chord.
pub fn score_candidates(
    spec: &CurveSpec,
    obstacles: &[CurveObstacle],
    style: &CurveStyle,
) -> Vec<CurveCandidate> {
    let chord = spec.end.sub(spec.start);
    let len = spec.start.distance(spec.end).0;
    if len <= 1e-9 {
        return Vec::new();
    }
    let ux = chord.x.0 / len;
    let uy = chord.y.0 / len;
    // Normal pointing to the requested side (y grows downward).
    let (mut nx, mut ny) = (-uy, ux);
    let wants_up = spec.side == CurveSide::Above;
    if (ny < 0.0) != wants_up {
        nx = -nx;
        ny = -ny;
    }
    let h0 = preferred_height(spec.kind, Sp(len), style);
    let mut out = Vec::new();
    for &o0 in &style.end_offsets {
        for &o1 in &style.end_offsets {
            for &arm in &style.arm_fractions {
                for &mult in &style.height_multipliers {
                    let q0 = SpPoint::xy(spec.start.x.0 + nx * o0.0, spec.start.y.0 + ny * o0.0);
                    let q1 = SpPoint::xy(spec.end.x.0 + nx * o1.0, spec.end.y.0 + ny * o1.0);
                    // For a symmetric cubic the apex offset is 3/4 of the
                    // control height.
                    let hc = h0.0 * mult / 0.75;
                    let c0 = SpPoint::xy(
                        q0.x.0 + ux * arm * len + nx * hc,
                        q0.y.0 + uy * arm * len + ny * hc,
                    );
                    let c1 = SpPoint::xy(
                        q1.x.0 - ux * arm * len + nx * hc,
                        q1.y.0 - uy * arm * len + ny * hc,
                    );
                    let points = [q0, c0, c1, q1];
                    if let Some(cand) =
                        score_one(spec, obstacles, style, &points, arm, (o0, o1), h0, (ux, uy), (nx, ny))
                    {
                        out.push(cand);
                    }
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn score_one(
    spec: &CurveSpec,
    obstacles: &[CurveObstacle],
    style: &CurveStyle,
    points: &[SpPoint; 4],
    arm: f64,
    offsets: (Sp, Sp),
    h0: Sp,
    chord_unit: (f64, f64),
    normal: (f64, f64),
) -> Option<CurveCandidate> {
    let samples = style.samples.max(5);
    let thickness = ThicknessProfile { end: style.thickness_end, mid: style.thickness_mid };
    let mut pts = Vec::with_capacity(samples);
    let mut apex = 0.0f64;
    for k in 0..samples {
        let u = k as f64 / (samples - 1) as f64;
        // Reject candidates that backtrack along the chord: the ribbon
        // would self-overlap.
        let d = cubic_derivative(points, u);
        if d.x.0 * chord_unit.0 + d.y.0 * chord_unit.1 <= 0.0 {
            return None;
        }
        let p = eval_cubic(points, u);
        // Height above the offset chord, measured along the normal.
        let rel = p.sub(points[0]);
        let h = rel.x.0 * normal.0 + rel.y.0 * normal.1;
        apex = apex.max(h);
        pts.push((u, p));
    }

    let mut raw_area = 0.0f64;
    let mut pen = 0.0f64;
    for ob in obstacles {
        let w = obstacle_weight(ob.class, style);
        let mut prev_depth = None;
        for (idx, &(u, p)) in pts.iter().enumerate() {
            let rho = thickness.at(u).0 * 0.5;
            let sd = ob.rect.signed_distance(p).0;
            let hard = (rho - sd).max(0.0);
            let soft = (rho + style.clearance.0 - sd).max(0.0);
            pen += w * soft * soft;
            if let Some(prev) = prev_depth {
                let ds = pts[idx].1.distance(pts[idx - 1].1).0;
                raw_area += 0.5 * (hard + prev) * ds;
            }
            prev_depth = Some(hard);
        }
    }

    // Endpoint tangent angles from horizontal, in degrees.
    let mut tangent = 0.0f64;
    for u in [0.0, 1.0] {
        let d = cubic_derivative(points, u);
        let ang = d.y.0.abs().atan2(d.x.0.abs()).to_degrees();
        let excess = (ang - style.tangent_limit_deg).max(0.0);
        tangent += style.weight_tangent * excess * excess;
    }

    // Staff-line hugging: fraction of samples within a quarter space of a
    // line, weighted by how close they run.
    let mut nearness = 0.0f64;
    if !spec.staff_lines.is_empty() {
        for &(_, p) in &pts {
            let mut dist = f64::INFINITY;
            for line in &spec.staff_lines {
                dist = dist.min((p.y.0 - line.0).abs());
            }
            nearness += (1.0 - dist / 0.25).max(0.0);
        }
        nearness /= pts.len() as f64;
    }

    let dh = apex - h0.0;
    let da = arm - style.arm_preferred;
    let dasym = (offsets.0.0 - offsets.1.0).abs();
    let score = {
        let collision_area = style.weight_collision_area * raw_area;
        let penetration = style.weight_penetration * pen;
        let end_motion =
            style.weight_end_motion * (offsets.0.0 * offsets.0.0 + offsets.1.0 * offsets.1.0);
        let height_dev = style.weight_height * dh * dh;
        let arm_dev = style.weight_arm * da * da;
        let line_nearness = style.weight_line_nearness * nearness;
        let asymmetry = style.weight_asymmetry * dasym * dasym;
        let total = collision_area
            + penetration
            + end_motion
            + height_dev
            + arm_dev
            + tangent
            + line_nearness
            + asymmetry;
        CurveScore {
            collision_area,
            penetration,
            end_motion,
            height_dev,
            arm_dev,
            tangent,
            line_nearness,
            asymmetry,
            total,
            raw_collision_area: raw_area,
        }
    };
    Some(CurveCandidate { points: *points, arm, apex_height: Sp(apex), offsets, score })
}

/// Fit the best curve for the spec: generate candidates, score them, take
/// the cheapest (earliest generation order breaks exact ties). `None` only
/// for a degenerate zero-length chord.
pub fn fit_curve(
    spec: &CurveSpec,
    obstacles: &[CurveObstacle],
    style: &CurveStyle,
) -> Option<CurveFit> {
    let cands = score_candidates(spec, obstacles, style);
    let mut best: Option<&CurveCandidate> = None;
    for c in &cands {
        let better = match best {
            None => true,
            Some(b) => c.score.total < b.score.total,
        };
        if better {
            best = Some(c);
        }
    }
    let len = spec.start.distance(spec.end);
    best.map(|c| CurveFit {
        points: c.points,
        thickness: ThicknessProfile { end: style.thickness_end, mid: style.thickness_mid },
        preferred_height: preferred_height(spec.kind, len, style),
        score: c.score,
    })
}

#[cfg(test)]
mod curve_tests {
    use super::*;
    use crate::style::CurveStyle;

    fn style() -> CurveStyle {
        CurveStyle::default()
    }

    fn slur_spec(x1: f64) -> CurveSpec {
        CurveSpec {
            start: SpPoint::xy(0.0, 0.0),
            end: SpPoint::xy(x1, 0.0),
            side: CurveSide::Above,
            kind: CurveKind::Slur,
            staff_lines: Vec::new(),
        }
    }

    #[test]
    fn thickness_profile_hand_values() {
        let t = ThicknessProfile { end: Sp(0.10), mid: Sp(0.22) };
        assert!((t.at(0.0).0 - 0.10).abs() < 1e-12);
        assert!((t.at(1.0).0 - 0.10).abs() < 1e-12);
        assert!((t.at(0.5).0 - 0.22).abs() < 1e-12);
        // t(0.25) = 0.10 + 0.12 * 4 * 0.25 * 0.75 = 0.19.
        assert!((t.at(0.25).0 - 0.19).abs() < 1e-12);
    }

    #[test]
    fn preferred_heights_hand_values() {
        let s = style();
        // Slur, L = 5: 0.55 + 0.12 * 5 = 1.15 (inside the clamp).
        assert!((preferred_height(CurveKind::Slur, Sp(5.0), &s).0 - 1.15).abs() < 1e-12);
        // Tie, L = 5: 0.30 + 0.08 * 5 = 0.70.
        assert!((preferred_height(CurveKind::Tie, Sp(5.0), &s).0 - 0.70).abs() < 1e-12);
        // Clamps bind at the extremes.
        assert_eq!(preferred_height(CurveKind::Slur, Sp(0.1), &s), s.slur_height_min);
        assert_eq!(preferred_height(CurveKind::Slur, Sp(100.0), &s), s.slur_height_max);
        assert_eq!(preferred_height(CurveKind::Tie, Sp(100.0), &s), s.tie_height_max);
    }

    #[test]
    fn unobstructed_slur_picks_preferred_shape() {
        let s = style();
        let spec = slur_spec(8.0);
        let fit = fit_curve(&spec, &[], &s).unwrap();
        // No obstacles: no endpoint motion, preferred arm, zero collision.
        assert_eq!(fit.points[0], SpPoint::xy(0.0, 0.0));
        assert_eq!(fit.points[3], SpPoint::xy(8.0, 0.0));
        assert_eq!(fit.score.raw_collision_area, 0.0);
        assert_eq!(fit.score.end_motion, 0.0);
        assert!(fit.score.arm_dev < 1e-12, "arm deviates: {:?}", fit.score);
        // Arches upward: control points above the chord (negative y).
        assert!(fit.points[1].y.0 < 0.0 && fit.points[2].y.0 < 0.0);
        // The apex sits near the preferred height, but the tangent limit
        // legitimately flattens a span this short: at full preferred
        // height the launch angle would exceed 35 degrees, so the winner
        // trades a little height for legal tangents (the tangent weight
        // outranks the height weight by design).
        let apex = -eval_cubic(&fit.points, 0.5).y.0;
        assert!(
            apex >= 0.70 * fit.preferred_height.0 && apex <= 1.05 * fit.preferred_height.0,
            "apex {} far from preferred {}",
            apex,
            fit.preferred_height.0
        );
        assert_eq!(fit.score.tangent, 0.0, "winner should have legal tangents");
    }

    #[test]
    fn tie_is_flatter_than_slur() {
        let s = style();
        let mut spec = slur_spec(6.0);
        let slur = fit_curve(&spec, &[], &s).unwrap();
        spec.kind = CurveKind::Tie;
        let tie = fit_curve(&spec, &[], &s).unwrap();
        let slur_apex = -eval_cubic(&slur.points, 0.5).y.0;
        let tie_apex = -eval_cubic(&tie.points, 0.5).y.0;
        assert!(tie_apex < slur_apex, "tie {} not flatter than slur {}", tie_apex, slur_apex);
    }

    #[test]
    fn below_side_arches_downward() {
        let s = style();
        let mut spec = slur_spec(8.0);
        spec.side = CurveSide::Below;
        let fit = fit_curve(&spec, &[], &s).unwrap();
        assert!(fit.points[1].y.0 > 0.0 && fit.points[2].y.0 > 0.0);
    }

    /// The categorical property: the winner is collision-free whenever a
    /// clean candidate exists, and any *visible* ink overlap (area beyond
    /// a grazing threshold) is priced above the ugliest clean candidate in
    /// the whole grid — collision dominates aesthetics, it does not trade
    /// against them.
    #[test]
    fn collision_free_always_beats_colliding() {
        let s = style();
        let spec = slur_spec(8.0);
        // A tall accidental under the middle of the slur, reaching up to
        // just past the preferred apex (h0 = 1.51 for L = 8).
        let obstacles = [CurveObstacle {
            rect: SpRect::xywh(3.5, -1.7, 1.0, 1.7),
            class: ObstacleClass::NoteCore,
        }];
        let cands = score_candidates(&spec, &obstacles, &s);
        let fit = fit_curve(&spec, &obstacles, &s).unwrap();
        assert_eq!(fit.score.raw_collision_area, 0.0, "winner collides: {:?}", fit.score);
        let clean_worst = cands
            .iter()
            .filter(|c| c.score.raw_collision_area == 0.0)
            .map(|c| c.score.total)
            .fold(f64::NEG_INFINITY, f64::max);
        // The dominance threshold: a visible overlap of 0.15 sp^2 (well
        // under one notehead) already outprices the worst clean candidate.
        let visible = 0.15;
        assert!(
            clean_worst < s.weight_collision_area * visible,
            "aesthetic ceiling {} defeats the collision weight",
            clean_worst
        );
        for c in &cands {
            if c.score.raw_collision_area >= visible {
                assert!(
                    c.score.total > clean_worst,
                    "visibly colliding candidate ({}) beats a clean one ({})",
                    c.score.total,
                    clean_worst
                );
            }
        }
        // And the actual best colliding candidate loses to the winner.
        let colliding_best = cands
            .iter()
            .filter(|c| c.score.raw_collision_area > 0.0)
            .map(|c| c.score.total)
            .fold(f64::INFINITY, f64::min);
        assert!(colliding_best > fit.score.total);
        // The winner clears the obstacle by rising, not by giving up on
        // the span: its apex exceeds the preferred height.
        let apex = -eval_cubic(&fit.points, 0.5).y.0;
        assert!(apex > fit.preferred_height.0);
    }

    #[test]
    fn staff_line_hugging_is_penalized() {
        let s = style();
        // Fit once with no lines; then place a staff line exactly at the
        // winner's apex and refit. Candidate geometry is unchanged, so we
        // can compare the old winner's would-be score directly.
        let mut spec = slur_spec(7.0);
        let clean = fit_curve(&spec, &[], &s).unwrap();
        let apex_y = eval_cubic(&clean.points, 0.5).y.0;
        spec.staff_lines = vec![Sp(apex_y)];
        let cands = score_candidates(&spec, &[], &s);
        let naive = cands
            .iter()
            .find(|c| c.points == clean.points)
            .expect("candidate grid changed under staff lines");
        assert!(naive.score.line_nearness > 0.0, "hugging term never engaged");
        let steered = fit_curve(&spec, &[], &s).unwrap();
        // The line-aware winner never does worse than re-using the naive
        // curve, and never hugs the line harder.
        assert!(steered.score.total <= naive.score.total + 1e-9);
        assert!(
            steered.score.line_nearness <= naive.score.line_nearness + 1e-9,
            "line-aware fit hugs the line more than the naive curve"
        );
    }

    #[test]
    fn degenerate_chord_returns_none() {
        let s = style();
        let spec = CurveSpec {
            start: SpPoint::xy(2.0, 1.0),
            end: SpPoint::xy(2.0, 1.0),
            side: CurveSide::Above,
            kind: CurveKind::Tie,
            staff_lines: Vec::new(),
        };
        assert!(fit_curve(&spec, &[], &s).is_none());
    }

    #[test]
    fn deterministic_bitwise() {
        let s = style();
        let spec = slur_spec(9.0);
        let obstacles = [CurveObstacle {
            rect: SpRect::xywh(4.0, -1.2, 0.8, 1.2),
            class: ObstacleClass::Marking,
        }];
        let a = fit_curve(&spec, &obstacles, &s).unwrap();
        let b = fit_curve(&spec, &obstacles, &s).unwrap();
        assert_eq!(a.score.total.to_bits(), b.score.total.to_bits());
        for (p, q) in a.points.iter().zip(&b.points) {
            assert_eq!(p.x.0.to_bits(), q.x.0.to_bits());
            assert_eq!(p.y.0.to_bits(), q.y.0.to_bits());
        }
    }
}
