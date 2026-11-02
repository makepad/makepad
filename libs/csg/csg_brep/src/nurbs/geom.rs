//! NLib Euclidean geometry: vector arithmetic, lines, planes, min-max boxes, line/line
//! and line/plane intersection, point projection and distances, rotation and
//! axis-to-axis transform matrices, control-point reflection and collinearity, and
//! the bounding boxes of point polygons and nets.
//!
//! Upstream: `source/SMLib/src/NL_Geom.cpp`, `source/SMLib/src/NL_FrameBasic.cpp`
//! (declarations in `source/SMLib/inc/NL_Geom.h`, `NL_FrameBasic.h`; structures in
//! `NL_Geometry.h` and `NL_DataStruct.h`; tolerances in `src/NL_Globals.cpp`).
//!
//! | Rust                                   | Upstream                              |
//! |----------------------------------------|---------------------------------------|
//! | [`LineSeg`]                            | `NL_LINESEG`                          |
//! | [`LineSeg::new`]                       | `N_CreateLineStartDirVector`          |
//! | field [`LineSeg::bounded`]             | `N_LineSetBoundingFlag`               |
//! | fields of [`LineSeg`]                  | `N_LineGetData`                       |
//! | [`Plane`], [`Plane::new`]              | `NL_PLANE`, `N_CreatePlanePtNormal`   |
//! | fields of [`Plane`]                    | `N_PlaneGetData`, `N_PlaneGetNormal`  |
//! | [`MinMaxBox`], [`MinMaxBox::new`]      | `NL_MINMAXBOX`, `N_BBoxDefine`        |
//! | fields of [`MinMaxBox`]                | `N_GetBBoxData`                       |
//! | [`MinMaxBox::diagonal`]                | `N_BBoxGetDiagonal`                   |
//! | [`Interval`], [`Interval::new`]        | `NL_INTERVAL`, `N_CreateInterval`     |
//! | fields of [`Interval`]                 | `N_IntervalGetData`                   |
//! | [`Rectangle`], [`Rectangle::new`]      | `NL_RECTANGLE`, `N_CreateRectangle`   |
//! | fields of [`Rectangle`]                | `N_RectangleGetData`                  |
//! | `&[Vec3]` / `&[CPoint]` slices         | `NL_EPOLYGON` / `NL_CPOLYGON`, `N_EPolygonFromPts`, `N_EPolygonGetPts`, `N_CPolygonFromCPts`, `N_CPolygonGetCPts` |
//! | `&[Vec<Vec3>]` / `&[Vec<CPoint>]`      | `NL_ENET` / `NL_CNET`, `N_ENetFromPts`, `N_ENetGetPts`, `N_CNetFromCPts` |
//! | [`polygon_bbox`]                       | `N_PolygonGetBBox`                    |
//! | [`net_bbox`]                           | `N_ENetGetBBox`                       |
//! | [`Vec3::dot`]                          | `N_VectorDot`, `N_VectorDotRef`       |
//! | [`Vec3::cross`]                        | `N_VectorCross`, `N_VectorCrossRef`   |
//! | `alpha * v` / `v * alpha`              | `N_VectorScale`, `N_VectorScaleRef`   |
//! | `-v`                                   | `N_VectorReverse`, `N_VectorReverseInPlace` |
//! | `a + b`                                | `N_VectorSum`                         |
//! | [`combine2_pts`]                       | `N_VectorCombine`                     |
//! | `a - b` / `pe - ps`                    | `N_VectorDiff` / `N_VectorDir`        |
//! | [`vector_diff_cpts`]                   | `N_VectorDiffCPts`                    |
//! | [`Vec3::new`], `Copy`                  | `N_VectorCreate`, `N_VectorCopy`      |
//! | [`Vec3::length`]                       | `N_VectorMagnitude`, `N_VectorMagnitudeRef` |
//! | [`normalize`]                          | `N_VectorNormalize`, `N_VectorNormalizeRef` |
//! | [`pt_along_vector`]                    | `N_VectorPtAlongVector`               |
//! | [`vectors_cos_angle`]                  | `N_VectorsCosAngle`                   |
//! | [`vectors_angle`]                      | `N_VectorsAngle`                      |
//! | [`isect_line_line`], [`LineLineIsect`] | `N_IsectLineLine`                     |
//! | [`isect_line_plane`], [`LinePlaneIsect`] | `N_IsectLinePlane`                  |
//! | [`project_pt_line`]                    | `N_ProjectPtLine`                     |
//! | [`project_pt_plane`]                   | `N_ProjectPtPlane`                    |
//! | [`dist_pt_pt`] / [`dist_sq_pt_pt`]     | `N_DistPtPt` / `N_DistSqPtPt`         |
//! | `==` on [`Vec3`]                       | `N_PtsAreEqual`                       |
//! | [`dist_pt_line_seg`], [`dist_pt_line_seg_tol`] | `N_DistPtLineSeg`             |
//! | [`dist_perp_pt_line_seg`], [`dist_perp_pt_line_seg_tol`] | `N_DistPerpPtLineSeg` |
//! | [`dist_polygon`] / [`dist_cpolygon`]   | `N_DistPolygon` / `N_DistCPolygon`    |
//! | [`dist_cpt_cpt`] / [`dist_cpt_cpt_homo`] | `N_DistCptCpt` / `N_DistCptCptHomo` |
//! | [`dist_pt_inf_line`]                   | `N_DistPtInfLine`                     |
//! | [`dist_signed_pt_plane`]               | `N_DistSignedPtPlane`                 |
//! | [`rotation_matrix_about_axis`]         | `N_CreateRotationMatrixAboutAxis`     |
//! | [`transform_matrix_from_axes`], [`Frame`] | `N_CreateTransformMatrixFromAxes`  |
//! | [`cpt_reflect`]                        | `N_CptReflect`                        |
//! | [`cpts_are_colinear`], [`cpts_are_colinear_tol`], [`ColinearDim`] | `N_CPtsAreColinear` (`NL_THREED` / `NL_FOURD`) |
//! | [`plane_pt_normal_to_implicit`]        | `N_PlanePtNormalToImplicit`           |
//! | [`PI`], [`LINE_TOL`]                   | `NL_PI`, `NL_LTOL`                    |
//!
//! Deviations:
//! - Out-params become return values. Where upstream leaves an output unwritten the
//!   Rust result has no value for it: [`isect_line_line`] returns `None` when the
//!   lines are parallel or miss a bounded segment; [`isect_line_plane`] and
//!   [`project_pt_line`] return a [`LinePlaneIsect`] that carries the parameter only
//!   when upstream sets it; [`dist_perp_pt_line_seg`] returns `None` where upstream
//!   writes `-1.0`.
//! - [`project_pt_plane`] reads an intersection point that upstream leaves
//!   uninitialised when the (unbounded) line along the normal does not hit the plane.
//!   That cannot happen for finite input (the line direction and the plane normal are
//!   the same unit vector); the Rust function returns `InvalidInput` there instead of
//!   reading garbage. ([`dist_pt_inf_line`] starts its point at the origin, as upstream
//!   does.)
//! - [`dist_pt_line_seg`] leaves the distance unwritten when the position tests are
//!   all false (NaN coordinates); the Rust function returns NaN.
//!   [`cpts_are_colinear`] (3D) ignores the error of `N_DistPtLineSeg` and then reads
//!   that unwritten distance; the Rust function answers `false` in both cases.
//! - `N_DistPtLineSeg`, `N_DistPerpPtLineSeg` and `N_CPtsAreColinear` read the
//!   global `NL_MTOL`, which `N_SrfIsClosedSmooth` overrides temporarily. The plain
//!   functions use [`MODEL_TOL`]; the `*_tol` variants take it as an argument.
//! - `N_CPtsAreColinear` answers `NL_NO` for a `dim` other than `NL_THREED` or
//!   `NL_FOURD`; [`ColinearDim`] has no other values.
//! - The rotation and transform matrices are returned as new 4x4 ([`RealMatrix`]
//!   `n = m = 3`, full) matrices; upstream writes into the caller's matrix
//!   (allocating it when null). The `NL_STACKS` arguments have no counterpart.
//!   The eight point/axis arguments of `N_CreateTransformMatrixFromAxes` are two
//!   [`Frame`]s.
//! - Polygons and nets are slices; an empty polygon or net is `InvalidInput` for the
//!   bounding boxes (upstream reads `P[0]`).
//! - Errors are [`SolidError::Nlib`] values with upstream's `NL_*_ERR` number
//!   (`NL_GEO_ERR` for a zero-length vector, `NL_NUM_ERR` for a bad division).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::math::{
    combine2_pts, float_op_is_bad, CPoint, FloatOp, MatrixType, RealMatrix, MODEL_TOL, NOW, NOZ,
};

/// `NL_PI` (`3.1415926535897932`, which rounds to [`core::f64::consts::PI`]).
pub const PI: f64 = core::f64::consts::PI;
/// `NL_LTOL`: parallel line tolerance of [`isect_line_plane`].
pub const LINE_TOL: f64 = 1.0e-09;
/// `NL_ZCTL`: zero coefficient tolerance (parallelism of [`isect_line_line`],
/// 4D collinearity of [`cpts_are_colinear`]).
const ZERO_COEFFICIENT_TOL: f64 = 1.0e-12;
/// `NOZ_TOL` of `N_CptReflect`.
const REFLECT_NOZ_TOL: f64 = 1.0e-6;

const OP_NORMALIZE: &str = "nurbs::geom::normalize";
const OP_COS_ANGLE: &str = "nurbs::geom::vectors_cos_angle";
const OP_ANGLE: &str = "nurbs::geom::vectors_angle";
const OP_ISECT_LL: &str = "nurbs::geom::isect_line_line";
const OP_ISECT_LP: &str = "nurbs::geom::isect_line_plane";
const OP_PROJECT_PLANE: &str = "nurbs::geom::project_pt_plane";
const OP_DIST_LSEG: &str = "nurbs::geom::dist_pt_line_seg";
const OP_DIST_PERP: &str = "nurbs::geom::dist_perp_pt_line_seg";
const OP_BBOX: &str = "nurbs::geom::polygon_bbox";
const OP_NET_BBOX: &str = "nurbs::geom::net_bbox";
const OP_ROTATION: &str = "nurbs::geom::rotation_matrix_about_axis";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

// ---- structures -------------------------------------------------------------------

/// A line through `p` with direction and length `v` (upstream `NL_LINESEG`).
///
/// A bounded line is the segment `p + t*v`, `0 <= t <= 1`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LineSeg {
    /// Start point.
    pub p: Vec3,
    /// Direction vector (its magnitude is the segment length).
    pub v: Vec3,
    /// `true` for a segment (`NL_TRUE`/`NL_BOUNDED`), `false` for an infinite line.
    pub bounded: bool,
}

impl LineSeg {
    /// Line from start point, direction and bounding flag
    /// (`N_CreateLineStartDirVector`).
    #[must_use]
    pub const fn new(p: Vec3, v: Vec3, bounded: bool) -> Self {
        Self { p, v, bounded }
    }
}

/// A plane through `p` with normal `n` (upstream `NL_PLANE`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Plane {
    /// Any point on the plane.
    pub p: Vec3,
    /// Plane normal (not necessarily unit).
    pub n: Vec3,
}

impl Plane {
    /// Plane from a point and a normal (`N_CreatePlanePtNormal`).
    #[must_use]
    pub const fn new(p: Vec3, n: Vec3) -> Self {
        Self { p, n }
    }
}

/// An axis-aligned min-max box (upstream `NL_MINMAXBOX`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MinMaxBox {
    /// Minimum x.
    pub xl: f64,
    /// Maximum x.
    pub xr: f64,
    /// Minimum y.
    pub yb: f64,
    /// Maximum y.
    pub yt: f64,
    /// Minimum z.
    pub zn: f64,
    /// Maximum z.
    pub zf: f64,
}

impl MinMaxBox {
    /// Box from its six bounds (`N_BBoxDefine`).
    #[must_use]
    pub const fn new(xl: f64, xr: f64, yb: f64, yt: f64, zn: f64, zf: f64) -> Self {
        Self {
            xl,
            xr,
            yb,
            yt,
            zn,
            zf,
        }
    }

    /// Length of the diagonal from `(xl, yb, zn)` to `(xr, yt, zf)`
    /// (`N_BBoxGetDiagonal`).
    #[must_use]
    pub fn diagonal(&self) -> f64 {
        let pll = Vec3::new(self.xl, self.yb, self.zn);
        let pur = Vec3::new(self.xr, self.yt, self.zf);
        dist_pt_pt(pll, pur)
    }
}

/// A parameter interval `[ul, ur]` (upstream `NL_INTERVAL`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Interval {
    /// Left end.
    pub ul: f64,
    /// Right end.
    pub ur: f64,
}

impl Interval {
    /// Interval from its ends (`N_CreateInterval`).
    #[must_use]
    pub const fn new(ul: f64, ur: f64) -> Self {
        Self { ul, ur }
    }
}

/// A parameter rectangle `[ul, ur] x [vb, vt]` (upstream `NL_RECTANGLE`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rectangle {
    /// Left `u`.
    pub ul: f64,
    /// Right `u`.
    pub ur: f64,
    /// Bottom `v`.
    pub vb: f64,
    /// Top `v`.
    pub vt: f64,
}

impl Rectangle {
    /// Rectangle from its bounds (`N_CreateRectangle`).
    #[must_use]
    pub const fn new(ul: f64, ur: f64, vb: f64, vt: f64) -> Self {
        Self { ul, ur, vb, vt }
    }
}

// ---- bounding boxes ---------------------------------------------------------------

/// Running min-max update, in upstream's comparison order.
fn grow(b: &mut MinMaxBox, p: Vec3) {
    let (x, y, z) = (p.x, p.y, p.z);
    if x < b.xl {
        b.xl = x;
    }
    if x > b.xr {
        b.xr = x;
    }
    if y < b.yb {
        b.yb = y;
    }
    if y > b.yt {
        b.yt = y;
    }
    if z < b.zn {
        b.zn = z;
    }
    if z > b.zf {
        b.zf = z;
    }
}

fn start_box(p: Vec3) -> MinMaxBox {
    MinMaxBox::new(p.x, p.x, p.y, p.y, p.z, p.z)
}

/// Min-max box of a point polygon (`N_PolygonGetBBox`).
pub fn polygon_bbox(pts: &[Vec3]) -> Result<MinMaxBox> {
    let (first, rest) = pts.split_first().ok_or(SolidError::InvalidInput {
        operation: OP_BBOX,
        reason: "empty polygon",
    })?;
    let mut b = start_box(*first);
    for p in rest {
        grow(&mut b, *p);
    }
    Ok(b)
}

/// Min-max box of a point net, rows in order (`N_ENetGetBBox`).
pub fn net_bbox(net: &[Vec<Vec3>]) -> Result<MinMaxBox> {
    let first = net
        .first()
        .and_then(|row| row.first())
        .ok_or(SolidError::InvalidInput {
            operation: OP_NET_BBOX,
            reason: "empty net",
        })?;
    let mut b = start_box(*first);
    for row in net {
        for p in row {
            grow(&mut b, *p);
        }
    }
    Ok(b)
}

// ---- vectors ----------------------------------------------------------------------

/// Difference `P - Q` of the Euclidean images of two control points
/// (`N_VectorDiffCPts`).
#[must_use]
pub fn vector_diff_cpts(pw: CPoint, qw: CPoint) -> Vec3 {
    pw.to_euclid() - qw.to_euclid()
}

/// `P + alpha*V` (`N_VectorPtAlongVector`).
#[must_use]
pub fn pt_along_vector(p: Vec3, alpha: f64, v: Vec3) -> Vec3 {
    Vec3::new(p.x + alpha * v.x, p.y + alpha * v.y, p.z + alpha * v.z)
}

/// Unit vector and magnitude of `a` (`N_VectorNormalize`, `N_VectorNormalizeRef`).
///
/// Errors with `NL_GEO_ERR` when `1/|a|` is out of range (`|a|` zero, subnormal or
/// huge).
pub fn normalize(a: Vec3) -> Result<(Vec3, f64)> {
    let d = a.length();
    if float_op_is_bad(1.0, d, FloatOp::Division) {
        return Err(nlib(OP_NORMALIZE, NlibCode::Geometry));
    }
    let invd = 1.0 / d;
    Ok((Vec3::new(invd * a.x, invd * a.y, invd * a.z), d))
}

/// Cosine of the angle between `a` and `b`, clamped to `[-1, 1]`
/// (`N_VectorsCosAngle`). Errors with `NL_GEO_ERR` for a bad division.
pub fn vectors_cos_angle(a: Vec3, b: Vec3) -> Result<f64> {
    let dot = a.x * b.x + a.y * b.y + a.z * b.z;
    let ma = a.x * a.x + a.y * a.y + a.z * a.z;
    let mb = b.x * b.x + b.y * b.y + b.z * b.z;
    let den = (ma * mb).sqrt();
    if float_op_is_bad(dot, den, FloatOp::Division) {
        return Err(nlib(OP_COS_ANGLE, NlibCode::Geometry));
    }
    // Upstream's two ifs; `clamp` gives the same result, NaN included.
    Ok((dot / den).clamp(-1.0, 1.0))
}

/// Angle between `a` and `b` in degrees (`N_VectorsAngle`). Errors with
/// `NL_GEO_ERR` for a bad division.
pub fn vectors_angle(a: Vec3, b: Vec3) -> Result<f64> {
    let dot = a.dot(b);
    let ma = a.length();
    let mb = b.length();
    if float_op_is_bad(dot, ma * mb, FloatOp::Division) {
        return Err(nlib(OP_ANGLE, NlibCode::Geometry));
    }
    // Upstream's two ifs; `clamp` gives the same result, NaN included.
    let arg = (dot / (ma * mb)).clamp(-1.0, 1.0);
    let rad = arg.acos();
    Ok(180.0 * (rad / PI))
}

// ---- intersections and projections -------------------------------------------------

/// The intersection of two lines found by [`isect_line_line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineLineIsect {
    /// Midpoint of the closest points of the two lines.
    pub p: Vec3,
    /// Parameter on the first line (`p1 + t1*v1`).
    pub t1: f64,
    /// Parameter on the second line.
    pub t2: f64,
}

/// Closest-point "intersection" of two lines (`N_IsectLineLine`): `None` when the
/// lines are parallel or a bounded line's parameter falls outside `[0, 1]`.
///
/// Errors: `NL_GEO_ERR` for a zero direction, `NL_NUM_ERR` for a bad division.
pub fn isect_line_line(ls1: LineSeg, ls2: LineSeg) -> Result<Option<LineLineIsect>> {
    let (p1, v1, bd1) = (ls1.p, ls1.v, ls1.bounded);
    let (p2, v2, bd2) = (ls2.p, ls2.v, ls2.bounded);

    let (v1, m1) = normalize(v1)?;
    let (v2, m2) = normalize(v2)?;

    let p21 = p2 - p1;
    let a00 = v1.dot(v1);
    let a01 = v1.dot(v2);
    let a11 = v2.dot(v2);
    let b0 = v1.dot(p21);
    let b1 = v2.dot(p21);

    let den = a00 * a11 - a01 * a01;
    if den.abs() < ZERO_COEFFICIENT_TOL {
        return Ok(None);
    }

    let numa = b0 * a11 - b1 * a01;
    let numb = b0 * a01 - b1 * a00;
    if float_op_is_bad(numa, den, FloatOp::Division)
        || float_op_is_bad(numb, den, FloatOp::Division)
    {
        return Err(nlib(OP_ISECT_LL, NlibCode::Numerical));
    }
    let alpha = numa / den;
    let beta = numb / den;

    if float_op_is_bad(alpha, m1, FloatOp::Division) || float_op_is_bad(beta, m2, FloatOp::Division)
    {
        return Err(nlib(OP_ISECT_LL, NlibCode::Numerical));
    }
    let u1 = alpha / m1;
    let u2 = beta / m2;

    if bd1 && (u1 < 0.0 || u1 > 1.0) {
        return Ok(None);
    }
    if bd2 && (u2 < 0.0 || u2 > 1.0) {
        return Ok(None);
    }

    let q1 = pt_along_vector(p1, alpha, v1);
    let q2 = pt_along_vector(p2, beta, v2);
    Ok(Some(LineLineIsect {
        p: combine2_pts(0.5, q1, 0.5, q2),
        t1: u1,
        t2: u2,
    }))
}

/// Result of [`isect_line_plane`] / [`project_pt_line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinePlaneIsect {
    /// The line is parallel to the plane (`|V.N| < NL_LTOL`, unit vectors).
    Parallel,
    /// The line is bounded and the intersection parameter `t` is outside `[0, 1]`.
    OutOfBounds {
        /// Line parameter of the intersection.
        t: f64,
    },
    /// The intersection point `p` at line parameter `t`.
    Hit {
        /// Intersection point.
        p: Vec3,
        /// Line parameter (`lsg.p + t*lsg.v`).
        t: f64,
    },
}

/// Intersection of a line and a plane (`N_IsectLinePlane`).
///
/// Errors: `NL_GEO_ERR` for a zero direction or normal, `NL_NUM_ERR` for a bad
/// division.
pub fn isect_line_plane(lsg: LineSeg, pln: Plane) -> Result<LinePlaneIsect> {
    let (q, v, bnd) = (lsg.p, lsg.v, lsg.bounded);
    let (r, n) = (pln.p, pln.n);

    let (v, mv) = normalize(v)?;
    let (n, _mn) = normalize(n)?;

    let pl = r - q;
    let den = v.dot(n);
    if den.abs() < LINE_TOL {
        return Ok(LinePlaneIsect::Parallel);
    }
    let num = pl.dot(n);
    if float_op_is_bad(num, den, FloatOp::Division) {
        return Err(nlib(OP_ISECT_LP, NlibCode::Numerical));
    }
    let alpha = num / den;
    if float_op_is_bad(alpha, mv, FloatOp::Division) {
        return Err(nlib(OP_ISECT_LP, NlibCode::Numerical));
    }
    let t = alpha / mv;

    if bnd && (t < 0.0 || t > 1.0) {
        return Ok(LinePlaneIsect::OutOfBounds { t });
    }
    Ok(LinePlaneIsect::Hit {
        p: pt_along_vector(q, alpha, v),
        t,
    })
}

/// Projection of `p` onto a line: the intersection of the line with the plane
/// through `p` normal to it (`N_ProjectPtLine`).
pub fn project_pt_line(lsg: LineSeg, p: Vec3) -> Result<LinePlaneIsect> {
    let nor = Plane::new(p, lsg.v);
    isect_line_plane(lsg, nor)
}

/// Orthogonal projection of `p` onto a plane (`N_ProjectPtPlane`).
pub fn project_pt_plane(pln: Plane, p: Vec3) -> Result<Vec3> {
    let lsg = LineSeg::new(p, pln.n, false);
    match isect_line_plane(lsg, pln)? {
        LinePlaneIsect::Hit { p: q, .. } => Ok(q),
        LinePlaneIsect::Parallel | LinePlaneIsect::OutOfBounds { .. } => {
            Err(SolidError::InvalidInput {
                operation: OP_PROJECT_PLANE,
                reason: "normal line misses the plane (upstream reads an unset point)",
            })
        }
    }
}

// ---- distances --------------------------------------------------------------------

/// Distance between two points (`N_DistPtPt`).
#[must_use]
pub fn dist_pt_pt(p: Vec3, q: Vec3) -> f64 {
    dist_sq_pt_pt(p, q).sqrt()
}

/// Squared distance between two points (`N_DistSqPtPt`).
#[must_use]
pub fn dist_sq_pt_pt(p: Vec3, q: Vec3) -> f64 {
    (p.x - q.x) * (p.x - q.x) + (p.y - q.y) * (p.y - q.y) + (p.z - q.z) * (p.z - q.z)
}

/// Distance from `p` to the segment `ab` (`N_DistPtLineSeg` with the default
/// `NL_MTOL`, [`MODEL_TOL`]).
pub fn dist_pt_line_seg(p: Vec3, a: Vec3, b: Vec3) -> Result<f64> {
    dist_pt_line_seg_tol(p, a, b, MODEL_TOL)
}

/// Distance from `p` to the segment `ab`; a segment shorter than `mtol` counts as its
/// midpoint (`N_DistPtLineSeg` with `NL_MTOL = mtol`). NaN where upstream leaves the
/// distance unwritten. Errors with `NL_NUM_ERR` for a bad division.
pub fn dist_pt_line_seg_tol(p: Vec3, a: Vec3, b: Vec3, mtol: f64) -> Result<f64> {
    let dotab = dist_pt_pt(a, b);
    if dotab < mtol {
        let t = (a + b) * 0.5;
        return Ok(dist_pt_pt(p, t));
    }

    let ba = b - a;
    let pa = p - a;
    let bp = b - p;
    let dotap = pa.dot(ba);
    let dotbp = bp.dot(ba);

    if dotap >= 0.0 && dotbp >= 0.0 {
        let num = pa.cross(ba).length();
        let den = dotab;
        if float_op_is_bad(num, den, FloatOp::Division) {
            return Err(nlib(OP_DIST_LSEG, NlibCode::Numerical));
        }
        Ok(num / den)
    } else if dotap < 0.0 {
        Ok(dist_pt_pt(p, a))
    } else if dotbp < 0.0 {
        Ok(dist_pt_pt(p, b))
    } else {
        Ok(f64::NAN)
    }
}

/// Perpendicular distance from `q` to the segment `ab`, `None` when the foot of the
/// perpendicular is outside the segment (`N_DistPerpPtLineSeg` with the default
/// `NL_MTOL`).
pub fn dist_perp_pt_line_seg(q: Vec3, a: Vec3, b: Vec3) -> Result<Option<f64>> {
    dist_perp_pt_line_seg_tol(q, a, b, MODEL_TOL)
}

/// [`dist_perp_pt_line_seg`] with `NL_MTOL = mtol`: a segment shorter than `mtol`
/// counts as its midpoint. Errors with `NL_NUM_ERR` for a bad division.
pub fn dist_perp_pt_line_seg_tol(q: Vec3, a: Vec3, b: Vec3, mtol: f64) -> Result<Option<f64>> {
    let mut dotab = dist_pt_pt(a, b);
    if dotab < mtol {
        let t = (a + b) * 0.5;
        return Ok(Some(dist_pt_pt(q, t)));
    }

    let ba = b - a;
    let qa = q - a;
    let bq = b - q;
    let dotaq = qa.dot(ba);
    let dotbq = bq.dot(ba);

    if dotaq < 0.0 || dotbq < 0.0 {
        return Ok(None);
    }

    dotab = ba.dot(ba);
    let num = qa.cross(ba).length();
    if dotab < 0.0 {
        dotab = 0.0;
    }
    let den = dotab.sqrt();
    if float_op_is_bad(num, den, FloatOp::Division) {
        return Err(nlib(OP_DIST_PERP, NlibCode::Numerical));
    }
    Ok(Some(num / den))
}

/// Length of a point polygon (`N_DistPolygon`).
#[must_use]
pub fn dist_polygon(pts: &[Vec3]) -> f64 {
    let mut len = 0.0;
    for w in pts.windows(2) {
        if let [a, b] = w {
            len += dist_pt_pt(*a, *b);
        }
    }
    len
}

/// Length of a control polygon, measured between the Euclidean images
/// (`N_DistCPolygon`).
#[must_use]
pub fn dist_cpolygon(pw: &[CPoint]) -> f64 {
    let mut len = 0.0;
    for w in pw.windows(2) {
        if let [a, b] = w {
            len += dist_cpt_cpt(*a, *b);
        }
    }
    len
}

/// Distance between the Euclidean images of two control points (`N_DistCptCpt`).
#[must_use]
pub fn dist_cpt_cpt(pw: CPoint, qw: CPoint) -> f64 {
    dist_pt_pt(pw.to_euclid(), qw.to_euclid())
}

/// Distance between two control points in homogeneous space, skipping `z` / `w`
/// unless both have it (`N_DistCptCptHomo`).
#[must_use]
pub fn dist_cpt_cpt_homo(pw: CPoint, qw: CPoint) -> f64 {
    let mut disc = (pw.x - qw.x) * (pw.x - qw.x) + (pw.y - qw.y) * (pw.y - qw.y);
    if pw.z != NOZ && qw.z != NOZ {
        disc += (pw.z - qw.z) * (pw.z - qw.z);
    }
    if pw.w != NOW && qw.w != NOW {
        disc += (pw.w - qw.w) * (pw.w - qw.w);
    }
    disc.sqrt()
}

/// Distance from `p` to the infinite line through `lsg` (`N_DistPtInfLine`).
pub fn dist_pt_inf_line(lsg: LineSeg, p: Vec3) -> Result<f64> {
    let lsg = LineSeg {
        bounded: false,
        ..lsg
    };
    // `Q` starts at the origin: a projection that misses the line leaves it there
    let q = match project_pt_line(lsg, p)? {
        LinePlaneIsect::Hit { p: q, .. } => q,
        LinePlaneIsect::Parallel | LinePlaneIsect::OutOfBounds { .. } => Vec3::new(0.0, 0.0, 0.0),
    };
    Ok(dist_pt_pt(p, q))
}

/// `a*x + b*y + c*z + d` (`N_DistSignedPtPlane`).
#[must_use]
pub fn dist_signed_pt_plane(a: f64, b: f64, c: f64, d: f64, p: Vec3) -> f64 {
    a * p.x + b * p.y + c * p.z + d
}

// ---- matrices ---------------------------------------------------------------------

/// Writes `rows` (4x4, row-major) into a new full 4x4 matrix.
fn matrix4(rows: [[f64; 4]; 4], op: &'static str) -> Result<RealMatrix> {
    let mut m = RealMatrix::new(3, 3, MatrixType::Full, 3)?;
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            m.set(i, j, *v).map_err(|_| SolidError::InvalidInput {
                operation: op,
                reason: "matrix storage",
            })?;
        }
    }
    Ok(m)
}

/// 4x4 matrix rotating by `al` degrees about the axis through `p` along `v`
/// (`N_CreateRotationMatrixAboutAxis`). Errors with `NL_GEO_ERR` for a zero axis.
pub fn rotation_matrix_about_axis(p: Vec3, v: Vec3, al: f64) -> Result<RealMatrix> {
    let (v, _) = normalize(v)?;
    let (xp, yp, zp) = (p.x, p.y, p.z);
    let (xv, yv, zv) = (v.x, v.y, v.z);

    let rad = (al / 180.0) * PI;
    let cosal = rad.cos();
    let sinal = rad.sin();
    let omcos = 1.0 - cosal;

    let vx2 = xv * xv;
    let vy2 = yv * yv;
    let vz2 = zv * zv;
    let vxy = xv * yv;
    let vyz = yv * zv;
    let vzx = zv * xv;

    let r00 = vx2 + cosal * (1.0 - vx2);
    let r01 = vxy * omcos - zv * sinal;
    let r02 = vzx * omcos + yv * sinal;
    let r03 = xp - (xp * r00 + yp * r01 + zp * r02);

    let r10 = vxy * omcos + zv * sinal;
    let r11 = vy2 + cosal * (1.0 - vy2);
    let r12 = vyz * omcos - xv * sinal;
    let r13 = yp - (xp * r10 + yp * r11 + zp * r12);

    let r20 = vzx * omcos - yv * sinal;
    let r21 = vyz * omcos + xv * sinal;
    let r22 = vz2 + cosal * (1.0 - vz2);
    let r23 = zp - (xp * r20 + yp * r21 + zp * r22);

    matrix4(
        [
            [r00, r01, r02, r03],
            [r10, r11, r12, r13],
            [r20, r21, r22, r23],
            [0.0, 0.0, 0.0, 1.0],
        ],
        OP_ROTATION,
    )
}

/// A coordinate frame: an origin and three axes (the `O, X, Y, Z` argument groups of
/// `N_CreateTransformMatrixFromAxes`). The axes are used as given, not normalised.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    /// Origin.
    pub origin: Vec3,
    /// X axis.
    pub x: Vec3,
    /// Y axis.
    pub y: Vec3,
    /// Z axis.
    pub z: Vec3,
}

impl Frame {
    /// Frame from its origin and axes.
    #[must_use]
    pub const fn new(origin: Vec3, x: Vec3, y: Vec3, z: Vec3) -> Self {
        Self { origin, x, y, z }
    }
}

/// 4x4 matrix mapping the frame `from = (o1; x1, y1, z1)` onto
/// `to = (o2; x2, y2, z2)`: `T(o2) * R2 * R1^T * T(-o1)`
/// (`N_CreateTransformMatrixFromAxes`; the axes are used as given, not normalised).
pub fn transform_matrix_from_axes(from: Frame, to: Frame) -> Result<RealMatrix> {
    const OP: &str = "nurbs::geom::transform_matrix_from_axes";
    let Frame {
        origin: o1,
        x: x1,
        y: y1,
        z: z1,
    } = from;
    let Frame {
        origin: o2,
        x: x2,
        y: y2,
        z: z2,
    } = to;
    // Inverse rotation: X1, Y1, Z1 as rows.
    let rm1 = matrix4(
        [
            [x1.x, x1.y, x1.z, 0.0],
            [y1.x, y1.y, y1.z, 0.0],
            [z1.x, z1.y, z1.z, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        OP,
    )?;
    // Forward rotation: X2, Y2, Z2 as columns.
    let rmi = matrix4(
        [
            [x2.x, y2.x, z2.x, 0.0],
            [x2.y, y2.y, z2.y, 0.0],
            [x2.z, y2.z, z2.z, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        OP,
    )?;
    // Inverse translation by -O1.
    let rt1 = matrix4(
        [
            [1.0, 0.0, 0.0, -o1.x],
            [0.0, 1.0, 0.0, -o1.y],
            [0.0, 0.0, 1.0, -o1.z],
            [0.0, 0.0, 0.0, 1.0],
        ],
        OP,
    )?;
    // Forward translation by +O2.
    let rt2 = matrix4(
        [
            [1.0, 0.0, 0.0, o2.x],
            [0.0, 1.0, 0.0, o2.y],
            [0.0, 0.0, 1.0, o2.z],
            [0.0, 0.0, 0.0, 1.0],
        ],
        OP,
    )?;
    let rmw = rt2.multiply(&rmi)?;
    let rt2 = rmw.multiply(&rm1)?;
    rt2.multiply(&rt1)
}

// ---- control points ---------------------------------------------------------------

/// Reflection of a control point in a plane (`N_CptReflect`): the Euclidean image is
/// mirrored and re-weighted with `Pw`'s weight; a 2D point stays 2D when the plane
/// normal has no `z` component (within `1e-6`).
pub fn cpt_reflect(pw: CPoint, pln: Plane) -> Result<CPoint> {
    let p = pw.to_euclid();
    let q = project_pt_plane(pln, p)?;
    let r = q - p;
    let q = q + r;

    let (mut x, mut y, mut z) = (q.x, q.y, q.z);
    let (wz, w) = (pw.z, pw.w);
    if w != NOW {
        x *= w;
        y *= w;
        z *= w;
    }

    let (r, _) = normalize(pln.n)?;
    let vz = r.z;
    if wz == NOZ && vz.abs() <= REFLECT_NOZ_TOL {
        z = NOZ;
    }
    Ok(CPoint::new(x, y, z, w))
}

/// Space in which [`cpts_are_colinear`] tests (`NL_THREED` / `NL_FOURD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColinearDim {
    /// Euclidean images (`NL_THREED` = 3).
    Three,
    /// Weighted coordinates without the weight (`NL_FOURD` = 4).
    Four,
}

impl ColinearDim {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            Self::Three => 3,
            Self::Four => 4,
        }
    }
}

/// True when three control points are collinear (`N_CPtsAreColinear` with the
/// default `NL_MTOL`).
#[must_use]
pub fn cpts_are_colinear(pw: CPoint, qw: CPoint, rw: CPoint, dim: ColinearDim) -> bool {
    cpts_are_colinear_tol(pw, qw, rw, dim, MODEL_TOL)
}

/// [`cpts_are_colinear`] with `NL_MTOL = mtol`. In 3D, `Q` is within `2*mtol` of
/// the segment `PR`; in 4D, the unit directions `Q-P` and `R-Q` are parallel
/// (`|1 - cos^2| < NL_ZCTL`).
#[must_use]
pub fn cpts_are_colinear_tol(
    pw: CPoint,
    qw: CPoint,
    rw: CPoint,
    dim: ColinearDim,
    mtol: f64,
) -> bool {
    match dim {
        ColinearDim::Three => {
            let p = pw.to_euclid();
            let q = qw.to_euclid();
            let r = rw.to_euclid();
            matches!(dist_pt_line_seg_tol(q, p, r, mtol), Ok(d) if d <= 2.0 * mtol)
        }
        ColinearDim::Four => {
            let p = pw.drop_w();
            let q = qw.drop_w();
            let r = rw.drop_w();
            let Ok((v1, _)) = normalize(q - p) else {
                return false;
            };
            let Ok((v2, _)) = normalize(r - q) else {
                return false;
            };
            let a00 = v1.dot(v1);
            let a01 = v1.dot(v2);
            let a11 = v2.dot(v2);
            let d = a00 * a11 - a01 * a01;
            d.abs() < ZERO_COEFFICIENT_TOL
        }
    }
}

/// Implicit plane `a*x + b*y + c*z + d = 0` with unit normal `(a, b, c)` from a point
/// and a normal (`N_PlanePtNormalToImplicit`). Errors with `NL_GEO_ERR` for a zero
/// normal.
pub fn plane_pt_normal_to_implicit(p: Vec3, n: Vec3) -> Result<[f64; 4]> {
    let (n, _) = normalize(n)?;
    let dot = p.dot(n);
    Ok([n.x, n.y, n.z, -dot])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn normalize_zero_is_geo_error() {
        let e = normalize(Vec3::ZERO).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Geometry,
                ..
            }
        ));
        let (u, m) = normalize(v(3.0, 0.0, 4.0)).unwrap();
        assert_eq!(m, 5.0);
        assert_eq!(u, v(0.2 * 3.0, 0.0, 0.2 * 4.0));
    }

    #[test]
    fn angles() {
        assert_eq!(vectors_angle(Vec3::X, Vec3::Y).unwrap(), 90.0);
        assert_eq!(vectors_cos_angle(Vec3::X, -Vec3::X).unwrap(), -1.0);
        assert!(vectors_angle(Vec3::ZERO, Vec3::Y).is_err());
    }

    #[test]
    fn line_line() {
        let l1 = LineSeg::new(v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), true);
        let l2 = LineSeg::new(v(1.0, -1.0, 0.0), v(0.0, 2.0, 0.0), true);
        let hit = isect_line_line(l1, l2).unwrap().unwrap();
        assert_eq!(hit.p, v(1.0, 0.0, 0.0));
        assert_eq!((hit.t1, hit.t2), (0.5, 0.5));
        // Parallel.
        let l3 = LineSeg::new(v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0), false);
        assert!(isect_line_line(l1, l3).unwrap().is_none());
        // Bounded miss.
        let l4 = LineSeg::new(v(5.0, -1.0, 0.0), v(0.0, 2.0, 0.0), true);
        assert!(isect_line_line(l1, l4).unwrap().is_none());
    }

    #[test]
    fn line_plane_and_projections() {
        let pln = Plane::new(v(0.0, 0.0, 2.0), v(0.0, 0.0, 5.0));
        let l = LineSeg::new(Vec3::ZERO, v(0.0, 0.0, 1.0), true);
        assert_eq!(
            isect_line_plane(l, pln).unwrap(),
            LinePlaneIsect::OutOfBounds { t: 2.0 }
        );
        let l = LineSeg {
            bounded: false,
            ..l
        };
        assert_eq!(
            isect_line_plane(l, pln).unwrap(),
            LinePlaneIsect::Hit {
                p: v(0.0, 0.0, 2.0),
                t: 2.0
            }
        );
        let side = LineSeg::new(Vec3::ZERO, Vec3::X, false);
        assert_eq!(
            isect_line_plane(side, pln).unwrap(),
            LinePlaneIsect::Parallel
        );
        assert_eq!(
            project_pt_plane(pln, v(1.0, 2.0, 7.0)).unwrap(),
            v(1.0, 2.0, 2.0)
        );
        assert_eq!(
            dist_pt_inf_line(LineSeg::new(Vec3::ZERO, Vec3::X, true), v(9.0, 3.0, 4.0)).unwrap(),
            5.0
        );
    }

    #[test]
    fn segment_distances() {
        let a = Vec3::ZERO;
        let b = v(4.0, 0.0, 0.0);
        assert_eq!(dist_pt_line_seg(v(2.0, 3.0, 0.0), a, b).unwrap(), 3.0);
        assert_eq!(dist_pt_line_seg(v(-3.0, 4.0, 0.0), a, b).unwrap(), 5.0);
        assert_eq!(dist_pt_line_seg(v(7.0, 4.0, 0.0), a, b).unwrap(), 5.0);
        assert_eq!(dist_perp_pt_line_seg(v(7.0, 4.0, 0.0), a, b).unwrap(), None);
        assert_eq!(
            dist_perp_pt_line_seg(v(1.0, 4.0, 0.0), a, b).unwrap(),
            Some(4.0)
        );
        // Degenerate segment: distance to its midpoint.
        assert_eq!(dist_pt_line_seg(v(0.0, 2.0, 0.0), a, a).unwrap(), 2.0);
        assert!(dist_pt_line_seg(v(f64::NAN, 0.0, 0.0), a, b)
            .unwrap()
            .is_nan());
        assert_eq!(dist_polygon(&[a, b, v(4.0, 3.0, 0.0)]), 7.0);
        assert_eq!(dist_polygon(&[]), 0.0);
    }

    #[test]
    fn boxes() {
        let pts = [v(1.0, 5.0, -1.0), v(-2.0, 0.0, 3.0), v(0.0, 7.0, 0.0)];
        let b = polygon_bbox(&pts).unwrap();
        assert_eq!(b, MinMaxBox::new(-2.0, 1.0, 0.0, 7.0, -1.0, 3.0));
        assert!(polygon_bbox(&[]).is_err());
        let n = net_bbox(&[pts.to_vec(), vec![v(9.0, 9.0, 9.0); 3]]).unwrap();
        assert_eq!(n.xr, 9.0);
        assert_eq!(MinMaxBox::new(0.0, 3.0, 0.0, 4.0, 0.0, 0.0).diagonal(), 5.0);
    }

    #[test]
    fn rotation_quarter_turn() {
        let m = rotation_matrix_about_axis(Vec3::ZERO, v(0.0, 0.0, 2.0), 90.0).unwrap();
        assert!((m.get(0, 1).unwrap() + 1.0).abs() < 1e-15);
        assert!((m.get(1, 0).unwrap() - 1.0).abs() < 1e-15);
        assert_eq!(m.get(3, 3).unwrap(), 1.0);
        assert!(rotation_matrix_about_axis(Vec3::ZERO, Vec3::ZERO, 1.0).is_err());
    }

    #[test]
    fn transform_identity_frames() {
        let o = v(1.0, 2.0, 3.0);
        let f = Frame::new(o, Vec3::X, Vec3::Y, Vec3::Z);
        let m = transform_matrix_from_axes(f, f).unwrap();
        for i in 0..4 {
            for j in 0..4 {
                let e = if i == j { 1.0 } else { 0.0 };
                assert_eq!(m.get(i, j).unwrap(), e);
            }
        }
    }

    #[test]
    fn reflect_and_colinear() {
        let pln = Plane::new(Vec3::ZERO, v(1.0, 0.0, 0.0));
        let q = cpt_reflect(CPoint::new(2.0, 4.0, NOZ, 2.0), pln).unwrap();
        assert_eq!(q, CPoint::new(-2.0, 4.0, NOZ, 2.0));
        let a = CPoint::from_point(Vec3::ZERO);
        let b = CPoint::from_point(v(1.0, 1.0, 1.0));
        let c = CPoint::from_point(v(2.0, 2.0, 2.0));
        assert!(cpts_are_colinear(a, b, c, ColinearDim::Three));
        assert!(cpts_are_colinear(a, b, c, ColinearDim::Four));
        let d = CPoint::from_point(v(2.0, 2.0, 2.5));
        assert!(!cpts_are_colinear(a, b, d, ColinearDim::Three));
        assert!(!cpts_are_colinear(a, a, d, ColinearDim::Four));
    }

    #[test]
    fn implicit_plane() {
        let r = plane_pt_normal_to_implicit(v(0.0, 0.0, 3.0), v(0.0, 0.0, -2.0)).unwrap();
        assert_eq!(r, [0.0, 0.0, -1.0, 3.0]);
        assert_eq!(
            dist_signed_pt_plane(r[0], r[1], r[2], r[3], v(5.0, 5.0, 1.0)),
            2.0
        );
    }
}
