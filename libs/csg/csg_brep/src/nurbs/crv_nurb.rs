//! NURBS curve evaluation and geometry: points, derivatives, tangents, curvature and
//! its derivative, Frenet frames, bounding boxes, affine and projective maps,
//! reversal, reparametrisation, Greville abscissae, linear and degenerate segments,
//! line/circle recognition and closure continuity of an NLib curve.
//!
//! Upstream: `source/SMLib/src/NL_CrvNurb.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvNurb.h`).
//!
//! | Rust                                    | Upstream                        |
//! |-----------------------------------------|---------------------------------|
//! | [`greville_abscissa`]                   | `N_CrvGetGrevilleAbscissa`      |
//! | [`Curve::eval_curvature`], [`Curvature`] | `N_CrvEvalCurvature`           |
//! | [`Curve::eval`]                         | `N_CrvEval`                     |
//! | [`Curve::bbox`]                         | `N_CrvGetBBox`                  |
//! | [`Curve::derivs`]                       | `N_CrvDerivs`                   |
//! | [`Curve::eval_tangent`]                 | `N_CrvEvalTangent`              |
//! | [`Curve::eval_frenet_frame`], [`FrenetFrame`] | `N_CrvEvalFrenetFrame`    |
//! | [`Curve::transform`]                    | `N_CrvTransform`                |
//! | [`Curve::min_max_weights_and_pts`], [`MinMaxWeights`] | `N_CrvGetMinMaxWeightsAndPts` |
//! | [`line_from_pt_and_vector`]             | `N_CrvLineFromPtAndVector`      |
//! | [`Curve::scale`]                        | `N_CrvScale`                    |
//! | [`Curve::scale_cpts`]                   | `N_CrvScaleCPts`                |
//! | [`Curve::translate`]                    | `N_CrvTranslate`                |
//! | [`Curve::project_onto_plane`], [`Projection`] | `N_CrvProjectOntoPlane` (`NL_PARALLEL` / `NL_PERSPECTIVE`) |
//! | [`Curve::reverse`] / [`Curve::reversed`] | `N_CrvReverse` (in place / into a new curve; knots by `NI_ReverseKnots`) |
//! | [`Curve::bbox_max_diag_dist`]           | `N_CrvGetBBoxMaxDiagDist`       |
//! | [`Curve::max_pos_vector`]               | `N_CrvGetMaxPosVector`          |
//! | [`Curve::linear_segs`], [`LinearSeg`]   | `N_CrvGetLinearSegs`            |
//! | [`Curve::degen_segs`]                   | `N_CrvGetDegenSegs`             |
//! | [`Curve::reparam`]                      | `N_CrvReparam`                  |
//! | [`Curve::get_type`], [`CurveType`]      | `N_CrvGetType` (`NL_NLINE` / `NL_NCIRCLE` / `NL_NFREEFORM`) |
//! | [`Curve::curvature_deriv`]              | `N_CrvGetCurvatureDeriv`        |
//! | [`Curve::is_closed_continuity`]         | `N_CrvIsClosedContinuity`       |
//!
//! Deviations:
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments and the memory error paths have no counterpart.
//! - Routines that write into an output curve that is NULL, an existing curve or the
//!   input itself (`N_CrvLineFromPtAndVector`, `N_CrvProjectOntoPlane`,
//!   `N_CrvReverse`) return a new [`Curve`] sized exactly as upstream sizes a NULL
//!   output, so the storage error path does not exist. `N_CrvReverse` keeps both of its
//!   (differently coded) forms: [`Curve::reverse`] in place, [`Curve::reversed`] into
//!   a new curve; for clamped curves (`m = n + p + 1`) they agree.
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed curves whose knots do not match the control points, degrees
//!   above `NL_MAXDEG`), the routines return [`SolidError::InvalidInput`] instead.
//! - `N_CrvGetGrevilleAbscissa` takes a knot pointer and its highest index;
//!   [`greville_abscissa`] takes the knot slice (highest index `len - 1`).
//! - `N_CrvGetLinearSegs` and `N_CrvGetDegenSegs` return C arrays plus their highest
//!   index; the Rust methods return `Vec`s (empty for upstream's `n = -1`).
//! - `N_CrvGetDegenSegs` ignores the error flags of its `N_CrvEval` calls (it then
//!   reads a point left unset); [`Curve::degen_segs`] propagates them.
//! - `N_CrvGetType` writes some outputs before failing in its circle branch; on an
//!   error [`Curve::get_type`] returns only the error. Its "not a line / circle" exits
//!   (including the cleared errors of `N_CrvEval` / `N_CrvEvalCurvature`) are
//!   [`CurveType::FreeForm`].
//! - `N_CrvTransform` converts a 2D curve to 3D before transforming; a matrix with
//!   less than 4x4 storage is `InvalidInput` (checked before the curve changes).
//! - `N_CrvIsClosedContinuity` with a level outside `0..=2` fails with `NL_INP_ERR`,
//!   as upstream; missing tolerances (upstream reads past `tols`) are `InvalidInput`.
//! - The `NL_CAL_ERR` paths of unknown `NL_PARALLEL` / `NL_PERSPECTIVE` and
//!   `NL_LEFT` / `NL_RIGHT` flags cannot be expressed with [`Projection`] / [`Side`].
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

// Branches keep upstream's `if( x NEQ NL_NOZ ) ... else ...` order for traceability.
#![allow(clippy::if_not_else)]

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::bezier::check_short_degree;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::geom::{
    dist_pt_pt, normalize, polygon_bbox, project_pt_plane, pt_along_vector, vector_diff_cpts,
    vectors_angle, vectors_cos_angle, Interval, MinMaxBox, Plane, LINE_TOL,
};
use crate::nurbs::index::{get, get2, oob, set, set2, sx};
use crate::nurbs::knots::{Side, MAX_DEGREE};
use crate::nurbs::math::{
    combine2_pts, float_op_is_bad, CPoint, FloatOp, RealMatrix, BIGD, MAX_DERIVATIVE, MODEL_TOL,
    NOW, NOZ,
};

/// The Greville abscissa of control point `idx`: the average of the `degree` knots
/// `U[idx+1..=idx+degree]` (`N_CrvGetGrevilleAbscissa`).
///
/// Errors with [`NlibCode::Index`] when `degree < 1` or `idx + degree` exceeds the
/// highest knot index.
pub fn greville_abscissa(knots: &[f64], degree: usize, idx: usize) -> Result<f64> {
    const OP: &str = "nurbs::crv_nurb::greville_abscissa";
    if degree < 1 {
        return Err(SolidError::nlib(OP, NlibCode::Index));
    }
    let window = idx
        .checked_add(degree)
        .and_then(|end| knots.get(idx + 1..=end))
        .ok_or(SolidError::nlib(OP, NlibCode::Index))?;
    // The Greville abscissa is the average of 'degree' knot values.
    let mut g = 0.0;
    for &k in window {
        g += k;
    }
    g /= degree as f64;
    Ok(g)
}

/// Result of [`Curve::eval_curvature`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curvature {
    /// Curvature `kap`; the osculating radius is `1/kap`.
    pub kappa: f64,
    /// Point on the curve.
    pub point: Vec3,
    /// Unit tangent.
    pub tangent: Vec3,
    /// The osculating circle `(center, unit normal of its plane)`, when asked for.
    pub circle: Option<(Vec3, Vec3)>,
}

/// Result of [`Curve::eval_frenet_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrenetFrame {
    /// Point on the curve.
    pub point: Vec3,
    /// Unit tangent.
    pub tangent: Vec3,
    /// Unit principal normal.
    pub normal: Vec3,
    /// Unit binormal.
    pub binormal: Vec3,
}

/// Result of [`Curve::min_max_weights_and_pts`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinMaxWeights {
    /// Smallest weight ([`NOW`] for a non-rational curve).
    pub wmin: f64,
    /// Largest weight ([`NOW`] for a non-rational curve).
    pub wmax: f64,
    /// Smallest distance of a Euclidean control point from their centroid.
    pub pmin: f64,
    /// Always `0.0` (upstream sets it so on purpose; see `N_CrvGetMinMaxWeightsAndPts`).
    pub pmax: f64,
}

/// Projection kind of [`Curve::project_onto_plane`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// `NL_PARALLEL` (1): `E` is the direction of projection.
    Parallel,
    /// `NL_PERSPECTIVE` (2): `E` is the center of projection.
    Perspective,
}

/// One segment of [`Curve::linear_segs`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearSeg {
    /// Start parameter.
    pub start: f64,
    /// End parameter.
    pub end: f64,
    /// `true` when the segment is a straight line.
    pub linear: bool,
}

/// Result of [`Curve::get_type`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurveType {
    /// `NL_NLINE` (1): within the tolerance of the segment `start .. start + dir`.
    Line {
        /// Start point (the first Euclidean control point).
        start: Vec3,
        /// Direction vector (last minus first control point).
        dir: Vec3,
    },
    /// `NL_NCIRCLE` (2): within the tolerance of a circular arc.
    Circle {
        /// Center of the circle.
        center: Vec3,
        /// Normal to the circle's plane (the average osculating normal).
        normal: Vec3,
        /// Unit x axis (towards the first control point).
        x_axis: Vec3,
        /// y axis (`normal x x_axis`).
        y_axis: Vec3,
        /// Radius.
        radius: f64,
        /// Sweep angle in degrees (`360` for a closed curve).
        angle: f64,
    },
    /// `NL_NFREEFORM` (11): neither a line nor a circle.
    FreeForm,
}

impl CurveType {
    /// The upstream flag value (`NL_NLINE = 1`, `NL_NCIRCLE = 2`, `NL_NFREEFORM = 11`).
    #[must_use]
    pub fn flag(&self) -> i32 {
        match self {
            CurveType::Line { .. } => 1,
            CurveType::Circle { .. } => 2,
            CurveType::FreeForm => 11,
        }
    }
}

/// A non-rational line from `P` to `P + V` on knots `0, 0, 1, 1`
/// (`N_CrvLineFromPtAndVector`).
pub fn line_from_pt_and_vector(p: Vec3, v: Vec3) -> Result<Curve> {
    let t = p + v;
    let pw = vec![CPoint::from_point(p), CPoint::from_point(t)];
    Curve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0])
        .map_err(|_| oob("nurbs::crv_nurb::line_from_pt_and_vector"))
}

impl Curve {
    /// Curvature at `u`, the point, the unit tangent and, when `circle`, the
    /// osculating circle (`N_CrvEvalCurvature`). With `circle` the point and tangent
    /// come from [`Curve::eval_frenet_frame`].
    ///
    /// Errors with [`NlibCode::Numerical`] when the curvature (or, with `circle`, the
    /// radius) is not computable, and with the errors of [`Curve::derivs`],
    /// [`Curve::eval_frenet_frame`] and the tangent normalisation.
    pub fn eval_curvature(&self, u: f64, side: Side, circle: bool) -> Result<Curvature> {
        const OP: &str = "nurbs::crv_nurb::Curve::eval_curvature";
        // Get derivatives.
        let d = self.derivs(u, side, 2)?;
        let d0 = get(&d, 0, OP)?;
        let d1 = get(&d, 1, OP)?;
        let d2 = get(&d, 2, OP)?;

        // Get curvature.
        let a = d1.cross(d2);
        let num = a.length();
        let den = d1.length();
        let den = den * den * den;
        if float_op_is_bad(num, den, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }
        let kap = num / den;

        // Get osculating circle.
        if circle {
            if float_op_is_bad(1.0, kap, FloatOp::Division) {
                return Err(SolidError::nlib(OP, NlibCode::Numerical));
            }
            let rad = 1.0 / kap;
            let f = self.eval_frenet_frame(u, side)?;
            let c = pt_along_vector(d0, rad, f.normal);
            Ok(Curvature {
                kappa: kap,
                point: f.point,
                tangent: f.tangent,
                circle: Some((c, f.binormal)),
            })
        } else {
            let (t, _) = normalize(d1)?;
            Ok(Curvature {
                kappa: kap,
                point: d0,
                tangent: t,
                circle: None,
            })
        }
    }

    /// The point at `u` (`N_CrvEval`): the non-vanishing basis functions times their
    /// control points, mapped to Euclidean space.
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is outside the knot range, and
    /// with the errors of [`crate::nurbs::knots::KnotVector::basis_eval`].
    pub fn eval(&self, u: f64, side: Side) -> Result<Vec3> {
        const OP: &str = "nurbs::crv_nurb::Curve::eval";
        let knt = self.knot_vector()?;
        let pw = self.cpts();
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;

        // Check parameter.
        if knt.is_param_out_of_bounds(u) {
            return Err(SolidError::nlib(OP, NlibCode::Parameter));
        }

        // Compute non-vanishing B-splines (on the stack, as upstream's `N`).
        let mut n = [0.0_f64; MAX_DEGREE + 2];
        let spn = sx(knt.basis_eval_into(self.degree(), u, side, &mut n)?, OP)?;

        // Compute the point on the curve.
        let mut cw = CPoint::new(0.0, 0.0, 0.0, 0.0);
        let j = spn - p;
        for i in j..=spn {
            let alpha = get(&n, i - j, OP)?;
            let q = get(pw, i, OP)?;
            cw.x += alpha * q.x;
            cw.y += alpha * q.y;
            if q.z != NOZ {
                cw.z += alpha * q.z;
            } else {
                cw.z = NOZ;
            }
            if q.w != NOW {
                cw.w += alpha * q.w;
            } else {
                cw.w = NOW;
            }
        }
        Ok(cw.to_euclid())
    }

    /// Min-max box of the Euclidean control polygon (`N_CrvGetBBox`).
    pub fn bbox(&self) -> Result<MinMaxBox> {
        // Map control polygon.
        let ppl = self.epolygon(0, self.n())?;
        // Get min-max box.
        polygon_bbox(&ppl)
    }

    /// Derivatives `C^(k)(u)` for `k = 0..=der` (`N_CrvDerivs`, algorithm A2.3 with
    /// the rational quotient rule). Derivatives above the degree are zero.
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is outside `[U[0], U[m]]`, with
    /// [`NlibCode::MaxDerivative`] when `der > NL_MAXDER`, and with
    /// [`NlibCode::Numerical`] when a basis quotient is not computable.
    pub fn derivs(&self, u: f64, side: Side, der: usize) -> Result<Vec<Vec3>> {
        const OP: &str = "nurbs::crv_nurb::Curve::derivs";
        const NP: usize = MAX_DEGREE + 1;
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;
        let uk = self.knots();
        let m = sx(self.m(), OP)?;
        let pw = self.cpts();

        // Check parameter and order of derivatives.
        if u < get(uk, 0, OP)? || u > get(uk, m, OP)? {
            return Err(SolidError::nlib(OP, NlibCode::Parameter));
        }
        if der > MAX_DERIVATIVE {
            return Err(SolidError::nlib(OP, NlibCode::MaxDerivative));
        }
        if pu > MAX_DEGREE {
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "degree above NL_MAXDEG (upstream scratch array size)",
            });
        }
        let der = sx(der, OP)?;

        // `left`, `right` and `dd` alias the rows of `a`, as upstream.
        let mut a = [[0.0_f64; NP]; 2];
        let mut bd = [[0.0_f64; NP]; MAX_DERIVATIVE + 1];
        let mut ndu = [[0.0_f64; NP]; NP];

        // Find the knot span u is in.
        let nsp = sx(knt.find_span(pu, u, side)?, OP)?;

        // Get maximum derivative index and set zero derivatives.
        let mder = p.min(der);
        for k in p + 1..=der {
            for j in 0..=p {
                set2(&mut bd, k, j, 0.0, OP)?;
            }
        }

        // Compute the basis functions.
        set2(&mut ndu, 0, 0, 1.0, OP)?;
        for j in 1..=p {
            set2(&mut a, 0, j, u - get(uk, nsp + 1 - j, OP)?, OP)?;
            set2(&mut a, 1, j, get(uk, nsp + j, OP)? - u, OP)?;
            let mut saved = 0.0;
            for r in 0..j {
                let right = get2(&a, 1, r + 1, OP)?;
                let left = get2(&a, 0, j - r, OP)?;
                set2(&mut ndu, j, r, right + left, OP)?;
                let num = get2(&ndu, r, j - 1, OP)?;
                let den = get2(&ndu, j, r, OP)?;
                if float_op_is_bad(num, den, FloatOp::Division) {
                    return Err(SolidError::nlib(OP, NlibCode::Numerical));
                }
                let temp = num / den;
                set2(&mut ndu, r, j, saved + right * temp, OP)?;
                saved = left * temp;
            }
            set2(&mut ndu, j, j, saved, OP)?;
        }

        // Load the basis functions.
        for j in 0..=p {
            set2(&mut bd, 0, j, get2(&ndu, j, p, OP)?, OP)?;
        }

        // Compute derivatives.
        for r in 0..=p {
            let mut s1: isize = 0;
            let mut s2: isize = 1;
            set2(&mut a, 0, 0, 1.0, OP)?;
            for k in 1..=mder {
                let mut d = 0.0;
                let rk = r - k;
                let pk = p - k;
                if r >= k {
                    let v = get2(&a, s1, 0, OP)? / get2(&ndu, pk + 1, rk, OP)?;
                    set2(&mut a, s2, 0, v, OP)?;
                    d = v * get2(&ndu, rk, pk, OP)?;
                }
                let j1 = if rk >= -1 { 1 } else { -rk };
                let j2 = if r - 1 <= pk { k - 1 } else { p - r };
                for j in j1..=j2 {
                    let v = (get2(&a, s1, j, OP)? - get2(&a, s1, j - 1, OP)?)
                        / get2(&ndu, pk + 1, rk + j, OP)?;
                    set2(&mut a, s2, j, v, OP)?;
                    d += v * get2(&ndu, rk + j, pk, OP)?;
                }
                if r <= pk {
                    let v = -get2(&a, s1, k - 1, OP)? / get2(&ndu, pk + 1, r, OP)?;
                    set2(&mut a, s2, k, v, OP)?;
                    d += v * get2(&ndu, r, pk, OP)?;
                }
                set2(&mut bd, k, r, d, OP)?;
                core::mem::swap(&mut s1, &mut s2);
            }
        }

        // Multiply through by the correct factors.
        let mut r = p;
        for k in 1..=mder {
            for j in 0..=p {
                let v = get2(&bd, k, j, OP)? * r as f64;
                set2(&mut bd, k, j, v, OP)?;
            }
            r *= p - k;
        }

        if self.is_rational() {
            // Get derivatives of the denominator (`dd` is row 0 of `a`).
            for k in 0..=der {
                set2(&mut a, 0, k, 0.0, OP)?;
                for j in nsp - p..=nsp {
                    let w = get(pw, j, OP)?.w;
                    let v = get2(&a, 0, k, OP)? + w * get2(&bd, k, j - nsp + p, OP)?;
                    set2(&mut a, 0, k, v, OP)?;
                }
            }

            // Compute derivatives of rational basis function: Pascal triangle first.
            let tri = pascal(der, OP)?;

            for i in 0..=p {
                let w = get(pw, nsp - p + i, OP)?.w;
                for k in 0..=der {
                    let mut d = w * get2(&bd, k, i, OP)?;
                    for j in 1..=k {
                        let t = get2(&tri, k, j, OP)? as f64;
                        d -= t * get2(&a, 0, j, OP)? * get2(&bd, k - j, i, OP)?;
                    }
                    set2(&mut bd, k, i, d / get2(&a, 0, 0, OP)?, OP)?;
                }
            }
        }

        // Compute derivatives.
        let j = nsp - p;
        let mut pts = [Vec3::ZERO; NP];
        for i in j..=nsp {
            set(&mut pts, i - j, get(pw, i, OP)?.to_euclid(), OP)?;
        }
        let mut cd = Vec::with_capacity(to_len(der));
        for k in 0..=der {
            let mut c = Vec3::new(0.0, 0.0, 0.0);
            for i in 0..=p {
                let d = get2(&bd, k, i, OP)?;
                let q = get(&pts, i, OP)?;
                c.x += d * q.x;
                c.y += d * q.y;
                c.z += d * q.z;
            }
            cd.push(c);
        }
        Ok(cd)
    }

    /// Point and unit tangent at `u` (`N_CrvEvalTangent`).
    ///
    /// Errors with the errors of [`Curve::derivs`] and with [`NlibCode::Geometry`]
    /// when the first derivative cannot be normalised.
    pub fn eval_tangent(&self, u: f64, side: Side) -> Result<(Vec3, Vec3)> {
        const OP: &str = "nurbs::crv_nurb::Curve::eval_tangent";
        let cd = self.derivs(u, side, 1)?;
        let p = get(&cd, 0, OP)?;
        let (t, _) = normalize(get(&cd, 1, OP)?)?;
        Ok((p, t))
    }

    /// Point and Frenet frame at `u` (`N_CrvEvalFrenetFrame`): unit tangent, unit
    /// binormal `C' x C''` and normal `B x T`.
    ///
    /// Errors with [`NlibCode::Degree`] for degrees below 2, with the errors of
    /// [`Curve::derivs`], and with [`NlibCode::Geometry`] when the tangent or the
    /// binormal cannot be normalised.
    pub fn eval_frenet_frame(&self, u: f64, side: Side) -> Result<FrenetFrame> {
        const OP: &str = "nurbs::crv_nurb::Curve::eval_frenet_frame";
        if self.degree() <= 1 {
            return Err(SolidError::nlib(OP, NlibCode::Degree));
        }
        let cd = self.derivs(u, side, 2)?;
        let c0 = get(&cd, 0, OP)?;
        let c1 = get(&cd, 1, OP)?;
        let c2 = get(&cd, 2, OP)?;

        // Get point and tangent.
        let (t, _) = normalize(c1)?;
        // Get binormal.
        let (b, _) = normalize(c1.cross(c2))?;
        // Get normal.
        let n = b.cross(t);
        Ok(FrenetFrame {
            point: c0,
            tangent: t,
            normal: n,
            binormal: b,
        })
    }

    /// Applies a 4x4 matrix to every control point (`N_CrvTransform`); a 2D curve is
    /// made 3D first.
    ///
    /// Errors with `InvalidInput` (changing nothing) when `rma`'s storage is smaller
    /// than 4x4.
    pub fn transform(&mut self, rma: &RealMatrix) -> Result<()> {
        const OP: &str = "nurbs::crv_nurb::Curve::transform";
        // Check the matrix before anything changes.
        for i in 0..4 {
            for j in 0..4 {
                rma.get(i, j).map_err(|_| oob(OP))?;
            }
        }
        // Convert to 3-D if needed.
        if !self.is_3d() {
            self.to_3d();
        }
        // Transform control points.
        for pw in self.cpts_mut() {
            *pw = pw.transform(rma)?;
        }
        Ok(())
    }

    /// Smallest and largest weight and smallest distance of a Euclidean control point
    /// from the control points' centroid (`N_CrvGetMinMaxWeightsAndPts`; `pmax` is
    /// always `0`).
    #[must_use]
    pub fn min_max_weights_and_pts(&self) -> MinMaxWeights {
        let pw = self.cpts();

        // Compute min-max weights.
        let (mut wmin, mut wmax) = (NOW, NOW);
        if self.is_rational() {
            if let Some((first, rest)) = pw.split_first() {
                wmin = first.w;
                wmax = first.w;
                for q in rest {
                    let w = q.w;
                    if w < wmin {
                        wmin = w;
                    }
                    if w > wmax {
                        wmax = w;
                    }
                }
            }
        }

        // Compute the euclidean center, as the new origin.
        let mut c = Vec3::new(0.0, 0.0, 0.0);
        for q in pw {
            c = q.to_euclid() + c;
        }
        let f = 1.0 / (self.n() as f64 + 1.0);
        c = Vec3::new(f * c.x, f * c.y, f * c.z);

        // Compute min-max position vector magnitudes (upstream then overwrites pmax
        // with 0, see its header comment).
        let mut pmin = 0.0;
        for (i, q) in pw.iter().enumerate() {
            let mag = (q.to_euclid() - c).length();
            if i == 0 || mag < pmin {
                pmin = mag;
            }
        }
        MinMaxWeights {
            wmin,
            wmax,
            pmin,
            pmax: 0.0,
        }
    }

    /// Scales the curve about `c` by the per-axis factors `f` (`N_CrvScale`); a 2D
    /// curve is made 3D first.
    pub fn scale(&mut self, c: Vec3, f: Vec3) {
        if !self.is_3d() {
            self.to_3d();
        }
        for pw in self.cpts_mut() {
            *pw = pw.scale_about(c, f);
        }
    }

    /// Multiplies every weighted control point (weight included) by `f`
    /// (`N_CrvScaleCPts`).
    pub fn scale_cpts(&mut self, f: f64) {
        for pw in self.cpts_mut() {
            *pw = pw.scale(f);
        }
    }

    /// Translates the curve by `t` (`N_CrvTranslate`); a 2D curve is made 3D first.
    pub fn translate(&mut self, t: Vec3) {
        if !self.is_3d() {
            self.to_3d();
        }
        for pw in self.cpts_mut() {
            *pw = pw.translate(t);
        }
    }

    /// Projects the curve onto the plane through `o` with normal `n`, along the
    /// direction `e` ([`Projection::Parallel`]) or from the center `e`
    /// ([`Projection::Perspective`]) (`N_CrvProjectOntoPlane`).
    ///
    /// Errors (parallel) with [`NlibCode::Geometry`] when `n` or `e` cannot be
    /// normalised and with [`NlibCode::Input`] when `e` is parallel to the plane;
    /// (perspective) with [`NlibCode::Numerical`] when a control point's projection
    /// is not computable.
    pub fn project_onto_plane(&self, o: Vec3, n: Vec3, e: Vec3, prj: Projection) -> Result<Curve> {
        const OP: &str = "nurbs::crv_nurb::Curve::project_onto_plane";
        let pw = self.cpts();
        let mut qw = pw.to_vec();
        let rat = self.is_rational();

        match prj {
            Projection::Parallel => {
                let (nn, _) = normalize(n)?;
                let (en, _) = normalize(e)?;
                let ne = nn.dot(en);
                if ne.abs() < LINE_TOL {
                    return Err(SolidError::nlib(OP, NlibCode::Input));
                }
                let ne = n.dot(e);
                for (q, p) in qw.iter_mut().zip(pw) {
                    let pe = p.to_euclid();
                    let op = o - pe;
                    let nop = n.dot(op);
                    let bet = nop / ne;
                    let qe = combine2_pts(1.0, pe, bet, e);
                    *q = if rat {
                        CPoint::weighted(qe, p.w)
                    } else {
                        CPoint::from_point(qe)
                    };
                }
            }
            Projection::Perspective => {
                let eo = e - o;
                let neo = n.dot(eo);
                for (q, p) in qw.iter_mut().zip(pw) {
                    let pe = p.to_euclid();
                    let ep = e - pe;
                    let nep = n.dot(ep);
                    if float_op_is_bad(neo, nep, FloatOp::Division) {
                        return Err(SolidError::nlib(OP, NlibCode::Numerical));
                    }
                    let alf = neo / nep;
                    let bet = 1.0 - alf;
                    let qe = combine2_pts(alf, pe, bet, e);
                    *q = if rat {
                        let w = p.w * nep;
                        CPoint::weighted(qe, w)
                    } else {
                        CPoint::from_point(qe)
                    };
                }
            }
        }

        // Copy knot vector.
        Curve::new(qw, self.degree(), self.knots().to_vec()).map_err(|_| oob(OP))
    }

    /// Reverses the curve in place (`N_CrvReverse` with `curQ == curP`): control
    /// points swapped end for end and the knots reversed by
    /// [`reverse_knots`](crate::geom::a_object::reverse_knots) (`NI_ReverseKnots`).
    ///
    /// Errors with `InvalidInput` (the curve may then be partly reversed) only for
    /// knot vectors shorter than upstream indexes.
    pub fn reverse(&mut self) -> Result<()> {
        const OP: &str = "nurbs::crv_nurb::Curve::reverse";
        check_short_degree(self.degree(), OP)?;
        let n = sx(self.n(), OP)?;
        let m = self.m();

        // Reverse curve.
        let pw = self.cpts_mut();
        let k = n / 2;
        for i in 0..=k {
            let a = get(pw, i, OP)?;
            let b = get(pw, n - i, OP)?;
            set(pw, i, b, OP)?;
            set(pw, n - i, a, OP)?;
        }
        // (each pair of the in-place knot reversal is read before it is written, so
        // reversing from a copy gives the same knots)
        let up = self.knots().to_vec();
        crate::geom::a_object::reverse_knots(&up, m, self.knots_mut())
    }

    /// The reversed curve (`N_CrvReverse` into a new curve): control points in reverse
    /// order and the knots reversed by
    /// [`reverse_knots`](crate::geom::a_object::reverse_knots) (`NI_ReverseKnots`).
    ///
    /// Errors with `InvalidInput` when the knots do not match the control points as
    /// upstream indexes them.
    pub fn reversed(&self) -> Result<Curve> {
        const OP: &str = "nurbs::crv_nurb::Curve::reversed";
        check_short_degree(self.degree(), OP)?;
        let n = sx(self.n(), OP)?;
        let pw = self.cpts();
        let up = self.knots();
        let mut qw = vec![CPoint::default(); self.n() + 1];
        let mut uq = vec![0.0; self.m() + 1];

        // Reverse curve.
        for i in 0..=n {
            set(&mut qw, i, get(pw, n - i, OP)?, OP)?;
        }
        crate::geom::a_object::reverse_knots(up, self.m(), &mut uq)?;
        Curve::new(qw, self.degree(), uq).map_err(|_| oob(OP))
    }

    /// Length of the diagonal of [`Curve::bbox`] (`N_CrvGetBBoxMaxDiagDist`).
    pub fn bbox_max_diag_dist(&self) -> Result<f64> {
        Ok(self.bbox()?.diagonal())
    }

    /// The longest position vector of the Euclidean control points and its magnitude
    /// (`N_CrvGetMaxPosVector`; the first of equally long ones).
    #[must_use]
    pub fn max_pos_vector(&self) -> (Vec3, f64) {
        let pw = self.cpts();
        let Some((first, rest)) = pw.split_first() else {
            return (Vec3::new(f64::NAN, f64::NAN, f64::NAN), f64::NAN);
        };
        let mut p = first.to_euclid();
        let mut mag = p.length();
        for q in rest {
            let q = q.to_euclid();
            let len = q.length();
            if len > mag {
                p = q;
                mag = len;
            }
        }
        (p, mag)
    }

    /// Splits the parameter range into maximal linear and curved segments
    /// (`N_CrvGetLinearSegs`). A span is linear when the control polygon over it turns
    /// by less than about one degree at every vertex and the extracted span is a line
    /// within `tol` ([`Curve::is_line`]); consecutive linear spans merge while their
    /// union stays a line.
    ///
    /// Errors with [`NlibCode::Degree`] for degrees below 2 and with the errors of
    /// [`Curve::extract_seg`].
    pub fn linear_segs(&self, tol: f64) -> Result<Vec<LinearSeg>> {
        const OP: &str = "nurbs::crv_nurb::Curve::linear_segs";
        let pw = self.cpts();
        let uk = self.knots();
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let m = sx(self.m(), OP)?;

        if p < 2 {
            return Err(SolidError::nlib(OP, NlibCode::Degree));
        }

        let mut segs = vec![LinearSeg {
            start: get(uk, 0, OP)?,
            end: 0.0,
            linear: false,
        }];
        let mut kn: isize = 0;

        // Loop thru knots and find linear/curved segments.
        let mut ii = p;
        loop {
            let mut line = false;

            let mut jj = ii - p + 1;
            let mut v2 = vector_diff_cpts(get(pw, jj, OP)?, get(pw, jj - 1, OP)?);
            while jj < ii {
                let v1 = v2;
                v2 = vector_diff_cpts(get(pw, jj + 1, OP)?, get(pw, jj, OP)?);
                if let Ok(cos) = vectors_cos_angle(v1, v2) {
                    if cos < 0.99985 {
                        break; // 1 degree angle
                    }
                }
                jj += 1;
            }

            if jj == ii {
                // Must do point-to-line distance check.
                let cseg = self.extract_seg(get(uk, ii, OP)?, get(uk, ii + 1, OP)?)?;
                line = cseg.is_line(tol);
            }

            let cur = get(&segs, kn, OP)?;
            if line {
                if ii == p {
                    set(
                        &mut segs,
                        kn,
                        LinearSeg {
                            linear: true,
                            ..cur
                        },
                        OP,
                    )?;
                } else {
                    if cur.linear {
                        let cseg = self.extract_seg(cur.start, get(uk, ii + 1, OP)?)?;
                        line = cseg.is_line(tol);
                    }
                    if !cur.linear || !line {
                        let at = get(uk, ii, OP)?;
                        set(&mut segs, kn, LinearSeg { end: at, ..cur }, OP)?;
                        segs.push(LinearSeg {
                            start: at,
                            end: 0.0,
                            linear: true,
                        });
                        kn += 1;
                    }
                }
            } else if ii == p {
                set(
                    &mut segs,
                    kn,
                    LinearSeg {
                        linear: false,
                        ..cur
                    },
                    OP,
                )?;
            } else if cur.linear {
                let at = get(uk, ii, OP)?;
                set(&mut segs, kn, LinearSeg { end: at, ..cur }, OP)?;
                segs.push(LinearSeg {
                    start: at,
                    end: 0.0,
                    linear: false,
                });
                kn += 1;
            }

            ii += 1; // go to the next span
            if get(uk, ii, OP)? >= get(uk, m, OP)? {
                break;
            }
            while get(uk, ii, OP)? == get(uk, ii + 1, OP)? {
                ii += 1;
            }
        }

        let last = get(&segs, kn, OP)?;
        set(
            &mut segs,
            kn,
            LinearSeg {
                end: get(uk, m, OP)?,
                ..last
            },
            OP,
        )?;
        Ok(segs)
    }

    /// Parameter intervals `[u1, u2]` (merged over consecutive spans) whose image has
    /// arc length at most `tol` (`N_CrvGetDegenSegs`).
    ///
    /// Errors with the errors of [`Curve::eval`] and [`Curve::arc_length`].
    pub fn degen_segs(&self, tol: f64) -> Result<Vec<(f64, f64)>> {
        const OP: &str = "nurbs::crv_nurb::Curve::degen_segs";
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let uk = self.knots();
        let m = sx(self.m(), OP)?;

        // Loop thru segments and find the degeneracies.
        let mut sg: Vec<(f64, f64)> = Vec::new();
        let mut len = 0.0;
        let mut ii = p;
        let mut ps = self.eval(get(uk, ii, OP)?, Side::Left)?;

        loop {
            let pe = self.eval(get(uk, ii + 1, OP)?, Side::Left)?;
            let d1 = dist_pt_pt(ps, pe);

            if d1 <= tol {
                if m <= 2 * p + 1 {
                    let mid = 0.5 * (get(uk, ii, OP)? + get(uk, ii + 1, OP)?);
                    let pm = self.eval(mid, Side::Left)?;
                    let d2 = dist_pt_pt(ps, pm);
                    let d1 = dist_pt_pt(pm, pe);
                    if d2 + d1 > tol {
                        break;
                    }
                }

                let d1 = self.arc_length(
                    get(uk, ii, OP)?,
                    get(uk, ii + 1, OP)?,
                    tol,
                    ToleranceKind::Absolute,
                )?;

                if d1 > tol {
                    len = 0.0;
                } else if len > 0.0 && len + d1 <= tol {
                    let last = sg.last_mut().ok_or_else(|| oob(OP))?;
                    last.1 = get(uk, ii + 1, OP)?;
                    len += d1;
                } else {
                    len = d1;
                    sg.push((get(uk, ii, OP)?, get(uk, ii + 1, OP)?));
                }
            } else {
                len = 0.0;
            }

            if ii >= m - p - 1 {
                break;
            }

            // Prepare for next pass thru loop.
            ps = pe;
            ii += 1;
            while get(uk, ii, OP)? == get(uk, ii + 1, OP)? {
                ii += 1;
            }
        }
        Ok(sg)
    }

    /// Rescales the knots to `[ul, ur]` without changing the geometry
    /// (`N_CrvReparam`, through [`Curve::reparam_to_interval`]).
    pub fn reparam(&mut self, ul: f64, ur: f64) -> Result<()> {
        self.reparam_to_interval(Interval::new(ul, ur))
    }

    /// Recognises lines and circular arcs within `tol` (`N_CrvGetType`).
    ///
    /// A curve is a line when sample points of every span (`2p` per span) project
    /// onto the chord of the end control points within `tol` and inside it. Otherwise
    /// it is a circle when the osculating radii of the samples vary by at most `2 tol`,
    /// the samples lie within `tol` of the plane of the average osculating circle and
    /// their distances from its average center are within `tol` of their mean.
    ///
    /// Errors with the errors of the plane projection, of normalising the x axis and of
    /// the sweep angle in the circle branch (upstream then has written the center and
    /// normal); everything else that stops recognition is [`CurveType::FreeForm`].
    pub fn get_type(&self, tol: f64) -> Result<CurveType> {
        const OP: &str = "nurbs::crv_nurb::Curve::get_type";
        let pw = self.cpts();
        let uk = self.knots();
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;

        let mut lin = true; // assume a line, tested and changed later

        // Check if line.
        let a = get(pw, 0, OP)?.to_euclid();
        let b = get(pw, n, OP)?.to_euclid();
        let t = b - a;
        let dab = t.length();

        // Upstream: `dab <= NL_MTOL && dab <= NL_PTOL` (`NL_PTOL < NL_MTOL`).
        if dab <= PTOL {
            lin = false; // closed curves are not lines
        } else if n > 1 {
            // When control point count is larger than 2, check for colinearity.
            let dab2 = dab * dab;
            let mut kh = 2 * p;
            let mut tl = get(uk, p, OP)?;
            let mut i = p + 1;

            while i < m {
                while i < m && get(uk, i, OP)? == get(uk, i + 1, OP)? {
                    i += 1;
                }
                let tr = get(uk, i, OP)?;
                let ti = (tr - tl) / kh as f64;

                if i >= m {
                    kh -= 1;
                }

                for k in 1..=kh {
                    let t_par = if k == kh && i < m {
                        tr
                    } else {
                        tl + k as f64 * ti
                    };

                    let Ok(q) = self.eval(t_par, Side::Left) else {
                        return Ok(CurveType::FreeForm);
                    };

                    // Project Q onto line [A,B] to get R.
                    let v = q - a;
                    let mut d = v.dot(t);
                    d /= dab2;
                    if d < 0.0 || d > 1.0 {
                        lin = false;
                        break;
                    }
                    let r = pt_along_vector(a, d, t);

                    // Projection distance.
                    if dist_pt_pt(q, r) > tol {
                        lin = false;
                        break;
                    }

                    // This distance should not be more than length of line seg.
                    if dist_pt_pt(q, a) > dab {
                        lin = false;
                        break;
                    }
                }

                if !lin {
                    break;
                }
                tl = tr;
                i += 1;
            }
        }

        if lin {
            return Ok(CurveType::Line { start: a, dir: t });
        }

        // Check if circle.
        let is_closed = dist_pt_pt(a, b) < tol;

        let mut rd: Vec<f64> = Vec::new();
        let mut cs: Vec<Vec3> = Vec::new();
        let mut ns: Vec<Vec3> = Vec::new();
        let mut ps: Vec<Vec3> = Vec::new();

        let kh = 2 * p;
        let mut tl = get(uk, p, OP)?;
        let mut i = p + 1;
        let tol2 = 2.0 * tol;
        let mut maxrad = 0.0;
        let mut minrad = BIGD;
        let mut ca = Vec3::new(0.0, 0.0, 0.0);
        let mut na = Vec3::new(0.0, 0.0, 0.0);

        // For all knot spans.
        while i < m {
            let kl = isize::from(i != p + 1);
            while i < m && get(uk, i, OP)? == get(uk, i + 1, OP)? {
                i += 1;
            }
            let kr = if is_closed && i == m { kh - 1 } else { kh };
            let tr = get(uk, i, OP)?;
            let ti = (tr - tl) / kh as f64;

            for k in kl..=kr {
                let t_par = if k == kh { tr } else { tl + k as f64 * ti };

                let Ok(cv) = self.eval_curvature(t_par, Side::Left, true) else {
                    return Ok(CurveType::FreeForm);
                };
                let kap = cv.kappa;
                let Some((c, nrm)) = cv.circle else {
                    return Ok(CurveType::FreeForm);
                };
                ps.push(cv.point);
                cs.push(c);
                ns.push(nrm);

                if float_op_is_bad(1.0, kap, FloatOp::Division) {
                    return Ok(CurveType::FreeForm);
                }
                let r = 1.0 / kap;
                rd.push(r);

                if minrad > r {
                    minrad = r;
                }
                if maxrad < r {
                    maxrad = r;
                }

                // Quit when tolerances are exceeded.
                if maxrad - minrad > tol2 {
                    return Ok(CurveType::FreeForm);
                }

                ca += c;
                na += nrm;
            }

            tl = tr;
            i += 1;
        }

        // Turn the sums into averages.
        let j = ps.len() as f64 - 1.0;
        let fac = 1.0 / (j + 1.0);
        ca = Vec3::new(fac * ca.x, fac * ca.y, fac * ca.z);
        na = Vec3::new(fac * na.x, fac * na.y, fac * na.z);

        // Define the average osculating circle plane.
        let pln = Plane::new(ca, na);

        // Get the radius from the sample positions.
        let mut ra = 0.0;
        let mut maxrad = 0.0;
        let mut minrad = BIGD;
        for &pi in &ps {
            let q = project_pt_plane(pln, pi)?;
            let r = dist_pt_pt(pi, ca);
            ra += r;
            if minrad > r {
                minrad = r;
            }
            if maxrad < r {
                maxrad = r;
            }
            if dist_pt_pt(pi, q) > tol {
                return Ok(CurveType::FreeForm);
            }
        }

        let fac = 1.0 / (j + 1.0);
        ra *= fac;

        if (maxrad - ra).abs() > tol {
            return Ok(CurveType::FreeForm);
        }
        if (ra - minrad).abs() > tol {
            return Ok(CurveType::FreeForm);
        }

        // Circle x axis = normalized(first control point - center).
        let (xx, _) = normalize(a - ca)?;
        // Circle y axis = normal x x axis.
        let yy = na.cross(xx);

        let al = if is_closed {
            360.0
        } else {
            let t = b - ca;
            let ang = vectors_angle(xx, t)?;
            if yy.dot(t) < 0.0 {
                360.0 - ang
            } else {
                ang
            }
        };

        Ok(CurveType::Circle {
            center: ca,
            normal: na,
            x_axis: xx,
            y_axis: yy,
            radius: ra,
            angle: al,
        })
    }

    /// Derivative of the curvature at `u` (`N_CrvGetCurvatureDeriv`); `0` where
    /// `|C' x C''| < NL_MTOL`.
    ///
    /// Errors with the errors of [`Curve::derivs`], with [`NlibCode::Geometry`] when
    /// the binormal cannot be normalised and with [`NlibCode::Numerical`] when the
    /// final quotient is not computable.
    pub fn curvature_deriv(&self, u: f64, side: Side) -> Result<f64> {
        const OP: &str = "nurbs::crv_nurb::Curve::curvature_deriv";
        let d = self.derivs(u, side, 3)?;
        let d1 = get(&d, 1, OP)?;
        let d2 = get(&d, 2, OP)?;
        let d3 = get(&d, 3, OP)?;

        // Get binormal.
        let z = d1.cross(d2);
        let f1 = z.length();
        if f1 < MODEL_TOL {
            return Ok(0.0);
        }
        let (z, _) = normalize(z)?;

        // Get curvature derivative.
        let den = d1.length();
        let a = d1.cross(d3);
        let f2 = a.dot(z);
        let f1 = den * den;
        let mut num = f1 * f2;

        let f1 = d1.dot(d2);
        let a = d1.cross(d2);
        let f2 = a.dot(z);
        num -= 3.0 * f1 * f2;
        let den = den * den * den * den * den;

        if float_op_is_bad(num, den, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }
        Ok(num / den)
    }

    /// `true` when the curve is G`n` closed (`N_CrvIsClosedContinuity`, `n = 0..=2`):
    /// end points within `tols[0]`, end tangents within `tols[1]` degrees (`n >= 1`),
    /// end curvatures within `tols[2]` (`n = 2`).
    ///
    /// Errors with [`NlibCode::Input`] for `n > 2`, with the errors of
    /// [`Curve::derivs`], with [`NlibCode::Geometry`] when the tangent angle is not
    /// computable and with [`NlibCode::Numerical`] when a curvature is not;
    /// `InvalidInput` when `tols` lacks a tolerance that is read.
    pub fn is_closed_continuity(&self, n: usize, tols: &[f64]) -> Result<bool> {
        const OP: &str = "nurbs::crv_nurb::Curve::is_closed_continuity";
        if n > 2 {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        let tol = |i: usize| tols.get(i).copied().ok_or_else(|| oob(OP));

        // Check continuity.
        let (us, ue) = self.param_bounds();
        let d1 = self.derivs(us, Side::Left, n)?;
        let d2 = self.derivs(ue, Side::Right, n)?;
        let at = |d: &[Vec3], i: usize| d.get(i).copied().ok_or_else(|| oob(OP));

        // Check G0.
        if dist_pt_pt(at(&d1, 0)?, at(&d2, 0)?) > tol(0)? {
            return Ok(false);
        }

        // Check G1.
        if n > 0 {
            let dd = vectors_angle(at(&d1, 1)?, at(&d2, 1)?)?;
            if dd > tol(1)? {
                return Ok(false);
            }
        }

        // Check G2.
        if n > 1 {
            let kappa = |d: &[Vec3]| -> Result<f64> {
                let dn = at(d, 1)?.cross(at(d, 2)?).length();
                let dd = at(d, 1)?.length();
                let dd = dd * dd * dd;
                if float_op_is_bad(dn, dd, FloatOp::Division) {
                    return Err(SolidError::nlib(OP, NlibCode::Numerical));
                }
                Ok(dn / dd)
            };
            let kap1 = kappa(&d1)?;
            let kap2 = kappa(&d2)?;
            if (kap1 - kap2).abs() > tol(2)? {
                return Ok(false);
            }
        }

        // Passed all the tests.
        Ok(true)
    }
}

/// `der + 1` as a capacity.
fn to_len(der: isize) -> usize {
    usize::try_from(der).map_or(0, |d| d.saturating_add(1))
}

/// Binomial coefficients `tri[k][j]`, `k, j <= NL_MAXDER`.
type PascalTriangle = [[i64; MAX_DERIVATIVE + 1]; MAX_DERIVATIVE + 1];

/// Pascal triangle rows `0..=der` as `N_CrvDerivs` builds them.
fn pascal(der: isize, op: &'static str) -> Result<PascalTriangle> {
    let mut tri: PascalTriangle = [[0; MAX_DERIVATIVE + 1]; MAX_DERIVATIVE + 1];
    set2(&mut tri, 0, 0, 1, op)?;
    set2(&mut tri, 1, 0, 1, op)?;
    set2(&mut tri, 1, 1, 1, op)?;
    if der > 1 {
        set2(&mut tri, 2, 0, 1, op)?;
        set2(&mut tri, 2, 1, 2, op)?;
        set2(&mut tri, 2, 2, 1, op)?;
        if der > 2 {
            for k in 3..=der {
                set2(&mut tri, k, 0, 1, op)?;
                let r = k / 2;
                let mut j2 = 1;
                for j in 1..=r {
                    let j1 = get2(&tri, k - 1, j, op)?;
                    let v = get2(&tri, k - 1, j, op)? + j2;
                    set2(&mut tri, k, j, v, op)?;
                    set2(&mut tri, k, k - j, v, op)?;
                    j2 = j1;
                }
                set2(&mut tri, k, k, 1, op)?;
            }
        }
    }
    Ok(tri)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(x: f64, y: f64, z: f64, w: f64) -> CPoint {
        CPoint::new(x, y, z, w)
    }

    /// Unit quarter circle in the xy plane (rational quadratic Bezier).
    fn quarter() -> Curve {
        let w = 0.5_f64.sqrt();
        Curve::new(
            vec![
                cp(1.0, 0.0, 0.0, 1.0),
                cp(w, w, 0.0, w),
                cp(0.0, 1.0, 0.0, 1.0),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    /// Full unit circle from four quarter arcs.
    fn full_circle() -> Curve {
        let w = 0.5_f64.sqrt();
        let pts = [
            (1.0, 0.0, 1.0),
            (w, w, w),
            (0.0, 1.0, 1.0),
            (-w, w, w),
            (-1.0, 0.0, 1.0),
            (-w, -w, w),
            (0.0, -1.0, 1.0),
            (w, -w, w),
            (1.0, 0.0, 1.0),
        ];
        Curve::new(
            pts.iter().map(|&(x, y, w)| cp(x, y, 0.0, w)).collect(),
            2,
            vec![
                0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
            ],
        )
        .unwrap()
    }

    /// Cubic with collinear control points.
    fn line3() -> Curve {
        Curve::new(
            (0..4)
                .map(|i| cp(f64::from(i), 2.0 * f64::from(i), 0.0, NOW))
                .collect(),
            3,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn eval_and_derivs_on_circle() {
        let c = quarter();
        let p = c.eval(0.5, Side::Left).unwrap();
        assert!((p.length() - 1.0).abs() < 1e-14);
        let d = c.derivs(0.5, Side::Left, 2).unwrap();
        assert_eq!(d.len(), 3);
        // Different algorithms (A4.1 vs A2.3 + quotient rule): equal to rounding.
        assert!((d[0] - p).length() < 1e-15);
        // The tangent is perpendicular to the radius.
        assert!(d[1].dot(p).abs() < 1e-12);
        assert_eq!(
            c.eval(2.0, Side::Left).unwrap_err(),
            SolidError::nlib("nurbs::crv_nurb::Curve::eval", NlibCode::Parameter)
        );
        assert!(matches!(
            c.derivs(0.5, Side::Left, 5),
            Err(SolidError::Nlib {
                code: NlibCode::MaxDerivative,
                ..
            })
        ));
    }

    #[test]
    fn curvature_of_unit_circle_is_one() {
        let c = quarter();
        let k = c.eval_curvature(0.3, Side::Left, true).unwrap();
        assert!((k.kappa - 1.0).abs() < 1e-12);
        let (center, normal) = k.circle.unwrap();
        assert!(center.length() < 1e-12);
        assert!((normal.z.abs() - 1.0).abs() < 1e-12);
        let f = c.eval_frenet_frame(0.3, Side::Left).unwrap();
        assert!((f.normal.dot(f.tangent)).abs() < 1e-12);
        assert!(c.curvature_deriv(0.3, Side::Left).unwrap().abs() < 1e-9);
    }

    #[test]
    fn greville() {
        let u = [0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0];
        assert_eq!(greville_abscissa(&u, 2, 0).unwrap(), 0.0);
        assert_eq!(greville_abscissa(&u, 2, 1).unwrap(), 0.5);
        assert_eq!(greville_abscissa(&u, 2, 2).unwrap(), 1.5);
        assert!(greville_abscissa(&u, 0, 0).is_err());
        assert!(greville_abscissa(&u, 2, 5).is_err());
    }

    #[test]
    fn reverse_forms_agree() {
        let c = full_circle();
        let r = c.reversed().unwrap();
        let mut ri = c.clone();
        ri.reverse().unwrap();
        assert_eq!(r, ri);
        assert_eq!(r.cpts()[1], c.cpts()[7]);
        let mut back = r.clone();
        back.reverse().unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn type_recognition() {
        assert!(matches!(
            full_circle().get_type(1e-6).unwrap(),
            CurveType::Circle { angle, .. } if angle == 360.0
        ));
        match quarter().get_type(1e-6).unwrap() {
            CurveType::Circle { radius, angle, .. } => {
                assert!((radius - 1.0).abs() < 1e-9);
                assert!((angle - 90.0).abs() < 1e-9);
            }
            t => panic!("not a circle: {t:?}"),
        }
        assert!(matches!(
            line3().get_type(1e-6).unwrap(),
            CurveType::Line { .. }
        ));
    }

    #[test]
    fn linear_segments() {
        let segs = line3().linear_segs(1e-6).unwrap();
        assert_eq!(
            segs,
            vec![LinearSeg {
                start: 0.0,
                end: 1.0,
                linear: true
            }]
        );
        assert!(!quarter().linear_segs(1e-6).unwrap()[0].linear);
        assert!(line_from_pt_and_vector(Vec3::ZERO, Vec3::X)
            .unwrap()
            .linear_segs(1e-6)
            .is_err());
    }

    #[test]
    fn degen_segments_need_arc_length() {
        assert!(quarter().degen_segs(1e-6).unwrap().is_empty());
        let pt = Curve::new(
            vec![cp(1.0, 1.0, 1.0, NOW); 4],
            3,
            vec![0.0; 4].into_iter().chain([1.0; 4]).collect(),
        )
        .unwrap();
        assert_eq!(pt.degen_segs(1e-6).unwrap(), vec![(0.0, 1.0)]);
    }

    #[test]
    fn closure_and_extents() {
        let c = full_circle();
        assert!(c.is_closed_continuity(2, &[1e-9, 1e-6, 1e-6]).unwrap());
        assert!(!quarter().is_closed_continuity(0, &[1e-9]).unwrap());
        assert!(c.is_closed_continuity(3, &[]).is_err());
        let b = c.bbox().unwrap();
        assert_eq!((b.xl, b.xr, b.yb, b.yt), (-1.0, 1.0, -1.0, 1.0));
        let mm = c.min_max_weights_and_pts();
        assert_eq!((mm.wmin, mm.wmax, mm.pmax), (0.5_f64.sqrt(), 1.0, 0.0));
        assert_eq!(line3().min_max_weights_and_pts().wmin, NOW);
        let (_, mag) = line3().max_pos_vector();
        assert_eq!(mag, 45.0_f64.sqrt());
    }

    #[test]
    fn maps() {
        let mut c = line3();
        c.translate(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(c.cpts()[0].x, 1.0);
        c.scale(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0));
        assert_eq!(c.cpts()[1].y, 4.0);
        let q = quarter()
            .project_onto_plane(
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::Z,
                Vec3::Z,
                Projection::Parallel,
            )
            .unwrap();
        assert!(q.cpts().iter().all(|p| p.z == p.w));
        assert!(quarter()
            .project_onto_plane(Vec3::ZERO, Vec3::Z, Vec3::X, Projection::Parallel)
            .is_err());
        let mut r = quarter();
        r.reparam(2.0, 4.0).unwrap();
        assert_eq!(r.knots(), &[2.0, 2.0, 2.0, 4.0, 4.0, 4.0]);
    }

    /// Fuzz regression: a huge degree overflowed the index arithmetic of
    /// `get_type` and `linear_segs`.
    #[test]
    fn huge_degree_is_an_error() {
        let c = quarter();
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let h = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            let r = h.get_type(1e-6);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            let r = h.linear_segs(1e-6);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
    }
}
