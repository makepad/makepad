//! Cross-module integration: the whole kernel pipeline on one synthetic
//! score, exercised end to end and checked for determinism.

use crate::breaking::{
    break_lines, break_pages, BreakRule, LineWidths, MeasureSpacing, PageSpec, SystemVertical,
    TurnRule,
};
use crate::curve::{fit_curve, CurveKind, CurveObstacle, CurveSide, CurveSpec, ObstacleClass};
use crate::skyline::{min_clearance, CollisionShape, SkySide, Skyline};
use crate::sp::{Sp, SpPoint, SpRect};
use crate::spacing::{regularize_equal_durations, solve_spacing, SpacingColumn, SpacingFit};
use crate::style::LayoutStyle;
use crate::testutil::Lcg;

/// A deterministic synthetic score: `n` measures of mixed rhythms.
fn synth_measures(n: usize, style: &LayoutStyle) -> Vec<Vec<SpacingColumn>> {
    let mut rng = Lcg::new(0x5eed_1000);
    (0..n)
        .map(|_| {
            let pattern = rng.next_u64() % 3;
            let durations: &[f64] = match pattern {
                0 => &[0.25, 0.25, 0.25, 0.25],
                1 => &[0.5, 0.125, 0.125, 0.25],
                _ => &[0.125, 0.125, 0.125, 0.125, 0.25, 0.25],
            };
            durations
                .iter()
                .map(|&d| {
                    let headroom = Sp(0.4 + 0.4 * rng.next_f64());
                    let minimum = headroom + Sp(0.6);
                    SpacingColumn::from_duration(d, headroom, minimum, &style.spacing)
                })
                .collect()
        })
        .collect()
}

fn fold_bits(acc: u64, v: f64) -> u64 {
    acc.rotate_left(7) ^ v.to_bits()
}

/// Run the full pipeline once and produce a digest of every number that
/// matters, so two runs can be compared bit-for-bit.
fn run_pipeline() -> (u64, usize, usize) {
    let style = LayoutStyle::default();
    let measures = synth_measures(60, &style);
    let summaries: Vec<MeasureSpacing> = measures
        .iter()
        .map(|cols| MeasureSpacing::from_columns(cols, BreakRule::Allowed, 0.0))
        .collect();
    let widths = LineWidths { first: Sp(52.0), rest: Sp(58.0) };
    let lines = break_lines(&summaries, widths, &style.breaking, 3);
    assert!(!lines.emergency);
    let plan = &lines.alternatives[0];

    let mut digest = 0u64;
    let mut verts = Vec::new();
    let mut prev_lower: Option<Skyline> = None;
    for sys in &plan.systems {
        // Solve and regularize the system's springs.
        let cols: Vec<SpacingColumn> = sys
            .measures
            .clone()
            .flat_map(|mi| measures[mi].iter().cloned())
            .collect();
        let mut sol = solve_spacing(&cols, sys.solved_width());
        assert_eq!(sol.fit, SpacingFit::Exact);
        let pairs: Vec<(usize, usize)> = (0..cols.len() - 1)
            .filter(|&i| {
                cols[i].duration_class.is_some() && cols[i].duration_class == cols[i + 1].duration_class
            })
            .map(|i| (i, i + 1))
            .collect();
        regularize_equal_durations(&mut sol, &cols, &pairs, &style.spacing);
        assert!((sol.total().0 - sys.solved_width().0).abs() < 1e-6);
        for (c, w) in cols.iter().zip(&sol.widths) {
            assert!(w.0 >= c.minimum.0 - 1e-9);
            digest = fold_bits(digest, w.0);
        }

        // Build notehead-ish collision boxes at the solved x positions and
        // derive the system's skylines.
        let mut x = 0.0;
        let mut shapes = Vec::new();
        for (i, w) in sol.widths.iter().enumerate() {
            let y = -1.5 + ((i * 7) % 5) as f64 * 0.75;
            shapes.push(CollisionShape::Rect(SpRect::xywh(x, y, 1.18, 1.0)));
            x += w.0;
        }
        let upper = Skyline::from_shapes(SkySide::Upper, &shapes);
        let lower = Skyline::from_shapes(SkySide::Lower, &shapes);
        // Height of the system block from its own skylines plus the staff.
        let top = upper.extreme_y().unwrap().0.min(0.0);
        let bottom = lower.extreme_y().unwrap().0.max(4.0);
        let height = bottom - top;
        digest = fold_bits(digest, height);
        // The gap to the previous system respects skyline clearance.
        let gap_min = match &prev_lower {
            Some(pl) => min_clearance(pl, &upper)
                .map(|d| d + style.vertical.general_clearance)
                .unwrap_or(style.vertical.general_clearance)
                .max(Sp(0.0)),
            None => Sp(0.0),
        };
        verts.push(SystemVertical {
            height: Sp(height),
            gap_natural: (gap_min + Sp(2.0)).max(style.vertical.staff_distance),
            gap_min,
            gap_stretch: Sp(3.0),
            turn_after: TurnRule::Allowed,
        });
        prev_lower = Some(lower);
    }

    // Pages.
    let pages = break_pages(&verts, PageSpec { usable_height: Sp(70.0) }, &style.breaking);
    assert!(!pages.emergency);
    let mut at = 0;
    for p in &pages.pages {
        assert_eq!(p.systems.start, at);
        at = p.systems.end;
        digest = fold_bits(digest, p.adjustment);
    }
    assert_eq!(at, verts.len());

    // A slur over obstacles taken from the first system's boxes.
    let obstacles = [
        CurveObstacle { rect: SpRect::xywh(2.0, -1.4, 1.2, 1.4), class: ObstacleClass::NoteCore },
        CurveObstacle { rect: SpRect::xywh(4.4, -0.9, 0.9, 0.9), class: ObstacleClass::Marking },
    ];
    let spec = CurveSpec {
        start: SpPoint::xy(0.0, 0.0),
        end: SpPoint::xy(7.5, -0.5),
        side: CurveSide::Above,
        kind: CurveKind::Slur,
        staff_lines: vec![Sp(0.0), Sp(1.0), Sp(2.0), Sp(3.0), Sp(4.0)],
    };
    let fit = fit_curve(&spec, &obstacles, &style.curve).unwrap();
    assert_eq!(fit.score.raw_collision_area, 0.0, "pipeline slur collides");
    for p in &fit.points {
        digest = fold_bits(digest, p.x.0);
        digest = fold_bits(digest, p.y.0);
    }

    (digest, plan.systems.len(), pages.pages.len())
}

#[test]
fn full_pipeline_is_coherent_and_deterministic() {
    let (d1, systems, pages) = run_pipeline();
    let (d2, s2, p2) = run_pipeline();
    assert_eq!(d1, d2, "pipeline is not deterministic");
    assert_eq!(systems, s2);
    assert_eq!(pages, p2);
    assert!(systems >= 5, "unexpectedly few systems: {}", systems);
    assert!(pages >= 2, "unexpectedly few pages: {}", pages);
}
