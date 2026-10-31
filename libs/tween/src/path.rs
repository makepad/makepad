//! Motion paths: the geometry of GSAP's `MotionPathPlugin`, rebuilt for the
//! engine.
//!
//! A [`MotionPath`] is a chain of line, quadratic and cubic Bezier segments
//! with f64 coordinates, in one or more subpaths. Finishing a path builds its
//! arc-length table once; after that, sampling at a fraction `u` of the arc
//! length ([`MotionPath::sample`]) is allocation-free, lands exactly on the
//! path's ends and on every table boundary, and equal steps of `u` are equal
//! arc lengths (to about 1e-10 of the length).
//!
//! Coordinates are three-dimensional. The 2D builders ([`PathBuilder::move_to`],
//! [`MotionPath::from_svg`], [`MotionPath::through`], [`MotionPath::polyline`],
//! ...) set z = 0, so a 2D path samples bit-identically whichever API built
//! it; the `*3` counterparts ([`PathBuilder::cubic_to3`],
//! [`MotionPath::through3`], ...) take z. Arc lengths, tangents and the table
//! use full 3D distances, so a 3D path is exact, not projected.
//! [`PathPoint::angle_deg`] is the heading in the xy plane (the angle GSAP's
//! `autoRotate` writes).
//!
//! | Here | GSAP |
//! |---|---|
//! | [`MotionPath::from_svg`] | `motionPath: "M0,0 C.."` (an SVG path `d` string) |
//! | [`MotionPath::through`] | `motionPath: {path: [{x, y}, ..], type: "thru", curviness}` |
//! | [`MotionPath::cubic_points`] | `type: "cubic"` (anchor, control, control, anchor, ..) |
//! | [`MotionPath::sample_span`] | `start` / `end` |
//! | [`PathPoint::angle_deg`] | the `autoRotate` angle |
//!
//! Deviations from GSAP: arcs (`A`) become cubics of at most 90 degrees each
//! (the length is within about 1e-4 relative); the angle is unwrapped
//! (continuous) along the path, so a full lap adds 360 instead of jumping;
//! an open path clamps `start` / `end` fractions outside 0..1 (a closed
//! single-subpath path wraps); the `thru` tangent formula is this crate's
//! (curviness 0 is straight lines, 1 natural, 2 loose, as in GSAP).

use std::fmt;

/// Spans per curved segment before refinement.
pub const PATH_SPANS_MIN: u32 = 4;
/// Spans per curved segment at most (after at most four halvings).
pub const PATH_SPANS_MAX: u32 = 64;
/// [`MotionPath::through`]: a first and last point this close on every axis
/// make a closed path (GSAP 0.001).
pub const THRU_CLOSE_EPS: f64 = 1e-3;

/// Gauss-Legendre 8-point nodes (the positive half) and weights.
const GL_X: [f64; 4] = [
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_3,
];
const GL_W: [f64; 4] = [
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_5,
    0.101_228_536_290_376_3,
];
/// cos(15 deg): the table splits a span whose tangent turns more.
const TURN_COS: f64 = 0.965_925_826_289_068_3;

type V3 = [f64; 3];

#[inline]
fn v_sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn v_add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn v_scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

#[inline]
fn v_len(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

#[inline]
fn v_dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn v_div(a: V3, d: f64) -> V3 {
    [a[0] / d, a[1] / d, a[2] / d]
}

/// `d` wrapped into (-180, 180].
#[inline]
pub(crate) fn wrap180(d: f64) -> f64 {
    let r = d - 360.0 * ((d + 180.0) / 360.0).floor();
    if r == -180.0 {
        180.0
    } else {
        r
    }
}

/// The xy heading of a unit tangent in degrees; `None` when the tangent is
/// (nearly) parallel to the z axis.
#[inline]
fn heading(t: V3) -> Option<f64> {
    if t[0] * t[0] + t[1] * t[1] <= 1e-24 {
        None
    } else {
        Some(t[1].atan2(t[0]).to_degrees())
    }
}

/// What a [`PathSeg`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    /// A straight line.
    Line,
    /// A quadratic Bezier.
    Quad,
    /// A cubic Bezier.
    Cubic,
}

/// One segment of a [`MotionPath`].
///
/// `p` holds the control points: a line is `[a, b, b, b]`, a quadratic
/// `[a, c, b, b]`, a cubic `[a, c1, c2, b]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathSeg {
    /// Line, quadratic or cubic.
    pub kind: SegKind,
    /// The control points (see the type docs).
    pub p: [[f64; 3]; 4],
    /// The subpath this segment belongs to (0-based).
    pub subpath: u32,
}

impl PathSeg {
    /// The start point.
    #[inline]
    pub fn start(&self) -> [f64; 3] {
        self.p[0]
    }

    /// The end point.
    #[inline]
    pub fn end(&self) -> [f64; 3] {
        match self.kind {
            SegKind::Line => self.p[1],
            SegKind::Quad => self.p[2],
            SegKind::Cubic => self.p[3],
        }
    }

    /// The point at parameter `t`: exactly [`PathSeg::start`] for `t <= 0`
    /// and [`PathSeg::end`] for `t >= 1`, else the Bernstein form.
    pub fn point(&self, t: f64) -> [f64; 3] {
        if !(t > 0.0) {
            return self.start();
        }
        if t >= 1.0 {
            return self.end();
        }
        let [a, b, c, d] = self.p;
        let mt = 1.0 - t;
        match self.kind {
            SegKind::Line => [
                a[0] * mt + b[0] * t,
                a[1] * mt + b[1] * t,
                a[2] * mt + b[2] * t,
            ],
            SegKind::Quad => {
                let (w0, w1, w2) = (mt * mt, 2.0 * mt * t, t * t);
                [
                    a[0] * w0 + b[0] * w1 + c[0] * w2,
                    a[1] * w0 + b[1] * w1 + c[1] * w2,
                    a[2] * w0 + b[2] * w1 + c[2] * w2,
                ]
            }
            SegKind::Cubic => {
                let (w0, w1, w2, w3) =
                    (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                [
                    a[0] * w0 + b[0] * w1 + c[0] * w2 + d[0] * w3,
                    a[1] * w0 + b[1] * w1 + c[1] * w2 + d[1] * w3,
                    a[2] * w0 + b[2] * w1 + c[2] * w2 + d[2] * w3,
                ]
            }
        }
    }

    /// dB/dt at `t` (not normalised).
    pub fn derivative(&self, t: f64) -> [f64; 3] {
        let [a, b, c, d] = self.p;
        match self.kind {
            SegKind::Line => v_sub(b, a),
            SegKind::Quad => {
                let mt = 1.0 - t;
                [
                    2.0 * (mt * (b[0] - a[0]) + t * (c[0] - b[0])),
                    2.0 * (mt * (b[1] - a[1]) + t * (c[1] - b[1])),
                    2.0 * (mt * (b[2] - a[2]) + t * (c[2] - b[2])),
                ]
            }
            SegKind::Cubic => {
                let mt = 1.0 - t;
                let (w0, w1, w2) = (3.0 * mt * mt, 6.0 * mt * t, 3.0 * t * t);
                [
                    w0 * (b[0] - a[0]) + w1 * (c[0] - b[0]) + w2 * (d[0] - c[0]),
                    w0 * (b[1] - a[1]) + w1 * (c[1] - b[1]) + w2 * (d[1] - c[1]),
                    w0 * (b[2] - a[2]) + w1 * (c[2] - b[2]) + w2 * (d[2] - c[2]),
                ]
            }
        }
    }

    /// The unit tangent at `t`. Where the derivative vanishes (a cusp, a
    /// zero-length handle) it falls back to the direction the curve leaves
    /// or arrives in: at the start of a cubic `c2 - a`, at its end `b - c1`,
    /// inside it the derivative a step further, else the chord `b - a`;
    /// `(1, 0, 0)` when everything is zero.
    pub fn tangent(&self, t: f64) -> [f64; 3] {
        let eps = 1e-12 * self.extent();
        let unit = |v: V3| -> Option<V3> {
            let n = v_len(v);
            (n > eps && n > 0.0).then(|| v_div(v, n))
        };
        if let Some(u) = unit(self.derivative(t)) {
            return u;
        }
        let [a, c1, c2, b] = self.p;
        let near = match self.kind {
            SegKind::Cubic if t <= 1e-9 => unit(v_sub(c2, a)),
            SegKind::Cubic if t >= 1.0 - 1e-9 => unit(v_sub(b, c1)),
            SegKind::Cubic => {
                let tt = if t > 1.0 - 1e-6 { t - 1e-6 } else { t + 1e-6 };
                unit(self.derivative(tt))
            }
            _ => None,
        };
        near.or_else(|| unit(v_sub(self.end(), a)))
            .unwrap_or([1.0, 0.0, 0.0])
    }

    /// The arc length (adaptive Gauss-Legendre quadrature; exact for lines).
    pub fn length(&self) -> f64 {
        match self.kind {
            SegKind::Line => v_len(v_sub(self.p[1], self.p[0])),
            _ => arc_len(self, 0.0, 1.0, 1e-13 * self.extent().max(1.0)),
        }
    }

    /// The diagonal of the control points' bounding box.
    fn extent(&self) -> f64 {
        let mut lo = self.p[0];
        let mut hi = self.p[0];
        for q in &self.p[1..] {
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        v_len(v_sub(hi, lo))
    }

    /// Every control point within `1e-12 * (1 + max |coord|)` of the start.
    fn is_degenerate(&self) -> bool {
        let a = self.p[0];
        let m = self
            .p
            .iter()
            .flat_map(|q| q.iter())
            .fold(0.0f64, |m, c| m.max(c.abs()));
        let eps = 1e-12 * (1.0 + m);
        self.p[1..]
            .iter()
            .all(|q| (0..3).all(|i| (q[i] - a[i]).abs() <= eps))
    }
}

/// Gauss-Legendre 8-point arc length of `s` over `[a, b]`.
#[inline]
fn gl8(s: &PathSeg, a: f64, b: f64) -> f64 {
    let h = 0.5 * (b - a);
    let m = 0.5 * (a + b);
    let mut sum = 0.0;
    for i in 0..4 {
        let dx = h * GL_X[i];
        sum += GL_W[i] * (v_len(s.derivative(m - dx)) + v_len(s.derivative(m + dx)));
    }
    h * sum
}

/// Adaptive arc length (build time only): halves until two halves agree
/// with the whole within `tol`, or at depth 10.
fn arc_len(s: &PathSeg, a: f64, b: f64, tol: f64) -> f64 {
    fn rec(s: &PathSeg, a: f64, b: f64, whole: f64, tol: f64, depth: u32) -> f64 {
        let m = 0.5 * (a + b);
        let l = gl8(s, a, m);
        let r = gl8(s, m, b);
        if depth >= 10 || (l + r - whole).abs() <= tol {
            l + r
        } else {
            rec(s, a, m, l, tol, depth + 1) + rec(s, m, b, r, tol, depth + 1)
        }
    }
    rec(s, a, b, gl8(s, a, b), tol, 0)
}

/// One point sampled on a path.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PathPoint {
    /// The position.
    pub pos: [f64; 3],
    /// The unit tangent (the travel direction; reversed spans flip it).
    pub tangent: [f64; 3],
    /// The xy heading in degrees, unwrapped (continuous) along the path.
    pub angle_deg: f64,
    /// The segment the point lies on.
    pub seg: u32,
    /// The segment parameter of the point.
    pub t: f64,
}

impl PathPoint {
    /// [`PathPoint::angle_deg`] wrapped into (-180, 180].
    pub fn angle_wrapped(&self) -> f64 {
        wrap180(self.angle_deg)
    }
}

/// A tight axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PathBounds {
    /// The smallest coordinates.
    pub min: [f64; 3],
    /// The largest coordinates.
    pub max: [f64; 3],
}

impl PathBounds {
    /// `max - min`.
    pub fn size(&self) -> [f64; 3] {
        v_sub(self.max, self.min)
    }

    /// The centre.
    pub fn center(&self) -> [f64; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }

    fn include(&mut self, p: V3) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }
}

/// Why a path could not be built. `at` is a byte offset into an SVG `d`
/// string (0 for the builder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    /// Nothing drawable: no segment, or `through()` with fewer than two
    /// distinct points.
    Empty,
    /// Segments exist but all of them have zero length.
    ZeroLength,
    /// A NaN or infinite coordinate or curviness.
    NotFinite,
    /// A drawing command before the first `M` / `m` (a builder call with no
    /// current point: `at` is 0).
    NoCurrentPoint {
        /// Byte offset of the command.
        at: u32,
    },
    /// A byte that is not an SVG path command.
    UnknownCommand {
        /// Byte offset of the command.
        at: u32,
        /// The byte.
        cmd: u8,
    },
    /// A missing or malformed number or flag, or a stray number.
    Syntax {
        /// Byte offset of the offending token.
        at: u32,
    },
    /// [`MotionPath::cubic_points`]: the point count is not `3k + 1` with `k >= 1`.
    PointCount {
        /// How many points were given.
        got: u32,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PathError::Empty => write!(f, "motion path: nothing to draw"),
            PathError::ZeroLength => write!(f, "motion path: every segment has zero length"),
            PathError::NotFinite => {
                write!(
                    f,
                    "motion path: a coordinate or the curviness is not finite"
                )
            }
            PathError::NoCurrentPoint { at } => write!(
                f,
                "motion path: a drawing command before the first M at byte {at}"
            ),
            PathError::UnknownCommand { at, cmd } => {
                if cmd.is_ascii_graphic() {
                    write!(
                        f,
                        "motion path: unknown command '{}' at byte {at}",
                        cmd as char
                    )
                } else {
                    write!(f, "motion path: unknown command 0x{cmd:02x} at byte {at}")
                }
            }
            PathError::Syntax { at } => {
                write!(f, "motion path: a missing or malformed number at byte {at}")
            }
            PathError::PointCount { got } => write!(
                f,
                "motion path: cubic points need 3k + 1 points (k >= 1), got {got}"
            ),
        }
    }
}

impl std::error::Error for PathError {}

/// Builds a [`MotionPath`] command by command (SVG's path model).
///
/// The 2D calls set z = 0; the `*3` calls take z. The first error recorded
/// (a drawing call with no current point, a non-finite argument) is what
/// [`PathBuilder::finish`] returns.
///
/// ```
/// use makepad_tween::*;
/// let mut b = MotionPath::builder();
/// b.move_to(0.0, 0.0).cubic_to(0.0, 100.0, 200.0, -100.0, 200.0, 0.0).line_to(300.0, 0.0);
/// let path = b.finish().unwrap();
/// assert_eq!(path.segment_count(), 2);
/// assert_eq!(path.sample(1.0).pos, [300.0, 0.0, 0.0]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct PathBuilder {
    segs: Vec<PathSeg>,
    cur: Option<[f64; 3]>,
    sub_start: [f64; 3],
    sub: u32,
    sub_used: bool,
    closed: Vec<bool>,
    err: Option<PathError>,
}

impl PathBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    fn fail(&mut self, e: PathError) {
        if self.err.is_none() {
            self.err = Some(e);
        }
    }

    fn finite(&mut self, v: &[f64]) -> bool {
        if v.iter().all(|x| x.is_finite()) {
            true
        } else {
            self.fail(PathError::NotFinite);
            false
        }
    }

    fn from_cur(&mut self) -> Option<[f64; 3]> {
        if self.cur.is_none() {
            self.fail(PathError::NoCurrentPoint { at: 0 });
        }
        self.cur
    }

    fn push(&mut self, kind: SegKind, p: [[f64; 3]; 4]) {
        if !self.sub_used {
            self.sub_used = true;
            self.closed.push(false);
        }
        let seg = PathSeg {
            kind,
            p,
            subpath: self.sub,
        };
        self.cur = Some(seg.end());
        self.segs.push(seg);
    }

    /// Starts a subpath at (x, y, 0) (SVG `M`).
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.move_to3(x, y, 0.0)
    }

    /// Starts a subpath at (x, y, z). A move right after another (no segment
    /// drawn yet) only moves the start.
    pub fn move_to3(&mut self, x: f64, y: f64, z: f64) -> &mut Self {
        if !self.finite(&[x, y, z]) {
            return self;
        }
        if self.sub_used {
            self.sub += 1;
            self.sub_used = false;
        }
        self.cur = Some([x, y, z]);
        self.sub_start = [x, y, z];
        self
    }

    /// A line to (x, y, 0) (SVG `L`).
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.line_to3(x, y, 0.0)
    }

    /// A line to (x, y, z).
    pub fn line_to3(&mut self, x: f64, y: f64, z: f64) -> &mut Self {
        if !self.finite(&[x, y, z]) {
            return self;
        }
        let Some(a) = self.from_cur() else {
            return self;
        };
        let b = [x, y, z];
        self.push(SegKind::Line, [a, b, b, b]);
        self
    }

    /// A quadratic Bezier with control (cx, cy, 0) to (x, y, 0) (SVG `Q`).
    pub fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) -> &mut Self {
        self.quad_to3(cx, cy, 0.0, x, y, 0.0)
    }

    /// A quadratic Bezier with control (cx, cy, cz) to (x, y, z).
    pub fn quad_to3(&mut self, cx: f64, cy: f64, cz: f64, x: f64, y: f64, z: f64) -> &mut Self {
        if !self.finite(&[cx, cy, cz, x, y, z]) {
            return self;
        }
        let Some(a) = self.from_cur() else {
            return self;
        };
        let b = [x, y, z];
        self.push(SegKind::Quad, [a, [cx, cy, cz], b, b]);
        self
    }

    /// A cubic Bezier with controls (c1x, c1y, 0), (c2x, c2y, 0) to
    /// (x, y, 0) (SVG `C`).
    pub fn cubic_to(
        &mut self,
        c1x: f64,
        c1y: f64,
        c2x: f64,
        c2y: f64,
        x: f64,
        y: f64,
    ) -> &mut Self {
        self.cubic_to3([c1x, c1y, 0.0], [c2x, c2y, 0.0], [x, y, 0.0])
    }

    /// A cubic Bezier with controls `c1`, `c2` to `p`.
    pub fn cubic_to3(&mut self, c1: [f64; 3], c2: [f64; 3], p: [f64; 3]) -> &mut Self {
        if !self.finite(&[c1[0], c1[1], c1[2], c2[0], c2[1], c2[2], p[0], p[1], p[2]]) {
            return self;
        }
        let Some(a) = self.from_cur() else {
            return self;
        };
        self.push(SegKind::Cubic, [a, c1, c2, p]);
        self
    }

    /// An elliptical arc to (x, y, 0) (SVG `A`, SVG 1.1 F.6.5 / F.6.6),
    /// converted to cubics of at most 90 degrees each; the last one ends
    /// exactly on (x, y). An endpoint equal to the current point draws
    /// nothing, a zero radius draws a line, radii too small for the chord
    /// are scaled up. The arc is planar: its points have z = 0.
    #[allow(clippy::too_many_arguments)]
    pub fn arc_to(
        &mut self,
        rx: f64,
        ry: f64,
        x_rot_deg: f64,
        large: bool,
        sweep: bool,
        x: f64,
        y: f64,
    ) -> &mut Self {
        if !self.finite(&[rx, ry, x_rot_deg, x, y]) {
            return self;
        }
        let Some(c0) = self.from_cur() else {
            return self;
        };
        let (x1, y1) = (c0[0], c0[1]);
        if x == x1 && y == y1 {
            if c0[2] != 0.0 {
                self.line_to(x, y);
            }
            return self;
        }
        if rx == 0.0 || ry == 0.0 {
            return self.line_to(x, y);
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        let (s, c) = x_rot_deg.to_radians().sin_cos();
        let dx = (x1 - x) / 2.0;
        let dy = (y1 - y) / 2.0;
        let x1p = c * dx + s * dy;
        let y1p = -s * dx + c * dy;
        let lam = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
        if lam > 1.0 {
            let k = lam.sqrt();
            rx *= k;
            ry *= k;
        }
        let (rx2, ry2) = (rx * rx, ry * ry);
        let num = rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p;
        let den = rx2 * y1p * y1p + ry2 * x1p * x1p;
        let sign = if large == sweep { -1.0 } else { 1.0 };
        // Radii scaled up to the chord put the centre on it: q is 0 exactly
        // (the formula would leave a roundoff residue of about 1e-8).
        let q = if lam >= 1.0 {
            0.0
        } else {
            sign * (num / den).max(0.0).sqrt()
        };
        let cxp = q * rx * y1p / ry;
        let cyp = -q * ry * x1p / rx;
        let cx = c * cxp - s * cyp + (x1 + x) / 2.0;
        let cy = s * cxp + c * cyp + (y1 + y) / 2.0;
        let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
        let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
        let th1 = uy.atan2(ux);
        let mut dth = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if !sweep && dth > 0.0 {
            dth -= std::f64::consts::TAU;
        } else if sweep && dth < 0.0 {
            dth += std::f64::consts::TAU;
        }
        let n = ((dth.abs() / std::f64::consts::FRAC_PI_2 - 1e-9).ceil()).max(1.0) as u32;
        let d = dth / n as f64;
        let k = 4.0 / 3.0 * (d / 4.0).tan();
        let map = |vx: f64, vy: f64| -> [f64; 3] {
            [
                cx + c * rx * vx - s * ry * vy,
                cy + s * rx * vx + c * ry * vy,
                0.0,
            ]
        };
        for i in 0..n {
            let a0 = th1 + i as f64 * d;
            let a1 = a0 + d;
            let (s0, c0) = a0.sin_cos();
            let (s1, c1) = a1.sin_cos();
            let p1 = map(c0 - k * s0, s0 + k * c0);
            let p2 = map(c1 + k * s1, s1 - k * c1);
            let p3 = if i == n - 1 { [x, y, 0.0] } else { map(c1, s1) };
            self.cubic_to3(p1, p2, p3);
        }
        self
    }

    /// Closes the subpath (SVG `Z`): a line back to its start unless the
    /// current point is there already. The current point becomes the
    /// subpath's start; a following drawing call starts a new subpath there.
    pub fn close(&mut self) -> &mut Self {
        let Some(c) = self.from_cur() else {
            return self;
        };
        if self.sub_used {
            let s = self.sub_start;
            if c != s {
                self.line_to3(s[0], s[1], s[2]);
            }
            self.closed[self.sub as usize] = true;
            self.sub += 1;
            self.sub_used = false;
        }
        self.cur = Some(self.sub_start);
        self
    }

    /// The current point, if any.
    pub fn current(&self) -> Option<[f64; 3]> {
        self.cur
    }

    /// Takes the segments and builds the path (its arc-length table once).
    /// The first recorded error wins; segments of zero size are dropped;
    /// no segment gives [`PathError::Empty`], only zero-size ones
    /// [`PathError::ZeroLength`]. The builder is empty afterwards.
    pub fn finish(&mut self) -> Result<MotionPath, PathError> {
        let b = std::mem::take(self);
        if let Some(e) = b.err {
            return Err(e);
        }
        if b.segs.is_empty() {
            return Err(PathError::Empty);
        }
        let mut segs = Vec::with_capacity(b.segs.len());
        let (mut subpaths, mut prev, mut closed) = (0u32, u32::MAX, false);
        for mut s in b.segs {
            if s.is_degenerate() {
                continue;
            }
            if s.subpath != prev {
                prev = s.subpath;
                subpaths += 1;
                closed = b.closed.get(s.subpath as usize).copied().unwrap_or(false);
            }
            s.subpath = subpaths - 1;
            segs.push(s);
        }
        if segs.is_empty() {
            return Err(PathError::ZeroLength);
        }
        Ok(MotionPath::from_segs(
            segs,
            subpaths,
            subpaths == 1 && closed,
        ))
    }
}

/// One entry of the arc-length table: a parameter interval of one segment
/// and its arc-length interval along the whole path.
#[derive(Clone, Copy, Debug)]
struct ArcSpan {
    s0: f64,
    s1: f64,
    t0: f64,
    t1: f64,
    seg: u32,
    /// The unwrapped heading at `t0`.
    ang0: f64,
}

/// A motion path: segments, subpaths, tight bounds and an arc-length table.
///
/// `Default` is the empty placeholder: length 0, [`MotionPath::sample`]
/// answers `PathPoint::default()`.
///
/// ```
/// use makepad_tween::*;
/// let wave = MotionPath::through(&[[0.0, 100.0], [100.0, 20.0], [200.0, 180.0]], 1.0).unwrap();
/// let p = wave.sample(0.5); // halfway along the arc length
/// assert!(p.pos[0] > 0.0 && p.pos[0] < 200.0);
/// assert_eq!(wave.sample(1.0).pos, [200.0, 180.0, 0.0]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct MotionPath {
    segs: Vec<PathSeg>,
    spans: Vec<ArcSpan>,
    len: f64,
    bounds: PathBounds,
    subpaths: u32,
    closed: bool,
    turn: f64,
    /// What each extra lap adds to the heading: `turn` plus the corner at
    /// the seam (a whole number of turns on a closed path).
    lap: f64,
}

impl MotionPath {
    /// A [`PathBuilder`].
    pub fn builder() -> PathBuilder {
        PathBuilder::new()
    }

    fn from_segs(segs: Vec<PathSeg>, subpaths: u32, closed: bool) -> MotionPath {
        let mut spans = Vec::with_capacity(segs.len() * PATH_SPANS_MIN as usize);
        let mut s = 0.0;
        for (i, seg) in segs.iter().enumerate() {
            if seg.kind == SegKind::Line {
                let s1 = s + v_len(v_sub(seg.p[1], seg.p[0]));
                spans.push(ArcSpan {
                    s0: s,
                    s1,
                    t0: 0.0,
                    t1: 1.0,
                    seg: i as u32,
                    ang0: 0.0,
                });
                s = s1;
            } else {
                let ext = seg.extent();
                let tol = 1e-13 * ext.max(1.0);
                let n = PATH_SPANS_MIN as f64;
                for k in 0..PATH_SPANS_MIN {
                    let (a, b) = (k as f64 / n, (k + 1) as f64 / n);
                    refine(seg, i as u32, a, b, 0, ext, tol, &mut spans, &mut s);
                }
            }
        }
        // Unwrapped headings: each span continues the previous one.
        let mut last: Option<f64> = None;
        for sp in spans.iter_mut() {
            let seg = &segs[sp.seg as usize];
            let raw0 = heading(seg.tangent(sp.t0)).or(last).unwrap_or(0.0);
            let ang0 = match last {
                None => raw0,
                Some(l) => l + wrap180(raw0 - l),
            };
            let raw1 = heading(seg.tangent(sp.t1)).unwrap_or(ang0);
            sp.ang0 = ang0;
            last = Some(ang0 + wrap180(raw1 - ang0));
        }
        let first = spans.first().map_or(0.0, |sp| sp.ang0);
        let end = last.unwrap_or(0.0);
        let turn = end - first;
        // A lap continues from the end heading into the start heading: the
        // turn plus the seam's corner (0 when the path is smooth there).
        let lap = turn + wrap180(first - end);
        let mut bounds = PathBounds {
            min: segs[0].p[0],
            max: segs[0].p[0],
        };
        for seg in &segs {
            seg_bounds(seg, &mut bounds);
        }
        MotionPath {
            segs,
            spans,
            len: s,
            bounds,
            subpaths,
            closed,
            turn,
            lap,
        }
    }

    /// Parses an SVG path `d` string: `M L H V C S Q T A Z`, both cases,
    /// SVG number syntax (`"M1.5.5L-2-2"`, `1e1`), arc flags that abut the
    /// next token. Arcs become cubics. Errors carry the byte offset.
    pub fn from_svg(d: &str) -> Result<MotionPath, PathError> {
        parse_svg(d)?.finish()
    }

    /// A smooth curve through `points` (GSAP `type: "thru"`), z = 0.
    /// `curviness` 0 draws straight lines, 1 a natural curve, 2 a loose one.
    /// Consecutive duplicates are dropped; a last point within
    /// [`THRU_CLOSE_EPS`] of the first closes the path when at least three
    /// distinct points remain (an out-and-back of two points stays open).
    pub fn through(points: &[[f64; 2]], curviness: f64) -> Result<MotionPath, PathError> {
        let p: Vec<[f64; 3]> = points.iter().map(|p| [p[0], p[1], 0.0]).collect();
        Self::through3(&p, curviness)
    }

    /// [`MotionPath::through`] in 3D.
    ///
    /// Each point's tangent is the bisector of its unit chords (Catmull-Rom
    /// with duplicated end points at open ends); each segment is a cubic
    /// whose handles are `curviness * chord / 3` long along those tangents.
    pub fn through3(points: &[[f64; 3]], curviness: f64) -> Result<MotionPath, PathError> {
        if !curviness.is_finite() || points.iter().any(|p| p.iter().any(|c| !c.is_finite())) {
            return Err(PathError::NotFinite);
        }
        let k = curviness.max(0.0);
        let mut q: Vec<[f64; 3]> = Vec::with_capacity(points.len());
        for &p in points {
            if let Some(&l) = q.last() {
                if (0..3).all(|i| (p[i] - l[i]).abs() <= 1e-9 * (1.0 + p[i].abs())) {
                    continue;
                }
            }
            q.push(p);
        }
        let dup = |a: V3, b: V3| (0..3).all(|i| (a[i] - b[i]).abs() <= 1e-9 * (1.0 + a[i].abs()));
        let mut closed = q.len() >= 3 && {
            let (f, l) = (q[0], q[q.len() - 1]);
            (0..3).all(|i| (l[i] - f[i]).abs() <= THRU_CLOSE_EPS)
        };
        if closed {
            // The loop drops the closing point, and any point left equal to
            // the first across the seam. Fewer than three distinct points
            // make no loop: an out-and-back stays an open path.
            let mut ring = q.clone();
            ring.pop();
            while ring.len() > 1 && dup(ring[ring.len() - 1], ring[0]) {
                ring.pop();
            }
            if ring.len() >= 3 {
                q = ring;
            } else {
                closed = false;
            }
        }
        let n = q.len();
        if n < 2 {
            return Err(PathError::Empty);
        }
        if k == 0.0 {
            return Self::polyline3(&q, closed);
        }
        let unit = |v: V3| -> V3 {
            let l = v_len(v);
            if l > 0.0 {
                v_div(v, l)
            } else {
                [1.0, 0.0, 0.0]
            }
        };
        let mut tan = Vec::with_capacity(n);
        for i in 0..n {
            let t = if !closed && i == 0 {
                unit(v_sub(q[1], q[0]))
            } else if !closed && i == n - 1 {
                unit(v_sub(q[n - 1], q[n - 2]))
            } else {
                let prev = q[(i + n - 1) % n];
                let next = q[(i + 1) % n];
                let d1 = v_sub(q[i], prev);
                let d2 = v_sub(next, q[i]);
                let (r1, r2) = (v_len(d1), v_len(d2));
                let s = v_add(v_div(d1, r1), v_div(d2, r2));
                let ls = v_len(s);
                if ls < 1e-9 {
                    v_div(d2, r2)
                } else {
                    v_div(s, ls)
                }
            };
            tan.push(t);
        }
        let mut b = PathBuilder::new();
        b.move_to3(q[0][0], q[0][1], q[0][2]);
        let count = if closed { n } else { n - 1 };
        for i in 0..count {
            let j = (i + 1) % n;
            let h = k * v_len(v_sub(q[j], q[i])) / 3.0;
            let c1 = v_add(q[i], v_scale(tan[i], h));
            let c2 = v_sub(q[j], v_scale(tan[j], h));
            b.cubic_to3(c1, c2, q[j]);
        }
        if closed {
            b.close();
        }
        b.finish()
    }

    /// Cubics from anchor, control, control, anchor, ... points (GSAP
    /// `type: "cubic"`), z = 0: `3k + 1` points with `k >= 1`. Closed when
    /// the last point equals the first.
    pub fn cubic_points(points: &[[f64; 2]]) -> Result<MotionPath, PathError> {
        let p: Vec<[f64; 3]> = points.iter().map(|p| [p[0], p[1], 0.0]).collect();
        Self::cubic_points3(&p)
    }

    /// [`MotionPath::cubic_points`] in 3D.
    pub fn cubic_points3(points: &[[f64; 3]]) -> Result<MotionPath, PathError> {
        let n = points.len();
        if n < 4 || (n - 1) % 3 != 0 {
            return Err(PathError::PointCount { got: n as u32 });
        }
        let mut b = PathBuilder::new();
        b.move_to3(points[0][0], points[0][1], points[0][2]);
        for i in 0..(n - 1) / 3 {
            b.cubic_to3(points[3 * i + 1], points[3 * i + 2], points[3 * i + 3]);
        }
        if points[n - 1] == points[0] {
            b.close();
        }
        b.finish()
    }

    /// Straight lines through `points` (z = 0), closed back to the first
    /// point when asked.
    pub fn polyline(points: &[[f64; 2]], closed: bool) -> Result<MotionPath, PathError> {
        let p: Vec<[f64; 3]> = points.iter().map(|p| [p[0], p[1], 0.0]).collect();
        Self::polyline3(&p, closed)
    }

    /// [`MotionPath::polyline`] in 3D.
    pub fn polyline3(points: &[[f64; 3]], closed: bool) -> Result<MotionPath, PathError> {
        let mut b = PathBuilder::new();
        if let Some(p) = points.first() {
            b.move_to3(p[0], p[1], p[2]);
        }
        for p in points.iter().skip(1) {
            b.line_to3(p[0], p[1], p[2]);
        }
        if closed && !points.is_empty() {
            b.close();
        }
        b.finish()
    }

    /// The total arc length.
    pub fn length(&self) -> f64 {
        self.len
    }

    /// The tight bounding box (curve extremes included).
    pub fn bounds(&self) -> PathBounds {
        self.bounds
    }

    /// Exactly one subpath, and it is closed: only then does `u` wrap.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// How many subpaths (each with at least one segment).
    pub fn subpath_count(&self) -> u32 {
        self.subpaths
    }

    /// How many segments.
    pub fn segment_count(&self) -> u32 {
        self.segs.len() as u32
    }

    /// Segment `i` (panics when `i >= segment_count()`).
    pub fn segment(&self, i: u32) -> PathSeg {
        self.segs[i as usize]
    }

    /// How many arc-length spans the table holds (one per line, 4 to 64
    /// per curve).
    pub fn span_count(&self) -> u32 {
        self.spans.len() as u32
    }

    /// The first point (the origin for the empty path).
    pub fn start_point(&self) -> [f64; 3] {
        self.segs.first().map_or([0.0; 3], |s| s.start())
    }

    /// The last point (the origin for the empty path).
    pub fn end_point(&self) -> [f64; 3] {
        self.segs.last().map_or([0.0; 3], |s| s.end())
    }

    /// The unwrapped heading at the end minus the one at the start: 360 for
    /// one clockwise lap of a circle in y-down coordinates, 270 for a square
    /// (its fourth corner is at the seam, see [`MotionPath::lap_turn`]).
    pub fn total_turn(&self) -> f64 {
        self.turn
    }

    /// What each extra lap of a closed path adds to the heading
    /// ([`MotionPath::sample`] at `u + 1`): [`MotionPath::total_turn`] plus
    /// the corner at the seam, so a whole number of turns (360 for the
    /// circle and for the square). Meaningless for an open path, which does
    /// not wrap.
    pub fn lap_turn(&self) -> f64 {
        self.lap
    }

    /// Moves the path by (dx, dy, 0).
    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.translate3(dx, dy, 0.0);
    }

    /// Moves the path by (dx, dy, dz): control points and bounds (the arc
    /// lengths and headings do not change).
    pub fn translate3(&mut self, dx: f64, dy: f64, dz: f64) {
        let d = [dx, dy, dz];
        for s in self.segs.iter_mut() {
            for q in s.p.iter_mut() {
                *q = v_add(*q, d);
            }
        }
        self.bounds.min = v_add(self.bounds.min, d);
        self.bounds.max = v_add(self.bounds.max, d);
    }

    /// The point at fraction `u` of the arc length: exactly the start point
    /// at 0 and the end point at 1. A closed path wraps (`u` 1.25 is 0.25
    /// one lap on, its angle [`MotionPath::lap_turn`] further); an open one clamps. A
    /// non-finite `u` counts as 0. Allocation-free.
    pub fn sample(&self, u: f64) -> PathPoint {
        if self.segs.is_empty() {
            return PathPoint::default();
        }
        let (w, laps) = self.wrap(u);
        self.point_at(w * self.len, laps)
    }

    /// GSAP `start` / `end`: the point at eased ratio `e` (clamped to 0..=1)
    /// of the way from fraction `start` to fraction `end`. `e >= 1` lands
    /// on `end` exactly. When `end < start` the point faces the travel
    /// direction (tangent reversed, angle + 180). Non-finite fractions count
    /// as 0 and 1.
    pub fn sample_span(&self, start: f64, end: f64, e: f64) -> PathPoint {
        let e = if e.is_nan() { 0.0 } else { e.clamp(0.0, 1.0) };
        let start = if start.is_finite() { start } else { 0.0 };
        let end = if end.is_finite() { end } else { 1.0 };
        let u = if e >= 1.0 {
            end
        } else {
            start + (end - start) * e
        };
        let mut p = self.sample(u);
        if end < start {
            p.tangent = [-p.tangent[0], -p.tangent[1], -p.tangent[2]];
            p.angle_deg += 180.0;
        }
        p
    }

    /// The point at arc length `s`, clamped to `0..=length()` (no wrap).
    pub fn sample_at_length(&self, s: f64) -> PathPoint {
        if self.segs.is_empty() {
            return PathPoint::default();
        }
        let s = if s.is_nan() {
            0.0
        } else {
            s.clamp(0.0, self.len)
        };
        self.point_at(s, 0.0)
    }

    /// Walks the path as polylines for drawing: `f(point, starts_subpath)`.
    /// Each subpath starts with its first point; a line emits its end, a
    /// curve `steps` points (its end last). No allocation.
    pub fn flatten(&self, steps: u32, mut f: impl FnMut([f64; 3], bool)) {
        let steps = steps.max(1);
        let mut prev = u32::MAX;
        for seg in &self.segs {
            if seg.subpath != prev {
                prev = seg.subpath;
                f(seg.start(), true);
            }
            if seg.kind == SegKind::Line {
                f(seg.end(), false);
            } else {
                for i in 1..=steps {
                    f(seg.point(i as f64 / steps as f64), false);
                }
            }
        }
    }

    /// `u` folded into 0..=1 and the number of laps (closed paths only).
    #[inline]
    fn wrap(&self, u: f64) -> (f64, f64) {
        if !u.is_finite() {
            return (0.0, 0.0);
        }
        if (0.0..=1.0).contains(&u) {
            return (u, 0.0);
        }
        if !self.closed {
            return (u.clamp(0.0, 1.0), 0.0);
        }
        let fl = u.floor();
        let f = u - fl;
        if f == 0.0 {
            if u > 0.0 {
                (1.0, u - 1.0)
            } else {
                (0.0, u)
            }
        } else {
            (f, fl)
        }
    }

    /// The point at arc length `s` (within `0..=len`): a binary search of
    /// the table, then Newton on the exact span arc length with a bisection
    /// fallback. Span ends are exact.
    fn point_at(&self, s: f64, laps: f64) -> PathPoint {
        let i = self
            .spans
            .partition_point(|sp| sp.s1 < s)
            .min(self.spans.len() - 1);
        let sp = self.spans[i];
        let seg = &self.segs[sp.seg as usize];
        let target = s - sp.s0;
        let l = sp.s1 - sp.s0;
        let t = if target <= 0.0 {
            sp.t0
        } else if target >= l {
            sp.t1
        } else if seg.kind == SegKind::Line {
            sp.t0 + (sp.t1 - sp.t0) * (target / l)
        } else {
            let mut t = sp.t0 + (sp.t1 - sp.t0) * target / l;
            let (mut lo, mut hi) = (sp.t0, sp.t1);
            let tol = 1e-12 * self.len.max(1.0);
            for _ in 0..12 {
                let f = gl8(seg, sp.t0, t) - target;
                if f.abs() <= tol {
                    break;
                }
                if f > 0.0 {
                    hi = t;
                } else {
                    lo = t;
                }
                let d = v_len(seg.derivative(t));
                let n = t - f / d;
                t = if d > 1e-300 && lo < n && n < hi {
                    n
                } else {
                    0.5 * (lo + hi)
                };
            }
            t
        };
        let pos = seg.point(t);
        let tangent = seg.tangent(t);
        let angle = if seg.kind == SegKind::Line {
            sp.ang0
        } else {
            sp.ang0 + wrap180(heading(tangent).unwrap_or(sp.ang0) - sp.ang0)
        };
        PathPoint {
            pos,
            tangent,
            angle_deg: angle + laps * self.lap,
            seg: sp.seg,
            t,
        }
    }
}

/// Splits `[a, b]` of curve `seg` while it bulges, turns more than 15
/// degrees or changes speed by more than 1.5x (at most four halvings), and
/// appends the resulting spans.
#[allow(clippy::too_many_arguments)]
fn refine(
    seg: &PathSeg,
    ix: u32,
    a: f64,
    b: f64,
    depth: u32,
    ext: f64,
    tol: f64,
    spans: &mut Vec<ArcSpan>,
    s: &mut f64,
) {
    let l = arc_len(seg, a, b, tol);
    if depth < 4 && needs_split(seg, a, b, l, ext) {
        let m = 0.5 * (a + b);
        refine(seg, ix, a, m, depth + 1, ext, tol, spans, s);
        refine(seg, ix, m, b, depth + 1, ext, tol, spans, s);
    } else {
        let s0 = *s;
        let s1 = s0 + l;
        spans.push(ArcSpan {
            s0,
            s1,
            t0: a,
            t1: b,
            seg: ix,
            ang0: 0.0,
        });
        *s = s1;
    }
}

fn needs_split(seg: &PathSeg, a: f64, b: f64, l: f64, ext: f64) -> bool {
    let m = 0.5 * (a + b);
    if v_len(v_sub(seg.point(b), seg.point(a))) < l * (1.0 - 1e-3) {
        return true;
    }
    let (ta, tm, tb) = (seg.tangent(a), seg.tangent(m), seg.tangent(b));
    if v_dot(ta, tm) < TURN_COS || v_dot(tm, tb) < TURN_COS {
        return true;
    }
    let va = v_len(seg.derivative(a));
    let vm = v_len(seg.derivative(m));
    let vb = v_len(seg.derivative(b));
    let lo = va.min(vm).min(vb);
    let hi = va.max(vm).max(vb);
    lo < 1e-12 * ext || hi > 1.5 * lo
}

/// Extends `b` by segment `s`: its ends and the points where a coordinate's
/// derivative vanishes inside it.
fn seg_bounds(s: &PathSeg, b: &mut PathBounds) {
    b.include(s.start());
    b.include(s.end());
    match s.kind {
        SegKind::Line => {}
        SegKind::Quad => {
            for i in 0..3 {
                let (a, c, e) = (s.p[0][i], s.p[1][i], s.p[2][i]);
                let den = a - 2.0 * c + e;
                if den != 0.0 {
                    let t = (a - c) / den;
                    if t > 0.0 && t < 1.0 {
                        b.include(s.point(t));
                    }
                }
            }
        }
        SegKind::Cubic => {
            for i in 0..3 {
                let (a, c1, c2, e) = (s.p[0][i], s.p[1][i], s.p[2][i], s.p[3][i]);
                let qa = e - 3.0 * c2 + 3.0 * c1 - a;
                let qb = 2.0 * (c2 - 2.0 * c1 + a);
                let qc = c1 - a;
                let mut roots = [f64::NAN; 2];
                if qa.abs() < 1e-12 {
                    if qb != 0.0 {
                        roots[0] = -qc / qb;
                    }
                } else {
                    let disc = qb * qb - 4.0 * qa * qc;
                    if disc >= 0.0 {
                        let q = -0.5 * (qb + qb.signum() * disc.sqrt());
                        roots[0] = q / qa;
                        if q != 0.0 {
                            roots[1] = qc / q;
                        }
                    }
                }
                for t in roots {
                    if t > 0.0 && t < 1.0 {
                        b.include(s.point(t));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SVG path data
// ---------------------------------------------------------------------------

/// The previous command group, for the S / T reflections.
#[derive(Clone, Copy)]
enum Prev {
    Other,
    /// A C / S group and its second control point.
    Cubic([f64; 2]),
    /// A Q / T group and its (possibly implied) control point.
    Quad([f64; 2]),
}

struct Lexer<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len()
            && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r' | 0x0C | b',')
        {
            self.i += 1;
        }
    }

    /// Whether the next byte (separators skipped) starts a number.
    fn at_number(&mut self) -> bool {
        self.skip_sep();
        self.i < self.b.len() && matches!(self.b[self.i], b'+' | b'-' | b'.' | b'0'..=b'9')
    }

    fn number(&mut self) -> Result<f64, PathError> {
        self.skip_sep();
        let b = self.b;
        let start = self.i;
        let syntax = PathError::Syntax { at: start as u32 };
        let mut j = start;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let int0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let mut digits = j - int0;
        if j < b.len() && b[j] == b'.' {
            j += 1;
            let f0 = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            digits += j - f0;
        }
        if digits == 0 {
            return Err(syntax);
        }
        if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
            let mut k = j + 1;
            if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                k += 1;
            }
            let e0 = k;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k == e0 {
                return Err(syntax);
            }
            j = k;
        }
        let v: f64 = self.s[start..j].parse().map_err(|_| syntax)?;
        if !v.is_finite() {
            return Err(PathError::NotFinite);
        }
        self.i = j;
        Ok(v)
    }

    fn flag(&mut self) -> Result<bool, PathError> {
        self.skip_sep();
        let v = match self.b.get(self.i) {
            Some(b'0') => false,
            Some(b'1') => true,
            _ => return Err(PathError::Syntax { at: self.i as u32 }),
        };
        self.i += 1;
        Ok(v)
    }

    fn pair(&mut self) -> Result<[f64; 2], PathError> {
        Ok([self.number()?, self.number()?])
    }
}

/// Parses SVG path data into a builder (the grammar of SVG 1.1 8.3.9).
fn parse_svg(d: &str) -> Result<PathBuilder, PathError> {
    let mut lx = Lexer {
        s: d,
        b: d.as_bytes(),
        i: 0,
    };
    let mut bld = PathBuilder::new();
    lx.skip_sep();
    if lx.i >= lx.b.len() {
        return Err(PathError::Empty);
    }
    let mut cur = [0.0f64; 2];
    let mut start = [0.0f64; 2];
    let mut prev = Prev::Other;
    // The command whose argument groups repeat (M becomes L after its
    // first pair); None at the start and after Z.
    let mut active: Option<u8> = None;
    let mut first = true;
    loop {
        lx.skip_sep();
        if lx.i >= lx.b.len() {
            break;
        }
        let at = lx.i as u32;
        let c = lx.b[lx.i];
        let cmd = if c.is_ascii_alphabetic() {
            if !b"MmLlHhVvCcSsQqTtAaZz".contains(&c) {
                return Err(PathError::UnknownCommand { at, cmd: c });
            }
            if first && c != b'M' && c != b'm' {
                return Err(PathError::NoCurrentPoint { at });
            }
            lx.i += 1;
            c
        } else if lx.at_number() {
            match active {
                Some(a) => a,
                None => return Err(PathError::Syntax { at }),
            }
        } else {
            return Err(PathError::UnknownCommand { at, cmd: c });
        };
        first = false;
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { [0.0, 0.0] };
        let off = |p: [f64; 2]| [p[0] + base[0], p[1] + base[1]];
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = off(lx.pair()?);
                bld.move_to(p[0], p[1]);
                cur = p;
                start = p;
                prev = Prev::Other;
                active = Some(if rel { b'l' } else { b'L' });
                continue;
            }
            b'L' => {
                let p = off(lx.pair()?);
                bld.line_to(p[0], p[1]);
                cur = p;
                prev = Prev::Other;
            }
            b'H' => {
                let x = lx.number()? + base[0];
                bld.line_to(x, cur[1]);
                cur[0] = x;
                prev = Prev::Other;
            }
            b'V' => {
                let y = lx.number()? + base[1];
                bld.line_to(cur[0], y);
                cur[1] = y;
                prev = Prev::Other;
            }
            b'C' => {
                let c1 = off(lx.pair()?);
                let c2 = off(lx.pair()?);
                let p = off(lx.pair()?);
                bld.cubic_to(c1[0], c1[1], c2[0], c2[1], p[0], p[1]);
                cur = p;
                prev = Prev::Cubic(c2);
            }
            b'S' => {
                let c2 = off(lx.pair()?);
                let p = off(lx.pair()?);
                let c1 = match prev {
                    Prev::Cubic(q) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                    _ => cur,
                };
                bld.cubic_to(c1[0], c1[1], c2[0], c2[1], p[0], p[1]);
                cur = p;
                prev = Prev::Cubic(c2);
            }
            b'Q' => {
                let c = off(lx.pair()?);
                let p = off(lx.pair()?);
                bld.quad_to(c[0], c[1], p[0], p[1]);
                cur = p;
                prev = Prev::Quad(c);
            }
            b'T' => {
                let p = off(lx.pair()?);
                let c = match prev {
                    Prev::Quad(q) => [2.0 * cur[0] - q[0], 2.0 * cur[1] - q[1]],
                    _ => cur,
                };
                bld.quad_to(c[0], c[1], p[0], p[1]);
                cur = p;
                prev = Prev::Quad(c);
            }
            b'A' => {
                let rx = lx.number()?;
                let ry = lx.number()?;
                let rot = lx.number()?;
                let large = lx.flag()?;
                let sweep = lx.flag()?;
                let p = off(lx.pair()?);
                bld.arc_to(rx, ry, rot, large, sweep, p[0], p[1]);
                cur = p;
                prev = Prev::Other;
            }
            _ => {
                // Z / z
                bld.close();
                cur = start;
                prev = Prev::Other;
                active = None;
                continue;
            }
        }
        active = Some(cmd);
    }
    Ok(bld)
}
