//! Piecewise-linear collision skylines.
//!
//! A skyline is the silhouette of a set of shapes seen from above (upper) or
//! below (lower), stored as sorted, disjoint linear segments with gaps where
//! nothing exists. Vertical placement of staves, systems and outside-staff
//! objects is done entirely with skylines — no pixels, no GPU readback.
//!
//! With y growing downward, the *upper* skyline of a group is its minimum
//! ink y per x, and the *lower* skyline its maximum ink y per x. Stacking a
//! group B below a group A requires
//!
//! ```text
//! dy_min(A, B) = max_x(lower_A(x) - upper_B(x)) + clearance
//! ```
//!
//! computed by [`min_clearance`] as an interval sweep over the merged
//! breakpoints: within each elementary interval both envelopes are linear,
//! so the maximum lives at interval endpoints. Construction from `n` shapes
//! is a balanced divide-and-conquer of pairwise envelope merges —
//! O((n + m) log(n + m)) total, verified by an operation-counting test, not
//! the quadratic repeated-insertion that kills large scores.

use crate::sp::{Sp, SpPoint, SpRect};

/// Which silhouette a skyline represents. Y grows downward.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkySide {
    /// The top silhouette: minimum ink y per x. Smaller y dominates.
    Upper,
    /// The bottom silhouette: maximum ink y per x. Larger y dominates.
    Lower,
}

impl SkySide {
    /// True when `a` wins over `b` on this side (ties prefer `a`).
    fn dominates(self, a: f64, b: f64) -> bool {
        match self {
            SkySide::Upper => a <= b,
            SkySide::Lower => a >= b,
        }
    }
}

/// One linear piece of a skyline, from `(x0, y0)` to `(x1, y1)`, `x0 < x1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SkySeg {
    /// Left x.
    pub x0: Sp,
    /// Left y.
    pub y0: Sp,
    /// Right x.
    pub x1: Sp,
    /// Right y.
    pub y1: Sp,
}

impl SkySeg {
    fn y_at(&self, x: f64) -> f64 {
        let t = (x - self.x0.0) / (self.x1.0 - self.x0.0);
        self.y0.0 + t * (self.y1.0 - self.y0.0)
    }

    fn slope(&self) -> f64 {
        (self.y1.0 - self.y0.0) / (self.x1.0 - self.x0.0)
    }
}

/// A collision outline for skyline construction.
#[derive(Clone, PartialEq, Debug)]
pub enum CollisionShape {
    /// An axis-aligned rectangle.
    Rect(SpRect),
    /// A polygon given by its vertex loop (closed implicitly). Convexity is
    /// not required: the silhouette is the extremal envelope of its edges.
    Poly(Vec<SpPoint>),
}

/// A piecewise-linear silhouette: sorted, non-overlapping segments with
/// gaps allowed where the group has no ink.
#[derive(Clone, PartialEq, Debug)]
pub struct Skyline {
    /// Which extremal envelope this is.
    pub side: SkySide,
    segs: Vec<SkySeg>,
}

impl Skyline {
    /// A skyline with no ink at all.
    pub fn empty(side: SkySide) -> Skyline {
        Skyline { side, segs: Vec::new() }
    }

    /// The silhouette of one rectangle.
    pub fn from_rect(side: SkySide, rect: SpRect) -> Skyline {
        if rect.x1.0 <= rect.x0.0 {
            return Skyline::empty(side);
        }
        let y = match side {
            SkySide::Upper => rect.y0,
            SkySide::Lower => rect.y1,
        };
        Skyline { side, segs: vec![SkySeg { x0: rect.x0, y0: y, x1: rect.x1, y1: y }] }
    }

    /// The silhouette of one line segment (a beam edge, a bracket arm).
    /// Vertical segments have zero x-extent and contribute nothing.
    pub fn from_segment(side: SkySide, a: SpPoint, b: SpPoint) -> Skyline {
        let (p, q) = if a.x.0 <= b.x.0 { (a, b) } else { (b, a) };
        if q.x.0 - p.x.0 <= 0.0 {
            return Skyline::empty(side);
        }
        Skyline { side, segs: vec![SkySeg { x0: p.x, y0: p.y, x1: q.x, y1: q.y }] }
    }

    /// The silhouette of one shape.
    pub fn from_shape(side: SkySide, shape: &CollisionShape) -> Skyline {
        match shape {
            CollisionShape::Rect(r) => Skyline::from_rect(side, *r),
            CollisionShape::Poly(pts) => {
                if pts.len() < 2 {
                    return Skyline::empty(side);
                }
                let mut edges = Vec::with_capacity(pts.len());
                for i in 0..pts.len() {
                    let a = pts[i];
                    let b = pts[(i + 1) % pts.len()];
                    let s = Skyline::from_segment(side, a, b);
                    if !s.segs.is_empty() {
                        edges.push(s.segs);
                    }
                }
                let mut steps = 0;
                Skyline { side, segs: merge_many(side, edges, &mut steps) }
            }
        }
    }

    /// The combined silhouette of many shapes, built by balanced pairwise
    /// merging: O(N log N) in the total number of silhouette segments.
    pub fn from_shapes(side: SkySide, shapes: &[CollisionShape]) -> Skyline {
        Skyline::from_shapes_counted(side, shapes).0
    }

    /// [`Skyline::from_shapes`] returning the number of elementary sweep
    /// intervals processed — the complexity tests assert on it.
    pub(crate) fn from_shapes_counted(
        side: SkySide,
        shapes: &[CollisionShape],
    ) -> (Skyline, usize) {
        let mut steps = 0usize;
        let leaves: Vec<Vec<SkySeg>> = shapes
            .iter()
            .map(|s| Skyline::from_shape(side, s).segs)
            .filter(|s| !s.is_empty())
            .collect();
        let segs = merge_many(side, leaves, &mut steps);
        (Skyline { side, segs }, steps)
    }

    /// Union another skyline into this one (same side).
    pub fn add(&mut self, other: &Skyline) {
        assert_eq!(self.side, other.side, "cannot merge skylines of different sides");
        let mut steps = 0;
        self.segs = merge_pair(self.side, &self.segs, &other.segs, &mut steps);
    }

    /// The union of two same-side skylines.
    pub fn merge(&self, other: &Skyline) -> Skyline {
        let mut out = self.clone();
        out.add(other);
        out
    }

    /// The silhouette height at `x`, or `None` over a gap. On the shared
    /// endpoint of two segments the dominant value wins.
    pub fn height_at(&self, x: Sp) -> Option<Sp> {
        let xv = x.0;
        // First segment whose right end reaches x.
        let idx = self.segs.partition_point(|s| s.x1.0 < xv);
        let mut best: Option<f64> = None;
        for seg in self.segs.iter().skip(idx).take(2) {
            if seg.x0.0 <= xv && xv <= seg.x1.0 {
                let y = seg.y_at(xv);
                best = Some(match best {
                    Some(b) if self.side.dominates(b, y) => b,
                    _ => y,
                });
            }
        }
        best.map(Sp)
    }

    /// The x extent covered (leftmost, rightmost), or `None` when empty.
    pub fn x_range(&self) -> Option<(Sp, Sp)> {
        Some((self.segs.first()?.x0, self.segs.last()?.x1))
    }

    /// The extremal y over the whole skyline (max y for a lower skyline,
    /// min y for an upper one), or `None` when empty.
    pub fn extreme_y(&self) -> Option<Sp> {
        let mut best: Option<f64> = None;
        for s in &self.segs {
            for y in [s.y0.0, s.y1.0] {
                best = Some(match best {
                    Some(b) if self.side.dominates(b, y) => b,
                    _ => y,
                });
            }
        }
        best.map(Sp)
    }

    /// The skyline translated by `(dx, dy)`.
    pub fn translated(&self, dx: Sp, dy: Sp) -> Skyline {
        Skyline {
            side: self.side,
            segs: self
                .segs
                .iter()
                .map(|s| SkySeg {
                    x0: s.x0 + dx,
                    y0: s.y0 + dy,
                    x1: s.x1 + dx,
                    y1: s.y1 + dy,
                })
                .collect(),
        }
    }

    /// Borrow the segments (sorted by x, non-overlapping).
    pub fn segments(&self) -> &[SkySeg] {
        &self.segs
    }
}

/// How far group B (described by its upper skyline) must sit below group A
/// (described by its lower skyline) so that they just touch:
/// `max_x(lower_a(x) - upper_b(x))`. Returns `None` when their x ranges do
/// not overlap (no constraint between them — they may interlock freely).
/// Add the style clearance and any padding to the result.
pub fn min_clearance(lower_a: &Skyline, upper_b: &Skyline) -> Option<Sp> {
    assert_eq!(lower_a.side, SkySide::Lower, "first argument must be a lower skyline");
    assert_eq!(upper_b.side, SkySide::Upper, "second argument must be an upper skyline");
    let mut steps = 0;
    min_clearance_counted(&lower_a.segs, &upper_b.segs, &mut steps)
}

pub(crate) fn min_clearance_counted(
    a: &[SkySeg],
    b: &[SkySeg],
    steps: &mut usize,
) -> Option<Sp> {
    let mut best: Option<f64> = None;
    sweep_intervals(a, b, steps, |fa, fb, xl, xr| {
        if let (Some(sa), Some(sb)) = (fa, fb) {
            for x in [xl, xr] {
                let d = sa.y_at(x) - sb.y_at(x);
                best = Some(match best {
                    Some(v) if v >= d => v,
                    _ => d,
                });
            }
        }
    });
    best.map(Sp)
}

/// Merge many segment lists by balanced pairwise rounds.
fn merge_many(side: SkySide, mut items: Vec<Vec<SkySeg>>, steps: &mut usize) -> Vec<SkySeg> {
    if items.is_empty() {
        return Vec::new();
    }
    while items.len() > 1 {
        let mut next = Vec::with_capacity((items.len() + 1) / 2);
        let mut it = items.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => next.push(merge_pair(side, &a, &b, steps)),
                None => next.push(a),
            }
        }
        items = next;
    }
    items.pop().unwrap()
}

/// Sweep the union of both lists' x-breakpoints; call `f` once per
/// elementary interval with each side's covering segment (if any).
fn sweep_intervals<'a>(
    a: &'a [SkySeg],
    b: &'a [SkySeg],
    steps: &mut usize,
    mut f: impl FnMut(Option<&'a SkySeg>, Option<&'a SkySeg>, f64, f64),
) {
    let mut xs: Vec<f64> = Vec::with_capacity(2 * (a.len() + b.len()));
    for s in a.iter().chain(b.iter()) {
        xs.push(s.x0.0);
        xs.push(s.x1.0);
    }
    xs.sort_by(|p, q| p.total_cmp(q));
    xs.dedup();
    let mut ia = 0usize;
    let mut ib = 0usize;
    for w in xs.windows(2) {
        let (xl, xr) = (w[0], w[1]);
        if xr <= xl {
            continue;
        }
        *steps += 1;
        while ia < a.len() && a[ia].x1.0 <= xl {
            ia += 1;
        }
        while ib < b.len() && b[ib].x1.0 <= xl {
            ib += 1;
        }
        // Every segment endpoint is a breakpoint, so coverage of an
        // elementary interval is all-or-nothing.
        let fa = a.get(ia).filter(|s| s.x0.0 <= xl && s.x1.0 >= xr);
        let fb = b.get(ib).filter(|s| s.x0.0 <= xl && s.x1.0 >= xr);
        f(fa, fb, xl, xr);
    }
}

/// Extremal envelope of two segment lists. O(n + m): one linear sweep;
/// within an elementary interval two linear functions cross at most once.
fn merge_pair(side: SkySide, a: &[SkySeg], b: &[SkySeg], steps: &mut usize) -> Vec<SkySeg> {
    let mut out: Vec<SkySeg> = Vec::with_capacity(a.len() + b.len());
    let mut push = |seg: SkySeg| {
        if seg.x1.0 - seg.x0.0 <= 0.0 {
            return;
        }
        if let Some(last) = out.last_mut() {
            if last.x1 == seg.x0
                && last.y1 == seg.y0
                && (last.slope() - seg.slope()).abs() <= 1e-12 * (1.0 + last.slope().abs())
            {
                last.x1 = seg.x1;
                last.y1 = seg.y1;
                return;
            }
        }
        out.push(seg);
    };
    sweep_intervals(a, b, steps, |fa, fb, xl, xr| {
        let piece = |s: &SkySeg| SkySeg {
            x0: Sp(xl),
            y0: Sp(s.y_at(xl)),
            x1: Sp(xr),
            y1: Sp(s.y_at(xr)),
        };
        match (fa, fb) {
            (None, None) => {}
            (Some(sa), None) => push(piece(sa)),
            (None, Some(sb)) => push(piece(sb)),
            (Some(sa), Some(sb)) => {
                let (al, ar) = (sa.y_at(xl), sa.y_at(xr));
                let (bl, br) = (sb.y_at(xl), sb.y_at(xr));
                let a_left = side.dominates(al, bl);
                let a_right = side.dominates(ar, br);
                if a_left == a_right {
                    push(piece(if a_left { sa } else { sb }));
                } else {
                    // Exactly one crossing inside the interval.
                    let dl = al - bl;
                    let dr = ar - br;
                    let t = dl / (dl - dr);
                    let xm = xl + t * (xr - xl);
                    let ym = al + t * (ar - al);
                    let (first, second) = if a_left { (sa, sb) } else { (sb, sa) };
                    push(SkySeg { x0: Sp(xl), y0: Sp(first.y_at(xl)), x1: Sp(xm), y1: Sp(ym) });
                    push(SkySeg { x0: Sp(xm), y0: Sp(ym), x1: Sp(xr), y1: Sp(second.y_at(xr)) });
                }
            }
        }
    });
    out
}

#[cfg(test)]
mod skyline_tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn rect_pair_hand_worked() {
        // Two overlapping rects: the lower skyline steps between their
        // bottom edges, taking the deeper one where they overlap.
        let a = SpRect::xywh(0.0, 0.0, 4.0, 2.0); // bottom at y = 2
        let b = SpRect::xywh(2.0, 0.0, 4.0, 3.0); // bottom at y = 3
        let sky = Skyline::from_shapes(
            SkySide::Lower,
            &[CollisionShape::Rect(a), CollisionShape::Rect(b)],
        );
        assert_eq!(sky.height_at(Sp(1.0)), Some(Sp(2.0)));
        assert_eq!(sky.height_at(Sp(3.0)), Some(Sp(3.0))); // b dominates in overlap
        assert_eq!(sky.height_at(Sp(5.0)), Some(Sp(3.0)));
        assert_eq!(sky.height_at(Sp(7.0)), None); // gap
        assert_eq!(sky.x_range(), Some((Sp(0.0), Sp(6.0))));
        assert_eq!(sky.extreme_y(), Some(Sp(3.0)));

        // Upper skyline of the same pair: both tops at y = 0.
        let sky = Skyline::from_shapes(
            SkySide::Upper,
            &[CollisionShape::Rect(a), CollisionShape::Rect(b)],
        );
        assert_eq!(sky.height_at(Sp(3.0)), Some(Sp(0.0)));
    }

    #[test]
    fn triangle_silhouette() {
        // Triangle (0,0) (4,0) (2,3): lower silhouette is the two sloped
        // edges peaking at (2,3); upper silhouette is the flat top edge.
        let tri = CollisionShape::Poly(vec![
            SpPoint::xy(0.0, 0.0),
            SpPoint::xy(4.0, 0.0),
            SpPoint::xy(2.0, 3.0),
        ]);
        let lower = Skyline::from_shape(SkySide::Lower, &tri);
        assert_eq!(lower.height_at(Sp(2.0)), Some(Sp(3.0)));
        assert!((lower.height_at(Sp(1.0)).unwrap().0 - 1.5).abs() < 1e-12);
        assert!((lower.height_at(Sp(3.0)).unwrap().0 - 1.5).abs() < 1e-12);
        let upper = Skyline::from_shape(SkySide::Upper, &tri);
        assert_eq!(upper.height_at(Sp(1.0)), Some(Sp(0.0)));
        assert_eq!(upper.height_at(Sp(3.0)), Some(Sp(0.0)));
    }

    #[test]
    fn min_clearance_hand_worked() {
        // A's lower reaches y = 2 over [0,4] and y = 5 over [4,6];
        // B's upper reaches y = 1 over [3,8].
        let a = Skyline::from_shapes(
            SkySide::Lower,
            &[
                CollisionShape::Rect(SpRect::xywh(0.0, 0.0, 4.0, 2.0)),
                CollisionShape::Rect(SpRect::xywh(4.0, 0.0, 2.0, 5.0)),
            ],
        );
        let b = Skyline::from_shapes(
            SkySide::Upper,
            &[CollisionShape::Rect(SpRect::xywh(3.0, 1.0, 5.0, 2.0))],
        );
        // Deepest requirement: A at y=5 against B at y=1 -> 4.
        assert_eq!(min_clearance(&a, &b), Some(Sp(4.0)));
        // Disjoint x ranges -> no constraint.
        let c = Skyline::from_shapes(
            SkySide::Upper,
            &[CollisionShape::Rect(SpRect::xywh(50.0, 0.0, 2.0, 2.0))],
        );
        assert_eq!(min_clearance(&a, &c), None);
    }

    fn random_shapes(rng: &mut Lcg, n: usize) -> Vec<CollisionShape> {
        (0..n)
            .map(|_| {
                let x = rng.next_f64() * 100.0;
                let y = rng.next_f64() * 20.0 - 10.0;
                if rng.next_f64() < 0.7 {
                    CollisionShape::Rect(SpRect::xywh(
                        x,
                        y,
                        0.2 + rng.next_f64() * 8.0,
                        0.2 + rng.next_f64() * 4.0,
                    ))
                } else {
                    let w = 0.5 + rng.next_f64() * 6.0;
                    let h = 0.5 + rng.next_f64() * 4.0;
                    CollisionShape::Poly(vec![
                        SpPoint::xy(x, y),
                        SpPoint::xy(x + w, y + 0.3 * h),
                        SpPoint::xy(x + 0.5 * w, y + h),
                    ])
                }
            })
            .collect()
    }

    /// Brute-force silhouette of a shape set at one x.
    fn brute_at(shapes: &[CollisionShape], side: SkySide, x: f64) -> Option<f64> {
        let mut best: Option<f64> = None;
        let mut consider = |y: f64| {
            best = Some(match best {
                Some(b) if side.dominates(b, y) => b,
                _ => y,
            });
        };
        for s in shapes {
            match s {
                CollisionShape::Rect(r) => {
                    if r.x0.0 <= x && x <= r.x1.0 {
                        consider(match side {
                            SkySide::Upper => r.y0.0,
                            SkySide::Lower => r.y1.0,
                        });
                    }
                }
                CollisionShape::Poly(pts) => {
                    for i in 0..pts.len() {
                        let a = pts[i];
                        let b = pts[(i + 1) % pts.len()];
                        let (p, q) = if a.x.0 <= b.x.0 { (a, b) } else { (b, a) };
                        if p.x.0 <= x && x <= q.x.0 && q.x.0 > p.x.0 {
                            let t = (x - p.x.0) / (q.x.0 - p.x.0);
                            consider(p.y.0 + t * (q.y.0 - p.y.0));
                        }
                    }
                }
            }
        }
        best
    }

    /// Skyline construction agrees with the brute-force silhouette on
    /// random shape sets, on both sides.
    #[test]
    fn matches_brute_force_on_random_shapes() {
        let mut rng = Lcg::new(0x5eed_0004);
        for _case in 0..20 {
            let shapes = random_shapes(&mut rng, 40);
            for side in [SkySide::Upper, SkySide::Lower] {
                let sky = Skyline::from_shapes(side, &shapes);
                for i in 0..600 {
                    let x = i as f64 * 110.0 / 600.0 - 2.0;
                    let got = sky.height_at(Sp(x)).map(|v| v.0);
                    let want = brute_at(&shapes, side, x);
                    match (got, want) {
                        (None, None) => {}
                        (Some(g), Some(w)) => {
                            assert!((g - w).abs() < 1e-9, "at x={}: got {} want {}", x, g, w)
                        }
                        // A sample exactly on a segment boundary can differ
                        // in coverage by one ulp; only accept a mismatch if
                        // a boundary is within float noise of the sample.
                        (g, w) => panic!("coverage mismatch at x={}: {:?} vs {:?}", x, g, w),
                    }
                }
            }
        }
    }

    /// Min-clearance sweep agrees with an exhaustive check at every
    /// breakpoint of both skylines (the difference is linear between
    /// breakpoints, so this is exact, not a sampling approximation).
    #[test]
    fn min_clearance_matches_brute_force() {
        let mut rng = Lcg::new(0x5eed_0005);
        for _case in 0..40 {
            let a_shapes = random_shapes(&mut rng, 25);
            let b_shapes = random_shapes(&mut rng, 25);
            let a = Skyline::from_shapes(SkySide::Lower, &a_shapes);
            let b = Skyline::from_shapes(SkySide::Upper, &b_shapes);
            let got = min_clearance(&a, &b).map(|v| v.0);
            // Exhaustive: evaluate the difference at every breakpoint x
            // where both are defined.
            let mut xs: Vec<f64> = Vec::new();
            for s in a.segments().iter().chain(b.segments()) {
                xs.push(s.x0.0);
                xs.push(s.x1.0);
            }
            xs.sort_by(|p, q| p.total_cmp(q));
            let mut want: Option<f64> = None;
            for &x in &xs {
                if let (Some(ya), Some(yb)) = (a.height_at(Sp(x)), b.height_at(Sp(x))) {
                    let d = ya.0 - yb.0;
                    want = Some(match want {
                        Some(w) if w >= d => w,
                        _ => d,
                    });
                }
            }
            match (got, want) {
                (None, None) => {}
                (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "got {} want {}", g, w),
                (g, w) => panic!("clearance coverage mismatch: {:?} vs {:?}", g, w),
            }
        }
    }

    /// Construction work grows near-linearithmically, not quadratically:
    /// doubling the input must scale the elementary-interval count by well
    /// under 4x, and the absolute count stays under a c*n*log2(n) budget.
    #[test]
    fn construction_is_linearithmic() {
        let mut rng = Lcg::new(0x5eed_0006);
        let mut counts = Vec::new();
        for &n in &[256usize, 512, 1024] {
            let shapes = random_shapes(&mut rng, n);
            let (sky, steps) = Skyline::from_shapes_counted(SkySide::Lower, &shapes);
            // Output stays linear in the input.
            assert!(sky.segments().len() <= 6 * n, "envelope blew up: {}", sky.segments().len());
            counts.push((n, steps));
        }
        for w in counts.windows(2) {
            let (n0, s0) = w[0];
            let (n1, s1) = w[1];
            let ratio = s1 as f64 / s0 as f64;
            assert!(
                ratio < 2.8,
                "step growth {} -> {} measures ratio {:.2}: quadratic-ish",
                n0,
                n1,
                ratio
            );
            let budget = 25.0 * n1 as f64 * (n1 as f64).log2();
            assert!((s1 as f64) < budget, "steps {} exceed budget {}", s1, budget);
        }
    }

    #[test]
    fn deterministic_bitwise() {
        let mut rng = Lcg::new(0x5eed_0007);
        let shapes = random_shapes(&mut rng, 64);
        let a = Skyline::from_shapes(SkySide::Lower, &shapes);
        let b = Skyline::from_shapes(SkySide::Lower, &shapes);
        assert_eq!(a.segments().len(), b.segments().len());
        for (p, q) in a.segments().iter().zip(b.segments()) {
            assert_eq!(p.x0.0.to_bits(), q.x0.0.to_bits());
            assert_eq!(p.y0.0.to_bits(), q.y0.0.to_bits());
            assert_eq!(p.x1.0.to_bits(), q.x1.0.to_bits());
            assert_eq!(p.y1.0.to_bits(), q.y1.0.to_bits());
        }
    }
}
