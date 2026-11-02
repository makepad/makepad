//! `SmBSplineCurve`, part 3: scale-translate-rotate copies, internal pole and segment
//! line tests, single control point access, plane containment, joining another curve
//! onto this one, subdivision at discontinuities, closing small gaps between curves
//! and the `AssertValid` checks.
//!
//! Upstream: `source/SMLib/src/SmBSplineCurve.cpp`, `source/SMLib/inc/SmBSplineCurve.h`.
//!
//! | Rust                                              | Upstream                               |
//! |---------------------------------------------------|----------------------------------------|
//! | [`BSplineCurve::create_by_scale_trans_rot`]       | `CreateByScaleTransRot`                |
//! | [`BSplineCurve::has_internal_pole`]               | `HasInternalPole`                      |
//! | [`BSplineCurve::is_segment_line`], [`SegmentLine`] | `IsSegmentLine`                       |
//! | [`BSplineCurve::control_point`]                   | `GetControlPoint`                      |
//! | [`BSplineCurve::is_on_plane`], [`OnPlane`]        | `IsOnPlane`                            |
//! | [`BSplineCurve::join_with`]                       | `JoinWith`                             |
//! | [`BSplineCurve::subdivide_at_discontinuity`]      | `SubdivideAtDiscontinuity`             |
//! | [`BSplineCurve::trim_gap`], [`GapFix`]            | `TrimGap`                              |
//! | [`BSplineCurve::assert_valid`]                    | `AssertValid`                          |
//! | [`BSplineCurve::assert_no_periodic_jump`], [`assert_no_periodic_jump_of`] | `AssertNoPeriodicJump` |
//!
//! The other members of the file belong to `nurbs.b_spline_curve_p1`, `_p2` and
//! `nurbs.b_spline_curve_n_lib`.
//!
//! Deviations:
//! - **Out-params, contexts.** Outputs are return values; `SmContext` arguments (and
//!   the local context of `TrimGap`) only choose where upstream allocates and are
//!   dropped. `IsSegmentLine` and `IsOnPlane` leave some outputs unset on some paths;
//!   those are `None` in [`SegmentLine`] / [`OnPlane`]. `IsSegmentLine` returns an
//!   `SmStatus` through its `SmBoolean` when an evaluation fails (`SER`); the port
//!   returns the error. Its sample count is bounded by [`MAX_SEGMENT_SAMPLES`]
//!   (`InvalidInput` above it, checked only where upstream starts sampling): upstream
//!   evaluates any count, so an absurd one spins for hours.
//! - **Join ends** (`ULONG` 0 = start, 1 = end) are `bool`s (`true` = end), the
//!   `TrimGap` mode (`int` 0, 1, 2) is [`GapFix`] (upstream treats any other value as
//!   0) and its `int` result a `bool`.
//! - **Join in place.** `JoinWith` hands `this` and the other curve to
//!   `CreateByJoining`, which may elevate or flatten them in place; the port moves both
//!   into the segment slice and back, so the other curve sees the same edits.
//! - **Ignored statuses.** Where upstream ignores a status (the `TrimGap` evaluations,
//!   extensions, trims and control point edits, `SubdivideAtDiscontinuity`'s trims,
//!   `AssertValid`'s evaluations and bounding box), the port does too, except that
//!   [`SolidError::Unported`] is always passed on. Ignored evaluations leave
//!   default-constructed `SmVector3d`s ([`Vec3::UNDEF`]), as upstream.
//! - **Crashes.** Where upstream dereferences a `NULL` (a curve without NURB in
//!   `GetControlPoint` / `IsSegmentLine`, an extension `TrimGap` did not make, an
//!   unclamped knot vector indexing before the control points in `HasInternalPole`)
//!   the port returns `SM_ERR` or `InvalidInput`. `GetControlPoint` of a curve of
//!   dimension 1 is `InvalidInput` (see `GetControlPointsExpert` in `p2`).
//! - **`AssertValid`.** The report list is an [`AssertArray`]; `this` is identified by
//!   the caller's [`ObjectKey`]. Only `SM_LEVEL_GIVEN` changes anything without
//!   `SM_DEBUG_CODE` (it skips the base class checks), so the level is the
//!   `level_given` flag; the walk flag and test requests are unused upstream and
//!   dropped. `MakeNurb` cannot make a NURB for a plain B-spline, so the method takes
//!   `&self`. `SM_ASSERT_SET_VALUE` / `SM_ASSERT_SET_OWNER_PARAM` after a report are
//!   applied to the new report before it is added (the same result: they only touch
//!   a report that was added, and report equality ignores those fields). `WARN`
//!   messages are not ported.
//! - **Back-edges.** `HasInternalPole` calls `IsDegenerate` (`p2`); `IsSegmentLine`,
//!   `IsOnPlane` and `JoinWith` test `IsKindOf(SmLine_TYPE)` (false for a
//!   `BSplineCurve`; a line is a [`crate::curve::line::Line`], whose `JoinWith`
//!   overrides this one) and `JoinWith` measures `SmCurve::ApproximateLength` without
//!   a gap tolerance; `SubdivideAtDiscontinuity` calls `CalculateContinuities`
//!   (`SmCurve::EvaluateContinuity`);
//!   `TrimGap` intersects with `SmCurve::GlobalCurveIntersect` (`curve.curve_p8`,
//!   whose status upstream ignores: a failure finds no intersection here, upstream
//!   keeps the solutions found before it); `AssertValid` calls
//!   `SmCurve::GlobalPropertyAnalysis` (`curve.curve_p7`); its
//!   `SmCurve::AssertValid` is [`crate::curve::curve::p4::assert_valid_base`] for a
//!   curve without owner. Those calls return
//!   [`SolidError::Unported`] until the units are ported. `SmTol::IsDegenerate(this)`
//!   (unit `geom.tol`, which does not port the object dispatch) is computed locally for
//!   a curve without edge: `IsDegenerate` at twice the system zone tolerance, so
//!   `AssertValid` takes the [`Kernel`]; `SmCurve::IsAnalytic` / `IsBounded` are the
//!   base `FALSE` / `TRUE`.

use crate::curve::curve::p2::{Curve, CurvePropertyType};
use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::error::{ignored, Result, SolidError};
use crate::geom::assert_array::{labels, AssertArray, AssertReport, ObjectKey, ReportSpec};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_point_distance, plane_point_distance, point_project_to_plane,
    segment_point_distance,
};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::solution_array::{ObjectKind, SolutionEnd, SolutionObject, SolutionType};
use crate::geom::tol::{
    is_zero_to_tol, tol3d, xsect_tol_3d_from_zones, zone_tol_3d, SmallTopoType, ZONE_TOL_3D,
};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, max3, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_curve::p2::{BoundingBoxRequest, ControlPointForm};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::geom::dist_cpt_cpt;
use crate::nurbs::nurbs_crv::to_euclid;
use crate::surface::surface::Surface;

/// `SmLine_TYPE` (`CURV_BASE_TYPE + 10`).
const LINE_TYPE: i64 = BSplineCurve::TYPE - 33 + 10;

/// Largest sample count [`BSplineCurve::is_segment_line`] accepts (a port bound,
/// far above upstream's callers; upstream evaluates any count).
pub const MAX_SEGMENT_SAMPLES: u64 = 1 << 20;

/// Cosine of `SM_CONTINUITY_ANGLE` (1 degree), as written in `AssertValid`.
const COSINE_TOL: f64 = 0.999_847_695_156_391_3;

const OP_SCALE: &str = "nurbs::b_spline_curve::p3::BSplineCurve::create_by_scale_trans_rot";
const OP_POLE: &str = "nurbs::b_spline_curve::p3::BSplineCurve::has_internal_pole";
const OP_SEG: &str = "nurbs::b_spline_curve::p3::BSplineCurve::is_segment_line";
const OP_CP: &str = "nurbs::b_spline_curve::p3::BSplineCurve::control_point";
const OP_JOIN: &str = "nurbs::b_spline_curve::p3::BSplineCurve::join_with";
const OP_SUBDIV: &str = "nurbs::b_spline_curve::p3::BSplineCurve::subdivide_at_discontinuity";
const OP_GAP: &str = "nurbs::b_spline_curve::p3::BSplineCurve::trim_gap";
const OP_ASSERT: &str = "nurbs::b_spline_curve::p3::BSplineCurve::assert_valid";

/// The outputs of [`BSplineCurve::is_segment_line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentLine {
    /// The `SmBoolean` result: the segment is a line.
    pub is_line: bool,
    /// The curve start point and chord (`rLinePointArg`, `rLineVectorArg`), when
    /// upstream sets them (also for a short chord whose midpoint coincides with the
    /// ends, which is not reported as a line).
    pub line: Option<(Point3, Vec3)>,
}

/// The outputs of [`BSplineCurve::is_on_plane`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnPlane {
    /// All curve points are within the tolerance of the plane (`rbIsOnPlaneArg`).
    pub on_plane: bool,
    /// The largest distance of the end, middle and control points to the plane
    /// (`rdMaxDistToPlaneArg`); `None` when a sample point is off the plane (upstream
    /// leaves it unset).
    pub max_dist: Option<f64>,
}

/// Which end points [`BSplineCurve::trim_gap`] moves (`CrvToFix`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GapFix {
    /// `0`: both curves to the midpoint of the extended curves' intersection ends.
    Both = 0,
    /// `1`: both curves to this curve's initial end point.
    ToThis = 1,
    /// `2`: both curves to the other curve's initial end point.
    ToOther = 2,
}

// ---- back-edges -------------------------------------------------------------------

/// `SmCurve::ApproximateLength` of `curve` over `interval` with `samples` samples.
fn approximate_length(
    k: &Kernel,
    curve: &BSplineCurve,
    interval: &Extent1d,
    samples: usize,
) -> Result<f64> {
    Ok(crate::curve::curve::p2::approximate_length(k, curve, interval, samples, false)?.0)
}

/// A curve of [`global_curve_intersect`] in its solutions (`0`: the first curve, `1`:
/// the second, `2` / `3`: their analytic replacements) and the natural interval of
/// the curve (upstream's objects are the curves, whose natural intervals
/// `IdenticalSolutions` reads).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GapCurve(u8, Extent1d);

impl SolutionObject for GapCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// `SmCurve::GlobalCurveIntersect` of `curve0` over `ivl0` with `curve1` over `ivl1`
/// at tolerance `tol`: the solutions' types and start ends. The status is ignored
/// upstream: a failed intersection gives no solutions here (upstream keeps those found
/// before the failure); [`SolidError::Unported`] is passed on.
fn global_curve_intersect(
    k: &Kernel,
    curve0: &BSplineCurve,
    ivl0: &Extent1d,
    curve1: &BSplineCurve,
    ivl1: &Extent1d,
    tol: f64,
) -> Result<Vec<(SolutionType, SolutionEnd)>> {
    let c0 = Curve::from(curve0.clone());
    let c1 = Curve::from(curve1.clone());
    let opts = CurveIntersectOptions {
        skip_global_coincidence_check: false,
        handles: [
            Some(GapCurve(0, c0.natural_interval())),
            Some(GapCurve(1, c1.natural_interval())),
        ],
        analytic_handles: [
            Some(GapCurve(2, c0.natural_interval())),
            Some(GapCurve(3, c1.natural_interval())),
        ],
    };
    let found = ignored(c0.global_curve_intersect(k, ivl0, &c1, ivl1, tol, &opts))?;
    Ok(found.map_or_else(Vec::new, |sols| {
        sols.as_slice()
            .iter()
            .map(|s| (s.solution_type, s.start))
            .collect()
    }))
}

/// The property `SmCurve::GlobalPropertyAnalysis` minimizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurveProperty {
    /// `SM_CP_MINIMIZE_FIRST_DERIVATIVE`.
    MinimizeFirstDerivative,
    /// `SM_CP_MINIMIZE_RADIUS_OF_CURVATURE`.
    MinimizeRadiusOfCurvature,
}

/// The analysed curve in the solutions of [`global_property_analysis`] (unread).
#[derive(Debug, Clone, Copy, PartialEq)]
struct SelfCurve;

impl SolutionObject for SelfCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

/// `SmCurve::GlobalPropertyAnalysis` of `curve` over `domain` (no extra arguments) with
/// minimum separation `min_sep`: the solutions' start ends. The status is ignored
/// upstream: a failed analysis gives no solutions here (upstream keeps those found
/// before the failure); [`SolidError::Unported`] is passed on.
fn global_property_analysis(
    k: &Kernel,
    curve: &BSplineCurve,
    domain: &Extent1d,
    property: CurveProperty,
    min_sep: f64,
) -> Result<Vec<SolutionEnd>> {
    let property = match property {
        CurveProperty::MinimizeFirstDerivative => CurvePropertyType::MinimizeFirstDerivative,
        CurveProperty::MinimizeRadiusOfCurvature => CurvePropertyType::MinimizeRadiusOfCurvature,
    };
    let crv = Curve::from(curve.clone());
    let opts: PropertyAnalysisOptions<'_, SelfCurve> = PropertyAnalysisOptions::default();
    let found = ignored(crv.global_property_analysis(k, domain, property, min_sep, &opts))?;
    Ok(found.map_or_else(Vec::new, |r| {
        r.solutions.as_slice().iter().map(|s| s.start).collect()
    }))
}

/// `SmCurve::AssertValid(pAList, eTestLevel, SM_NO_WALK, pTestRequests)` of `curve`
/// (reporting for `key`); a B-spline here has no owner.
fn curve_assert_valid(
    curve: &BSplineCurve,
    list: Option<&mut AssertArray>,
    key: ObjectKey,
) -> Result<bool> {
    crate::curve::curve::p4::assert_valid_base(
        None,
        curve.dim(),
        "SmBSplineCurve",
        list,
        false,
        key,
    )
}

/// `SmTol::IsDegenerate(const SmObject*)` of `curve` (its `SmCurve_TYPE` branch,
/// computed here): `IsDegenerate` at the intersection tolerance of two zone
/// tolerances of the curve, where a curve without edge (a B-spline here has no owner)
/// has the system zone tolerance (`SmTol::GetZoneTol3d(const SmObject*)`).
fn tol_is_degenerate(k: &Kernel, is_degenerate: &dyn Fn(f64) -> Result<bool>) -> Result<bool> {
    let zone = zone_tol_3d(k, SmallTopoType::NormalSize);
    let xsect = xsect_tol_3d_from_zones(k, zone, zone);
    is_degenerate(xsect)
}

/// The virtual methods of a class derived from `SmBSplineCurve` that
/// `SmBSplineCurve::AssertValid` calls on `this`.
pub(crate) struct DerivedOverrides<'a> {
    /// `GetDegree` (`None`: the NURB's degree, the base method).
    pub degree: Option<usize>,
    /// `IsAnalytic`.
    pub analytic: bool,
    /// `IsBounded`.
    pub bounded: bool,
    /// `IsDegenerate(tol)` over the natural interval.
    pub is_degenerate: &'a dyn Fn(f64) -> Result<bool>,
    /// Not upstream: the surface of the face whose UV trim this curve is
    /// (`GetEdgeuse()->GetFace()->GetSurface()`), read only with
    /// `Kernel::fixes.uv_trim_checks_on_surface` (WR-560); `None` for any other curve.
    pub trim_surface: Option<&'a Surface>,
}

/// The first fundamental form `[E, F, G]` of `surface` at `uv`, its derivatives taken
/// on the given sides (the fork's `sm_FirstFundamentalForm`, as
/// `topology::brep::validation::pointers` has it for WR-406); `None` where the surface
/// cannot be evaluated.
fn first_fundamental_form(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    u_from_left: bool,
    v_from_left: bool,
) -> Result<Option<[f64; 3]>> {
    match surface.evaluate_1st_derivatives(k, uv, u_from_left, v_from_left) {
        Ok((_, du, dv)) => Ok(Some([du.dot(du), du.dot(dv), dv.dot(dv)])),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// Whether a first fundamental form is positive definite: no direction of zero length
/// (not a singular point of its surface) (the fork's `sm_IsPositiveDefinite`).
fn is_positive_definite(efg: [f64; 3]) -> bool {
    efg[0] > 0.0 && efg[0] * efg[2] - efg[1] * efg[1] > 0.0
}

/// The map `(u, v) -> (a u + b v, c v)` that measures a UV trim's steps as lengths on
/// its face's surface (the fork's `sm_TrimMetric`, WR-560): `[a, b, c]`, the Cholesky
/// factor `L` of the surface's first fundamental form (`|L d|^2 = d.G.d`) at the
/// trim's middle, or at the centre of the surface's natural domain where the middle is
/// a singular point. `None` for a curve that is no UV trim of a face (`surface` `None`,
/// a dimension other than 2), an unbounded one, or one whose surface has no metric at
/// either point.
fn trim_metric(
    k: &Kernel,
    trim: &BSplineCurve,
    bounded: bool,
    surface: Option<&Surface>,
    domain: &Extent1d,
) -> Result<Option<[f64; 3]>> {
    let Some(surface) = surface.filter(|_| trim.dim == 2) else {
        return Ok(None);
    };
    if !bounded {
        return Ok(None);
    }
    let form_at = |uv: Point2| -> Result<Option<[f64; 3]>> {
        Ok(first_fundamental_form(k, surface, uv, true, true)?
            .filter(|efg| is_positive_definite(*efg)))
    };
    let mut form = match trim.evaluate_point(domain.evaluate(0.5)) {
        Ok(mid) => form_at(Point2::new(mid.x, mid.y))?,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => None,
    };
    if form.is_none() {
        let centre = match surface.natural_uv_domain_with_nurb(k) {
            Ok(d) => Some(d.evaluate(0.5, 0.5)),
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => None,
        };
        if let Some(centre) = centre {
            form = form_at(centre)?;
        }
    }
    let Some(efg) = form else {
        return Ok(None);
    };
    let a = efg[0].sqrt();
    let b = efg[1] / a;
    let c = (efg[0] * efg[2] - efg[1] * efg[1]).sqrt() / a;
    Ok(Some([a, b, c]))
}

/// A copy of `trim` with its control points mapped by [`trim_metric`]'s `l` (the
/// fork's `sm_MappedTrim`): linear, so it maps homogeneous control points as it maps
/// cartesian ones. `None` when the copy has no NURB.
fn mapped_trim(trim: &BSplineCurve, l: [f64; 3]) -> Option<BSplineCurve> {
    let mut copy = trim.clone();
    let nurb = copy.nurb.as_mut()?;
    for pw in nurb.cpts_mut() {
        let (x, y) = (pw.x, pw.y);
        pw.x = l[0] * x + l[1] * y;
        pw.y = l[2] * y;
    }
    Some(copy)
}

/// `SmCurve::IsAnalytic` of a B-spline: the base `FALSE` (`SmBSplineCurve` does not
/// override it; see [`crate::curve::curve::Curve::is_analytic`]).
const fn curve_is_analytic(_curve: &BSplineCurve) -> bool {
    false
}

/// `SmCurve::IsBounded` of a B-spline: the base `TRUE` (`SmBSplineCurve` does not
/// override it; see [`crate::curve::curve::Curve::is_bounded`]).
const fn curve_is_bounded(_curve: &BSplineCurve) -> bool {
    true
}

// ---- helpers ----------------------------------------------------------------------

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// An `EvaluatePoint` whose status is ignored: [`Vec3::UNDEF`] (the
/// default-constructed point) on failure.
fn point_or_undef(c: &BSplineCurve, t: f64) -> Result<Point3> {
    Ok(ignored(c.evaluate_point(t))?.unwrap_or(Vec3::UNDEF))
}

/// The `SM_ASSERT_*_REPORT` macros of `AssertValid` (`SM_LEVEL_0` tests, so the
/// predicate always runs): `true` when `passed`; otherwise, with a list, a report for
/// test `test_index` (tolerance `tol`, value `val`, then `set` applied) is added
/// unless an equal one is there.
fn report(
    list: &mut Option<&mut AssertArray>,
    key: ObjectKey,
    test_index: u64,
    passed: bool,
    tol: f64,
    val: f64,
    set: impl FnOnce(&mut AssertReport),
) -> Result<bool> {
    if passed {
        if let Some(l) = list.as_deref_mut() {
            l.set_last_add(false);
        }
        return Ok(true);
    }
    if let Some(l) = list.as_deref_mut() {
        let mut r = AssertReport::new(ReportSpec {
            reporter: Some(key),
            test_index,
            test_class: labels::B_SPLINE_CURVE_TYPE,
            owner: Some(key),
            owner_type: labels::B_SPLINE_CURVE_TYPE,
            owner_type_string: Some("SmBSplineCurve"),
            other: None,
            other_type: 0,
            other_type_string: Some("NULL"),
            tol3d: tol3d(tol),
            value: val,
            list_index: 0,
            message: None,
        })?;
        set(&mut r);
        l.add_unique_delete_duplicate(r);
    }
    Ok(false)
}

/// `SM_ASSERT_BOOLEAN_REPORT`: [`report`] without tolerance and value.
fn bool_report(
    list: &mut Option<&mut AssertArray>,
    key: ObjectKey,
    test_index: u64,
    passed: bool,
) -> Result<bool> {
    report(
        list,
        key,
        test_index,
        passed,
        UNDEF_DOUBLE,
        UNDEF_DOUBLE,
        |_| {},
    )
}

impl BSplineCurve {
    /// A copy whose control points are moved along a blend of `start_vec` and
    /// `end_vec` (by their projection on the chord: `start_vec` at the start point,
    /// `end_vec` at the end point) and rotated by `rotation_deg` about the moved chord
    /// (`CreateByScaleTransRot`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a closed curve (end points within `SM_EFF_ZERO`) or without a NURB;
    /// `InvalidInput` (`SM_ERR_INVALID_INPUT`) when the moved end points coincide.
    pub fn create_by_scale_trans_rot(
        &self,
        start_vec: Vec3,
        end_vec: Vec3,
        rotation_deg: f64,
    ) -> Result<Self> {
        let ivl = self.natural_interval();
        let start = self.evaluate_point(ivl.min())?;
        let end = self.evaluate_point(ivl.max())?;

        if start.closer_than(EFF_ZERO, end) {
            return Err(SolidError::sm_err(OP_SCALE));
        }
        let mut res = self.clone();

        let start2 = start + start_vec;
        let end2 = end + end_vec;
        let rot_vec = (end2 - start2).unitize()?;

        let mut rot = Axis2Placement::default();
        rot.rotate_about_axis_at_point(rotation_deg * PI / 180.0, start2, rot_vec);

        let form = if res.is_rational()? {
            ControlPointForm::EuclideanRational
        } else {
            ControlPointForm::NonRational
        };

        // Now Let's go through and project each control point down onto
        // the line and use that to compute a transformation.
        let line_vec = end - start;
        for i in 0..res.number_control_points()? {
            let (pnt, weight) = res.control_point(form, i)?;
            // Parameter contains relative stength of start v.s. end.
            let param = line_closest_point(start, line_vec, pnt);
            // dParam may be outside [0, 1]: the effect of the vectors continues.
            let trans_vec = start_vec + param * (end_vec - start_vec);
            let pnt = pnt + trans_vec; // Move point
            let pnt = rot.transform_point(pnt); // Rotate point

            // Now put point back into curve
            res.set_control_point(form, i, pnt, weight)?;
        }
        Ok(res)
    }

    /// Whether some span has `degree + 1` coincident control points (within `tol`,
    /// `SM_ZONE_TOL_3D` when `tol <= 0`), mapping the whole span to one point
    /// (`HasInternalPole`). `false` without a NURB and for a degenerate curve.
    ///
    /// # Errors
    ///
    /// Those of the degeneracy test; `InvalidInput` for an unclamped knot vector
    /// (upstream reads outside the control points).
    pub fn has_internal_pole(&self, k: &Kernel, tol: f64) -> Result<bool> {
        let mut rtn = false;

        // no work - no BSplineCurve representation
        let Some(cur) = self.nurb.as_ref() else {
            return Ok(false);
        };

        // no work - degenerate curve
        if self.is_degenerate(k, tol, None)? {
            return Ok(false);
        }

        let degree = cur.degree();
        let (unique_knots, mults) = self.knots(None)?;
        let mut knot_count = 0usize;

        // pick tolerance
        let tolerance = if tol > 0.0 { tol } else { ZONE_TOL_3D };
        let pw = cur.cpts();
        let cpt = |i: usize| {
            pw.get(i)
                .map(|&p| to_euclid(p))
                .ok_or_else(|| invalid(OP_POLE, "control point index out of range"))
        };

        // for every knot span - seek sets of coincident control points
        let mut ii = 0usize;
        while ii + 1 < unique_knots.len() && !rtn {
            knot_count += mults
                .get(ii)
                .copied()
                .ok_or_else(|| invalid(OP_POLE, "missing multiplicity"))?;

            // the 1st control point whose basis function is nonzero on the span
            let cp_index = knot_count
                .checked_sub(1 + degree)
                .ok_or_else(|| invalid(OP_POLE, "knot vector not clamped"))?;
            let pt = cpt(cp_index)?;

            let mut test = true;
            // for the next Degree number of neighbor control points
            let mut jj = 1usize;
            while jj <= degree && test {
                let pt_u = cpt(cp_index + jj)?;
                test &= pt.closer_than(tolerance, pt_u);
                jj += 1;
            }

            // remember when ivl is degenerate
            rtn |= test;
            ii += 1;
        }
        Ok(rtn)
    }

    /// Whether the curve over `interval` is a line, from `sample_count` (at least 3)
    /// samples and the tolerance `tol` (`IsSegmentLine`): every interior sample within
    /// `tol` of the chord, and its derivative's sideways drift over a sample step
    /// within `tol`. A chord shorter than `tol` only checks the middle sample, and is
    /// never a line (the outputs are set when the middle coincides with the ends).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when an evaluation fails; `InvalidInput` for more
    /// than [`MAX_SEGMENT_SAMPLES`] samples.
    pub fn is_segment_line(
        &self,
        interval: &Extent1d,
        sample_count: u64,
        tol: f64,
    ) -> Result<SegmentLine> {
        if self.nurb.is_none() {
            return Err(SolidError::sm_err(OP_SEG));
        }
        let mut sample_count = sample_count;

        // curve end points
        let start_pt = self.evaluate_point(interval.min())?;
        let end_pt = self.evaluate_point(interval.max())?;

        // Used only if the chord length is substantial
        let chord = end_pt - start_pt;
        let mut unit_chord = Vec3::default(); // set if chord sane

        // If the chord is short wrt the tolerance, use only 3 points
        let chord_length = (start_pt - end_pt).length();
        let mut chord_short = false;

        // for endpoints close together (short lines and closed curves)
        if chord_length < tol {
            sample_count = 3;
            chord_short = true;
        } else {
            // unitize the chord (the status only feeds an assert)
            unit_chord = chord.unitize_or_z();
        }

        // When this curve is of type SmLine
        if self.is_kind_of(LINE_TYPE)? {
            // (SE(SM_ERR) only reports for a short chord)
            return Ok(SegmentLine {
                is_line: true,
                line: Some((start_pt, chord)),
            });
        }

        // Use no less than 3 points
        if sample_count < 3 {
            sample_count = 3;
        }
        // (port bound: upstream samples any count, spinning for absurd ones)
        if sample_count > MAX_SEGMENT_SAMPLES {
            return Err(SolidError::invalid_input(
                OP_SEG,
                "sample count above MAX_SEGMENT_SAMPLES",
            ));
        }

        // Evaluating the curve and its derivative in all internal points
        let mut i = 1u64;
        while i + 1 < sample_count {
            let param = interval.evaluate(i as f64 / (sample_count as f64 - 1.0));
            let pt_dir = self.evaluate(param, 1, true, false)?;
            let p0 = pt_dir.first().copied().unwrap_or(Vec3::UNDEF);
            let p1 = pt_dir.get(1).copied().unwrap_or(Vec3::UNDEF);

            // If the segment is short, check if the midpoint coincided with
            // the start/end points, and decide upon the result
            if chord_short {
                let di = (p0 - start_pt).length();
                if di > tol {
                    return Ok(SegmentLine {
                        is_line: false,
                        line: None,
                    });
                }
                // DANGER: line, degenerating to a single point (SE(SM_ERR) only
                // reports)
                return Ok(SegmentLine {
                    is_line: false,
                    line: Some((start_pt, chord)),
                });
            }

            // the chord is long now: check the distance between the curve point
            // and the assumed line
            let (dist, _) = line_point_distance(start_pt, chord, p0);
            if dist > tol {
                return Ok(SegmentLine {
                    is_line: false,
                    line: None,
                });
            }

            let deriv_len = p1.length();

            // we have no other clue wrt the world's resolution than the eps given to us
            if deriv_len > tol {
                let sin_angle = p1.cross(unit_chord).length() / deriv_len;
                let discr = sin_angle.abs() * chord_length / sample_count as f64;
                // the derivative should not cause a large departure from the line
                if discr > tol {
                    return Ok(SegmentLine {
                        is_line: false,
                        line: None,
                    });
                }
            }
            i += 1;
        }

        // If we are here, it should be a line
        Ok(SegmentLine {
            is_line: true,
            line: Some((start_pt, chord)),
        })
    }

    /// Control point `index` in `form` and its weight (`GetControlPoint`): through
    /// [`BSplineCurve::control_points_expert`] with a stride of 4 into `[_, _, 1, 1]`;
    /// a 2D curve gets `z = 0` and the weight from the third slot.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index above `n` or a dimension below 2; `SM_ERR` without
    /// a NURB.
    pub fn control_point(&self, form: ControlPointForm, index: usize) -> Result<(Point3, f64)> {
        // Default for non-rational, 2d or 3d.
        let mut tmp = [0.0, 0.0, 1.0, 1.0];
        let vals = self.control_points_expert(form, index, index, 4)?;
        // the slots GetControlPointsExpert writes (the others keep their defaults)
        let written =
            2 + usize::from(self.dim > 2) + usize::from(form != ControlPointForm::NonRational);
        for (slot, v) in tmp.iter_mut().zip(&vals).take(written) {
            *slot = *v;
        }
        let [x, y, z, w] = tmp;
        let mut control_point = Vec3::new(x, y, z);
        let mut weight = w;
        if self.dim == 2 {
            control_point.z = 0.0;
            weight = z;
        }
        if vals.len() < written {
            return Err(invalid(OP_CP, "control point values missing"));
        }
        Ok((control_point, weight))
    }

    /// Whether the curve over `interval` (the natural interval for `None`) lies within
    /// `tol` of the plane through `plane_point` with normal `plane_normal`
    /// (`IsOnPlane`): its start, middle and end points, then (unless it is a line)
    /// the control points of the spans covering the interval.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, when an evaluation fails or for a non-unit normal
    /// (`smgu_PlanePointDistance`); `GetPolygon`'s errors.
    pub fn is_on_plane(
        &self,
        plane_point: Point3,
        plane_normal: Vec3,
        tol: f64,
        interval: Option<&Extent1d>,
    ) -> Result<OnPlane> {
        let iv = match interval {
            Some(i) => *i,
            None => self.natural_interval(),
        };

        // get Curve start, mid, end points
        let start_pt = self.evaluate_point(iv.min())?;
        let end_pt = self.evaluate_point(iv.max())?;
        let mid_pt = self.evaluate_point(iv.evaluate(0.5))?;

        // project points to plane
        let proj_start_pt = point_project_to_plane(start_pt, plane_point, plane_normal);
        let proj_end_pt = point_project_to_plane(end_pt, plane_point, plane_normal);
        let proj_mid_pt = point_project_to_plane(mid_pt, plane_point, plane_normal);

        // get start, mid, end point distances to plane
        let st_discr = (start_pt - proj_start_pt).length();
        let end_discr = (end_pt - proj_end_pt).length();
        let mid_discr = (mid_pt - proj_mid_pt).length();

        // low work - points not on plane
        if st_discr > tol || end_discr > tol || mid_discr > tol {
            return Ok(OnPlane {
                on_plane: false,
                max_dist: None,
            });
        }

        // get max of 3 sample point distances - good enough for lines
        let mut max_dist = max3(st_discr, mid_discr, end_discr);

        // for nonLine curves - get max distance to controlPoint
        if !self.is_kind_of(LINE_TYPE)? {
            for pnt in self.polygon(&iv)? {
                let dist = plane_point_distance(plane_point, plane_normal, pnt)?;
                if dist > max_dist {
                    max_dist = dist;
                }
            }
        }

        Ok(OnPlane {
            on_plane: max_dist < tol,
            max_dist: Some(max_dist),
        })
    }

    /// Joins `other` onto this curve (`JoinWith`): this curve's end (`this_at_end`) to
    /// the other's end (`other_at_end`), which must be within `gap_tolerance` (by
    /// default `1000 * SM_EFF_ZERO_SQRT` times the approximate lengths). Two lines that
    /// continue each other become one line segment; otherwise the curves are joined
    /// with [`BSplineCurve::create_by_joining`] (which may change `other` in place).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for ends too far apart or when the join fails or without a NURB;
    /// [`SolidError::Unported`] from `IsLine`'s back-edges.
    pub fn join_with(
        &mut self,
        k: &Kernel,
        this_at_end: bool,
        other: &mut Self,
        other_at_end: bool,
        gap_tolerance: Option<f64>,
    ) -> Result<()> {
        let ivl1 = self.natural_interval();
        let ivl2 = other.natural_interval();
        let end_this = f64::from(u8::from(this_at_end));
        let end_other = f64::from(u8::from(other_at_end));

        // Get a tolerance to use.
        let tol = if let Some(t) = gap_tolerance {
            t
        } else {
            let length =
                approximate_length(k, self, &ivl1, 10)? + approximate_length(k, other, &ivl2, 10)?;
            length * EFF_ZERO_SQRT * 1000.0
        };

        // Make sure that the points of join ends are close enough
        let pnt1 = self.evaluate_point(ivl1.evaluate(end_this))?;
        let pnt2 = other.evaluate_point(ivl2.evaluate(end_other))?;
        if pnt1.distance(pnt2) > tol {
            return Err(SolidError::sm_err(OP_JOIN));
        }

        // First let's handle the simple case of two lines with the same vector.
        if self.is_line(5, tol)?.is_some() && other.is_line(5, tol)?.is_some() {
            // Create a line from the non-joining ends and test the other points.
            let pnt1b = self.evaluate_point(ivl1.evaluate(1.0 - end_this))?;
            let pnt2b = other.evaluate_point(ivl2.evaluate(1.0 - end_other))?;
            let mut can_join_lines = false;
            let (dist, _) = segment_point_distance(pnt2b, pnt1b, pnt1);
            if dist < tol {
                // Only can join continuous lines.
                let (dist, _) = segment_point_distance(pnt2b, pnt1b, pnt2);
                if dist < tol {
                    can_join_lines = true;
                }
            }
            if can_join_lines {
                // (upstream's `SM_CAST_NONNULL_PTR(SmLine, this)` branch extends an
                // `SmLine`'s STEP interval; a `BSplineCurve` is never a line here, lines
                // join through `crate::curve::line::Line::join_with`)
                let new_line = if this_at_end {
                    Self::create_line_segment(self.dim, pnt1b, pnt2b)?
                } else {
                    Self::create_line_segment(self.dim, pnt2b, pnt1b)?
                };
                return self.set_from_gw_nurb(new_line.gw_nurb());
            }
        }

        // Try using nurb-join: the segments in join order and their senses
        let this_curve = std::mem::replace(self, Self::new(self.dim));
        let other_curve = std::mem::replace(other, Self::new(other.dim));
        let (mut curves, orients, this_first) = if this_at_end {
            ([this_curve, other_curve], [true, !other_at_end], true)
        } else {
            ([other_curve, this_curve], [other_at_end, true], false)
        };
        let composite = Self::create_by_joining(&mut curves, Some(&orients));
        let [a, b] = curves;
        let (this_back, other_back) = if this_first { (a, b) } else { (b, a) };
        *self = this_back;
        *other = other_back;
        let composite = match composite {
            Ok(c) => c,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_JOIN)),
        };
        self.set_from_gw_nurb(composite.gw_nurb())
    }

    /// Copies of the curve split at every interior knot whose continuity is at most
    /// `continuity_to_split` (with the angle tolerance `continuity_angle_tol`), each
    /// trimmed to its piece (`SubdivideAtDiscontinuity`); a single copy when nothing
    /// splits it.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB; those of `SmCurve::EvaluateContinuity`.
    pub fn subdivide_at_discontinuity(
        &self,
        k: &Kernel,
        continuity_to_split: ContinuityType,
        continuity_angle_tol: f64,
    ) -> Result<Vec<Self>> {
        let mut resulting_curves = Vec::new();

        let (knots, _) = self.knots(None)?; // Unique knot vector
        let (_, continuities) = self.calculate_continuities(k, continuity_angle_tol)?;

        // check state
        if knots.len() != continuities.len() {
            return Err(SolidError::sm_err(OP_SUBDIV));
        }

        // init iter params
        let mut start = *knots
            .first()
            .ok_or_else(|| invalid(OP_SUBDIV, "no knots"))?;

        // for every unique interval (every unique knot but the first)
        for (ii, (&current, &cont)) in knots.iter().zip(&continuities).enumerate().skip(1) {
            // for internal discontinuites or the last segment
            if ii == knots.len() - 1 || (cont as i32) <= (continuity_to_split as i32) {
                // copy the curve
                let mut copy = self.clone();

                // trim it to the interval between discontinuities (may be the whole
                // curve; may snap the interval to existing knots)
                let mut trim = Extent1d::from_min_max(start, current);
                ignored(copy.trim(k, &mut trim, TrimMode::Notify))?;

                resulting_curves.push(copy);

                // update iter param
                start = current;
            }
        }
        Ok(resulting_curves)
    }

    /// Closes a gap below `d_min` between the closest ends of this curve and `other`
    /// (`TrimGap`): both curves are extended (G1, by 3% of their domain) past those
    /// ends and intersected, and the end control points are moved per `fix` (to the
    /// intersection, or to one curve's end). `false` when the gap is too large or the
    /// extensions do not intersect; `true` when there was no gap or it was closed.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] when the intersection of the extended curves reaches
    /// an unported unit; `InvalidInput` when an
    /// end point cannot be compared (NaN) or an extension is missing (upstream crashes).
    pub fn trim_gap(
        &mut self,
        k: &Kernel,
        other: &mut Self,
        fix: GapFix,
        d_min: f64,
    ) -> Result<bool> {
        // only close gap if less than dMin
        let d_min_sq = d_min * d_min;

        // curve 0 start and end points
        let mut extent0 = self.natural_interval();
        let t0size = extent0.max() - extent0.min();
        let s0 = point_or_undef(self, extent0.min())?;
        let e0 = point_or_undef(self, extent0.max())?;

        // curve 1 start and end points
        let mut extent1 = other.natural_interval();
        let t1size = extent1.max() - extent1.min();
        let s1 = point_or_undef(other, extent1.min())?;
        let e1 = point_or_undef(other, extent1.max())?;

        // Extend curves based on closest endpoints by domain * 0.3
        let d_s0s1 = (s0 - s1).length_squared();
        let d_s0e1 = (s0 - e1).length_squared();
        let d_e0s1 = (e0 - s1).length_squared();
        let d_e0e1 = (e0 - e1).length_squared();
        let dmin = min2(min2(d_s0s1, d_s0e1), min2(d_e0s1, d_e0e1));

        // Test if Gap is greater than dMin
        if dmin > d_min_sq {
            return Ok(false); // gap is too large to fix
        }
        if dmin < EFF_ZERO {
            return Ok(true); // curves have no gap
        }

        // figure out which end points to move
        let ext_case = if dmin >= d_s0s1 {
            1
        } else if dmin >= d_s0e1 {
            2
        } else if dmin >= d_e0s1 {
            3
        } else if dmin >= d_e0e1 {
            4
        } else {
            0
        };
        let start0 = ext_case == 1 || ext_case == 2;
        let end0 = ext_case == 3 || ext_case == 4;
        let start1 = ext_case == 1 || ext_case == 3;
        let end1 = ext_case == 2 || ext_case == 4;

        // extend the input curves past the ends with the smallest gaps
        let extend = |c: &Self, t: f64| -> Result<Option<Self>> {
            Ok(ignored(c.create_extended_curve(t, ContinuityType::G1, true))?.flatten())
        };
        let mut curve0e = None;
        let mut curve1e = None;
        if start0 {
            curve0e = extend(self, extent0.min() - 0.03 * t0size)?;
        }
        if end0 {
            curve0e = extend(self, extent0.max() + 0.03 * t0size)?;
        }
        if start1 {
            curve1e = extend(other, extent1.min() - 0.03 * t1size)?;
        }
        if end1 {
            curve1e = extend(other, extent1.max() + 0.03 * t1size)?;
        }
        let (Some(mut curve0e), Some(mut curve1e)) = (curve0e, curve1e) else {
            return Err(invalid(OP_GAP, "curve extension missing"));
        };

        // intersect the extended curves
        let solutions = global_curve_intersect(
            k,
            &curve0e,
            &curve0e.natural_interval(),
            &curve1e,
            &curve1e.natural_interval(),
            dmin * 5.0,
        )?;

        // failure - the extended curves don't intersect
        if solutions.is_empty() {
            return Ok(false);
        }

        // for every intersection
        for (solution_type, start) in &solutions {
            if *solution_type == SolutionType::SingleValue {
                let t0 = start.param(0)?; // 1st curve intersection param
                let t1 = start.param(1)?; // 2nd curve intersection param
                extent0 = curve0e.natural_interval();
                extent1 = curve1e.natural_interval();

                // Trim the domain of the two curves to the intersection
                let tmin0 = if start0 { t0 } else { extent0.min() };
                let tmax0 = if end0 { t0 } else { extent0.max() };
                let tmin1 = if start1 { t1 } else { extent1.min() };
                let tmax1 = if end1 { t1 } else { extent1.max() };

                let mut ext0 = Extent1d::from_min_max(tmin0, tmax0);
                let mut ext1 = Extent1d::from_min_max(tmin1, tmax1);
                ignored(curve0e.trim(k, &mut ext0, TrimMode::Notify))?;
                ignored(curve1e.trim(k, &mut ext1, TrimMode::Notify))?;

                // only do this once
                break;
            }
        }

        // Now lets average the two trimmed endpoints
        extent0 = curve0e.natural_interval(); // after trimming
        extent1 = curve1e.natural_interval();

        let mut p0 = Vec3::UNDEF;
        let mut p1 = Vec3::UNDEF;
        if start0 {
            p0 = point_or_undef(&curve0e, extent0.min())?;
        }
        if end0 {
            p0 = point_or_undef(&curve0e, extent0.max())?;
        }
        if start1 {
            p1 = point_or_undef(&curve1e, extent1.min())?;
        }
        if end1 {
            p1 = point_or_undef(&curve1e, extent1.max())?;
        }

        // the new common end point
        let mid = match fix {
            GapFix::ToThis => {
                if start0 {
                    s0
                } else {
                    e0
                }
            }
            GapFix::ToOther => {
                if start1 {
                    s1
                } else {
                    e1
                }
            }
            GapFix::Both => 0.5 * (p0 + p1),
        };

        // replace the endpoint of the original curves
        let form = ControlPointForm::NonRational;
        if start0 {
            ignored(self.set_control_point(form, 0, mid, 1.0))?;
        }
        if end0 {
            let last = self.number_control_points()?.wrapping_sub(1);
            ignored(self.set_control_point(form, last, mid, 1.0))?;
        }
        if start1 {
            ignored(other.set_control_point(form, 0, mid, 1.0))?;
        }
        if end1 {
            let last = other.number_control_points()?.wrapping_sub(1);
            ignored(other.set_control_point(form, last, mid, 1.0))?;
        }

        Ok(true) // success...we have modified the curves
    }

    /// The validity checks of the curve (`AssertValid`), reporting failures for `key`
    /// into `list`: the base curve checks (unless `level_given`), a NURB (test 0), end
    /// knot multiplicities `p + 1` (1), interior ones at most `p` (2), consistent knot
    /// and control point counts (16), coincident end control points (3, 4), and for a
    /// curve neither degenerate nor analytic: zero derivatives (5, 6) and kinks (7, 8)
    /// at interior knots of multiplicity `p`, vanishing first derivatives (11, 12),
    /// tiny radii of curvature (13, 14); then coincident interior control points (15).
    /// `true` when every check passes.
    ///
    /// # Errors
    ///
    /// Past the knot checks those of `SmCurve::GlobalPropertyAnalysis`. `k` gives the system zone tolerance of
    /// `SmTol::IsDegenerate(this)`.
    pub fn assert_valid(
        &self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
    ) -> Result<bool> {
        let is_degenerate = |tol: f64| self.is_degenerate(k, tol, None);
        self.assert_valid_with(
            k,
            list,
            level_given,
            key,
            &DerivedOverrides {
                degree: None,
                analytic: curve_is_analytic(self),
                bounded: curve_is_bounded(self),
                is_degenerate: &is_degenerate,
                trim_surface: None,
            },
        )
    }

    /// [`BSplineCurve::assert_valid`] of a UV trim on `trim_surface`, its face's
    /// surface: with `Kernel::fixes.uv_trim_checks_on_surface` (WR-560) the radius of
    /// curvature (13) and coincident control point (15) tests measure the trim on that
    /// surface; otherwise the same as [`BSplineCurve::assert_valid`].
    ///
    /// # Errors
    ///
    /// As [`BSplineCurve::assert_valid`], and those of the surface's evaluations
    /// ([`SolidError::Unported`] only; a failed evaluation leaves the trim unmapped).
    pub fn assert_valid_on(
        &self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        trim_surface: Option<&Surface>,
    ) -> Result<bool> {
        let is_degenerate = |tol: f64| self.is_degenerate(k, tol, None);
        self.assert_valid_with(
            k,
            list,
            level_given,
            key,
            &DerivedOverrides {
                degree: None,
                analytic: curve_is_analytic(self),
                bounded: curve_is_bounded(self),
                is_degenerate: &is_degenerate,
                trim_surface,
            },
        )
    }

    /// [`BSplineCurve::assert_valid`] run for a class derived from `SmBSplineCurve`
    /// (an ellipse, ...): the virtual methods `AssertValid` calls come from `ovr`.
    /// The caller makes a missing NURB first (`MakeNurb` is virtual).
    ///
    /// # Errors
    ///
    /// As [`BSplineCurve::assert_valid`].
    pub(crate) fn assert_valid_with(
        &self,
        k: &Kernel,
        list: Option<&mut AssertArray>,
        level_given: bool,
        key: ObjectKey,
        ovr: &DerivedOverrides<'_>,
    ) -> Result<bool> {
        let mut list = list;
        let mut rtn = true;

        // call the base class AssertValid
        rtn &= if level_given {
            true
        } else {
            curve_assert_valid(self, list.as_deref_mut(), key)?
        };

        // (MakeNurb cannot build a missing NURB for a plain B-spline)

        /*  0 */
        // check for nonNULL m_pNurb pointer
        rtn &= bool_report(&mut list, key, 0, self.nurb.is_some())?;
        let Some(cur) = self.nurb.as_ref() else {
            return Ok(rtn);
        };

        // Check knot vector
        let (knots, mults) = self.knots(None)?;
        let num_knots = knots.len();
        let mult = |i: usize| {
            mults
                .get(i)
                .copied()
                .ok_or_else(|| invalid(OP_ASSERT, "missing multiplicity"))
        };

        /*  1 */
        // check end knots for expected multiplicity count == Degree + 1
        let deg = match ovr.degree {
            Some(d) => d,
            None => self.degree()?,
        };
        let ends_ok = mult(0)? == deg + 1 && mult(num_knots.wrapping_sub(1))? == deg + 1;
        rtn &= bool_report(&mut list, key, 1, ends_ok)?;
        if !rtn {
            return Ok(rtn);
        }

        /*  2 */
        // check internal knots for multiplicty count limit <= Degree
        for idx in 1..num_knots.saturating_sub(1) {
            let ok = mult(idx)? <= deg;
            rtn &= bool_report(&mut list, key, 2, ok)?;
        }
        if !rtn {
            return Ok(rtn);
        }

        /* 16 */
        // check number of knots consistent with number of ControlPoints and degree
        let counts_ok = cur.m() + 1 == cur.n() + 1 + cur.degree() + 1;
        rtn &= bool_report(&mut list, key, 16, counts_ok)?;
        if !rtn {
            return Ok(rtn);
        }

        // check for coincident end control points when the curve is not degenerate
        let degenerate = tol_is_degenerate(k, ovr.is_degenerate)?;
        let pw = cur.cpts();
        let cpt = |i: usize| {
            pw.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_ASSERT, "control point index out of range"))
        };
        if !degenerate {
            // for now - allow endPair ControlPoint coincidence, but output a warning

            // check for start ControlPoint coincidence
            let euclid = cpt(0)?.to_euclid();
            let start_scaled_zero = EFF_ZERO * (1.0 + euclid.max_dimension());
            let start_cpt_dist = dist_cpt_cpt(cpt(0)?, cpt(1)?);

            if is_zero_to_tol(start_cpt_dist, start_scaled_zero) {
                /*  3 */
                let ok = !is_zero_to_tol(start_cpt_dist, start_scaled_zero);
                rtn &= report(
                    &mut list,
                    key,
                    3,
                    ok,
                    start_scaled_zero,
                    start_cpt_dist,
                    |_| {},
                )?;
            }

            // check for end ControlPoint coincidence
            let i_n = cur.n();
            let euclid = cpt(i_n)?.to_euclid();
            let end_scaled_zero = EFF_ZERO * (1.0 + euclid.max_dimension());
            let end_cpt_dist = dist_cpt_cpt(cpt(i_n.wrapping_sub(1))?, cpt(i_n)?);
            if is_zero_to_tol(end_cpt_dist, end_scaled_zero) {
                // (upstream tests and reports the start pair's values here)
                let ok = !is_zero_to_tol(start_cpt_dist, start_scaled_zero);
                rtn &= report(
                    &mut list,
                    key,
                    4,
                    ok,
                    start_scaled_zero,
                    start_cpt_dist,
                    |_| {},
                )?;
            }
        }

        // Check for kinks at internal knot multiplicities; skip kink and zero
        // internal 1st derivative checks for degenerate and analytic curves.
        let analytic = ovr.analytic;

        // for debug - data at failing tests
        let mut min_radius = BIG_DOUBLE;
        let dim = self.dim;
        let undef2 = || vec![Vec3::UNDEF; 2];
        if !degenerate && !analytic {
            // for non degenerate curves
            let mut idx = 1usize;
            while idx < num_knots.saturating_sub(1) && rtn {
                let i = idx;
                idx += 1;

                // skip knots with low mults that guarantee C1 or better continuity
                if mult(i)? < deg {
                    continue;
                }

                // Lower interval evaluation (status ignored)
                let param = knots.get(i).copied().unwrap_or(UNDEF_DOUBLE);
                let v1 = ignored(self.evaluate(param, 1, false, false))?.unwrap_or_else(undef2);
                let v1_dir = v1.get(1).copied().unwrap_or(Vec3::UNDEF);
                let unit1 = v1_dir.unitize();
                let Ok(u1) = unit1 else {
                    let test = if dim == 2 {
                        Some(5)
                    } else if dim == 3 {
                        Some(6)
                    } else {
                        None
                    };
                    if let Some(t) = test {
                        rtn &= report(&mut list, key, t, false, EFF_ZERO, UNDEF_DOUBLE, |r| {
                            r.set_owner_param(Point2::new(param, 0.0));
                            r.set_value(v1_dir.length());
                        })?;
                    }
                    // skip further checks - only need to report the 1st problem
                    continue;
                };

                // upper interval eval
                let v2 = ignored(self.evaluate(param, 1, true, false))?.unwrap_or_else(undef2);
                let v2_dir = v2.get(1).copied().unwrap_or(Vec3::UNDEF);
                let Ok(u2) = v2_dir.unitize() else {
                    let test = if dim == 2 {
                        Some(5)
                    } else if dim == 3 {
                        Some(6)
                    } else {
                        None
                    };
                    if let Some(t) = test {
                        report(&mut list, key, t, false, EFF_ZERO, UNDEF_DOUBLE, |r| {
                            r.set_owner_param(Point2::new(param, 0.0));
                            r.set_value(v2_dir.length());
                        })?;
                    }
                    rtn = false;
                    // skip further checks - only need to report the 1st problem
                    continue;
                };

                // Check the angle between the unitized 1st derivatives
                let dot = u1.dot(u2);

                // when angle between lower and upper ivl 1st Derivs is too big - not G1
                if dot < COSINE_TOL {
                    let max_sin_ang_rad_param = param;
                    let test = if dim == 2 {
                        Some(7)
                    } else if dim == 3 {
                        Some(8)
                    } else {
                        None
                    };
                    if let Some(t) = test {
                        let ok = dot > COSINE_TOL;
                        report(&mut list, key, t, ok, COSINE_TOL, dot, |r| {
                            r.set_owner_param(Point2::new(max_sin_ang_rad_param, 0.0));
                            r.set_value(dot);
                        })?;
                    }
                    rtn = false;
                    // (upstream continues: only the 1st problem is reported)
                }
            }

            // Check for nonregular curve: vanishing first derivative. Allow zero 1st
            // derivatives at the NaturalParameter ends of the curve.

            // Get a representive size (status ignored)
            let domain = self.natural_interval();
            let bbox = ignored(self.calculate_bounding_box(
                &domain,
                BoundingBoxRequest {
                    normal: true,
                    pseudo: false,
                    polar: false,
                },
            ))?
            .and_then(|b| b.normal)
            .unwrap_or_default();
            let size_xyz = bbox.size().length();
            let size_t = domain.length();

            // finite curves: proportional to the average curve speed; else absolute
            let bounded = ovr.bounded;
            let tol = if bounded {
                (size_xyz / size_t) / 10000.0
            } else {
                1e-03
            };
            let min_sep = if bounded {
                size_xyz / max2(20., size_t)
            } else {
                5.0
            };

            let solutions = global_property_analysis(
                k,
                self,
                &domain,
                CurveProperty::MinimizeFirstDerivative,
                min_sep,
            )?;

            // The solution value is not necessarily the length of the first
            // derivative, so evaluate it at the solution parameter.
            for start in &solutions {
                // solution parameter value
                let param = start.param(0)?;

                // skip solutions on the natural boundary ends - they are not a problem
                if domain
                    .is_value_on_boundary(param, EFF_ZERO * (1.0 + domain.length()))
                    .0
                {
                    continue;
                }

                // 1stDeriv magnitude
                let pt_der = ignored(self.evaluate(param, 1, false, false))?.unwrap_or_else(undef2);
                let deriv_length = pt_der.get(1).copied().unwrap_or(Vec3::UNDEF).length();

                // check 1stDeriv > tolerance
                if deriv_length < tol {
                    let min_1st_deriv_param = param;
                    let test = if dim == 2 {
                        Some(11)
                    } else if dim == 3 {
                        Some(12)
                    } else {
                        None
                    };
                    if let Some(t) = test {
                        let ok = deriv_length >= tol;
                        rtn &= report(&mut list, key, t, ok, tol, deriv_length, |r| {
                            r.set_owner_param(Point2::new(min_1st_deriv_param, 0.0));
                            r.set_value(deriv_length);
                        })?;
                    }
                    // only need to report one problem
                    break;
                }
            }

            // Not upstream (fork source fork b4f8c57, WR-560, opt-in): a radius of
            // curvature is a length, and a UV trim's lengths are those of its face's
            // surface. The trim is tested mapped by the surface's first fundamental form
            // at its middle, with the mapped trim's tolerance and solution spacing; a
            // mapped trim that cannot be sized fails test 13 and is not analysed.
            let mut mapped: Option<BSplineCurve> = None;
            let (mut radius_sized, mut radius_tol, mut radius_sep) = (true, tol, min_sep);
            if k.fixes.uv_trim_checks_on_surface {
                if let Some(l) = trim_metric(k, self, bounded, ovr.trim_surface, &domain)? {
                    mapped = mapped_trim(self, l);
                    let mapped_box = match mapped.as_ref() {
                        Some(m) => ignored(m.calculate_bounding_box(
                            &domain,
                            BoundingBoxRequest {
                                normal: true,
                                pseudo: false,
                                polar: false,
                            },
                        ))?
                        .and_then(|b| b.normal),
                        None => None,
                    };
                    radius_sized = mapped_box.is_some();
                    rtn &= bool_report(&mut list, key, 13, radius_sized)?;
                    if let Some(mapped_box) = mapped_box {
                        let mapped_size = mapped_box.size().length();
                        radius_tol = (mapped_size / size_t) / 10000.0;
                        radius_sep = mapped_size / max2(20., size_t);
                    }
                }
            }
            let copy_curve = mapped.as_ref().unwrap_or(self);

            // Another type of kink check: vanishing radius of curvature.
            let solutions = if radius_sized {
                global_property_analysis(
                    k,
                    copy_curve,
                    &domain,
                    CurveProperty::MinimizeRadiusOfCurvature,
                    radius_sep,
                )?
            } else {
                Vec::new()
            };

            // (upstream compares before setting the minimum radius, so the test never
            // fires; the opt-in `Kernel::fixes.bspline_check_min_radius` reads it first:
            // WR-382, source issue #475)
            let first_radius = solutions.first().map_or(BIG_DOUBLE, |s| s.solution_value);
            if k.fixes.bspline_check_min_radius {
                min_radius = first_radius;
            }
            if min_radius < radius_tol {
                min_radius = first_radius;
                let min_radius_param = match solutions.first() {
                    Some(s) => s.param(0)?,
                    None => 0.0,
                };
                let test = if dim == 2 {
                    Some(13)
                } else if dim == 3 {
                    Some(14)
                } else {
                    None
                };
                if let Some(t) = test {
                    let ok = min_radius >= radius_tol;
                    report(&mut list, key, t, ok, radius_tol, min_radius, |r| {
                        r.set_owner_param(Point2::new(min_radius_param, 0.0));
                    })?;
                }
                rtn = false;
            }
        }

        /* 15 */
        // Are Curve Internal ControlPoints Coincident
        let pole_tol = ZONE_TOL_3D / 10.0;
        // Not upstream (fork source fork 54ca3e0, WR-560, opt-in): the tolerance is
        // a length, and a UV trim's control points are measured as lengths on its face's
        // surface, as the radius test above.
        let mut pole_curve: Option<BSplineCurve> = None;
        if k.fixes.uv_trim_checks_on_surface {
            let interval = self.natural_interval();
            if let Some(l) = trim_metric(k, self, ovr.bounded, ovr.trim_surface, &interval)? {
                pole_curve = mapped_trim(self, l);
            }
        }
        let has_internal_pole = pole_curve
            .as_ref()
            .unwrap_or(self)
            .has_internal_pole(k, pole_tol)?;
        rtn &= report(
            &mut list,
            key,
            15,
            !has_internal_pole,
            pole_tol,
            UNDEF_DOUBLE,
            |_| {},
        )?;

        Ok(rtn)
    }

    /// Whether the control polygon of this 2D (UV trim) curve stays clear of a jump
    /// across a periodic boundary of the surface domain `surface_uv_domain`
    /// (`AssertNoPeriodicJump`): `false` when two consecutive control points differ by
    /// at least nine tenths of the domain length in a direction the surface is closed
    /// in (`surface_closed_u` / `surface_closed_v`). A curve that is not 2D, a surface
    /// closed in neither direction and a two-point polygon are `true`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB (upstream makes one or dereferences `NULL`; a plain
    /// B-spline cannot make one); those of [`BSplineCurve::polygon`].
    pub fn assert_no_periodic_jump(
        &self,
        surface_closed_u: bool,
        surface_closed_v: bool,
        surface_uv_domain: &Extent2d,
    ) -> Result<bool> {
        // return value
        let mut rtn = true;

        // no work - not 2d, not periodic
        if self.dim() != 2 || (!surface_closed_u && !surface_closed_v) {
            return Ok(true);
        }

        // get curve control points in 2d euclidean space (the NURB is made by the
        // caller, `assert_no_periodic_jump_of`, for the kinds that can make one)
        let ivl = self.natural_interval();
        let polygon = self.polygon(&ivl)?;

        // no work - single pair control point curves are assumed to not jump boundaries
        if polygon.len() == 2 {
            return Ok(true);
        }

        // get slightly shortened surface periods
        let period_u = 0.9 * surface_uv_domain.x_length();
        let period_v = 0.9 * surface_uv_domain.y_length();

        // for every pair of control Points
        for pair in polygon.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            let jump_u = (b.x - a.x).abs();
            let jump_v = (b.y - a.y).abs();

            // look for a potential bad U Jump
            if surface_closed_u && jump_u >= period_u {
                rtn = false;
            }
            if surface_closed_v && jump_v >= period_v {
                rtn = false;
            }
        }

        // all done
        Ok(rtn)
    }
}

/// `pUVTrimCurve->AssertNoPeriodicJump(bClosedU, bClosedV, rSurfaceUVDomain)` for a
/// curve of any B-spline kind: [`BSplineCurve::assert_no_periodic_jump`] of its
/// `SmBSplineCurve` part, a line's NURB made on a copy first (`GetPolygon`'s
/// `GetOrCreateGwNurbPointer`; upstream makes it on the curve itself).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a curve that is not an `SmBSplineCurve` (upstream's
/// UV trim curves always are); those of [`BSplineCurve::assert_no_periodic_jump`].
pub fn assert_no_periodic_jump_of(
    curve: &Curve,
    surface_closed_u: bool,
    surface_closed_v: bool,
    surface_uv_domain: &Extent2d,
) -> Result<bool> {
    use crate::curve::curve::p2::CurveKind;
    let base = match curve.kind() {
        CurveKind::BSpline(c) => std::borrow::Cow::Borrowed(c),
        CurveKind::Line(c) => c.base_with_nurb()?,
        CurveKind::Ellipse(c) => std::borrow::Cow::Borrowed(c.base()),
        CurveKind::Parabola(c) => std::borrow::Cow::Borrowed(c.base()),
        CurveKind::Hyperbola(c) => std::borrow::Cow::Borrowed(c.base()),
        _ => {
            return Err(SolidError::invalid_input(
                "nurbs::b_spline_curve::p3::assert_no_periodic_jump",
                "the curve is not an SmBSplineCurve",
            ))
        }
    };
    base.assert_no_periodic_jump(surface_closed_u, surface_closed_v, surface_uv_domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;

    /// A non-rational 3D curve of degree `p` through `pts` with `knots`.
    fn curve(p: usize, pts: &[[f64; 3]], knots: &[f64]) -> BSplineCurve {
        let n = pts.len() - 1;
        let mut cur = allocate_nurb_curve(n, p, knots.len() - 1).unwrap();
        for (d, s) in cur.cpts_mut().iter_mut().zip(pts) {
            *d = CPoint::new(s[0], s[1], s[2], NOW);
        }
        cur.knots_mut().copy_from_slice(knots);
        BSplineCurve::from_nurb(cur, 3, false)
    }

    fn quad() -> BSplineCurve {
        curve(
            2,
            &[
                [0.0, 0.0, 0.0],
                [1.0, 2.0, 0.0],
                [3.0, 2.0, 0.0],
                [4.0, 0.0, 0.0],
            ],
            &[0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
        )
    }

    #[test]
    fn control_point_forms() {
        let c = quad();
        let (p, w) = c.control_point(ControlPointForm::NonRational, 1).unwrap();
        assert_eq!(p, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(w, 1.0);
        let (p, w) = c
            .control_point(ControlPointForm::EuclideanRational, 3)
            .unwrap();
        assert_eq!(p, Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(w, 1.0);
        assert!(c.control_point(ControlPointForm::NonRational, 9).is_err());
    }

    #[test]
    fn scale_trans_rot_moves_ends() {
        let c = quad();
        let r = c
            .create_by_scale_trans_rot(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0), 0.0)
            .unwrap();
        let (p, _) = r.control_point(ControlPointForm::NonRational, 0).unwrap();
        assert_eq!(p, Vec3::new(0.0, 0.0, 1.0));
        let (p, _) = r.control_point(ControlPointForm::NonRational, 3).unwrap();
        assert_eq!(p, Vec3::new(4.0, 0.0, 1.0));
    }

    #[test]
    fn on_plane_and_off_plane() {
        let c = quad();
        let z = Vec3::new(0.0, 0.0, 1.0);
        let on = c
            .is_on_plane(Vec3::new(0.0, 0.0, 5.0), z, 1e-6, None)
            .unwrap();
        assert!(!on.on_plane);
        assert_eq!(on.max_dist, None);
        // on the plane the SmLine test is reached (false for a plain B-spline)
        let on = c.is_on_plane(Vec3::default(), z, 1e-6, None).unwrap();
        assert!(on.on_plane);
    }

    #[test]
    fn trim_gap_early_exits() {
        let k = Kernel::default();
        let mut a = curve(
            1,
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        let mut b = curve(
            1,
            &[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        assert!(a.trim_gap(&k, &mut b, GapFix::Both, 0.1).unwrap());
        let mut c = curve(
            1,
            &[[3.0, 0.0, 0.0], [4.0, 1.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        assert!(!a.trim_gap(&k, &mut c, GapFix::Both, 0.1).unwrap());
    }

    #[test]
    fn join_with_gap_too_large() {
        let k = &Kernel::default();
        let mut a = quad();
        let mut b = curve(
            1,
            &[[9.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        assert!(a.join_with(k, true, &mut b, false, Some(0.01)).is_err());
    }

    #[test]
    fn join_with_curves() {
        let k = &Kernel::default();
        // two lines at an angle: joined by CreateByJoining
        let mut a = curve(
            1,
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        let mut b = curve(
            1,
            &[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        a.join_with(k, true, &mut b, false, Some(0.01)).unwrap();
        let e = a.ends().unwrap();
        assert_eq!(e.start, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(e.end, Vec3::new(1.0, 1.0, 0.0));
        // collinear lines reach the SmLine test (false for plain B-splines)
        let mut c = curve(
            1,
            &[[1.0, 1.0, 0.0], [1.0, 2.0, 0.0]],
            &[0.0, 0.0, 1.0, 1.0],
        );
        let r = b.join_with(k, true, &mut c, false, Some(0.01));
        assert!(!matches!(
            r,
            Err(SolidError::Unported {
                unit: "curve.curve_p2",
                ..
            })
        ));
    }

    #[test]
    fn assert_valid_reports_bad_knots() {
        let k = Kernel::default();
        let mut cur = allocate_nurb_curve(2, 1, 4).unwrap();
        for (i, d) in cur.cpts_mut().iter_mut().enumerate() {
            *d = CPoint::new(i as f64, 0.0, 0.0, NOW);
        }
        // start multiplicity 1 (not p + 1 = 2)
        cur.knots_mut().copy_from_slice(&[0.0, 0.5, 0.7, 1.0, 1.0]);
        let c = BSplineCurve::from_nurb(cur, 3, true);
        let mut list = AssertArray::new();
        let ok = c
            .assert_valid(&k, Some(&mut list), true, ObjectKey(7))
            .unwrap();
        assert!(!ok);
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).map(|r| r.test_index), Some(1));
        // no NURB: test 0
        let e = BSplineCurve::new(3);
        let mut list = AssertArray::new();
        assert!(!e
            .assert_valid(&k, Some(&mut list), true, ObjectKey(1))
            .unwrap());
        assert_eq!(list.get(0).map(|r| r.test_index), Some(0));
        // the base checks pass (no owner); test 0 still fails
        assert!(!e.assert_valid(&k, None, false, ObjectKey(1)).unwrap());
    }

    /// source issue #475 (WR-382): tests 13/14 (an extremely small radius of
    /// curvature) compare the minimum radius before it is read, so they never fire; the
    /// opt-in reads it first. A quadratic with a sharp bend: radius 1/h = 1e-3 at its
    /// middle against a tolerance of about h / 20000 = 0.05.
    #[test]
    fn issue_475_min_radius_of_curvature_test() {
        let mut k = Kernel::default();
        let h = 1000.0;
        let c = curve(
            2,
            &[[-1.0, 0.0, 0.0], [0.0, h, 0.0], [1.0, 0.0, 0.0]],
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        );
        let tests = |k: &Kernel| {
            let mut list = AssertArray::new();
            let ok = c
                .assert_valid(k, Some(&mut list), true, ObjectKey(3))
                .unwrap();
            let idx: Vec<_> = (0..list.len())
                .filter_map(|i| list.get(i).map(|r| r.test_index))
                .collect();
            (ok, idx)
        };
        let (_, idx) = tests(&k);
        assert!(!idx.contains(&14), "{idx:?}");
        k.fixes.bspline_check_min_radius = true;
        let (ok, idx) = tests(&k);
        assert!(!ok && idx.contains(&14), "{idx:?}");
    }

    #[test]
    fn subdivide_single_span() {
        let k = Kernel::default();
        let c = curve(
            2,
            &[[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 2.0, 0.0]],
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        );
        let parts = c
            .subdivide_at_discontinuity(&k, ContinuityType::C0, 1.0)
            .unwrap();
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn segment_line_sample_count_is_bounded() {
        // fuzz regression (nurbs.b_spline_curve_p3 fuzz00318): i64::MAX samples on a
        // straight curve ran for hours
        let c = curve(
            2,
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
            ],
            &[0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
        );
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        assert!(c.is_segment_line(&ivl, 10, 1e-6).unwrap().is_line);
        assert!(c.is_segment_line(&ivl, u64::MAX, 1e-6).is_err());
        assert!(c
            .is_segment_line(&ivl, MAX_SEGMENT_SAMPLES + 1, 1e-6)
            .is_err());
    }

    /// The exact rational trim of a circle of radius `r` about `(cx, cy)` on a plane
    /// spanning `sx` x `sy` over `[0,1]^2`, and that plane: in (u, v) an ellipse
    /// `sy / sx` times as wide as it is long.
    fn circle_trim_on_plane(
        (cx, cy, r): (f64, f64, f64),
        (sx, sy): (f64, f64),
    ) -> (BSplineCurve, Surface) {
        use crate::nurbs::b_spline_curve::p1::{BSplineCurveForm, CanonicalData, KnotType};
        use crate::nurbs::b_spline_surface::BSplineSurface;
        let s = 2.0_f64.sqrt();
        let control: Vec<Vec3> = (0..9_i32)
            .map(|i| {
                let t = f64::from(i) * PI / 4.0;
                let scale = if i % 2 == 1 { s } else { 1.0 };
                Vec3::new(
                    (cx + scale * r * t.cos()) / sx,
                    (cy + scale * r * t.sin()) / sy,
                    0.0,
                )
            })
            .collect();
        let w = 0.5_f64.sqrt();
        let trim = BSplineCurve::create_canonical(&CanonicalData {
            dim: 2,
            degree: 2,
            control_points: &control,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &[3, 2, 2, 2, 3],
            knots: &[0.0, 0.25, 0.5, 0.75, 1.0],
            knot_type: KnotType::Unspecified,
            weights: Some(&[1.0, w, 1.0, w, 1.0, w, 1.0, w, 1.0]),
            trim: None,
        })
        .unwrap();
        let plane = Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(sx, 0.0, 0.0),
                Vec3::new(0.0, sy, 0.0),
                Vec3::new(sx, sy, 0.0),
            )
            .unwrap(),
        );
        (trim, plane)
    }

    /// The failed test indices of `trim`'s `AssertValid` on `surface`.
    fn failed_tests(k: &Kernel, trim: &BSplineCurve, surface: Option<&Surface>) -> Vec<u64> {
        let mut list = AssertArray::new();
        trim.assert_valid_on(k, Some(&mut list), true, ObjectKey(5), surface)
            .unwrap();
        (0..list.len())
            .filter_map(|i| list.get(i).map(|r| r.test_index))
            .collect()
    }

    /// WR-560 (fork source fork b4f8c57, #891 item 2): the exact trim of a r 1
    /// circle on a plane spanning 600 x 3 is an ellipse 200 times as long as it is wide
    /// in (u, v), of radius 8.3e-6 at its ends against a tolerance of about 6.7e-5; on
    /// the plane it is the circle of radius 1. Test 13 needs `bspline_check_min_radius`
    /// to fire at all (WR-382).
    #[test]
    fn issue_891_uv_trim_radius_is_measured_on_its_surface() {
        let mut k = Kernel::default();
        k.fixes.bspline_check_min_radius = true;
        let (trim, plane) = circle_trim_on_plane((300.0, 1.5, 1.0), (600.0, 3.0));
        let idx = failed_tests(&k, &trim, Some(&plane));
        assert!(idx.contains(&13), "{idx:?}");
        k.fixes.uv_trim_checks_on_surface = true;
        let idx = failed_tests(&k, &trim, Some(&plane));
        assert!(!idx.contains(&13), "{idx:?}");
        // a curve that is no face's trim is tested as it is
        let idx = failed_tests(&k, &trim, None);
        assert!(idx.contains(&13), "{idx:?}");
    }

    /// WR-560 (fork source fork 54ca3e0, #1524): the trim of a r 0.003 hole on a
    /// plane spanning 4500 x 4500 has control points 6.7e-7 apart in (u, v), under the
    /// pole tolerance `SM_ZONE_TOL_3D / 10`; on the plane they are 0.003 apart.
    #[test]
    fn issue_1524_uv_trim_control_points_are_measured_on_its_surface() {
        let mut k = Kernel::default();
        let (trim, plane) = circle_trim_on_plane((2250.0, 2250.0, 0.003), (4500.0, 4500.0));
        let idx = failed_tests(&k, &trim, Some(&plane));
        assert!(idx.contains(&15), "{idx:?}");
        k.fixes.uv_trim_checks_on_surface = true;
        let idx = failed_tests(&k, &trim, Some(&plane));
        assert!(!idx.contains(&15), "{idx:?}");
    }
}
