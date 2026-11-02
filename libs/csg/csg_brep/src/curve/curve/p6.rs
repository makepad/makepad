//! `SmCurve`, part 6: the sampled bounding boxes of the base class and the
//! `CalculateBoundingBox` dispatch, the planarity test, dropping a point onto a
//! curve, the STEP point solve dispatch, the largest gap between two curves, the
//! simple and global curve/curve coincidence checkers, and deforming a curve to new
//! end point targets.
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                                    | Upstream                              |
//! |---------------------------------------------------------|---------------------------------------|
//! | [`Curve::calculate_bounding_box`], [`calculate_bounding_box_base`] | `SmCurve::CalculateBoundingBox` (and its dispatch, with the `SmCrvOnSurf` override) |
//! | [`Curve::is_planar`]                                    | `SmCurve::IsPlanar`                   |
//! | [`Curve::drop_point`], [`Curve::drop_point_base`], [`DroppedPoint`] | `DropPoint` (virtual: the `SmCrvOnSurf` override), `SmCurve::DropPoint` |
//! | [`Curve::global_point_solve_step`]                      | `SmCurve::GlobalPointSolveSTEP` (and its dispatch) |
//! | [`Curve::curve_max_distance_between`], [`MaxDistance`]  | `SmCurve::CurveMaxDistanceBetween`    |
//! | [`Curve::simple_coincidence_checker`]                   | `SmCurve::SimpleCoincidenceChecker`   |
//! | [`Curve::global_coincidence_checker`], [`Curve::global_coincidence_checker_with_partial`], [`Coincidence`]  | `SmCurve::GlobalCoincidenceChecker`   |
//! | [`Curve::deform_to_end_point_targets`]                  | `SmCurve::DeformToEndPointTargets`    |
//!
//! Deviations: endpoint deformation rejects non-B-spline UV curves with
//! `InvalidInput`; ordinary UV-drop errors use `Vec2::UNDEF` rather than preserving
//! partially written native out-params; UV replacement can fail on the Rust-only
//! evaluation-copy depth guard. Other deviations:
//! - **Out-params.** Every function returns its outputs ([`DroppedPoint`],
//!   [`MaxDistance`], [`Coincidence`], the solution list of
//!   [`Curve::simple_coincidence_checker`], the new curve of
//!   [`Curve::deform_to_end_point_targets`]). `CurveMaxDistanceBetween` leaves
//!   `pbOptIncorrectDir` untouched on its early returns: [`MaxDistance::incorrect_dir`]
//!   is then `None`. Solutions name the caller's handles of the two curves
//!   (`objects`), standing for `this` / `&crOtherCurve`; solution fields upstream does
//!   not write keep their [`Solution::new`] values.
//! - **Boxes.** The `CalculateBoundingBox` dispatch takes a [`BoundingBoxRequest`]
//!   and returns fresh [`BoundingBoxes`] (the overrides of the B-spline, hyperbola
//!   (its B-spline base), line and Hermite kinds start from default-constructed
//!   boxes, which is what every caller in this unit passes).
//! - **Ignored statuses.** `IsPlanar` ignores the status of `CalculateBoundingBox`
//!   (a failure leaves the default pseudo box); the base `CalculateBoundingBox`
//!   ignores its evaluations (a failure keeps the previous values, initially
//!   [`Vec3::UNDEF`]), `SetMinMax` and `SetBasis`; `CurveMaxDistanceBetween`
//!   ignores `GetKnots`. [`SolidError::Unported`] is always propagated.
//! - **Bad `DropPoint` operation.** Upstream warns and uses `SM_SO_MINIMIZE`; so does
//!   the port (without the warning).
//! - **Object identity.** `&crOtherCurve == this` is [`core::ptr::eq`].
//! - **`DeformToEndPointTargets`.** The legacy entry point omits the optional
//!   surface; [`Curve::deform_to_end_point_targets_on_surface`] retains it and the
//!   optional orientation. Matching surfaces edit the UV B-spline in place,
//!   replacing derived analytic UV curves with their deformed B-spline.
//! - **Handles.** The global point solves of `DropPoint` and `CurveMaxDistanceBetween`
//!   name a private handle of the curve (with its natural interval, as upstream's
//!   `this`); those solutions are not returned.
//! - **`ApproximateCurve`** in `DeformToEndPointTargets`: upstream ignores its status
//!   and dereferences the new curve; a failure is an error here (an approximation
//!   that is not a B-spline is `InvalidInput`).
//! - **Curve caches.** Upstream's `DropPoint` reaches `GlobalPointSolve`, which reads
//!   the cache kept on the curve (`GetOrCreateObjectCache`); a port without one
//!   rebuilds it for every drop. `GlobalCoincidenceChecker` (four drops per call,
//!   tripled per split) keeps the two curves' caches for its whole recursion instead
//!   (`Curve::global_point_solve_cached`), built at the first global solve as
//!   upstream's; the build is deterministic, so the solutions are those of a fresh
//!   cache. [`Curve::drop_point_in`] keeps the caches of a caller's curves in a
//!   [`DropPointCaches`] (`IsPointOnCurve` in `FindInteriorCurves`). Other
//!   `DropPoint` callers still build one per call.
//! - **Recursion.** `GlobalCoincidenceChecker` splits closed and half closed curves
//!   recursively; past [`MAX_COINCIDENCE_DEPTH`] nested splits it fails with
//!   `InvalidInput` (upstream recurses without bound). Each split makes three calls, so
//!   the calls grow as `3^depth` well before that depth: past
//!   [`MAX_COINCIDENCE_CALLS`] calls in one check it fails with `InvalidInput` too
//!   (`1e300` control points split 15 to 30 levels deep and ran for minutes, fuzz
//!   `curve.curve_p8` fuzz01456; upstream fixtures and the committed difftest cases
//!   make at most 15,139 calls and split 32 levels deep).
//! - **Seam test.** `sm_IsPointSolutionAcrossClosedSeamBoundary` (unit
//!   `curve.curve_p5`) is told whether the solution's first object is its `this`
//!   curve: the split solutions name `[this, other]`, so it is when the coincidence
//!   check split the other curve, or `this` and the other are the same curve.

use crate::curve::curve::knots_ignoring_status;
use crate::curve::curve::p1::{ApproximateCurveOptions, GlobalPointSolveOptions};
use crate::curve::curve::p2::{
    at, snap_sol_to_specified_endpoint, Curve, CurveKind, CurvePropertyType,
};
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::p5::{
    is_point_solution_across_closed_seam_boundary, local_property_analysis,
};
use crate::curve::curve_cache::CurveCache;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{
    GlobalSolve, GlobalSolver, PreferredSolution, SolutionPairType, SolutionRequested,
    SolverOperation,
};
use crate::geom::object::ObjectRef;
use crate::geom::polar_box::{PolarBox, PolarBoxState};
use crate::geom::pseudo_box::{Planar, PseudoBox};
use crate::geom::solution_array::{
    ObjectKind, ParameterDomains, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::{approx_tol_3d, are_same, is_zero, SmallTopoType, ZONE_TOL_3D};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest, BoundingBoxes};
use crate::nurbs::b_spline_curve_n_lib::InValueType;

const OP_BOX: &str = "curve::curve::p6::calculate_bounding_box";
const OP_DEFORM: &str = "curve::curve::p6::deform_to_end_point_targets";

const OP_COINCIDENCE: &str = "curve::curve::p6::global_coincidence_checker";

/// Deepest nesting of the closed / half closed splits of
/// [`Curve::global_coincidence_checker`] (each split shrinks a parameter interval to at
/// most 37.5% and stops below a tenth of the zone tolerance, so upstream inputs need
/// fewer than 40 levels).
pub const MAX_COINCIDENCE_DEPTH: usize = 200;

/// Most calls (the first and every split piece) one
/// [`Curve::global_coincidence_checker`] makes (not upstream; see the module
/// deviations): `2^18`, 17 times the most any upstream fixture or committed difftest
/// case makes (15,139).
pub const MAX_COINCIDENCE_CALLS: usize = 1 << 18;

/// `lNumSpans` of the base `CalculateBoundingBox`.
const NUM_SPANS: u64 = 12;

// ---- callees ----------------------------------------------------------------------------

/// The handle of a curve inside this unit's own solves (upstream `this`): a curve with
/// its natural interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// The two curves of [`Curve::global_coincidence_checker`] as its merged solutions name
/// them (upstream's `this` and `crOtherCurve`): each handle's natural interval is its
/// curve's (a handle's own answer first).
struct CurvePair<O> {
    objects: [Option<O>; 2],
    intervals: [Extent1d; 2],
}

impl<O: SolutionObject> ParameterDomains<O> for CurvePair<O> {
    fn natural_interval(&self, curve: &O) -> Option<Extent1d> {
        curve.natural_interval().or_else(|| {
            self.objects
                .iter()
                .zip(&self.intervals)
                .find_map(|(h, ivl)| (h.as_ref() == Some(curve)).then_some(*ivl))
        })
    }
}

/// The `SmGlobalSolver` merging the split parts' solutions, its `IdenticalSolutions`
/// (not overridden upstream) reading the curves' natural intervals ([`CurvePair`]).
struct MergeSolver<'t, O> {
    base: GlobalSolver<'t, O>,
    domains: CurvePair<O>,
}

impl<'t, O: SolutionObject + 't> GlobalSolve<'t, O> for MergeSolver<'t, O> {
    fn solver(&self) -> &GlobalSolver<'t, O> {
        &self.base
    }

    fn solver_mut(&mut self) -> &mut GlobalSolver<'t, O> {
        &mut self.base
    }

    fn identical_solutions(
        &self,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        self.base.identical_solutions_in(sol1, sol2, &self.domains)
    }
}

/// The curve caches of [`Curve::drop_point_in`], kept across calls on the same
/// curves (performance only: upstream keeps each cache on its curve; see
/// [`Curve::global_point_solve_cached`]). Slot `i` belongs to the caller's curve `i`,
/// which must not change while the slots are used.
#[derive(Debug, Default)]
pub(crate) struct DropPointCaches(Vec<Option<CurveCache<CurveHandle>>>);

// ---- CalculateBoundingBox --------------------------------------------------------------

/// The base-class `SmCurve::CalculateBoundingBox` of `curve` over `interval`: the
/// boxes of samples at the ends of 12 equal sub-intervals plus, in each, the
/// points where the tangent is parallel to the sub-interval chord (found by
/// `LocalPropertyAnalysis` from 3 guesses); when that analysis fails on a curve that
/// is not degenerate, the samples of a tessellation are used. The pseudo box takes
/// its basis from the first, middle and last samples; the polar box holds the sampled
/// tangents. With `expand` the position boxes grow by a hundredth of the
/// approximate length, the polar box by 0.2 degrees.
///
/// # Errors
///
/// Those of `LocalPropertyAnalysis` and the base `Tessellate`; those of `IsDegenerate` and
/// `ApproximateLength`; `InvalidInput` where upstream would read past the samples.
pub fn calculate_bounding_box_base(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    req: BoundingBoxRequest,
    expand: bool,
) -> Result<BoundingBoxes> {
    // locals
    let mut points: Vec<Point3> = Vec::with_capacity(32);
    let mut tangents: Vec<Vec3> = Vec::with_capacity(32);
    let want_tangents = req.polar;

    // Algorithm: break the curve into spans, and for each span, find the point(s)
    // in the span that are farthest away from the line segment connecting the start
    // and end points of the span (where the curve tangent is parallel to it).
    let mut stat_ok = false;

    let mut sol: Solution<ObjectRef> = Solution::new();
    let delta = 1.0 / NUM_SPANS as f64;

    let mut rel_t1 = 0.0;
    let num_derivs = usize::from(want_tangents);
    let mut pt_der = [Vec3::UNDEF, Vec3::UNDEF];
    let sample = |t: f64, pt_der: &mut [Vec3; 2]| -> Result<()> {
        // nonZeroTangent values (status ignored)
        if let Some(v) = ignored(curve.evaluate(k, t, num_derivs, false, true))? {
            for (dst, src) in pt_der.iter_mut().zip(v) {
                *dst = src;
            }
        }
        Ok(())
    };
    sample(interval.min(), &mut pt_der)?;
    let [p, d] = pt_der;
    let mut pt1 = p;
    if want_tangents {
        tangents.push(d);
    }
    points.push(pt1);

    // Check at three guess points on subinterval.
    let guess_fractions = [0.2, 0.48, 0.8];

    let mut sub_ivl = Extent1d::new();
    for ii in 0..NUM_SPANS {
        let rel_t0 = rel_t1;
        rel_t1 = (ii + 1) as f64 * delta;
        if ii >= NUM_SPANS - 1 {
            rel_t1 = 1.0;
        }
        ignored(sub_ivl.set_min_max(interval.evaluate(rel_t0), interval.evaluate(rel_t1)))?;

        let pt0 = pt1;
        sample(sub_ivl.max(), &mut pt_der)?;
        let [p, d] = pt_der;
        pt1 = p;
        if want_tangents {
            tangents.push(d);
        }
        let this_vec = pt1 - pt0;
        let mut prev_t = sub_ivl.min();

        for &fraction in &guess_fractions {
            let guess_t = sub_ivl.evaluate(fraction);

            // Maybe the last iteration found a good one here.
            if guess_t < prev_t + EFF_ZERO_SQRT {
                continue;
            }

            let found = match local_property_analysis(
                k,
                curve,
                &sub_ivl,
                CurvePropertyType::ParallelToVector,
                guess_t,
                None,
                &[this_vec],
                &mut sol,
            ) {
                Ok(found) => {
                    stat_ok = true;
                    found
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => {
                    stat_ok = false;
                    // Quit and try something different.
                    break;
                }
            };

            if found && sol.solution_type == SolutionType::SingleValue {
                let this_t = sol.start.param(0)?;
                if this_t > prev_t + EFF_ZERO_SQRT {
                    sample(this_t, &mut pt_der)?;
                    let [p, d] = pt_der;
                    points.push(p);
                    if want_tangents {
                        tangents.push(d);
                    }
                }
                prev_t = this_t;
            }
        }

        if !stat_ok {
            // Maybe the curve is degenerate.
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                // Just go with the first point, which is already in the points.
                stat_ok = true;
            }
            // (otherwise: shouldn't happen; try tessellation)
            break;
        }

        // Add the end point of the span.
        points.push(pt1);
    }

    // pick tessellate control accuracies
    let (size, _) = curve.approximate_length(k, interval, 5, false)?;
    let angle_tol_deg = 2.0;

    // LocalPropertyAnalysis failed, use Tess.
    if !stat_ok {
        // sample the target curve over target interval
        let chord_height = size / 100_000.0;
        // (Tessellate resets the arrays it is given)
        let tess = curve.tessellate(k, interval, chord_height, angle_tol_deg, 0)?;
        points = tess.points;
        if want_tangents {
            tangents = tess.tangents;
        }
    }

    let count = points.len();
    let point = |i: usize| -> Result<Point3> {
        points.get(i).copied().ok_or(SolidError::invalid_input(
            OP_BOX,
            "sample index past the samples",
        ))
    };
    let tangent = |i: usize| -> Result<Vec3> {
        tangents.get(i).copied().ok_or(SolidError::invalid_input(
            OP_BOX,
            "tangent index past the sampled tangents",
        ))
    };

    let mut out = BoundingBoxes::default();

    // init pBBox
    let mut bbox = Extent3d::new();
    if req.normal {
        let p0 = point(0)?;
        ignored(bbox.set_min_max(p0, p0))?;
    }

    // init pseudobox
    let mut pseudo = PseudoBox::new();
    if req.pseudo {
        // init intervals - leave basis vectors alone
        pseudo.init();

        // Use first, mid, and last points to try to compute a basis vector set
        let (mut l1, mut l2, mut l3) = (0, count / 2, count.wrapping_sub(1));
        let mut v1 = point(l3)? - point(l1)?;

        // for closed curves - try different sample points to generate V1
        if is_zero(v1.length_squared()) {
            (l1, l2, l3) = match count {
                0 | 1 => (0, 0, 0),
                2 => (0, 0, 1),
                3 | 4 => (0, 1, 2),
                _ => (1, count / 2, count - 2),
            };
            v1 = point(l3)? - point(l1)?;
        }

        if is_zero(v1.length_squared()) {
            // use default (x,y,z) basis vectors
            pseudo = PseudoBox::new();
        } else {
            // use basis vectors based on selected sample points
            let v2 = point(l3)? - point(l2)?;
            let [b1, b2, b3] = v1.make_unit_ortho_vectors(Some(v2))?;
            ignored(pseudo.set_basis(b1, b2, b3))?;
        }
    }

    // init polarBox - use midSample point as polar axis
    let mut polar = PolarBox::new();
    if req.polar {
        polar.reset();
        let mid = tangents.len() / 2;
        polar.add_vector(tangent(mid)?, None);
    }

    // for every sample point - accumulate bounding boxes
    for (i, &p) in points.iter().enumerate() {
        if req.normal {
            bbox.add_point(p);
        }
        if req.pseudo {
            pseudo.add_point(p);
        }
        if req.polar {
            polar.add_vector(tangent(i)?, None);
        }
    }

    // when asked - expand the output boxes
    if expand {
        if req.normal {
            bbox.expand_absolute(size / 100.0);
        }
        if req.pseudo {
            pseudo.expand_absolute(size / 100.0);
        }
        if req.polar {
            // this expands arc
            polar.expand_absolute((angle_tol_deg / 10.0) * (PI / 180.0));
        }
    }

    out.normal = req.normal.then_some(bbox);
    out.pseudo = req.pseudo.then_some(pseudo);
    out.polar = req.polar.then_some(polar);
    Ok(out)
}

/// The result of [`Curve::drop_point`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DroppedPoint {
    /// A drop point was found (`rbSuccess`).
    pub success: bool,
    /// Its curve parameter (`rdDroppedParameter`).
    pub parameter: f64,
    /// Its distance (`rdDistanceToCurve`).
    pub distance: f64,
}

/// The result of [`Curve::curve_max_distance_between`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaxDistance {
    /// The largest gap seen (`rdMaxDistanceFound`).
    pub distance: f64,
    /// This curve's parameter of that gap (`pOptCurveT`).
    pub this_t: f64,
    /// The other curve's parameter of that gap (`pOptOtherT`).
    pub other_t: f64,
    /// Corresponding points were found out of order (`pbOptIncorrectDir`; `None`
    /// on the early returns, which leave it unset).
    pub incorrect_dir: Option<bool>,
}

/// The result of [`Curve::global_coincidence_checker`].
#[derive(Debug, Clone)]
pub struct Coincidence<O> {
    /// More intersections may exist between the curves (`rbAdditionalWorkNeeded`).
    pub additional_work_needed: bool,
    /// The coincident segments (`rSolutions`).
    pub solutions: Vec<Solution<O>>,
}

/// A two-curve solution of `objects` at `start` (and, for a range, `end`): `(t1, t2,
/// value)` each.
fn pair_solution<O: Copy>(
    objects: [Option<O>; 2],
    start: (f64, f64, f64),
    end: Option<(f64, f64, f64)>,
) -> Solution<O> {
    let mut s = Solution::new();
    s.num_variables = 2;
    s.num_objects = 2;
    s.start.parameters[0] = start.0;
    s.start.parameters[1] = start.1;
    s.start.solution_value = start.2;
    if let Some((e1, e2, ev)) = end {
        s.end.parameters[0] = e1;
        s.end.parameters[1] = e2;
        s.end.solution_value = ev;
        s.solution_type = SolutionType::RangeOfValues;
    } else {
        s.solution_type = SolutionType::SingleValue;
    }
    s.objects[0] = objects[0];
    s.objects[1] = objects[1];
    s
}

impl Curve {
    /// `CalculateBoundingBox`: the B-spline (and hyperbola), line and Hermite
    /// overrides, else [`calculate_bounding_box_base`]. `expand` is
    /// `bExpandBox` (upstream default `TRUE`; the overrides ignore it).
    ///
    /// # Errors
    ///
    /// Those of the override or of [`calculate_bounding_box_base`].
    pub fn calculate_bounding_box(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
    ) -> Result<BoundingBoxes> {
        match self.kind() {
            CurveKind::BSpline(c) => c.calculate_bounding_box(interval, req),
            CurveKind::Hyperbola(c) => c.base().calculate_bounding_box(interval, req),
            CurveKind::Ellipse(c) => c.base().calculate_bounding_box(interval, req),
            CurveKind::Parabola(c) => c.base().calculate_bounding_box(interval, req),
            CurveKind::Line(c) => Ok(c.calculate_bounding_box(interval, req)),
            // (its cache update is lost on this copy)
            CurveKind::CrvOnSurf(c) => c.calculate_bounding_box_ref(k, interval, req, expand),
            // the SmCrvOnSurf override on the derived object (its Evaluate)
            CurveKind::TangentField(c) => {
                c.base()
                    .calculate_bounding_box_ref_as(k, interval, req, expand, Some(c.members()))
            }
            CurveKind::Hermite(c) => {
                let mut normal = Extent3d::new();
                let mut pseudo = PseudoBox::new();
                let mut polar = PolarBox::new();
                c.calculate_bounding_box(
                    req.normal.then_some(&mut normal),
                    req.pseudo.then_some(&mut pseudo),
                    req.polar.then_some(&mut polar),
                )?;
                Ok(BoundingBoxes {
                    normal: req.normal.then_some(normal),
                    pseudo: req.pseudo.then_some(pseudo),
                    polar: req.polar.then_some(polar),
                })
            }
            CurveKind::Composite(c) => c.calculate_bounding_box(k, interval, req, expand),
            _ => calculate_bounding_box_base(k, self, interval, req, expand),
        }
    }

    /// `IsPlanar`: [`PseudoBox::is_planar`] of the pseudo box of the curve over its
    /// natural interval (`tol == EFF_ZERO` lets the box pick its scaled zero).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`Curve::calculate_bounding_box`] (whose other
    /// failures leave the default box, as upstream ignores the status).
    pub fn is_planar(&self, k: &Kernel, tol: f64) -> Result<Planar> {
        let req = BoundingBoxRequest {
            normal: false,
            pseudo: true,
            polar: false,
        };
        let pseudo = ignored(self.calculate_bounding_box(k, &self.natural_interval(), req, true))?
            .and_then(|b| b.pseudo)
            .unwrap_or_else(PseudoBox::new);
        Ok(pseudo.is_planar(tol))
    }

    /// `DropPoint`: the curve point over `interval` closest to `point`. With a
    /// `guess` a local solve from it is tried first; without one, or when it fails
    /// (or, for [`SolverOperation::Intersect`], lies farther than `dist_tol`), the
    /// global solve. A solution within `dist_tol` of an interval end is snapped to it
    /// or rejected according to `vec_to_inside` (the curve tangent pointing from the
    /// wanted end into the curve). [`SolverOperation::Intersect`] accepts only
    /// solutions closer than `dist_tol`; operations other than `Minimize`,
    /// `Intersect` and `Normalize` become `Minimize`.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::local_point_solve`] and the snapping evaluations;
    /// [`SolidError::Unported`] for the global solve (`SmCurve::GlobalPointSolve`,
    /// unit `curve.curve_p1`).
    pub fn drop_point(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
    ) -> Result<DroppedPoint> {
        self.drop_point_cached(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            &mut None,
        )
    }

    /// [`Curve::drop_point`] keeping the curve's cache for the global solve in slot
    /// `index` of `caches` (see [`DropPointCaches`]).
    ///
    /// # Errors
    ///
    /// As [`Curve::drop_point`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drop_point_in(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        caches: &mut DropPointCaches,
        index: usize,
    ) -> Result<DroppedPoint> {
        if caches.0.len() <= index {
            caches.0.resize_with(index + 1, || None);
        }
        let mut fresh = None;
        let slot = caches.0.get_mut(index).unwrap_or(&mut fresh);
        self.drop_point_cached(k, interval, point, vec_to_inside, dist_tol, guess, op, slot)
    }

    /// [`Curve::drop_point`] keeping the curve's cache for the global solve in
    /// `cache` across calls on this unchanged curve (upstream keeps it on the curve;
    /// see [`Curve::global_point_solve_cached`]). The `SmCrvOnSurf` override (also as
    /// the base of an `SmTangentField`) passes it to its base-class drop.
    fn drop_point_cached(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        cache: &mut Option<CurveCache<CurveHandle>>,
    ) -> Result<DroppedPoint> {
        match self.kind() {
            // the SmCrvOnSurf override (its approximation and break updates are lost on
            // this copy; the curve cache is the caller's)
            CurveKind::CrvOnSurf(c) => c.drop_point_ref_as(
                k,
                interval,
                point,
                vec_to_inside,
                dist_tol,
                guess,
                op,
                None,
                cache,
            ),
            // the SmCrvOnSurf override on the derived object (its Evaluate)
            CurveKind::TangentField(c) => c.base().drop_point_ref_as(
                k,
                interval,
                point,
                vec_to_inside,
                dist_tol,
                guess,
                op,
                Some(c.members()),
                cache,
            ),
            _ => self.drop_point_base_cached(
                k,
                interval,
                point,
                vec_to_inside,
                dist_tol,
                guess,
                op,
                cache,
            ),
        }
    }

    /// `SmCurve::DropPoint` (the base-class body of [`Curve::drop_point`]).
    ///
    /// # Errors
    ///
    /// As [`Curve::drop_point`].
    pub fn drop_point_base(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
    ) -> Result<DroppedPoint> {
        self.drop_point_base_cached(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            &mut None,
        )
    }

    /// [`Curve::drop_point_base`] with the global solve's curve cache kept in `cache`
    /// (see [`Curve::drop_point_cached`]).
    pub(crate) fn drop_point_base_cached(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        cache: &mut Option<CurveCache<CurveHandle>>,
    ) -> Result<DroppedPoint> {
        // init output
        let mut out = DroppedPoint::default();

        // check inputs
        let op = if op != SolverOperation::Minimize
            && op != SolverOperation::Intersect
            && op != SolverOperation::Normalize
        {
            // Bad DropPoint eOperationType - selecting eOperationType = SM_SO_MINIMIZE
            SolverOperation::Minimize
        } else {
            op
        };

        // For Intersect, the solution must be within the given tolerance.
        let xsect_tol = if op == SolverOperation::Intersect {
            dist_tol
        } else {
            BIG_DOUBLE
        };

        // when given a guess - pass the call to LocalPointSolve and save answers
        let mut local_converged = false;
        let mut solution: Solution<()> = Solution::new();

        if let Some(guess) = guess {
            let (converged, sol) = self.local_point_solve::<()>(
                k,
                interval,
                op,
                point,
                &PointSolveOptions::default(),
                guess,
            )?;
            local_converged = converged;
            solution = sol;

            // If local solve converged, set output whether good enough or not.
            if local_converged {
                out.parameter = solution.start.param(0)?;
                out.distance = solution.start.solution_value;
            }

            // when a good enough solution was found
            if local_converged && solution.start.solution_value < xsect_tol {
                let (snapped, keep_sol) = snap_sol_to_specified_endpoint(
                    k,
                    self,
                    interval,
                    vec_to_inside,
                    solution.start.param(0)?,
                    dist_tol,
                )?;
                *solution.start.param_mut(0)? = snapped;

                // when solution passed the test - use it
                if keep_sol && solution.start.solution_value < xsect_tol {
                    out.success = true;
                    out.parameter = solution.start.param(0)?;
                    out.distance = solution.start.solution_value;
                }
            }
        }

        // If no guess given OR local solve failed, pass call to GlobalPointSolve.
        if !local_converged
            || (op == SolverOperation::Intersect && solution.start.solution_value > xsect_tol)
        {
            let opts = GlobalPointSolveOptions {
                handle: Some(CurveHandle(self.natural_interval())),
                ..GlobalPointSolveOptions::default()
            };
            let solutions = self.global_point_solve_cached(
                k,
                interval,
                op,
                point,
                dist_tol,
                SolutionRequested::Single,
                &opts,
                cache,
            )?;

            // for every solution
            for sol in solutions.as_slice() {
                let (snapped, keep_sol) = snap_sol_to_specified_endpoint(
                    k,
                    self,
                    interval,
                    vec_to_inside,
                    sol.start.param(0)?,
                    dist_tol,
                )?;

                // set output with solutions that pass all tests
                if keep_sol && sol.start.solution_value < xsect_tol {
                    out.success = true;
                    out.parameter = snapped;
                    out.distance = sol.start.solution_value;
                    break;
                }
            }
        }

        // all done
        Ok(out)
    }

    /// `GlobalPointSolveSTEP`: the line, hyperbola, ellipse and parabola overrides;
    /// the base passes the call to [`Curve::global_point_solve`]. `this` names the
    /// curve in the solutions (the hyperbola's, ellipse's and parabola's name no
    /// object, see their units); `analytic` names the analytic replacement the base
    /// solve may answer from ([`GlobalPointSolveOptions::analytic_handle`]: upstream's
    /// pointer to the cache's analytic curve).
    ///
    /// # Errors
    ///
    /// Those of the hyperbola and parabola overrides and of
    /// [`Curve::global_point_solve`].
    pub fn global_point_solve_step<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        step_interval: &Extent1d,
        op: SolverOperation,
        test_point: Point3,
        dist_tol: f64,
        target_distance: Option<f64>,
        vectors: Option<&[Vec3]>,
        requested: SolutionRequested,
        this: Option<O>,
        analytic: Option<O>,
    ) -> Result<Vec<Solution<O>>> {
        let owner = self.owner();
        match self.kind_mut() {
            CurveKind::Line(l) => {
                Ok(l.global_point_solve_step(step_interval, op, test_point, dist_tol, this))
            }
            CurveKind::Hyperbola(h) => {
                let sols = h.global_point_solve_step(
                    k,
                    step_interval,
                    op,
                    test_point,
                    dist_tol,
                    target_distance,
                    requested,
                    owner,
                )?;
                Ok(sols
                    .iter()
                    .map(|s| {
                        let mut r: Solution<O> = Solution::new();
                        r.solution_type = s.solution_type;
                        r.num_variables = s.num_variables;
                        r.start = s.start;
                        r.end = s.end;
                        r.num_objects = s.num_objects;
                        r.nodes = s.nodes;
                        r
                    })
                    .collect())
            }
            CurveKind::Ellipse(e) => {
                let sols = e.global_point_solve_step(
                    k,
                    op,
                    test_point,
                    dist_tol,
                    target_distance,
                    requested,
                )?;
                Ok(sols
                    .iter()
                    .map(|s| {
                        let mut r: Solution<O> = Solution::new();
                        r.solution_type = s.solution_type;
                        r.num_variables = s.num_variables;
                        r.start = s.start;
                        r.end = s.end;
                        r.num_objects = s.num_objects;
                        r.nodes = s.nodes;
                        r
                    })
                    .collect())
            }
            CurveKind::Parabola(p) => {
                let sols = p.global_point_solve_step(
                    k,
                    owner,
                    step_interval,
                    op,
                    test_point,
                    dist_tol,
                    target_distance,
                    requested,
                )?;
                Ok(sols
                    .iter()
                    .map(|s| {
                        let mut r: Solution<O> = Solution::new();
                        r.solution_type = s.solution_type;
                        r.num_variables = s.num_variables;
                        r.start = s.start;
                        r.end = s.end;
                        r.num_objects = s.num_objects;
                        r.nodes = s.nodes;
                        r
                    })
                    .collect())
            }
            _ => {
                let opts = GlobalPointSolveOptions {
                    target_distance,
                    opt_vectors: vectors,
                    handle: this,
                    analytic_handle: analytic,
                    zone_tol_3d: None,
                };
                Ok(self
                    .global_point_solve(
                        k,
                        step_interval,
                        op,
                        test_point,
                        dist_tol,
                        requested,
                        &opts,
                    )?
                    .as_slice()
                    .to_vec())
            }
        }
    }

    /// `CurveMaxDistanceBetween`: the largest gap between `other` over
    /// `[other_start, other_end]` (mapped onto this curve's `interval`, reversed when
    /// `other_end < other_start`) and this curve, from `num_samples` uniform samples
    /// of `other` dropped onto this curve by local solves (global solves for
    /// unborrowed B-splines when they fail). `num_samples == 0` samples by the knot
    /// counts and refines the largest gap with a local curve/curve maximisation; two
    /// lines take two samples. With `max_needed` the search stops at the first gap
    /// larger than it. The same curve over the same interval has no gap.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, the global solve and the local solves.
    pub fn curve_max_distance_between(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_start: f64,
        other_end: f64,
        num_samples: u64,
        max_needed: Option<f64>,
    ) -> Result<MaxDistance> {
        // No work: curves and intervals are the same.
        if core::ptr::eq(other, self)
            && ((are_same(interval.min(), other_start) && are_same(interval.max(), other_end))
                || (are_same(interval.min(), other_end) && are_same(interval.max(), other_start)))
        {
            return Ok(MaxDistance {
                distance: 0.0,
                this_t: interval.mid(),
                other_t: interval.mid(),
                incorrect_dir: None,
            });
        }

        // locals
        let mut other_interval = Extent1d::from_value(other_start);
        other_interval.add_value(other_end);

        let same_orientation = other_end >= other_start;
        let orientation = if same_orientation { 1.0 } else { -1.0 };
        let mut max_dist = 0.0;
        let mut do_precise = false;
        let mut num_samples = num_samples;

        // Do a very large sample and adjust it upward to compensate for cases where
        // there are a large number of knots.
        if num_samples == 0 {
            do_precise = true;
            let knots = knots_ignoring_status(self.knots_with_status(k, None))?
                .0
                .len();
            let other_knots = knots_ignoring_status(other.knots_with_status(k, None))?
                .0
                .len();
            num_samples = 22 + knots as u64 + other_knots as u64;
        }

        // special case line/line check
        if self.degree()? == 1
            && self.number_natural_knots(k)? == 4
            && other.degree()? == 1
            && other.number_natural_knots(k)? == 4
        {
            do_precise = false;
            num_samples = 2;
        }

        // init maxGap Point to interval midPoints
        let mut this_t_max = interval.evaluate(0.5);
        let mut other_t_max = other_interval.evaluate(0.5);

        let mut incorrect_dir = false;
        let mut prev_parameter: Option<f64> = None;
        let opts = PointSolveOptions {
            distance_tolerance: None,
            target_distance: max_needed,
            vectors: None,
        };

        // Uniformly sample OtherCurve from beginning to end
        for ii in 0..num_samples {
            let per = ii as f64 / (num_samples - 1) as f64;
            let t_guess1 = interval.evaluate(per);
            let mut t_guess2 = if orientation > 0.0 {
                other_interval.evaluate(per)
            } else {
                other_interval.evaluate(1.0 - per)
            };

            // locals for localPointSolve
            let mut found_parameter = 0.0;
            let mut pnt = other.evaluate_point(k, t_guess2)?;

            // Find ThisCurvePoint Normal to OtherCurvePoint
            let (mut found, mut solution) = self.local_point_solve::<()>(
                k,
                interval,
                SolverOperation::Minimize,
                pnt,
                &opts,
                t_guess1,
            )?;

            // when no solution was found for 1st guess
            if !found && ii == 0 {
                // try moving ThisCurvePoint a small distance from its endPoint
                t_guess2 = other_interval.evaluate(0.001);
                pnt = other.evaluate_point(k, t_guess2)?;
                (found, solution) = self.local_point_solve::<()>(
                    k,
                    interval,
                    SolverOperation::Minimize,
                    pnt,
                    &opts,
                    t_guess1,
                )?;
            }

            // when no solution was found for last guess
            if !found && ii == num_samples - 1 {
                // try moving ThisCurvePoint a small distance from its endPoint
                t_guess2 = other_interval.evaluate(0.999);
                pnt = other.evaluate_point(k, t_guess2)?;
                (found, solution) = self.local_point_solve::<()>(
                    k,
                    interval,
                    SolverOperation::Minimize,
                    pnt,
                    &opts,
                    t_guess1,
                )?;
            }

            // get ThisCurve solution point and gap vector distance
            let mut found_dist = BIG_DOUBLE;
            if found {
                let test_pnt = self.evaluate_point(k, solution.start.param(0)?)?;
                found_dist = test_pnt.distance(pnt);
                found_parameter = solution.start.param(0)?;
            }

            // Save biggest solution gap vector
            if found && found_dist > max_dist {
                this_t_max = solution.start.param(0)?;
                other_t_max = t_guess2;
                max_dist = found_dist;
            }

            // when LocalPoint solver failed to find a solution - try GlobalPoint Solver
            if !found {
                // Note that borrowed NURBS are on the stack and can not do global operations.
                let borrowed = match self.kind() {
                    CurveKind::BSpline(c) => Some(c.is_nurb_borrowed()),
                    CurveKind::Hyperbola(c) => Some(c.base().is_nurb_borrowed()),
                    CurveKind::Ellipse(c) => Some(c.base().is_nurb_borrowed()),
                    CurveKind::Parabola(c) => Some(c.base().is_nurb_borrowed()),
                    CurveKind::Line(c) => Some(c.base.is_nurb_borrowed()),
                    _ => None,
                };
                if borrowed == Some(false) {
                    // look for ThisCurve nearest Point to sOtherCurvePoint using GlobalPoint solution
                    let opts = GlobalPointSolveOptions {
                        target_distance: max_needed,
                        handle: Some(CurveHandle(self.natural_interval())),
                        ..GlobalPointSolveOptions::default()
                    };
                    let solutions = self.global_point_solve(
                        k,
                        interval,
                        SolverOperation::Minimize,
                        pnt,
                        EFF_ZERO_SQRT,
                        SolutionRequested::Single,
                        &opts,
                    )?;

                    // when a solution was found
                    if let Some(first) = solutions.as_slice().first() {
                        found = true;
                        found_parameter = first.start.param(0)?;

                        // save the largest gap value
                        if first.start.solution_value > max_dist {
                            this_t_max = first.start.param(0)?;
                            other_t_max = t_guess2;
                            max_dist = first.start.solution_value;
                        }
                    } else if let Some(needed) = max_needed {
                        // If no solution found, assume it's a big gap (signals failure).
                        max_dist = 2.0 * needed;
                        break;
                    }
                } else {
                    // Find ThisCurvePoint Closest (not Normal) to OtherCurvePoint
                    (found, solution) = self.local_point_solve::<()>(
                        k,
                        interval,
                        SolverOperation::Minimize,
                        pnt,
                        &opts,
                        t_guess1,
                    )?;
                    if found {
                        found_parameter = solution.start.param(0)?;
                    }

                    // save the largest gap seen
                    if found && solution.start.solution_value > max_dist {
                        this_t_max = solution.start.param(0)?;
                        other_t_max = t_guess2;
                        max_dist = solution.start.solution_value;
                    }
                }
            }

            if found {
                if let Some(prev) = prev_parameter {
                    if same_orientation {
                        if prev > found_parameter {
                            incorrect_dir = true;
                        }
                    } else if prev < found_parameter {
                        incorrect_dir = true;
                    }
                }
                prev_parameter = Some(found_parameter);
            }

            // When given a MaxDistance limit - and the limit is exceeded
            if let Some(needed) = max_needed {
                if needed < max_dist {
                    // Set outputs and quit.
                    return Ok(MaxDistance {
                        distance: max_dist,
                        this_t: this_t_max,
                        other_t: other_t_max,
                        incorrect_dir: None,
                    });
                }
            }
        }

        // get maxGap ThisCurve Point
        let pnt = self.evaluate_point(k, this_t_max)?;

        // when doing precise and MaxDist found is greater than zero
        let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());
        if do_precise && max_dist > scaled_zero {
            // find the gap maximum near the largest sampled gap point
            let (found, solution) = self.local_curve_solve::<()>(
                k,
                interval,
                other,
                &other_interval,
                SolverOperation::Maximize,
                EFF_ZERO_SQRT,
                None,
                None,
                [this_t_max, other_t_max],
                [None, None],
            )?;

            // when largest gap maximum was found
            if found {
                // get Gap points and size
                let t_max1 = solution.start.param(0)?;
                let t_max2 = solution.start.param(1)?;
                let pnt1 = self.evaluate_point(k, t_max1)?;
                let pnt2 = other.evaluate_point(k, t_max2)?;
                let found_dist = pnt1.distance(pnt2);

                // save largest gap seen
                if found_dist > max_dist {
                    this_t_max = t_max1;
                    other_t_max = t_max2;
                    max_dist = found_dist;
                }
            }
        }

        // set output and return
        Ok(MaxDistance {
            distance: max_dist,
            this_t: this_t_max,
            other_t: other_t_max,
            incorrect_dir: Some(incorrect_dir),
        })
    }

    /// `SimpleCoincidenceChecker` (the pre `SM_TOLERANT_XSECT` version): drops the four
    /// interval end points onto the other curve (skipping an end farther than
    /// `tol` from the other curve's optional pseudo box), keeps the drops closer than
    /// `tol` sorted by this curve's parameter, and returns nothing (no drop), one
    /// point (one drop or a tiny parameter range: the better end), two points (the
    /// first and last drops when the curves separate by more than `tol` between them,
    /// or either range is degenerate), or one coincident range. `objects` name this
    /// curve and `other` in the solutions.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, [`Curve::local_point_solve`] and
    /// [`Curve::curve_max_distance_between`].
    pub fn simple_coincidence_checker<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        pseudo_box: Option<&PseudoBox>,
        pseudo_box_other: Option<&PseudoBox>,
        objects: [Option<O>; 2],
    ) -> Result<Vec<Solution<O>>> {
        // evaluate both curveInterval min/max endPoints
        let start_point1 = self.evaluate_point(k, interval.min())?;
        let end_point1 = self.evaluate_point(k, interval.max())?;
        let start_point2 = other.evaluate_point(k, other_interval.min())?;
        let end_point2 = other.evaluate_point(k, other_interval.max())?;

        // Curve/curve MinDistance parameter pairs and distance value
        let mut drops: Vec<(f64, f64, f64)> = Vec::with_capacity(4);
        let opts = PointSolveOptions::default();

        // check OtherCurve/ThisStartPoint min distance for coincidence
        if pseudo_box_other.is_none_or(|b| b.distance_to_point(start_point1) < tol) {
            // find OtherCurve Point closest to ThisCurve StartPoint
            let guess = other_interval.evaluate(0.5);
            let (found, sol) = other.local_point_solve::<()>(
                k,
                other_interval,
                SolverOperation::Minimize,
                start_point1,
                &opts,
                guess,
            )?;

            // When OtherCurve Is within Tolerance of ThisCurve StartPoint
            if found && sol.start.solution_value < tol {
                drops.push((
                    interval.min(),
                    sol.start.param(0)?,
                    sol.start.solution_value,
                ));
            }
        }

        // check ThisCurve/OtherStartPoint min distance for coincidence
        if pseudo_box.is_none_or(|b| b.distance_to_point(start_point2) < tol) {
            // find ThisCurve Point closest to OtherCurve StartPoint
            let guess = interval.evaluate(0.5);
            let (found, sol) = self.local_point_solve::<()>(
                k,
                interval,
                SolverOperation::Minimize,
                start_point2,
                &opts,
                guess,
            )?;

            // When ThisCurve Is within Tolerance of OtherCurve StartPoint
            if found && sol.start.solution_value < tol {
                // throw away small bit errors
                let mut t = sol.start.param(0)?;
                if t < interval.min() {
                    t = interval.min();
                } else if t > interval.max() {
                    t = interval.max();
                }
                drops.push((t, other_interval.min(), sol.start.solution_value));
            }
        }

        // check ThisCurve/OtherEndPoint min distance for coincidence
        if pseudo_box.is_none_or(|b| b.distance_to_point(end_point2) < tol) {
            // find ThisCurve Point closest to OtherCurve EndPoint
            let guess = interval.evaluate(0.5);
            let (found, sol) = self.local_point_solve::<()>(
                k,
                interval,
                SolverOperation::Minimize,
                end_point2,
                &opts,
                guess,
            )?;

            // When ThisCurve Is within Tolerance of OtherCurve EndPoint
            if found && sol.start.solution_value < tol {
                // throw away small bit errors
                let mut t = sol.start.param(0)?;
                if t < interval.min() {
                    t = interval.min();
                } else if t > interval.max() {
                    t = interval.max();
                }

                // add solutions sorted by ThisCurve parameter values
                let entry = (t, other_interval.max(), sol.start.solution_value);
                match drops.last() {
                    Some(&(last_t, _, _)) if t < last_t => {
                        // Need to swap this solution with the last
                        drops.insert(drops.len() - 1, entry);
                    }
                    _ => drops.push(entry),
                }
            }
        }

        // check OtherCurve/ThisEndPoint min distance for coincidence
        if pseudo_box_other.is_none_or(|b| b.distance_to_point(end_point1) < tol) {
            // find OtherCurve Point closest to ThisCurve EndPoint
            let guess = other_interval.evaluate(0.5);
            let (found, sol) = other.local_point_solve::<()>(
                k,
                other_interval,
                SolverOperation::Minimize,
                end_point1,
                &opts,
                guess,
            )?;

            // When OtherCurve Is within Tolerance of ThisCurve EndPoint
            if found && sol.start.solution_value < tol {
                drops.push((
                    interval.max(),
                    sol.start.param(0)?,
                    sol.start.solution_value,
                ));
            }
        }

        // No points dropped just return
        let (Some(&first), Some(&last)) = (drops.first(), drops.last()) else {
            return Ok(Vec::new());
        };
        let num_t = drops.len();

        // If we either have just a single point which dropped or a very small
        // interval on the first curve then we have just a single solution.
        if num_t == 1
            || (num_t > 1
                && ((last.0 - first.0).abs() < EFF_ZERO || (last.1 - first.1).abs() < EFF_ZERO))
        {
            let pick = if num_t == 1 || first.2 < last.2 {
                first
            } else {
                last
            };
            return Ok(vec![pair_solution(objects, pick, None)]);
        }

        // If made it here we either have two singular points or a range of values.
        let mut found_coincidence = true;
        let ivl_t1 = Extent1d::from_min_max(first.0, last.0);
        let mut ivl_t2 = Extent1d::from_value(first.1);
        ivl_t2.add_value(last.1);
        let max_degree = self.degree()?.max(other.degree()?);

        // degenerate intervals are never coincidentIntervals
        if are_same(ivl_t1.max(), ivl_t1.min()) || are_same(ivl_t2.max(), ivl_t2.min()) {
            found_coincidence = false;
        } else {
            // find maxDistance between the two intervals
            let gap = self.curve_max_distance_between(
                k,
                &ivl_t1,
                other,
                first.1,
                last.1,
                max_degree as u64 + 3,
                Some(tol),
            )?;

            // when maxDistance is greater than tolerance
            if gap.distance > tol {
                found_coincidence = false;
            }
        }

        // when intervals are not coincident
        if !found_coincidence {
            // add two point intersections to the solutions array.
            return Ok(vec![
                pair_solution(objects, first, None),
                pair_solution(objects, last, None),
            ]);
        }

        // add 1 coincident intersection to solution array
        Ok(vec![pair_solution(objects, first, Some(last))])
    }

    /// `GlobalCoincidenceChecker` (the pre `SM_TOLERANT_XSECT` version): the coincident
    /// segment of the two curves bounded by their interval ends. Degenerate curves
    /// and curves with disjoint boxes have none. A closed or half closed curve (whose
    /// tangents sweep through 180 degrees or more) is split into three parts checked
    /// recursively, their solutions merged sorted. Otherwise the four ends are
    /// dropped onto the other curve (`SM_SO_INTERSECT` within `tol`, snapping to the
    /// ends with `snap_to_ends`) and, when at least two distinct drops bound a range
    /// along which the curves stay within `tol`, it is returned as one range solution.
    /// Opposite end to end contacts are added as point solutions.
    /// [`Coincidence::additional_work_needed`] says whether the range may miss
    /// intersections. `objects` name this curve and `other` in the solutions.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, box, closure, degeneracy and drop computations;
    /// `InvalidInput` past [`MAX_COINCIDENCE_DEPTH`] nested splits or
    /// [`MAX_COINCIDENCE_CALLS`] calls; those of the seam test of split solutions.
    pub fn global_coincidence_checker<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        snap_to_ends: bool,
        objects: [Option<O>; 2],
    ) -> Result<Coincidence<O>> {
        // the two curves' caches for the end point drops, built at the first global
        // solve and kept for the whole split recursion (upstream keeps them on the
        // curves, which do not change here)
        let mut caches = [None, None];
        let mut calls = 0;
        self.global_coincidence_checker_at(
            k,
            interval,
            other,
            other_interval,
            tol,
            snap_to_ends,
            objects,
            0,
            &mut caches,
            &mut calls,
        )
    }

    /// [`Curve::global_coincidence_checker`] for a caller that ignores its status, as
    /// upstream's callers that read `rSolutions` whatever the call returned: the check
    /// and, with its error, the solutions it had added before failing (the end to end
    /// point solutions; a split check adds its parts only once all of them succeeded).
    pub fn global_coincidence_checker_with_partial<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        snap_to_ends: bool,
        objects: [Option<O>; 2],
    ) -> (Coincidence<O>, Result<()>) {
        let mut caches = [None, None];
        let mut calls = 0;
        let mut out = Coincidence {
            additional_work_needed: false,
            solutions: Vec::new(),
        };
        let r = self.global_coincidence_checker_into(
            k,
            interval,
            other,
            other_interval,
            tol,
            snap_to_ends,
            objects,
            0,
            &mut caches,
            &mut calls,
            &mut out,
        );
        (out, r)
    }

    /// [`Curve::global_coincidence_checker`] at split depth `depth`; `calls` counts the
    /// calls of the whole check.
    fn global_coincidence_checker_at<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        snap_to_ends: bool,
        objects: [Option<O>; 2],
        depth: usize,
        caches: &mut [Option<CurveCache<CurveHandle>>; 2],
        calls: &mut usize,
    ) -> Result<Coincidence<O>> {
        let mut out = Coincidence {
            additional_work_needed: false,
            solutions: Vec::new(),
        };
        self.global_coincidence_checker_into(
            k,
            interval,
            other,
            other_interval,
            tol,
            snap_to_ends,
            objects,
            depth,
            caches,
            calls,
            &mut out,
        )?;
        Ok(out)
    }

    /// [`Curve::global_coincidence_checker_at`] into `out` (upstream's `rSolutions` and
    /// `rbAdditionalWorkNeeded`): on an error `out` keeps what the check had added
    /// before it (the end to end point solutions).
    fn global_coincidence_checker_into<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        snap_to_ends: bool,
        objects: [Option<O>; 2],
        depth: usize,
        caches: &mut [Option<CurveCache<CurveHandle>>; 2],
        calls: &mut usize,
        out: &mut Coincidence<O>,
    ) -> Result<()> {
        *calls += 1;
        if depth > MAX_COINCIDENCE_DEPTH {
            return Err(SolidError::invalid_input(
                OP_COINCIDENCE,
                "coincidence check split deeper than MAX_COINCIDENCE_DEPTH",
            ));
        }
        if *calls > MAX_COINCIDENCE_CALLS {
            return Err(SolidError::invalid_input(
                OP_COINCIDENCE,
                "coincidence check makes more than MAX_COINCIDENCE_CALLS calls",
            ));
        }

        // init output
        *out = Coincidence {
            additional_work_needed: false,
            solutions: Vec::new(),
        };

        // no work - either curve is degenerate - no solutions possible
        if self.is_degenerate(k, tol / 2.0, Some(interval))? {
            return Ok(());
        }
        if other.is_degenerate(k, tol / 2.0, Some(other_interval))? {
            return Ok(());
        }

        // Cheap reject: segments whose bounding boxes are farther apart than the
        // tolerance can have no coincidence/intersection.
        {
            let req = BoundingBoxRequest {
                normal: true,
                pseudo: false,
                polar: false,
            };
            if let Some(this_box) = ignored(self.calculate_bounding_box(k, interval, req, true))? {
                if let Some(other_box) =
                    ignored(other.calculate_bounding_box(k, other_interval, req, true))?
                {
                    if let (Some(a), Some(b)) = (this_box.normal, other_box.normal) {
                        if a.are_disjoint(&b, tol) {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Special Case: closed or half-closed curves: split it into smaller pieces
        // and recursively call itself.
        if self.is_split_needed(k, interval, tol)? {
            let pieces = split_in_three(interval);
            let mut parts = Vec::with_capacity(3);
            for piece in &pieces {
                parts.push(
                    self.global_coincidence_checker_at(
                        k,
                        piece,
                        other,
                        other_interval,
                        tol,
                        true,
                        objects,
                        depth + 1,
                        caches,
                        calls,
                    )?
                    .solutions,
                );
            }
            *out = self.merge_split_solutions(
                k,
                &parts,
                interval,
                other,
                other_interval,
                true,
                objects,
            )?;
            return Ok(());
        }

        // Do the same recursive division if the second curve is closed.
        if other.is_split_needed(k, other_interval, tol)? {
            let pieces = split_in_three(other_interval);
            let mut parts = Vec::with_capacity(3);
            for piece in &pieces {
                parts.push(
                    self.global_coincidence_checker_at(
                        k,
                        interval,
                        other,
                        piece,
                        tol,
                        true,
                        objects,
                        depth + 1,
                        caches,
                        calls,
                    )?
                    .solutions,
                );
            }
            *out = self.merge_split_solutions(
                k,
                &parts,
                interval,
                other,
                other_interval,
                false,
                objects,
            )?;
            return Ok(());
        }

        // Two non closed intervals: drop the endPoints, sort them and test the
        // maximum distance between the curves.

        // get thisCurve start and stop position/tangents
        let line_pt1 = self.evaluate(k, interval.min(), 1, true, true)?;
        let (line_pt1_0, line_pt1_1) = (at(&line_pt1, 0), at(&line_pt1, 1));
        let end1 = self.evaluate(k, interval.max(), 1, true, true)?;
        let (end1_0, end1_1) = (at(&end1, 0), -at(&end1, 1)); // Negate end derivatives

        // get otherCurve start and stop position/tangents
        let line_pt2 = other.evaluate(k, other_interval.min(), 1, true, true)?;
        let (line_pt2_0, line_pt2_1) = (at(&line_pt2, 0), at(&line_pt2, 1));
        let end2 = other.evaluate(k, other_interval.max(), 1, true, true)?;
        let (end2_0, end2_1) = (at(&end2, 0), -at(&end2, 1)); // Negate end derivatives

        // Add any endPoint/endPoint coincident cases with open gaps as single solutions
        let dist11 = line_pt1_0.distance(line_pt2_0);
        if dist11 < tol && line_pt1_1.dot(line_pt2_1) < 0.0 {
            out.solutions.push(pair_solution(
                objects,
                (interval.min(), other_interval.min(), dist11),
                None,
            ));
        }
        let dist12 = line_pt1_0.distance(end2_0);
        if dist12 < tol && line_pt1_1.dot(end2_1) < 0.0 {
            out.solutions.push(pair_solution(
                objects,
                (interval.min(), other_interval.max(), dist12),
                None,
            ));
        }
        let dist21 = end1_0.distance(line_pt2_0);
        if dist21 < tol && end1_1.dot(line_pt2_1) < 0.0 {
            out.solutions.push(pair_solution(
                objects,
                (interval.max(), other_interval.min(), dist21),
                None,
            ));
        }
        let dist22 = end1_0.distance(end2_0);
        if dist22 < tol && end1_1.dot(end2_1) < 0.0 {
            out.solutions.push(pair_solution(
                objects,
                (interval.max(), other_interval.max(), dist22),
                None,
            ));
        }

        // locals
        let mut drops: Vec<(f64, f64, f64)> = Vec::with_capacity(4);

        // Drop curve end points using Intersect.
        let solver_op = SolverOperation::Intersect;
        let snap = |v: Vec3| snap_to_ends.then_some(v);

        // drop thisCurve StartPoint onto otherCurve snapping to sLinePt1[0]
        let [this_cache, other_cache] = caches;
        let d = other.drop_point_cached(
            k,
            other_interval,
            line_pt1_0,
            snap(line_pt1_1),
            tol,
            None,
            solver_op,
            other_cache,
        )?;
        if d.success && d.distance < tol {
            drops.push((interval.min(), d.parameter, d.distance));
        }

        // drop otherCurve startPoint onto thisCurve
        let d = self.drop_point_cached(
            k,
            interval,
            line_pt2_0,
            snap(line_pt2_1),
            tol,
            None,
            solver_op,
            this_cache,
        )?;
        if d.success && d.distance < tol {
            drops.push((d.parameter, other_interval.min(), d.distance));
        }

        // drop otherCurve EndPoint onto thisCurve
        let d = self.drop_point_cached(
            k,
            interval,
            end2_0,
            snap(end2_1),
            tol,
            None,
            solver_op,
            this_cache,
        )?;
        if d.success && d.distance < tol {
            let entry = (d.parameter, other_interval.max(), d.distance);
            match drops.last() {
                Some(&(last_t, _, _)) if d.parameter < last_t => {
                    // keep sorted relative to this curve's parameterization
                    drops.insert(drops.len() - 1, entry);
                }
                _ => drops.push(entry),
            }
        }

        // No work - No points dropped so far no solutions possible
        if drops.is_empty() {
            out.additional_work_needed = true;
            return Ok(());
        }

        // drop thisCurve EndPoint onto otherCurve
        let d = other.drop_point_cached(
            k,
            other_interval,
            end1_0,
            snap(end1_1),
            tol,
            None,
            solver_op,
            other_cache,
        )?;
        if d.success && d.distance < tol {
            drops.push((interval.max(), d.parameter, d.distance));
        }

        let (Some(&first), Some(&last)) = (drops.first(), drops.last()) else {
            out.additional_work_needed = true;
            return Ok(());
        };
        let num_t = drops.len();

        // If we either have just a single point which dropped or a very small
        // interval on the first or second curve then there is just a single point
        // solution (not added: it would mess up combining interval ranges).
        let scaled_zero1 = EFF_ZERO * (1.0 + max2(last.0.abs(), first.0.abs()));
        let scaled_zero2 = EFF_ZERO * (1.0 + max2(last.1.abs(), first.1.abs()));
        if num_t == 1
            || (num_t > 1
                && ((last.0 - first.0).abs() < scaled_zero1
                    || (last.1 - first.1).abs() < scaled_zero2))
        {
            out.additional_work_needed = true;
            return Ok(());
        }

        // Test additional points for intersection.
        let ivl_t1 = Extent1d::from_min_max(first.0, last.0);
        let max_degree = self.degree()?.max(other.degree()?);

        // find the max distance between the curves
        let gap = self.curve_max_distance_between(
            k,
            &ivl_t1,
            other,
            first.1,
            last.1,
            max_degree as u64 + 3,
            Some(tol),
        )?;

        // if the max distance between the curves is too big curves are not
        // coincident along their entire lengths
        if gap.distance > tol {
            // we need additional work - to pull out the sub-intervals that are coincident
            out.additional_work_needed = true;
            return Ok(());
        }

        // Add coincident Intersection to solution
        out.solutions
            .push(pair_solution(objects, first, Some(last)));

        // Now determine if we need additional intersection checking
        out.additional_work_needed = true;

        // Take a closer look, using tolerances: 3d distance is parameter delta times
        // derivative magnitude.
        let mut t0 = min2(first.0, last.0);
        let mut t1 = max2(first.0, last.0);

        let mut delta_t0 = (t0 - interval.min()).abs();
        let mut delta_t1 = (t1 - interval.max()).abs();

        let mut d0 = if delta_t0 > 0.0 {
            delta_t0 * line_pt1_1.length()
        } else {
            0.0
        };
        let mut d1 = if delta_t1 > 0.0 {
            delta_t1 * end1_1.length()
        } else {
            0.0
        };
        if d0 < tol && d1 < tol {
            out.additional_work_needed = false;
        }

        // Same check on other curve.
        if out.additional_work_needed {
            t0 = min2(first.1, last.1);
            t1 = max2(first.1, last.1);

            delta_t0 = (t0 - other_interval.min()).abs();
            delta_t1 = (t1 - other_interval.max()).abs();

            d0 = if delta_t0 > 0.0 {
                delta_t0 * line_pt2_1.length()
            } else {
                0.0
            };
            d1 = if delta_t1 > 0.0 {
                delta_t1 * end2_1.length()
            } else {
                0.0
            };

            if d0 < tol && d1 < tol {
                out.additional_work_needed = false;
            }
        }

        // all done
        Ok(())
    }

    /// Whether [`Curve::global_coincidence_checker`] splits this curve over
    /// `interval`: closed, or (for an interval longer than a tenth of the zone
    /// tolerance) tangents sweeping through 180 degrees or more.
    fn is_split_needed(&self, k: &Kernel, interval: &Extent1d, tol: f64) -> Result<bool> {
        let mut do_split = self.is_closed(k, interval, tol)?;

        // In addition to closed curves, split curves which are half closed
        if !do_split {
            // Needed to break infinite recursion
            if interval.length() > ZONE_TOL_3D / 10.0 {
                let polar_box = self.approximate_polar_box(k, interval, 5)?;
                let state = polar_box.state();
                if state == PolarBoxState::Unbounded
                    || ((state == PolarBoxState::Arc || state == PolarBoxState::Region)
                        && polar_box.domain().size().max_dimension() >= PI - EFF_ZERO)
                {
                    do_split = true;
                }
            }
        }
        Ok(do_split)
    }

    /// Merges the solutions of the three split parts of
    /// [`Curve::global_coincidence_checker`] (this curve split when `split_this`,
    /// else the other), skipping point solutions across a closed seam, sorted by all
    /// parameters, and decides whether more work is needed: not when the leading
    /// contiguous run of range solutions covers all of either interval. The solutions
    /// name this curve and `other` (`objects`), whose natural intervals the merge's
    /// `IdenticalSolutions` reads ([`MergeSolver`]).
    fn merge_split_solutions<O: SolutionObject>(
        &self,
        k: &Kernel,
        parts: &[Vec<Solution<O>>],
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        split_this: bool,
        objects: [Option<O>; 2],
    ) -> Result<Coincidence<O>> {
        // Now Combine the solutions if there are any
        let mut base: GlobalSolver<'_, O> = GlobalSolver::new();
        base.num_trees = 2;
        base.num_variables = 2;
        base.tolerance_3d = 0.0;
        base.set_solutions(SolutionArray::new());
        let mut gs = MergeSolver {
            base,
            domains: CurvePair {
                objects,
                intervals: [self.natural_interval(), other.natural_interval()],
            },
        };

        for part in parts {
            for sol in part {
                // Skip Closed Curve Point Solutions using wrong endPoint
                // (the solutions' objects are [this, other]; upstream compares
                // the object pointers with the seam test's `this` curve)
                let across = if split_this {
                    is_point_solution_across_closed_seam_boundary(
                        k,
                        sol,
                        other,
                        other_interval,
                        self,
                        interval,
                        core::ptr::eq(other, self),
                    )?
                } else {
                    is_point_solution_across_closed_seam_boundary(
                        k,
                        sol,
                        self,
                        interval,
                        other,
                        other_interval,
                        true,
                    )?
                };
                if across {
                    continue;
                }

                // add solution to the solver's (now the output) solutions
                gs.add_sorted_solution(sol, SortKey::ByAllParameters, true)?;
            }
        }
        let solutions = gs.base.set_solutions(SolutionArray::new());

        // accumulate coincident range domains
        let mut check_ivl1 = Extent1d::new();
        let mut check_ivl2 = Extent1d::new();
        for sol in solutions.as_slice() {
            // when solution is a range
            if sol.solution_type == SolutionType::RangeOfValues {
                let s0 = sol.start.param(0)?;
                let s1 = sol.start.param(1)?;
                let e0 = sol.end.param(0)?;
                let e1 = sol.end.param(1)?;

                // when there is a non coincident gap in solutions (solutions are
                // ordered by first curve parameters)
                if (check_ivl1.has_negative_length()
                    && !are_same(s0, interval.min())
                    && !(other_interval.is_value_on_boundary(s1, EFF_ZERO).0
                        || other_interval.is_value_on_boundary(e1, EFF_ZERO).0))
                    || (!check_ivl1.has_negative_length() && !are_same(s0, check_ivl1.max()))
                {
                    // not all of one curve or the other is coincident
                    break;
                }

                // add this interval to the check intervals
                check_ivl1.add_value(s0);
                check_ivl1.add_value(e0);

                check_ivl2.add_value(s1);
                check_ivl2.add_value(e1);
            }
        }

        // need more work when coincident region does not include all of one curve or the other
        let additional_work_needed = !((check_ivl1.length() - interval.length()).abs() < EFF_ZERO
            || (check_ivl2.length() - other_interval.length()).abs() < EFF_ZERO);

        Ok(Coincidence {
            additional_work_needed,
            solutions: solutions.as_slice().to_vec(),
        })
    }

    /// `DeformToEndPointTargets`: moves the curve ends to `tgt_start` / `tgt_end` and
    /// sets the end tangents as `start_tan_type` / `end_tan_type` ask. A line uses
    /// its own override (edited in place for unconstrained tangents, else a new
    /// B-spline); a B-spline is edited in place; any other curve becomes a new
    /// B-spline (the NURB of a curve derived from B-splines, else an approximation)
    /// deformed to the targets and returned. Nothing happens when nothing is asked.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a new curve is needed but `want_new_curve` is false; those
    /// of the line and B-spline deformations; [`SolidError::Unported`] for the
    /// approximation of curves not derived from B-splines (`SmCurve::ApproximateCurve`,
    /// unit `curve.curve_p1`).
    pub fn deform_to_end_point_targets(
        &mut self,
        k: &Kernel,
        tgt_start: Option<Point3>,
        tgt_end: Option<Point3>,
        want_new_curve: bool,
        start_tan: Option<Vec3>,
        start_tan_type: InValueType,
        end_tan: Option<Vec3>,
        end_tan_type: InValueType,
    ) -> Result<Option<Self>> {
        self.deform_to_end_point_targets_on_surface(
            k,
            tgt_start,
            tgt_end,
            want_new_curve,
            start_tan,
            start_tan_type,
            end_tan,
            end_tan_type,
            None,
        )
    }

    /// Deforms endpoints, retaining a curve on a matching `surface` by editing its
    /// parameter-space curve. `orientation` reverses the UV target assignment when
    /// it is opposite. This is the optional-surface branch of the upstream method.
    ///
    /// # Errors
    ///
    /// Those of [`Self::deform_to_end_point_targets`], surface comparison and point
    /// dropping; `InvalidInput` when the UV curve does not derive from a B-spline.
    pub fn deform_to_end_point_targets_on_surface(
        &mut self,
        k: &Kernel,
        tgt_start: Option<Point3>,
        tgt_end: Option<Point3>,
        want_new_curve: bool,
        start_tan: Option<Vec3>,
        start_tan_type: InValueType,
        end_tan: Option<Vec3>,
        end_tan_type: InValueType,
        surface: Option<(
            &crate::surface::surface::Surface,
            Option<crate::nurbs::b_spline_surface_n_lib::p2::OrientType>,
        )>,
    ) -> Result<Option<Self>> {
        let same_or_free =
            |t: InValueType| t == InValueType::Same || t == InValueType::Unconstrained;

        // no work - no requested changes
        if tgt_start.is_none()
            && tgt_end.is_none()
            && same_or_free(start_tan_type)
            && same_or_free(end_tan_type)
        {
            return Ok(None);
        }

        if let (Some((surface, orientation)), CurveKind::CrvOnSurf(curve)) =
            (surface, self.kind_mut())
        {
            if !want_new_curve {
                return Err(SolidError::invalid_input(
                    OP_DEFORM,
                    "ppOptNewCurve must be NonNULL for non SmBSplineCurve_TYPE curves",
                ));
            }
            if surface.approx_eq(k, curve.surface())? {
                let ends = curve.uv_curve().ends(k)?;
                let domain = surface.natural_uv_domain_with_nurb(k)?;
                let drop_target = |target: Option<Point3>,
                                   current: Point3|
                 -> Result<Option<Point3>> {
                    let Some(target) = target else {
                        return Ok(None);
                    };
                    let options = crate::surface::surface::p5::DropPointOptions {
                        guess_uv: Some(crate::math::Vec2::new(current.x, current.y)),
                        ..Default::default()
                    };
                    let uv = ignored(surface.drop_point::<crate::surface::surface::p4::NoObject>(
                        k, target, &domain, &options,
                    ))?
                    .map_or(crate::math::Vec2::UNDEF, |result| result.result_uv);
                    Ok(Some(Point3::new(uv.x, uv.y, 0.0)))
                };
                let start = drop_target(tgt_start, ends.start)?;
                let end = drop_target(tgt_end, ends.end)?;
                let (start, end) = if orientation
                    == Some(crate::nurbs::b_spline_surface_n_lib::p2::OrientType::Opposite)
                {
                    (end, start)
                } else {
                    (start, end)
                };
                let uv = curve.uv_curve_mut();
                if !uv.is_kind_of(crate::curve::curve::p2::BSPLINE_CURVE_TYPE) {
                    return Err(SolidError::invalid_input(
                        OP_DEFORM,
                        "the UV curve must derive from SmBSplineCurve",
                    ));
                }
                let replacement = uv.deform_to_end_point_targets(
                    k,
                    start,
                    end,
                    true,
                    None,
                    InValueType::Same,
                    None,
                    InValueType::Same,
                )?;
                if let Some(mut replacement) = replacement {
                    ignore_status(replacement.convert_to_2d(k))?;
                    curve.set_uv_curve(replacement)?;
                }
                return Ok(None);
            }
        }

        let dim = self.dim();
        let mut new_curve = match self.kind_mut() {
            // SmLines - edited in place when both tangent types are unconstrained,
            // otherwise outputs a new deformed BSplineCurve
            CurveKind::Line(l) => {
                return Ok(l
                    .deform_to_end_point_targets(
                        tgt_start,
                        tgt_end,
                        want_new_curve,
                        start_tan,
                        start_tan_type,
                        end_tan,
                        end_tan_type,
                    )?
                    .map(Self::from));
            }

            // SmBSplineCurves - edited in place
            CurveKind::BSpline(c) => {
                c.deform_to_end_point_targets(
                    tgt_start,
                    tgt_end,
                    start_tan,
                    start_tan_type,
                    end_tan,
                    end_tan_type,
                )?;
                return Ok(None);
            }

            // arrive here when the curve is neither a line nor a B-spline
            _ if !want_new_curve => {
                return Err(SolidError::invalid_input(
                    OP_DEFORM,
                    "ppOptNewCurve must be NonNULL for non SmBSplineCurve_TYPE curves",
                ));
            }

            // for derived BSpline types: copy the Nurb curve as a plain BSplineCurve
            CurveKind::Hyperbola(h) => {
                let nurb = h.base.get_or_create_gw_nurb()?;
                BSplineCurve::from_gw_nurb(dim, nurb)
            }
            CurveKind::Ellipse(e) => {
                // GetOrCreateGwNurbPointer: the virtual MakeNurb when missing
                if e.base.gw_nurb().is_none() {
                    ignore_status(e.make_nurb(k))?;
                }
                let nurb = e.base.get_or_create_gw_nurb()?;
                BSplineCurve::from_gw_nurb(dim, nurb)
            }
            CurveKind::Parabola(p) => {
                let nurb = p.base_mut().get_or_create_gw_nurb()?;
                BSplineCurve::from_gw_nurb(dim, nurb)
            }

            // not derived BSpline type: build a new approximation BSplineCurve
            _ => {
                let ivl = self.natural_interval();
                let breaks = [ivl.min(), ivl.max()];
                let opts = ApproximateCurveOptions {
                    create_analytics: false,
                    match_parameterization: false,
                    just_copy_bsplines: true,
                };
                // (upstream ignores the status and dereferences the new curve)
                let approx = self.approximate_curve(
                    k,
                    &breaks,
                    approx_tol_3d(k, SmallTopoType::default()),
                    &opts,
                )?;
                match approx.curve.into_kind() {
                    CurveKind::BSpline(c) => c,
                    _ => {
                        return Err(SolidError::invalid_input(
                            OP_DEFORM,
                            "the approximation is not an SmBSplineCurve",
                        ))
                    }
                }
            }
        };

        // Deform new approximate BSplineCurve to the TgtEnd values
        new_curve.deform_to_end_point_targets(
            tgt_start,
            tgt_end,
            start_tan,
            start_tan_type,
            end_tan,
            end_tan_type,
        )?;
        Ok(Some(Self::from(new_curve)))
    }
}

/// `interval` split at 37.5% and 62.5% (`sIvl1`, `sIvl2`, `sIvl3`).
fn split_in_three(interval: &Extent1d) -> [Extent1d; 3] {
    [
        Extent1d::from_min_max(interval.min(), interval.evaluate(0.375)),
        Extent1d::from_min_max(interval.evaluate(0.375), interval.evaluate(0.625)),
        Extent1d::from_min_max(interval.evaluate(0.625), interval.max()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    /// A full circle of radius 2 in the XY plane (rational quadratic B-spline).
    fn circle_of_radius_two(start_deg: f64, end_deg: f64) -> Curve {
        use crate::geom::axis2_placement::Axis2Placement;
        use crate::nurbs::b_spline_curve::NurbCircleParam;
        Curve::from(
            BSplineCurve::create_circle_segment(
                3,
                &Axis2Placement::IDENTITY,
                2.0,
                start_deg,
                end_deg,
                NurbCircleParam::Quadratic,
            )
            .unwrap(),
        )
    }

    /// The reuse path itself (PR #224): an end point drop answered from a cache slot an
    /// earlier drop filled gives exactly the result of a fresh slot for that drop alone
    /// (no reuse at all), and lands where it should. Points on and just off the circle
    /// at 48 angles, so the global solve runs on the pre-filled slot.
    #[test]
    fn drop_with_prefilled_cache_slot_matches_fresh_slot_per_drop() {
        let k = Kernel::default();
        let full = circle_of_radius_two(0.0, 360.0);
        let ivl = full.natural_interval();
        let tol = 1e-6;
        let drop_at = |p: Point3, slot: &mut Option<CurveCache<CurveHandle>>| {
            full.drop_point_base_cached(
                &k,
                &ivl,
                p,
                None,
                tol,
                None,
                SolverOperation::Intersect,
                slot,
            )
            .unwrap()
        };

        let mut shared = None;
        let _ = drop_at(Vec3::new(0.0, -2.0, 0.0), &mut shared);
        assert!(shared.is_some());
        for i in 0..48 {
            let a = (f64::from(i) + 0.25) * std::f64::consts::TAU / 48.0;
            let on = Vec3::new(2.0 * a.cos(), 2.0 * a.sin(), 0.0);
            for r in [2.0, 2.0 + 0.5 * tol] {
                let p = Vec3::new(r * a.cos(), r * a.sin(), 0.0);
                let fresh = drop_at(p, &mut None);
                let reused = drop_at(p, &mut shared);
                // `Debug` prints every float exactly: equal strings are equal bits
                assert_eq!(format!("{fresh:?}"), format!("{reused:?}"), "angle {a}");
                assert!(
                    reused.success && reused.distance < tol,
                    "angle {a}: {reused:?}"
                );
                let q = full.evaluate_point(&k, reused.parameter).unwrap();
                assert!(q.distance(on) < tol, "angle {a}: {q:?}");
            }
        }
    }

    /// A closed curve is split in three (recursively) and every end drop of the check
    /// reuses one cache slot per curve (PR #224): checking again with the slots the
    /// first check filled gives the same result, which covers exactly the upper half of
    /// the circle.
    #[test]
    fn coincidence_with_prefilled_cache_slots_finds_the_half_circle() {
        use crate::surface::surface::p4::NoObject;

        let k = Kernel::default();
        let full = circle_of_radius_two(0.0, 360.0);
        let half = circle_of_radius_two(0.0, 180.0);
        let (ivl, other_ivl) = (full.natural_interval(), half.natural_interval());
        let tol = 1e-6;
        let objects = [None::<NoObject>, None];
        assert!(full.is_split_needed(&k, &ivl, tol).unwrap());

        let mut caches = [None, None];
        let mut check = || {
            full.global_coincidence_checker_at(
                &k,
                &ivl,
                &half,
                &other_ivl,
                tol,
                false,
                objects,
                0,
                &mut caches,
                &mut 0,
            )
            .unwrap()
        };
        let filling = check();
        let prefilled = check();
        assert!(caches.iter().all(Option::is_some));
        assert_eq!(format!("{filling:?}"), format!("{prefilled:?}"));

        // every solution end is a point of the upper half on both curves, and the
        // solutions' angular spans cover [0, pi] without a gap
        assert!(!prefilled.solutions.is_empty());
        let mut spans = Vec::new();
        for s in &prefilled.solutions {
            let mut angles = [0.0; 2];
            for (angle, e) in angles.iter_mut().zip([&s.start, &s.end]) {
                let p = full.evaluate_point(&k, e.param(0).unwrap()).unwrap();
                let q = half.evaluate_point(&k, e.param(1).unwrap()).unwrap();
                assert!(p.distance(q) < tol, "{p:?} vs {q:?}");
                assert!(p.y > -tol && (p.length() - 2.0).abs() < tol, "{p:?}");
                *angle = p.y.max(0.0).atan2(p.x);
            }
            let [a, b] = angles;
            spans.push((a.min(b), a.max(b)));
        }
        spans.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut covered = 0.0_f64;
        for &(lo, hi) in &spans {
            assert!(lo <= covered + tol, "gap before {lo} in {spans:?}");
            covered = covered.max(hi);
        }
        assert!(covered >= std::f64::consts::PI - tol, "{spans:?}");
    }

    fn quarter_line() -> Curve {
        let c = BSplineCurve::create_line_segment(
            3,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
        )
        .unwrap();
        Curve::from(c)
    }

    fn hermite() -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            3,
        ))
    }

    #[test]
    fn planar_line_and_hermite() {
        assert!(
            quarter_line()
                .is_planar(&Kernel::default(), EFF_ZERO)
                .unwrap()
                .is_planar
        );
        assert!(
            hermite()
                .is_planar(&Kernel::default(), EFF_ZERO)
                .unwrap()
                .is_planar
        );
    }

    #[test]
    fn drop_point_with_guess() {
        let c = quarter_line();
        let ivl = c.natural_interval();
        let d = c
            .drop_point(
                &Kernel::default(),
                &ivl,
                Vec3::new(1.0, 1.0, 0.0),
                None,
                1e-3,
                Some(0.5),
                SolverOperation::Minimize,
            )
            .unwrap();
        assert!(d.success);
        assert!((d.distance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn drop_point_without_guess_uses_the_global_solve() {
        let c = quarter_line();
        let ivl = c.natural_interval();
        let d = c
            .drop_point(
                &Kernel::default(),
                &ivl,
                Vec3::new(1.0, 1.0, 0.0),
                None,
                1e-3,
                None,
                SolverOperation::Minimize,
            )
            .unwrap();
        assert!(d.success);
        assert!((d.distance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn same_curve_has_no_gap() {
        let c = quarter_line();
        let ivl = c.natural_interval();
        let g = c
            .curve_max_distance_between(&Kernel::default(), &ivl, &c, ivl.min(), ivl.max(), 5, None)
            .unwrap();
        assert_eq!(g.distance, 0.0);
        assert_eq!(g.incorrect_dir, None);
    }

    #[test]
    fn coincident_lines_simple() {
        let a = quarter_line();
        let b = Curve::from(
            BSplineCurve::create_line_segment(
                3,
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
            )
            .unwrap(),
        );
        let sols = a
            .simple_coincidence_checker::<u8>(
                &Kernel::default(),
                &a.natural_interval(),
                &b,
                &b.natural_interval(),
                1e-6,
                None,
                None,
                [Some(0), Some(1)],
            )
            .unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0].solution_type, SolutionType::RangeOfValues);
    }

    #[test]
    fn surface_deformation_edits_uv_and_honors_opposite_orientation() {
        use crate::curve::crv_on_surf::CrvOnSurf;
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        use crate::surface::plane::Plane;
        use crate::surface::surface::Surface;
        let k = Kernel::default();
        let surface = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)));
        for derived in [false, true] {
            for orientation in [None, Some(OrientType::Same), Some(OrientType::Opposite)] {
                let uv = Curve::from(
                    BSplineCurve::create_line_segment(
                        2,
                        Vec3::new(0.0, 0.0, 0.0),
                        Vec3::new(1.0, 0.0, 0.0),
                    )
                    .unwrap(),
                );
                let uv = if derived {
                    Curve::from(crate::curve::line::Line::from_points(
                        Vec3::ZERO,
                        Vec3::new(1.0, 0.0, 0.0),
                        2,
                    ))
                } else {
                    uv
                };
                let mut curve = Curve::from(
                    CrvOnSurf::new(&k, uv, surface.clone_for_evaluation().unwrap(), None, 0)
                        .unwrap(),
                );
                let start_uv = crate::math::Vec2::new(0.25, 0.5);
                let end_uv = crate::math::Vec2::new(0.75, 0.5);
                let start = surface.evaluate_point(&k, start_uv).unwrap();
                let end = surface.evaluate_point(&k, end_uv).unwrap();
                let replacement = curve
                    .deform_to_end_point_targets_on_surface(
                        &k,
                        Some(start),
                        Some(end),
                        true,
                        None,
                        InValueType::Unconstrained,
                        None,
                        InValueType::Unconstrained,
                        Some((&surface, orientation)),
                    )
                    .unwrap();
                assert!(replacement.is_none());
                let CurveKind::CrvOnSurf(result) = curve.kind() else {
                    panic!("changed curve kind")
                };
                assert!(matches!(result.uv_curve().kind(), CurveKind::BSpline(_)));
                let ends = result.uv_curve().ends(&k).unwrap();
                let (expected_start, expected_end) = if orientation == Some(OrientType::Opposite) {
                    (end_uv, start_uv)
                } else {
                    (start_uv, end_uv)
                };
                assert!((ends.start.x - expected_start.x).abs() < 1e-12);
                assert!((ends.start.y - expected_start.y).abs() < 1e-12);
                assert!((ends.end.x - expected_end.x).abs() < 1e-12);
                assert!((ends.end.y - expected_end.y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn deform_hermite_without_slot_is_invalid() {
        let mut c = hermite();
        let k = Kernel::default();
        let e = c
            .deform_to_end_point_targets(
                &k,
                Some(Vec3::new(0.0, 1.0, 0.0)),
                None,
                false,
                None,
                InValueType::Same,
                None,
                InValueType::Same,
            )
            .unwrap_err();
        assert!(matches!(e, SolidError::InvalidInput { .. }));
    }
}
