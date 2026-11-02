//! `SmBSplineSurface` with NLib, part 2: corner blends, skinned surfaces, trimming to a
//! domain and evaluation with zero-derivative corrections.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurfaceNLib.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//!
//! | Rust                                          | Upstream                                  |
//! |-----------------------------------------------|-------------------------------------------|
//! | [`BSplineSurface::create_corner_blend`], [`OrientType`], [`BlendNeighbour`] | `CreateCornerBlend`, `SmOrientType`, its `SmSurface*` |
//! | [`BSplineSurface::create_skinned_surface`], [`SkinnedSurfaceOptions`] | `CreateSkinnedSurface` |
//! | [`BSplineSurface::trim_with_domain`], [`BSplineSurface::trim_with_domain_with`], [`TrimOptions`] | `TrimWithDomain` |
//! | [`BSplineSurface::evaluate`], [`EvaluateFlags`] | `Evaluate`                              |
//!
//! The other functions of the file belong to `nurbs.b_spline_surface_n_lib_p1` and
//! later units.
//!
//! Deviations:
//! - **Memory and contexts.** `SmNLibStackHandler`, `NL_STACKS` and `SmContext`
//!   arguments are dropped; new surfaces are return values (upstream out-params).
//!   `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT)` in `TrimWithDomain` is omitted (a
//!   surface here has no owner to notify).
//! - **Curve and surface types.** `CreateCornerBlend` takes [`BSplineCurve`] 3D curves
//!   and [`BlendNeighbour`] neighbour surfaces (upstream `SmCurve*` / `SmSurface*`;
//!   `sm_CreateNonRationalCurve` fails with `NER` for any other curve type). A
//!   neighbour is the caller's surface of any kind, evaluated through the virtual
//!   `SmSurface` methods (`DropCurve`, `EvaluateNormal`, `EvaluateNormalSection`,
//!   `Evaluate`, `GetNaturalUVDomain`). `NULL` UV curves and surfaces are `None`. Where upstream indexes past the end of the orientation, UV curve or
//!   surface arrays, or dereferences a `NULL` UV curve or surface (n-sided blends,
//!   bilinear blends with more than four curves), the port returns `InvalidInput`.
//! - **`CreateSkinnedSurface`**'s many arguments are the fields of
//!   [`SkinnedSurfaceOptions`]. Upstream casts away the constness of the rails and
//!   reparametrises them when both are given (`SyncronizeKnotsOfCurves`); the rails
//!   are `&mut` here and are changed the same way. No cross sections is `InvalidInput`
//!   (upstream calls NLib with a highest index of `-1`). The NLib skinning routines
//!   change the cross sections, derivative surfaces and rails of the retries in place,
//!   as upstream.
//! - **`Evaluate`** returns the derivative matrix `D[i][j]` (`i <= u`, `j <= v`
//!   derivatives) instead of writing the caller's array; entries upstream leaves
//!   untouched (the lower half with `bOnlyUpperHalf`) are zero. The unwritten lower
//!   half is 0 here and `SM_UNDEF` upstream (a fresh caller's `SmVector3d` array);
//!   unreachable today (no caller reads it). Its flags are an
//!   [`EvaluateFlags`] value; the internal `bDoZeroSampling` argument is private.
//!   `SmSurfaceEval` retains the matrix written before a zero-tangent correction
//!   failure, because upstream accessors ignore that failure and read the output.
//! - **Tangent fields** are [`TangentField`]s of `SmCurve` views of the B-spline
//!   curves (the same data, not a copy-constructed curve).
//! - **Tangent-field approximation.** `SmTangentField::ApproximateCurve` is the
//!   inherited `SmCrvOnSurf::ApproximateCurve`, which on the new field's empty cache
//!   runs `SmCurve::ApproximateCurve` with the field as the virtual curve.
//!   [`crate::curve::crv_on_surf::CrvOnSurf`] and `Curve::approximate_curve_base` take
//!   a [`crate::curve::curve::p2::Curve`], which has no tangent-field kind, so that
//!   body (and `sm_TestSpanAccuracy`) is repeated here over the field, same order and
//!   bounds (`approximate_tangent_field`). (`SmSurface::EvaluateNormalSection`
//!   is `surface::surface::p3`'s, `SmSurface::EvaluateNormal`
//!   is `surface::surface::p7`'s, `Evaluate1stDerivatives` `surface::surface::p1`'s.) Upstream ignores the status of
//!   most of these calls; an unported one still stops the port.
//! - **Not upstream: `TrimWithDomain` knot snapping** (source issue #472), only
//!   with [`TrimOptions::snap_roundoff_to_knots`] ([`BSplineSurface::trim_with_domain`]
//!   keeps upstream's behaviour). Upstream moves a trim boundary that lies within
//!   `SM_EFF_ZERO_SQRT * (1 + |t|)` *inside* a knot away from it by ten times that, so a
//!   boundary one rounding error inside a knot leaves a sliver span (2.1e-4 on a fillet
//!   round at u = 20) where a boundary on the knot leaves none. With the option a
//!   boundary within `SM_EFF_ZERO_PARAM` of a knot is snapped onto it, and the
//!   requested domain passed to `sm_CheckTrimBoundaries` takes the knot too, so the
//!   `[B306]` widening does not run for that rounding error.
//! - Status values upstream ignores stay ignored (`EditParameterization` before
//!   `MakeCurvesCompatible`, the curve evaluations of the n-sided blend, `Scale`,
//!   `GetKnots`, `N_CrvElevateDegree`, `SetMinMax`).
//! - **Not upstream: a Coons corner patch tangent to its sides** (WR-547, the fork's
//!   source fork@cae8618), only with
//!   `Kernel::fixes.corner_blend_coons_patch_tangent_to_sides`: the four-sided
//!   `CreateCornerBlend` (not bilinear) replaces its Coons patch by
//!   `make_coons_patch_tangent_to_sides` (`sm_MakeCoonsPatchTangentToSides` and its
//!   helpers `sm_CoonsSideNormal`, `sm_CoonsSideAngle`, `sm_RefineToSpans` and
//!   `sm_ProjectCoonsSideRows`, all private). Its refinement is bounded (8 to 64 spans).

use crate::curve::curve::p1::{SpanAccuracy, MAX_APPROX_SPANS};
use crate::curve::curve::p2::Curve as SmCurve;
use crate::curve::curve::p2::{
    add_to_curve, flush_crv, pop_stack, push_point, stack_look, CurveEvaluate, CurveKind,
};
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p4::is_degenerate_base;
use crate::curve::hermite_curve::HermiteCurve;
use crate::curve::tangent_field::{CurveEnd as TangentFieldEnd, TangentDirType, TangentField};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::{Extent2d, SS_UMAX, SS_UMIN, SS_VMIN};
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::ANG_TOL_DEG;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::acos;
use crate::geom::vector3d::Point3;
use crate::geom::vector3d::Vec3;
use crate::math::{
    max2, min2, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::{
    BSplineCurve, BSplineCurveForm, BoundingBoxRequest, CanonicalData, ControlPointForm, KnotType,
};
use crate::nurbs::b_spline_surface::{BSplineSurface, BSplineSurfaceForm, SurfParamType};
use crate::nurbs::crv_basic::Boundary;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::geom::Rectangle;
use crate::nurbs::index::at;
use crate::nurbs::knots::Side;
use crate::nurbs::math::{srf_power_basis_eval_derivs, MAX_DERIVATIVE};
use crate::nurbs::srf_adv::p1::{create_skin_spine, SkinDerivs, SkinSpineOptions, SpineAlignment};
use crate::nurbs::srf_adv::p5::{
    create_skin_srf_boundary_continuity, BoundaryContinuity, SkinBoundaryOptions,
};
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_tool::p1::extract_patch;
use crate::surface::surface::Surface as SmSurface;

use super::p1::{
    create_nlib_curve, create_nlib_surface, create_non_rational_curve_with_context, make_knots,
};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;

/// `SM_ZONE_TOL_3D`.
const ZONE_TOL_3D: f64 = 1.0e-5;

const OP_CORNER: &str = "nurbs::b_spline_surface_n_lib::p2::BSplineSurface::create_corner_blend";
const OP_SKIN: &str = "nurbs::b_spline_surface_n_lib::p2::BSplineSurface::create_skinned_surface";
const OP_TRIM: &str = "nurbs::b_spline_surface_n_lib::p2::BSplineSurface::trim_with_domain";
const OP_EVAL: &str = "nurbs::b_spline_surface_n_lib::p2::BSplineSurface::evaluate";

// ---- types ----------------------------------------------------------------------------

/// Orientation of a boundary curve relative to the curve order (`SmOrientType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OrientType {
    /// `SM_OT_UNKNOWN` (0).
    #[default]
    Unknown,
    /// `SM_OT_SAME` (1): parametrised in the direction of the curve order.
    Same,
    /// `SM_OT_OPPOSITE` (2): parametrised against it.
    Opposite,
}

/// A neighbour surface of [`BSplineSurface::create_corner_blend`] (upstream's
/// `SmSurface*`).
#[derive(Debug, Clone, Copy)]
pub struct BlendNeighbour<'a> {
    /// The surface itself, with its kind and owner: `sm_CreateNonRationalCurve`
    /// drops rational curves onto it through the virtual `SmSurface::DropCurve`, and
    /// the cross-tangent construction evaluates it through the virtual
    /// `SmSurface` evaluators.
    pub surface: &'a SmSurface,
}

/// Options of [`BSplineSurface::trim_with_domain_with`]. `Default` is upstream's
/// `TrimWithDomain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TrimOptions {
    /// Not upstream (source issue #472): a trim boundary at most
    /// `SM_EFF_ZERO_PARAM` inside a knot is snapped onto the knot instead of being
    /// moved `10 * SM_EFF_ZERO_SQRT * (1 + |t|)` away from it, which leaves a sliver
    /// span. Off by default (upstream).
    pub snap_roundoff_to_knots: bool,
}

/// The flags of [`BSplineSurface::evaluate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvaluateFlags {
    /// Span picked at a u knot (`bUFromLeft`: `TRUE` is [`Side::Left`], the span
    /// starting at the knot).
    pub u_side: Side,
    /// Span picked at a v knot (`bVFromLeft`).
    pub v_side: Side,
    /// Compute only `D[i][j]` with `i + j <= highest_u_deriv` (`bOnlyUpperHalf`; the
    /// two orders must then be equal).
    pub only_upper_half: bool,
    /// Replace zero first derivatives by short, properly directed vectors
    /// (`bNonZeroTangents`, upstream default `TRUE`).
    pub non_zero_tangents: bool,
}

impl Default for EvaluateFlags {
    /// From the left in both directions, full matrix, non-zero tangents.
    fn default() -> Self {
        Self {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half: false,
            non_zero_tangents: true,
        }
    }
}

/// The optional inputs of [`BSplineSurface::create_skinned_surface`].
#[derive(Debug)]
pub struct SkinnedSurfaceOptions<'a> {
    /// The cross sections have the same degree and are synchronized (`bSyncronized`).
    pub synchronized: bool,
    /// [`SurfParamType::U`]: the cross sections become u isoparameter curves (NLib
    /// `NL_UDIR`); anything else `NL_VDIR` (`eCrossSectionSurfParam`).
    pub cross_section_surf_param: SurfParamType,
    /// `0` interpolates the cross sections, else approximates within this tolerance
    /// (`dThisApproxTol3d`).
    pub approx_tol: f64,
    /// Rail along the minimum parameter side (`pRail1`).
    pub rail1: Option<&'a mut BSplineCurve>,
    /// Rail along the maximum parameter side (`pRail2`).
    pub rail2: Option<&'a mut BSplineCurve>,
    /// Use `rail1` as spine curve, ignoring `rail2` (`bRail1UsedAsSpineCurve`).
    pub rail1_used_as_spine_curve: bool,
    /// Rail parameters of the cross section / rail intersections
    /// (`pIntersectionsWithRails`): `[0..k]` on `rail1`, `[k..2k]` on `rail2`.
    pub intersections_with_rails: Option<&'a [f64]>,
    /// Cross-boundary derivative surfaces at the start and end of the skin
    /// (`pDerivSurf`; `None` is a `NULL` array).
    pub deriv_surfs: Option<[Option<&'a BSplineSurface>; 2]>,
    /// Degree in the sweep direction (`lDegree`, upstream default 3).
    pub degree: usize,
}

impl Default for SkinnedSurfaceOptions<'_> {
    fn default() -> Self {
        Self {
            synchronized: false,
            cross_section_surf_param: SurfParamType::U,
            approx_tol: 0.0,
            rail1: None,
            rail2: None,
            rail1_used_as_spine_curve: false,
            intersections_with_rails: None,
            deriv_surfs: None,
            degree: 3,
        }
    }
}

// ---- tangent-field callees --------------------------------------------------------------

/// `curve` as an `SmCurve` (the same object upstream: the NURB and flags as they are,
/// without the copy constructor's knot fix-up).
fn as_sm_curve(curve: &BSplineCurve) -> SmCurve {
    SmCurve::from(BSplineCurve {
        dim: curve.dim,
        nurb: curve.nurb.clone(),
        nurb_is_borrowed: curve.nurb_is_borrowed,
        out_of_bounds_enabled: curve.out_of_bounds_enabled,
        knot_type: curve.knot_type,
        form: curve.form,
    })
}

/// The operation path of [`approximate_tangent_field`].
const OP_APPROX_FIELD: &str = "nurbs::b_spline_surface_n_lib::p2::approximate_tangent_field";

/// A tangent field seen as an `SmCurve` (its virtual `Evaluate`, `EvaluatePoint`,
/// `GetNaturalInterval` and `GetDim` as `SmCurve::ApproximateCurve` reaches them).
struct FieldCurve<'f, 'a>(&'f TangentField<'a, SmSurface>);

impl CurveEvaluate for FieldCurve<'_, '_> {
    fn dim(&self) -> u64 {
        // SmCrvOnSurf: always 3D
        3
    }

    fn natural_interval(&self) -> Extent1d {
        self.0.natural_interval()
    }

    fn evaluate(
        &self,
        k: &crate::kernel::Kernel,
        t: f64,
        num_derivs: usize,
        _from_left: bool,
        _non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        self.0.evaluate(k, t, num_derivs)
    }

    fn evaluate_point(&self, k: &crate::kernel::Kernel, t: f64) -> Result<Point3> {
        self.0.evaluate_point(k, t)
    }
}

/// `Evaluate(t, pv, 1, bFromLeft, bNonZeroTangents)` into a position/tangent pair.
fn field_eval2(
    k: &crate::kernel::Kernel,
    curve: &FieldCurve<'_, '_>,
    t: f64,
    from_left: bool,
) -> Result<[Vec3; 2]> {
    let v = curve.evaluate(k, t, 1, from_left, true)?;
    let mut out = [Vec3::UNDEF; 2];
    for (d, s) in out.iter_mut().zip(v) {
        *d = s;
    }
    Ok(out)
}

/// Maximum adaptive Hermite subdivisions in one approximation, as
/// `curve::curve::p1`'s `SmCurve::ApproximateCurve` (see its deviations).
const MAX_APPROXIMATION_SUBDIVISIONS: usize = 8192;

/// `sm_TestSpanAccuracy` on a tangent field (the body of
/// [`crate::curve::curve::p1::test_span_accuracy`], whose curve is the field).
fn field_span_accuracy(
    k: &crate::kernel::Kernel,
    curve: &FieldCurve<'_, '_>,
    last_t: f64,
    last_pv: [Point3; 2],
    curr_t: f64,
    curr_pv: [Point3; 2],
    approx_tol: f64,
    match_parameterization: bool,
) -> Result<SpanAccuracy> {
    // init output
    let mut out = SpanAccuracy {
        passes: false,
        max_dist: 0.0,
    };

    // locals
    let mut force_pass = false;
    let this_span = curr_t - last_t;
    let t_scale = 1.0 + curr_t.abs();
    let ivl = curve.natural_interval();
    let min_step = 1500.0;
    let mut num_test_points: u32 = 3;

    // when span is getting very small - just pass it
    if this_span < EFF_ZERO * t_scale {
        force_pass = true;
    }

    // Don't let the interval get too small
    if this_span < (ivl.length()) / min_step {
        force_pass = true;
    }

    // hermite cuve between interval end points
    let hermite = HermiteCurve::new(
        last_pv[0],
        last_pv[1] * this_span,
        curr_pv[0],
        curr_pv[1] * this_span,
        3,
    );
    let hermite_curve = SmCurve::from(hermite);

    if force_pass {
        // get a better error value if we're going to keep this step.
        num_test_points = 7;
    }

    for ii in 0..num_test_points {
        let normalized_param = (f64::from(ii) + 1.0) / (f64::from(num_test_points) + 1.0);
        let crv_param = last_t + normalized_param * this_span;

        // Evaluate the curve.  Then, if we're matching parameterization,
        // evaluate the Hermite curve at the same (relative) parameter,
        // else find the closest point on the Hermite curve.
        let crv_pnt = curve.evaluate_point(k, crv_param)?;

        let approx_dist = if match_parameterization {
            // Approx dist = DistanceBetween(Curve(param), Hermite(param))
            let herm_pnt = hermite.evaluate_point(normalized_param);
            herm_pnt.distance(crv_pnt)
        } else {
            // Approx dist = DistanceBetween(Curve(param), NearestPointOnHermite)
            let hermite_domain = Extent1d::from_min_max(0.0, 1.0);
            let r = hermite_curve.local_point_solve::<()>(
                k,
                &hermite_domain,
                SolverOperation::Minimize,
                crv_pnt,
                &PointSolveOptions::default(),
                normalized_param,
            );
            // save nearest distance (when drop point failed - load a big distance to
            // quit testing)
            match ignored(r)? {
                Some((true, sol)) => sol.start.solution_value,
                _ => 2.0 * approx_tol,
            }
        };

        // When ApproxDist is too big
        if approx_dist > approx_tol && !force_pass {
            // set output and quit testing
            out.passes = false;
            return Ok(out);
        }
        out.max_dist = max2(out.max_dist, approx_dist);
    }

    // when bForcePass == TRUE
    out.passes = true;
    Ok(out)
}

/// `SmCrvOnSurf::ApproximateCurve(ctx, knots, ang_tol, max_gap, curve, TRUE, FALSE,
/// FALSE)` on a new tangent field (inherited): its `GetOrCreateBSApproxPointer` with
/// the given breaks and `bMatchParam = FALSE` on an empty cache runs
/// `SmCurve::ApproximateCurve(ctx, breaks, tol, gap, approx, TRUE, FALSE, FALSE)` and
/// copies the result. The body is that of
/// [`crate::curve::curve::p1`]'s `Curve::approximate_curve_base` with the field as the
/// virtual curve (a tangent field is no B-spline, so neither the copy shortcut nor
/// the "input curve is better" copy applies).
fn approximate_tangent_field(
    k: &crate::kernel::Kernel,
    field: &TangentField<'_, SmSurface>,
    breaks: &[f64],
    approx_tol_3d: f64,
) -> Result<BSplineCurve> {
    let curve = FieldCurve(field);
    let create_analytics = true;
    let match_parameterization = false;

    // need 2 breakpoints to define the curve interval
    let (Some(&first), Some(&last)) = (breaks.first(), breaks.last()) else {
        return Err(SolidError::sm_err(OP_APPROX_FIELD));
    };
    if breaks.len() < 2 {
        return Err(SolidError::sm_err(OP_APPROX_FIELD));
    }

    // low work - degenerate curves
    if is_degenerate_base(k, &curve, EFF_ZERO, None)? {
        let pnt = curve.evaluate_point(k, first)?;
        let degenerate = BSplineCurve::create_degenerate_curve(curve.dim(), pnt)?;
        let mut c = SmCurve::from(degenerate);
        let ivl = Extent1d::from_min_max(first, last);
        // (status ignored)
        ignored(c.edit_parameterization(k, &ivl))?;
        let CurveKind::BSpline(bs) = c.into_kind() else {
            return Err(SolidError::invalid_input(
                OP_APPROX_FIELD,
                "the approximation is not an SmBSplineCurve",
            ));
        };
        return Ok(bs.copy());
    }

    // locals
    let mut stack: Vec<f64> = Vec::with_capacity(256 * 4 * 2);
    let mut ctrl_poly: Vec<Point3> = Vec::with_capacity(256);
    let mut knots: Vec<f64> = Vec::with_capacity(256);
    let mut subdivisions = 0;

    // for every interval defined by input BreakParams array
    for w in breaks.windows(2) {
        let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else {
            continue;
        };
        let mut last_t = a;
        let mut curr_t = b;

        // interval start position and tangent (Last)
        let mut last_pv = field_eval2(k, &curve, last_t, true)?; // NonZeroTangent values
        {
            // walk to a nonZero Tangent evaluation
            let mut step = 0.0001;
            while last_pv[1].length_squared() < EFF_ZERO_SQ * 100000.0 {
                let tmp = last_pv[0];
                last_pv = field_eval2(k, &curve, last_t + step * (curr_t - last_t), true)?;
                last_pv[0] = tmp;
                step *= 2.0;
                if step > 0.5 {
                    break;
                }
            }
        }

        // interval end position and tangent (Curr)
        let mut curr_pv = field_eval2(k, &curve, curr_t, false)?; // NonZeroTangent values
        {
            let mut step = 0.0001;
            while curr_pv[1].length_squared() < EFF_ZERO_SQ * 100000.0 {
                let tmp = curr_pv[0];
                curr_pv = field_eval2(k, &curve, curr_t - step * (curr_t - last_t), true)?;
                curr_pv[0] = tmp;
                step *= 2.0;
                if step > 0.5 {
                    break;
                }
            }
        }

        // Place Curr Point [param, posXYZ, tanXYZ] on sStack
        push_point(curr_t, curr_pv, &mut stack);

        // For each span subdivide until our approximating curve is accurate enough.
        while !stack.is_empty() {
            // get last point's param, pos, and tan from stack - leave stack unmodified
            (curr_t, curr_pv) = stack_look(&stack)?;

            // Test if HermiteSpan is within ApproxTol of thisCurve interval
            let acc = field_span_accuracy(
                k,
                &curve,
                last_t,
                last_pv,
                curr_t,
                curr_pv,
                approx_tol_3d,
                match_parameterization,
            )?;

            // When Span passes test (or If step size gets real small just accept it)
            if acc.passes || (last_t - curr_t).abs() < EFF_ZERO {
                // (not upstream) bound the growing curve: see MAX_APPROX_SPANS
                if knots.len() > MAX_APPROX_SPANS {
                    return Err(SolidError::invalid_input(
                        OP_APPROX_FIELD,
                        "approximation needs more than MAX_APPROX_SPANS spans",
                    ));
                }

                // Append Hermite interval to end of growing piecewise cubic bezier
                add_to_curve(last_pv, curr_pv, last_t, curr_t, &mut ctrl_poly, &mut knots)?;

                // for next iteration - move Curr param, pos, and tan to Last
                last_t = curr_t;
                last_pv = curr_pv;

                // for next iteration - pop stack
                pop_stack(&mut stack)?;
            } else {
                if subdivisions >= MAX_APPROXIMATION_SUBDIVISIONS {
                    return Err(SolidError::sm_err(OP_APPROX_FIELD));
                }
                subdivisions += 1;

                // get param mid point and associated curve pos and tan
                let mid_t = (last_t + curr_t) / 2.0;
                if !mid_t.is_finite() || mid_t == last_t || mid_t == curr_t {
                    return Err(SolidError::sm_err(OP_APPROX_FIELD));
                }
                let mid_pv = field_eval2(k, &curve, mid_t, true)?; // NonZeroTangent values
                curr_t = mid_t;
                curr_pv = mid_pv;

                // push midPoint onto stach
                push_point(curr_t, curr_pv, &mut stack);
            }
        }
    }

    // create pBSC curve from control polygon
    let bsc = flush_crv(approx_tol_3d, &mut ctrl_poly, &mut knots, create_analytics)?.ok_or(
        SolidError::invalid_input(OP_APPROX_FIELD, "no approximating curve built"),
    )?;

    // pBSCurve->Copy(crContext, rpNewBSplineCurve)
    Ok(bsc.copy())
}

/// `SmTangentField::Create(ctx, tol, boundary, uv, surface, start, t0, SAME, end, t1,
/// SAME)` followed by its `ApproximateCurve` over the knots of `boundary`: the
/// cross-tangent curve of `boundary` on `surface`.
fn tangent_field_curve(
    k: &crate::kernel::Kernel,
    tol: f64,
    boundary: &BSplineCurve,
    uv_curve: &BSplineCurve,
    surface: &SmSurface,
    (start, start_param): (&BSplineCurve, f64),
    (end, end_param): (&BSplineCurve, f64),
    ang_tol: f64,
) -> Result<BSplineCurve> {
    let base_3d = as_sm_curve(boundary);
    let base_uv = as_sm_curve(uv_curve);
    let start_crv = as_sm_curve(start);
    let end_crv = as_sm_curve(end);
    let field = TangentField::create(
        k,
        tol,
        &base_3d,
        &base_uv,
        surface,
        TangentFieldEnd {
            curve: &start_crv,
            param: start_param,
            opposite: false,
        },
        TangentFieldEnd {
            curve: &end_crv,
            param: end_param,
            opposite: false,
        },
    )?;

    // build an approximate piecewise hermite curve to the crossTangent curve
    let knots = unique_knots(boundary);
    approximate_tangent_field(k, &field, &knots, ang_tol)
}

/// `new SmTangentField(uv, surface, SM_INTERPOLATE_DIR, start, end)` followed by its
/// `ApproximateCurve(knots, ang_tol, ..)`.
fn interpolated_tangent_field_curve(
    k: &crate::kernel::Kernel,
    uv_curve: &BSplineCurve,
    surface: &SmSurface,
    start: Vec3,
    end: Vec3,
    knots: &[f64],
    ang_tol: f64,
) -> Result<BSplineCurve> {
    let base_uv = as_sm_curve(uv_curve);
    let field = TangentField::new(
        k,
        &base_uv,
        surface,
        TangentDirType::Interpolate,
        start,
        end,
        None,
    )?;
    approximate_tangent_field(k, &field, knots, ang_tol)
}

/// `SmSurface::EvaluateNormal(uv, TRUE, TRUE, normal)`.
fn evaluate_normal(k: &crate::kernel::Kernel, surface: &SmSurface, uv: Point2) -> Result<Vec3> {
    surface.evaluate_normal(k, uv, Side::Left, Side::Left)
}

/// `SmSurface::EvaluateNormalSection(uv, TRUE, TRUE, direction, ..)`: the cross
/// tangent (the unit projection of `direction` into the tangent plane).
fn evaluate_normal_section(
    k: &crate::kernel::Kernel,
    surface: &SmSurface,
    uv: Point2,
    dir: Vec3,
) -> Result<Vec3> {
    Ok(surface
        .evaluate_normal_section(k, uv, Side::Left, Side::Left, dir)?
        .tangent)
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`: `(Su, Sv)`.
fn evaluate_1st_derivatives(
    k: &crate::kernel::Kernel,
    surface: &SmSurface,
    uv: Point2,
) -> Result<(Vec3, Vec3)> {
    let (_, du, dv) = crate::surface::surface::p1::evaluate_1st_derivatives(
        k,
        surface,
        uv,
        Side::Left,
        Side::Left,
    )?;
    Ok((du, dv))
}

/// `SmCurve::LocalCurveIntersect(domain, other, other_domain, tol, guess, other_guess,
/// ..)`: whether an intersection was found and its parameter on `rail` (the status is
/// ignored upstream: a failure is no intersection; `Unported` is propagated).
fn local_curve_intersect(
    k: &crate::kernel::Kernel,
    rail: &BSplineCurve,
    domain: &Extent1d,
    other: &BSplineCurve,
    other_domain: &Extent1d,
    tol: f64,
    guess: f64,
    other_guess: f64,
) -> Result<(bool, f64)> {
    let r = ignored(crate::curve::curve::p3::local_curve_intersect(
        k,
        rail,
        domain,
        other,
        other_domain,
        tol,
        guess,
        other_guess,
    ))?;
    Ok(match r.flatten() {
        Some(i) => (true, i.my_param),
        None => (false, UNDEF_DOUBLE),
    })
}

// ---- helpers --------------------------------------------------------------------------

/// Runs `f` on the two curves `curves[a]` and `curves[b]` as one slice (upstream's
/// array of pointers to them).
fn with_pair<R>(
    curves: &mut [BSplineCurve],
    a: usize,
    b: usize,
    f: impl FnOnce(&mut [BSplineCurve]) -> R,
) -> Result<R> {
    let oob = || SolidError::invalid_input(OP_CORNER, "curve index out of range");
    let first = core::mem::take(curves.get_mut(a).ok_or_else(oob)?);
    let second = core::mem::take(curves.get_mut(b).ok_or_else(oob)?);
    let mut pair = [first, second];
    let r = f(&mut pair);
    let [first, second] = pair;
    *curves.get_mut(a).ok_or_else(oob)? = first;
    *curves.get_mut(b).ok_or_else(oob)? = second;
    Ok(r)
}

/// `rail.GetKnots(knots)`: the distinct knots (empty on failure; upstream ignores the
/// status).
fn unique_knots(curve: &BSplineCurve) -> Vec<f64> {
    curve.knots(None).map(|k| k.0).unwrap_or_default()
}

impl BSplineSurface {
    // ---- CreateCornerBlend ----------------------------------------------------------

    /// Blend surfaces filling the region bounded by `curves_3d` (`CreateCornerBlend`).
    ///
    /// Curve order is counterclockwise starting with the left curve (u minimum):
    /// left, bottom, right, top for four curves; `curve_orients[i]` tells whether
    /// curve `i` runs in that direction ([`OrientType::Same`]). Rational curves are
    /// first approximated within `approx_tol / 2` by non-rational ones (with their UV
    /// curves dropped onto the surfaces).
    ///
    /// With four curves the result is one Coons patch
    /// ([`BSplineSurface::create_coons_patch`]): bilinear with
    /// `create_bilinear_patch` (UV curves and surfaces are then optional), else
    /// bicubic matching the cross tangents of the `neighbours` surfaces along their
    /// `uv_curves` within `tangency_tol_radians` (one degenerate curve without surface
    /// gives a triangular patch). More curves give an n-sided patch
    /// ([`BSplineSurface::create_n_sided_patch`]) matching the neighbours' cross
    /// tangents.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for fewer than four curves, a non-degenerate curve without surface,
    /// corner gaps above `approx_tol` and the failures of the called routines;
    /// `SM_ERR_NULL_POINTER` when a G1 blend misses too many UV curves or surfaces;
    /// `InvalidInput` for input arrays shorter than `curves_3d` or missing UV curves or
    /// surfaces where upstream dereferences them; [`SolidError::Unported`] from the
    /// neighbour surfaces' evaluators.
    pub fn create_corner_blend(
        k: &crate::kernel::Kernel,
        curves_3d: &[BSplineCurve],
        curve_orients: &[OrientType],
        uv_curves: &[Option<BSplineCurve>],
        neighbours: &[Option<BlendNeighbour<'_>>],
        approx_tol: f64,
        tangency_tol_radians: f64,
        create_bilinear_patch: bool,
    ) -> Result<Vec<Self>> {
        let refs: Vec<Option<&BSplineCurve>> = curves_3d.iter().map(Some).collect();
        Self::create_corner_blend_in::<crate::surface::surface::p4::NoObject>(
            k,
            &refs,
            curve_orients,
            uv_curves,
            neighbours,
            approx_tol,
            tangency_tol_radians,
            create_bilinear_patch,
            None,
        )
    }

    /// [`BSplineSurface::create_corner_blend`] over general 3D [`SmCurve`]s
    /// (`const SmTArray<SmCurve *>&`, downcast per curve via `SM_CAST_PTR(SmBSplineCurve,
    /// pCurve)` in loop order) whose rational-curve drops onto the neighbour surfaces
    /// run under the caller's face context: `context` builds the surface caches and
    /// `handles[i]` names `neighbours[i]` in them.
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::create_corner_blend`], `SM_ERR_NULL_POINTER` when a reached
    /// 3D curve is not `SmBSplineCurve`-derived, and those of the context.
    pub fn create_corner_blend_with_context<O: crate::geom::solution_array::SolutionObject>(
        k: &crate::kernel::Kernel,
        curves_3d: &[SmCurve],
        curve_orients: &[OrientType],
        uv_curves: &[Option<BSplineCurve>],
        neighbours: &[Option<BlendNeighbour<'_>>],
        approx_tol: f64,
        tangency_tol_radians: f64,
        create_bilinear_patch: bool,
        context: Option<(
            &mut dyn crate::surface::surface::p1::SurfaceSolveContext<O>,
            &[Option<O>],
        )>,
    ) -> Result<Vec<Self>> {
        let refs: Vec<Option<&BSplineCurve>> = curves_3d
            .iter()
            .map(|c| match c.kind() {
                CurveKind::BSpline(b) => Some(b),
                CurveKind::Line(l) => Some(&l.base),
                CurveKind::Ellipse(e) => Some(e.base()),
                CurveKind::Hyperbola(h) => Some(h.base()),
                CurveKind::Parabola(p) => Some(p.base()),
                _ => None,
            })
            .collect();
        Self::create_corner_blend_in(
            k,
            &refs,
            curve_orients,
            uv_curves,
            neighbours,
            approx_tol,
            tangency_tol_radians,
            create_bilinear_patch,
            context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_corner_blend_in<O: crate::geom::solution_array::SolutionObject>(
        k: &crate::kernel::Kernel,
        curves_3d: &[Option<&BSplineCurve>],
        curve_orients: &[OrientType],
        uv_curves: &[Option<BSplineCurve>],
        neighbours: &[Option<BlendNeighbour<'_>>],
        approx_tol: f64,
        tangency_tol_radians: f64,
        create_bilinear_patch: bool,
        mut context: Option<(
            &mut dyn crate::surface::surface::p1::SurfaceSolveContext<O>,
            &[Option<O>],
        )>,
    ) -> Result<Vec<Self>> {
        const OP: &str = OP_CORNER;

        // check state - must have 4 or more edges
        if curves_3d.len() < 4 {
            return Err(SolidError::sm_err(OP));
        }
        let curve_cnt = curves_3d.len();
        let surfaces: Vec<Option<&SmSurface>> =
            neighbours.iter().map(|n| n.map(|n| n.surface)).collect();
        let surfaces = surfaces.as_slice();
        if uv_curves.len() < curve_cnt || surfaces.len() < curve_cnt {
            return Err(SolidError::invalid_input(
                OP,
                "fewer UV curves or surfaces than curves",
            ));
        }

        // check state - when bCreateBilinearPatch == FALSE, UVCurves and surfaces are
        // required for every curve except when the 4th curve in a 4 sided patch is
        // degenerate
        if !create_bilinear_patch {
            let mut null_count = 0usize;

            // for every boundary curve
            for (uv, srf) in uv_curves.iter().zip(surfaces).take(curve_cnt) {
                // count the associated NULL UVTrimCurve and Surface inputs
                if uv.is_none() || srf.is_none() {
                    null_count += 1;
                }

                // Check inputs for triangle, 4 sided and NSided cases
                if ((curve_cnt == 4 && null_count > 1) || (curve_cnt > 4 && null_count > 0))
                    && (uv.is_none() || srf.is_none())
                {
                    return Err(SolidError::null_pointer(OP));
                }
            }
        }

        // convert all input edges from NURBs to NUBs
        let mut boundaries: Vec<BSplineCurve> = Vec::new();
        let mut uv_crvs: Vec<Option<BSplineCurve>> = Vec::new();
        for (i, ((curve, uv), srf)) in curves_3d.iter().zip(uv_curves).zip(neighbours).enumerate() {
            // sm_CreateNonRationalCurve: SmBSplineCurve *pNurb = SM_CAST_PTR(SmBSplineCurve, pCurve); NER(pNurb);
            let curve = (*curve).ok_or_else(|| SolidError::null_pointer(OP))?;
            // copy NUBs and their UVTrimCurves, approx NURBs and drop them to make their
            // UVTrimCurves
            let base = srf.map(|n| n.surface);
            let ctx = context.as_mut().map(|(solver, handles)| DropSolveContext {
                solver: &mut **solver,
                handle: handles.get(i).copied().flatten(),
            });
            let (boundary, ps_curve) = create_non_rational_curve_with_context(
                k,
                curve,
                uv.as_ref(),
                base,
                approx_tol / 2.0,
                ctx,
            )?;
            boundaries.push(boundary); // cleaned up 3d boundary curves
            uv_crvs.push(ps_curve); // associated UVTrimCurves
        }

        // If we have 4 curves then we need to reverse opposite ones and make them
        // compatible
        if boundaries.len() == 4 {
            return corner_blend_4(
                k,
                boundaries,
                uv_crvs,
                curve_orients,
                surfaces,
                approx_tol,
                tangency_tol_radians,
                create_bilinear_patch,
            );
        }

        corner_blend_n(
            k,
            boundaries,
            uv_crvs,
            curve_orients,
            surfaces,
            approx_tol,
            tangency_tol_radians,
        )
    }

    // ---- CreateSkinnedSurface -------------------------------------------------------

    /// The surface skinned through `cross_sections`, optionally along one or two rails
    /// and with cross-boundary derivative surfaces at its start and end
    /// (`CreateSkinnedSurface`).
    ///
    /// With `rail1_used_as_spine_curve` the cross sections are aligned along `rail1`
    /// as spine (`N_CreateSkinSpine`); otherwise they are skinned with
    /// `N_CreateSkinSrfBoundaryContinuity`, retried without the knot vector and then
    /// without the derivative surfaces when it fails. Without derivative surfaces the
    /// degree is lowered to `cross sections - 1` when needed. Two rails are
    /// synchronized first (and changed in place).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a spine without `rail1` or intersection parameters;
    /// `SM_ERR` for rails without intersection parameters, curves or surfaces without
    /// NURB, or when the skinning fails; `InvalidInput` for no cross sections or too
    /// few intersection parameters; [`SolidError::Unported`] when two rails change
    /// their domains (`SmCurve::LocalCurveIntersect`).
    pub fn create_skinned_surface(
        k: &crate::kernel::Kernel,
        cross_sections: &[BSplineCurve],
        opts: SkinnedSurfaceOptions<'_>,
    ) -> Result<Self> {
        const OP: &str = OP_SKIN;
        let SkinnedSurfaceOptions {
            synchronized,
            cross_section_surf_param,
            approx_tol,
            mut rail1,
            mut rail2,
            rail1_used_as_spine_curve,
            intersections_with_rails,
            deriv_surfs,
            degree,
        } = opts;
        let num_sections = cross_sections.len();

        // check input - when using a spine, spine curve and IntersectionsWithRails
        // should be given
        if rail1_used_as_spine_curve {
            if rail1.is_none() {
                return Err(SolidError::null_pointer(OP));
            }
            if intersections_with_rails.is_none() {
                return Err(SolidError::null_pointer(OP));
            }
        }

        if (rail1.is_some() || rail2.is_some()) && intersections_with_rails.is_none() {
            return Err(SolidError::sm_err(OP));
        }
        if num_sections == 0 {
            return Err(SolidError::invalid_input(OP, "no cross sections"));
        }

        // We'll need a local non-const copy of this:
        let mut int_params: Option<Vec<f64>> = intersections_with_rails.map(<[f64]>::to_vec);

        // When given two rails - synchronize the knot vectors
        if let (Some(r1), Some(r2)) = (rail1.as_deref_mut(), rail2.as_deref_mut()) {
            // Note, this will reparameterize the curves, which will render the
            // intersection parameters invalid. We'll have to correct that.
            let domain1_before = r1.natural_interval();
            let domain2_before = r2.natural_interval();

            let mut rails = [core::mem::take(r1), core::mem::take(r2)];
            let sync = BSplineCurve::synchronize_knots_of_curves(&mut rails, EFF_ZERO * 100.0);
            let [a, b] = rails;
            *r1 = a;
            *r2 = b;
            sync?;

            // See about adjusting the intersection parameters.
            let domain1_after = r1.natural_interval();
            let domain2_after = r2.natural_interval();

            if let Some(params) = int_params.as_mut() {
                let tol = 0.001
                    * crate::curve::curve::p2::approximate_length(
                        k,
                        &*r1,
                        &domain1_after,
                        6,
                        false,
                    )?
                    .0;

                // Check rail 1.
                if (domain1_after.min() - domain1_before.min()).abs() > EFF_ZERO
                    || (domain1_after.max() - domain1_before.max()).abs() > EFF_ZERO
                {
                    for (ii, sect_crv) in cross_sections.iter().enumerate() {
                        // Map the parameter from the old domain to the new one.
                        let fraction = domain1_before
                            .inversion(at(params, ii, OP)?)
                            .parameter
                            .unwrap_or(UNDEF_DOUBLE);
                        let new_param = domain1_after.evaluate(fraction);

                        // reintersect, to be safe
                        let cross_domain = sect_crv.natural_interval();
                        let (found, t_rail) = local_curve_intersect(
                            k,
                            r1,
                            &domain1_after,
                            sect_crv,
                            &cross_domain,
                            tol,
                            new_param,
                            cross_domain.min(),
                        )?;
                        let slot = params.get_mut(ii).ok_or_else(|| {
                            SolidError::invalid_input(OP, "too few intersection parameters")
                        })?;
                        *slot = if found { t_rail } else { new_param };
                    }
                }

                // Check rail 2.
                if (domain2_after.min() - domain2_before.min()).abs() > EFF_ZERO
                    || (domain2_after.max() - domain2_before.max()).abs() > EFF_ZERO
                {
                    for (ii, sect_crv) in cross_sections.iter().enumerate() {
                        // Map the parameter from the old domain to the new one.
                        let fraction = domain2_before
                            .inversion(at(params, num_sections + ii, OP)?)
                            .parameter
                            .unwrap_or(UNDEF_DOUBLE);
                        let new_param = domain2_after.evaluate(fraction);

                        let cross_domain = sect_crv.natural_interval();
                        let (found, t_rail) = local_curve_intersect(
                            k,
                            r2,
                            &domain2_after,
                            sect_crv,
                            &cross_domain,
                            tol,
                            new_param,
                            cross_domain.max(),
                        )?;
                        let slot = params.get_mut(num_sections + ii).ok_or_else(|| {
                            SolidError::invalid_input(OP, "too few intersection parameters")
                        })?;
                        *slot = if found { t_rail } else { new_param };
                    }
                }
            }
        } // end synchronizing knot vectors, if given both rails

        // load rails into rails array and save knot vector
        let mut rails: [Option<Curve>; 2] = [None, None];
        let mut knots: Vec<f64> = Vec::new();
        if let Some(r2) = rail2.as_deref() {
            knots = unique_knots(r2);
            rails[1] = Some(create_nlib_curve(r2)?);
        }
        if let Some(r1) = rail1.as_deref() {
            knots = unique_knots(r1);
            rails[0] = Some(create_nlib_curve(r1)?);
        }

        // If there are only a couple of profile curves, the degree of the rails must be
        // high enough to fit the constraints, in the sweep direction.  [B542]
        if num_sections <= 3 {
            let mut num_deriv_surfs = 0;
            if let Some(ds) = &deriv_surfs {
                num_deriv_surfs = ds.iter().filter(|d| d.is_some()).count();
            }
            let num_constraints = num_sections + num_deriv_surfs;
            for rail in rails.iter_mut().flatten() {
                let deg = rail.degree();
                if deg + 1 < num_constraints {
                    let increment = num_constraints - 1 - deg;
                    // status ignored upstream
                    if let Ok(elevated) = rail.elevate_degree(increment) {
                        *rail = elevated;
                    }
                }
            }
        }

        // iter every crossSection curve building the cross sections array
        let mut curves: Vec<Curve> = cross_sections
            .iter()
            .map(create_nlib_curve)
            .collect::<Result<_>>()?;

        // load start and end skin surface Cross-boundary derivatives
        let mut dir_array: Option<[Option<Surface>; 2]> = None;
        if let Some(ds) = &deriv_surfs {
            let mut nl_derivs: [Option<Surface>; 2] = [None, None];
            for (slot, d) in nl_derivs.iter_mut().zip(ds) {
                if let Some(d) = d {
                    *slot = Some(create_nlib_surface(d)?);
                }
            }
            if ds.iter().any(Option::is_some) {
                dir_array = Some(nl_derivs);
            }
        }

        // NLib locals
        let num_cross_curves = num_sections;
        let nl_dir = if cross_section_surf_param == SurfParamType::U {
            Direction::U
        } else {
            Direction::V
        };
        let has_rails = rail1.is_some() || rail2.is_some();
        let mut nl_deg = degree;

        // If there are no additional constraints, then use the lowest degree that will
        // interpolate the given number of cross curves in a single span. [5/28/10]
        if dir_array.is_none() && degree >= num_cross_curves {
            nl_deg = num_cross_curves - 1;
        }

        // when pIntersectionsWithRails is given
        let mut nl_knots: Option<Vec<f64>> = None;
        if let Some(params) = &int_params {
            if !knots.is_empty() {
                // use the rail knot vector
                if let Some(r) = &rails[0] {
                    nl_knots = Some(r.knots().to_vec());
                } else if let Some(r) = &rails[1] {
                    nl_knots = Some(r.knots().to_vec());
                }
            } else if dir_array.is_some() {
                // use a regularly spaced knot vector (only used with cross-boundary
                // derivatives, below)
                let mut kv = vec![0.0; num_cross_curves + nl_deg + 1];
                make_knots(params, num_cross_curves - 1, nl_deg, &mut kv)?;
                nl_knots = Some(kv);
            }
        }

        // when cross-derivatives are not specified - don't specify a knot vector
        if dir_array.is_none() {
            nl_knots = None;
        }

        let nl_surf = if rail1_used_as_spine_curve {
            // sub = Subdivision level
            let sub = if num_cross_curves < 9 { 5 } else { 2 };

            // evaluate rail curve start and use the tangent to build a orthonormal
            // vector set
            let r1 = rail1
                .as_deref()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let pv = r1.evaluate(r1.natural_interval().min(), 1, true, false)?;
            let tangent = pv.get(1).copied().ok_or_else(|| SolidError::sm_err(OP))?;
            let [_, _, z_axis] = tangent.make_unit_ortho_vectors(None)?;

            let spine = rails[0]
                .as_ref()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            let params = int_params
                .as_deref()
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            create_skin_spine(
                &mut curves,
                spine,
                params,
                &SkinSpineOptions {
                    make_compatible: !synchronized,
                    align: Some(SpineAlignment {
                        z0: z_axis,
                        subdiv: sub,
                    }),
                    derivs: if nl_deg < 2 {
                        SkinDerivs::None
                    } else {
                        SkinDerivs::Ends
                    },
                    degree: nl_deg,
                    dir: nl_dir,
                },
            )
            .map_err(SolidError::nl_ser(OP))?
        } else {
            // skin crossSections with optional rail curves and start/end
            // cross-derivative constraints
            let has_dir_array = dir_array.is_some();
            let mut sk = SkinBoundaryOptions {
                der: dir_array,
                cmp: synchronized,
                cnt: [BoundaryContinuity::G1, BoundaryContinuity::G1],
                bnd: [Boundary::Bottom, Boundary::Bottom],
                rail: if has_rails { Some(rails) } else { None },
                deg: nl_deg,
                es: approx_tol,
                dir: nl_dir,
                ts: int_params,
                kns: nl_knots,
            };
            match create_skin_srf_boundary_continuity(&mut curves, &mut sk) {
                Ok(s) => s,
                Err(e @ SolidError::Unported { .. }) => return Err(e),
                Err(_) => {
                    // If not successful try without knots.
                    sk.kns = None;
                    match create_skin_srf_boundary_continuity(&mut curves, &mut sk) {
                        Ok(s) => s,
                        Err(e @ SolidError::Unported { .. }) => return Err(e),
                        Err(_) => {
                            // It failed with derivative fields try it without them
                            if has_dir_array {
                                sk.der = None;
                                create_skin_srf_boundary_continuity(&mut curves, &mut sk)
                                    .map_err(SolidError::nl_ser(OP))?
                            } else {
                                // its not working - signal an error
                                return Err(SolidError::sm_err(OP));
                            }
                        }
                    }
                }
            }
        };

        // set output
        Ok(Self::from_gw_nurb(&nl_surf))
    }

    // ---- TrimWithDomain -------------------------------------------------------------

    /// Trims the surface to `trim_domain` (`TrimWithDomain`): the domain is snapped
    /// to knots (or moved away from them) so that no span shorter than about
    /// `10 * SM_EFF_ZERO_SQRT` is created, never shrunk into the requested domain,
    /// and the patch over it is extracted (`N_SrfExtractPatch`) and becomes the NURB.
    /// `trim_domain` receives the snapped domain.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `trim_domain` is not inside the natural domain or the extraction
    /// fails; `InvalidInput` when the snapped u boundaries are too close;
    /// `SM_ERR_NULL_POINTER` without NURB (once a trim is needed).
    pub fn trim_with_domain(&mut self, trim_domain: &mut Extent2d) -> Result<()> {
        self.trim_with_domain_with(trim_domain, TrimOptions::default())
    }

    /// [`BSplineSurface::trim_with_domain`] with [`TrimOptions`]; the default options
    /// are upstream's `TrimWithDomain`.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::trim_with_domain`].
    pub fn trim_with_domain_with(
        &mut self,
        trim_domain: &mut Extent2d,
        options: TrimOptions,
    ) -> Result<()> {
        const OP: &str = OP_TRIM;
        let snap = options.snap_roundoff_to_knots;
        // get current natural boundary
        let natural_domain = self.natural_uv_domain();

        // check input - trim domain not contained within natural domain
        if !trim_domain.is_contained_by(&natural_domain, EFF_ZERO_PARAM) {
            return Err(SolidError::sm_err(OP));
        }

        // no work - natural domain is same size as trim domain
        if natural_domain.is_contained_by(trim_domain, EFF_ZERO_PARAM) {
            return Ok(());
        }

        // get trim domain corner points
        let mut ul = trim_domain.min().x;
        let mut ur = trim_domain.max().x;
        let mut vl = trim_domain.min().y;
        let mut vr = trim_domain.max().y;

        // Not upstream (#472): the requested domain as sm_CheckTrimBoundaries sees it,
        // with any boundary snapped onto a knot by `snap_roundoff_to_knots`.
        let (mut req_ul, mut req_ur, mut req_vl, mut req_vr) = (ul, ur, vl, vr);

        // don't build too small spans in u direction.
        {
            // scale tolerances to domain size
            let tol_min = EFF_ZERO_SQRT * (1.0 + ul.abs());
            let tol_max = EFF_ZERO_SQRT * (1.0 + ur.abs());

            let mut idx_min: Option<usize> = None;
            let mut idx_max: Option<usize> = None;

            // get current U knots
            let knots: Vec<f64> = self
                .knots(SurfParamType::U, None)
                .map(|k| k.0)
                .unwrap_or_default();
            let last = knots.len().saturating_sub(1);

            // for every U knot - looking to snap trim boundaries
            for (i, &knot) in knots.iter().enumerate() {
                // when lower trim boundary is close to knot
                if (knot - ul).abs() < tol_min {
                    // when trim boundary is slightly larger than knot - set trim
                    // boundary to knot (or, not upstream, a rounding error smaller)
                    if ul >= knot || i == 0 || (snap && knot - ul <= EFF_ZERO_PARAM) {
                        if snap && ul < knot {
                            req_ul = knot;
                        }
                        ul = knot;
                        idx_min = Some(i);
                    } else {
                        // Set UL to either the previous knot or slightly less than
                        // current knot
                        ul = knot - 10.0 * tol_min;
                        idx_min = Some(i);
                        let prev = at(&knots, i - 1, OP)?;
                        if ul < prev {
                            ul = prev;
                            idx_min = Some(i - 1);
                        }
                    }
                }

                // when upper trim boundary is close to knot
                if (knot - ur).abs() < tol_max {
                    if ur <= knot || i == last || (snap && ur - knot <= EFF_ZERO_PARAM) {
                        if snap && ur > knot {
                            req_ur = knot;
                        }
                        ur = knot;
                        idx_max = Some(i);
                    } else {
                        ur = knot + 10.0 * tol_max;
                        let next = at(&knots, i + 1, OP)?;
                        if ur > next {
                            ur = next;
                            idx_max = Some(i + 1);
                        }
                    }
                }
            }

            // Make sure we're not shrinking the input domain. [B306]
            (ul, ur) = super::p1::check_trim_boundaries(
                &knots,
                ul,
                idx_min,
                ur,
                idx_max,
                &Extent2d::from_bounds(req_ul, req_vl, req_ur, req_vr),
                SurfParamType::U,
            )?;

            // no work - snapped trim boundaries are within tolerance of one another
            if (ur - ul).abs() < tol_min + tol_max {
                // Error here because trim points are too close together
                return Err(SolidError::invalid_input(
                    OP,
                    "trim boundaries too close together",
                ));
            }
        }

        // don't build too small spans in V direction.
        {
            let tol_min = EFF_ZERO_SQRT * (1.0 + vl.abs());
            let tol_max = EFF_ZERO_SQRT * (1.0 + vr.abs());

            let mut idx_min: Option<usize> = None;
            let mut idx_max: Option<usize> = None;

            let knots: Vec<f64> = self
                .knots(SurfParamType::V, None)
                .map(|k| k.0)
                .unwrap_or_default();
            let last = knots.len().saturating_sub(1);
            for (i, &knot) in knots.iter().enumerate() {
                // snap lower V Trim boundaries as appropriate
                if (knot - vl).abs() < tol_min {
                    if vl >= knot || i == 0 || (snap && knot - vl <= EFF_ZERO_PARAM) {
                        if snap && vl < knot {
                            req_vl = knot;
                        }
                        vl = knot;
                        idx_min = Some(i);
                    } else {
                        vl = knot - 10.0 * tol_min;
                        idx_min = Some(i);
                        let prev = at(&knots, i - 1, OP)?;
                        if vl < prev {
                            vl = prev;
                            idx_min = Some(i - 1);
                        }
                    }
                }

                // snap upper V Trim Boundaries as appropriate
                if (knot - vr).abs() < tol_max {
                    if vr <= knot || i == last || (snap && vr - knot <= EFF_ZERO_PARAM) {
                        if snap && vr > knot {
                            req_vr = knot;
                        }
                        vr = knot;
                        idx_max = Some(i);
                    } else {
                        vr = knot + 10.0 * tol_max;
                        idx_max = Some(i);
                        let next = at(&knots, i + 1, OP)?;
                        if vr > next {
                            vr = next;
                            idx_max = Some(i + 1);
                        }
                    }
                }
            }

            // Make sure we're not shrinking the input domain. [B306]
            (vl, vr) = super::p1::check_trim_boundaries(
                &knots,
                vl,
                idx_min,
                vr,
                idx_max,
                &Extent2d::from_bounds(req_ul, req_vl, req_ur, req_vr),
                SurfParamType::V,
            )?;
        }

        // place snapped Trim Boundary values into input TrimDomain (status ignored)
        let _ = trim_domain.set_min_max(Point2::new(ul, vl), Point2::new(ur, vr));

        // local surface pointer
        let sur_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP))?;

        // extract patch defined by trim boundaries from surP into sur1
        let sur1 =
            extract_patch(sur_p, Rectangle::new(ul, ur, vl, vr)).map_err(SolidError::nl_ser(OP))?;

        // build SMLib surface from NLib sur1 and swap the NURBs
        self.nurb = Self::from_gw_nurb(&sur1).nurb;
        Ok(())
    }

    // ---- Evaluate -------------------------------------------------------------------

    /// The point and partial derivatives at `uv` (`Evaluate`).
    ///
    /// Returns `D[i][j] = d^(i+j) S / du^i dv^j` for `i <= highest_u_deriv`,
    /// `j <= highest_v_deriv` ([`BSplineSurface::evaluate_simple`] semantics: `uv` is
    /// clamped to the natural domain unless out-of-bounds evaluation is enabled or the
    /// surface is in power-basis form). With `flags.non_zero_tangents` a first
    /// derivative shorter than `10 * SM_EFF_ZERO * (1 + max|P|)` is replaced: from the
    /// second derivatives (`Suv`, else `Suu` / `Svv`), or else by the direction of a
    /// good first derivative at a nearby point (stepping `1e-5 .. 0.1` towards the
    /// middle of the domain and diagonally), scaled to `1.1` times that limit.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `only_upper_half` with different orders or an order above
    /// `NL_MAXDER` (4); `SM_ERR` without NURB, when NLib fails, or when no
    /// replacement for a zero first derivative is found.
    pub fn evaluate(
        &self,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.evaluate_impl(uv, highest_u_deriv, highest_v_deriv, flags, true, None)
    }

    /// Preserve the output written before zero-tangent correction reports failure.
    /// `SmSurfaceEval` accessors deliberately read that output after a failed eval.
    pub(crate) fn evaluate_with_partial_output(
        &self,
        uv: Point2,
        flags: EvaluateFlags,
    ) -> (Result<()>, Vec<Vec<Vec3>>) {
        let mut partial = Vec::new();
        match self.evaluate_impl(uv, 2, 2, flags, true, Some(&mut partial)) {
            Ok(values) => (Ok(()), values),
            Err(error) => (Err(error), partial),
        }
    }

    /// `Evaluate` for a caller that ignores its status (as `SmSurface::IsPeriodic`
    /// does): the derivative array as upstream leaves it in `aDerivatives`, also when
    /// the zero-tangent repair fails after the NLib evaluation wrote it (the raw or
    /// partly repaired derivatives); `None` when the call fails before writing it (bad
    /// orders, no NURB), leaving the caller's array as it was.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when NLib fails: upstream's array then holds whatever NLib wrote
    /// before failing, which the port does not reproduce.
    pub fn evaluate_ignoring_status(
        &self,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Option<Vec<Vec<Vec3>>>> {
        // the checks `Evaluate` fails on before anything is written
        if (flags.only_upper_half && highest_u_deriv != highest_v_deriv)
            || highest_u_deriv > MAX_DERIVATIVE
            || highest_v_deriv > MAX_DERIVATIVE
            || self.nurb.is_none()
        {
            return Ok(None);
        }
        let mut written = Vec::new();
        match self.evaluate_impl(
            uv,
            highest_u_deriv,
            highest_v_deriv,
            flags,
            true,
            Some(&mut written),
        ) {
            Ok(d) => Ok(Some(d)),
            Err(_) if !written.is_empty() => Ok(Some(written)),
            Err(e) => Err(e),
        }
    }

    /// `Evaluate` with its internal `bDoZeroSampling` argument.
    fn evaluate_impl(
        &self,
        cr_uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
        do_zero_sampling: bool,
        failed_output: Option<&mut Vec<Vec<Vec3>>>,
    ) -> Result<Vec<Vec<Vec3>>> {
        const OP: &str = OP_EVAL;
        let EvaluateFlags {
            u_side,
            v_side,
            only_upper_half,
            non_zero_tangents,
        } = flags;

        // check input - bOnlyUpperHalf can be TRUE only if deriv requests are equal.
        if only_upper_half && highest_u_deriv != highest_v_deriv {
            return Err(SolidError::invalid_input(
                OP,
                "only upper half with different orders",
            ));
        }
        if highest_u_deriv > MAX_DERIVATIVE || highest_v_deriv > MAX_DERIVATIVE {
            return Err(SolidError::invalid_input(
                OP,
                "derivative order above NL_MAXDER",
            ));
        }

        // When appropriate, clamp input parameters to surface domain
        let mut s_uv = cr_uv;
        let natural_uv_domain = self.natural_uv_domain();
        if self.form != BSplineSurfaceForm::Polynomial
            && !natural_uv_domain.contains_point(s_uv, 0.0)
            && !self.is_out_of_bounds_enabled()
        {
            s_uv = natural_uv_domain.clamp_point(s_uv);
        }

        // Setup for NLib call
        let (uflg, vflg) = (u_side, v_side);

        let sur = self.nurb.as_ref().ok_or_else(|| SolidError::sm_err(OP))?;
        let mut d = if self.form == BSplineSurfaceForm::Polynomial {
            srf_power_basis_eval_derivs(
                &sur.cpts(),
                sur.p(),
                sur.q(),
                s_uv.x,
                s_uv.y,
                only_upper_half,
                highest_u_deriv,
                highest_v_deriv,
            )
        } else {
            sur.derivs(
                s_uv.x,
                s_uv.y,
                uflg,
                vflg,
                only_upper_half,
                highest_u_deriv,
                highest_v_deriv,
            )
        }
        .map_err(SolidError::nl_ser(OP))?;

        // The zero-tangent repair below works on the matrix NLib wrote; upstream's
        // caller array keeps it (raw or partly repaired) whatever the repair's status,
        // including a failed recursive re-evaluation.
        let status = (|| -> Result<()> {
            let d = &mut d;
            // If we don't have to check for zero first derivatives, we're done.
            if !non_zero_tangents {
                return Ok(());
            }
            if highest_u_deriv < 1 && highest_v_deriv < 1 {
                return Ok(());
            }

            // flat index `k` of the upstream array: D[k / (nv + 1)][k % (nv + 1)]
            let nv1 = highest_v_deriv + 1;
            let at = |d: &[Vec<Vec3>], k: usize| -> Result<Vec3> {
                d.get(k / nv1)
                    .and_then(|row| row.get(k % nv1))
                    .copied()
                    .ok_or_else(|| SolidError::sm_err(OP))
            };
            let set = |d: &mut [Vec<Vec3>], k: usize, v: Vec3| -> Result<()> {
                let slot = d
                    .get_mut(k / nv1)
                    .and_then(|row| row.get_mut(k % nv1))
                    .ok_or_else(|| SolidError::sm_err(OP))?;
                *slot = v;
                Ok(())
            };

            // position and tangents
            let orig_pnt = at(d, 0)?;
            let orig_du = if highest_u_deriv >= 1 {
                at(d, nv1)?
            } else {
                at(d, 1)?
            };
            let orig_dv = if highest_v_deriv >= 1 {
                at(d, 1)?
            } else {
                at(d, nv1)?
            };

            // tangent sizes
            let orig_du_len = orig_du.length();
            let orig_dv_len = orig_dv.length();

            let deriv_lim = crate::surface::surface::p2::derivative_limit(orig_pnt);

            // test
            let zero_du = orig_du_len < deriv_lim;
            let zero_dv = orig_dv_len < deriv_lim;

            // no work - no zero tangents to fix
            if !zero_du && !zero_dv {
                return Ok(());
            }

            // when not enough derivatives were asked for - recurse to this function
            if highest_u_deriv < 2 || highest_v_deriv < 2 {
                let re = self.evaluate_impl(
                    cr_uv,
                    2,
                    2,
                    EvaluateFlags {
                        u_side,
                        v_side,
                        only_upper_half: true,
                        non_zero_tangents: true,
                    },
                    true,
                    None,
                )?;
                let re_at = |i: usize, j: usize| -> Result<Vec3> {
                    re.get(i)
                        .and_then(|row| row.get(j))
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP))
                };

                // save the kluged tangent values (aReDer[3] = Su, aReDer[1] = Sv)
                if highest_u_deriv >= 1 {
                    set(d, nv1, re_at(1, 0)?)?;
                }
                if highest_v_deriv >= 1 {
                    set(d, 1, re_at(0, 1)?)?;
                }
                return Ok(());
            }

            // Ok, now we have at least two derivatives.
            let lnv = highest_v_deriv;
            let mut su = at(d, lnv + 1)?;
            let mut sv = at(d, 1)?;
            let ok = super::p1::check_zero_surface_derivs(
                s_uv,
                &natural_uv_domain,
                &mut su,
                &mut sv,
                at(d, 2 * lnv + 2)?,
                at(d, lnv + 2)?,
                at(d, 2)?,
                deriv_lim,
            );
            set(d, lnv + 1, su)?;
            set(d, 1, sv)?;

            // all done?
            if ok {
                return Ok(());
            }

            // Try finding a nonZeroTangent value with a more brute-force method: step
            // towards the center of the domain.
            let step_dir_u = if s_uv.x > natural_uv_domain.u_interval().mid() {
                -1.0
            } else {
                1.0
            };
            let step_dir_v = if s_uv.y > natural_uv_domain.v_interval().mid() {
                -1.0
            } else {
                1.0
            };

            let mut step = ZONE_TOL_3D / 10.0; // works well in practice.

            // for larger and larger step off distances
            while do_zero_sampling && step < 0.1 {
                // increase the step off distance
                step *= 10.0;

                // for this step size: try 5 directions; last 4 are diagonals.
                // Make the first step go in the most likely direction: direction of good
                // deriv, towards center of surface.
                let first = if orig_du_len < orig_dv_len {
                    // step in v
                    Point2::new(s_uv.x, s_uv.y + step_dir_v * step)
                } else {
                    // step in u
                    Point2::new(s_uv.x + step_dir_u * step, s_uv.y)
                };
                let test_uv = [
                    first,
                    Point2::new(s_uv.x + step, s_uv.y + step),
                    Point2::new(s_uv.x - step, s_uv.y - step),
                    Point2::new(s_uv.x - step, s_uv.y + step),
                    Point2::new(s_uv.x + step, s_uv.y - step),
                ];

                // for all 5 sample directions at this step size
                for t_uv in test_uv {
                    let t_uv = natural_uv_domain.clamp_point(t_uv);
                    // just take what's naturally there; one derivative is enough.
                    let Ok(re) = self.evaluate_impl(
                        t_uv,
                        1,
                        1,
                        EvaluateFlags {
                            u_side,
                            v_side,
                            only_upper_half: true,
                            non_zero_tangents: false,
                        },
                        false,
                        None,
                    ) else {
                        continue;
                    };
                    // aReDer[2] = Su, aReDer[1] = Sv
                    let mut re_su = re
                        .get(1)
                        .and_then(|r| r.first())
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP))?;
                    let mut re_sv = re
                        .first()
                        .and_then(|r| r.get(1))
                        .copied()
                        .ok_or_else(|| SolidError::sm_err(OP))?;

                    // Save the nearby tangent values, if they're good enough.
                    if re_su.length() >= deriv_lim && re_sv.length() >= deriv_lim {
                        // Set magnitudes of degenerate derivatives to 1.1 * dDerivLim.
                        if zero_du {
                            re_su = re_su.unitize_or_z();
                            re_su *= 1.1 * deriv_lim;
                            set(d, highest_v_deriv + 1, re_su)?;
                        }
                        if zero_dv {
                            re_sv = re_sv.unitize_or_z();
                            re_sv *= 1.1 * deriv_lim;
                            set(d, 1, re_sv)?;
                        }

                        // all done
                        return Ok(());
                    }
                }
            }

            // If we make it to here we are unable to get both of the first derivatives to
            // a non-zero size.
            Err(SolidError::sm_err(OP))
        })();
        match status {
            Ok(()) => Ok(d),
            Err(e) => {
                // C++ has already written the matrix to the caller's array.
                if let Some(output) = failed_output {
                    *output = d;
                }
                Err(e)
            }
        }
    }
}

// ---- CreateCornerBlend: four curves -----------------------------------------------------

const LEFT: usize = 0;
const BOT: usize = 1;
const RIGHT: usize = 2;
const TOP: usize = 3;

/// The four-sided branch of `CreateCornerBlend`.
fn corner_blend_4(
    k: &crate::kernel::Kernel,
    mut boundaries: Vec<BSplineCurve>,
    mut uv_crvs: Vec<Option<BSplineCurve>>,
    curve_orients: &[OrientType],
    surfaces: &[Option<&SmSurface>],
    approx_tol: f64,
    tangency_tol_radians: f64,
    create_bilinear_patch: bool,
) -> Result<Vec<BSplineSurface>> {
    const OP: &str = OP_CORNER;
    let oob = || SolidError::invalid_input(OP, "curve index out of range");

    // for every boundary - set orientations
    for ii in 0..4 {
        // make bot & right orientation = Same, top & left orientation = Opposite
        let orient = at(curve_orients, ii, OP)?;
        if ((ii == TOP || ii == LEFT) && orient == OrientType::Same)
            || ((ii == BOT || ii == RIGHT) && orient != OrientType::Same)
        {
            let b = boundaries.get_mut(ii).ok_or_else(oob)?;
            let ivl = b.natural_interval();
            b.reverse_parameterization(&ivl)?;
            if let Some(uv) = uv_crvs.get_mut(ii).and_then(Option::as_mut) {
                let ivl = uv.natural_interval();
                uv.reverse_parameterization(&ivl)?;
            }
        }
    }

    // make left/right curve pair compatible (union knot vectors), normalizing the
    // domains first (MakeCurvesCompatible normalizes them anyway)
    let unit_ivl = Extent1d::from_min_max(0.0, 1.0);
    // There's a hard-coded knot tolerance everywhere of 1e-8 ... use that, increased a
    // bit for safety.   [T1000C, bd, 21 Dec 05]
    let knot_tol = 2.0e-8;
    with_pair(&mut boundaries, LEFT, RIGHT, |pair| {
        for c in pair.iter_mut() {
            let _ = c.edit_parameterization(&unit_ivl);
        }
        BSplineCurve::make_curves_compatible(pair, knot_tol)
    })??;

    // make top/bot curve pair compatible (union knot vectors)
    with_pair(&mut boundaries, BOT, TOP, |pair| {
        BSplineCurve::make_curves_compatible(pair, knot_tol)
    })??;

    let mut tans: Vec<BSplineCurve> = Vec::new();

    // when working with crossTangents (making g1 patches)
    if !create_bilinear_patch {
        // make sure all UVCurves have same parameter range as 3DCurve
        for (b, uv) in boundaries.iter().zip(uv_crvs.iter_mut()) {
            if let Some(uv) = uv {
                uv.edit_parameterization(&b.natural_interval())?;
            }
        }

        // for every boundary curve - Create Tangent Fields
        let mut tan_curves: Vec<BSplineCurve> = Vec::with_capacity(4);
        for ii in 0..4 {
            let (start_idx, end_idx) = if ii % 2 == 1 {
                (LEFT, RIGHT)
            } else {
                (BOT, TOP)
            };
            let start_curve = boundaries.get(start_idx).ok_or_else(oob)?;
            let end_curve = boundaries.get(end_idx).ok_or_else(oob)?;
            let normalized = if ii >= 2 { 1.0 } else { 0.0 };
            let start_param = start_curve.natural_interval().evaluate(normalized);
            let end_param = end_curve.natural_interval().evaluate(normalized);

            let boundary_curve = boundaries.get(ii).ok_or_else(oob)?;

            let tan_curve = if let Some(srf) = at(surfaces, ii, OP)? {
                // build temporary crossTangent curve function and an approximate
                // piecewise hermite curve to it
                let uv = uv_crvs
                    .get(ii)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| SolidError::invalid_input(OP, "missing UV curve"))?;
                tangent_field_curve(
                    k,
                    2.0 * approx_tol,
                    boundary_curve,
                    uv,
                    srf,
                    (start_curve, start_param),
                    (end_curve, end_param),
                    tangency_tol_radians,
                )?
            } else {
                // a degenerate curve which does not 'lie' on a surface
                if !boundary_curve.is_degenerate(k, EFF_ZERO, None)? {
                    return Err(SolidError::sm_err(OP));
                }

                // get neighbor curve endPoint tangents (SmCurve::Evaluate default:
                // non-zero tangents)
                let pv1 = start_curve.evaluate(start_param, 1, true, true)?;
                let pv2 = end_curve.evaluate(end_param, 1, true, true)?;
                let t1 = at(&pv1, 1, OP)?;
                let t2 = at(&pv2, 1, OP)?;

                // Create degree 3 crossTangent Vector linear interpolation of
                // crossTangent values
                let cntrl_poly = [t1, t1, t2, t2];
                let mut tan = BSplineCurve::create_canonical(&CanonicalData {
                    dim: 3,
                    degree: 3,
                    control_points: &cntrl_poly,
                    form: BSplineCurveForm::Unspecified,
                    knot_multiplicities: &[4, 4],
                    knots: &[0.0, 1.0],
                    knot_type: KnotType::Unspecified,
                    weights: None,
                    trim: None,
                })?;
                let trim_ivl = boundary_curve.natural_interval();
                tan.edit_parameterization(&trim_ivl)?;
                tan
            };

            // add crossTangent function to ordered array
            tan_curves.push(tan_curve);
        }

        // create [left, right, bot, top] ordered crossTangent curve array
        let mut tc: Vec<Option<BSplineCurve>> = tan_curves.into_iter().map(Some).collect();
        for idx in [LEFT, RIGHT, BOT, TOP] {
            let c = tc.get_mut(idx).and_then(Option::take).ok_or_else(oob)?;
            tans.push(c);
        }
    }

    // let tol = SM_ZONE_TOL_3D * boundaryCurve->BoundingBox maxDimension
    let mut bbox = Extent3d::new();
    for b in &boundaries {
        let ivl = b.natural_interval();
        let boxes = b.calculate_bounding_box(
            &ivl,
            BoundingBoxRequest {
                normal: true,
                pseudo: false,
                polar: false,
            },
        )?;
        let cbox = boxes.normal.unwrap_or_default();
        bbox = bbox.union(&cbox);
    }
    let tol = ZONE_TOL_3D * bbox.size().max_dimension();

    // create [left, right, bot, top] ordered boundary curve array
    let mut bc: Vec<Option<BSplineCurve>> = boundaries.into_iter().map(Some).collect();
    let mut bounds: Vec<BSplineCurve> = Vec::with_capacity(4);
    for idx in [LEFT, RIGHT, BOT, TOP] {
        let c = bc.get_mut(idx).and_then(Option::take).ok_or_else(oob)?;
        bounds.push(c);
    }

    // build coons patch and save result
    let coons =
        BSplineSurface::create_coons_patch(tol, create_bilinear_patch, &mut bounds, &mut tans)?;

    // Not upstream (WR-547; source fork@cae8618): the patch tangent to its sides'
    // surfaces
    if k.fixes.corner_blend_coons_patch_tangent_to_sides && !create_bilinear_patch {
        if let Some(tangent) =
            make_coons_patch_tangent_to_sides(k, &coons, &uv_crvs, surfaces, tangency_tol_radians)?
        {
            return Ok(vec![tangent]);
        }
    }
    Ok(vec![coons])
}

// ---- the fork's Coons patch tangency (source fork@cae8618) ----------------------

/// The fork's `sm_CoonsSideNormal`: the unit normal of `surface` along a Coons patch's
/// side, at patch parameter `s` along that side (`patch_ivl`: the side's interval in
/// the patch's parameters), found through the side's UV curve.
fn coons_side_normal(
    k: &crate::kernel::Kernel,
    uv_curve: &BSplineCurve,
    surface: &SmSurface,
    patch_ivl: &Extent1d,
    s: f64,
) -> Result<Vec3> {
    let ivl = uv_curve.natural_interval();
    let t = ivl.evaluate((s - patch_ivl.min()) / patch_ivl.length());
    let uv = uv_curve.evaluate_point(t)?;
    evaluate_normal(k, surface, Point2::new(uv.x, uv.y))
}

/// Side `ii`'s UV curve and surface, `None` when either is missing.
fn coons_side<'a>(
    uv_curves: &'a [Option<BSplineCurve>],
    surfaces: &[Option<&'a SmSurface>],
    ii: usize,
) -> Option<(&'a BSplineCurve, &'a SmSurface)> {
    let uv = uv_curves.get(ii)?.as_ref()?;
    let srf = (*surfaces.get(ii)?)?;
    Some((uv, srf))
}

/// The fork's `sm_CoonsSideAngle`: the largest angle (radians) between a Coons patch's
/// normal and the normals of the surfaces along its sides, at four stations a knot
/// span (33 at least). Sides are ordered left (u min), bottom (v min), right (u max),
/// top (v max); a side with no surface is skipped, and so is a station where the
/// patch has no normal (a degenerate side's point).
fn coons_side_angle(
    k: &crate::kernel::Kernel,
    patch: &BSplineSurface,
    uv_curves: &[Option<BSplineCurve>],
    surfaces: &[Option<&SmSurface>],
) -> Result<f64> {
    let mut max_angle = 0.0;
    let domain = patch.natural_uv_domain();
    let su = Extent1d::from_min_max(domain.min.x, domain.max.x);
    let sv = Extent1d::from_min_max(domain.min.y, domain.max.y);
    for ii in 0..4 {
        let Some((uv_curve, surface)) = coons_side(uv_curves, surfaces, ii) else {
            continue;
        };
        let along_u = ii == BOT || ii == TOP;
        let along = if along_u { &su } else { &sv };
        let dir = if along_u {
            SurfParamType::U
        } else {
            SurfParamType::V
        };
        // four stations a knot span, so a miss between control points is seen
        let spans = patch
            .number_control_points(dir)
            .saturating_sub(patch.degree(dir));
        let stations = (4 * spans + 1).max(33);
        for kk in 0..stations {
            let s = along.evaluate(kk as f64 / (stations - 1) as f64);
            let uv = match ii {
                LEFT => Point2::new(su.min(), s),
                BOT => Point2::new(s, sv.min()),
                RIGHT => Point2::new(su.max(), s),
                _ => Point2::new(s, sv.max()),
            };
            let side = |below_max: bool| if below_max { Side::Left } else { Side::Right };
            let (_, du, dv) = crate::surface::surface::p1::evaluate_1st_derivatives(
                k,
                patch,
                uv,
                side(uv.x < su.max()),
                side(uv.y < sv.max()),
            )?;
            let normal = du.cross(dv);
            let scale = du.length() * dv.length();
            if scale < EFF_ZERO_SQ || normal.length() <= EFF_ZERO_SQRT * scale {
                continue;
            }
            let normal = normal.unitize()?;
            let side_normal = coons_side_normal(k, uv_curve, surface, along, s)?;
            let cos = normal.dot(side_normal).abs();
            let angle = acos(if cos < 1.0 { cos } else { 1.0 });
            if angle > max_angle {
                max_angle = angle;
            }
        }
    }
    Ok(max_angle)
}

/// The fork's `sm_RefineToSpans`: knots inserted in `param`'s direction until no span
/// is longer than `1 / spans` of the domain (the surface's shape is unchanged).
fn refine_to_spans(surface: &mut BSplineSurface, param: SurfParamType, spans: usize) -> Result<()> {
    const OP: &str = OP_CORNER;
    let (knots, _) = surface.knots(param, None)?;
    let (Some(&first), Some(&last)) = (knots.first(), knots.last()) else {
        return Err(SolidError::sm_err(OP));
    };
    if knots.len() < 2 {
        return Err(SolidError::sm_err(OP));
    }
    let max_span = (last - first) / spans as f64;
    let mut new_knots = Vec::new();
    for pair in knots.windows(2) {
        let [lo, hi] = pair else {
            continue;
        };
        let length = hi - lo;
        let pieces = (length / max_span - 1.0e-9).ceil() as usize;
        for kk in 1..pieces {
            new_knots.push(lo + length * kk as f64 / pieces as f64);
        }
    }
    if !new_knots.is_empty() {
        surface.insert_knots(param, &new_knots)?;
    }
    Ok(())
}

/// The fork's `sm_ProjectCoonsSideRows`: each control point next to a side of a Coons
/// patch moved, by the least distance, into the tangent plane of that side's surface at
/// the point's Greville station along the side; a point next to two sides (at a
/// corner) onto the mean of their planes. Needs at least 5 control points each way, so
/// that no point is next to two opposite sides. Every plane is read from the unmoved
/// net before any point is written.
fn project_coons_side_rows(
    k: &crate::kernel::Kernel,
    patch: &mut BSplineSurface,
    uv_curves: &[Option<BSplineCurve>],
    surfaces: &[Option<&SmSurface>],
) -> Result<()> {
    const OP: &str = OP_CORNER;
    let nu = patch.number_control_points(SurfParamType::U);
    let nv = patch.number_control_points(SurfParamType::V);
    let pu = patch.degree(SurfParamType::U);
    let pv = patch.degree(SurfParamType::V);
    if nu < 5 || nv < 5 || pu < 1 || pv < 1 {
        return Err(SolidError::sm_err(OP));
    }
    let nurb = patch.gw_nurb().ok_or_else(|| SolidError::sm_err(OP))?;
    let (ku, kv) = (nurb.u_knots().to_vec(), nurb.v_knots().to_vec());
    if ku.len() != nu + pu + 1 || kv.len() != nv + pv + 1 {
        return Err(SolidError::sm_err(OP));
    }
    let (Some(&ku_first), Some(&ku_last), Some(&kv_first), Some(&kv_last)) =
        (ku.first(), ku.last(), kv.first(), kv.last())
    else {
        return Err(SolidError::sm_err(OP));
    };
    let su = Extent1d::from_min_max(ku_first, ku_last);
    let sv = Extent1d::from_min_max(kv_first, kv_last);

    // the Greville station of control point `index` in one direction
    let greville = |knots: &[f64], degree: usize, index: usize| -> Result<f64> {
        let mut sum = 0.0;
        for kk in 1..=degree {
            sum += at(knots, index + kk, OP)?;
        }
        Ok(sum / degree as f64)
    };

    let mut moved: Vec<(usize, usize, Point3)> = Vec::new();
    for i in 1..nu - 1 {
        for j in 1..nv - 1 {
            // the sides this control point is next to: (side, anchor u, anchor v)
            let mut sides: Vec<(usize, usize, usize)> = Vec::with_capacity(2);
            if i == 1 {
                sides.push((LEFT, 0, j));
            }
            if i == nu - 2 {
                sides.push((RIGHT, nu - 1, j));
            }
            if j == 1 {
                sides.push((BOT, i, 0));
            }
            if j == nv - 2 && sides.len() < 2 {
                sides.push((TOP, i, nv - 1));
            }
            if sides.is_empty() {
                continue;
            }

            let (mut pnt, _) = patch.control_point(ControlPointForm::NonRational, i, j)?;

            // each side's plane: unit normal and the signed distance of pnt from it
            let mut planes: Vec<(Vec3, f64)> = Vec::with_capacity(2);
            for &(side, anchor_u, anchor_v) in &sides {
                let Some((uv_curve, surface)) = coons_side(uv_curves, surfaces, side) else {
                    continue;
                };
                let along_u = side == BOT || side == TOP;
                let s = if along_u {
                    greville(&ku, pu, i)?
                } else {
                    greville(&kv, pv, j)?
                };
                let (anchor, _) =
                    patch.control_point(ControlPointForm::NonRational, anchor_u, anchor_v)?;
                let normal =
                    coons_side_normal(k, uv_curve, surface, if along_u { &su } else { &sv }, s)?;
                planes.push((normal, normal.dot(pnt - anchor)));
            }
            match planes.as_slice() {
                [] => continue,
                [(normal, dist)] => pnt -= *normal * *dist,
                [(normal0, dist0), (normal1, dist1), ..] => {
                    // next to two sides at a corner: both planes are near the corner's
                    // tangent plane, their line far off; onto their mean plane, which
                    // leaves each a second-order miss
                    let mean = if normal0.dot(*normal1) < 0.0 {
                        *normal0 - *normal1
                    } else {
                        *normal0 + *normal1
                    };
                    let Ok(mean) = mean.unitize() else {
                        continue;
                    };
                    let anchor0 = pnt - *normal0 * *dist0;
                    let anchor1 = pnt - *normal1 * *dist1;
                    let dist = 0.5 * (mean.dot(pnt - anchor0) + mean.dot(pnt - anchor1));
                    pnt -= mean * dist;
                }
            }
            moved.push((i, j, pnt));
        }
    }

    // write the moved points once every plane was read from the unmoved net
    for (i, j, pnt) in moved {
        patch.set_control_point(ControlPointForm::NonRational, i, j, pnt, 1.0)?;
    }
    Ok(())
}

/// The fork's `sm_MakeCoonsPatchTangentToSides`: a Coons corner patch tangent to the
/// surfaces along its sides, to within `SM_CONTINUITY_ANGLE` (or `tangency_tol_radians`
/// where smaller). A bicubic Coons patch meets its cross-boundary derivative curves
/// only where their twists agree at the corners; where two rounds of different
/// curvature meet, `sm_CreateCoonsSrf` takes the mean twist and the patch leaves both
/// rounds at an angle. A copy of the patch is refined to 8, 16, 32 and 64 spans each
/// way and the control points next to each side moved into the side's tangent planes,
/// until every side reads within tolerance. Returns the first copy within it, or else
/// the one nearest it if that is nearer than `patch`; `None` when `patch` is rational,
/// within tolerance already, or no copy is nearer.
fn make_coons_patch_tangent_to_sides(
    k: &crate::kernel::Kernel,
    patch: &BSplineSurface,
    uv_curves: &[Option<BSplineCurve>],
    surfaces: &[Option<&SmSurface>],
    tangency_tol_radians: f64,
) -> Result<Option<BSplineSurface>> {
    if patch.is_rational() {
        return Ok(None);
    }
    // a seam SMLib calls tangent (SmEdgeuse::IsTangentSector) is within
    // SM_CONTINUITY_ANGLE
    let mut target = ANG_TOL_DEG * (PI / 180.0);
    if tangency_tol_radians < target {
        target = tangency_tol_radians;
    }
    let mut best_angle = coons_side_angle(k, patch, uv_curves, surfaces)?;
    let mut best = None;
    let mut spans = 8;
    while spans <= 64 && best_angle > target {
        let mut attempt = patch.clone();
        refine_to_spans(&mut attempt, SurfParamType::U, spans)?;
        refine_to_spans(&mut attempt, SurfParamType::V, spans)?;
        project_coons_side_rows(k, &mut attempt, uv_curves, surfaces)?;
        let angle = coons_side_angle(k, &attempt, uv_curves, surfaces)?;
        if angle < best_angle {
            best_angle = angle;
            best = Some(attempt);
        }
        spans *= 2;
    }
    Ok(best)
}

// ---- CreateCornerBlend: n curves --------------------------------------------------------

/// The n-sided branch of `CreateCornerBlend`.
fn corner_blend_n(
    k: &crate::kernel::Kernel,
    mut boundaries: Vec<BSplineCurve>,
    mut uv_crvs: Vec<Option<BSplineCurve>>,
    curve_orients: &[OrientType],
    surfaces: &[Option<&SmSurface>],
    approx_tol: f64,
    tangency_tol_radians: f64,
) -> Result<Vec<BSplineSurface>> {
    const OP: &str = OP_CORNER;
    let missing = || SolidError::invalid_input(OP, "missing UV curve or surface");
    let oob = || SolidError::invalid_input(OP, "curve index out of range");
    let mut negate_curr_normal = false;
    let mut negate_tan_curves: Option<bool> = None; // UNSURE

    // for every boundary - make all curves SM_OT_SAME
    for (ii, &orient) in curve_orients.iter().enumerate() {
        if orient == OrientType::Opposite {
            let b = boundaries.get_mut(ii).ok_or_else(oob)?;
            let ivl1 = b.natural_interval();
            b.reverse_parameterization(&ivl1)?;

            if let Some(uv) = uv_crvs.get_mut(ii).and_then(Option::as_mut) {
                let ivl2 = uv.natural_interval();
                uv.reverse_parameterization(&ivl2)?;
            }
        }
    }

    // Now Create CrossTangent Curves
    let cnt = boundaries.len();
    let mut tan_curves: Vec<BSplineCurve> = Vec::with_capacity(cnt);
    for ii in 0..cnt {
        let prev_i = if ii == 0 { cnt - 1 } else { ii - 1 };
        let next_i = if ii == cnt - 1 { 0 } else { ii + 1 };

        let curr = boundaries.get(ii).ok_or_else(oob)?;
        let prev = boundaries.get(prev_i).ok_or_else(oob)?;
        let next = boundaries.get(next_i).ok_or_else(oob)?;

        let uv_of = |i: usize| uv_crvs.get(i).and_then(Option::as_ref).ok_or_else(missing);
        let srf_of = |i: usize| -> Result<&SmSurface> { at(surfaces, i, OP)?.ok_or_else(missing) };
        let curr_uv = uv_of(ii)?;
        let prev_uv = uv_of(prev_i)?;
        let next_uv = uv_of(next_i)?;
        let curr_srf = srf_of(ii)?;
        let prev_srf = srf_of(prev_i)?;
        let next_srf = srf_of(next_i)?;

        let prev_end_param = prev_uv.natural_interval().max();
        let curr_start_param = curr_uv.natural_interval().min();
        let curr_end_param = curr_uv.natural_interval().max();
        let next_start_param = next_uv.natural_interval().min();

        // 3d Boundary end Curve Points and tangents (statuses ignored upstream)
        let undef = || vec![Vec3::UNDEF; 2];
        let prev_end_pv = prev
            .evaluate(prev_end_param, 1, true, false)
            .unwrap_or_else(|_| undef());
        let curr_start_pv = curr
            .evaluate(curr_start_param, 1, true, false)
            .unwrap_or_else(|_| undef());
        let curr_end_pv = curr
            .evaluate(curr_end_param, 1, true, false)
            .unwrap_or_else(|_| undef());
        let next_start_pv = next
            .evaluate(next_start_param, 1, true, false)
            .unwrap_or_else(|_| undef());

        // 2d Boundary end UV Points
        let uv2 = |c: &BSplineCurve, t: f64| {
            let p = c.evaluate_point(t).unwrap_or(Vec3::UNDEF);
            Point2::new(p.x, p.y)
        };
        let prev_end_uv = uv2(prev_uv, prev_end_param);
        let curr_start_uv = uv2(curr_uv, curr_start_param);
        let curr_end_uv = uv2(curr_uv, curr_end_param);
        let next_start_uv = uv2(next_uv, next_start_param);

        // Surface Normals at boundary endPoints
        let mut prev_end_normal = evaluate_normal(k, prev_srf, prev_end_uv)?;
        let mut curr_start_normal = evaluate_normal(k, curr_srf, curr_start_uv)?;
        let mut curr_end_normal = evaluate_normal(k, curr_srf, curr_end_uv)?;
        let mut next_start_normal = evaluate_normal(k, next_srf, next_start_uv)?;

        // compare the neighbor and current surface normal directions
        let prev_dot_curr_normal = prev_end_normal.dot(curr_start_normal);
        let next_dot_curr_normal = next_start_normal.dot(curr_end_normal);

        // remember the neighbor to curr normal relationships
        let prev_normal_same = prev_dot_curr_normal > 0.0;
        let next_normal_same = next_dot_curr_normal > 0.0;

        // flip surface normals as needed
        if (negate_curr_normal && prev_normal_same) || (!negate_curr_normal && !prev_normal_same) {
            prev_end_normal = -prev_end_normal;
        }
        if negate_curr_normal {
            curr_start_normal = -curr_start_normal;
        }
        if negate_curr_normal {
            curr_end_normal = -curr_end_normal;
        }
        if (negate_curr_normal && next_normal_same) || (!negate_curr_normal && !next_normal_same) {
            next_start_normal = -next_start_normal;
        }

        // get ready for next iteration
        negate_curr_normal =
            (negate_curr_normal && next_normal_same) || (!negate_curr_normal && !next_normal_same);

        let pv1 = |pv: &[Vec3]| pv.get(1).copied().unwrap_or(Vec3::UNDEF);
        let pv0 = |pv: &[Vec3]| pv.first().copied().unwrap_or(Vec3::UNDEF);

        // Let CrossVec be the direction out of the new patch
        let prev_end_cross_vec = prev_end_normal.cross(pv1(&prev_end_pv));
        let curr_start_cross_vec = curr_start_normal.cross(pv1(&curr_start_pv));
        let curr_end_cross_vec = curr_end_normal.cross(pv1(&curr_end_pv));
        let next_start_cross_vec = next_start_normal.cross(pv1(&next_start_pv));

        // see if the first assumption about surface normals can be corrected by a
        // boundary surface case
        if negate_tan_curves.is_none() {
            // classify the ends of the curve against the surface UVDomain
            let curr_uv_domain = curr_srf.natural_uv_domain_with_nurb(k)?;
            let tol_uv = curr_uv_domain.size() / 1000.0;
            let tol = min2(tol_uv.x, tol_uv.y);
            let start_bndries = curr_uv_domain.point_boundaries(curr_start_uv, tol);
            let end_bndries = curr_uv_domain.point_boundaries(curr_end_uv, tol);
            let bndries = start_bndries & end_bndries;

            // when pCurr starts and stop on a common boundary - check orientation
            if bndries != 0 {
                // Surface 1st derivs at CurveStart
                let (srf_du, srf_dv) = evaluate_1st_derivatives(k, curr_srf, curr_start_uv)?;

                // pick the appropriate vector and make it point out of the patch
                let outward = if bndries & SS_UMIN != 0 {
                    srf_du
                } else if bndries & SS_VMIN != 0 {
                    srf_dv
                } else if bndries & SS_UMAX != 0 {
                    -srf_du
                } else {
                    -srf_dv
                };

                let cross_vec_dot_outward = curr_start_cross_vec.dot(outward);
                negate_tan_curves = Some(cross_vec_dot_outward < 0.0);
            }
        }

        // Surface CrossTangents at boundary endPoints
        let prev_end_cross_tang =
            evaluate_normal_section(k, prev_srf, prev_end_uv, prev_end_cross_vec)?;
        let curr_start_cross_tang =
            evaluate_normal_section(k, curr_srf, curr_start_uv, curr_start_cross_vec)?;
        let curr_end_cross_tang =
            evaluate_normal_section(k, curr_srf, curr_end_uv, curr_end_cross_vec)?;
        let next_start_cross_tang =
            evaluate_normal_section(k, next_srf, next_start_uv, next_start_cross_vec)?;

        // check boundary curve gap sizes
        let dist_prev = pv0(&prev_end_pv).distance(pv0(&curr_start_pv));
        let dist_next = pv0(&curr_end_pv).distance(pv0(&next_start_pv));
        if dist_prev > approx_tol || dist_next > approx_tol {
            return Err(SolidError::sm_err(OP)); // Ends don't match
        }

        // get the desired tangentField endTangent vecs
        let start_tf = (prev_end_cross_tang + curr_start_cross_tang) / 2.0;
        let end_tf = (curr_end_cross_tang + next_start_cross_tang) / 2.0;

        // use pCurr knot vector and build a curve approximation to the tangent field
        let knots = unique_knots(curr);
        let tan_curve = interpolated_tangent_field_curve(
            k,
            curr_uv,
            curr_srf,
            start_tf,
            end_tf,
            &knots,
            tangency_tol_radians,
        )?;
        tan_curves.push(tan_curve);
    }

    // When we've figured out that the crossTangent vectors need to be negated
    if negate_tan_curves == Some(true) {
        for tan in &mut tan_curves {
            let _ = tan.scale(-1.0);
        }
    }

    // pass the call along to create the NSidedPatch
    let mut ders: Vec<Option<BSplineCurve>> = tan_curves.into_iter().map(Some).collect();
    BSplineSurface::create_n_sided_patch_with(
        &mut boundaries,
        &mut ders,
        tangency_tol_radians,
        None,
        k.fixes.corner_blend_stays_inside_neighbours,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{CPoint, NOW};

    /// A bilinear-ish patch over `[0, 1]^2` of degree `(p, q)`.
    fn plane(p: usize, q: usize, z: impl Fn(usize, usize) -> f64) -> BSplineSurface {
        let n = p;
        let m = q;
        let net: Vec<Vec<CPoint>> = (0..=n)
            .map(|i| {
                (0..=m)
                    .map(|j| CPoint::new(i as f64 / n as f64, j as f64 / m as f64, z(i, j), NOW))
                    .collect()
            })
            .collect();
        let mut u = vec![0.0; p + 1];
        u.extend(vec![1.0; p + 1]);
        let mut v = vec![0.0; q + 1];
        v.extend(vec![1.0; q + 1]);
        let s = Surface::new(net, p, q, u, v).unwrap();
        BSplineSurface::from_nurb(s, false)
    }

    #[test]
    fn evaluate_matches_simple_without_zero_tangents() {
        let s = plane(2, 2, |i, j| (i * j) as f64 * 0.3);
        let uv = Point2::new(0.3, 0.6);
        let a = s.evaluate(uv, 2, 2, EvaluateFlags::default()).unwrap();
        let b = s.evaluate_simple(uv, 2, 2, true, true, false).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn evaluate_rejects_bad_orders() {
        let s = plane(1, 1, |_, _| 0.0);
        let uv = Point2::new(0.5, 0.5);
        let flags = EvaluateFlags {
            only_upper_half: true,
            ..EvaluateFlags::default()
        };
        assert!(matches!(
            s.evaluate(uv, 1, 2, flags),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            s.evaluate(uv, 5, 0, EvaluateFlags::default()),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn evaluate_fixes_degenerate_side() {
        // the u = 0 row collapses to a point: Sv = 0 there
        let mut s = plane(2, 2, |_, _| 0.0);
        if let Some(sur) = s.nurb.as_mut() {
            for j in 0..=2 {
                *sur.cpt_mut(0, j).unwrap() = CPoint::new(0.0, 0.5, 0.0, NOW);
            }
        }
        let uv = Point2::new(0.0, 0.5);
        let d = s.evaluate(uv, 1, 1, EvaluateFlags::default()).unwrap();
        assert!(d[0][1].length() > 0.0);
        let raw = s.evaluate_simple(uv, 1, 1, true, true, false).unwrap();
        assert_eq!(raw[0][1].length(), 0.0);
    }

    #[test]
    fn trim_with_domain_extracts_patch() {
        let mut s = plane(2, 3, |i, j| (i + j) as f64 * 0.1);
        let mut dom = Extent2d::from_bounds(0.25, 0.1, 0.75, 0.9);
        s.trim_with_domain(&mut dom).unwrap();
        let nd = s.natural_uv_domain();
        assert!((nd.min().x - 0.25).abs() < 1e-12);
        assert!((nd.max().y - 0.9).abs() < 1e-12);
        // outside the domain fails
        let mut bad = Extent2d::from_bounds(-1.0, 0.0, 0.5, 0.5);
        assert!(s.trim_with_domain(&mut bad).is_err());
    }

    /// A cubic-by-linear surface with u knots `u` (clamped) and v knots `v`, its
    /// control points on z = 0.
    fn net_with_knots(u: &[f64], v: &[f64]) -> BSplineSurface {
        let (p, q) = (3, 1);
        let nu = u.len() - p - 1;
        let nv = v.len() - q - 1;
        let net: Vec<Vec<CPoint>> = (0..nu)
            .map(|i| {
                (0..nv)
                    .map(|j| CPoint::new(i as f64, j as f64, 0.0, NOW))
                    .collect()
            })
            .collect();
        let s = Surface::new(net, p, q, u.to_vec(), v.to_vec()).unwrap();
        BSplineSurface::from_nurb(s, false)
    }

    fn u_knots(s: &BSplineSurface) -> Vec<f64> {
        s.knots(SurfParamType::U, None).unwrap().0
    }

    fn v_knots(s: &BSplineSurface) -> Vec<f64> {
        s.knots(SurfParamType::V, None).unwrap().0
    }

    /// source issue #472: the round's u knots and face domains of the
    /// (+x,-y) and (-x,+y) rounds of a drafted box (their domains differ by one ULP).
    #[test]
    fn trim_one_ulp_inside_a_knot_leaves_a_sliver_upstream_and_none_with_snapping() {
        let k = 20.152507010543033_f64;
        let (k2, end) = (60.45751479, 80.61001657);
        let u = [0.0, 0.0, 0.0, 0.0, k, k, k, k2, k2, k2, end, end, end, end];
        let v = [0.0, 0.0, 0.5, 1.0, 1.0];
        let below = f64::from_bits(k.to_bits() - 1);
        assert!(below < k && k - below < 4e-15);
        let ur = 60.601175783809218;

        // on the knot: no sliver
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(k, 0.0, ur, 1.0);
        s.trim_with_domain(&mut dom).unwrap();
        assert_eq!(u_knots(&s), vec![k, k2, ur]);
        assert_eq!(dom.min().x, k);

        // one ULP inside the knot: upstream moves the boundary 10 * 1e-6 * (1 + u) away
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(below, 0.0, ur, 1.0);
        s.trim_with_domain(&mut dom).unwrap();
        let sliver = k - 10.0 * EFF_ZERO_SQRT * (1.0 + below.abs());
        assert_eq!(u_knots(&s), vec![sliver, k, k2, ur]);
        assert!((k - sliver - 2.115e-4).abs() < 1e-6);

        // with snapping: the boundary goes onto the knot, as for the face on it
        let snap = TrimOptions {
            snap_roundoff_to_knots: true,
        };
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(below, 0.0, ur, 1.0);
        s.trim_with_domain_with(&mut dom, snap).unwrap();
        assert_eq!(u_knots(&s), vec![k, k2, ur]);
        assert_eq!(dom.min().x, k);
        assert_eq!(s.natural_uv_domain().min().x, k);
    }

    #[test]
    fn snapping_covers_upper_and_v_boundaries() {
        let u = [0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 5.0, 5.0, 5.0, 5.0];
        let v = [0.0, 0.0, 0.5, 1.0, 1.0];
        let snap = TrimOptions {
            snap_roundoff_to_knots: true,
        };
        let above = 2.0 + 1e-12;
        let v_above = 0.5 + 1e-13;

        // upstream: an upper boundary just outside a knot is moved further out
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(0.5, 0.0, above, v_above);
        s.trim_with_domain(&mut dom).unwrap();
        assert_eq!(dom.max().x, 2.0 + 10.0 * EFF_ZERO_SQRT * (1.0 + above));
        assert_eq!(dom.max().y, 0.5 + 10.0 * EFF_ZERO_SQRT * (1.0 + v_above));
        assert_eq!(u_knots(&s).len(), 3);

        // with snapping both go onto their knots
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(0.5, 0.0, above, v_above);
        s.trim_with_domain_with(&mut dom, snap).unwrap();
        assert_eq!((dom.max().x, dom.max().y), (2.0, 0.5));
        assert_eq!(u_knots(&s), vec![0.5, 2.0]);
        assert_eq!(v_knots(&s), vec![0.0, 0.5]);

        // lower v boundary just inside a knot
        let mut s = net_with_knots(&u, &v);
        let mut dom = Extent2d::from_bounds(0.5, 0.5 - 1e-13, 4.0, 1.0);
        s.trim_with_domain_with(&mut dom, snap).unwrap();
        assert_eq!(dom.min().y, 0.5);
        assert_eq!(v_knots(&s), vec![0.5, 1.0]);
    }

    #[test]
    fn snapping_leaves_boundaries_beyond_eff_zero_param_to_upstream() {
        let u = [0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 5.0, 5.0, 5.0, 5.0];
        let v = [0.0, 0.0, 1.0, 1.0];
        let snap = TrimOptions {
            snap_roundoff_to_knots: true,
        };
        for ul in [2.0 - 3e-8, 2.0 - 1e-6, 2.0 + 1e-9, 1.0] {
            let mut a = net_with_knots(&u, &v);
            let mut b = a.clone();
            let mut da = Extent2d::from_bounds(ul, 0.0, 4.0, 1.0);
            let mut db = da;
            a.trim_with_domain(&mut da).unwrap();
            b.trim_with_domain_with(&mut db, snap).unwrap();
            assert_eq!(da, db, "ul = {ul}");
            assert_eq!(u_knots(&a), u_knots(&b), "ul = {ul}");
        }
    }

    #[test]
    fn corner_blend_needs_four_curves() {
        let k = &crate::Kernel::default();
        let r = BSplineSurface::create_corner_blend(k, &[], &[], &[], &[], 1e-3, 0.1, true);
        assert!(r.is_err());
    }

    #[test]
    fn skinned_surface_needs_sections() {
        let k = &crate::Kernel::default();
        let r = BSplineSurface::create_skinned_surface(k, &[], SkinnedSurfaceOptions::default());
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }
}
