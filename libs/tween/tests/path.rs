//! Motion paths (libs/tween/src/path.rs) and the engine's `PropTo::path`:
//! the SVG parser, the arc-length table, tangents and headings, wrap and
//! spans, `through` / `cubic_points`, 3D, and path tweens (exact ends,
//! clamped overshoot, align, offset, overwrite, stagger, keyframes,
//! `sample`, the path lifecycle, repeat refresh).

mod util;

use makepad_tween::*;
use std::f64::consts::PI;
use util::*;

const R: PropKey = PropKey(10);
const Z: PropKey = PropKey(11);
const K: PropKey = PropKey(12);

fn svg(d: &str) -> MotionPath {
    MotionPath::from_svg(d).unwrap_or_else(|e| panic!("{d}: {e}"))
}

#[track_caller]
fn close2(a: [f64; 3], b: [f64; 2], tol: f64, what: &str) {
    close(a[0], b[0], tol, what);
    close(a[1], b[1], tol, what);
}

fn speed(s: &PathSeg, t: f64) -> f64 {
    let d = s.derivative(t);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The test's own arc length: adaptive Simpson on |B'|.
fn simpson(s: &PathSeg, a: f64, b: f64, eps: f64) -> f64 {
    #[allow(clippy::too_many_arguments)]
    fn rec(
        s: &PathSeg,
        a: f64,
        b: f64,
        fa: f64,
        fm: f64,
        fb: f64,
        whole: f64,
        eps: f64,
        depth: u32,
    ) -> f64 {
        let m = 0.5 * (a + b);
        let (lm, rm) = (0.5 * (a + m), 0.5 * (m + b));
        let (flm, frm) = (speed(s, lm), speed(s, rm));
        let left = (m - a) / 6.0 * (fa + 4.0 * flm + fm);
        let right = (b - m) / 6.0 * (fm + 4.0 * frm + fb);
        let d = left + right - whole;
        if depth > 40 || d.abs() <= 15.0 * eps {
            left + right + d / 15.0
        } else {
            rec(s, a, m, fa, flm, fm, left, eps / 2.0, depth + 1)
                + rec(s, m, b, fm, frm, fb, right, eps / 2.0, depth + 1)
        }
    }
    let (fa, fm, fb) = (speed(s, a), speed(s, 0.5 * (a + b)), speed(s, b));
    let whole = (b - a) / 6.0 * (fa + 4.0 * fm + fb);
    rec(s, a, b, fa, fm, fb, whole, eps, 0)
}

/// Arc length along `path` between two sampled points (p before q).
fn arc_between(path: &MotionPath, p: &PathPoint, q: &PathPoint) -> f64 {
    if p.seg == q.seg {
        return simpson(&path.segment(p.seg), p.t, q.t, 1e-12);
    }
    let mut sum = simpson(&path.segment(p.seg), p.t, 1.0, 1e-12);
    for i in p.seg + 1..q.seg {
        sum += simpson(&path.segment(i), 0.0, 1.0, 1e-12);
    }
    sum + simpson(&path.segment(q.seg), 0.0, q.t, 1e-12)
}

fn circle() -> MotionPath {
    svg("M100,50 A50,50 0 1,1 0,50 A50,50 0 1,1 100,50 Z")
}

// ---------------------------------------------------------------------------
// The SVG parser
// ---------------------------------------------------------------------------

#[test]
fn svg_lines_and_relative_forms() {
    let p = svg("M0,0 L10,0");
    assert_eq!(p.length(), 10.0);
    assert!(!p.is_closed());

    let p = svg("M0 0 H10 V10 h-10 z");
    assert!(p.is_closed());
    assert_eq!(p.segment_count(), 4);
    assert_eq!(p.subpath_count(), 1);
    assert_eq!(p.length(), 40.0);
    assert_eq!(p.segment(3).end(), [0.0, 0.0, 0.0]);

    let p = svg("m5,5 l5,0");
    assert_eq!(p.start_point(), [5.0, 5.0, 0.0]);
    assert_eq!(p.end_point(), [10.0, 5.0, 0.0]);

    // Implicit L after M.
    let p = svg("M0,0 10,0 10,10");
    assert_eq!(p.segment_count(), 2);
    assert!(p.segment(1).kind == SegKind::Line);
    assert_eq!(p.end_point(), [10.0, 10.0, 0.0]);

    // Implicit l after m; relative c groups share their start.
    let p = svg("m1,1 2,0 c1,0 2,1 3,0 1,0 2,-1 3,0");
    assert_eq!(p.segment(0).end(), [3.0, 1.0, 0.0]);
    assert_eq!(p.segment(1).p[1], [4.0, 1.0, 0.0]);
    assert_eq!(p.segment(1).p[2], [5.0, 2.0, 0.0]);
    assert_eq!(p.segment(1).end(), [6.0, 1.0, 0.0]);
    assert_eq!(p.segment(2).p[1], [7.0, 1.0, 0.0]);
    assert_eq!(p.end_point(), [9.0, 1.0, 0.0]);

    // After Z a drawing command starts a new subpath at the old start.
    let p = svg("M0,0 L10,0 L10,10 Z L0,20");
    assert_eq!(p.subpath_count(), 2);
    assert!(!p.is_closed());
    assert_eq!(p.segment(3).start(), [0.0, 0.0, 0.0]);
    assert_eq!(p.segment(3).subpath, 1);
}

#[test]
fn svg_number_forms() {
    let p = svg("M1.5.5L-2-2");
    assert_eq!(p.start_point(), [1.5, 0.5, 0.0]);
    assert_eq!(p.end_point(), [-2.0, -2.0, 0.0]);
    let p = svg("M1e1,0 L0,0");
    assert_eq!(p.start_point(), [10.0, 0.0, 0.0]);
    let p = svg("M+1.,-.5E1\t\n\r\x0cL2,3");
    assert_eq!(p.start_point(), [1.0, -5.0, 0.0]);
    // Arc flags abut the next token.
    let p = svg("M0,0 a25 25 0 1050 0");
    assert_eq!(p.segment_count(), 2);
    assert_eq!(p.end_point(), [50.0, 0.0, 0.0]);
    close(p.length(), PI * 25.0, 0.012, "half circle r 25");
}

#[test]
fn svg_s_and_t_reflect() {
    // S after C reflects c2 about the current point, exactly.
    let p = svg("M0,0 C10,20 30,20 40,0 S70,-20 80,0");
    assert_eq!(p.segment(1).p[1], [50.0, -20.0, 0.0]);
    assert_eq!(p.segment(1).p[2], [70.0, -20.0, 0.0]);
    // S after S reflects the previous S.
    let p = svg("M0,0 C10,20 30,20 40,0 S70,-20 80,0 S110,20 120,0");
    assert_eq!(p.segment(2).p[1], [90.0, 20.0, 0.0]);
    // T after Q, and T after T (its implied control).
    let p = svg("M0,0 Q10,20 20,0 T40,0 T60,0");
    assert_eq!(p.segment(1).kind, SegKind::Quad);
    assert_eq!(p.segment(1).p[1], [30.0, -20.0, 0.0]);
    assert_eq!(p.segment(2).p[1], [50.0, 20.0, 0.0]);
    // S after L, T after C: the control is the current point.
    let p = svg("M0,0 L10,0 S20,10 30,0");
    assert_eq!(p.segment(1).p[1], [10.0, 0.0, 0.0]);
    let p = svg("M0,0 C0,10 10,10 10,0 T30,0");
    assert_eq!(p.segment(1).p[1], [10.0, 0.0, 0.0]);
}

#[test]
fn svg_arc_half_circle() {
    let p = svg("M0,0 A50,50 0 0,1 100,0");
    assert_eq!(p.segment_count(), 2);
    assert!(p.segment(0).kind == SegKind::Cubic);
    assert_eq!(p.end_point(), [100.0, 0.0, 0.0]);
    close2(p.sample(0.5).pos, [50.0, -50.0], 1e-9, "arc midpoint");
    // Two 90-degree cubics are 1.4e-4 longer than the arc.
    close(p.length(), PI * 50.0, 0.025, "arc length");
    let b = p.bounds();
    close2(b.min, [0.0, -50.0], 1e-9, "bounds min");
    close2(b.max, [100.0, 0.0], 1e-9, "bounds max");
    // Rotated ellipse ends exactly on its endpoint too.
    let p = svg("M10,10 A40,20 30 1,0 70,40");
    assert_eq!(p.end_point(), [70.0, 40.0, 0.0]);
    assert!(p.segment_count() >= 3);
}

#[test]
fn svg_arc_degenerate() {
    // A zero radius draws a line.
    let p = svg("M0,0 A0,10 0 0,1 10,0");
    assert_eq!(p.segment_count(), 1);
    assert_eq!(p.segment(0).kind, SegKind::Line);
    // The same endpoint draws nothing.
    let p = svg("M0,0 L10,0 A5,5 0 0,1 10,0");
    assert_eq!(p.segment_count(), 1);
    // Radii too small for the chord are scaled up (to a half circle).
    let p = svg("M0,0 A10,10 0 0,1 100,0");
    close2(
        p.sample(0.5).pos,
        [50.0, -50.0],
        1e-9,
        "scaled arc midpoint",
    );
    // Negative radii count as their magnitude.
    let q = svg("M0,0 A-10,-10 0 0,1 100,0");
    assert_eq!(p.sample(0.3).pos, q.sample(0.3).pos);
}

#[test]
fn svg_errors() {
    use PathError::*;
    let err = |d: &str| MotionPath::from_svg(d).unwrap_err();
    assert_eq!(err(""), Empty);
    assert_eq!(err(" , \n"), Empty);
    assert_eq!(err("M0,0"), Empty);
    assert_eq!(err("L10,10"), NoCurrentPoint { at: 0 });
    assert_eq!(err("  c1,1 2,2 3,3"), NoCurrentPoint { at: 2 });
    assert_eq!(err("M0,0 X"), UnknownCommand { at: 5, cmd: b'X' });
    assert_eq!(err("M0,0 L10"), Syntax { at: 8 });
    assert_eq!(err("M0,0 L10 L5,5"), Syntax { at: 9 });
    assert_eq!(err("M0,0 L1e999,0"), NotFinite);
    assert_eq!(err("M5,5 L5,5"), ZeroLength);
    assert_eq!(err("M0,0 Z 5"), Syntax { at: 7 });
    assert_eq!(err("5 M0,0"), Syntax { at: 0 });
    assert_eq!(err("M1e,0 L1,1"), Syntax { at: 1 });
    assert_eq!(err("M0,0 . L1,1"), Syntax { at: 5 });
    assert_eq!(err("M0,0 A5,5 0 2,0 9,9"), Syntax { at: 12 });
    assert_eq!(err("M0,0 #"), UnknownCommand { at: 5, cmd: b'#' });
    let text = err("M0,0 X").to_string();
    assert!(
        text.starts_with("motion path: ") && text.ends_with("at byte 5"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------
// The arc-length table
// ---------------------------------------------------------------------------

fn test_cubic() -> MotionPath {
    let mut b = MotionPath::builder();
    b.move_to(0.0, 0.0)
        .cubic_to(0.0, 200.0, 300.0, -100.0, 300.0, 100.0);
    b.finish().unwrap()
}

#[test]
fn equal_u_steps_are_equal_arc_lengths() {
    let p = test_cubic();
    let total = simpson(&p.segment(0), 0.0, 1.0, 1e-12);
    close(p.length(), total, 1e-9, "table length vs Simpson");
    let step = p.length() / 64.0;
    let mut prev = p.sample(0.0);
    for k in 1..=64 {
        let q = p.sample(k as f64 / 64.0);
        let d = arc_between(&p, &prev, &q);
        close(d, step, 1e-6, &format!("step {k}"));
        prev = q;
    }
    // sample_at_length agrees with sample.
    let a = p.sample_at_length(p.length() * 0.3);
    let b = p.sample(0.3);
    close2(a.pos, [b.pos[0], b.pos[1]], 1e-9, "at_length vs sample");
}

#[test]
fn span_counts_follow_the_density_rule() {
    let lines = MotionPath::polyline(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], false).unwrap();
    assert_eq!(lines.span_count(), 2);
    let c = test_cubic();
    let n = c.span_count();
    assert!((PATH_SPANS_MIN..=PATH_SPANS_MAX).contains(&n), "{n}");
    assert!(
        n > PATH_SPANS_MIN,
        "a curve that bends this much refines: {n}"
    );
    // A straight cubic with even handles needs no refinement.
    let mut b = MotionPath::builder();
    b.move_to(0.0, 0.0)
        .cubic_to(10.0, 0.0, 20.0, 0.0, 30.0, 0.0);
    assert_eq!(b.finish().unwrap().span_count(), PATH_SPANS_MIN);
    let s = svg("M20,160 C120,-20 220,220 320,100 S520,0 580,160 A60,60 0 0,1 460,160 L400,120 Q340,60 300,140 T160,160 Z");
    let curves = (0..s.segment_count())
        .filter(|&i| s.segment(i).kind != SegKind::Line)
        .count() as u32;
    let lines = s.segment_count() - curves;
    assert!(s.span_count() >= lines + curves * PATH_SPANS_MIN);
    assert!(s.span_count() <= lines + curves * PATH_SPANS_MAX);
}

#[test]
fn ends_are_exact() {
    let wave = MotionPath::through(
        &[[0.0, 100.0], [100.0, 20.0], [200.0, 180.0], [300.0, 60.0]],
        1.0,
    )
    .unwrap();
    for p in [test_cubic(), wave, svg("M3,4 Q10,-7 20,5 L31,9")] {
        assert_eq!(p.sample(0.0).pos, p.start_point());
        assert_eq!(p.sample(1.0).pos, p.end_point());
        assert_eq!(p.sample(0.0).t, 0.0);
        assert_eq!(p.sample(1.0).t, 1.0);
        assert_eq!(p.sample_at_length(p.length()).pos, p.end_point());
        assert_eq!(p.sample_span(0.2, 0.7, 1.0), p.sample(0.7));
    }
    // A span boundary (here a line's end) is hit exactly.
    let p = MotionPath::polyline(&[[0.0, 0.0], [10.0, 0.0], [10.0, 30.0]], false).unwrap();
    let q = p.sample(0.25);
    assert_eq!((q.pos, q.seg, q.t), ([10.0, 0.0, 0.0], 0, 1.0));
    assert_eq!(p.sample(0.625).pos, [10.0, 15.0, 0.0]);
}

// ---------------------------------------------------------------------------
// Tangents and headings
// ---------------------------------------------------------------------------

#[test]
fn tangents_and_headings() {
    let p = svg("M0,0 L10,10");
    let s = p.sample(0.5);
    close(s.tangent[0], 0.5f64.sqrt(), 1e-15, "tx");
    close(s.tangent[1], 0.5f64.sqrt(), 1e-15, "ty");
    close(s.angle_deg, 45.0, 1e-12, "45");
    close(svg("M0,0 L0,10").sample(0.3).angle_deg, 90.0, 1e-12, "90");
    close(svg("M10,0 L0,0").sample(0.3).angle_deg, 180.0, 1e-12, "180");
    // A cusp at the start (zero first handle): toward c2.
    let c = svg("M0,0 C0,0 10,10 20,0").sample(0.0);
    close(c.tangent[0], 0.5f64.sqrt(), 1e-12, "cusp tx");
    close(c.tangent[1], 0.5f64.sqrt(), 1e-12, "cusp ty");
    // A zero last handle: from c1.
    let seg = svg("M0,0 C10,10 20,0 20,0").segment(0);
    let t = seg.tangent(1.0);
    close(t[0], 0.5f64.sqrt(), 1e-12, "end cusp tx");
    close(t[1], -(0.5f64.sqrt()), 1e-12, "end cusp ty");
    // Wrapped angle.
    let pt = PathPoint {
        angle_deg: 540.0,
        ..PathPoint::default()
    };
    assert_eq!(pt.angle_wrapped(), 180.0);
    let pt = PathPoint {
        angle_deg: -190.0,
        ..PathPoint::default()
    };
    close(pt.angle_wrapped(), 170.0, 1e-12, "wrap -190");
}

#[test]
fn a_full_circle_turns_360_degrees() {
    let p = circle();
    assert!(p.is_closed());
    assert_eq!(p.segment_count(), 4);
    close(p.sample(0.0).angle_deg, 90.0, 1e-9, "start angle");
    close(p.total_turn(), 360.0, 1e-9, "total turn");
    close2(p.sample(0.25).pos, [50.0, 100.0], 1e-9, "quarter");
    close2(p.sample(0.5).pos, [0.0, 50.0], 1e-9, "half");
    close(p.length(), PI * 100.0, 0.05, "circumference");
    let mut prev = f64::NEG_INFINITY;
    for k in 0..=200 {
        let a = p.sample(k as f64 / 200.0).angle_deg;
        assert!(a > prev, "angle not increasing at {k}: {a} after {prev}");
        prev = a;
    }
    close(prev, 450.0, 1e-9, "end angle");
}

// ---------------------------------------------------------------------------
// Wrap and spans
// ---------------------------------------------------------------------------

#[test]
fn closed_paths_wrap_and_open_paths_clamp() {
    let c = circle();
    let a = c.sample(0.25);
    let b = c.sample(1.25);
    assert_eq!(a.pos, b.pos);
    close(b.angle_deg, a.angle_deg + 360.0, 1e-9, "one lap on");
    let n = c.sample(-0.75);
    assert_eq!(n.pos, a.pos);
    close(n.angle_deg, a.angle_deg - 360.0, 1e-9, "one lap back");
    assert_eq!(c.sample(2.0).pos, c.end_point());
    close(
        c.sample(2.0).angle_deg,
        c.sample(1.0).angle_deg + 360.0,
        1e-9,
        "u 2",
    );
    // Span (0.75, 1.25) runs over the seam.
    close2(
        c.sample_span(0.75, 1.25, 0.5).pos,
        [100.0, 50.0],
        1e-9,
        "seam",
    );
    assert_eq!(c.sample_span(0.75, 1.25, 1.0), c.sample(1.25));

    let l = svg("M0,0 L100,0");
    assert_eq!(l.sample(-0.5), l.sample(0.0));
    assert_eq!(l.sample(1.5), l.sample(1.0));
    assert_eq!(l.sample(f64::NAN), l.sample(0.0));
    close(l.sample_span(0.2, 0.8, 0.5).pos[0], 50.0, 1e-12, "span mid");
    let r = l.sample_span(0.8, 0.2, 0.0);
    close(r.pos[0], 80.0, 1e-12, "reversed start");
    assert_eq!(r.tangent, [-1.0, -0.0, -0.0]);
    close(r.angle_deg, 180.0, 1e-12, "reversed angle");
    close(
        l.sample_span(0.8, 0.2, 1.0).pos[0],
        20.0,
        1e-12,
        "reversed end",
    );
    // The eased ratio is clamped; non-finite fractions count as 0 and 1.
    assert_eq!(l.sample_span(0.2, 0.8, 1.7), l.sample_span(0.2, 0.8, 1.0));
    assert_eq!(l.sample_span(0.2, 0.8, -0.3), l.sample_span(0.2, 0.8, 0.0));
    assert_eq!(l.sample_span(f64::NAN, f64::INFINITY, 1.0), l.sample(1.0));
    // An empty path answers the default point.
    assert_eq!(MotionPath::default().sample(0.5), PathPoint::default());
}

#[test]
fn subpaths_jump_with_zero_arc_length_and_flatten_in_runs() {
    let p = svg("M0,0 L10,0 M20,0 L30,0");
    assert_eq!(p.subpath_count(), 2);
    assert!(!p.is_closed());
    assert_eq!(p.length(), 20.0);
    assert_eq!(p.sample(0.5).pos, [10.0, 0.0, 0.0]);
    assert_eq!(p.sample(0.75).pos, [25.0, 0.0, 0.0]);
    let mut pts = Vec::new();
    p.flatten(8, |q, first| pts.push((q[0], first)));
    assert_eq!(
        pts,
        vec![(0.0, true), (10.0, false), (20.0, true), (30.0, false)]
    );
    let wave = MotionPath::through(&[[0.0, 0.0], [10.0, 5.0], [20.0, 0.0]], 1.0).unwrap();
    let mut n = 0;
    let mut last = [0.0; 3];
    wave.flatten(24, |q, first| {
        assert_eq!(first, n == 0);
        n += 1;
        last = q;
    });
    assert_eq!(n, 1 + 2 * 24);
    assert_eq!(last, wave.end_point());
}

#[test]
fn translate_moves_points_and_bounds() {
    let p = svg("M0,0 A50,50 0 0,1 100,0");
    let mut q = p.clone();
    q.translate(5.0, -3.0);
    assert_eq!(q.length(), p.length());
    let (a, b) = (p.sample(0.4).pos, q.sample(0.4).pos);
    close(b[0], a[0] + 5.0, 1e-12, "x");
    close(b[1], a[1] - 3.0, 1e-12, "y");
    close2(q.bounds().min, [5.0, -53.0], 1e-9, "min");
    close2(q.bounds().size(), [100.0, 50.0], 1e-9, "size");
    close2(q.bounds().center(), [55.0, -28.0], 1e-9, "center");
}

// ---------------------------------------------------------------------------
// through / cubic_points / polyline
// ---------------------------------------------------------------------------

#[test]
fn thru_matches_hand_computed_cubics() {
    let pts = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]];
    let p = MotionPath::through(&pts, 1.0).unwrap();
    assert_eq!(p.segment_count(), 2);
    let (s0, s1) = (p.segment(0), p.segment(1));
    let h = 23.57022603955158;
    close2(s0.p[1], [33.333333333333336, 0.0], 1e-12, "seg0 c1");
    close2(s0.p[2], [100.0 - h, -h], 1e-12, "seg0 c2");
    close2(s1.p[1], [100.0 + h, h], 1e-12, "seg1 c1");
    close2(s1.p[2], [100.0, 66.66666666666667], 1e-12, "seg1 c2");
    let p2 = MotionPath::through(&pts, 2.0).unwrap();
    close2(
        p2.segment(0).p[1],
        [66.66666666666667, 0.0],
        1e-12,
        "k 2 c1",
    );
    close2(
        p2.segment(1).p[1],
        [100.0 + 2.0 * h, 2.0 * h],
        1e-12,
        "k 2 seg1 c1",
    );
    let p0 = MotionPath::through(&pts, 0.0).unwrap();
    assert_eq!(p0.segment_count(), 2);
    assert!((0..2).all(|i| p0.segment(i).kind == SegKind::Line));
    assert_eq!(p0.length(), 200.0);
    // Negative curviness counts as 0; NaN is an error.
    assert_eq!(
        MotionPath::through(&pts, -1.0).unwrap().segment(0).kind,
        SegKind::Line
    );
    assert_eq!(
        MotionPath::through(&pts, f64::NAN).unwrap_err(),
        PathError::NotFinite
    );
}

#[test]
fn thru_closes_when_first_meets_last() {
    let sq = [
        [0.0, 0.0],
        [100.0, 0.0],
        [100.0, 100.0],
        [0.0, 100.0],
        [0.0005, 0.0],
    ];
    let p = MotionPath::through(&sq, 1.0).unwrap();
    assert!(p.is_closed());
    assert_eq!(p.segment_count(), 4);
    assert!((0..4).all(|i| p.segment(i).kind == SegKind::Cubic));
    assert_eq!(p.end_point(), [0.0, 0.0, 0.0]);
    // Closed: the tangent at the seam is the bisector of both neighbours.
    let t0 = p.sample(0.0).tangent;
    close(t0[0], 0.5f64.sqrt(), 1e-12, "seam tx");
    close(t0[1], -(0.5f64.sqrt()), 1e-12, "seam ty");
    // Two points never close.
    let two = MotionPath::through(&[[0.0, 0.0], [0.0, 0.0005]], 1.0);
    assert!(!two.unwrap().is_closed());
}

#[test]
fn thru_drops_duplicates() {
    let p = MotionPath::through(
        &[
            [0.0, 0.0],
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 1e-12],
            [20.0, 5.0],
        ],
        1.0,
    )
    .unwrap();
    assert_eq!(p.segment_count(), 2);
    assert_eq!(
        MotionPath::through(&[[1.0, 1.0], [1.0, 1.0]], 1.0).unwrap_err(),
        PathError::Empty
    );
    assert_eq!(MotionPath::through(&[], 1.0).unwrap_err(), PathError::Empty);
    assert_eq!(
        MotionPath::through(&[[0.0, f64::INFINITY], [1.0, 1.0]], 1.0).unwrap_err(),
        PathError::NotFinite
    );
}

#[test]
fn cubic_points_counts() {
    let p =
        MotionPath::cubic_points(&[[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]]).unwrap();
    assert_eq!(p.segment_count(), 1);
    assert_eq!(p.segment(0).p[2], [10.0, 10.0, 0.0]);
    assert_eq!(
        MotionPath::cubic_points(&[[0.0, 0.0]; 5]).unwrap_err(),
        PathError::PointCount { got: 5 }
    );
    assert_eq!(
        MotionPath::cubic_points(&[[0.0, 0.0]; 1]).unwrap_err(),
        PathError::PointCount { got: 1 }
    );
    let closed = MotionPath::cubic_points(&[
        [0.0, 0.0],
        [0.0, 10.0],
        [10.0, 10.0],
        [10.0, 0.0],
        [10.0, -10.0],
        [0.0, -10.0],
        [0.0, 0.0],
    ])
    .unwrap();
    assert!(closed.is_closed());
    assert_eq!(closed.segment_count(), 2);
}

#[test]
fn builder_errors_and_subpaths() {
    let mut b = PathBuilder::new();
    b.line_to(1.0, 1.0);
    assert_eq!(b.finish().unwrap_err(), PathError::NoCurrentPoint { at: 0 });
    let mut b = PathBuilder::new();
    b.move_to(0.0, 0.0).line_to(f64::NAN, 1.0).line_to(1.0, 1.0);
    assert_eq!(b.finish().unwrap_err(), PathError::NotFinite);
    // The builder is empty after finish.
    let mut b = PathBuilder::new();
    b.move_to(0.0, 0.0).line_to(1.0, 0.0);
    assert!(b.finish().is_ok());
    assert_eq!(b.current(), None);
    assert_eq!(b.finish().unwrap_err(), PathError::Empty);
    // A move right after a move only moves the start; close adds the line.
    let mut b = PathBuilder::new();
    b.move_to(5.0, 5.0)
        .move_to(0.0, 0.0)
        .line_to(10.0, 0.0)
        .line_to(10.0, 10.0)
        .close();
    assert_eq!(b.current(), Some([0.0, 0.0, 0.0]));
    let p = b.finish().unwrap();
    assert_eq!(
        (p.segment_count(), p.subpath_count(), p.is_closed()),
        (3, 1, true)
    );
    // A zero-length segment is dropped.
    let mut b = PathBuilder::new();
    b.move_to(0.0, 0.0).line_to(0.0, 0.0).line_to(3.0, 4.0);
    let p = b.finish().unwrap();
    assert_eq!((p.segment_count(), p.length()), (1, 5.0));
}

// ---------------------------------------------------------------------------
// 3D
// ---------------------------------------------------------------------------

/// Three turns of the helix (cos t, sin t, t / 2pi), `n` cubics per turn:
/// each cubic takes the circle's optimal handle (4/3 tan(h/4)) in xy and
/// the exact linear handle in z.
fn helix(n: u32) -> MotionPath {
    let h = 2.0 * PI / n as f64;
    let k = 4.0 / 3.0 * (h / 4.0).tan();
    let dz = 1.0 / n as f64;
    let mut pts = vec![[1.0, 0.0, 0.0]];
    for i in 0..3 * n {
        let (a0, a1) = (i as f64 * h, (i + 1) as f64 * h);
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        let (z0, z1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        pts.push([c0 - k * s0, s0 + k * c0, z0 + dz / 3.0]);
        pts.push([c1 + k * s1, s1 - k * c1, z1 - dz / 3.0]);
        pts.push([c1, s1, z1]);
    }
    MotionPath::cubic_points3(&pts).unwrap()
}

#[test]
fn a_helix_is_measured_in_3d() {
    let p = helix(32);
    let exact = 3.0 * (4.0 * PI * PI + 1.0).sqrt();
    close(p.length(), exact, 1e-6, "helix length");
    assert_eq!(p.end_point()[2], 3.0);
    // Equal u steps are equal arc distances, measured in 3D.
    let step = p.length() / 90.0;
    let mut prev = p.sample(0.0);
    for k in 1..=90 {
        let q = p.sample(k as f64 / 90.0);
        close(
            arc_between(&p, &prev, &q),
            step,
            1e-6,
            &format!("helix step {k}"),
        );
        prev = q;
    }
    // The xy heading turns 3 x 360 degrees; the tangent climbs.
    close(p.total_turn(), 1080.0, 1e-6, "helix turn");
    let t = p.sample(0.5).tangent;
    close(
        t[2],
        1.0 / (4.0 * PI * PI + 1.0).sqrt(),
        1e-3,
        "helix tangent z",
    );
    let b = p.bounds();
    close(b.min[2], 0.0, 0.0, "z min");
    close(b.max[2], 3.0, 0.0, "z max");
    close(b.max[0], 1.0, 1e-9, "x max");
    // A line along z has a defined heading (the last one) and length.
    let v =
        MotionPath::polyline3(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 5.0]], false).unwrap();
    assert_eq!(v.length(), 6.0);
    assert_eq!(v.sample(0.9).angle_deg, 0.0);
    let q = v.sample(0.9).pos;
    close(q[0], 1.0, 1e-12, "x");
    close(q[2], 4.4, 1e-12, "z");
    let mut m = v.clone();
    m.translate3(1.0, 2.0, 3.0);
    assert_eq!(m.end_point(), [2.0, 2.0, 8.0]);
}

#[test]
fn a_2d_path_built_in_3d_samples_bitwise_the_same() {
    let flat = |p: &[[f64; 2]]| p.iter().map(|q| [q[0], q[1], 0.0]).collect::<Vec<_>>();
    let pts = [
        [0.0, 100.0],
        [100.0, 20.0],
        [200.0, 180.0],
        [300.0, 60.0],
        [400.0, 160.0],
    ];
    let loop_pts = [
        [300.0, 20.0],
        [520.0, 60.0],
        [560.0, 150.0],
        [300.0, 190.0],
        [60.0, 150.0],
        [300.0, 20.0],
    ];
    let cub = [[0.0, 0.0], [0.0, 200.0], [300.0, -100.0], [300.0, 100.0]];
    let pairs = [
        (
            MotionPath::through(&pts, 1.3).unwrap(),
            MotionPath::through3(&flat(&pts), 1.3).unwrap(),
        ),
        (
            MotionPath::through(&loop_pts, 1.0).unwrap(),
            MotionPath::through3(&flat(&loop_pts), 1.0).unwrap(),
        ),
        (
            MotionPath::polyline(&pts, true).unwrap(),
            MotionPath::polyline3(&flat(&pts), true).unwrap(),
        ),
        (
            MotionPath::cubic_points(&cub).unwrap(),
            MotionPath::cubic_points3(&flat(&cub)).unwrap(),
        ),
        (
            {
                let mut b = MotionPath::builder();
                b.move_to(1.0, 2.0)
                    .line_to(40.0, 7.0)
                    .quad_to(60.0, 90.0, 80.0, 10.0)
                    .cubic_to(90.0, -20.0, 130.0, 50.0, 150.0, 0.0);
                b.finish().unwrap()
            },
            {
                let mut b = MotionPath::builder();
                b.move_to3(1.0, 2.0, 0.0)
                    .line_to3(40.0, 7.0, 0.0)
                    .quad_to3(60.0, 90.0, 0.0, 80.0, 10.0, 0.0)
                    .cubic_to3([90.0, -20.0, 0.0], [130.0, 50.0, 0.0], [150.0, 0.0, 0.0]);
                b.finish().unwrap()
            },
        ),
    ];
    for (i, (a, b)) in pairs.iter().enumerate() {
        assert_eq!(a.length().to_bits(), b.length().to_bits(), "pair {i}");
        assert_eq!(a.span_count(), b.span_count(), "pair {i}");
        assert_eq!(a.bounds(), b.bounds(), "pair {i}");
        for k in 0..=97 {
            let u = k as f64 / 97.0 * 1.3 - 0.1;
            assert_eq!(a.sample(u), b.sample(u), "pair {i} u {u}");
        }
    }
}

#[test]
fn a_lap_adds_whole_turns_also_across_a_corner_seam() {
    let square = svg("M0,0 L10,0 L10,10 L0,10 Z");
    close(square.total_turn(), 270.0, 1e-9, "square turn");
    close(square.lap_turn(), 360.0, 1e-9, "square lap");
    close(
        square.sample(1.125).angle_deg,
        360.0,
        1e-9,
        "second lap, first edge",
    );
    close(square.sample(-0.875).angle_deg, -360.0, 1e-9, "lap before");
    close(circle().lap_turn(), 360.0, 1e-9, "circle lap");
    let s = svg("M20,160 C120,-20 220,220 320,100 S520,0 580,160 A60,60 0 0,1 460,160 L400,120 Q340,60 300,140 T160,160 Z");
    let lap = s.lap_turn();
    close(
        lap - 360.0 * (lap / 360.0).round(),
        0.0,
        1e-9,
        "svg lap is whole turns",
    );
    // The seam corner shows as a step between u = 1 and just past it.
    let (a, b) = (s.sample(1.0), s.sample(1.0 + 1e-12));
    let corner = b.angle_deg - a.angle_deg;
    let want = s.sample(0.0).angle_deg + lap - a.angle_deg;
    close(corner, want, 1e-6, "the corner at the seam");
}

#[test]
fn an_arc_scaled_to_its_chord_has_its_centre_on_it() {
    // Radii too small for the chord: the half-arc join is exact, not off by
    // the 1e-8 roundoff of the unscaled centre formula.
    let p = svg("M0,0 a1 1 0 00 2 2");
    assert_eq!(p.segment_count(), 2);
    let j = p.segment(0).end();
    close(j[0], 0.0, 1e-12, "join x");
    close(j[1], 2.0, 1e-12, "join y");
}

// ---------------------------------------------------------------------------
// Path tweens
// ---------------------------------------------------------------------------

fn wave() -> MotionPath {
    MotionPath::through(
        &[[0.0, 0.0], [100.0, -40.0], [200.0, 40.0], [300.0, 0.0]],
        1.0,
    )
    .unwrap()
}

#[test]
fn path_tween_lands_exactly_at_both_ends() {
    let mut e = TweenEngine::new();
    let g = wave();
    let p = e.add_path(g.clone());
    let opts = PathOpts::new().auto_rotate(R, 90.0);
    let id = e.to(
        one(0),
        &[PropTo::path(X, Y, p, opts)],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::InOutSine)
            .keep(true),
    );
    e.release_path(p);
    for _ in 0..70 {
        e.advance(1.0 / 60.0);
    }
    let end = g.sample_span(0.0, 1.0, 1.0);
    assert_eq!(val(&e, 0, X).to_bits(), end.pos[0].to_bits());
    assert_eq!(val(&e, 0, Y).to_bits(), end.pos[1].to_bits());
    assert_eq!(val(&e, 0, R).to_bits(), (end.angle_deg + 90.0).to_bits());
    assert_eq!(val(&e, 0, X), 300.0);
    e.anim(id).seek(Seek::Time(0.0), Emit::Suppress);
    let start = g.sample_span(0.0, 1.0, 0.0);
    assert_eq!(val(&e, 0, X).to_bits(), start.pos[0].to_bits());
    assert_eq!(val(&e, 0, Y).to_bits(), start.pos[1].to_bits());
    assert_eq!(val(&e, 0, R).to_bits(), (start.angle_deg + 90.0).to_bits());
    // No z key: no z slot.
    assert!(e.slot(tg(0), Z).is_none());
    // The path is held by the kept tween.
    assert_eq!(e.path_count(), 1);
    assert!(e.path(p).is_some());
}

#[test]
fn overshooting_ease_stays_on_the_path() {
    let mut e = TweenEngine::new();
    let g = wave();
    let p = e.add_path(g.clone());
    let id = e.to(
        one(0),
        &[PropTo::path(X, Y, p, PathOpts::new().span(0.1, 0.9))],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::OutBack)
            .keep(true),
    );
    e.release_path(p);
    let end = g.sample_span(0.1, 0.9, 1.0);
    let mut overshot = 0;
    for _ in 0..64 {
        e.advance(1.0 / 60.0);
        let prog = e.anim_ref(id).progress();
        let eased = Easing::OutBack.map(prog);
        let want = g.sample_span(0.1, 0.9, eased);
        close(val(&e, 0, X), want.pos[0], 1e-9, "x on the path");
        close(val(&e, 0, Y), want.pos[1], 1e-9, "y on the path");
        if eased >= 1.0 {
            overshot += 1;
            // The plateau is the end, bitwise.
            assert_eq!(val(&e, 0, X).to_bits(), end.pos[0].to_bits());
            assert_eq!(val(&e, 0, Y).to_bits(), end.pos[1].to_bits());
        }
    }
    assert!(overshot > 5, "{overshot}");
}

#[test]
fn align_start_moves_the_path_to_the_target() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, 500.0.into());
    e.seed(tg(0), Y, 300.0.into());
    let g = wave();
    let p = e.add_path(g.clone());
    let opts = PathOpts::new().align(PathAlign::Start);
    let id = e.to(
        Targets::List(&[tg(0), tg(1)]),
        &[PropTo::path(X, Y, p, opts)],
        lin(1.0).keep(true),
    );
    e.release_path(p);
    e.advance(0.5);
    let mid = g.sample(0.5);
    close(val(&e, 0, X), 500.0 + mid.pos[0], 1e-9, "mid x");
    close(val(&e, 0, Y), 300.0 + mid.pos[1], 1e-9, "mid y");
    // Target 1 was never seeded: it does not align (and says so).
    assert_eq!(e.stats().unseeded_starts, 2);
    close(val(&e, 1, X), mid.pos[0], 1e-9, "unaligned x");
    e.advance(1.0);
    close(val(&e, 0, X), 800.0, 1e-9, "end x");
    close(val(&e, 0, Y), 300.0, 1e-9, "end y");
    assert_eq!(val(&e, 1, X), 300.0);
    e.anim(id).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(val(&e, 0, X), 500.0);
    assert_eq!(val(&e, 0, Y), 300.0);
}

#[test]
fn offset_and_radians() {
    let mut e = TweenEngine::new();
    let g = wave();
    let p = e.add_path(g.clone());
    let opts = PathOpts::new()
        .offset(10.0, -5.0)
        .auto_rotate_radians(R, 90.0)
        .span(0.0, 0.5);
    e.to(one(0), &[PropTo::path(X, Y, p, opts)], lin(1.0));
    e.release_path(p);
    e.advance(0.5);
    let q = g.sample_span(0.0, 0.5, 0.5);
    close(val(&e, 0, X), q.pos[0] + 10.0, 1e-9, "x");
    close(val(&e, 0, Y), q.pos[1] - 5.0, 1e-9, "y");
    close(
        val(&e, 0, R),
        (q.angle_deg + 90.0).to_radians(),
        1e-12,
        "radians",
    );
    e.advance(1.0);
    let z = g.sample_span(0.0, 0.5, 1.0);
    close(val(&e, 0, X), z.pos[0] + 10.0, 0.0, "end x");
    close(
        val(&e, 0, R),
        (z.angle_deg + 90.0).to_radians(),
        1e-12,
        "end radians",
    );
    // Tween and path are gone.
    assert_eq!((e.path_count(), e.stats().path_binds), (0, 0));
}

#[test]
fn overwrite_auto_on_x_leaves_y_on_the_path() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, 0.0.into());
    let g = wave();
    let p = e.add_path(g.clone());
    let id = e.to(
        one(0),
        &[PropTo::path(X, Y, p, PathOpts::new().auto_rotate(R, 0.0))],
        lin(1.0).keep(true),
    );
    e.release_path(p);
    e.advance(0.25);
    e.to(one(0), &[to(X, -50.0)], lin(0.5).overwrite(Overwrite::Auto));
    e.advance(0.25);
    let q = g.sample(0.5);
    close(val(&e, 0, Y), q.pos[1], 1e-9, "y stays on the path");
    close(
        val(&e, 0, R),
        q.angle_deg,
        1e-9,
        "rotation stays on the path",
    );
    assert!(
        val(&e, 0, X) < q.pos[0] - 20.0,
        "x left the path: {}",
        val(&e, 0, X)
    );
    e.advance(1.0);
    assert_eq!(val(&e, 0, X), -50.0);
    assert_eq!(val(&e, 0, Y), 0.0);
    assert!(e.anim_ref(id).is_alive());
    assert_eq!(e.stats().path_binds, 1);
}

#[test]
fn stagger_binds_each_target() {
    let mut e = TweenEngine::new();
    for i in 0..5 {
        e.seed(tg(i), X, (100.0 * i as f64).into());
        e.seed(tg(i), Y, (10.0 * i as f64).into());
    }
    let p = e.add_path(wave());
    e.to(
        Targets::Range { first: 0, count: 5 },
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().align(PathAlign::Start),
        )],
        lin(0.5).stagger(Stagger::each(0.1)),
    );
    e.release_path(p);
    assert_eq!(e.stats().path_binds, 5);
    assert_eq!(e.path_count(), 1);
    for _ in 0..60 {
        e.advance(1.0 / 60.0);
    }
    for i in 0..5 {
        close(val(&e, i, X), 100.0 * i as f64 + 300.0, 1e-9, "x");
        close(val(&e, i, Y), 10.0 * i as f64, 1e-9, "y");
    }
    assert_eq!((e.stats().path_binds, e.path_count()), (0, 0));
}

#[test]
fn percent_keyframes_carry_a_path_on_the_group() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), K, 0.0.into());
    let g = wave();
    let p = e.add_path(g.clone());
    let keys = [
        Key {
            at: 50.0,
            value: 10.0.into(),
            ease: None,
        },
        Key {
            at: 100.0,
            value: 0.0.into(),
            ease: None,
        },
    ];
    e.tween(
        one(0),
        &[
            PropTo::keys(K, &keys),
            PropTo::path(X, Y, p, PathOpts::new()),
        ],
        TweenOpts::new().duration(1.0),
    );
    e.release_path(p);
    assert_eq!(e.stats().path_binds, 1);
    e.advance(0.25);
    // The group's outer ease (linear by default) eases the path.
    let q = g.sample(0.25);
    close(val(&e, 0, X), q.pos[0], 1e-9, "x at 0.25");
    close(val(&e, 0, Y), q.pos[1], 1e-9, "y at 0.25");
    e.advance(1.0);
    assert_eq!(val(&e, 0, X), 300.0);
    assert_eq!(val(&e, 0, K), 0.0);
    assert_eq!(e.path_count(), 0);
}

#[test]
fn sample_matches_render_for_path_tracks() {
    let mut e = TweenEngine::new();
    let p = e.add_path(wave());
    let id = e.to(
        one(0),
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().auto_rotate(R, 45.0).offset(3.0, 4.0),
        )],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::OutQuad)
            .keep(true),
    );
    e.release_path(p);
    e.advance(0.01);
    let slots = [X, Y, R].map(|k| e.slot(tg(0), k).unwrap());
    for t in [0.25, 0.5, 0.8, 1.0] {
        let want: Vec<_> = slots.iter().map(|s| e.sample(id, *s, t).unwrap()).collect();
        e.anim(id).seek(Seek::Time(t), Emit::Suppress);
        for (k, s) in slots.iter().enumerate() {
            assert_eq!(e.value(*s), want[k], "t {t} lane {k}");
        }
    }
}

#[test]
fn path_lifecycle() {
    let mut e = TweenEngine::new();
    let g = wave();
    let p = e.add_path(g.clone());
    assert_eq!(e.path_count(), 1);
    assert_eq!(e.path(p).unwrap().length(), g.length());
    e.to(one(0), &[PropTo::path(X, Y, p, PathOpts::new())], lin(0.5));
    e.release_path(p);
    e.release_path(p); // a second release is a no-op
    assert_eq!(e.path_count(), 1);
    e.advance(0.25);
    assert!(e.path(p).is_some());
    e.advance(0.5); // completes and is reaped: the bind and the path go
    assert_eq!(e.path_count(), 0);
    assert!(e.path(p).is_none());
    assert_eq!(e.stats().paths, 0);
    // The slot is reused; the old handle stays stale.
    let q = e.add_path(svg("M0,0 L10,0"));
    assert_ne!(p, q);
    assert!(e.path(p).is_none());
    assert_eq!(e.path(q).unwrap().length(), 10.0);
    // A stale handle builds no track.
    let tracks = e.stats().tracks;
    e.to(one(1), &[PropTo::path(X, Y, p, PathOpts::new())], lin(0.5));
    assert_eq!(e.stats().bad_paths, 1);
    assert_eq!(e.stats().tracks, tracks);
    assert!(e.slot(tg(1), X).is_none());
    // An unused path is freed by its release at once.
    e.release_path(q);
    assert_eq!(e.path_count(), 0);
    assert!(e.path(q).is_none());
    assert!(PathId::NONE.is_none() && PathId::default().is_none());
    assert!(e.path(PathId::NONE).is_none());
    // Killing a kept tween frees its path.
    let r = e.add_path(wave());
    let id = e.to(
        one(2),
        &[PropTo::path(X, Y, r, PathOpts::new())],
        lin(1.0).keep(true),
    );
    e.release_path(r);
    e.advance(2.0);
    assert_eq!(e.path_count(), 1);
    e.anim(id).kill();
    assert_eq!(e.path_count(), 0);
    // forget_target kills the tween and frees the path too.
    let r = e.add_path(wave());
    e.to(one(3), &[PropTo::path(X, Y, r, PathOpts::new())], lin(1.0));
    e.release_path(r);
    e.advance(0.1);
    e.forget_target(tg(3));
    assert_eq!((e.path_count(), e.stats().path_binds), (0, 0));
}

#[test]
fn repeat_refresh_realigns_each_iteration() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, 0.0.into());
    e.seed(tg(0), Y, 0.0.into());
    let p = e.add_path(wave());
    e.to(
        one(0),
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().align(PathAlign::Start),
        )],
        lin(1.0).repeat(2).repeat_refresh(true),
    );
    e.release_path(p);
    e.advance(0.5);
    e.advance(1.0);
    close(
        val(&e, 0, X),
        300.0 + 150.0,
        1e-6,
        "second lap starts where the first ended",
    );
    for _ in 0..120 {
        e.advance(1.0 / 60.0);
    }
    close(val(&e, 0, X), 900.0, 1e-9, "three laps");
    close(val(&e, 0, Y), 0.0, 1e-9, "y");
}

#[test]
fn a_3d_path_writes_z_and_snap_rounds_the_positions_not_the_rotation() {
    let mut e = TweenEngine::new();
    let p =
        e.add_path(MotionPath::polyline3(&[[0.0, 0.0, 0.0], [10.0, 20.0, 30.0]], false).unwrap());
    e.to(
        one(0),
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().z(Z).offset3(0.0, 0.0, 1.0),
        )],
        lin(1.0),
    );
    e.to(
        one(1),
        &[PropTo::path(X, Y, p, PathOpts::new().z(Z).auto_rotate(R, 0.3)).snap(1.0)],
        lin(1.0),
    );
    e.release_path(p);
    e.advance(0.5);
    close(val(&e, 0, X), 5.0, 1e-12, "x");
    close(val(&e, 0, Y), 10.0, 1e-12, "y");
    close(val(&e, 0, Z), 16.0, 1e-12, "z");
    e.advance(0.21);
    assert_eq!(val(&e, 1, X), 7.0);
    assert_eq!(val(&e, 1, Y), 14.0);
    assert_eq!(val(&e, 1, Z), 21.0);
    // The rotation is not snapped: atan2(20, 10) = 63.43 deg, + 0.3.
    close(
        val(&e, 1, R),
        20f64.atan2(10.0).to_degrees() + 0.3,
        1e-9,
        "rotation",
    );
    e.advance(1.0);
    assert_eq!(val(&e, 0, Z), 31.0);
    assert_eq!(e.path_count(), 0);
}

#[test]
fn a_bind_goes_with_its_last_live_track() {
    let mut e = TweenEngine::new();
    for i in 0..2 {
        e.seed(tg(i), K, 0.0.into());
    }
    let p = e.add_path(wave());
    let id = e.to(
        Targets::List(&[tg(0), tg(1)]),
        &[
            PropTo::path(X, Y, p, PathOpts::new().auto_rotate(R, 0.0)),
            to(K, 1.0),
        ],
        lin(1.0).keep(true),
    );
    e.release_path(p);
    e.advance(0.25);
    assert_eq!((e.stats().path_binds, e.path_count()), (2, 1));
    // x and y of target 0 die: its rotation still follows the path.
    e.kill_tweens_of(one(0), Some(&[X, Y]));
    assert_eq!(e.stats().path_binds, 2);
    e.kill_tweens_of(one(0), Some(&[R]));
    assert_eq!(e.stats().path_binds, 1);
    // Target 1's lanes go: the path goes with them, the tween (K) stays.
    e.kill_tweens_of(one(1), Some(&[X, Y, R]));
    assert_eq!((e.stats().path_binds, e.path_count()), (0, 0));
    assert!(e.path(p).is_none());
    assert!(e.anim_ref(id).is_alive());
    e.advance(1.0);
    assert_eq!(val(&e, 1, K), 1.0);
    // Killing the node afterwards releases nothing twice.
    e.anim(id).kill();
    assert_eq!((e.stats().path_binds, e.path_count()), (0, 0));
    // The freed bind slot is reused by the next path tween.
    let q = e.add_path(wave());
    e.to(one(2), &[PropTo::path(X, Y, q, PathOpts::new())], lin(0.5));
    e.release_path(q);
    assert_eq!((e.stats().path_binds, e.path_count()), (1, 1));
    e.advance(1.0);
    assert_eq!((e.stats().path_binds, e.path_count()), (0, 0));
}

#[test]
fn set_jumps_to_the_path_end() {
    let mut e = TweenEngine::new();
    let p = e.add_path(wave());
    e.set(
        one(0),
        &[PropTo::path(X, Y, p, PathOpts::new().span(0.0, 0.5))],
        TweenOpts::new(),
    );
    e.release_path(p);
    let q = wave().sample(0.5);
    assert_eq!(val(&e, 0, X), q.pos[0]);
    assert_eq!(val(&e, 0, Y), q.pos[1]);
    assert_eq!(e.path_count(), 0);
}
