//! `SmCurve`, part 5: the chain clean-up and duplicate removal of curve lists, the
//! shape queries (`IsLine`, `IsLinear`, `IsArc`, `IsPeriodic`, `IsDomainBoundary`),
//! the arc length (`Length`), the 1D curve property solve (`LocalPropertyAnalysis`),
//! the curve cache (`CacheMakeOrValidate`), the extra intersection guesses near
//! tangencies, and the `Evaluate` members of the curve/curve Newton equations
//! (`SmCCIntersectENFO`, `SmFindCCNormalENFO`, `SmCCTangentIntENFO`).
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                         | Upstream                                       |
//! |----------------------------------------------|------------------------------------------------|
//! | [`Curve`] (construction, [`Clone`])          | `SmCurve::SmCurve` (dimension, copy constructors) |
//! | [`crvs_cleanup`], [`CleanedCurves`]          | `SmCurve::CrvsCleanup` (static)                |
//! | [`remove_duplicate_curves`]                  | `SmCurve::RemoveDuplicateCurves` (static)      |
//! | [`Curve::evaluate_step_point`]               | `SmCurve::EvaluateSTEPPoint` (virtual)         |
//! | [`is_point_solution_across_closed_seam_boundary`] | `sm_IsPointSolutionAcrossClosedSeamBoundary` (static) |
//! | [`find_additional_cc_tangent_guess`]         | `sm_FindAdditionalCcTangentGuess` (static)     |
//! | [`make_curve_cache`], [`validate_curve_cache`] | `SmCurve::CacheMakeOrValidate`               |
//! | [`make_curve_cache_with_zone_tol`]          | `SmCurve::CacheMakeOrValidate` (edge curve) |
//! | [`find_additional_cp_guess`]                 | `sm_FindAdditionalCpGuess` (static)            |
//! | [`is_arc_base`], [`Curve::is_arc`]           | `SmCurve::IsArc` (virtual: base, circle, line, parabola, hyperbola) |
//! | [`is_line_base`], [`Curve::is_line`]         | `SmCurve::IsLine` (virtual: base, B-spline, line) |
//! | [`Curve::is_linear`], [`Linearity`]          | `SmCurve::IsLinear`                            |
//! | `Curve::is_linear_after_ends` (crate helper) | general `IsLinear` branch after virtual `GetEnds` |
//! | [`Curve::is_domain_boundary`]                | `SmCurve::IsDomainBoundary` (virtual: base, ellipse, line) |
//! | [`is_periodic`], [`Curve::is_periodic`]      | `SmCurve::IsPeriodic`                          |
//! | [`Curve::length`], [`Curve::length_base`]    | `SmCurve::Length` (virtual: base, line)        |
//! | `Curve::length_with_zone_tol` (crate helper) | `SmCurve::Length` (edge-owned curve)          |
//! | [`local_property_analysis`]                  | `SmCurve::LocalPropertyAnalysis`               |
//! | [`CcTangentIntEnfo`] (`EvalNFunctions`)      | `SmCCTangentIntENFO::Evaluate` (in `p3`)       |
//! | [`CcIntersectEnfo::evaluate_equations`]      | `SmCCIntersectENFO::Evaluate`                  |
//! | [`FindCcNormalEnfo::evaluate_equations`]     | `SmFindCCNormalENFO::Evaluate`                 |
//!
//! Deviations:
//! - **Co-ports.** `IsLine`, `IsLinear`, the new-cache branch of
//!   `CacheMakeOrValidate` ([`make_curve_cache`]) and `sm_FindAdditionalCcTangentGuess`
//!   were ported ahead of the unit by `curve.curve_cache`, `curve.curve_p3`,
//!   `curve.curve_p1` and `curve.curve_p7`. `IsLine` was written for the only kind
//!   that reaches the base class first, [`BSplineCurve`] ([`is_line_base`]), and
//!   again over [`Curve`] for the kinds without an override.
//! - **Constructors.** `SmCurve(lDim)` and the copy constructor have no body of
//!   their own: the dimension lives in the curve kinds, and [`Curve`]'s `Clone`
//!   copies the geometry and resets the owner (the `Notify` calls are the object
//!   layer's).
//! - **Caches.** Upstream keeps the cache on the curve and reuses it until the curve
//!   changes (`SmCacheMgr`, deleted on `SM_NO_PRE_EDIT`). [`make_curve_cache`] keeps a
//!   memo of its result in the curve value instead (`CurveMemo`): the build is a
//!   deterministic function of the curve's geometry and the kernel's tolerance state
//!   (`CurveMemoKey`), so a hit gives the cache a fresh build gives (the same spans and
//!   tree, in the caller's handle space). An edit of the geometry through `&mut Curve`
//!   starts an empty memo, clones share it, and [`Kernel::memoize_curve_caches`] turns
//!   it off. No result depends on the memo. A caller that solves on one unchanged
//!   curve repeatedly can also keep the result (`Curve::global_point_solve_cached`,
//!   used by `GlobalCoincidenceChecker`). [`validate_curve_cache`] is the rest of
//!   `CacheMakeOrValidate`: the `SM_OC_CURVE` type check and the reuse of an existing
//!   cache; the context check has no counterpart (a [`Curve`] always
//!   has the kernel's context).
//! - **Owner tolerance.** Upstream builds the cache on the curve object itself, so a
//!   curve that defines an edge's shape tessellates with half the edge's tolerance
//!   (`SmTol::GetApproxTol3d(&m_crCurve)`). The caches hold no `Brep`, so a caller
//!   holding the B-rep passes the edge's tolerance (`make_curve_cache_with_zone_tol`
//!   or `Curve::length_with_zone_tol`); it is part of the memo key where cached.
//! - **Curve lists.** `CrvsCleanup` and `RemoveDuplicateCurves` take owned lists;
//!   removed curves are dropped (`bDelCrv` is always true; `NULL` entries are
//!   `None`). `CrvsCleanup` casts the curves around a removed one to
//!   `SmBSplineCurve` for `TrimGap`: any kind but [`CurveKind::BSpline`] there is
//!   `InvalidInput`.
//! - **Virtual calls in generic code.** The base numerics are generic over
//!   [`CurveEvaluate`]; the virtual `IsClosed` and the `SmCrvOnSurf` cast they need
//!   are [`CurveEvaluate::is_closed_virtual`] and [`CurveEvaluate::as_crv_on_surf`].
//!   [`is_arc_base`] takes the curve's `IsDegenerate` as a closure.
//! - **Seam test.** `sm_IsPointSolutionAcrossClosedSeamBoundary` compares the
//!   solution's object pointers with the curves; the caller says whether the first
//!   object is `this` (`this_is_object0`).
//! - **Evaluators.** The `Evaluate` members of `SmCCIntersectENFO` and
//!   `SmFindCCNormalENFO` are inherent `evaluate_equations` methods (their
//!   `EvalNFunctions` impls in `p3` call them). The Jacobian is written after the
//!   function values are known (upstream interleaves the writes; only a too small
//!   matrix, an upstream assert, could tell). Option vectors that upstream reads
//!   without a `NULL` check are `InvalidInput` when missing.
//! - **Length.** Spans of degree above 1 use the ported `SmIntegrator` (unit
//!   `mass.integrator`). `Length` needs the
//!   kernel (the cache tessellation reads its tolerances).
//! - **Out-params.** Flags with outputs are `Option`s (`IsLine`: point and vector;
//!   `IsArc`: frame, radius, start and end angle; the guesses); `IsLinear`'s
//!   optional outputs are the fields of [`Linearity`] (`want_deviation` stands for a
//!   non-`NULL` `pOptActualDeviation`).
//! - **Ignored statuses.** Where upstream ignores a status (`GetEnds`, sample
//!   evaluations, `SetCanonical`, `CCWAngleBetween`, `TrimGap`,
//!   `EvaluateSurface1stDerivDerivs`) the error is ignored and the previous value
//!   (initially [`Vec3::UNDEF`]) kept, except [`SolidError::Unported`], which is
//!   propagated.

use crate::curve::curve::p2::{
    convert_to_parameter_delta, evaluate_geometric, evaluate_step, unitize_tangent, Curve,
    CurveEvaluate, CurveKind, CurvePropertyType, BSPLINE_CURVE_TYPE,
};
use crate::curve::curve::p3::eval_n;
use crate::curve::curve::p4::{
    CcIntersectEnfo, CcTangentIntEnfo, CurvePropertyEfo, FindCcNormalEnfo,
};
use crate::curve::curve_cache::{CurveCache, CurveCacheOptions};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::a_object::{CacheId, ObjectCacheType};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    circle_center_from_3_points, circle_coplanar_circle_intersect, line_coplanar_circle_intersect,
    line_point_distance, plane_drop_point, plane_evaluate_point, plane_point_distance,
    CircleCircle,
};
use crate::geom::global_solver::SolverOperation;
use crate::geom::local_solve1d::{LocalSolve1d, SolverAlgorithm};
use crate::geom::matrix::Matrix;
use crate::geom::solution_array::{Solution, SolutionType};
use crate::geom::tol::are_same;
use crate::geom::vector3d::{acos, sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p2::ArcData;
use crate::nurbs::b_spline_curve::{BSplineCurve, GapFix};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;
use std::sync::{Arc, Mutex, OnceLock};

const OP_CLEANUP: &str = "curve::curve::p5::crvs_cleanup";
const OP_CACHE: &str = "curve::curve::p5::validate_curve_cache";
const OP_LPA: &str = "curve::curve::p5::local_property_analysis";
const OP_CCI: &str = "curve::curve::p5::CcIntersectEnfo::evaluate_equations";
const OP_NORMAL: &str = "curve::curve::p5::FindCcNormalEnfo::evaluate_equations";

/// `SmCurve::IsLine` for `curve`: the start point and the start-to-end vector when
/// `num_samples` (at least 3) evenly spaced samples step along the chord by equal
/// vectors (within `tol`) with tangents parallel to it (1 degree); `None` for a
/// curve shorter than `tol` or one that is not such a line. A non-rational degree 1
/// curve with two control points is a line.
///
/// # Errors
///
/// `SM_ERR` without a NURB; [`SolidError::Unported`] from the evaluations.
pub fn is_line_base(
    curve: &BSplineCurve,
    num_samples: u64,
    tol: f64,
) -> Result<Option<(Point3, Vec3)>> {
    let num_samples = num_samples.max(3);

    // locals
    let domain: Extent1d = curve.natural_interval();
    // (GetEnds has no status; it only fails without a NURB here)
    let ends = curve.ends()?;
    let (start_pt, end_pt) = (ends.start, ends.end);

    let base_vec = end_pt - start_pt;
    if base_vec.length_squared() < tol * tol {
        return Ok(None);
    }

    // A quick check:
    if curve.degree()? == 1 && curve.number_control_points()? == 2 && !curve.is_rational()? {
        return Ok(Some((start_pt, base_vec)));
    }

    let num_spans = (num_samples - 1) as f64;

    // standard step vector: total base vector divided by number of spans
    let standard_vec = base_vec / num_spans;

    let mut pt1 = start_pt;
    let mut pt_vec = [Vec3::UNDEF; 2];
    for ii in 1..num_samples {
        let pt0 = pt1;

        let t = if ii == num_samples - 1 {
            domain.max()
        } else {
            domain.evaluate(ii as f64 / num_spans)
        };

        // exact tangent values (status ignored)
        match curve.evaluate(t, 1, false, false) {
            Ok(v) => {
                if let Some(&p) = v.first() {
                    pt_vec[0] = p;
                }
                if let Some(&d) = v.get(1) {
                    pt_vec[1] = d;
                }
            }
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => {}
        }
        pt1 = pt_vec[0];
        let test_vec = pt1 - pt0;
        let error = test_vec - standard_vec;
        if error.length_squared() > tol * tol {
            return Ok(None);
        }

        // Check angle.
        if !pt_vec[1].is_parallel_to(base_vec, 1.0) {
            return Ok(None);
        }
    }

    // Looks like a line.
    Ok(Some((start_pt, base_vec)))
}

/// `SmCurve::IsLine` (the base-class body) for any [`Curve`]: as [`is_line_base`],
/// through the curve's virtual queries.
///
/// # Errors
///
/// Those of the degree, control point count and rationality queries;
/// [`SolidError::Unported`] from the evaluations.
fn curve_is_line_base(
    k: &Kernel,
    curve: &Curve,
    num_samples: u64,
    tol: f64,
) -> Result<Option<(Point3, Vec3)>> {
    let num_samples = num_samples.max(3);

    // locals
    let domain = curve.natural_interval();
    // (GetEnds has no status upstream)
    let (start_pt, end_pt) = match ignored(curve.ends(k))? {
        Some(e) => (e.start, e.end),
        None => (Vec3::UNDEF, Vec3::UNDEF),
    };

    let base_vec = end_pt - start_pt;
    if base_vec.length_squared() < tol * tol {
        return Ok(None);
    }

    // A quick check:
    if curve.degree()? == 1 && curve.number_control_points()? == 2 && !curve.is_rational()? {
        return Ok(Some((start_pt, base_vec)));
    }

    let num_spans = (num_samples - 1) as f64;

    // standard step vector: total base vector divided by number of spans
    let standard_vec = base_vec / num_spans;

    let mut pt1 = start_pt;
    let mut pt_vec = [Vec3::UNDEF; 2];
    for ii in 1..num_samples {
        let pt0 = pt1;

        let t = if ii == num_samples - 1 {
            domain.max()
        } else {
            domain.evaluate(ii as f64 / num_spans)
        };

        // exact tangent values (status ignored)
        if let Some(v) = ignored(curve.evaluate(k, t, 1, false, false))? {
            for (d, s) in pt_vec.iter_mut().zip(v) {
                *d = s;
            }
        }
        pt1 = pt_vec[0];
        let test_vec = pt1 - pt0;
        let error = test_vec - standard_vec;
        if error.length_squared() > tol * tol {
            return Ok(None);
        }

        // Check angle.
        if !pt_vec[1].is_parallel_to(base_vec, 1.0) {
            return Ok(None);
        }
    }

    // Looks like a line.
    Ok(Some((start_pt, base_vec)))
}

/// `SmCurve::CacheMakeOrValidate` building a new cache: the curve's [`CurveCache`]
/// over its natural interval, a full tree with chord height tolerance
/// `ApproximateLength(5) / min(250, 30 * spans)`, angle tolerance 15 degrees,
/// propagated parent data and minimum parametric ratio `0.0001`, tessellated.
/// `handle` is the curve's handle in the caller's object space (the tree's owner
/// object).
///
/// # Errors
///
/// Those of the length and knot count queries and of [`CurveCache::tessellate`]
/// (upstream then has no cache).
///
/// The approximation tolerance is half the tolerance of the curve's edge when a caller
/// recorded it on the curve ([`Curve::owner_zone_tol_3d`]), else half the system
/// tolerance.
pub fn make_curve_cache<O: Copy + PartialEq>(
    k: &Kernel,
    curve: &Curve,
    handle: Option<O>,
) -> Result<CurveCache<O>> {
    // SmTol::GetApproxTol3d(&m_crCurve): the edge's tolerance through the owner
    make_curve_cache_with_zone_tol(k, curve, handle, curve.owner_zone_tol_3d())
}

/// [`make_curve_cache`] for a curve that defines an edge's shape: `zone_tol_3d` is that
/// edge's tolerance (`SmTol::GetZoneTol3d(cpCurve)`, resolved by the caller, which holds
/// the B-rep), whose half is the tessellation's approximation tolerance
/// ([`CurveCache::set_owner_zone_tol_3d`]). `None` builds as [`make_curve_cache`]:
/// the system tolerance.
///
/// # Errors
///
/// Those of [`make_curve_cache`].
pub fn make_curve_cache_with_zone_tol<O: Copy + PartialEq>(
    k: &Kernel,
    curve: &Curve,
    handle: Option<O>,
    zone_tol_3d: Option<f64>,
) -> Result<CurveCache<O>> {
    // not upstream: the memo of this build in the curve value (see CurveMemo)
    let key = CurveMemoKey::of(k, curve, zone_tol_3d);
    if let Some(key) = &key {
        if let Some(hit) = curve.memo().lookup(key, handle) {
            return Ok(hit);
        }
    }
    let cache = build_curve_cache(k, curve, handle, zone_tol_3d)?;
    if let Some(key) = key {
        if let Some(neutral) = cache.retag::<()>(None) {
            curve.memo().store(key, neutral);
        }
    }
    Ok(cache)
}

/// The kernel and curve state [`make_curve_cache`]'s build reads besides the curve's
/// geometry (not upstream): the tolerances, the context's scalar state and the upstream fixes. A memo
/// entry is valid for a later build only when its key is equal (the context scalars
/// bit for bit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CurveMemoKey {
    tolerances: crate::kernel::Tolerances,
    context_bits: [u64; 3],
    context_flags: [bool; 1],
    fixes: crate::kernel::UpstreamFixes,
    /// The bits of the owner zone tolerance the build was given
    /// ([`make_curve_cache_with_zone_tol`]).
    owner_zone_tol_bits: Option<u64>,
}

impl CurveMemoKey {
    /// The key of a build of `curve` under `k` with the owner zone tolerance
    /// `zone_tol_3d`, or `None` when the build must not be memoised: the memo is off,
    /// or the curve is not a pure value ([`memo_pure`]). The owner itself is not an
    /// input: the build works on its own copy of the curve, which has none, and reads
    /// the owner only through `zone_tol_3d`.
    fn of(k: &Kernel, curve: &Curve, zone_tol_3d: Option<f64>) -> Option<Self> {
        if !k.memoize_curve_caches || !memo_pure(curve) {
            return None;
        }
        Self::of_kernel(k).map(|key| Self {
            owner_zone_tol_bits: zone_tol_3d.map(f64::to_bits),
            ..key
        })
    }

    /// The kernel part of a memo key, for any memo of a build that reads `k` only
    /// through its tolerances and context scalars; `None` when the memo is off.
    pub(crate) fn of_kernel(k: &Kernel) -> Option<Self> {
        if !k.memoize_curve_caches {
            return None;
        }
        let c = &k.context;
        Some(Self {
            tolerances: k.tolerances,
            context_bits: [
                c.this_zone_tol_3d().to_bits(),
                c.stored_model_size_estimate().to_bits(),
                c.stored_large_small_size_ratio().to_bits(),
            ],
            context_flags: [c.doing_boolean()],
            fixes: k.fixes,
            owner_zone_tol_bits: None,
        })
    }
}

/// Whether `curve`'s cache build reads nothing but the curve's own value, so that a memo
/// of it is valid for every clone: B-spline, line, ellipse, hyperbola, parabola, Hermite
/// and conic curves, and offset, projected and composite curves made only of such
/// curves. Curves on surfaces, iso curves and curves in volumes hold a surface or volume
/// with cache state of its own (the shared surface cache slot, lazy break and offset
/// caches), so they are never memoised. The walk uses an explicit stack and gives up
/// (no memo) past [`MAX_GEOMETRY_NESTING`] levels.
fn memo_pure(curve: &Curve) -> bool {
    let mut stack: Vec<(&Curve, usize)> = vec![(curve, 0)];
    while let Some((c, depth)) = stack.pop() {
        if depth > MAX_GEOMETRY_NESTING {
            return false;
        }
        match c.kind() {
            CurveKind::BSpline(_)
            | CurveKind::Line(_)
            | CurveKind::Ellipse(_)
            | CurveKind::Hyperbola(_)
            | CurveKind::Parabola(_)
            | CurveKind::Hermite(_)
            | CurveKind::Conic(_) => {}
            CurveKind::Offset(o) => stack.extend(o.base_curve().map(|b| (b, depth + 1))),
            CurveKind::Projected(p) => stack.push((p.curve(), depth + 1)),
            CurveKind::Composite(cc) => {
                stack.extend(cc.segments().iter().map(|seg| (&seg.curve, depth + 1)));
            }
            CurveKind::Iso(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::CrvInVolume(_) => {
                return false;
            }
        }
    }
    true
}

/// One memoised build: its key and the cache, without a handle.
#[derive(Debug)]
struct CurveMemoEntry {
    key: CurveMemoKey,
    cache: CurveCache<()>,
}

/// The memo of a curve's [`CurveCache`] (not upstream; upstream keeps the cache in
/// `SmCacheMgr`, keyed by the curve object). It lives in the [`Curve`] value: a clone
/// shares it (same geometry, same cache), an edit through `&mut Curve` replaces it with
/// an empty one, and it is freed with the last curve that shares it. So there is at
/// most one cache per live curve value. A hit gives what a fresh build gives, so no
/// result depends on the memo, its fill order or which clone filled it. It adds no
/// auto-trait bound ([`Curve`] is `Send` and not `Sync`, as without it) and needs no
/// `unsafe`.
#[derive(Default)]
pub(crate) struct CurveMemo(OnceLock<Arc<Mutex<Option<CurveMemoEntry>>>>);

impl core::fmt::Debug for CurveMemo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // the memo is not part of the curve's observable state
        f.write_str("CurveMemo")
    }
}

impl CurveMemo {
    /// An empty memo of its own.
    pub(crate) const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// A memo shared with this one (for a clone of the curve).
    pub(crate) fn share(&self) -> Self {
        let slot = Arc::clone(self.0.get_or_init(Arc::default));
        let shared = OnceLock::new();
        let _ = shared.set(slot);
        Self(shared)
    }

    /// The memoised cache for `key`, as a copy naming `handle`.
    fn lookup<O: Copy + PartialEq>(
        &self,
        key: &CurveMemoKey,
        handle: Option<O>,
    ) -> Option<CurveCache<O>> {
        let slot = self.0.get()?;
        let entry = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entry
            .as_ref()
            .filter(|e| e.key == *key)
            .and_then(|e| e.cache.retag(handle))
    }

    fn store(&self, key: CurveMemoKey, cache: CurveCache<()>) {
        let slot = self.0.get_or_init(Arc::default);
        let mut entry = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *entry = Some(CurveMemoEntry { key, cache });
    }
}

/// [`make_curve_cache`]'s build, without the memo.
fn build_curve_cache<O: Copy + PartialEq>(
    k: &Kernel,
    curve: &Curve,
    handle: Option<O>,
    zone_tol_3d: Option<f64>,
) -> Result<CurveCache<O>> {
    // cast Object to SmCurve and get its interval
    let ivl = curve.natural_interval();

    // pick dChordHeightTol and dAngularTol tessellation control parameter values

    // get an Idea of the curve's size.
    let length = curve.approximate_length(k, &ivl, 5, false)?.0;
    // (int arithmetic: GetNumberNaturalKnots() - 1, then 30 * NumberOfSpans)
    let number_of_spans = curve.number_natural_knots(k)? as i64 - 1;
    let chord_factor = 250.min(30 * number_of_spans) as f64; // 0.004 of length may be too tight

    // set chordHeight tolerance based on curve size
    let chord_height_tol = length / chord_factor;

    // set angular tolerance
    let angular_tol_deg = 15.0; // 15 degrees might be ok

    // construct new CurveCache Object
    let mut cache = CurveCache::new(
        curve.clone_detached()?,
        ivl,
        CurveCacheOptions {
            make_full_tree: true,
            chord_height_tolerance: chord_height_tol,
            angle_tol_deg: angular_tol_deg,
            off_axis_tolerance: 0.0,
            min_segments: 0,
            compute_aux_tree_data: true,
            max_3d_distance: 0.0,
            min_parametric_ratio: 0.0001,
        },
    );
    cache.set_curve_handle(handle);
    // not upstream: the cached copy has no owner, so the builder gives its edge's
    // tolerance (SmTol::GetApproxTol3d(&m_crCurve) in Tessellate)
    cache.set_owner_zone_tol_3d(zone_tol_3d);

    // build the CruveCache's curve subdivision tree
    cache.tessellate(k)?;
    Ok(cache)
}

impl Curve {
    /// `IsLine(lNumSamples, dTol, ..)`: the line point and vector when the curve is a
    /// straight, constant-speed line ([`BSplineCurve::is_line`] for B-splines,
    /// hyperbolas and parabolas, always for a line, else the base body sampling `num_samples`
    /// points as [`is_line_base`]).
    ///
    /// # Errors
    ///
    /// Those of the overrides and of the curve queries; [`SolidError::Unported`] from
    /// the evaluations.
    pub fn is_line(
        &self,
        k: &Kernel,
        num_samples: u64,
        tol: f64,
    ) -> Result<Option<(Point3, Vec3)>> {
        match self.kind() {
            CurveKind::BSpline(c) => c.is_line(num_samples, tol),
            CurveKind::Hyperbola(c) => c.base().is_line(num_samples, tol),
            CurveKind::Ellipse(c) => c.base().is_line(num_samples, tol),
            CurveKind::Parabola(c) => c.base().is_line(num_samples, tol),
            CurveKind::Line(l) => Ok(Some(l.is_line())),
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => curve_is_line_base(k, self, num_samples, tol),
        }
    }
}

/// Result of [`Curve::is_linear`] (`SmCurve::IsLinear`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Linearity {
    /// The return value: the curve lies on a line (within the tolerance).
    pub linear: bool,
    /// `*pOptLinePoint`: a point on the line, written only when [`Self::linear`]
    /// ([`Vec3::UNDEF`] otherwise).
    pub line_point: Point3,
    /// `*pOptLineTangent`: the unit line direction, written only when
    /// [`Self::linear`] ([`Vec3::UNDEF`] otherwise).
    pub line_tangent: Vec3,
    /// `*pOptActualDeviation`: the largest sample distance from the line (`0` unless
    /// the deviation was asked for and the samples were reached).
    pub deviation: f64,
}

impl Curve {
    /// `SmCurve::IsLinear(dTol, ..)`: whether the curve lies on a line within `tol`.
    /// A B-spline with a NURB and two control points is linear when its start tangent
    /// is longer than `tol` (and `SM_EFF_ZERO`). Otherwise a base line from the start
    /// point (to the end point, or to the 0.38 / 0.62 samples of a closed curve) is
    /// checked at 9 more samples: each must lie within `tol` of the line and, unless
    /// the deviation is asked for, have a tangent parallel to it (1 degree; zero
    /// tangents are skipped). With `want_deviation` the samples only accumulate the
    /// largest distance and the curve is linear when it is below `tol`.
    ///
    /// # Errors
    ///
    /// Those of the control point count; [`SolidError::Unported`] from the
    /// evaluations (their other failures are ignored, as upstream).
    pub fn is_linear(&self, k: &Kernel, tol: f64, want_deviation: bool) -> Result<Linearity> {
        let mut out = Linearity {
            linear: false,
            line_point: Vec3::UNDEF,
            line_tangent: Vec3::UNDEF,
            deviation: 0.0,
        };

        // Special case check: a B-Spline with only two points.
        if self.is_kind_of(BSPLINE_CURVE_TYPE) {
            let has_nurb = match self.kind() {
                CurveKind::BSpline(c) => c.gw_nurb().is_some(),
                CurveKind::Line(c) => c.base().gw_nurb().is_some(),
                CurveKind::Hyperbola(c) => c.base().gw_nurb().is_some(),
                CurveKind::Ellipse(c) => c.base().gw_nurb().is_some(),
                CurveKind::Parabola(c) => c.base().gw_nurb().is_some(),
                _ => false,
            };
            // don't force building Nurb
            if has_nurb && self.number_control_points()? == 2 {
                let mut pv = [Vec3::UNDEF; 2];
                // exact Tangent values (status ignored)
                if let Some(v) =
                    ignored(self.evaluate(k, self.natural_interval().min(), 1, false, false))?
                {
                    for (d, s) in pv.iter_mut().zip(v) {
                        *d = s;
                    }
                }

                let len = pv[1].length();
                if len > tol && len > EFF_ZERO {
                    out.linear = true;
                    out.line_point = pv[0];
                    out.line_tangent = pv[1] / len;
                }
                // else: zero-length line.
                return Ok(out);
            }
        }

        let domain = self.natural_interval();

        // Get a basis: a line that (might) contain the whole curve.
        // First try vector from start to end, and if that doesn't work
        // (closed curve), try a couple of interior points.
        let (start_pt, end_pt) = match ignored(self.ends(k))? {
            Some(e) => (e.start, e.end),
            None => (Vec3::UNDEF, Vec3::UNDEF),
        };
        self.is_linear_after_ends(k, tol, want_deviation, domain, start_pt, end_pt)
    }

    /// General `IsLinear` branch after `GetEnds`, retaining the interval captured
    /// before that call. An owned lazy line can build its owner's NURB in `GetEnds`.
    pub(crate) fn is_linear_after_ends(
        &self,
        k: &Kernel,
        tol: f64,
        want_deviation: bool,
        domain: Extent1d,
        start_pt: Vec3,
        end_pt: Vec3,
    ) -> Result<Linearity> {
        let mut out = Linearity {
            linear: false,
            line_point: Vec3::UNDEF,
            line_tangent: Vec3::UNDEF,
            deviation: 0.0,
        };
        let mut pt = Vec3::UNDEF;
        let mut base_vec = end_pt - start_pt;
        if base_vec.length_squared() < tol * tol {
            if let Some(p) = ignored(self.evaluate_point(k, domain.evaluate(0.38)))? {
                pt = p;
            }
            base_vec = pt - start_pt;
            if base_vec.length_squared() < tol * tol {
                if let Some(p) = ignored(self.evaluate_point(k, domain.evaluate(0.62)))? {
                    pt = p;
                }
                base_vec = pt - start_pt;
                if base_vec.length_squared() < tol * tol {
                    // Four coincident points: must be degenerate.
                    return Ok(out);
                }
            }
        }

        // We have a basis.  Unitize it.
        let Ok(base_vec) = base_vec.unitize() else {
            // Again, must be degenerate w.r.t. SM_EFF_ZERO.
            return Ok(out);
        };

        // Sample at several points looking for off-basis points.
        // Note, as in IsLine(), we also have to check angles. (Helix counterexample.)
        let mut pt_vec = [Vec3::UNDEF; 2];
        let num_samples: u64 = 10;
        let num_spans = (num_samples - 1) as f64; // for convenience

        for ii in 1..num_samples {
            let frac = ii as f64 / num_spans;
            let t = domain.evaluate(frac);
            // nonZeroTangent values (status ignored)
            if let Some(v) = ignored(self.evaluate(k, t, 1, false, true))? {
                for (d, s) in pt_vec.iter_mut().zip(v) {
                    *d = s;
                }
            }
            let (dist, _) = line_point_distance(start_pt, base_vec, pt_vec[0]);

            if want_deviation {
                if dist > out.deviation {
                    out.deviation = dist;
                }
            } else {
                if dist > tol {
                    return Ok(out);
                }

                // Don't return False because of a single zero-length derivative. [B424]
                // (IsParallelTo() returns False for a zero vector.)
                if pt_vec[1].length_squared() <= EFF_ZERO_SQ {
                    // The criterion used in SmParallelTo()
                    continue;
                }

                // Check angle.
                if !pt_vec[1].is_parallel_to(base_vec, 1.0) {
                    return Ok(out);
                }
            }
        } // end for each sample point.

        // Either it's all within tol, or we're calculating pOptActualDeviation.
        let ret = !want_deviation || out.deviation < tol;
        if ret {
            out.linear = true;
            out.line_point = start_pt;
            out.line_tangent = base_vec;
        }
        Ok(out)
    }
}

// ---- sm_FindAdditionalCcTangentGuess (co-ported by unit curve.curve_p7) -------------

/// `sm_FindAdditionalCcTangentGuess`: a second guess `(t1, t2)` near the curve/curve
/// intersection at `(t1, t2)` when the curves cross within 20 degrees: the curves
/// are replaced by their osculating circles (a line where one has no curvature),
/// whose other intersection, stepped back onto the curve tangents
/// ([`convert_to_parameter_delta`]), is the guess, clamped to the intervals. `None`
/// when the crossing is steep, both curves are straight, a circle normal vanishes,
/// the circles do not cross twice, or the guess lies outside both intervals.
///
/// # Errors
///
/// Those of the evaluations, of the unitizations and of the line/circle and
/// circle/circle intersections.
pub fn find_additional_cc_tangent_guess<C1, C2>(
    k: &Kernel,
    curve1: &C1,
    interval1: &Extent1d,
    curve2: &C2,
    interval2: &Extent1d,
    tsect_t1: f64,
    tsect_t2: f64,
) -> Result<Option<(f64, f64)>>
where
    C1: CurveEvaluate + ?Sized,
    C2: CurveEvaluate + ?Sized,
{
    let pv1 = eval2(k, curve1, tsect_t1)?; // NonZeroTangent values
    let pv2 = eval2(k, curve2, tsect_t2)?; // NonZeroTangent values

    // First do a quick test to determine the angle of
    // intersection between the curves and see if we need to look
    // for a tangency.  If within 20 degrees look for tangency.
    if !pv1[1].is_parallel_to(pv2[1], 20.0) {
        return Ok(None);
    }

    // Get the geometric properties of the curves at the intersection
    let geom1 = geom3(k, curve1, tsect_t1)?;
    let mut curvature_vector1 = geom1[2];
    let curve_curvature1 = curvature_vector1.length();
    if curve_curvature1 > EFF_ZERO {
        curvature_vector1 = curvature_vector1.unitize()?;
    }

    let geom2 = geom3(k, curve2, tsect_t2)?;
    let mut curvature_vector2 = geom2[2];
    let curve_curvature2 = curvature_vector2.length();
    if curve_curvature2 > EFF_ZERO {
        curvature_vector2 = curvature_vector2.unitize()?;
    }

    // If the normal curvature is zero then use a line instead of
    // a circle for the surface.
    let mut line_circle_intersection = false;
    let mut circle2_radius = 0.0;
    let mut line_point = Vec3::UNDEF;
    let mut line_vec = Vec3::UNDEF;
    let mut circle2_center = Vec3::UNDEF;
    let mut circle2_normal = Vec3::UNDEF;
    let scaled_tol = 1.0e-4 * (1.0 + curve_curvature1.abs() + curve_curvature2.abs());
    let circle1_radius;
    let circle1_center;
    let mut circle1_normal;
    if curve_curvature1.abs() < scaled_tol {
        // If the curve2 also has zero curvature then we are done
        if curve_curvature2.abs() < scaled_tol {
            // Done with no new guesses needed
            return Ok(None);
        }
        circle1_radius = (1.0 / curve_curvature2).abs();
        circle1_center = pv2[0] + curvature_vector2 * circle1_radius;
        circle1_normal = geom2[1].cross(geom2[2]);
        if circle1_normal.length_squared() < EFF_ZERO_SQ {
            return Ok(None);
        }
        circle1_normal = circle1_normal.unitize()?;
        line_point = pv1[0];
        line_vec = pv1[1].unitize()?;
        line_circle_intersection = true;
    } else {
        // have curve with curvature
        if curve_curvature2.abs() < scaled_tol {
            line_point = pv2[0];
            line_vec = pv2[1].unitize()?;
            line_circle_intersection = true;
        } else {
            circle2_radius = (1.0 / curve_curvature2).abs();
            circle2_center = pv2[0] + curvature_vector2 * circle2_radius;
            circle2_normal = geom2[1].cross(geom2[2]);
            if circle2_normal.length_squared() < EFF_ZERO_SQ {
                return Ok(None);
            }
            circle2_normal = circle2_normal.unitize()?;
        }
        circle1_radius = (1.0 / curve_curvature1).abs();
        circle1_center = pv1[0] + curvature_vector1 * circle1_radius;
        circle1_normal = geom1[1].cross(geom1[2]);
        if circle1_normal.length_squared() < EFF_ZERO_SQ {
            return Ok(None);
        }
        circle1_normal = circle1_normal.unitize()?;
    }
    // (circle 2's normal is computed but not used, as upstream)
    let _ = circle2_normal;

    let found_vector = if line_circle_intersection {
        let tsect = line_coplanar_circle_intersect(
            line_point,
            line_vec,
            circle1_center,
            circle1_normal,
            circle1_radius,
            EFF_ZERO,
        )?;
        // If only have one or zero intersections we are done without
        // picking up extra guess
        let [a0, a1] = match tsect.as_slice() {
            [a0, a1] => [*a0, *a1],
            _ => return Ok(None),
        };
        // Find the other intersection -- i.e. the one we don't have
        // The one we have would be at param 0, so use the farther from zero.
        let mut line_t = a0;
        if a1.abs() > a0.abs() {
            line_t = a1;
        }
        line_t * line_vec
    } else {
        // Do circle circle intersection
        let tsect = circle_coplanar_circle_intersect(
            circle1_normal,
            circle1_center,
            circle1_radius,
            circle2_center,
            circle2_radius,
            EFF_ZERO,
        )?;
        // If only have one or zero intersections we are done without
        // picking up extra guess
        let CircleCircle::Crossing(p0, p1) = tsect else {
            return Ok(None);
        };
        let mut found = p0 - pv1[0];
        if found.length() < p1.distance(pv1[0]) {
            found = p1 - pv1[0];
        }
        found
    };

    let guess1 = tsect_t1 + convert_to_parameter_delta(pv1[1], found_vector);
    let guess2 = tsect_t2 + convert_to_parameter_delta(pv2[1], found_vector);

    if !interval1.contains_value(guess1, 0.0) && !interval2.contains_value(guess2, 0.0) {
        // If the guess is outside of the domain we'll be able to pick it up using
        // another method.
        return Ok(None);
    }
    Ok(Some((
        interval1.clamp_value(guess1),
        interval2.clamp_value(guess2),
    )))
}

/// Position and first derivative (non-zero tangent, from the left).
fn eval2<C: CurveEvaluate + ?Sized>(k: &Kernel, curve: &C, t: f64) -> Result<[Vec3; 2]> {
    let v = curve.evaluate(k, t, 1, true, true)?;
    let mut out = [Vec3::UNDEF; 2];
    for (d, s) in out.iter_mut().zip(v) {
        *d = s;
    }
    Ok(out)
}

/// `EvaluateGeometric(t, 2, TRUE, ...)`: position, tangent, curvature vector and
/// (unwritten) torsion slot.
fn geom3<C: CurveEvaluate + ?Sized>(k: &Kernel, curve: &C, t: f64) -> Result<[Vec3; 4]> {
    let v = evaluate_geometric(k, curve, t, 2, true)?;
    let mut out = [Vec3::UNDEF; 4];
    for (d, s) in out.iter_mut().zip(v) {
        *d = s;
    }
    Ok(out)
}

// ---- CrvsCleanup, RemoveDuplicateCurves ------------------------------------------------

/// `SmCurve::CrvsCleanup(sCrvs, dTol, bDelCrv)`: removes the tiny curves (start,
/// mid (`0.4567`) and end points closer than `tol`) from the ordered chain `curves`
/// and closes the gap between the curves around each removed one
/// ([`BSplineCurve::trim_gap`], both ends moved, status ignored). Returns the number
/// of curves removed (dropped, as upstream with `bDelCrv`) and the input index of
/// every remaining curve.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations and the gap closing;
/// `InvalidInput` when a gap is to be closed between curves that are not
/// [`CurveKind::BSpline`] (upstream casts them to `SmBSplineCurve`).
pub fn crvs_cleanup(k: &Kernel, curves: &mut Vec<Curve>, tol: f64) -> Result<CleanedCurves> {
    // locals
    let mut origin: Vec<usize> = (0..curves.len()).collect();
    let mut ii = 0;
    let mut num_removed: u64 = 0;
    let tol_sq = tol * tol; // for squared test
    let mut end_point1 = Vec3::UNDEF;
    let mut start_point1 = Vec3::UNDEF;
    let mut mid_point = Vec3::UNDEF;

    // for every curve - only increment ii when Curve is NOT removed
    while ii < curves.len() {
        let mut removed = false;
        let crv1 = curves
            .get(ii)
            .ok_or(SolidError::invalid_input(OP_CLEANUP, "curve index"))?;
        let domain1 = crv1.natural_interval();

        // Curve Start and End Points (statuses ignored)
        if let Some(p) = ignored(crv1.evaluate_point(k, domain1.evaluate(0.0)))? {
            start_point1 = p;
        }
        if let Some(p) = ignored(crv1.evaluate_point(k, domain1.evaluate(1.0)))? {
            end_point1 = p;
        }

        // If this is a tiny curve, check midpoint
        if (end_point1 - start_point1).length_squared() < tol_sq {
            // Test for closed curve by examining near midpoint
            if let Some(p) = ignored(crv1.evaluate_point(k, domain1.evaluate(0.4567)))? {
                mid_point = p;
            }

            // skip closed curves by checking mid point spacings
            if (end_point1 - mid_point).length_squared() > tol_sq {
                ii += 1;
                continue;
            }

            // remove small curve from array - compress array afterwards
            curves.remove(ii);
            if ii < origin.len() {
                origin.remove(ii);
            }
            removed = true;
            num_removed += 1;

            // join together adjacent curves across gaps
            if ii > 0 && ii < curves.len() {
                let (head, tail) = curves.split_at_mut(ii);
                let (Some(c0), Some(c1)) = (head.last_mut(), tail.first_mut()) else {
                    return Err(SolidError::invalid_input(OP_CLEANUP, "curve index"));
                };
                let (CurveKind::BSpline(bsp0), CurveKind::BSpline(bsp1)) =
                    (c0.kind_mut(), c1.kind_mut())
                else {
                    return Err(SolidError::invalid_input(
                        OP_CLEANUP,
                        "gap closing needs B-spline curves",
                    ));
                };
                // 0 = change both curve end points to XSect of extended curves
                ignore_status(bsp0.trim_gap(k, bsp1, GapFix::Both, tol))?;
            }
        } // end tiny curve check

        if !removed {
            ii += 1;
        }
    } // end iter every curve - looking short segments

    // all done
    Ok(CleanedCurves {
        removed: num_removed,
        origin,
    })
}

/// Result of [`crvs_cleanup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanedCurves {
    /// The return value: the number of tiny curves removed.
    pub removed: u64,
    /// The input index of every remaining curve (not reported upstream).
    pub origin: Vec<usize>,
}

/// `SmCurve::RemoveDuplicateCurves(rInputCurves)`: drops every curve whose points at
/// 8 fixed fractions of its natural interval match (within `SM_EFF_ZERO_SQ`, squared)
/// those of an earlier curve, and the empty (`NULL`) entries; lists of fewer than two
/// entries are left as they are. Evaluation statuses are ignored (the previous point
/// is kept, as upstream). Returns the input index of every remaining entry (not
/// reported upstream).
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations.
pub fn remove_duplicate_curves(k: &Kernel, curves: &mut Vec<Option<Curve>>) -> Result<Vec<usize>> {
    let num_curves = curves.len();
    if num_curves < 2 {
        return Ok((0..num_curves).collect());
    }

    let mut keep: Vec<usize> = Vec::new();
    let params = [0.0, 1.0, 0.456, 0.687, 0.231, 0.744, 0.355, 0.899];
    let mut p1 = Vec3::UNDEF;
    let mut p2 = Vec3::UNDEF;

    for i in 0..num_curves {
        if curves.get(i).is_none_or(Option::is_none) {
            continue;
        }
        keep.push(i);

        for j in (i + 1)..num_curves {
            let (Some(Some(crv1)), Some(Some(crv2))) = (curves.get(i), curves.get(j)) else {
                continue;
            };

            let ivl1 = crv1.natural_interval();
            let ivl2 = crv2.natural_interval();

            let mut duplicate = true;
            for &t in &params {
                if let Some(p) = ignored(crv1.evaluate_point(k, ivl1.evaluate(t)))? {
                    p1 = p;
                }
                if let Some(p) = ignored(crv2.evaluate_point(k, ivl2.evaluate(t)))? {
                    p2 = p;
                }
                if p1.distance_squared(p2) > EFF_ZERO_SQ {
                    duplicate = false;
                    break;
                }
            }
            if duplicate {
                // then they are duplicates
                if let Some(slot) = curves.get_mut(j) {
                    *slot = None;
                }
            }
        }
    }

    let mut kept = Vec::with_capacity(keep.len());
    for &i in &keep {
        if let Some(slot) = curves.get_mut(i) {
            kept.push(slot.take());
        }
    }
    *curves = kept;
    Ok(keep)
}

// ---- EvaluateSTEPPoint, IsDomainBoundary, IsPeriodic -----------------------------------

impl Curve {
    /// `EvaluateSTEPPoint(dSTEPParameter, rPoint)`: the point at a STEP parameter. The
    /// base class evaluates `EvaluateSTEP` without derivatives (the line, hyperbola
    /// and parabola STEP evaluators, else the NURBS evaluation); the ellipse overrides
    /// it.
    ///
    /// # Errors
    ///
    /// Those of the STEP evaluation.
    pub fn evaluate_step_point(&self, k: &Kernel, step_param: f64) -> Result<Point3> {
        let pts = match self.kind() {
            CurveKind::Ellipse(e) => return e.evaluate_step_point(step_param),
            CurveKind::Line(l) => l.evaluate_step(step_param, 0),
            CurveKind::Hyperbola(h) => h.evaluate_step(step_param, 0)?,
            CurveKind::Parabola(p) => p.evaluate_step(step_param, 0)?,
            _ => evaluate_step(k, self, step_param, 0, true)?,
        };
        Ok(pts.first().copied().unwrap_or(Vec3::UNDEF))
    }

    /// `IsDomainBoundary(rDomain, dTol)`: which side of the 2D `domain` the curve lies
    /// on (`x` / `y` within `tol` of `UMin`, `UMax`, `VMin` or `VMax` at both ends
    /// and at 3 interior samples, whose tangents must also have no component across
    /// that side), else `Neither`. The ellipse and line override it.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations (their other failures are
    /// ignored, as upstream).
    pub fn is_domain_boundary(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        tol: f64,
    ) -> Result<SurfParamType> {
        match self.kind() {
            CurveKind::Ellipse(e) => Ok(e.is_domain_boundary()),
            CurveKind::Line(l) => Ok(l.is_domain_boundary(domain, tol)),
            _ => self.is_domain_boundary_base(k, domain, tol),
        }
    }

    /// The base-class `SmCurve::IsDomainBoundary` (see [`Curve::is_domain_boundary`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn is_domain_boundary_base(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        tol: f64,
    ) -> Result<SurfParamType> {
        let ret = SurfParamType::Neither;

        // First an easy check (GetEnds has no status upstream)
        let (pt0, pt1) = match ignored(self.ends(k))? {
            Some(e) => (e.start, e.end),
            None => (Vec3::UNDEF, Vec3::UNDEF),
        };
        let vec = pt1 - pt0;
        if vec.x.abs() > tol && vec.y.abs() > tol {
            return Ok(ret);
        }

        let u_min = domain.u_min();
        let u_max = domain.u_max();
        let v_min = domain.v_min();
        let v_max = domain.v_max();

        let u_min0 = not_above((pt0.x - u_min).abs(), tol);
        let u_max0 = not_above((pt0.x - u_max).abs(), tol);
        let v_min0 = not_above((pt0.y - v_min).abs(), tol);
        let v_max0 = not_above((pt0.y - v_max).abs(), tol);

        if !(u_min0 || u_max0 || v_min0 || v_max0) {
            return Ok(ret);
        }

        let u_min1 = not_above((pt1.x - u_min).abs(), tol);
        let u_max1 = not_above((pt1.x - u_max).abs(), tol);
        let v_min1 = not_above((pt1.y - v_min).abs(), tol);
        let v_max1 = not_above((pt1.y - v_max).abs(), tol);

        if !(u_min1 || u_max1 || v_min1 || v_max1) {
            return Ok(ret);
        }

        let (ret, prm, idx) = if u_min0 && u_min1 {
            (SurfParamType::UMin, u_min, 0)
        } else if u_max0 && u_max1 {
            (SurfParamType::UMax, u_max, 0)
        } else if v_min0 && v_min1 {
            (SurfParamType::VMin, v_min, 1)
        } else if v_max0 && v_max1 {
            (SurfParamType::VMax, v_max, 1)
        } else {
            return Ok(ret);
        };

        // Start and end are on the same boundary.
        // Test a couple of interior points, including tangents.
        let params = [0.19, 0.47, 0.78];
        let mut pt_der = [Vec3::UNDEF; 2];
        let dom = self.natural_interval();
        for &frac in &params {
            let t = dom.evaluate(frac);
            // (status ignored; `SmCurve::Evaluate`'s default `bNonZeroTangents = TRUE`)
            if let Some(v) = ignored(self.evaluate(k, t, 1, false, true))? {
                for (d, s) in pt_der.iter_mut().zip(v) {
                    *d = s;
                }
            }

            if (pt_der[0].component(idx) - prm).abs() > tol {
                return Ok(SurfParamType::Neither);
            }
            if pt_der[1].component(idx).abs() > tol {
                return Ok(SurfParamType::Neither);
            }
        }

        Ok(ret)
    }

    /// `IsPeriodic(crInterval)` ([`is_periodic`]).
    ///
    /// # Errors
    ///
    /// Those of [`is_periodic`].
    pub fn is_periodic(&self, k: &Kernel, interval: &Extent1d) -> Result<bool> {
        is_periodic(k, self, interval)
    }
}

/// `SmCurve::IsPeriodic(crInterval)`: the curve is closed over `interval` (the
/// virtual `IsClosed`, default tolerance) and its end tangents are within 1 degree.
/// The tangent evaluation statuses are ignored, as upstream.
///
/// # Errors
///
/// Those of the closure test; [`SolidError::Unported`] from the evaluations.
pub fn is_periodic<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
) -> Result<bool> {
    const PERIODICITY_ANGLE_DEGREES: f64 = 1.0;
    if curve.is_closed_virtual(k, interval, 0.0)? {
        // check to see if tangents are nearly aligned
        let mut pv1 = [Vec3::UNDEF; 2];
        let mut pv2 = [Vec3::UNDEF; 2];
        // nonZeroTangent values (statuses ignored)
        if let Some(v) = ignored(curve.evaluate(k, interval.min(), 1, true, true))? {
            for (d, s) in pv1.iter_mut().zip(v) {
                *d = s;
            }
        }
        if let Some(v) = ignored(curve.evaluate(k, interval.max(), 1, true, true))? {
            for (d, s) in pv2.iter_mut().zip(v) {
                *d = s;
            }
        }
        let Ok(angle) = pv1[1].angle_between(pv2[1]) else {
            return Ok(false);
        };
        if angle < PERIODICITY_ANGLE_DEGREES * (PI / 180.0) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `!(value > limit)`: false only when `value` is above `limit` (a NaN passes, as
/// upstream's `if (value > limit) flag = FALSE`).
fn not_above(value: f64, limit: f64) -> bool {
    value.partial_cmp(&limit) != Some(core::cmp::Ordering::Greater)
}

// ---- sm_IsPointSolutionAcrossClosedSeamBoundary -------------------------------------

/// `sm_IsPointSolutionAcrossClosedSeamBoundary(rSol, crThisCurve, crInterval,
/// crOtherCurve, crOtherInterval)`: whether the point solution `sol` of a
/// curve/curve coincidence check lies on an end of the closed `this` curve with the
/// tangents of the two curves (end tangents negated to point into their intervals)
/// opposed: it then joins the wrong side of the seam. `this_is_object0` says
/// whether the solution's first object is `this` (upstream compares the object
/// pointers; otherwise the parameters are swapped).
///
/// # Errors
///
/// Those of the closure test and of the evaluations.
pub fn is_point_solution_across_closed_seam_boundary<O: Copy>(
    k: &Kernel,
    sol: &Solution<O>,
    this: &Curve,
    interval: &Extent1d,
    other: &Curve,
    other_interval: &Extent1d,
    this_is_object0: bool,
) -> Result<bool> {
    let (start1, start2) = if this_is_object0 {
        (sol.start.param(0)?, sol.start.param(1)?)
    } else {
        (sol.start.param(1)?, sol.start.param(0)?)
    };

    // when point solution is on a closedCurve endPoint
    if sol.solution_type == SolutionType::SingleValue
        && this.is_closed(k, interval, 0.0)?
        && interval.is_value_on_boundary(start1, EFF_ZERO).0
    {
        // get Curves' pos and vec at solution point (NonZeroTangent values;
        // statuses ignored)
        let mut pv1 = [Vec3::UNDEF; 2];
        let mut pv2 = [Vec3::UNDEF; 2];
        if let Some(v) = ignored(this.evaluate(k, start1, 1, true, true))? {
            for (d, s) in pv1.iter_mut().zip(v) {
                *d = s;
            }
        }
        if let Some(v) = ignored(other.evaluate(k, start2, 1, true, true))? {
            for (d, s) in pv2.iter_mut().zip(v) {
                *d = s;
            }
        }

        // negate endPoint tangents so that they point into their intervals
        if are_same(start1, interval.max()) {
            pv1[1] = -pv1[1];
        }
        if are_same(start2, other_interval.max()) {
            pv2[1] = -pv2[1];
        }

        // When Curve Tangents oppose - assume sol is connecting
        // different sides of a seam
        if pv1[1].dot(pv2[1]) < 0.0 {
            return Ok(true);
        }
    } // end clsed curve endPoint solution
    Ok(false)
}

// ---- CacheMakeOrValidate ----------------------------------------------------------------

/// `SmCurve::CacheMakeOrValidate` without building: `SM_ERR` for a cache type other
/// than [`ObjectCacheType::Curve`], else the existing cache `old`; `None` when a new
/// cache must be built ([`make_curve_cache`], which needs the curve).
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::Upstream`]) for a cache type other than `SM_OC_CURVE`.
pub fn validate_curve_cache(
    cache_type: ObjectCacheType,
    old: Option<CacheId>,
) -> Result<Option<CacheId>> {
    // when requested ObjectCache Type is not Curve return NULL
    if cache_type != ObjectCacheType::Curve {
        return Err(SolidError::sm_err(OP_CACHE));
    }
    // low work - use OldCache when its given
    Ok(old)
}

// ---- sm_FindAdditionalCpGuess -------------------------------------------------------------

/// `sm_FindAdditionalCpGuess(crCurve, crInterval, crABC, dTsectT, ..)`: a second
/// curve/plane intersection guess near the intersection at `tsect_t` when the curve
/// crosses the plane (normal `abc`) within 10 degrees of tangency: the other
/// intersection of the plane with the osculating circle, stepped back onto the
/// curve tangent ([`convert_to_parameter_delta`]) and clamped to `interval`. `None`
/// for a steep crossing, a straight curve, a vanishing circle normal or line, fewer
/// than two line/circle intersections or a guess outside the interval.
///
/// # Errors
///
/// Those of the evaluations, the unitizations and the line/circle intersection.
pub fn find_additional_cp_guess<C: CurveEvaluate + ?Sized>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    abc: Vec3,
    tsect_t: f64,
) -> Result<Option<f64>> {
    let pv = eval2(k, curve, tsect_t)?; // nonZeroTangent values

    // First do a quick test to determine the angle of
    // intersection between the curve and  and see if we need to look
    // for a tangency.  If within 10 degrees look for tangency.
    if !pv[1].is_perpendicular_to(abc, 10.0) {
        return Ok(None);
    }

    // Get the geometric properties of the curve at the intersection
    let geom_vec = geom3(k, curve, tsect_t)?;
    let mut curvature_vector = geom_vec[2];
    let curve_curvature = curvature_vector.length();
    if curve_curvature > EFF_ZERO {
        curvature_vector = curvature_vector.unitize()?;
    }

    // If the curve curvature is zero then no other intersections near here
    if curve_curvature.abs() < EFF_ZERO {
        return Ok(None);
    }

    // Otherwise intersect plane and circle.
    let circle_radius = (1.0 / curve_curvature).abs();
    let circle_center = pv[0] + curvature_vector * circle_radius;
    let mut circle_normal = geom_vec[1].cross(geom_vec[2]);
    if circle_normal.length_squared() < EFF_ZERO_SQ {
        return Ok(None);
    }
    circle_normal = circle_normal.unitize()?;
    let mut line_vec = abc.cross(circle_normal);
    if line_vec.length_squared() < EFF_ZERO_SQ {
        return Ok(None);
    }
    line_vec = line_vec.unitize_or_z();
    let line_point = pv[0];

    let tsect = line_coplanar_circle_intersect(
        line_point,
        line_vec,
        circle_center,
        circle_normal,
        circle_radius,
        EFF_ZERO,
    )?;
    // If only have one or zero intersections we are done without
    // picking up extra guess
    let [a0, a1] = match tsect.as_slice() {
        [a0, a1] => [*a0, *a1],
        _ => return Ok(None),
    };
    // Find the other intersection -- i.e. the one we don't have
    let mut line_t = a0;
    if a1.abs() > a0.abs() {
        line_t = a1;
    }
    let found_vector = line_t * line_vec;

    let new_t_guess = tsect_t + convert_to_parameter_delta(pv[1], found_vector);

    if !interval.contains_value(new_t_guess, 0.0) {
        // If the guess is outside of the domain we'll be able to pick it up using
        // another method.
        return Ok(None);
    }
    Ok(Some(interval.clamp_value(new_t_guess))) // Found a guess
}

// ---- IsArc --------------------------------------------------------------------------------

/// `SmCurve::IsArc(lNumberOfSamplePoints, dTol3d, ..)` (the base-class body) for
/// `curve`, whose virtual `IsDegenerate(dTol3d)` is `is_degenerate`: the circle
/// through the points at `0`, `0.45` and `0.9` of the natural interval, accepted when
/// `num_samples` evenly spaced samples (from the second) lie within `tol` of its
/// radius and plane. Returns the frame (X towards the start point, Y towards the
/// `0.45` point), the average radius, the start angle `0` and the end angle in
/// degrees (`(0, 360]`; near-zero snaps to `360`); `None` for a degenerate curve,
/// coincident or collinear samples, a radius too large for the arc length, or a
/// failed sample.
///
/// # Errors
///
/// Those of `is_degenerate`; [`SolidError::Unported`] from the evaluations (their
/// other failures are ignored where upstream ignores them, or mean "not an arc").
pub fn is_arc_base<C, D>(
    k: &Kernel,
    curve: &C,
    is_degenerate: D,
    num_samples: u64,
    tol: f64,
) -> Result<Option<ArcData>>
where
    C: CurveEvaluate + ?Sized,
    D: FnOnce(&C, f64) -> Result<bool>,
{
    // no work - degenerate curve
    if is_degenerate(curve, tol)? {
        return Ok(None);
    }

    // get 3 samplePoints [.00, .45, .90] from the curve
    // Don't use [0, .5, 1] in case of closed curve.
    let interval = curve.natural_interval();
    let p1 = ignored(curve.evaluate_point(k, interval.evaluate(0.00)))?.unwrap_or(Vec3::UNDEF);
    let p2 = ignored(curve.evaluate_point(k, interval.evaluate(0.45)))?.unwrap_or(Vec3::UNDEF);
    let p3 = ignored(curve.evaluate_point(k, interval.evaluate(0.90)))?.unwrap_or(Vec3::UNDEF);

    // get vectors between sample points
    let v1 = p2 - p1;
    let v2 = p3 - p1;
    let approx_length_of_arc = p1.distance(p2) + p2.distance(p3);

    // no work - sample points have no separation (probably a degenerate curve)
    if v1.length_squared() < EFF_ZERO_SQ {
        return Ok(None);
    }

    // create coordinate system: xAxis = sV1, XYPlane = sV1/sV2 plane
    let Ok([x, y, z]) = v1.make_unit_ortho_vectors(Some(v2)) else {
        // fail when sV1/sV2 don't specify a unique coordinate system
        return Ok(None);
    };

    // place coordinate system in SmAxis2Placement object (status ignored)
    let mut plane = Axis2Placement::default();
    let _ = plane.set_canonical(p1, x, y);

    // drop 3 points to the plane (they should already be on the plane to tolerances)
    let uv_p1 = plane_drop_point(&plane, p1);
    let uv_p2 = plane_drop_point(&plane, p2);
    let uv_p3 = plane_drop_point(&plane, p3);

    // get circle center from 3Points
    let Ok(uv_circle_center) = circle_center_from_3_points(uv_p1, uv_p2, uv_p3) else {
        return Ok(None);
    };

    let center = plane_evaluate_point(&plane, uv_circle_center);

    // get circle radius
    let radius = center.distance(p1);

    // failure - radius is too large
    if radius > approx_length_of_arc / EFF_ZERO_SQRT {
        // Size of radius too big to make an accurate arc.
        return Ok(None);
    }

    // Now test each point to see if it is correct distance from
    // center of circle and on plane.
    let mut ave_radius = radius;
    for i in 1..num_samples {
        // get next samplePoint param value
        let param = if i == num_samples - 1 {
            interval.max()
        } else {
            interval.evaluate(i as f64 / (num_samples as f64 - 1.0))
        };

        // get next samplePoint
        let Some(pnt) = ignored(curve.evaluate_point(k, param))? else {
            return Ok(None);
        };

        // get samplePoint/CircleCenter distance
        let test_radius = pnt.distance(center);

        // failure - dist is not close to current radius value
        if (test_radius - radius).abs() > tol {
            return Ok(None);
        }

        ave_radius += test_radius;

        // get dist to plane
        let Ok(dist_to_plane) = plane_point_distance(center, z, pnt) else {
            return Ok(None);
        };

        // failure - samplePoint is too far from circle plane
        if dist_to_plane > tol {
            // Not in plane
            return Ok(None);
        }
    } // end iter every samplePoint

    // get average radius size
    let out_radius = ave_radius / num_samples as f64;

    // If we made it to here then we have a circle.
    let center_to_p1 = p1 - center;
    let center_to_p2 = p2 - center;

    // create final coordinate system - xAxis = CenterPoint to SP1
    //                                  yAxis = perp to xAxis in direction of sCenterToP2
    let Ok([x, y, z]) = center_to_p1.make_unit_ortho_vectors(Some(center_to_p2)) else {
        return Ok(None);
    };

    let mut frame = Axis2Placement::default();
    let _ = frame.set_canonical(center, x, y);

    // arc start is 0.0
    let start_ang_deg = 0.0;

    // get arc stop in degrees (statuses ignored)
    let end = ignored(curve.evaluate_point(k, interval.max()))?.unwrap_or(Vec3::UNDEF);
    let v_to_end = end - center;
    let mut end_ang_deg = z.ccw_angle_between(x, v_to_end).unwrap_or(0.0);
    end_ang_deg = end_ang_deg * 180.0 / PI;

    // snap near closed circles to closed circles and
    // make interval positive
    if end_ang_deg.abs() < EFF_ZERO_SQRT {
        end_ang_deg = 360.0;
    } else if end_ang_deg < 0.0 {
        end_ang_deg += 360.0;
    }

    // all done
    Ok(Some((frame, out_radius, start_ang_deg, end_ang_deg)))
}

impl Curve {
    /// `IsArc(lNumberOfSamplePoints, dTol3d, ..)`: the circular arc the curve is (see
    /// [`is_arc_base`]); never for a line, parabola or hyperbola; a circle is its own
    /// arc ([`crate::curve::ellipse::Ellipse::is_arc`]).
    ///
    /// # Errors
    ///
    /// Those of [`is_arc_base`].
    pub fn is_arc(&self, k: &Kernel, num_samples: u64, tol: f64) -> Result<Option<ArcData>> {
        match self.kind() {
            CurveKind::Line(_) | CurveKind::Parabola(_) | CurveKind::Hyperbola(_) => Ok(None),
            CurveKind::Ellipse(e) => e.is_arc(k, num_samples, tol),
            CurveKind::BSpline(c) => is_arc_base(
                k,
                c,
                |c, tol| c.is_degenerate(k, tol, None),
                num_samples,
                tol,
            ),
            _ => is_arc_base(
                k,
                self,
                |c, tol| c.is_degenerate(k, tol, None),
                num_samples,
                tol,
            ),
        }
    }
}

// ---- Length -------------------------------------------------------------------------------

impl Curve {
    /// `Length(crInterval, dDesiredAccuracy)`: the arc length over `interval` (the
    /// line override, else [`Curve::length_base`]).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::length_base`].
    pub fn length(&self, k: &Kernel, interval: &Extent1d, desired_accuracy: f64) -> Result<f64> {
        self.length_with_zone_tol(k, interval, desired_accuracy, None)
    }

    /// Arc length with the owning edge's resolved zone tolerance, independent of
    /// the desired integration accuracy. Retains the virtual line override.
    pub(crate) fn length_with_zone_tol(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        desired_accuracy: f64,
        owner_zone_tol_3d: Option<f64>,
    ) -> Result<f64> {
        match self.kind() {
            CurveKind::Line(l) => Ok(l.length(interval)),
            _ => self.length_base_with_zone_tol(k, interval, desired_accuracy, owner_zone_tol_3d),
        }
    }

    /// The base-class `SmCurve::Length`: the sum over the spans of a curve cache
    /// (chord height `ApproximateLength(5) / 10`, 90 degrees) of the span chord for a
    /// degree 1 curve, else the Romberg integral of `|C'|`; the end point distance
    /// for an interval shorter than `SM_EFF_ZERO` or a curve that does not
    /// tessellate (kinds not derived from `SmBSplineCurve`).
    ///
    /// # Errors
    ///
    /// Those of the length estimate and of the degree query;
    /// [`SolidError::Unported`] from the tessellation or curve evaluator.
    pub fn length_base(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        desired_accuracy: f64,
    ) -> Result<f64> {
        self.length_base_with_zone_tol(k, interval, desired_accuracy, None)
    }

    fn length_base_with_zone_tol(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        desired_accuracy: f64,
        owner_zone_tol_3d: Option<f64>,
    ) -> Result<f64> {
        // Now compute a reasonable a chord height tolerance based on
        // approximate total size of the curve.
        // Sample at 5 points to get an Idea of how big the curve is.
        let length = self.approximate_length(k, interval, 5, false)?.0;

        let chord_height_tol = length / 10.0; // 0.01 of length may be too tight

        // Pick an angular tolerance - need to experiment a little to see
        // which values are optimal for the length function.
        let angular_tol = 90.0; // 90 degrees might be ok

        // Create the cache (over the curve itself upstream: an exact copy that keeps
        // the owner, not the copy constructor's fixed-up knots)
        let mut cache: CurveCache<()> = CurveCache::new(
            self.clone_for_evaluation()?,
            *interval,
            CurveCacheOptions {
                make_full_tree: true,
                chord_height_tolerance: chord_height_tol,
                angle_tol_deg: angular_tol,
                off_axis_tolerance: 0.0,
                min_segments: 0,
                compute_aux_tree_data: false,
                max_3d_distance: 0.0,
                min_parametric_ratio: 0.0001,
            },
        );
        cache.set_owner_zone_tol_3d(owner_zone_tol_3d);
        let degenerate = interval.length() < EFF_ZERO;
        let tessellated = degenerate || ignored(cache.tessellate(k))?.is_some();

        // for degenerate intervals and for all curves that don't tessellate
        //  note: only curves derived from SmBSplineCurve tessellate
        if degenerate || !tessellated {
            // let the length equal the end-point distance (statuses ignored)
            let p1 = ignored(self.evaluate_point(k, interval.min()))?.unwrap_or(Vec3::UNDEF);
            let p2 = ignored(self.evaluate_point(k, interval.max()))?.unwrap_or(Vec3::UNDEF);
            return Ok(p1.distance(p2));
        }

        let mut total_length = 0.0;
        let span_count = cache.span_count();
        // (dSpanAccuracy = dDesiredAccuracy / span count: the integration tolerance)
        let span_accuracy = desired_accuracy / span_count as f64;

        let bsp = cache.curve();
        for i in 0..span_count {
            let bez_elem = cache.span(i)?;
            let (p1, p2, seg_ivl) = bez_elem.aux.ends_on(k, bsp)?;

            // Linear segment
            if bsp.degree()? == 1 {
                // Just need to sum start and end point
                total_length += p1.distance(p2);
                continue;
            }

            // Greater than degree 1 if make it to here
            // Do an integration to get the length of the
            // first derivatives integrated over the entire curve.
            total_length += crate::mass::integrator::integrate(
                seg_ivl.min(),
                seg_ivl.max(),
                crate::mass::integrator::IntegrationAlgorithm::Romberg,
                span_accuracy,
                |t| {
                    let pv = bsp.evaluate(k, t, 1, true, false)?;
                    pv.get(1)
                        .map(|v| v.length())
                        .ok_or_else(|| SolidError::sm_err("curve::length"))
                },
            )?;
        }

        Ok(total_length)
    }
}

// ---- LocalPropertyAnalysis --------------------------------------------------------------

/// `SmCurve::LocalPropertyAnalysis(crInterval, eCurveProperty, dGuessParameter,
/// cpdPropertyValue, cpOptVectors, rbFoundAnswer, rSolution)`: a Newton (or Brent,
/// for the minimise / maximise properties) search from `guess` on `interval` for a
/// zero of the curve property ([`CurvePropertyEfo`]), accuracy `SM_EFF_ZERO_SQ`,
/// accepted at `SM_EFF_ZERO_SQRT`. Returns whether an answer was found; then
/// `solution` is a single value at the parameter with the property value (for the
/// angle properties the signed angle, for the projected point properties the
/// projected distance). An empty `vectors` stands for `NULL`. The min / max
/// properties always report the point they stopped at.
///
/// # Errors
///
/// `SM_ERR` when the solver fails (an evaluation error or the iteration limit);
/// `InvalidInput` for missing option vectors that the angle and projected point
/// properties read; [`SolidError::Unported`] from the evaluations.
pub fn local_property_analysis<C: CurveEvaluate + ?Sized, O: Copy>(
    k: &Kernel,
    curve: &C,
    interval: &Extent1d,
    property: CurvePropertyType,
    guess: f64,
    value: Option<f64>,
    vectors: &[Vec3],
    solution: &mut Solution<O>,
) -> Result<bool> {
    let vec = |i: usize| -> Result<Vec3> {
        vectors
            .get(i)
            .copied()
            .ok_or(SolidError::invalid_input(OP_LPA, "missing option vector"))
    };

    // construct CurveProperty evaluator object
    let opt_vectors = if vectors.is_empty() {
        None
    } else {
        Some(vectors)
    };
    let mut efo = CurvePropertyEfo::new(k, curve, property, value, opt_vectors, EFF_ZERO_SQ);

    // construct NewtonRaphson solver
    // Note, since this is a local solve, we don't want it to jump seams. [B90]
    let set_periodic = false; // IsPeriodic( crInterval );

    let mut ls = LocalSolve1d::with_interval(*interval, set_periodic);
    // Not upstream (WR-253, source issue #481): opt-in, a failed evaluation
    // (e.g. the radius of curvature of a straight piece) is no sign change
    ls.set_failed_guess_no_sign_change(k.fixes.oscillating_skips_failed_guesses);

    // select the solver and the desired accuracy.
    if property == CurvePropertyType::MinimizeFirstDerivative
        || property == CurvePropertyType::MinimizeRadiusOfCurvature
    {
        ls.set_solver_algorithm(SolverAlgorithm::BrentMinimize);
    }
    // Note: the objective functions for these two are set up with
    // the reciprocal of the property to be maximized,
    // so we minimize those functions.
    if property == CurvePropertyType::MaximizeFirstDerivative
        || property == CurvePropertyType::MaximizeRadiusOfCurvature
    {
        ls.set_solver_algorithm(SolverAlgorithm::BrentMinimize); // not MAXIMIZE.
    }

    // Set desired accuracy for very tight convergence.
    ls.set_desired_accuracy(EFF_ZERO_SQ);

    // solve with NewtonRaphson iteration
    let outcome = match ls.solve_it(&mut efo, guess, EFF_ZERO_SQRT) {
        Ok(o) => o,
        Err(e) if e.is_unported() => return Err(e),
        // Something went wrong in 1d solver return error
        Err(_) => return Err(SolidError::sm_err(OP_LPA)),
    };
    let found_t = outcome.t;

    // when no solution was found
    if !outcome.found {
        // If we terminate for minimization or maximization problem
        // for any normal reason then we don't need to fail.
        if property != CurvePropertyType::MinimizeFirstDerivative
            && property != CurvePropertyType::MaximizeFirstDerivative
            && property != CurvePropertyType::MinimizeRadiusOfCurvature
            && property != CurvePropertyType::MaximizeRadiusOfCurvature
        {
            return Ok(false);
        }
    } // end no solution check

    // else a solution was found - set the outputs
    solution.solution_type = SolutionType::SingleValue;
    *solution.start.param_mut(0)? = found_t;
    solution.start.solution_value = ls.curr.f;

    // For certain operations we need to adjust the returned solution value.
    // for min directed angle problems
    if property == CurvePropertyType::MinimizeAngleToPlane
        || property == CurvePropertyType::MinimizeDirectedAngle
        || property == CurvePropertyType::MinimizeCcwAngle
    {
        // get the solution point and the vector between it and testPoint projected to the appropriate plane
        let found_pnt = ignored(curve.evaluate_point(k, found_t))?.unwrap_or(Vec3::UNDEF);
        let mut v = found_pnt - vec(0)?;
        v = vec(1)?.cross(v).cross(vec(1)?);

        // when the projected vector is not zeroLength
        if v.length_squared() > EFF_ZERO_SQ {
            v = v.unitize()?;

            // compute and save appropriate signed angle as solution value
            let cos_theta = if property == CurvePropertyType::MinimizeAngleToPlane {
                PI / 2.0 - acos(v.dot(vec(2)?).abs())
            } else {
                // SM_CP_MINIMIZE_DIRECTED_ANGLE (status ignored)
                let mut c = vec(1)?.ccw_angle_between(vec(2)?, v).unwrap_or(0.0);
                if c > PI {
                    c = 2.0 * PI - c; // make range 0 to -180
                }
                if property == CurvePropertyType::MinimizeCcwAngle && c < 0.0 {
                    c += 2.0 * PI;
                }
                c
            };
            solution.start.solution_value = cos_theta;
        } // end nonZeroLength projected solution vector check
    } // end min directed angle problem check to set solution angle values

    // for Projected point cases
    if property == CurvePropertyType::ProjectedPointMinimize
        || property == CurvePropertyType::ProjectedPointMaximize
    {
        // get the solution point and the vector between it and testPoint projected to the appropriate plane
        let found_pnt = ignored(curve.evaluate_point(k, found_t))?.unwrap_or(Vec3::UNDEF);
        let v = found_pnt - vec(0)?;
        let v_proj = vec(1)?.cross(v).cross(vec(1)?);

        // set solutionValue = length projected vector
        solution.start.solution_value = v_proj.length();
    } // end projected point problem check to set solution ProjectedVector length

    Ok(true)
}

// ---- SmCCIntersectENFO::Evaluate ---------------------------------------------------------

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> CcIntersectEnfo<'_, C1, C2> {
    /// `SmCCIntersectENFO::Evaluate`: at `x = [t1, t2]`, `F = [gap . u1, gap . u2]`
    /// with `gap = C1 - C2` and the unit tangents `u` ([`unitize_tangent`]; the
    /// Jacobian's second order terms limited to half the first order ones), or the
    /// tangent intersection equations ([`CcTangentIntEnfo::static_evaluate`], status
    /// ignored) when the gap is below `0.1` and the tangents are parallel within
    /// 0.001 degree. Records the gap length as the solution size. Returns
    /// `rbFoundAnswer` (false).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `x` or `f` shorter than 2 (upstream asserts their size);
    /// those of the evaluations and of the Jacobian writes.
    pub fn evaluate_equations(
        &mut self,
        k: &Kernel,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let (Some(&x0), Some(&x1)) = (x.first(), x.get(1)) else {
            return Err(SolidError::invalid_input(OP_CCI, "need two parameters"));
        };
        if f.len() < 2 {
            return Err(SolidError::invalid_input(
                OP_CCI,
                "need two function values",
            ));
        }

        // init output
        let mut found = false;

        // Evaluate both curves; need two derivatives.
        let pvv1: [Vec3; 3] = eval_n(k, self.curve1, x0, 2, true, true)?; // nonZeroTangent values
        let pvv2: [Vec3; 3] = eval_n(k, self.curve2, x1, 2, true, true)?; // nonZeroTangent values

        // get the gap - save the SolutionSize
        let gap_vec = pvv1[0] - pvv2[0];
        self.solution_size = gap_vec.length();
        self.scaled_zero =
            EFF_ZERO * (1.0 + max2(pvv1[0].max_dimension(), pvv2[0].max_dimension()));

        // Check for nearby tangent intersection cases - this allows
        // large near-maximum conditions to 'fall off' the hill. [B98 B115 B567]
        let switch_to_tan_fcns = true;

        let ang_lim_deg = 0.001; // See note above.
        let dist_lim_sq = 0.01;
        let is_tangent = switch_to_tan_fcns
            && gap_vec.length_squared() < dist_lim_sq
            && pvv1[1].is_parallel_to(pvv2[1], ang_lim_deg);

        // when near a tangent curve/curve intersection
        if is_tangent {
            // switch to the TangentIntersection Solver Objective Functions
            // (status ignored)
            if let Some(r) = ignored(CcTangentIntEnfo::static_evaluate(
                k,
                self.curve1,
                self.curve2,
                x,
                f,
                jacobian,
            ))? {
                found = r;
            }
        } else {
            // the usual case, not tangent.
            // Get the unitized derivatives.
            let (c1_t_unit, _) = unitize_tangent(pvv1, -gap_vec);
            let (c2_t_unit, _) = unitize_tangent(pvv2, gap_vec);

            // Function values:
            if let [f0, f1, ..] = f {
                *f0 = gap_vec.dot(c1_t_unit);
                *f1 = gap_vec.dot(c2_t_unit);
            }

            // Derivatives:
            if let Some(j) = jacobian {
                // Deriv of the unitized C_t  ( C_t / || C_t || )
                let c1_t_unit_t = pvv1[1].unitized_derivative(pvv1[2]);
                let c2_t_unit_t = pvv2[1].unitized_derivative(pvv2[2]);

                let term11 = pvv1[1].dot(c1_t_unit);
                let mut term12 = gap_vec.dot(c1_t_unit_t);
                let term21 = -pvv2[1].dot(c2_t_unit);
                let mut term22 = gap_vec.dot(c2_t_unit_t);

                if term12 < -term11 / 2.0 {
                    // note, dTerm11 > 0 always
                    term12 = -term11 / 2.0;
                }
                if term22 > -term21 / 2.0 {
                    // note, dTerm21 < 0 always
                    term22 = -term21 / 2.0;
                }
                j.set_at(0, 0, term11 + term12)?;
                j.set_at(0, 1, -pvv2[1].dot(c1_t_unit))?;
                j.set_at(1, 0, pvv1[1].dot(c2_t_unit))?;
                j.set_at(1, 1, term21 + term22)?;
            }
        } // end not near tangent branch

        Ok(found)
    }
}

// ---- SmFindCCNormalENFO::Evaluate --------------------------------------------------------

impl<C1: CurveEvaluate + ?Sized, C2: CurveEvaluate + ?Sized> FindCcNormalEnfo<'_, C1, C2> {
    /// `SmFindCCNormalENFO::Evaluate`: the curve/curve extremum equations of the
    /// solver operation at `x = [t1, t2]` (normalise / minimise / maximise by
    /// default: both tangents perpendicular to the gap; projected tangency, projected
    /// and directed min / max, at distance, signed pivot and surface projected
    /// intersection have their own equations, see upstream). Counts the evaluations
    /// with a Jacobian; on the first one (and from then on once triggered) a gap
    /// below the squared tolerance is moved off by a unit curvature vector. Returns
    /// `rbFoundAnswer`: the (projected) gap or its projections on both tangents are
    /// below the squared tolerance (normalise and projected min / max only).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for (nearly) zero tangents, a missing option vector where upstream
    /// checks it, a projected gap below the tolerance (projected tangency), a
    /// failed unitization (at distance) or a second curve that is not a curve on a
    /// surface (surface projected intersection); `InvalidInput` for `x` or `f`
    /// shorter than 2 and for a missing pivot point (signed pivot, which upstream
    /// reads unchecked); those of the evaluations and of the Jacobian writes.
    #[allow(clippy::too_many_lines)] // one upstream function, one branch per operation
    pub fn evaluate_equations(
        &mut self,
        k: &Kernel,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let (Some(&x0), Some(&x1)) = (x.first(), x.get(1)) else {
            return Err(SolidError::invalid_input(OP_NORMAL, "need two parameters"));
        };
        if f.len() < 2 {
            return Err(SolidError::invalid_input(
                OP_NORMAL,
                "need two function values",
            ));
        }
        let opt_vec = |i: usize| -> Result<Vec3> {
            self.opt_vectors
                .and_then(|v| v.get(i).copied())
                .ok_or(SolidError::sm_err(OP_NORMAL))
        };

        // count iterations
        if jacobian.is_some() {
            self.count += 1;
        }

        // init output
        let mut found = false;

        // curve evals
        let pvv1: [Vec3; 3] = eval_n(k, self.curve1, x0, 2, true, true)?; // this curve  nonZeroTangent values
        let pvv2: [Vec3; 3] = eval_n(k, self.curve2, x1, 2, true, true)?; // other curve nonZeroTangent values

        // curve/curve distance
        let mut vec_f_minus_g = pvv1[0] - pvv2[0];

        // failure - zero length tangents
        let tan_len_sq1 = pvv1[1].length_squared();
        let tan_len_sq2 = pvv2[1].length_squared();

        if tan_len_sq1 < EFF_ZERO_SQ || tan_len_sq2 < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_NORMAL));
        }

        // Try a little trick here if the distance between them
        // gets very small.  Try to translate a curve by its
        // curvature vector in the appropriate direction.
        if (self.count == 1 || self.step_back)
            && vec_f_minus_g.length_squared() < self.squared_tolerance
            && self.solver_operation != SolverOperation::AtDistance
            && self.solver_operation != SolverOperation::ProjectedTangency
        {
            self.step_back = true;
            let mut trans = Vec3::new(0.0, 0.0, 0.0);

            // curve 1 curvature
            let pv = geom3(k, self.curve1, x0)?;
            if pv[2].length_squared() > EFF_ZERO_SQ {
                trans = pv[2].unitize_or_z();
            } else {
                // curve 2 curvature
                let pv = geom3(k, self.curve2, x1)?;
                if pv[2].length_squared() > EFF_ZERO_SQ {
                    trans = pv[2].unitize_or_z();
                }
            }

            if trans.dot(vec_f_minus_g) >= 0.0 {
                vec_f_minus_g = pvv1[0] + trans - pvv2[0];
            } else {
                vec_f_minus_g = pvv1[0] - trans - pvv2[0];
            }
        } // end m_lCount == 1 || m_bStepBack check

        let zero_vec = Vec3::new(0.0, 0.0, 0.0);
        let values: [f64; 2];
        let mut jac: Option<[[f64; 2]; 2]> = None;
        let want_jac = jacobian.is_some();

        match self.solver_operation {
            SolverOperation::ProjectedTangency => {
                let n = opt_vec(0)?;
                // fun[0] = (f' x v) . (f' x v)
                // fun[1] = (g' x v) . (g' x v)
                let fp = n.cross(pvv1[1]).cross(n);
                let fpp = n.cross(pvv1[2]).cross(n);
                let gp = n.cross(pvv2[1]).cross(n);
                let gpp = n.cross(pvv2[2]).cross(n);
                let v = n.cross(vec_f_minus_g).cross(n);

                // If projection of vector between curves is too small just error out
                if v.length_squared() < self.squared_tolerance {
                    return Err(SolidError::sm_err(OP_NORMAL));
                }

                let f0 = fp.cross(v);
                let f1 = gp.cross(v);
                let f0df = fp.derivative_of_cross_product(v, fpp, fp);
                let f0dg = fp.derivative_of_cross_product(v, zero_vec, -gp);
                let f1df = gp.derivative_of_cross_product(v, zero_vec, fp);
                let f1dg = gp.derivative_of_cross_product(v, gpp, -gp);

                if want_jac {
                    jac = Some([
                        [2.0 * f0.dot(f0df), 2.0 * f0.dot(f0dg)],
                        [2.0 * f1.dot(f1df), 2.0 * f1.dot(f1dg)],
                    ]);
                }

                values = [f0.dot(f0), f1.dot(f1)];
            } // end SM_SO_PROJECTED_TANGENCY

            SolverOperation::ProjectedMinimize | SolverOperation::ProjectedMaximize => {
                let n = opt_vec(0)?;
                // fun[0] = |f'| . v ; fun[1] = |g'| . v (unit projected tangents)
                let v = n.cross(vec_f_minus_g).cross(n);
                let fp = n.cross(pvv1[1]).cross(n);
                let fpp = n.cross(pvv1[2]).cross(n);
                let gp = n.cross(pvv2[1]).cross(n);
                let gpp = n.cross(pvv2[2]).cross(n);

                let v_dot_v = v.dot(v);
                let v_dot_fp = v.dot(fp);
                let v_dot_fpp = v.dot(fpp);
                let v_dot_gp = v.dot(gp);
                let v_dot_gpp = v.dot(gpp);
                let fp_dot_fp = fp.dot(fp);
                let fp_dot_fpp = fp.dot(fpp);
                let fp_dot_gp = fp.dot(gp);
                let gp_dot_gp = gp.dot(gp);
                let gp_dot_gpp = gp.dot(gpp);
                let fp_len = sqrt(fp_dot_fp);
                let gp_len = sqrt(gp_dot_gp);

                if want_jac {
                    // projected tangent might be zero
                    let row0 = if fp_len > EFF_ZERO {
                        [
                            ((v_dot_fpp + fp_dot_fp) * fp_len - (v_dot_fp * fp_dot_fpp) / fp_len)
                                / fp_dot_fp,
                            -fp_dot_gp / fp_len,
                        ]
                    } else {
                        [0.0, 0.0]
                    };

                    // projected tangent might be zero
                    let row1 = if gp_len > EFF_ZERO {
                        [
                            fp_dot_gp / gp_len,
                            ((v_dot_gpp - gp_dot_gp) * gp_len - (v_dot_gp * gp_dot_gpp) / gp_len)
                                / gp_dot_gp,
                        ]
                    } else {
                        [0.0, 0.0]
                    };
                    jac = Some([row0, row1]);
                }

                // fun[0] = f'(v), fun[1] = g'(v)
                values = [
                    if fp_len > EFF_ZERO {
                        fp.dot(v) / fp_len
                    } else {
                        0.0
                    },
                    if gp_len > EFF_ZERO {
                        gp.dot(v) / gp_len
                    } else {
                        0.0
                    },
                ];

                if v_dot_v < self.squared_tolerance {
                    found = true;
                } else {
                    let proj_dist_squared1 = (values[0] * values[0]) / v_dot_v;
                    let proj_dist_squared2 = (values[1] * values[1]) / v_dot_v;

                    if proj_dist_squared1 < self.squared_tolerance
                        && proj_dist_squared2 < self.squared_tolerance
                    {
                        found = true;
                    }
                }
            } // end SM_SO_PROJECTED_MINIMIZE or SM_SO_PROJECTED_MAXIMIZE

            SolverOperation::SignedDirectedMinimize
            | SolverOperation::DirectedMinimize
            | SolverOperation::DirectedMaximize => {
                let n = opt_vec(0)?;
                let dir = opt_vec(1)?;
                // fun[0] = (f' x N) . g'
                // fun[1] = (f-g) . V
                let vec_f_minus_g_local = pvv1[0] - pvv2[0];
                let v = dir.cross(n);
                let fpxn = pvv1[1].cross(n);

                if want_jac {
                    let fpxnp = pvv1[1].derivative_of_cross_product(n, pvv1[2], zero_vec);
                    jac = Some([
                        [fpxnp.dot(pvv2[1]), fpxn.dot(pvv2[2])],
                        [pvv1[1].dot(v), -pvv2[1].dot(v)],
                    ]);
                }

                values = [fpxn.dot(pvv2[1]), vec_f_minus_g_local.dot(v)];
            } // end SM_SO_SIGNED_DIRECTED_MINIMIZE or SM_SO_DIRECTED_MINIMIZE or SM_SO_DIRECTED_MAXIMIZE

            SolverOperation::AtDistance => {
                // fun[0] = g'/|g'| . (g-f)
                // fun[1] = (g-f)^2 - c^2
                let vec_g_minus_f = pvv2[0] - pvv1[0];

                let _ufp = pvv1[1]
                    .unitize()
                    .map_err(|_| SolidError::sm_err(OP_NORMAL))?;
                let ugp = pvv2[1]
                    .unitize()
                    .map_err(|_| SolidError::sm_err(OP_NORMAL))?;
                let _ufpp = pvv1[1].unitized_derivative(pvv1[2]);
                let ugpp = pvv2[1].unitized_derivative(pvv2[2]);

                if want_jac {
                    jac = Some([
                        [
                            -ugp.dot(pvv1[1]),
                            ugpp.dot(vec_g_minus_f) + ugp.dot(pvv2[1]),
                        ],
                        [
                            -2.0 * pvv1[1].dot(vec_g_minus_f),
                            2.0 * pvv2[1].dot(vec_g_minus_f),
                        ],
                    ]);
                }

                // function values
                values = [
                    ugp.dot(vec_g_minus_f),
                    vec_g_minus_f.dot(vec_g_minus_f) - self.squared_at_distance,
                ];
            } // end SM_SO_AT_DISTANCE

            SolverOperation::SignedPivotMinimize => {
                // (upstream reads the vectors without a NULL check)
                let vecs = self.opt_vectors.ok_or(SolidError::invalid_input(
                    OP_NORMAL,
                    "signed pivot needs the option vectors",
                ))?;
                let (Some(&n), Some(&pivot)) = (vecs.first(), vecs.get(1)) else {
                    return Err(SolidError::invalid_input(
                        OP_NORMAL,
                        "signed pivot needs the option vectors",
                    ));
                };
                let fp = n.cross(pvv1[1]).cross(n);
                let fpp = n.cross(pvv1[2]).cross(n);
                let gp = n.cross(pvv2[1]).cross(n);
                let gpp = n.cross(pvv2[2]).cross(n);
                // (statuses ignored)
                let ufp = fp.unitize_or_z();
                let ugp = gp.unitize_or_z();
                let piv_f = n.cross(pvv1[0] - pivot).cross(n);
                let piv_g = n.cross(pvv2[0] - pivot).cross(n);
                let mut sign = 1.0;
                if fp.dot(piv_f) * gp.dot(piv_g) < 0.0 {
                    // Flip the tangent vector so that two tangents point to same 'side'
                    sign = -1.0;
                }
                let ufpp = fp.unitized_derivative(fpp);
                let ugpp = gp.unitized_derivative(gpp);
                if want_jac {
                    jac = Some([
                        [2.0 * piv_f.dot(fp), -2.0 * piv_g.dot(gp)],
                        [
                            fp.dot(ufp) + piv_f.dot(ufpp),
                            -sign * (gp.dot(ugp) + piv_g.dot(ugpp)),
                        ],
                    ]);
                }

                // Compute function values
                values = [
                    piv_f.dot(piv_f) - piv_g.dot(piv_g),
                    piv_f.dot(ufp) - sign * piv_g.dot(ugp),
                ];
            } // end SM_SO_SIGNED_PIVOT_MINIMIZE

            SolverOperation::SurfaceProjectedIntersect => {
                let Some(crv_on_surf) = self.curve2.as_crv_on_surf() else {
                    return Err(SolidError::sm_err(OP_NORMAL));
                };

                let ct = pvv1[1];
                let ss = pvv2[1]; // deriv of 3-d surface curve w.r.t. s

                let gap = -vec_f_minus_g;

                // Surface quantities (status ignored)
                let (u_ders, v_ders) =
                    ignored(crv_on_surf.evaluate_surface_1st_deriv_derivs(k, x1, false))?
                        .unwrap_or(([Vec3::UNDEF; 2], [Vec3::UNDEF; 2]));
                let [su, sus] = u_ders;
                let [sv, svs] = v_ders;

                // Unitize derivatives (statuses ignored):
                let su_unit = su.unitize_or_z();
                let su_unit_s = su.unitized_derivative(sus);
                let sv_unit = sv.unitize_or_z();
                let sv_unit_s = sv.unitized_derivative(svs);

                // Compute function values
                values = [gap.dot(su_unit), gap.dot(sv_unit)];

                if want_jac {
                    jac = Some([
                        [-ct.dot(su_unit), ss.dot(su_unit) + gap.dot(su_unit_s)],
                        [-ct.dot(sv_unit), ss.dot(sv_unit) + gap.dot(sv_unit_s)],
                    ]);
                }
            } // end SM_SO_SURFACE_PROJECTED_INTERSECT

            _ => {
                // use normalization function
                // fun[0] = f'(f-g)
                // fun[1] = g'(f-g)
                if want_jac {
                    let j01 = -pvv1[1].dot(pvv2[1]);
                    jac = Some([
                        [pvv1[2].dot(vec_f_minus_g) + pvv1[1].dot(pvv1[1]), j01],
                        [-j01, pvv2[2].dot(vec_f_minus_g) - pvv2[1].dot(pvv2[1])],
                    ]);
                }

                values = [pvv1[1].dot(vec_f_minus_g), pvv2[1].dot(vec_f_minus_g)];

                if vec_f_minus_g.length_squared() < self.squared_tolerance {
                    found = true;
                } else {
                    let proj_dist_squared1 =
                        (values[0] * values[0]) / vec_f_minus_g.length_squared();
                    let proj_dist_squared2 =
                        (values[1] * values[1]) / vec_f_minus_g.length_squared();

                    if proj_dist_squared1 < self.squared_tolerance
                        && proj_dist_squared2 < self.squared_tolerance
                    {
                        found = true;
                    }
                }
            } // end default
        } // end switch on m_eSolverOperation

        if let (Some(j), Some(m)) = (jacobian, jac) {
            for (r, row) in m.iter().enumerate() {
                for (c, &v) in row.iter().enumerate() {
                    j.set_at(r, c, v)?;
                }
            }
        }
        if let [f0, f1, ..] = f {
            *f0 = values[0];
            *f1 = values[1];
        }

        // all done
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::geom::local_solve1d::{EvalFunction, FunctionValue};
    use crate::geom::local_solve_nd::EvalNFunctions;
    use crate::nurbs::b_spline_curve::NurbCircleParam;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;

    /// A degree 1 B-spline from `a` to `b`.
    fn bseg(a: [f64; 3], b: [f64; 3]) -> Curve {
        let mut cur = allocate_nurb_curve(1, 1, 3).unwrap();
        for (d, s) in cur.cpts_mut().iter_mut().zip([a, b]) {
            d.x = s[0];
            d.y = s[1];
            d.z = s[2];
            d.w = -f64::MAX;
        }
        cur.knots_mut().copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    fn hermite(p1: [f64; 3], d1: [f64; 3], p2: [f64; 3], d2: [f64; 3]) -> Curve {
        Curve::from(HermiteCurve::new(
            Vec3::from_array(p1),
            Vec3::from_array(d1),
            Vec3::from_array(p2),
            Vec3::from_array(d2),
            3,
        ))
    }

    fn arc(radius: f64, a0: f64, a1: f64) -> Curve {
        let frame = Axis2Placement::new(
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        Curve::from(
            BSplineCurve::create_circle_segment(
                3,
                &frame,
                radius,
                a0,
                a1,
                NurbCircleParam::Quadratic,
            )
            .unwrap(),
        )
    }

    #[test]
    fn cleanup_removes_tiny_curves() {
        let k = Kernel::default();
        let mut curves = vec![
            bseg([0.0, 0.0, 0.0], [5.0, 0.0, 0.0]),
            bseg([5.0, 0.0, 0.0], [5.0, 1e-4, 0.0]),
            bseg([5.0, 0.0, 0.0], [5.0, 3.0, 0.0]),
        ];
        let r = crvs_cleanup(&k, &mut curves, 1e-3).unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(r.origin, vec![0, 2]);
        assert_eq!(curves.len(), 2);
    }

    #[test]
    fn duplicates_are_removed() {
        let k = &Kernel::default();
        let mut curves = vec![
            Some(bseg([0.0; 3], [1.0, 0.0, 0.0])),
            None,
            Some(bseg([0.0; 3], [1.0, 0.0, 0.0])),
            Some(bseg([0.0; 3], [0.0, 1.0, 0.0])),
        ];
        let origin = remove_duplicate_curves(k, &mut curves).unwrap();
        assert_eq!(origin, vec![0, 3]);
        assert_eq!(curves.len(), 2);
        let mut single = vec![None];
        assert_eq!(remove_duplicate_curves(k, &mut single).unwrap(), vec![0]);
        assert_eq!(single.len(), 1);
    }

    #[test]
    fn arc_and_line_recognition() {
        let k = &Kernel::default();
        let c = arc(3.0, 0.0, 90.0);
        let (frame, r, s, e) = c.is_arc(k, 5, 1e-9).unwrap().unwrap();
        assert!((r - 3.0).abs() < 1e-9);
        assert!(frame.origin().distance(Vec3::new(1.0, 2.0, 0.0)) < 1e-9);
        assert_eq!(s, 0.0);
        assert!((e - 90.0).abs() < 1e-6, "{e}");
        let l = bseg([0.0; 3], [2.0, 1.0, 0.0]);
        assert!(l.is_arc(k, 5, 1e-9).unwrap().is_none());
        assert!(l.is_line(k, 5, 1e-9).unwrap().is_some());
        assert!(c.is_line(k, 5, 1e-9).unwrap().is_none());
    }

    #[test]
    fn periodicity_and_domain_boundary() {
        let k = &Kernel::default();
        let full = arc(2.0, 0.0, 360.0);
        assert!(full.is_periodic(k, &full.natural_interval()).unwrap());
        let half = arc(2.0, 0.0, 180.0);
        assert!(!half.is_periodic(k, &half.natural_interval()).unwrap());
        let domain = Extent2d::from_bounds(-5.0, -3.0, 5.0, 3.0);
        let edge = bseg([-5.0, -3.0, 0.0], [-5.0, 3.0, 0.0]);
        assert_eq!(
            edge.is_domain_boundary(k, &domain, 1e-12).unwrap(),
            SurfParamType::UMin
        );
        let inner = bseg([0.0, -3.0, 0.0], [0.0, 3.0, 0.0]);
        assert_eq!(
            inner.is_domain_boundary(k, &domain, 1e-12).unwrap(),
            SurfParamType::Neither
        );
    }

    #[test]
    fn length_of_linear_and_curved_spans() {
        let k = Kernel::default();
        let l = bseg([0.0; 3], [3.0, 4.0, 0.0]);
        let len = l
            .length(&k, &Extent1d::from_min_max(0.0, 1.0), 1e-8)
            .unwrap();
        assert!((len - 5.0).abs() < 1e-12);
        // a degenerate interval: the end point distance
        let h = hermite([0.0; 3], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, -1.0, 0.0]);
        let r = h.length(&k, &Extent1d::from_min_max(0.5, 0.5), 1e-8);
        assert_eq!(r.unwrap(), 0.0);
        let c = arc(1.0, 0.0, 90.0);
        let length = c.length(&k, &c.natural_interval(), 1e-8).unwrap();
        assert!((length - core::f64::consts::FRAC_PI_2).abs() < 1e-8);
    }

    #[test]
    fn point_inversion_and_plane_guess() {
        let k = &Kernel::default();
        let h = hermite([0.0; 3], [3.0, 3.0, 0.0], [4.0, 0.0, 0.0], [3.0, -3.0, 0.0]);
        let mut sol: Solution<()> = Solution::new();
        let target = h.evaluate_point(k, 0.3).unwrap();
        let found = local_property_analysis(
            k,
            &h,
            &Extent1d::from_min_max(0.0, 1.0),
            CurvePropertyType::PointInversion,
            0.5,
            None,
            &[target],
            &mut sol,
        )
        .unwrap();
        assert!(found);
        assert!((sol.start.param(0).unwrap() - 0.3).abs() < 1e-7);
        // a plane nearly tangent at the apex of the hermite: no extra guess for a
        // perpendicular crossing
        let g = find_additional_cp_guess(
            k,
            &h,
            &Extent1d::from_min_max(0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            0.5,
        )
        .unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn curve_curve_equations() {
        let k = &Kernel::default();
        let a = bseg([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        let b = bseg([1.0, -1.0, 0.0], [1.0, 1.0, 0.0]);
        let mut e = CcIntersectEnfo::new(k, &a, &b, EFF_ZERO_SQ);
        let mut f = [9.0; 2];
        let mut j = Matrix::new(2, 2);
        assert!(!e.evaluate(&[0.5, 0.5], &mut f, Some(&mut j)).unwrap());
        assert!(f[0].abs() < 1e-15 && f[1].abs() < 1e-15);
        assert!(e.is_converged());
        let mut n = FindCcNormalEnfo::new(k, &a, &b, 1e-12, 0.0, SolverOperation::Normalize, None);
        let mut f = [9.0; 2];
        assert!(n.evaluate_equations(k, &[0.25, 0.5], &mut f, None).is_ok());
        assert_eq!(n.count, 0);
        assert!(n
            .evaluate_equations(k, &[0.25, 0.5], &mut f, Some(&mut j))
            .is_ok());
        assert_eq!(n.count, 1);
        // a missing pivot point is invalid input, a missing plane normal SM_ERR
        let mut p = FindCcNormalEnfo::new(
            k,
            &a,
            &b,
            1e-12,
            0.0,
            SolverOperation::SignedPivotMinimize,
            None,
        );
        assert!(matches!(
            p.evaluate_equations(k, &[0.25, 0.5], &mut f, None),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut q = FindCcNormalEnfo::new(
            k,
            &a,
            &b,
            1e-12,
            0.0,
            SolverOperation::ProjectedMinimize,
            None,
        );
        assert!(matches!(
            q.evaluate_equations(k, &[0.25, 0.5], &mut f, None),
            Err(SolidError::Upstream { .. })
        ));
    }

    #[test]
    fn seam_and_cache_validation() {
        let k = &Kernel::default();
        let full = arc(2.0, 0.0, 360.0);
        let ivl = full.natural_interval();
        let line = bseg([3.0, -1.0, 0.0], [3.0, 5.0, 0.0]);
        let mut sol: Solution<()> = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.start.parameters[0] = ivl.max();
        sol.start.parameters[1] = 0.5;
        let r = is_point_solution_across_closed_seam_boundary(
            k,
            &sol,
            &full,
            &ivl,
            &line,
            &line.natural_interval(),
            true,
        );
        assert!(r.is_ok());
        assert!(validate_curve_cache(ObjectCacheType::Brep, None).is_err());
        assert_eq!(
            validate_curve_cache(ObjectCacheType::Curve, None).unwrap(),
            None
        );
        let pt = line.evaluate_step_point(k, 0.5).unwrap();
        assert!(pt.distance(Vec3::new(3.0, 2.0, 0.0)) < 1e-12);
    }

    /// A closed cubic through 8 points of an ellipse.
    fn ellipse_spline() -> Curve {
        let mut pts: Vec<Point3> = (0..8)
            .map(|i| {
                let t = core::f64::consts::PI * f64::from(i) / 4.0;
                Point3::new(5.0 * t.cos(), 3.0 * t.sin(), 0.5 * t)
            })
            .collect();
        pts.push(pts[0]);
        let c = BSplineCurve::interpolate_points(
            &pts,
            None,
            3,
            None,
            None,
            true,
            crate::nurbs::crv_fit::p1::ParamMethod::ChordLength,
        )
        .unwrap();
        Curve::from(c)
    }

    #[test]
    fn memo_hit_is_the_fresh_build() {
        let k = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        let off = Kernel {
            memoize_curve_caches: false,
            ..Kernel::default()
        };
        for c in [
            bseg([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]),
            ellipse_spline(),
            hermite([0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ] {
            let fresh = format!("{:?}", make_curve_cache(&off, &c, Some(7u32)).unwrap());
            let key = CurveMemoKey::of(&k, &c, None).unwrap();
            assert!(c.memo().lookup(&key, None::<u32>).is_none());
            let miss = format!("{:?}", make_curve_cache(&k, &c, Some(7u32)).unwrap());
            assert!(c.memo().lookup(&key, None::<u32>).is_some());
            let hit = format!("{:?}", make_curve_cache(&k, &c, Some(7u32)).unwrap());
            // another handle space and a clone: the same as their fresh builds
            let copy = c.clone();
            let hit_copy = format!("{:?}", make_curve_cache(&k, &copy, Some(-1i64)).unwrap());
            let fresh_copy = format!("{:?}", make_curve_cache(&off, &copy, Some(-1i64)).unwrap());
            assert_eq!(miss, fresh);
            assert_eq!(hit, fresh);
            assert_eq!(hit_copy, fresh_copy);
            // the memo was off for `off`, so it stored nothing
            assert!(CurveMemoKey::of(&off, &c, None).is_none());
        }
    }

    #[test]
    fn memo_follows_edits_and_kernel_state() {
        let k = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        let mut c = ellipse_spline();
        let key = CurveMemoKey::of(&k, &c, None).unwrap();
        let _ = make_curve_cache(&k, &c, None::<u32>).unwrap();
        let copy = c.clone();
        assert!(copy.memo().lookup(&key, None::<u32>).is_some());
        // an edit of one copy empties its memo only
        c.reverse_parameterization(&k, &c.natural_interval())
            .unwrap();
        assert!(c.memo().lookup(&key, None::<u32>).is_none());
        assert!(copy.memo().lookup(&key, None::<u32>).is_some());
        let off = Kernel {
            memoize_curve_caches: false,
            ..Kernel::default()
        };
        let edited = format!("{:?}", make_curve_cache(&k, &c, None::<u32>).unwrap());
        let edited_fresh = format!("{:?}", make_curve_cache(&off, &c, None::<u32>).unwrap());
        assert_eq!(edited, edited_fresh);
        // other tolerances do not match the stored key
        let mut loose = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        loose.tolerances.zone_tol_3d *= 10.0;
        let loose_key = CurveMemoKey::of(&loose, &copy, None).unwrap();
        assert!(copy.memo().lookup(&loose_key, None::<u32>).is_none());
        // a Hermite curve is a pure value and memoised
        let h = hermite([0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(CurveMemoKey::of(&k, &h, None).is_some());
    }

    #[test]
    fn owner_zone_tol_sets_the_approximation_tolerance_and_the_memo_key() {
        let k = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        // at 1e10 the Bezier spans' rounding (above half the system tolerance) splits
        // spans, half an edge tolerance of 1 does not; a Hermite curve, since spans cut
        // from a B-spline skip the distance check (v1.0.1)
        let big = hermite(
            [1.2e9, 0.0, 0.0],
            [3e10, 6e10, 0.0],
            [6.1e10, 0.0, 2e10],
            [3e10, -6e10, 3e10],
        );
        let system = make_curve_cache(&k, &big, None::<u32>).unwrap();
        let edge = make_curve_cache_with_zone_tol(&k, &big, None::<u32>, Some(1.0)).unwrap();
        assert!(system.span_count() > edge.span_count());
        // each tolerance has its own key: no hit across them
        let again = make_curve_cache(&k, &big, None::<u32>).unwrap();
        assert_eq!(again.span_count(), system.span_count());
        let key = CurveMemoKey::of(&k, &big, Some(1.0)).unwrap();
        assert!(big.memo().lookup(&key, None::<u32>).is_none());
    }

    /// A planar degree-4 B-spline `C(t) = (x(s(t)), s(t), 0)` over `[-1, 12]` with
    /// `x(s) = (1-s)^3 - 2(1-s) + 2` integrated, so `C'(t) . (1, 0, 0) =
    /// (1-s)^3 - 2(1-s) + 2` where `s(t) = t`: from 0, Newton on it cycles between
    /// 0 and 1 (`F = 1, F' = -1` at 0; `F = 2, F' = 2` at 1; the mirror of the
    /// `t^3 - 2t + 2` cycle) without a root nearby (`F > 0.9` on `[0, 1]`). `s` is
    /// piecewise linear with C0 joints: it stalls (`C' = 0`, so the perpendicular
    /// property cannot be evaluated) on `[stall, 0.992]`, catches up on
    /// `[0.992, 0.996]`, and is `s = t` elsewhere. The pieces holding 0 and 1 do not
    /// depend on `stall`, so neither do the Newton iterates.
    fn stalled_quartic(stall: f64) -> Curve {
        let (a, b, c) = (stall, 0.992, 0.996);
        // (s0, s1) per Bezier piece; the knots are -1, 0.5, a, b, c, 12
        let pieces = [(-1.0, 0.5), (0.5, a), (a, a), (a, c), (c, 12.0)];
        // x(s) = -(1-s)^4/4 + (1-s)^2 + 2s and its derivatives
        let x = |s: f64| {
            (-((1.0 - s) * (1.0 - s) * (1.0 - s) * (1.0 - s)) / 4.0 + (1.0 - s) * (1.0 - s))
                + 2.0 * s
        };
        let dx = |s: f64| ((1.0 - s) * (1.0 - s) * (1.0 - s) - 2.0 * (1.0 - s)) + 2.0;
        let d2x = |s: f64| -3.0 * (1.0 - s) * (1.0 - s) + 2.0;
        let d3x = |s: f64| 6.0 * (1.0 - s);
        let binom = |n: usize, k: usize| -> f64 {
            (0..k).fold(1.0, |acc, j| acc * (n - j) as f64 / (j + 1) as f64)
        };
        let mut cps = Vec::new();
        for (i, &(s0, s1)) in pieces.iter().enumerate() {
            // Bezier points of q(u) = P(s0 + d u) from its Taylor coefficients
            let d = s1 - s0;
            let cx = [
                x(s0),
                dx(s0) * d,
                d2x(s0) / 2.0 * d * d,
                d3x(s0) / 6.0 * d * d * d,
                -(d * d * d * d) / 4.0,
            ];
            let cy = [s0, d, 0.0, 0.0, 0.0];
            for j in usize::from(i > 0)..=4 {
                let bx: f64 = (0..=j).map(|k| binom(j, k) / binom(4, k) * cx[k]).sum();
                let by: f64 = (0..=j).map(|k| binom(j, k) / binom(4, k) * cy[k]).sum();
                cps.push([bx, by]);
            }
        }
        let mut cur = allocate_nurb_curve(20, 4, 25).unwrap();
        for (dst, src) in cur.cpts_mut().iter_mut().zip(&cps) {
            dst.x = src[0];
            dst.y = src[1];
            dst.z = 0.0;
            dst.w = -f64::MAX;
        }
        let mut knots = vec![-1.0; 5];
        for t in [0.5, a, b, c] {
            knots.extend([t; 4]);
        }
        knots.extend([12.0; 5]);
        cur.knots_mut().copy_from_slice(&knots);
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    /// `PerpendicularToVector` to `(1, 0, 0)` on [`stalled_quartic`] from 0.
    fn perpendicular_on_stalled_quartic(k: &Kernel, stall: f64) -> Result<(bool, f64, f64)> {
        let curve = stalled_quartic(stall);
        let mut sol: Solution<()> = Solution::new();
        let found = local_property_analysis(
            k,
            &curve,
            &Extent1d::from_min_max(-1.0, 12.0),
            CurvePropertyType::PerpendicularToVector,
            0.0,
            None,
            &[Vec3::new(1.0, 0.0, 0.0)],
            &mut sol,
        )?;
        Ok((found, sol.start.param(0).unwrap(), sol.start.solution_value))
    }

    /// Where the Newton cycle of [`stalled_quartic`] puts `SolveOscillating`'s last
    /// interior guess (0.99 of the way from 0 to 1, as the iterates round): the
    /// stall starts exactly there, so that guess alone cannot be evaluated, and
    /// every point the bisection tries below it can.
    const STALL_AT_LAST_GUESS: f64 = 0.989_999_999_999_999_2;

    /// WR-253 (source issue #481) through the Kernel: with
    /// `fixes.oscillating_skips_failed_guesses` the failed last guess is no sign
    /// change, so the perpendicular solve on a curve whose `F` stays above 0.9
    /// finds nothing.
    #[test]
    fn issue_481_kernel_fix_skips_the_failed_last_guess() {
        let mut k = Kernel::default();
        k.fixes.oscillating_skips_failed_guesses = true;
        let (found, _, _) = perpendicular_on_stalled_quartic(&k, STALL_AT_LAST_GUESS).unwrap();
        assert!(!found);
    }

    /// The default (upstream) reads the failed guess's zeroed `F` as a sign
    /// change, brackets it, and reports a root exactly at the unevaluable point,
    /// with value 0, although `F` is near 1.98 on both sides of it.
    #[test]
    fn oscillating_failed_guess_is_a_root_by_default() {
        let k = Kernel::default();
        assert!(!k.fixes.oscillating_skips_failed_guesses);
        let (found, t, value) = perpendicular_on_stalled_quartic(&k, STALL_AT_LAST_GUESS).unwrap();
        assert!(found);
        assert_eq!(t.to_bits(), STALL_AT_LAST_GUESS.to_bits());
        assert_eq!(value, 0.0);
        // the point really cannot be evaluated, and its neighbour below is far from 0
        let curve = stalled_quartic(STALL_AT_LAST_GUESS);
        let v = [Vec3::new(1.0, 0.0, 0.0)];
        let mut efo = CurvePropertyEfo::new(
            &k,
            &curve,
            CurvePropertyType::PerpendicularToVector,
            None,
            Some(&v),
            EFF_ZERO_SQ,
        );
        let mut out = FunctionValue::default();
        assert!(efo.evaluate(STALL_AT_LAST_GUESS, &mut out, true).is_err());
        efo.evaluate(
            f64::from_bits(STALL_AT_LAST_GUESS.to_bits() - 1),
            &mut out,
            true,
        )
        .unwrap();
        assert!(out.f > 1.9, "{out:?}");
        // a stall that misses the guess changes nothing: both settings find no root
        let mut fixed = Kernel::default();
        fixed.fixes.oscillating_skips_failed_guesses = true;
        for k in [&Kernel::default(), &fixed] {
            assert!(!perpendicular_on_stalled_quartic(k, 0.991).unwrap().0);
        }
    }
}
