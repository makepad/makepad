//! Review regression tests for motion paths (libs/tween/src/path.rs and the
//! engine's path tracks): the arc-length table against a brute-force dense
//! polyline, parser edge cases, bounds, spans and wraps, `through()` edge
//! cases, the path lifecycle, and allocation AND deallocation during frames
//! in which paths die.
//!
//! Four of them reproduced defects the review found at 2e647ecf (fixed
//! since): later laps of a closed path with a corner at its seam
//! (`later_laps_keep_the_heading`, `auto_rotate_follows_the_travel_direction_after_the_seam`),
//! and `through()` on an out-and-back and on a seam duplicate.

mod util;

use makepad_tween::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use util::*;

// ---------------------------------------------------------------------------
// A counting allocator (thread-local, so parallel tests do not interfere):
// allocations AND deallocations, since the crate docs promise that a path
// freed inside a frame is not deallocated there either.
// ---------------------------------------------------------------------------

struct Counting;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: every call forwards to the system allocator unchanged; the only
// additions are thread-local counter increments, which do not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = FREES.try_with(|c| c.set(c.get() + 1));
        System.dealloc(p, l)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn heap_ops() -> (usize, usize) {
    (ALLOCS.with(|c| c.get()), FREES.with(|c| c.get()))
}

const R: PropKey = PropKey(10);

fn svg(d: &str) -> MotionPath {
    MotionPath::from_svg(d).unwrap_or_else(|e| panic!("{d}: {e}"))
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|j| (a[j] - b[j]).powi(2)).sum::<f64>().sqrt()
}

/// Brute force: every segment as `per_seg` chords, with the cumulative
/// length (a subpath jump adds nothing).
fn dense(p: &MotionPath, per_seg: u32) -> (Vec<[f64; 3]>, Vec<f64>) {
    let mut pts: Vec<[f64; 3]> = Vec::new();
    let mut cum = Vec::new();
    let mut s = 0.0;
    for i in 0..p.segment_count() {
        let seg = p.segment(i);
        for k in 0..=per_seg {
            let q = seg.point(k as f64 / per_seg as f64);
            if k > 0 {
                s += dist(q, *pts.last().unwrap());
            }
            pts.push(q);
            cum.push(s);
        }
    }
    (pts, cum)
}

/// The brute-force point at arc length `s`.
fn brute_at(pts: &[[f64; 3]], cum: &[f64], s: f64) -> [f64; 3] {
    let i = cum.partition_point(|&c| c < s).clamp(1, cum.len() - 1);
    let (c0, c1) = (cum[i - 1], cum[i]);
    let f = if c1 > c0 { (s - c0) / (c1 - c0) } else { 0.0 };
    let (a, b) = (pts[i - 1], pts[i]);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

fn helix() -> MotionPath {
    let tau = std::f64::consts::TAU;
    let mut b = MotionPath::builder();
    b.move_to3(1.0, 0.0, 0.0);
    let n = 32;
    for i in 0..3 * n {
        let (a0, a1) = (i as f64 / n as f64 * tau, (i + 1) as f64 / n as f64 * tau);
        let k = 4.0 / 3.0 * ((a1 - a0) / 4.0).tan();
        let (z0, z1) = (a0 / tau, a1 / tau);
        let dz = (z1 - z0) / 3.0;
        b.cubic_to3(
            [a0.cos() - k * a0.sin(), a0.sin() + k * a0.cos(), z0 + dz],
            [a1.cos() + k * a1.sin(), a1.sin() - k * a1.cos(), z1 - dz],
            [a1.cos(), a1.sin(), z1],
        );
    }
    b.finish().unwrap()
}

const STORY_SVG: &str = "M20,160 C120,-20 220,220 320,100 S520,0 580,160 A60,60 0 0,1 460,160 L400,120 Q340,60 300,140 T160,160 Z";
const STORY_LOOP: [[f64; 2]; 7] = [
    [300.0, 20.0],
    [520.0, 60.0],
    [560.0, 150.0],
    [300.0, 190.0],
    [60.0, 150.0],
    [80.0, 50.0],
    [300.0, 20.0],
];

// ---------------------------------------------------------------------------
// Path maths
// ---------------------------------------------------------------------------

/// `sample(u)` against a 200k-chords-per-segment polyline: the length to
/// 1e-9 relative and every point to 1e-7 of the length.
#[test]
fn arc_length_matches_a_brute_force_polyline() {
    let cases: Vec<(&str, MotionPath)> = vec![
        ("cubic", svg("M0,0 C0,200 300,-100 300,100")),
        ("quad", svg("M0,0 Q150,300 300,0")),
        ("rotated elliptic arc", svg("M0,0 A50,30 20 1,1 100,0")),
        (
            "circle of four arcs",
            svg("M100,50 A50,50 0 0,1 50,100 A50,50 0 0,1 0,50 A50,50 0 0,1 50,0 A50,50 0 0,1 100,50 Z"),
        ),
        // Cusp at t = 1/3 (not on a table boundary): 4A + 4B + C = 0.
        ("interior cusp", svg("M0,0 C100,100 -50,100 150,-300")),
        ("cusp at t = 1/2", svg("M0,0 C100,100 0,100 100,0")),
        ("helix", helix()),
        ("storybook svg", svg(STORY_SVG)),
        ("two subpaths", svg("M0,0 C0,100 100,100 100,0 M200,0 Q250,80 300,0")),
    ];
    for (name, p) in cases {
        let (pts, cum) = dense(&p, 200_000);
        let bl = *cum.last().unwrap();
        assert!(
            ((p.length() - bl) / bl).abs() < 1e-9,
            "{name}: length {} brute {bl}",
            p.length()
        );
        let mut worst: f64 = 0.0;
        for k in 0..=997 {
            let u = k as f64 / 997.0;
            worst = worst.max(dist(p.sample(u).pos, brute_at(&pts, &cum, u * bl)));
        }
        assert!(worst < 1e-7 * bl, "{name}: worst point error {worst:e}");
        // Exact ends.
        assert_eq!(p.sample(0.0).pos, p.start_point(), "{name}");
        if !p.is_closed() {
            assert_eq!(p.sample(1.0).pos, p.end_point(), "{name}");
        }
        assert_eq!(p.sample_at_length(p.length()).pos, p.end_point(), "{name}");
    }
}

/// A circle of four 90-degree arcs: the quarter points are the joins, so
/// they and their headings are exact; the length is the cubic
/// approximation's (1.4e-4 relative above 2 pi r, the documented deviation).
#[test]
fn a_circle_of_four_arcs() {
    let c = svg(
        "M100,50 A50,50 0 0,1 50,100 A50,50 0 0,1 0,50 A50,50 0 0,1 50,0 A50,50 0 0,1 100,50 Z",
    );
    assert!(c.is_closed());
    let rel = c.length() / (std::f64::consts::TAU * 50.0) - 1.0;
    assert!(rel > 0.0 && rel < 2e-4, "{rel:e}");
    let want = [[100.0, 50.0], [50.0, 100.0], [0.0, 50.0], [50.0, 0.0]];
    let a0 = c.sample(0.0).angle_deg;
    for (k, w) in want.iter().enumerate() {
        let p = c.sample(k as f64 * 0.25);
        close(p.pos[0], w[0], 1e-9, "x");
        close(p.pos[1], w[1], 1e-9, "y");
        close(p.angle_deg, a0 + 90.0 * k as f64, 1e-9, "angle");
    }
    close(c.total_turn(), 360.0, 1e-9, "turn");
    // Bounds are the circle's.
    let b = c.bounds();
    for (got, want) in [
        (b.min[0], 0.0),
        (b.min[1], 0.0),
        (b.max[0], 100.0),
        (b.max[1], 100.0),
    ] {
        close(got, want, 1e-9, "bounds");
    }
}

#[test]
fn parser_edge_cases() {
    let seg = |p: &MotionPath, i: u32| p.segment(i);
    // Relative after Z: from the subpath start.
    let p = svg("M0,0 L10,0 Z l5,5");
    assert_eq!(p.subpath_count(), 2);
    assert_eq!(seg(&p, 2).p[0], [0.0, 0.0, 0.0]);
    assert_eq!(seg(&p, 2).end(), [5.0, 5.0, 0.0]);
    // Implicit repeats: M's extra pairs are lines (relative for m).
    let p = svg("M0,0 L 1 2 3 4");
    assert_eq!(p.segment_count(), 2);
    assert_eq!(seg(&p, 1).end(), [3.0, 4.0, 0.0]);
    let p = svg("m1,1 2,2");
    assert_eq!(seg(&p, 0).end(), [3.0, 3.0, 0.0]);
    // A second move before any segment only moves the start.
    let p = svg("M0 0 m10 0 l0 10");
    assert_eq!((p.subpath_count(), seg(&p, 0).p[0]), (1, [10.0, 0.0, 0.0]));
    // S reflects a C's c2; S after Q uses the current point; T chains.
    let p = svg("M0,0 C0,10 10,10 10,0 S20,-10 20,0");
    assert_eq!(seg(&p, 1).p[1], [10.0, -10.0, 0.0]);
    let p = svg("M0,0 Q10,10 20,0 S30,10 40,0");
    assert_eq!(seg(&p, 1).p[1], [20.0, 0.0, 0.0]);
    let p = svg("M0,0 Q10,10 20,0 T40,0 T60,0");
    assert_eq!(seg(&p, 1).p[1], [30.0, -10.0, 0.0]);
    assert_eq!(seg(&p, 2).p[1], [50.0, 10.0, 0.0]);
    let p = svg("M0,0 C0,10 10,10 10,0 T30,0");
    assert_eq!(seg(&p, 1).p[1], [10.0, 0.0, 0.0]);
    // Numbers: exponents, signs as separators, "1." and ".5".
    let p = svg("M1e1,1E-1 L2e+1,0");
    assert_eq!(p.start_point(), [10.0, 0.1, 0.0]);
    let p = svg("M1-2L3-4");
    assert_eq!(
        (p.start_point(), p.end_point()),
        ([1.0, -2.0, 0.0], [3.0, -4.0, 0.0])
    );
    let p = svg("M.5.5L-.5-.5");
    assert_eq!(p.end_point(), [-0.5, -0.5, 0.0]);
    let p = svg("M0,0 L1.e1,0");
    assert_eq!(p.end_point(), [10.0, 0.0, 0.0]);
    // Arc flags without separators.
    let p = svg("M0,0 a1 1 0 00 2 2");
    assert_eq!(p.end_point(), [2.0, 2.0, 0.0]);
    // A trailing comma, a doubled Z.
    assert_eq!(svg("M 0,0 L 10,0,").length(), 10.0);
    assert_eq!(svg("M0,0 L10,0 Z Z L5,5").subpath_count(), 2);
    // Errors carry the offending byte.
    for (d, e) in [
        (
            "M0,0 L1,1 x",
            PathError::UnknownCommand { at: 10, cmd: b'x' },
        ),
        ("M0,0 LNaN,0", PathError::Syntax { at: 6 }),
        ("M0,0 L1e,0", PathError::Syntax { at: 6 }),
        ("M0,0 L1e-,0", PathError::Syntax { at: 6 }),
        ("M0,0 L+-1,0", PathError::Syntax { at: 6 }),
        ("M0,0 A1,1 0 2 0 2,2", PathError::Syntax { at: 12 }),
        ("M0,0 L10,0 10", PathError::Syntax { at: 13 }),
        ("M0,0 L1,1 z 2,2", PathError::Syntax { at: 12 }),
        ("M5", PathError::Syntax { at: 2 }),
        ("M5,5", PathError::Empty),
        ("M0,0 Z", PathError::Empty),
        ("", PathError::Empty),
        (
            "M0,0\u{a0}L1,1",
            PathError::UnknownCommand { at: 4, cmd: 0xc2 },
        ),
    ] {
        assert_eq!(MotionPath::from_svg(d).err(), Some(e), "{d:?}");
    }
    // A zero-length segment in the middle is dropped.
    let p = svg("M0,0 L10,0 L10,0 L20,0");
    assert_eq!((p.segment_count(), p.length()), (2, 20.0));
    // The empty placeholder.
    let d = MotionPath::default();
    assert_eq!((d.length(), d.sample(0.5)), (0.0, PathPoint::default()));
}

/// Bounds contain every point of a dense sampling and are attained by it
/// (to the sampling's resolution).
#[test]
fn bounds_are_tight() {
    for d in [
        "M0,0 C0,100 100,100 100,0",
        STORY_SVG,
        "M0,0 Q50,-80 100,0",
        "M0,0 A50,30 20 1,1 100,0",
        "M1e6,1e6 C1000000.001,1000100 1000100,1000100 1000100,1000000",
    ] {
        let p = svg(d);
        let b = p.bounds();
        let (pts, _) = dense(&p, 100_000);
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for q in &pts {
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        for i in 0..3 {
            let tol = 1e-9 * (1.0 + b.max[i].abs());
            assert!(
                b.min[i] <= lo[i] + tol && lo[i] - b.min[i] < 1e-6,
                "{d}: min {i}"
            );
            assert!(
                b.max[i] >= hi[i] - tol && b.max[i] - hi[i] < 1e-6,
                "{d}: max {i}"
            );
        }
    }
}

#[test]
fn translate_moves_samples_and_keeps_the_table() {
    let mut p = svg("M0,0 C0,200 300,-100 300,100");
    let before: Vec<PathPoint> = (0..=16).map(|k| p.sample(k as f64 / 16.0)).collect();
    let (len, b0) = (p.length(), p.bounds());
    p.translate(1000.0, -50.0);
    assert_eq!(p.length(), len);
    close(p.bounds().min[0], b0.min[0] + 1000.0, 1e-9, "bounds");
    for (k, a) in before.iter().enumerate() {
        let q = p.sample(k as f64 / 16.0);
        close(q.pos[0], a.pos[0] + 1000.0, 1e-9, "x");
        close(q.pos[1], a.pos[1] - 50.0, 1e-9, "y");
        assert_eq!(q.angle_deg, a.angle_deg);
    }
}

/// A reversed span that crosses the seam of a smooth closed path: positions
/// and the unwrapped angle move continuously.
#[test]
fn a_reversed_span_across_the_seam_is_continuous() {
    let c = svg("M100,50 A50,50 0 1,1 0,50 A50,50 0 1,1 100,50 Z");
    let step = std::f64::consts::TAU * 50.0 * 0.5 / 400.0;
    let mut prev: Option<PathPoint> = None;
    for i in 0..=400 {
        let q = c.sample_span(1.25, 0.75, i as f64 / 400.0);
        if let Some(p) = prev {
            assert!(dist(q.pos, p.pos) < 1.01 * step, "position jump at {i}");
            assert!((q.angle_deg - p.angle_deg).abs() < 1.0, "angle jump at {i}");
        }
        prev = Some(q);
    }
}

/// The unwrapped angle stays continuous across subpath jumps.
#[test]
fn subpath_jumps_keep_the_angle_continuous() {
    let p = svg("M0,0 L10,0 M20,0 L20,-10 M30,0 L20,0");
    let mut prev = p.sample(0.0).angle_deg;
    for k in 1..=300 {
        let a = p.sample(k as f64 / 300.0).angle_deg;
        assert!((a - prev).abs() <= 90.0 + 1e-9, "{prev} -> {a}");
        prev = a;
    }
}

/// On a closed path the heading of a later lap is the first lap's heading
/// plus whole turns (`lap_turn()`: the total turn plus the corner at the
/// seam), also when the seam is a corner: a polygon, an SVG path closed
/// with Z, a curviness-0 loop.
#[test]
fn later_laps_keep_the_heading() {
    let square = svg("M0,0 L10,0 L10,10 L0,10 Z");
    let lap0 = square.sample(0.125);
    let lap1 = square.sample(1.125);
    assert_eq!(lap1.tangent, lap0.tangent); // the geometry is right
    close(
        lap1.angle_wrapped(),
        lap0.angle_wrapped(),
        1e-9,
        "square: heading on the second lap (the tangent points right)",
    );
    for (name, p) in [
        ("storybook svg (Z corner)", svg(STORY_SVG)),
        (
            "storybook loop at curviness 0",
            MotionPath::through(&STORY_LOOP, 0.0).unwrap(),
        ),
    ] {
        assert!(p.is_closed());
        for u in [1.1, 1.5, 2.3, -0.4] {
            let q = p.sample(u);
            let heading = q.tangent[1].atan2(q.tangent[0]).to_degrees();
            close(
                wrap(q.angle_deg - heading),
                0.0,
                1e-6,
                &format!("{name}: angle vs tangent at u {u}"),
            );
        }
    }
}

fn wrap(d: f64) -> f64 {
    let r = d - 360.0 * ((d + 180.0) / 360.0).floor();
    if r == -180.0 {
        180.0
    } else {
        r
    }
}

/// The same as the engine writes it: an auto-rotated follower on the
/// storybook's SVG shape with span 0.5 -> 1.5 points along its direction of
/// travel after the seam too.
#[test]
fn auto_rotate_follows_the_travel_direction_after_the_seam() {
    let mut e = TweenEngine::new();
    let g = svg(STORY_SVG);
    let p = e.add_path(g.clone());
    e.to(
        one(0),
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().span(0.5, 1.5).auto_rotate(R, 0.0),
        )],
        lin(1.0),
    );
    e.release_path(p);
    e.advance(0.75); // u = 1.25: past the seam
    let q = g.sample(1.25);
    let heading = q.tangent[1].atan2(q.tangent[0]).to_degrees();
    close(
        wrap(val(&e, 0, R) - heading),
        0.0,
        1e-6,
        "written rotation vs travel direction",
    );
}

// ---------------------------------------------------------------------------
// through()
// ---------------------------------------------------------------------------

#[test]
fn through_edge_cases() {
    // Two points: one straight cubic, exactly the chord long.
    let p = MotionPath::through(&[[0.0, 0.0], [10.0, 0.0]], 1.0).unwrap();
    close(p.length(), 10.0, 1e-12, "two points");
    // Negative curviness clamps to 0: lines.
    let p = MotionPath::through(&[[0.0, 0.0], [10.0, 5.0], [20.0, 0.0]], -1.0).unwrap();
    assert_eq!(p.segment(0).kind, SegKind::Line);
    // Only duplicates: nothing to draw; NaN: not finite.
    assert_eq!(
        MotionPath::through(&[[1.0, 1.0], [1.0, 1.0]], 1.0).err(),
        Some(PathError::Empty)
    );
    assert_eq!(
        MotionPath::through(&[[0.0, f64::NAN], [1.0, 1.0]], 1.0).err(),
        Some(PathError::NotFinite)
    );
    // A closed triangle turns once.
    let p = MotionPath::through(&[[0.0, 0.0], [10.0, 0.0], [5.0, 8.0], [0.0, 0.0]], 1.0).unwrap();
    assert!(p.is_closed());
    close(p.total_turn(), 360.0, 1e-9, "closed thru turn");
}

/// Two distinct points out and back (a ping-pong, which `polyline` also
/// accepts) are too few for a loop: an open path out and back to the start,
/// so the script's `motion_path: [[0, 0], [100, 0], [0, 0]]` draws too.
#[test]
fn through_out_and_back_is_an_open_path() {
    let pts = [[0.0, 0.0], [100.0, 0.0], [0.0, 0.0]];
    assert!(MotionPath::polyline(&pts, false).is_ok());
    let r = MotionPath::through(&pts, 1.0);
    assert!(r.is_ok(), "{r:?}");
    let p = r.unwrap();
    assert!(!p.is_closed());
    assert_eq!(p.segment_count(), 2);
    assert_eq!(p.end_point(), [0.0, 0.0, 0.0]);
    assert_eq!(MotionPath::through(&pts, 0.0).unwrap().length(), 200.0);
}

/// Finite points whose closing drop leaves a point equal to the first
/// across the seam: that duplicate is dropped too (no chord of length 0),
/// so the loop is the triangle, with any curviness.
#[test]
fn through_drops_a_seam_duplicate() {
    let pts = [
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 0.0],
        [0.0, 0.0005],
    ];
    for k in [0.0, 1.0] {
        let r = MotionPath::through(&pts, k);
        assert!(r.is_ok(), "{r:?}");
        let p = r.unwrap();
        assert!(p.is_closed());
        assert_eq!(p.segment_count(), 3);
        assert!(p.length().is_finite());
    }
    close(
        MotionPath::through(&pts, 0.0).unwrap().length(),
        20.0 + 200f64.sqrt(),
        1e-12,
        "triangle",
    );
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

fn wave() -> MotionPath {
    MotionPath::through(
        &[[0.0, 0.0], [100.0, -40.0], [200.0, 40.0], [300.0, 0.0]],
        1.0,
    )
    .unwrap()
}

/// `align: Start` reads the slots when the tween starts (its first render),
/// not when it is built.
#[test]
fn align_reads_the_slots_at_the_start_not_at_the_build() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, 1.0.into());
    e.seed(tg(0), Y, 2.0.into());
    let p = e.add_path(wave());
    e.to(
        one(0),
        &[PropTo::path(
            X,
            Y,
            p,
            PathOpts::new().align(PathAlign::Start),
        )],
        lin(1.0).delay(0.5),
    );
    e.release_path(p);
    e.seed(tg(0), X, 500.0.into());
    e.seed(tg(0), Y, 300.0.into());
    e.advance(0.5 + 1e-9);
    close(val(&e, 0, X), 500.0, 1e-6, "x at the start");
    close(val(&e, 0, Y), 300.0, 1e-6, "y at the start");
    e.advance(1.0);
    close(val(&e, 0, X), 800.0, 1e-9, "x at the end");
}

/// One path, two tweens: killing one mid-flight keeps it; the second going
/// frees it; its slot is reused and every old handle stays stale.
#[test]
fn a_shared_path_lives_until_its_last_tween_goes() {
    let mut e = TweenEngine::new();
    let p = e.add_path(wave());
    let a = e.to(one(0), &[PropTo::path(X, Y, p, PathOpts::new())], lin(1.0));
    let b = e.to(
        Targets::List(&[tg(1), tg(2)]),
        &[PropTo::path(X, Y, p, PathOpts::new().auto_rotate(R, 0.0))],
        lin(2.0),
    );
    e.release_path(p);
    assert_eq!((e.path_count(), e.stats().path_binds), (1, 3));
    e.advance(0.25);
    e.anim(a).kill();
    assert_eq!((e.path_count(), e.stats().path_binds), (1, 2));
    assert!(e.path(p).is_some());
    // Forgetting one target of b keeps the path.
    e.forget_target(tg(1));
    assert!(e.path(p).is_some());
    e.advance(0.25);
    close(
        val(&e, 2, X),
        wave().sample(0.25).pos[0],
        1e-9,
        "b still follows",
    );
    e.kill_tweens_of(one(2), None);
    assert!(!e.anim_ref(b).is_alive());
    assert_eq!((e.path_count(), e.stats().path_binds), (0, 0));
    assert!(e.path(p).is_none());
    let q = e.add_path(wave());
    assert!(e.path(p).is_none() && e.path(q).is_some());
    let bad = e.stats().bad_paths;
    e.to(one(3), &[PropTo::path(X, Y, p, PathOpts::new())], lin(1.0));
    assert_eq!(e.stats().bad_paths, bad + 1);
    // Released before the build: stale at once.
    e.release_path(q);
    e.to(one(4), &[PropTo::path(X, Y, q, PathOpts::new())], lin(1.0));
    assert_eq!(e.stats().bad_paths, bad + 2);
    assert!(e.slot(tg(4), X).is_none());
}

/// Paths that die inside `advance` (their last tween completes, or an
/// auto-overwrite kills it at another tween's first render) allocate
/// nothing and free nothing there; the next building call frees them.
#[test]
fn paths_dying_inside_advance_neither_allocate_nor_free() {
    let mut e = TweenEngine::new();
    for i in 0..8u32 {
        e.seed(tg(i), X, 0.0.into());
        e.seed(tg(i), Y, 0.0.into());
    }
    // Four completing path tweens, each on its own path.
    for i in 0..4u32 {
        let p = e.add_path(wave());
        e.to(
            one(i),
            &[PropTo::path(X, Y, p, PathOpts::new().auto_rotate(R, 0.0))],
            lin(0.5 + 0.1 * i as f64),
        );
        e.release_path(p);
    }
    // Four long path tweens overwritten (auto) by delayed tweens of X / Y / R.
    for i in 4..8u32 {
        let p = e.add_path(wave());
        e.to(
            one(i),
            &[PropTo::path(X, Y, p, PathOpts::new().auto_rotate(R, 0.0))],
            lin(10.0),
        );
        e.release_path(p);
        e.to(
            one(i),
            &[to(X, 1.0), to(Y, 1.0), to(R, 1.0)],
            lin(1.0)
                .delay(0.3 + 0.1 * i as f64)
                .overwrite(Overwrite::Auto),
        );
    }
    let mut buf: Vec<TweenEvent> = Vec::with_capacity(1024);
    e.advance(0.01);
    e.swap_events(&mut buf);
    assert_eq!(e.path_count(), 8);
    let before = heap_ops();
    for _ in 0..120 {
        e.advance(1.0 / 60.0);
        e.swap_events(&mut buf);
    }
    let after = heap_ops();
    assert_eq!(e.path_count(), 0, "every path died in the loop");
    assert_eq!(
        (after.0 - before.0, after.1 - before.1),
        (0, 0),
        "(allocations, deallocations) inside advance"
    );
    // The next building call drops the geometry.
    let before = heap_ops();
    let p = e.add_path(MotionPath::default());
    assert!(
        heap_ops().1 - before.1 >= 8,
        "the dead geometry is freed here"
    );
    e.release_path(p);
}
