//! Curves on surfaces: a 3D curve defined as a 2D curve traced through the parameter
//! space of a surface, `C(t) = S(uv(t))` (`SmCrvOnSurf`).
//!
//! Upstream: `source/SMLib/src/SmCrvOnSurf.cpp`, `source/SMLib/inc/SmCrvOnSurf.h`.
//!
//! | Rust                                            | Upstream                                       |
//! |-------------------------------------------------|------------------------------------------------|
//! | [`CrvOnSurf`]                                   | `SmCrvOnSurf`                                  |
//! | [`CrvOnSurf::new`]                              | `SmCrvOnSurf(rUVCurve, rSurface, ...)`, `SetAll` |
//! | [`CrvOnSurf::from_uv_line`]                     | `SmCrvOnSurf(rStartUV, rEndUV, rSurface, ...)` |
//! | [`CrvOnSurf::empty`]                            | `SmCrvOnSurf()` (empty constructor for I/O)    |
//! | [`CrvOnSurf::copy`], `Clone`                    | copy constructor, `Copy`                       |
//! | `Drop` (implicit)                               | `~SmCrvOnSurf`                                 |
//! | [`CrvOnSurf::approx_eq`]                        | `operator==`                                   |
//! | [`CrvOnSurf::approximate_curve`]                | `ApproximateCurve`                             |
//! | [`CrvOnSurf::edit_parameterization`]            | `EditParameterization`                         |
//! | [`CrvOnSurf::evaluate`]                         | `Evaluate`                                     |
//! | [`CrvOnSurf::evaluate_point`]                   | `EvaluatePoint`                                |
//! | [`CrvOnSurf::evaluate_uv_point`]                | `EvaluateUVPoint`                              |
//! | [`CrvOnSurf::evaluate_surface_normal`]          | `EvaluateSurfaceNormal`                        |
//! | [`CrvOnSurf::evaluate_surface_1st_deriv_derivs`] | `EvaluateSurface1stDerivDerivs`               |
//! | [`CrvOnSurf::calculate_bounding_box`]           | `CalculateBoundingBox`                         |
//! | [`CrvOnSurf::calculate_continuities`]           | `CalculateContinuities`                        |
//! | [`CrvOnSurf::drop_point`]                       | `DropPoint`                                    |
//! | [`CrvOnSurf::knots`]                            | `GetKnots`                                     |
//! | [`CrvOnSurf::number_natural_knots`]             | `GetNumberNaturalKnots`                        |
//! | [`CrvOnSurf::get_or_create_bs_approx`]          | `GetOrCreateBSApproxPointer`                   |
//! | [`CrvOnSurf::set_uv_curve`]                     | `SetUVCurve`                                   |
//! | [`CrvOnSurf::reverse_parameterization`]         | `ReverseParameterization`                      |
//! | [`CrvOnSurf::trim`]                             | `Trim`                                         |
//! | [`CrvOnSurf::transform`]                        | `Transform`                                    |
//! | `eval_dist_and_dev` (private)                   | `sm_EvalDistAndDev` (static)                   |
//! | [`CrvOnSurf::surface_curve_max_distance_between`], [`SurfaceCurveGap`] | `SurfaceCurveMaxDistanceBetween` |
//! | [`CrvOnSurf::read_from_db`]                     | `ReadFromDB`                                   |
//! | [`CrvOnSurf::natural_interval`], [`CrvOnSurf::uv_curve`], [`CrvOnSurf::surface`], [`CrvOnSurf::bs_approx_max_gap_3d`] | `GetNaturalInterval`, `GetUVCurve`, `GetBaseSurface`, `GetBSApproxMaxGap3d` |
//! | [`CrvOnSurf::is_analytic`], [`CrvOnSurf::is_bounded`] | `IsAnalytic`, `IsBounded`                |
//! | [`CrvOnSurf::TYPE`], [`CrvOnSurf::type_code`], [`CrvOnSurf::type_string`], [`CrvOnSurf::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`CrvOnSurf::is_kind_of`]                       | `IsKindOf`                                     |
//! | `SetContext` (dropped)                          | `SetContext`                                   |
//!
//! Deviations:
//! - **Surface.** `m_pSurface` is an `SmSurface*`, held by value as a [`Surface`].
//!   Its concrete virtual dispatch (`GetNaturalUVDomain`, `Evaluate`, `EvaluatePoint`,
//!   `EvaluateNormal`, `Evaluate2ndDerivatives`, `operator==`, `Copy`, and
//!   `Transform`) is retained. `ReadFromDB` reads through
//!   [`Surface::read_from_db`]. The derivative lifts are those of
//!   [`crate::surface::surface::p2`].
//! - **Ownership.** The UV curve and the surface are held by value. Upstream shares or
//!   owns (and maybe copies) them as the owner flag says; the flag is kept as data and
//!   decides what upstream observes: the copies made for flags 1 and 2 (`Copy`), whether
//!   `Trim` and `ReverseParameterization` are allowed, whether `Transform` moves the
//!   surface, and the owner flag of a copy. The UV-line constructor copies the surface
//!   for flag 2 and then discards the copy, as upstream. The copy constructor copies both
//!   parts when it owns the curve. Unowned parts and temporary `this` copies use
//!   exact snapshots, retaining their NURBS data and owner chains; native owned
//!   branches still use virtual `Copy`. Notifications and contexts are not modelled.
//! - **Caches.** Upstream's const knot queries update the break cache. Here that
//!   value-only cache uses interior mutability; no cache borrow spans a geometry
//!   call. `breaks()` returns an owned snapshot. Shared [`Curve`] knot dispatch
//!   retains these writes. The approximation cache (`m_pBSApproxCurve`,
//!   `m_dMaxGap3d`, `m_bParametrizationMatches`) is interior-mutable the same way:
//!   approximation queries use `&mut self`, and their `*_ref` variants run on an
//!   exact copy and then keep its break and approximation caches on this curve, as
//!   upstream's const `DropPoint` and `CalculateBoundingBox` write them. The cached
//!   approximation is handed out as an exact copy of the stored one (upstream hands
//!   out the object itself).
//! - **Base-class calls.** `SmCurve::ApproximateCurve`, `SmCurve::DropPoint` and
//!   `SmCurve::CalculateBoundingBox` on `this` run on a [`Curve`] wrapping a copy of
//!   this object ([`Curve::approximate_curve_base`], [`Curve::drop_point_base`],
//!   [`calculate_bounding_box_base`]). The base drop's actual break-cache state is
//!   copied back even on failure; local solves do not force a cache build. The base
//!   drop's global solve reads the curve cache the caller keeps for this curve
//!   ([`Curve::drop_point_in`] and `GlobalCoincidenceChecker`, as for every other kind;
//!   upstream keeps it on the curve), not one built per drop on the copy; a caller
//!   without one reads the cache kept on this object (built by its first global solve
//!   under the same kernel state, dropped by every edit; a copy starts without it), as
//!   upstream's one cache per curve object. Where
//!   upstream falls back to the base class
//!   after a failed approximation (`CalculateBoundingBox`, `DropPoint`), a
//!   [`SolidError::Unported`] failure is propagated instead.
//! - **Out-params.** Outputs are returned: `GetKnots` returns the multiplicities
//!   always (upstream fills them only when asked; the knots do not depend on it);
//!   `EvaluateSurfaceNormal` returns the UV point and tangent with the normal.
//!   Derivatives `Evaluate` cannot compute (the surface gave fewer) are
//!   [`Vec3::UNDEF`] (upstream leaves the caller's values).
//! - **Requested derivative storage.** `Evaluate` accepts orders through the shared
//!   [`crate::curve::MAX_EVAL_DERIVS`] bound, retaining zero derivatives above third order.
//!   Larger requests return [`SolidError::InvalidInput`] after the bounded UV
//!   and surface evaluations, preserving their error precedence. Upstream writes
//!   into caller-owned storage without a bound; the port must bound allocation
//!   and zero-filling work, well above the native fixture orders (at most six).
//! - **Ignored statuses.** Where upstream ignores a status (`GetKnots` of the UV curve
//!   and of this curve in `CalculateContinuities`, `GetNumberNaturalKnots`,
//!   `GetOrCreateBSApproxPointer` and `SurfaceCurveMaxDistanceBetween`, the
//!   approximation's `Transform`) the port ignores it too except for
//!   [`SolidError::Unported`]. `EvaluateContinuity`'s status (ignored upstream, leaving
//!   an uninitialised continuity) and a failing `Copy` of a part are propagated.
//! - **Undefined behaviour.** Where upstream reads outside its arrays (`GetKnots` with
//!   no UV knots or no classification interval, `CalculateContinuities` without
//!   knots) the port returns [`SolidError::InvalidInput`]. `EvaluateSurface1stDerivDerivs`'
//!   zero fill never runs upstream (`for (ii = 2; ii > lNumDers; ii++)` inside
//!   `if (lNumDers > 1)`); the port returns the two derivatives it computes.
//! - **`SurfaceCurveMaxDistanceBetween`** forces `bDoPrecise = FALSE` before its
//!   precise refinement upstream, so that code (`SmSurface::LocalCurveSolve` and
//!   `SmCurve::LocalCurveSolve`) is dead and not called.
//!   Object identity (`&crOtherCurve == this`) is [`core::ptr::eq`].
//! - **`ReadFromDB`** always builds a new object (the "fill the curve passed in"
//!   branch and its `IsKindOf` check have no counterpart; `lType`, `lDim` and the
//!   context are unused upstream). An ASCII value whose extraction fails keeps the
//!   empty object's value (`SM_UNDEF_DOUBLE`, `TRUE`); an unreadable type header is
//!   [`SolidError::InvalidInput`]. Counts are bounded by the bytes left
//!   (`plan/findings.md`, "header-declared sizes").
//! - **Not in this unit.** `EvaluateSurface`, `CreateMirrorCurve`,
//!   `CreatePlaneProjection`, `FindContext`, `SetBaseSurface`, `SetUVDomain`,
//!   `SetOwnerFlag`, `GetRootCurve`, `GetMemoryUsed`, `WriteToDB`, `Dump` and
//!   `AssertValid` belong to other units or are out of scope.
//! - **Geometry depth.** Every construction path, including `empty`, database
//!   loading and `set_uv_curve`, checks both children with the shared
//!   [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) (32)
//!   geometry visitor before any recursive domain or copy operation. The children
//!   are private and exposed only immutably, so infallible `Clone` and temporary
//!   evaluation snapshots can safely copy this validated graph exactly.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::curve::curve::p1::ApproximateCurveOptions;
use crate::curve::curve::p2::{
    at, evaluate_continuity, is_kind_of as curve_is_kind_of, knots_ignoring_status, Curve,
    CurveEvaluate, CurveKind, KnotsWithStatus, CRV_ON_SURF_TYPE,
};
use crate::curve::curve::p5::CurveMemoKey;
use crate::curve::curve::p6::{calculate_bounding_box_base, CurveHandle, DroppedPoint};
use crate::curve::curve_cache::CurveCache;
use crate::curve::line::Line;
use crate::curve::tangent_field::{TangentFieldCurve, TangentFieldMembers};
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::are_same;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest, BoundingBoxes};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::Surface;
use crate::topology::database_io::{DatabaseReader, FileType};

const OP: &str = "curve::crv_on_surf::CrvOnSurf";
const OP_EVAL: &str = "curve::crv_on_surf::CrvOnSurf::evaluate";
const OP_KNOTS: &str = "curve::crv_on_surf::CrvOnSurf::knots";
const OP_CONTS: &str = "curve::crv_on_surf::CrvOnSurf::calculate_continuities";
const OP_APPROX: &str = "curve::crv_on_surf::CrvOnSurf::get_or_create_bs_approx";
const OP_READ: &str = "curve::crv_on_surf::CrvOnSurf::read_from_db";

/// `MAX_DERIVS` of `Evaluate`: the surface lifts go up to third derivatives.
const MAX_DERIVS: usize = 3;

/// Requested output bound shared with all allocating curve evaluators.
/// Unlike a fallible reserve alone, this also bounds successful zero-filling work.
const MAX_REQUESTED_DERIVS: usize = crate::curve::MAX_EVAL_DERIVS;

/// `SM_APPROX_TOL_3D` (`SM_ZONE_TOL_3D / 2`): the default approximation tolerance of
/// `GetOrCreateBSApproxPointer`.
pub const APPROX_TOL_3D: f64 = 1.0e-5 / 2.0;

/// `SM_CONTINUITY_ANGLE` (degrees): the default angle of `CalculateContinuities`.
pub const CONTINUITY_ANGLE: f64 = 0.5;

// ---- back-edges ----------------------------------------------------------------------

/// `SmSurface::ReadFromDB(type, db, .., version)`: the concrete surface of the record.
fn surface_read_from_db(
    k: &Kernel,
    surface_type: i64,
    db: &mut DatabaseReader,
    version: u64,
) -> Result<Surface> {
    Surface::read_from_db(k, surface_type, db, version)
}

/// The parameter intervals of the `SmCurveClassification` of `curve` over `interval`
/// (tolerance `tol`) against the knot iso-parameter lines of `surface`
/// (`IntersectUVDiscontinuties`), in order.
fn classify_against_surface(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    surface: &Surface,
) -> Result<Vec<Extent1d>> {
    crate::intersect::curve_class::p8::uv_discontinuity_intervals(k, curve, *interval, tol, surface)
}

/// `smsurf_LiftSecondDerivative(Cs, Css, DU, DV, DUU, DUV, DVV)`: the second
/// derivative of `S(C(s))`.
fn lift_second_derivative(cs: Vec3, css: Vec3, du: Vec3, dv: Vec3, d2: [Vec3; 3]) -> Vec3 {
    let [duu, duv, dvv] = d2;
    crate::surface::surface::p2::lift_second_derivative(cs, css, du, dv, duu, duv, dvv)
}

/// `smsurf_LiftThirdDerivative(Cs, Css, Csss, Su, Sv, Suu, Suv, Svv, Suuu, Suuv, Suvv,
/// Svvv)`: the third derivative of `S(C(s))`.
fn lift_third_derivative(c: [Vec3; 3], d1: [Vec3; 2], d2: [Vec3; 3], d3: [Vec3; 4]) -> Vec3 {
    let [cs, css, csss] = c;
    let [su, sv] = d1;
    let [suu, suv, svv] = d2;
    let [suuu, suuv, suvv, svvv] = d3;
    crate::surface::surface::p2::lift_third_derivative(
        cs, css, csss, su, sv, suu, suv, svv, suuu, suuv, suvv, svvv,
    )
}

/// The span side of a `bFromLeft` flag.
const fn side(from_left: bool) -> Side {
    if from_left {
        Side::Left
    } else {
        Side::Right
    }
}

// ---- results ------------------------------------------------------------------------

/// Result of [`CrvOnSurf::surface_curve_max_distance_between`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceCurveGap {
    /// `rdMaxDistFound`: the largest distance seen along the surface normal.
    pub max_dist: f64,
    /// `rdMaxDevFound`: the largest lateral deviation seen.
    pub max_dev: f64,
    /// `pOptCurveTDist`: this curve's parameter of `max_dist`.
    pub curve_t_dist: f64,
    /// `pOptOtherTDist`: the other curve's parameter of `max_dist`.
    pub other_t_dist: f64,
    /// `pOptCurveTDev`: this curve's parameter of `max_dev`.
    pub curve_t_dev: f64,
    /// `pOptOtherTDev`: the other curve's parameter of `max_dev`.
    pub other_t_dev: f64,
}

/// Result of `sm_EvalDistAndDev`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DistAndDev {
    /// `rbSuccess`.
    success: bool,
    /// `rdOtherParam`.
    other_param: f64,
    /// `rdDist`.
    dist: f64,
    /// `rdDev`.
    dev: f64,
}

/// What [`CrvOnSurf::evaluate_surface_normal`] returns (`aUV`, `rSurfaceNormal`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceNormalEval {
    /// `aUV[0]`: the UV point.
    pub uv: Point2,
    /// `aUV[1]`: the UV tangent (exact).
    pub uv_tangent: Point2,
    /// The surface normal.
    pub normal: Vec3,
}

// ---- the curve ----------------------------------------------------------------------

/// The two fields written by upstream's const `GetKnots`. No borrow of this
/// value is held while calling geometry or a solver (which can query knots again).
#[derive(Debug, Clone)]
struct BreakCache {
    need_breaks: bool,
    breaks: Vec<f64>,
}

/// The approximation fields written by upstream's const callers of
/// `GetOrCreateBSApproxCurve` (`DropPoint`, `CalculateBoundingBox`). As for
/// [`BreakCache`], no borrow of this value is held while calling geometry.
#[derive(Debug)]
struct ApproxCache {
    /// `m_pBSApproxCurve`: cached B-spline approximation of the 3D curve.
    bs_approx: Option<Box<BSplineCurve>>,
    /// `m_dMaxGap3d`: the approximation's largest sampled deviation.
    max_gap_3d: f64,
    /// `m_bParametrizationMatches`: the approximation matches the parameterisation.
    parametrization_matches: bool,
}

impl ApproxCache {
    /// An exact copy (the approximation's NURBS data kept as is).
    fn clone_exact(&self) -> Self {
        Self {
            bs_approx: self.bs_approx.as_deref().map(|c| Box::new(c.clone_exact())),
            max_gap_3d: self.max_gap_3d,
            parametrization_matches: self.parametrization_matches,
        }
    }
}

/// A curve on a surface (`SmCrvOnSurf`): `C(t) = S(uv(t))` for a 2D parameter-space
/// curve `uv` and a surface `S`. Its "knots" are the UV curve's knots plus the
/// parameters where it crosses the surface's knot lines (cached once computed); a
/// B-spline approximation of the 3D curve is cached on demand.
#[derive(Debug)]
pub struct CrvOnSurf {
    /// `m_pUVCurve`: the parameter-space curve (image dimension 2).
    uv_curve: Box<Curve>,
    /// `m_pSurface`: the base surface.
    surface: Box<Surface>,
    /// `m_vUVDomain`: the domain of interest on the surface.
    uv_domain: Extent2d,
    /// `m_bNeedBreaks`, `m_vBreaks`: mutable even in upstream's const queries (a
    /// [`Mutex`] rather than a `RefCell`, so that the curve stays `Sync` and a B-rep
    /// can share its edge arenas through an `Arc` while staying `Send`).
    break_cache: Mutex<BreakCache>,
    /// `m_lOwnerFlag`: 0 shares both parts, 1 owns (a copy of) the curve, 2 owns copies
    /// of both, 3 owns both originals.
    owner_flag: u64,
    /// `m_pBSApproxCurve`, `m_dMaxGap3d`, `m_bParametrizationMatches`: mutable even
    /// in upstream's const queries, as `break_cache`.
    approx_cache: Mutex<ApproxCache>,
    /// The curve cache of the base-class drop's global solve, kept on this object as
    /// upstream's `SmCacheMgr` keeps one per curve (not a field upstream), with the
    /// kernel state it was built under (`CurveMemoKey::of_kernel`). A copy starts
    /// without one; every edit of the curve drops it.
    curve_cache: Mutex<Option<(CurveMemoKey, CurveCache<CurveHandle>)>>,
}

/// An exact copy as a separate curve (the [`Curve`] copy constructor's): as upstream's
/// copy constructor, which deep-copies the surface when the curve owns its UV curve or
/// surface (owner flags 1 to 3), the copy's surface then gets its own NURB cells; with
/// flag 0 both curves name the same surface upstream and the cells stay shared. The
/// cached approximation is copied with the copy constructor's knot fix-up
/// (`m_pBSApproxCurve->Copy()`).
impl Clone for CrvOnSurf {
    fn clone(&self) -> Self {
        let mut c = self.clone_exact_shared();
        if matches!(self.owner_flag, 1..=3) {
            c.surface.detach_nurb_cells();
        }
        let approx = c.approx_cache_mut();
        if let Some(bs) = approx.bs_approx.take() {
            approx.bs_approx = Some(Box::new(bs.copy()));
        }
        c
    }
}

impl CrvOnSurf {
    /// An exact copy that stands for this curve (an evaluation copy): the surface
    /// shares its NURB cells with this curve's surface.
    pub(crate) fn clone_exact_shared(&self) -> Self {
        // All factories and replacement paths validate the complete graph first.
        Self {
            uv_curve: Box::new(self.uv_curve.clone_exact_bounded()),
            surface: Box::new(self.surface.clone_exact_bounded()),
            uv_domain: self.uv_domain,
            break_cache: Mutex::new(self.lock_break_cache().clone()),
            owner_flag: self.owner_flag,
            approx_cache: Mutex::new(self.lock_approx_cache().clone_exact()),
            curve_cache: Mutex::new(None),
        }
    }
}

impl CrvOnSurf {
    /// An exact evaluation copy retaining both NURBS objects and all cache flags.
    /// `depth` is the UV curve's depth in the shared `MAX_GEOMETRY_NESTING` (32)
    /// geometry traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            uv_curve: Box::new(self.uv_curve.clone_for_evaluation_depth(depth)?),
            surface: Box::new(self.surface.clone_exact_bounded()),
            uv_domain: self.uv_domain,
            break_cache: Mutex::new(self.lock_break_cache().clone()),
            owner_flag: self.owner_flag,
            approx_cache: Mutex::new(self.lock_approx_cache().clone_exact()),
            curve_cache: Mutex::new(None),
        })
    }

    /// The break cache, locked only for the access (a poisoned lock is recovered: the
    /// cache is written whole).
    fn lock_break_cache(&self) -> MutexGuard<'_, BreakCache> {
        self.break_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The approximation cache, locked only for the access (as the break cache).
    fn lock_approx_cache(&self) -> MutexGuard<'_, ApproxCache> {
        self.approx_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn approx_cache_mut(&mut self) -> &mut ApproxCache {
        self.approx_cache
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn curve_cache_mut(&mut self) -> &mut Option<(CurveMemoKey, CurveCache<CurveHandle>)> {
        self.curve_cache
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the break and approximation caches of `copy`, an exact copy of this curve
    /// that a const query ran on: upstream's const queries write them on the curve
    /// itself. The curve cache lent to the copy ([`Self::drop_point_ref_as`]) comes back
    /// with them.
    fn keep_caches_of(&self, copy: Self) {
        *self.lock_break_cache() = copy
            .break_cache
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        *self.lock_approx_cache() = copy
            .approx_cache
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(kept) = copy
            .curve_cache
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
        {
            *self
                .curve_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(kept);
        }
    }

    /// `SmCrvOnSurf_TYPE` (`CURV_BASE_TYPE + 48`).
    pub const TYPE: i64 = CRV_ON_SURF_TYPE;

    /// The empty object of `ReadFromDB` (`SmCrvOnSurf()`), on the given parts.
    fn bare(uv_curve: Curve, surface: Surface, owner_flag: u64) -> Result<Self> {
        Surface::check_curve_evaluation_copy_depth(&uv_curve, 1)?;
        surface.check_evaluation_copy_depth(1)?;
        Ok(Self {
            uv_curve: Box::new(uv_curve),
            surface: Box::new(surface),
            uv_domain: Extent2d::new(),
            break_cache: Mutex::new(BreakCache {
                need_breaks: true,
                breaks: Vec::new(),
            }),
            owner_flag,
            approx_cache: Mutex::new(ApproxCache {
                bs_approx: None,
                max_gap_3d: BIG_DOUBLE,
                parametrization_matches: false,
            }),
            curve_cache: Mutex::new(None),
        })
    }

    /// The curve `uv_curve` on `surface` (the constructor, `SetAll`): the domain of
    /// interest is `uv_domain` or the surface's natural domain; owner flags 1 and 2
    /// copy the UV curve (`Copy`), flag 2 also the surface.
    ///
    /// # Errors
    ///
    /// `InvalidInput` if the complete geometry would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING), checked
    /// before querying the domain or copying either part; otherwise those of the
    /// UV curve's `Copy` (owner flags 1 and 2) and surface's `Copy` (owner flag 2).
    pub fn new(
        k: &Kernel,
        uv_curve: Curve,
        surface: Surface,
        uv_domain: Option<Extent2d>,
        owner_flag: u64,
    ) -> Result<Self> {
        let mut c = Self::bare(uv_curve, surface, owner_flag)?;

        // set m_vUVDomain
        c.uv_domain = match uv_domain {
            Some(d) => d,
            None => c.surface.natural_uv_domain_with_nurb(k)?,
        };

        // m_lOwnerFlag 1 or 2: Copy Curve to be deleted when SmCrvOnSurf is destructed
        if owner_flag == 1 || owner_flag == 2 {
            *c.uv_curve = c.uv_curve.copy(k)?;
        }

        // m_lOwnerFlag 2: Copy Surface to be deleted when SmCrvOnSurf is destructed
        if owner_flag == 2 {
            *c.surface = c.surface.copy(k)?;
        }
        Ok(c)
    }

    /// The line segment from `start_uv` to `end_uv` on `surface` (the UV line
    /// constructor): the new UV line is always owned; owner flags 2 and 3 also own
    /// the surface (flag 2's surface copy is discarded, as upstream), anything else
    /// becomes flag 1.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the two points coincide (upstream then dereferences a
    /// null line), or the complete geometry would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn from_uv_line(
        k: &Kernel,
        start_uv: Point2,
        end_uv: Point2,
        surface: Surface,
        uv_domain: Option<Extent2d>,
        owner_flag: u64,
    ) -> Result<Self> {
        // new UVLine
        let line = Line::create_line_segment(
            2,
            Point3::new(start_uv.x, start_uv.y, 0.0),
            Point3::new(end_uv.x, end_uv.y, 0.0),
            None,
        )?;

        // set all the values - use lOwnerFlag == 0 to just store rSurface and pNewLine
        let mut c = Self::new(k, Curve::from(line), surface, uv_domain, 0)?;

        // adjust the lOwnerFlag - always delete m_pUVCurve
        c.owner_flag = match owner_flag {
            2 => 2,
            3 => 3,
            _ => 1,
        };
        Ok(c)
    }

    /// An empty object as `ReadFromDB` makes before reading (`SmCrvOnSurf()`), on the
    /// given parts: owner flag 0, the "inside out" domain, no caches.
    ///
    /// # Errors
    ///
    /// `InvalidInput` if the complete geometry would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    pub fn empty(uv_curve: Curve, surface: Surface) -> Result<Self> {
        Self::bare(uv_curve, surface, 0)
    }

    /// The copy constructor (`Copy`): copies the UV curve when this object owns it and
    /// the surface when it owns either part (`Copy`); the owner flag becomes 2 (owns
    /// both), 1 (owns the curve) or 0; the break cache and the approximation (with its
    /// gap and parameterisation flag) are kept when present.
    ///
    /// # Errors
    ///
    /// Those of the parts' `Copy`.
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        // figure out what the target curve owns - those things will be deep copied
        let owns_curve = self.owner_flag == 1 || self.owner_flag == 2 || self.owner_flag == 3;
        let owns_surface = self.owner_flag == 2 || self.owner_flag == 3;

        let uv_curve = if owns_curve {
            self.uv_curve.copy(k)?
        } else {
            self.uv_curve.clone_exact_bounded()
        };
        let surface = if owns_curve || owns_surface {
            self.surface.copy(k)?
        } else {
            self.surface.clone_exact_bounded()
        };

        let mut c = Self::bare(uv_curve, surface, 0)?;
        c.uv_domain = self.uv_domain;
        c.break_cache
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .need_breaks = self.need_breaks();

        // set the owner flag
        c.owner_flag = if owns_curve && owns_surface {
            2
        } else {
            u64::from(owns_curve)
        };

        // copy curve discontinuity break points when appropriate
        if !self.need_breaks() {
            c.break_cache
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .breaks = self.breaks();
        }

        {
            let a = self.lock_approx_cache();
            if let Some(bs) = &a.bs_approx {
                let ca = c.approx_cache_mut();
                ca.bs_approx = Some(Box::new(bs.copy()));
                ca.max_gap_3d = a.max_gap_3d;
                ca.parametrization_matches = a.parametrization_matches;
            }
        }
        Ok(c)
    }

    /// `CreateMirrorCurve`: the same parameter-space curve and domain on the mirrored
    /// surface; the new curve owns both parts (owner flag 3).
    ///
    /// # Errors
    ///
    /// Those of the surface's [`Surface::create_mirror_surface`], the UV curve's `Copy`
    /// and of [`CrvOnSurf::new`].
    pub fn create_mirror_curve(&self, k: &Kernel, mirror_plane: &Axis2Placement) -> Result<Self> {
        // Create the new geometry
        let mirror_surface = self.surface.create_mirror_surface(k, mirror_plane)?;
        let new_uv_curve = self.uv_curve.copy(k)?;
        // Create the new curve on mirrored geometry. It owns the curve and surf
        Self::new(k, new_uv_curve, mirror_surface, Some(self.uv_domain), 3)
    }

    // ---- simple access --------------------------------------------------------------

    /// The parameter-space curve (`GetUVCurve`).
    #[must_use]
    pub fn uv_curve(&self) -> &Curve {
        &self.uv_curve
    }

    /// Mutable parameter curve for faithful in-place B-spline base editing.
    pub(crate) fn uv_curve_mut(&mut self) -> &mut Curve {
        *self.curve_cache_mut() = None;
        &mut self.uv_curve
    }

    /// The base surface (`GetBaseSurface`).
    #[must_use]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// The domain of interest on the surface (`GetUVDomain`).
    #[must_use]
    pub const fn uv_domain(&self) -> &Extent2d {
        &self.uv_domain
    }

    /// The owner flag (`m_lOwnerFlag`).
    #[must_use]
    pub const fn owner_flag(&self) -> u64 {
        self.owner_flag
    }

    /// Whether the break cache still has to be computed (`m_bNeedBreaks`).
    #[must_use]
    pub fn need_breaks(&self) -> bool {
        self.lock_break_cache().need_breaks
    }

    /// A snapshot of the cached breaks (`m_vBreaks`). Returning owned values keeps
    /// callers from holding a cache borrow across a knot query.
    #[must_use]
    pub fn breaks(&self) -> Vec<f64> {
        self.lock_break_cache().breaks.clone()
    }

    /// Loads the break cache (`m_bNeedBreaks`, `m_vBreaks`), as `ReadFromDB` does.
    pub fn set_breaks_cache(&mut self, need_breaks: bool, breaks: Vec<f64>) {
        // the knots the curve cache was built over may change
        let cache = self.lock_break_cache();
        let same = cache.need_breaks == need_breaks && cache.breaks == breaks;
        drop(cache);
        if !same {
            *self.curve_cache_mut() = None;
        }
        *self
            .break_cache
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner) = BreakCache {
            need_breaks,
            breaks,
        };
    }

    /// An exact copy of the cached B-spline approximation (`m_pBSApproxCurve`), if any.
    #[must_use]
    pub fn bs_approx(&self) -> Option<BSplineCurve> {
        self.lock_approx_cache()
            .bs_approx
            .as_deref()
            .map(BSplineCurve::clone_exact)
    }

    /// The approximation's largest sampled deviation (`GetBSApproxMaxGap3d`).
    #[must_use]
    pub fn bs_approx_max_gap_3d(&self) -> f64 {
        self.lock_approx_cache().max_gap_3d
    }

    /// Whether the approximation was made matching the parameterisation
    /// (`m_bParametrizationMatches`).
    #[must_use]
    pub fn parametrization_matches(&self) -> bool {
        self.lock_approx_cache().parametrization_matches
    }

    /// Replaces the UV curve (`SetUVCurve`); the caches are kept, as upstream.
    ///
    /// # Errors
    ///
    /// `InvalidInput` if the replacement would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING); leaves
    /// the curve and its caches unchanged on failure.
    pub fn set_uv_curve(&mut self, uv_curve: Curve) -> Result<()> {
        Surface::check_curve_evaluation_copy_depth(&uv_curve, 1)?;
        *self.uv_curve = uv_curve;
        *self.curve_cache_mut() = None;
        Ok(())
    }

    /// `IsAnalytic`: never.
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        false
    }

    /// `IsBounded`: the UV curve's.
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.uv_curve.is_bounded()
    }

    /// `GetNaturalInterval`: the UV curve's.
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        self.uv_curve.natural_interval()
    }

    /// Image-space dimension (`GetDim`): always 3.
    #[must_use]
    pub const fn dim(&self) -> u64 {
        3
    }

    /// `GetDegree` (not overridden: the `SmCurve` default, 3).
    #[must_use]
    pub const fn degree(&self) -> usize {
        3
    }

    /// `GetType`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmCrvOnSurf_TYPE"
    }

    /// `GetClassString`.
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmCrvOnSurf"
    }

    /// `IsKindOf`: `SmCrvOnSurf_TYPE` or a base type.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == Self::TYPE || curve_is_kind_of(t)
    }

    /// `operator==`: equal UV curves, equal surfaces and the same domains (each within
    /// its scaled zero) (`SmCurve::operator==` holds: same type, dimension 3).
    ///
    /// # Errors
    ///
    /// Those of the UV curve comparison.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // check equivalence of these objects
        let mut rtn = true;
        rtn &= self.uv_curve.approx_eq(k, &other.uv_curve)?;
        rtn &= self.surface.approx_eq(k, &other.surface)?;
        rtn &= self.uv_domain.are_same(&other.uv_domain);
        Ok(rtn)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// Position and derivatives `0..=num_derivs` at `t` (`Evaluate`): the UV curve
    /// evaluated (up to third derivatives) and lifted through the surface. When the
    /// surface cannot give that many derivatives, fewer are tried; derivatives above
    /// the third are zero.
    ///
    /// # Errors
    ///
    /// Those of the UV curve's evaluation; the surface's last failure.
    /// `InvalidInput` above [`crate::curve::MAX_EVAL_DERIVS`], after those geometry errors.
    pub fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        // limit the number of derivatives to 3
        let mut n_eval = num_derivs.min(MAX_DERIVS);

        // Curve eval
        let crv = self
            .uv_curve
            .evaluate(k, t, n_eval, from_left, non_zero_tangents)?;

        // surface eval locals
        let p = at(&crv, 0);
        let uv = Point2::new(p.x, p.y);
        let flags = |nz: bool| EvaluateFlags {
            u_side: side(from_left),
            v_side: side(from_left),
            only_upper_half: true,
            non_zero_tangents: nz,
        };

        // Surface eval
        let mut srf = self
            .surface
            .evaluate(k, uv, n_eval, n_eval, flags(non_zero_tangents));

        // If the surface can't handle this many derivatives (default bNonZeroTangents)
        while let Err(e) = &srf {
            if e.is_unported() || n_eval == 0 {
                break;
            }
            n_eval -= 1;
            srf = self.surface.evaluate(k, uv, n_eval, n_eval, flags(true));
        }
        let srf = srf?;

        // The children only see orders <= 3. Check the requested output size after
        // their native error paths, but before any caller-sized arithmetic/storage.
        crate::curve::check_eval_derivs(num_derivs, OP_EVAL)?;
        let output_len = num_derivs
            .checked_add(1)
            .filter(|&len| len <= MAX_REQUESTED_DERIVS + 1)
            .ok_or(SolidError::InvalidInput {
                operation: OP_EVAL,
                reason: "requested derivative output length exceeds the shared bound",
            })?;

        // For subscripting into the surface pt/deriv array
        let ss = |u: usize, v: usize| {
            srf.get(u)
                .and_then(|r| r.get(v))
                .copied()
                .unwrap_or(Vec3::UNDEF)
        };

        let mut out = vec![Vec3::UNDEF; output_len];
        let mut put = |i: usize, v: Vec3| {
            if let Some(o) = out.get_mut(i) {
                *o = v;
            }
        };

        // Position
        put(0, ss(0, 0));

        // 1st Derivs: lift 1st derivs from uv to xyz
        if n_eval >= 1 {
            let c1 = at(&crv, 1);
            put(1, c1.x * ss(1, 0) + c1.y * ss(0, 1));
        }

        // 2nd Derivs
        if n_eval >= 2 {
            put(
                2,
                lift_second_derivative(
                    at(&crv, 1),
                    at(&crv, 2),
                    ss(1, 0),
                    ss(0, 1),
                    [ss(2, 0), ss(1, 1), ss(0, 2)],
                ),
            );
        }

        // 3rd Derivs
        if n_eval >= 3 {
            put(
                3,
                lift_third_derivative(
                    [at(&crv, 1), at(&crv, 2), at(&crv, 3)],
                    [ss(1, 0), ss(0, 1)],
                    [ss(2, 0), ss(1, 1), ss(0, 2)],
                    [ss(3, 0), ss(2, 1), ss(1, 2), ss(0, 3)],
                ),
            );
        }

        // If too many requested, fill with zeros.
        for ii in MAX_DERIVS + 1..=num_derivs {
            put(ii, Vec3::new(0.0, 0.0, 0.0));
        }

        Ok(out)
    }

    /// The point at `t` (`EvaluatePoint`): the surface at the UV curve's point.
    ///
    /// # Errors
    ///
    /// Those of the UV curve's and the surface's point evaluation.
    pub fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        // UVCurve eval
        let p = self.uv_curve.evaluate_point(k, t)?;

        // surface eval
        self.surface.evaluate_point(k, Point2::new(p.x, p.y))
    }

    /// The UV point at `t` (`EvaluateUVPoint`).
    ///
    /// # Errors
    ///
    /// Those of the UV curve's point evaluation.
    pub fn evaluate_uv_point(&self, k: &Kernel, t: f64) -> Result<Point2> {
        let p = self.uv_curve.evaluate_point(k, t)?;
        Ok(Point2::new(p.x, p.y))
    }

    /// The UV point and exact tangent at `t` and the surface normal there
    /// (`EvaluateSurfaceNormal`).
    ///
    /// # Errors
    ///
    /// Those of the UV curve's evaluation; [`SolidError::Unported`] from the normal
    /// (`SmSurface::EvaluateNormal`).
    pub fn evaluate_surface_normal(
        &self,
        k: &Kernel,
        t: f64,
        from_left: bool,
    ) -> Result<SurfaceNormalEval> {
        // Curve eval - exact tangent values
        let crv = self.uv_curve.evaluate(k, t, 1, from_left, false)?;

        // set outputs
        let p = at(&crv, 0);
        let d = at(&crv, 1);
        let uv = Point2::new(p.x, p.y);
        let uv_tangent = Point2::new(d.x, d.y);

        let normal = self
            .surface
            .evaluate_normal(k, uv, side(from_left), side(from_left))?;
        Ok(SurfaceNormalEval {
            uv,
            uv_tangent,
            normal,
        })
    }

    /// The surface's first derivatives at `t` and their derivatives with respect to
    /// `t` (`EvaluateSurface1stDerivDerivs`): `([Su, dSu/dt], [Sv, dSv/dt])`.
    ///
    /// # Errors
    ///
    /// Those of the UV curve's evaluation; [`SolidError::Unported`] from the surface
    /// (`SmSurface::Evaluate2ndDerivatives`).
    pub fn evaluate_surface_1st_deriv_derivs(
        &self,
        k: &Kernel,
        t: f64,
        from_left: bool,
    ) -> Result<([Vec3; 2], [Vec3; 2])> {
        // Curve eval - non-zero tangent values
        let crv = self.uv_curve.evaluate(k, t, 1, from_left, true)?;
        let p = at(&crv, 0);
        let uv = Point2::new(p.x, p.y);
        let uvt = at(&crv, 1);

        let [_s, su, sv, suv, suu, svv] = self
            .surface
            .evaluate_2nd_derivatives(k, uv, from_left, from_left)?;

        // set outputs
        Ok((
            [su, uvt.x * suu + uvt.y * suv],
            [sv, uvt.x * suv + uvt.y * svv],
        ))
    }

    // ---- knots ----------------------------------------------------------------------

    /// The breaks ("knots"), as unique values, and multiplicities as if the curve were
    /// a cubic B-spline (`GetKnots`): the cached breaks (all of them, with
    /// multiplicities `4, 2, ..., 2, 4`) when at least two are cached; else the UV
    /// curve's knots (in `interval`, if given) plus every crossing of the surface's
    /// knot lines (as a C1 knot, multiplicity 2), which become the cache. On a
    /// failing classification: the adjusted UV curve knots with that status, nothing
    /// cached (upstream's `SER`).
    ///
    /// # Errors
    ///
    /// Those of the UV curve's degree; [`SolidError::Unported`] from the curve
    /// classification; [`SolidError::InvalidInput`] where upstream would read
    /// outside its arrays.
    pub fn knots_with_status(
        &self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<KnotsWithStatus> {
        // low work
        if !self.need_breaks() {
            let breaks = self.breaks();
            let num = breaks.len();
            if num >= 2 {
                // copy saved breaks array; multiplicities as if degree 3
                let mut mults = Vec::with_capacity(num);
                mults.push(4);
                mults.extend(core::iter::repeat_n(2, num - 2));
                mults.push(4);
                return Ok(KnotsWithStatus::success(breaks, mults));
            }
        }

        // pick a UVSpace tolerance
        let tol_2d =
            EFF_ZERO * (1.0 + self.surface.natural_uv_domain_with_nurb(k)?.max_dimension());

        // get the UVCurve knots and multiplicities (status ignored)
        let (mut knots, mut mults) = ignored(self.uv_curve.knots(k, interval))?.unwrap_or_default();

        // modify the mult list to act as if pUVCurve is a degree 3 BSpline
        let uv_degree = self.uv_curve.degree()?;
        if uv_degree != 3 {
            for m in &mut mults {
                *m = if uv_degree < *m + 3 {
                    *m + 3 - uv_degree
                } else {
                    1
                };
            }
        }

        // classify the UVTrimCurve against the surface knot isoParameter lines.
        let classes = match classify_against_surface(
            k,
            &self.uv_curve,
            &self.uv_curve.natural_interval(),
            tol_2d,
            &self.surface,
        ) {
            Ok(classes) => classes,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(e) => {
                let status = Err(e);
                return Ok(KnotsWithStatus {
                    knots,
                    mults,
                    status,
                });
            }
        };

        // add the classification boundaries uniquely and in order to the knot lists
        let cnt = classes.len();
        let short =
            || SolidError::invalid_input(OP_KNOTS, "no UV curve knots or classification intervals");
        let mut uv_knot = *knots.first().ok_or_else(short)?;
        let mut jj = 0usize;
        for ii in 0..=cnt {
            let class_bndry = if ii < cnt {
                classes.get(ii).ok_or_else(short)?.min()
            } else {
                classes.get(ii.wrapping_sub(1)).ok_or_else(short)?.max()
            };

            // when given an interval - skip ClassKnots outside the requested interval
            if let Some(ivl) = interval {
                // skip too small param values
                if ivl.min() > class_bndry + tol_2d {
                    continue;
                }
                // quit when a too large param value is seen
                if ivl.max() < class_bndry - tol_2d {
                    break;
                }
            }

            // check the next UVKnots until we get one bigger or equal to class_bndry
            while class_bndry > uv_knot + tol_2d {
                jj += 1;
                uv_knot = if jj == knots.len() {
                    BIG_DOUBLE
                } else {
                    *knots.get(jj).ok_or_else(short)?
                };
            }

            // when class_bndry value is already in the knots - move onto next ClassKnot
            if class_bndry > uv_knot - tol_2d {
                continue;
            }

            // add class boundary as if it were a C1 knot into the m_pUVCurve
            if jj > knots.len() {
                return Err(short());
            }
            knots.insert(jj, class_bndry);
            mults.insert(jj.min(mults.len()), 2);

            // skip to next UVKnot and continue
            jj += 1;
            if jj >= knots.len() {
                jj = knots.len() - 1;
            }
            uv_knot = *knots.get(jj).ok_or_else(short)?;
        }

        // cache the discontinuity break points
        *self.lock_break_cache() = BreakCache {
            need_breaks: false,
            breaks: knots.clone(),
        };
        Ok(KnotsWithStatus::success(knots, mults))
    }

    /// [`Self::knots_with_status`] with a failing status as the error.
    ///
    /// # Errors
    ///
    /// As [`Self::knots_with_status`], and its failing status.
    pub fn knots(&self, k: &Kernel, interval: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots_with_status(k, interval)?.into_result()
    }

    /// [`Self::knots`] for shared callers, retaining upstream's cache update.
    ///
    /// # Errors
    ///
    /// As [`Self::knots`].
    pub fn knots_ref(
        &self,
        k: &Kernel,
        interval: Option<&Extent1d>,
    ) -> Result<(Vec<f64>, Vec<usize>)> {
        self.knots(k, interval)
    }

    /// The sum of the break multiplicities (`GetNumberNaturalKnots`), caching the
    /// breaks; `GetKnots`' status is ignored (a failure counts nothing).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`Self::knots`].
    pub fn number_natural_knots(&self, k: &Kernel) -> Result<usize> {
        let (_, mults) = knots_ignoring_status(self.knots_with_status(k, None))?;
        Ok(mults.iter().sum())
    }

    /// [`Self::number_natural_knots`] for shared callers, retaining the cache.
    ///
    /// # Errors
    ///
    /// As [`Self::number_natural_knots`].
    pub fn number_natural_knots_ref(&self, k: &Kernel) -> Result<usize> {
        self.number_natural_knots(k)
    }

    /// The minimum continuity of the interior breaks and the continuity at every
    /// break (`CalculateContinuities`): the ends discontinuous, each interior break
    /// evaluated from both sides with `angle_tol`; the minimum is
    /// [`ContinuityType::Undefined`] without interior breaks. Caches the breaks. A
    /// failing evaluation keeps the previous break's continuity (upstream ignores the
    /// status; before any success it reads an uninitialised value, UB-263: here
    /// [`ContinuityType::Undefined`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`Self::knots`] or the evaluations;
    /// [`SolidError::InvalidInput`] without breaks.
    pub fn calculate_continuities(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        self.calculate_continuities_as(k, angle_tol, None)
    }

    /// [`Self::calculate_continuities`] as the base of a derived object (`derived`:
    /// the `SmTangentField` members; `EvaluateContinuity` evaluates it).
    pub(crate) fn calculate_continuities_as(
        &self,
        k: &Kernel,
        angle_tol: f64,
        derived: Option<TangentFieldMembers>,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        // upstream's `this` for EvaluateContinuity
        let this = derived.map(|m| self.as_curve(Some(m)));

        // init output
        let mut min_cont = ContinuityType::Undefined;

        // get effective knots and continuities (status ignored)
        let (knots, _) = knots_ignoring_status(self.knots_with_status(k, None))?;
        let n = knots.len();
        if n == 0 {
            return Err(SolidError::invalid_input(OP_CONTS, "no breaks"));
        }

        // start and end knots
        let mut conts = vec![ContinuityType::Undefined; n];
        if let Some(c) = conts.first_mut() {
            *c = ContinuityType::Discontinuous;
        }
        if let Some(c) = conts.last_mut() {
            *c = ContinuityType::Discontinuous;
        }

        // eThisContinuity: declared before the loop and left unchanged by a failing
        // EvaluateContinuity, whose status upstream ignores (uninitialised before the
        // first success upstream, UB-263; `SM_CT_UNDEFINED` here)
        let mut c = ContinuityType::Undefined;

        // build the output
        let mut ii = 1;
        while ii + 1 < n {
            let t = *knots
                .get(ii)
                .ok_or_else(|| SolidError::invalid_input(OP_CONTS, "no breaks"))?;

            // evaluate geometric continuities (status ignored)
            let r = match &this {
                None => evaluate_continuity(k, self, t, true, self, t, false, angle_tol),
                Some(d) => evaluate_continuity(k, d, t, true, d, t, false, angle_tol),
            };
            if let Some(e) = ignored(r)? {
                c = e.continuity;
            }

            // set output
            if let Some(d) = conts.get_mut(ii) {
                *d = c;
            }

            // save min continuity
            if min_cont == ContinuityType::Undefined || min_cont as i32 > c as i32 {
                min_cont = c;
            }
            ii += 1;
        }
        Ok((min_cont, conts))
    }

    /// [`Self::calculate_continuities`] for shared callers, retaining the cache.
    ///
    /// # Errors
    ///
    /// As [`Self::calculate_continuities`].
    pub fn calculate_continuities_ref(
        &self,
        k: &Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        self.calculate_continuities(k, angle_tol)
    }

    // ---- the B-spline approximation -------------------------------------------------

    /// A [`Curve`] wrapping a copy of this object (upstream's `this` for the base-class
    /// calls): the curve on surface itself, or (`derived`) the `SmTangentField` this
    /// object is the base of, so that the virtual calls reach the derived overrides.
    fn as_curve(&self, derived: Option<TangentFieldMembers>) -> Curve {
        match derived {
            None => Curve::from(self.clone()),
            Some(m) => Curve::from(TangentFieldCurve::from_parts(self.clone(), m)),
        }
    }

    /// The cached B-spline approximation of the 3D curve, made when needed
    /// (`GetOrCreateBSApproxPointer`). Given `breaks` that differ from the cached
    /// ones (or no cached breaks) replace them and force a new approximation, as does
    /// a different `match_parameterization` (default `true`). The cached approximation
    /// is used when its gap is within `approx_tol`; else the breaks are computed when
    /// needed ([`Self::knots`], status ignored) and `SmCurve::ApproximateCurve` makes a
    /// new one (with analytics, not copying B-splines) over them.
    ///
    /// # Errors
    ///
    /// Those of the approximation ([`Curve::approximate_curve_base`]);
    /// [`SolidError::Unported`] from [`Self::knots`]. The cache keeps upstream's state.
    pub fn get_or_create_bs_approx(
        &mut self,
        k: &Kernel,
        approx_tol: f64,
        breaks: Option<&[f64]>,
        match_parameterization: Option<bool>,
    ) -> Result<BSplineCurve> {
        self.get_or_create_bs_approx_as(k, approx_tol, breaks, match_parameterization, None)
    }

    /// [`Self::get_or_create_bs_approx`] as the base of a derived object (`derived`:
    /// the `SmTangentField` members; its virtual `Evaluate` makes the approximation).
    pub(crate) fn get_or_create_bs_approx_as(
        &mut self,
        k: &Kernel,
        approx_tol: f64,
        breaks: Option<&[f64]>,
        match_parameterization: Option<bool>,
        derived: Option<TangentFieldMembers>,
    ) -> Result<BSplineCurve> {
        let max_gap_3d = self.approx_cache_mut().max_gap_3d;
        let match_param = match_parameterization.unwrap_or(true);
        let mut recompute = false;

        // If we need breaks and have been given them, or have breaks and they are not
        // the same, then we will build a bspline using the supplied breaks.
        if let Some(b) = breaks {
            if self.need_breaks() || b != self.breaks().as_slice() {
                recompute = true;
                self.set_breaks_cache(false, b.to_vec());
            }
        }

        if match_param != self.approx_cache_mut().parametrization_matches {
            recompute = true;
        }

        if recompute {
            self.approx_cache_mut().bs_approx = None;
        }

        // If we have an approximation and the tolerance is good enough, use it
        // (upstream hands out the cached object itself: an exact copy)
        if let Some(a) = &self.approx_cache_mut().bs_approx {
            if max_gap_3d <= approx_tol {
                return Ok(a.clone_exact());
            }
        }

        // otherwise make one
        if self.need_breaks() {
            let (knots, _) = knots_ignoring_status(self.knots_with_status(k, None))?;
            self.set_breaks_cache(false, knots);
        }
        {
            let a = self.approx_cache_mut();
            a.bs_approx = None;
            a.parametrization_matches = match_param;
        }

        let opts = ApproximateCurveOptions {
            create_analytics: true,
            match_parameterization: match_param,
            just_copy_bsplines: false,
        };
        let approx =
            self.as_curve(derived)
                .approximate_curve_base(k, &self.breaks(), approx_tol, &opts)?;
        if let Some(g) = approx.achieved_tol {
            self.approx_cache_mut().max_gap_3d = g;
        }
        let CurveKind::BSpline(bs) = approx.curve.into_kind() else {
            return Err(SolidError::invalid_input(
                OP_APPROX,
                "the approximation is not an SmBSplineCurve",
            ));
        };
        // (the cached object is the approximation itself, as upstream's)
        self.approx_cache_mut().bs_approx = Some(Box::new(bs.clone_exact()));
        Ok(bs)
    }

    /// `ApproximateCurve`: a copy of the cached approximation over `breaks`
    /// ([`Self::get_or_create_bs_approx`] with `match_parameterization`) and the
    /// cache's gap. Upstream ignores `bOptCreateAnalytics` and `bCopyBSplines`.
    ///
    /// # Errors
    ///
    /// Those of [`Self::get_or_create_bs_approx`] (the gap is then
    /// [`Self::bs_approx_max_gap_3d`]).
    pub fn approximate_curve(
        &mut self,
        k: &Kernel,
        breaks: &[f64],
        approx_tol: f64,
        match_parameterization: bool,
    ) -> Result<(BSplineCurve, f64)> {
        self.approximate_curve_as(k, breaks, approx_tol, match_parameterization, None)
    }

    /// [`Self::approximate_curve`] as the base of a derived object
    /// ([`Self::get_or_create_bs_approx_as`]).
    pub(crate) fn approximate_curve_as(
        &mut self,
        k: &Kernel,
        breaks: &[f64],
        approx_tol: f64,
        match_parameterization: bool,
        derived: Option<TangentFieldMembers>,
    ) -> Result<(BSplineCurve, f64)> {
        let bs = self.get_or_create_bs_approx_as(
            k,
            approx_tol,
            Some(breaks),
            Some(match_parameterization),
            derived,
        )?;
        Ok((bs.copy(), self.approx_cache_mut().max_gap_3d))
    }

    /// The boxes of the curve over `interval` (`CalculateBoundingBox`): those of the
    /// cached approximation ([`Self::get_or_create_bs_approx`] with the defaults), or,
    /// when it cannot be made, of the base-class sampling.
    ///
    /// # Errors
    ///
    /// Those of the boxes; [`SolidError::Unported`] from the approximation.
    pub fn calculate_bounding_box(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
    ) -> Result<BoundingBoxes> {
        self.calculate_bounding_box_as(k, interval, req, expand, None)
    }

    /// [`Self::calculate_bounding_box`] as the base of a derived object
    /// ([`Self::get_or_create_bs_approx_as`]).
    pub(crate) fn calculate_bounding_box_as(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
        derived: Option<TangentFieldMembers>,
    ) -> Result<BoundingBoxes> {
        match self.get_or_create_bs_approx_as(k, APPROX_TOL_3D, None, None, derived) {
            Ok(bs) => bs.calculate_bounding_box(interval, req),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => {
                calculate_bounding_box_base(k, &self.as_curve(derived), interval, req, expand)
            }
        }
    }

    /// [`Self::calculate_bounding_box`] on `&self`: run on an exact copy whose break and
    /// approximation caches are then kept on this curve.
    ///
    /// # Errors
    ///
    /// As [`Self::calculate_bounding_box`].
    pub fn calculate_bounding_box_ref(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
    ) -> Result<BoundingBoxes> {
        self.calculate_bounding_box_ref_as(k, interval, req, expand, None)
    }

    /// [`Self::calculate_bounding_box_ref`] as the base of a derived object.
    pub(crate) fn calculate_bounding_box_ref_as(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
        derived: Option<TangentFieldMembers>,
    ) -> Result<BoundingBoxes> {
        // (an exact stand-in for `this`, whose caches are kept below)
        let mut copy = self.clone_exact_shared();
        let result = copy.calculate_bounding_box_as(k, interval, req, expand, derived);
        self.keep_caches_of(copy);
        result
    }

    /// `DropPoint`: with a `guess` the base-class drop; else the base-class drop
    /// guessed from the Greville abscissa of the approximation's control point
    /// closest to `point` (snapped to `interval`), or the base-class global drop when
    /// no approximation can be made.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::drop_point_base`]; [`SolidError::Unported`] from the
    /// approximation.
    pub fn drop_point(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
    ) -> Result<DroppedPoint> {
        self.drop_point_as(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            None,
            &mut None,
        )
    }

    /// [`Self::drop_point`] as the base of a derived object
    /// ([`Self::get_or_create_bs_approx_as`]; the base-class drop evaluates it), the
    /// base drop's global solve reading the caller's curve `cache` for this curve.
    pub(crate) fn drop_point_as(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        derived: Option<TangentFieldMembers>,
        cache: &mut Option<CurveCache<CurveHandle>>,
    ) -> Result<DroppedPoint> {
        // check inputs (upstream warns and selects SM_SO_MINIMIZE)
        let op = if op != SolverOperation::Minimize
            && op != SolverOperation::Intersect
            && op != SolverOperation::Normalize
        {
            SolverOperation::Minimize
        } else {
            op
        };

        if guess.is_some() {
            return self.drop_point_base_preserving_breaks(
                k,
                interval,
                point,
                vec_to_inside,
                dist_tol,
                guess,
                op,
                derived,
                cache,
            );
        }

        let bs = match self.get_or_create_bs_approx_as(k, APPROX_TOL_3D, None, None, derived) {
            Ok(bs) => bs,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => {
                return self.drop_point_base_preserving_breaks(
                    k,
                    interval,
                    point,
                    vec_to_inside,
                    dist_tol,
                    guess,
                    op,
                    derived,
                    cache,
                )
            }
        };

        let (control_points, _weights) = bs.control_polygon()?;

        let mut d_min = BIG_DOUBLE;
        let mut min_index = 0usize;
        for (ii, cp) in control_points.iter().enumerate() {
            let dist_sq = cp.distance_squared(point);
            if dist_sq < d_min {
                d_min = dist_sq;
                min_index = ii;
            }
        }

        let min_param = interval.snap_value(bs.greville_abscissa(min_index)?, EFF_ZERO);

        self.drop_point_base_preserving_breaks(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            Some(min_param),
            op,
            derived,
            cache,
        )
    }

    /// Run the base algorithm on an exact evaluation copy, then keep only the
    /// break-cache writes it actually made, including writes before an error.
    /// A successful local/fast solve may never request knots; do not force them.
    /// The global solve reads (and on first use builds) the caller's curve `cache`;
    /// when the caller holds none, this curve's own (see the `curve_cache` field),
    /// which the solve's build then refills.
    fn drop_point_base_preserving_breaks(
        &mut self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        derived: Option<TangentFieldMembers>,
        cache: &mut Option<CurveCache<CurveHandle>>,
    ) -> Result<DroppedPoint> {
        // this curve's own cache, for a caller without one (not for a derived object,
        // whose curve differs; not when the kernel memo is off)
        let own_key = match (cache.is_none(), derived.is_none()) {
            (true, true) => CurveMemoKey::of_kernel(k),
            _ => None,
        };
        if let Some(key) = own_key {
            if let Some((kept, c)) = self.curve_cache_mut().take() {
                if kept == key {
                    *cache = Some(c);
                }
            }
        }
        let curve = self.as_curve(derived);
        let result = curve.drop_point_base_cached(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            cache,
        );
        if let Some(key) = own_key {
            if let Some(c) = cache.take() {
                *self.curve_cache_mut() = Some((key, c));
            }
        }
        let base = match curve.into_kind() {
            CurveKind::CrvOnSurf(c) => Some(*c),
            CurveKind::TangentField(t) => Some(t.into_base()),
            _ => None,
        };
        if let Some(c) = base {
            *self
                .break_cache
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner) = c
                .break_cache
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner);
        }
        result
    }

    /// [`Self::drop_point`] on `&self`: run on an exact copy whose break and
    /// approximation caches are then kept on this curve.
    ///
    /// # Errors
    ///
    /// As [`Self::drop_point`].
    pub fn drop_point_ref(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
    ) -> Result<DroppedPoint> {
        self.drop_point_ref_as(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            None,
            &mut None,
        )
    }

    /// [`Self::drop_point_ref`] as the base of a derived object, the base drop's global
    /// solve reading the caller's curve `cache` for this curve (see
    /// [`Self::drop_point_as`]).
    pub(crate) fn drop_point_ref_as(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        point: Point3,
        vec_to_inside: Option<Vec3>,
        dist_tol: f64,
        guess: Option<f64>,
        op: SolverOperation,
        derived: Option<TangentFieldMembers>,
        cache: &mut Option<CurveCache<CurveHandle>>,
    ) -> Result<DroppedPoint> {
        // (an exact stand-in for `this`, whose caches are kept below)
        let mut copy = self.clone_exact_shared();
        // the copy stands for this curve: it uses (and gives back) this curve's cache
        *copy.curve_cache_mut() = self
            .curve_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let result = copy.drop_point_as(
            k,
            interval,
            point,
            vec_to_inside,
            dist_tol,
            guess,
            op,
            derived,
            cache,
        );
        self.keep_caches_of(copy);
        result
    }

    // ---- editing --------------------------------------------------------------------

    /// `EditParameterization`: rescales the cached breaks to `new_param` (or drops the
    /// cache when either interval has zero length), then reparameterises the UV curve.
    ///
    /// # Errors
    ///
    /// Those of the UV curve's edit (the breaks are rescaled already).
    pub fn edit_parameterization(&mut self, k: &Kernel, new_param: &Extent1d) -> Result<()> {
        *self.curve_cache_mut() = None;
        // when appropriate scale breaks
        if !self.need_breaks() {
            let ivl = self.uv_curve.natural_interval();
            let cache = self
                .break_cache
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner);
            if ivl.length() > EFF_ZERO && new_param.length() > EFF_ZERO {
                // for every break
                for b in &mut cache.breaks {
                    *b = ((*b - ivl.min()) / ivl.length()) * new_param.length() + new_param.min();
                }
            } else {
                // working with zero length intervals
                cache.breaks.clear();
                cache.need_breaks = true;
            }
        }

        // pass the call along to m_pUVCurve
        self.uv_curve.edit_parameterization(k, new_param)
    }

    /// `ReverseParameterization`: reverses the UV curve; returns the new interval of
    /// `old_interval`. The caches are kept, as upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when nothing is owned (flag 0); those of the UV curve's reversal.
    pub fn reverse_parameterization(
        &mut self,
        k: &Kernel,
        old_interval: &Extent1d,
    ) -> Result<Extent1d> {
        // Don't own curve can not reverse parameterization
        if self.owner_flag == 0 {
            return Err(SolidError::sm_err(OP));
        }
        *self.curve_cache_mut() = None;
        self.uv_curve.reverse_parameterization(k, old_interval)
    }

    /// `Trim`: trims the UV curve (`trim_interval` may be snapped) and drops the break
    /// cache (not the approximation, as upstream).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when nothing is owned (flag 0); those of the UV curve's trim.
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            crate::geom::tol::zone_tol_3d(k, crate::geom::tol::SmallTopoType::NormalSize),
        )
    }

    /// Trims the owned UV curve with the caller-resolved owning object's tolerance.
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        // Don't own curve can not trim the curve
        if self.owner_flag == 0 {
            return Err(SolidError::sm_err(OP));
        }

        // pass the call along to the UVCurve
        *self.curve_cache_mut() = None;
        self.uv_curve
            .trim_with_zone_tol(k, trim_interval, mode, zone_tol_3d)?;

        // clear the knots
        if !self.need_breaks() {
            self.set_breaks_cache(true, Vec::new());
        }
        Ok(())
    }

    /// `Transform`: transforms the cached approximation (status ignored) and, when the
    /// surface is owned (flag 2 or more), the surface; nothing for the identity.
    ///
    /// # Errors
    ///
    /// Those of the surface's transform, including `InvalidInput` when a growing
    /// volume map would nest deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING).
    /// Earlier cache edits remain.
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        *self.curve_cache_mut() = None;

        // Transform cached bspline approximation (status ignored)
        if let Some(a) = &mut self.approx_cache_mut().bs_approx {
            ignored(a.transform(rotate_n_move, scale))?;
        }

        // If we don't own the surface, the owner will transform it.
        if self.owner_flag < 2 {
            return Ok(());
        }

        // pass the call along to the surface
        self.surface
            .transform_at_depth(k, rotate_n_move, scale, depth + 1)
    }

    // ---- gaps -----------------------------------------------------------------------

    /// `sm_EvalDistAndDev`: the gap between this curve's surface point at `param` and
    /// the other curve: the other curve's point on the surface normal line there (the
    /// closest such point), or else its drop of the surface point from `guess`; split
    /// into the distance along the normal (absolute) and the deviation across it.
    fn eval_dist_and_dev(
        &self,
        k: &Kernel,
        param: f64,
        other: &Curve,
        guess: f64,
        other_interval: &Extent1d,
        drop_tol_3d: f64,
    ) -> Result<DistAndDev> {
        // init outputs
        let mut out = DistAndDev {
            success: false,
            other_param: guess,
            dist: 0.0,
            dev: 0.0,
        };

        // Evaluate surface point and normal at the given parameter.
        let sn = self.evaluate_surface_normal(k, param, true)?;
        let norm = sn.normal;
        let surf_pt = self.surface.evaluate_point(k, sn.uv)?;

        // Find point on 3d curve that would drop here: Intersect it with Surface Pt/Normal.
        let line = Line::infinite(surf_pt, norm, 3, false);
        let line_dom = Extent1d::from_min_max(-1000.0, 1000.0);
        let x = other.intersect_with_line(k, other_interval, &line, &line_dom, drop_tol_3d)?;

        let mut crv_param = 0.0;

        if !x.solutions.is_empty() && !x.needs_more_intersections {
            // In case of multiple solutions, take the closest one.
            let mut min_dist = BIG_DOUBLE;
            if x.solutions.len() > 1 {
                min_dist /= 2.0;
            }
            for s in &x.solutions {
                let this_param = s.start.param(0)?;
                let crv_pt = other.evaluate_point(k, this_param)?;
                let gap = crv_pt - surf_pt;
                let dist = gap.dot(norm).abs();
                if dist < min_dist {
                    min_dist = dist;
                    crv_param = this_param;
                }
            }
        } else {
            // If that didn't work, use DropPoint: drop that point to the other curve.
            let d = other.drop_point(
                k,
                other_interval,
                surf_pt,
                None,
                drop_tol_3d,
                Some(guess),
                SolverOperation::Minimize,
            )?;
            out.success = d.success;
            crv_param = d.parameter;
            if !out.success {
                return Ok(out);
            }
        }

        let crv_pt = other.evaluate_point(k, crv_param)?;

        // Get the gap and find its two components: parallel and perp to the normal.
        let gap = crv_pt - surf_pt;
        let dist = gap.dot(norm);
        let norm_pt = surf_pt + dist * norm;
        out.dev = crv_pt.distance(norm_pt);
        out.other_param = crv_param;
        out.dist = dist.abs();
        out.success = true;
        Ok(out)
    }

    /// `SurfaceCurveMaxDistanceBetween`: the largest gaps between this curve over
    /// `interval` and `other` over `[other_start, other_end]` (mapped to the ends of
    /// `interval`), sampled at `num_samples` points of this curve (`0`: 22 plus the
    /// knot counts of both curves): the largest distance along the surface normal and
    /// the largest deviation across it, with the parameters where they occur. With
    /// `max_needed`, stops once either exceeds it. The same object over the same
    /// interval (either direction) has no gap.
    ///
    /// # Errors
    ///
    /// Those of the gap evaluations; [`SolidError::Unported`] from the surface normal.
    pub fn surface_curve_max_distance_between(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Curve,
        other_start: f64,
        other_end: f64,
        num_samples: usize,
        drop_tol_3d: f64,
        max_needed: Option<f64>,
    ) -> Result<SurfaceCurveGap> {
        // a local
        let mut other_interval = Extent1d::from_value(other_start);
        other_interval.add_value(other_end);

        // Init outputs
        let mut out = SurfaceCurveGap {
            max_dist: 0.0,
            max_dev: 0.0,
            curve_t_dist: interval.mid(),
            other_t_dist: other_interval.mid(),
            curve_t_dev: interval.mid(),
            other_t_dev: other_interval.mid(),
        };

        // No work: curves and intervals are the same.
        let same =
            matches!(other.kind(), CurveKind::CrvOnSurf(c) if core::ptr::eq(c.as_ref(), self));
        if same
            && ((are_same(interval.min(), other_start) && are_same(interval.max(), other_end))
                || (are_same(interval.min(), other_end) && are_same(interval.max(), other_start)))
        {
            return Ok(out);
        }

        // more locals
        let orientation = if other_end >= other_start { 1.0 } else { -1.0 };
        let mut max_dist = 0.0;
        let mut max_dev = 0.0;
        let mut num_samples = num_samples;

        // Do a very large sample when asked for a precise answer (GetKnots ignored)
        if num_samples == 0 {
            let this_knots = knots_ignoring_status(self.clone().knots_with_status(k, None))?
                .0
                .len();
            let other_knots = knots_ignoring_status(other.knots_with_status(k, None))?
                .0
                .len();
            num_samples = 22 + this_knots + other_knots;
        }

        // special case line/line check
        if self.degree() == 1
            && self.number_natural_knots_ref(k)? == 4
            && other.degree()? == 1
            && other.number_natural_knots(k)? == 4
        {
            num_samples = 2;
        }

        // init maxGap Point to interval midPoints
        let mut this_t_max_dist = interval.mid();
        let mut other_t_max_dist = other_interval.mid();
        let mut this_t_max_dev = interval.mid();
        let mut other_t_max_dev = other_interval.mid();

        // Uniformly sample this Curve from beginning to end.
        for ii in 0..num_samples {
            let per = ii as f64 / (num_samples - 1) as f64;
            let mut t_guess_1 = interval.evaluate(per);
            let t_guess_2 = if orientation > 0.0 {
                other_interval.evaluate(per)
            } else {
                other_interval.evaluate(1.0 - per)
            };

            let mut r = self.eval_dist_and_dev(
                k,
                t_guess_1,
                other,
                t_guess_2,
                &other_interval,
                drop_tol_3d,
            )?;

            // when no solution was found for 1st guess
            if !r.success && ii == 0 {
                // try moving ThisCurvePoint a small distance from its endPoint
                t_guess_1 = interval.evaluate(0.001);
                r = self.eval_dist_and_dev(
                    k,
                    t_guess_1,
                    other,
                    t_guess_2,
                    &other_interval,
                    drop_tol_3d,
                )?;
            }

            // when no solution was found for last guess
            if !r.success && ii == num_samples - 1 {
                // try moving ThisCurvePoint a small distance from its endPoint
                t_guess_1 = interval.evaluate(0.999);
                r = self.eval_dist_and_dev(
                    k,
                    t_guess_1,
                    other,
                    t_guess_2,
                    &other_interval,
                    drop_tol_3d,
                )?;
            }

            // Save biggest solution gap vectors
            if r.success && r.dist > max_dist {
                max_dist = r.dist;
                this_t_max_dist = t_guess_1;
                other_t_max_dist = r.other_param;
            }
            if r.success && r.dev > max_dev {
                max_dev = r.dev;
                this_t_max_dev = t_guess_1;
                other_t_max_dev = r.other_param;
            }

            // When given a MaxDistance limit - and the limit is exceeded
            if max_needed.is_some_and(|m| max_dist > m || max_dev > m) {
                break;
            }
        }

        // (upstream forces bDoPrecise = FALSE here: set output and return)
        out.max_dist = max_dist;
        out.curve_t_dist = this_t_max_dist;
        out.other_t_dist = other_t_max_dist;
        out.max_dev = max_dev;
        out.curve_t_dev = this_t_max_dev;
        out.other_t_dev = other_t_max_dev;
        Ok(out)
    }

    // ---- persistence ----------------------------------------------------------------

    /// Reads a curve on surface from `db` (database version `version`)
    /// (`ReadFromDB`): the domain of interest, the cache flag, the UV curve (type
    /// header and data), the surface (type header and data) and, when cached, the
    /// breaks. The new object owns both parts (owner flag 2).
    ///
    /// # Errors
    ///
    /// Those of the reads and of the UV curve's reader; [`SolidError::Unported`] from
    /// the surface reader; [`SolidError::InvalidInput`] for an unreadable type header
    /// or a count larger than the input.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        let ascii = db.file_type() == FileType::Ascii;
        let no_stream = || SolidError::invalid_input(OP_READ, "no input stream");

        // locals (the empty object's values)
        let mut min = Point2::new(UNDEF_DOUBLE, UNDEF_DOUBLE);
        let mut max = Point2::new(UNDEF_DOUBLE, UNDEF_DOUBLE);
        let need_breaks;

        if ascii {
            let s = db.in_stream_mut().ok_or_else(no_stream)?;
            for v in [&mut min.x, &mut min.y, &mut max.x, &mut max.y] {
                if let Some(x) = s.extract_double() {
                    *v = x;
                }
            }
            db.go_to_next_line();
            let s = db.in_stream_mut().ok_or_else(no_stream)?;
            need_breaks = s
                .extract_int()
                .ok_or_else(|| SolidError::invalid_input(OP_READ, "breaks flag not read"))?
                != 0;
            db.go_to_next_line();
        } else {
            min.x = db.read_double()?;
            min.y = db.read_double()?;
            max.x = db.read_double()?;
            max.y = db.read_double()?;
            need_breaks = db.read_boolean()? != 0;
        }

        // UV curve
        if ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let uv_type = header
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "UV curve type not read"))?;
        let uv_dim = header
            .dim
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "UV curve dimension not read"))?;
        let uv_curve = Curve::read_from_db(k, uv_type, db, uv_dim, version)?;

        // compounding surface
        if ascii {
            db.go_to_next_line();
        }
        let surface_type = db
            .read_type(false)?
            .type_code
            .ok_or_else(|| SolidError::invalid_input(OP_READ, "surface type not read"))?;
        let surface = surface_read_from_db(k, surface_type, db, version)?;

        // when appropriate - breaks
        let mut breaks = Vec::new();
        if !need_breaks {
            let count = if ascii {
                let n = db
                    .in_stream_mut()
                    .ok_or_else(no_stream)?
                    .extract_ulong()
                    .ok_or_else(|| SolidError::invalid_input(OP_READ, "break count not read"))?;
                db.go_to_next_line();
                n
            } else {
                db.read_ulong()?
            };
            let count = usize::try_from(count)
                .map_err(|_| SolidError::invalid_input(OP_READ, "break count too large"))?;
            db.ensure_available(count, if ascii { 1 } else { 8 }, OP_READ)?;

            // read every break
            if ascii {
                let s = db.in_stream_mut().ok_or_else(no_stream)?;
                for _ in 0..count {
                    breaks.push(s.extract_double().unwrap_or(0.0));
                }
                if count > 0 {
                    db.go_to_next_line();
                }
            } else {
                breaks = db.read_doubles(count)?;
            }
        }

        // load obj
        let mut c = Self::bare(uv_curve, surface, 2)?;
        // (SetMinMax's status is ignored: an inverted domain keeps the empty one)
        let _ = c.uv_domain.set_min_max(min, max);
        c.set_breaks_cache(need_breaks, breaks);
        Ok(c)
    }
}

impl CurveEvaluate for CrvOnSurf {
    fn dim(&self) -> u64 {
        Self::dim(self)
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        k: &Kernel,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        Self::evaluate(self, k, t, num_derivs, from_left, non_zero_tangents)
    }

    fn evaluate_point(&self, k: &Kernel, t: f64) -> Result<Point3> {
        Self::evaluate_point(self, k, t)
    }

    fn as_crv_on_surf(&self) -> Option<&CrvOnSurf> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;
    use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;

    /// A bilinear patch over `[0, 1]^2`: `S(u, v) = (u, v, u * v)`.
    fn patch() -> BSplineSurface {
        BSplineSurface::create_bilinear_surface(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 1.0),
        )
        .unwrap()
    }

    /// A 2D quadratic UV curve on `[0, 1]`.
    fn uv() -> Curve {
        let mut cur = allocate_nurb_curve(2, 2, 5).unwrap();
        let pts = [(0.1, 0.2), (0.5, 0.9), (0.9, 0.3)];
        for (p, (x, y)) in cur.cpts_mut().iter_mut().zip(pts) {
            p.x = x;
            p.y = y;
            p.z = f64::MAX;
            p.w = -f64::MAX;
        }
        cur.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        Curve::from(BSplineCurve::from_nurb(cur, 2, false))
    }

    #[test]
    fn shared_knots_cache_refreshes_short_records_without_sharing_clones() {
        let k = Kernel::default();
        let mut c = CrvOnSurf::new(&k, uv(), patch().into(), None, 3).unwrap();
        // Native GetKnots refreshes a loaded cache with fewer than two breaks,
        // even when its need-breaks flag says it is already computed.
        c.set_breaks_cache(false, vec![0.25]);
        let copy = c.clone();
        let before = c.breaks();
        let curve = Curve::from(c);
        let (knots, _) = curve.knots(&k, None).unwrap();
        let CurveKind::CrvOnSurf(c) = curve.kind() else {
            panic!("wrong curve kind");
        };
        assert_eq!(knots, vec![0.0, 1.0]);
        assert_eq!(c.breaks(), knots);
        assert!(!c.need_breaks());
        assert_eq!(before, vec![0.25]);
        assert_eq!(copy.breaks(), before);
        let (_, mults) = curve
            .knots(&k, Some(&Extent1d::from_raw(0.2, 0.8)))
            .unwrap();
        assert_eq!(mults, vec![4, 4]);
    }

    #[test]
    fn const_queries_keep_their_caches_on_the_curve() {
        // DropPoint and CalculateBoundingBox on `&self` keep the approximation (and
        // breaks) they make on the curve, as upstream's const queries; a later query
        // reusing them answers as one on a fresh copy.
        let k = Kernel::default();
        let c = CrvOnSurf::new(&k, uv(), patch().into(), None, 0).unwrap();
        let ivl = c.natural_interval();
        assert!(c.bs_approx().is_none());
        let point = c.evaluate_point(&k, 0.4).unwrap() + Vec3::new(0.0, 0.0, 0.25);
        let drop = |c: &CrvOnSurf| {
            c.drop_point_ref(&k, &ivl, point, None, 1e-6, None, SolverOperation::Minimize)
                .unwrap()
        };
        let fresh = drop(&c.clone());
        let first = drop(&c);
        assert!(c.bs_approx().is_some());
        assert!(!c.need_breaks());
        let gap = c.bs_approx_max_gap_3d();
        let kept = drop(&c);
        for d in [first, kept] {
            assert_eq!(d.success, fresh.success);
            assert_eq!(d.parameter.to_bits(), fresh.parameter.to_bits());
            assert_eq!(d.distance.to_bits(), fresh.distance.to_bits());
        }
        assert_eq!(c.bs_approx_max_gap_3d().to_bits(), gap.to_bits());

        let b = CrvOnSurf::new(&k, uv(), patch().into(), None, 0).unwrap();
        let req = BoundingBoxRequest::default();
        let fresh_box = b.clone().calculate_bounding_box_ref(&k, &ivl, req, false);
        let first_box = b.calculate_bounding_box_ref(&k, &ivl, req, false);
        assert!(b.bs_approx().is_some());
        let kept_box = b.calculate_bounding_box_ref(&k, &ivl, req, false);
        assert_eq!(format!("{first_box:?}"), format!("{fresh_box:?}"));
        assert_eq!(format!("{kept_box:?}"), format!("{fresh_box:?}"));
    }

    #[test]
    fn base_drop_keeps_only_the_cache_writes_of_the_taken_solver_path() {
        let k = Kernel::default();
        let mut c = CrvOnSurf::new(&k, uv(), patch().into(), None, 0).unwrap();
        let ivl = c.natural_interval();
        let point = c.evaluate_point(&k, 0.25).unwrap();
        let local = c
            .drop_point(
                &k,
                &ivl,
                point,
                None,
                1e-6,
                Some(0.25),
                SolverOperation::Minimize,
            )
            .unwrap();
        assert!(local.success);
        assert!(c.need_breaks());
        assert!(c.breaks().is_empty());

        // Normalize without a guess skips the fast Minimize/Intersect solve and
        // requests the curve cache. Its knot queries must survive the base copy.
        c.drop_point_base_preserving_breaks(
            &k,
            &ivl,
            point,
            None,
            1e-6,
            None,
            SolverOperation::Normalize,
            None,
            &mut None,
        )
        .unwrap();
        assert!(!c.need_breaks());
        assert_eq!(c.breaks(), vec![0.0, 1.0]);
    }

    #[test]
    fn the_curve_keeps_its_drop_cache_until_edited() {
        // A caller without a curve cache of its own drops through the curve's: built
        // by the first global solve, then reused, with the drops of a fresh build; a
        // copy starts without it and an edit of the curve drops it.
        let k = Kernel::default();
        let mut c = CrvOnSurf::new(&k, uv(), patch().into(), None, 0).unwrap();
        let ivl = c.natural_interval();
        let has_cache = |c: &CrvOnSurf| c.curve_cache.lock().unwrap().is_some();
        for t in [0.25, 0.6, 0.9] {
            let point = c.evaluate_point(&k, t).unwrap();
            let drop = |c: &mut CrvOnSurf| {
                c.drop_point_base_preserving_breaks(
                    &k,
                    &ivl,
                    point,
                    None,
                    1e-6,
                    None,
                    SolverOperation::Normalize,
                    None,
                    &mut None,
                )
                .unwrap()
            };
            let fresh = drop(&mut c.clone());
            let kept = drop(&mut c);
            assert!(has_cache(&c));
            assert_eq!(kept.success, fresh.success);
            assert_eq!(kept.parameter.to_bits(), fresh.parameter.to_bits());
            assert_eq!(kept.distance.to_bits(), fresh.distance.to_bits());
        }
        assert!(!has_cache(&c.clone()));
        let mut edited = c.clone_exact_shared();
        assert!(!has_cache(&edited));
        edited.set_uv_curve(c.uv_curve().clone()).unwrap();
        c.set_uv_curve(c.uv_curve().clone()).unwrap();
        assert!(!has_cache(&c));
    }

    #[test]
    fn a_kept_curve_cache_drops_as_a_fresh_one() {
        // The curve cache built by the first global solve, kept on the curve for a
        // caller without one and in the caller's slot for a caller with one, gives the
        // drops of a cache built for each one.
        let k = Kernel::default();
        let mut c = CrvOnSurf::new(&k, uv(), patch().into(), None, 0).unwrap();
        let ivl = c.natural_interval();
        let mut held = None;
        for t in [0.25, 0.6, 0.9] {
            let point = c.evaluate_point(&k, t).unwrap();
            let drop = |c: &mut CrvOnSurf, cache: &mut Option<CurveCache<CurveHandle>>| {
                c.drop_point_base_preserving_breaks(
                    &k,
                    &ivl,
                    point,
                    None,
                    1e-6,
                    None,
                    SolverOperation::Normalize,
                    None,
                    cache,
                )
                .unwrap()
            };
            let fresh = drop(&mut c.clone(), &mut None);
            // the curve's own cache (built by the first drop, then reused)
            let kept = drop(&mut c, &mut None);
            assert!(c.curve_cache.lock().unwrap().is_some());
            // a caller's slot takes precedence and stays the caller's
            if held.is_none() {
                held = c.curve_cache.lock().unwrap().take().map(|(_, cache)| cache);
            }
            let reused = drop(&mut c.clone(), &mut held);
            assert!(held.is_some());
            for d in [kept, reused] {
                assert_eq!(d.success, fresh.success);
                assert_eq!(d.parameter.to_bits(), fresh.parameter.to_bits());
                assert_eq!(d.distance.to_bits(), fresh.distance.to_bits());
            }
        }
    }

    #[test]
    fn evaluate_request_bound_preserves_lifted_derivatives() {
        use crate::surface::offset_surface::OffsetSurface;

        let k = Kernel::default();
        let surface = OffsetSurface::new(&k, 0.15, patch().into(), true, 0).unwrap();
        let curve = CrvOnSurf::new(&k, uv(), surface.into(), None, 0).unwrap();
        let expected = curve.evaluate(&k, 0.3, 3, true, false).unwrap();
        for n in [0, 1, 2, 3, 4, 6, MAX_REQUESTED_DERIVS] {
            let values = curve.evaluate(&k, 0.3, n, true, false).unwrap();
            assert_eq!(values.len(), n + 1);
            let computed = n.min(3) + 1;
            assert_eq!(&values[..computed], &expected[..computed]);
            assert!(values.iter().skip(4).all(|v| *v == Vec3::ZERO));
        }
        // Include every fuzzed giant order and the architecture's own maximum.
        let excessive = [
            (MAX_REQUESTED_DERIVS + 1) as u64,
            2_147_483_648,
            4_294_967_296,
            9_223_372_036_854_775_807,
        ];
        for n in excessive
            .into_iter()
            .filter_map(|n| usize::try_from(n).ok())
            .chain(std::iter::once(usize::MAX))
        {
            assert!(matches!(
                curve.evaluate(&k, 0.3, n, true, false),
                Err(SolidError::InvalidInput {
                    operation: OP_EVAL,
                    ..
                })
            ));
        }
    }

    #[test]
    fn evaluate_excessive_request_preserves_uv_error_precedence() {
        let k = Kernel::default();
        let curve = CrvOnSurf::empty(BSplineCurve::new(2).into(), patch().into()).unwrap();
        let expected = curve.evaluate(&k, 0.3, 3, true, false).unwrap_err();
        assert_eq!(
            curve
                .evaluate(&k, 0.3, usize::MAX, true, false)
                .unwrap_err(),
            expected
        );
    }

    #[test]
    fn shared_surface_copies_preserve_the_face_owner_chain() {
        use crate::geom::attribute::ObjectKey;
        use crate::geom::object::ObjectRef;
        use crate::surface::offset_surface::OffsetSurface;

        let k = Kernel::default();
        let face = ObjectRef {
            type_code: crate::geom::assert_array::labels::FACE_TYPE,
            key: ObjectKey(19),
        };
        let offset = OffsetSurface::new(&k, 0.15, patch().into(), true, 0).unwrap();
        let mut surface = Surface::new(crate::surface::surface::SurfaceKind::OffsetSurface(offset));
        surface.set_surface_owner(face, None);
        let curve = CrvOnSurf::new(&k, uv(), surface, None, 0).unwrap();

        for copy in [curve.copy(&k).unwrap(), curve.clone()] {
            assert_eq!(copy.surface().reached_face(), Some(face));
            let crate::surface::surface::SurfaceKind::OffsetSurface(offset) = copy.surface().kind()
            else {
                panic!("copy lost the concrete offset surface");
            };
            assert_eq!(offset.base_surface().reached_face(), Some(face));
        }
    }

    #[test]
    fn copy_of_owning_curve_detaches_the_plane_nurb_cell() {
        use crate::surface::plane::Plane;
        use crate::surface::surface::SurfaceKind;
        let k = Kernel::default();
        let on = |c: &Curve| match c.kind() {
            CurveKind::CrvOnSurf(c) => c.surface().clone_exact_bounded(),
            _ => panic!("not a curve on a surface"),
        };
        let has_nurb = |c: &Curve| match on(c).kind() {
            SurfaceKind::Plane(p) => p.base().gw_nurb().is_some(),
            _ => panic!("not a curve on a plane"),
        };
        let domain = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        // flag 0 names the same surface upstream (shared cell); flag 1 deep-copies it
        for (flag, shared) in [(0, true), (1, false)] {
            let plane = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
            let curve = Curve::from(CrvOnSurf::new(&k, uv(), plane, Some(domain), flag).unwrap());
            assert!(!has_nurb(&curve));
            let evaluation = curve.clone_exact_bounded();
            let copy = curve.clone();
            on(&curve).natural_uv_domain_with_nurb(&k).unwrap();
            assert!(has_nurb(&curve));
            assert!(has_nurb(&evaluation));
            assert_eq!(has_nurb(&copy), shared);
        }
    }

    #[test]
    fn direct_curve_depth_boundary_rejects_construction_and_replacement() {
        use crate::surface::plane::Plane;
        let k = Kernel::default();
        let plane = || Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::Z));
        let mut nested = uv();
        for _ in 0..MAX_GEOMETRY_NESTING {
            nested = Curve::from(CrvOnSurf::new(&k, nested, plane(), None, 0).unwrap());
        }
        let expected = nested.evaluate_point(&k, 0.25).unwrap();
        assert_eq!(nested.clone().evaluate_point(&k, 0.25).unwrap(), expected);
        assert_eq!(
            nested.copy(&k).unwrap().evaluate_point(&k, 0.25).unwrap(),
            expected
        );
        assert!(matches!(
            CrvOnSurf::new(&k, nested.clone(), plane(), None, 0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            CrvOnSurf::empty(nested.clone(), plane()),
            Err(SolidError::InvalidInput { .. })
        ));
        let mut target = CrvOnSurf::new(&k, uv(), plane(), None, 0).unwrap();
        target.set_breaks_cache(false, vec![0.0, 1.0]);
        let before = target.evaluate_point(&k, 0.25).unwrap();
        assert!(matches!(
            target.set_uv_curve(nested),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(target.evaluate_point(&k, 0.25).unwrap(), before);
        assert_eq!(target.breaks(), &[0.0, 1.0]);
    }

    /// Shared next maps retain the complete chain across native virtual copies.
    fn volume_mapped_curve(
        k: &Kernel,
        tail: crate::surface::volume::Volume,
        prefix_maps: usize,
        offset: bool,
    ) -> CrvOnSurf {
        use crate::geom::transform::Transform;
        use crate::surface::offset_surface::OffsetSurface;
        use crate::surface::plane::Plane;
        use crate::surface::srf_in_volume::SrfInVolume;
        use crate::surface::volume::{Volume, VolumeKind};

        let mut volume = tail;
        for _ in 0..prefix_maps {
            let mut prefix = Volume::new(VolumeKind::Transform(Transform::IDENTITY));
            prefix.add_next_map(volume, 0).unwrap();
            volume = prefix;
        }
        let mut surface: Surface = SrfInVolume::new(
            k,
            Plane::from_normal(Vec3::ZERO, Vec3::Z).into(),
            true,
            volume,
            0,
            3,
        )
        .unwrap()
        .into();
        if offset {
            surface = OffsetSurface::new(k, 0.125, surface, true, 0)
                .unwrap()
                .into();
        }
        CrvOnSurf::new(k, uv(), surface, None, 3).unwrap()
    }

    #[test]
    fn transform_rejects_volume_growth_without_undoing_the_earlier_cache_edit() {
        use crate::surface::twist_volume::TwistVolume;

        let k = Kernel::default();
        let translation = Vec3::new(2.0, -3.0, 4.0);
        let motion = Axis2Placement::new(translation, Vec3::X, Vec3::Y);
        // Exercise both direct SrfInVolume and the owned OffsetSurface path.
        for offset in [false, true] {
            let twist = TwistVolume::create(Vec3::ZERO, Vec3::Z, Vec3::X, 10.0, 1e-6);
            // Crv depth 0, surface depth 1 (+1 for an offset), volume tail at
            // MAX_GEOMETRY_NESTING.
            let prefix_maps = MAX_GEOMETRY_NESTING - 2 - usize::from(offset);
            let mut curve = volume_mapped_curve(&k, twist, prefix_maps, offset);
            let sample = 0.25;
            let before = curve.evaluate(&k, sample, 1, true, false).unwrap();
            curve
                .transform(&k, &Axis2Placement::IDENTITY, None)
                .unwrap();
            assert_eq!(curve.evaluate(&k, sample, 1, true, false).unwrap(), before);
            let approx = curve
                .get_or_create_bs_approx(&k, 1e-5, Some(&[0.0, 1.0]), Some(true))
                .unwrap();
            let cached_before = approx.evaluate_point(sample).unwrap();
            let gap = curve.bs_approx_max_gap_3d();
            let breaks = curve.breaks();

            assert!(matches!(
                curve.transform(&k, &motion, None),
                Err(SolidError::InvalidInput { .. })
            ));
            // The native cache edit precedes the failed surface transform.
            let cached_after = curve.bs_approx().unwrap().evaluate_point(sample).unwrap();
            assert!((cached_after - cached_before - translation).length() < 1e-12);
            assert_eq!(curve.bs_approx_max_gap_3d(), gap);
            assert_eq!(curve.breaks(), breaks);
            assert_eq!(curve.evaluate(&k, sample, 1, true, false).unwrap(), before);
            for copy in [curve.copy(&k).unwrap(), curve.clone()] {
                assert_eq!(copy.evaluate(&k, sample, 1, true, false).unwrap(), before);
                assert_eq!(
                    copy.bs_approx().unwrap().evaluate_point(sample).unwrap(),
                    cached_after
                );
            }
            if offset {
                // Offset's native nonuniform-scale refusal precedes reaching the
                // full volume chain and must not become a depth refusal.
                assert!(matches!(
                    curve.transform(&k, &motion, Some(Vec3::new(2.0, 3.0, 4.0))),
                    Err(SolidError::Upstream { code: 1001, .. })
                ));
                assert_eq!(curve.evaluate(&k, sample, 1, true, false).unwrap(), before);
            }
        }
    }

    #[test]
    fn boundary_transform_allows_absorption_and_an_append_with_one_level_left() {
        use crate::geom::transform::Transform;
        use crate::surface::twist_volume::TwistVolume;
        use crate::surface::volume::{Volume, VolumeKind};

        let k = Kernel::default();
        let translation = Vec3::new(2.0, -3.0, 4.0);
        let motion = Axis2Placement::new(translation, Vec3::X, Vec3::Y);
        for offset in [false, true] {
            let depth_adjustment = usize::from(offset);
            let absorb = volume_mapped_curve(
                &k,
                Volume::new(VolumeKind::Transform(Transform::IDENTITY)),
                MAX_GEOMETRY_NESTING - 2 - depth_adjustment,
                offset,
            );
            let append = volume_mapped_curve(
                &k,
                TwistVolume::create(Vec3::ZERO, Vec3::Z, Vec3::X, 10.0, 1e-6),
                MAX_GEOMETRY_NESTING - 3 - depth_adjustment,
                offset,
            );
            for mut curve in [absorb, append] {
                let sample = 0.25;
                let before = curve.evaluate(&k, sample, 1, true, false).unwrap();
                curve.transform(&k, &motion, None).unwrap();
                let after = curve.evaluate(&k, sample, 1, true, false).unwrap();
                assert!((after[0] - before[0] - translation).length() < 1e-12);
                assert_eq!(after[1], before[1]);
                for copy in [curve.copy(&k).unwrap(), curve.clone()] {
                    assert_eq!(copy.evaluate(&k, sample, 1, true, false).unwrap(), after);
                }
            }
        }
    }

    #[test]
    fn shared_close_knots_remain_exact_through_evaluation_and_approximation() {
        use crate::nurbs::nurbs_srf::allocate_nurb_surface;
        let k = Kernel::default();
        let delta = 1.0e-9;
        let sample = 0.75;
        let mut raw = allocate_nurb_surface(2, 1, 1, 1, 4, 3).unwrap();
        raw.u_knots_mut()
            .copy_from_slice(&[0.0, 0.0, delta, 1.0, 1.0]);
        raw.v_knots_mut().copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
        for i in 0..3 {
            for j in 0..2 {
                let p = raw.cpt_mut(i, j).unwrap();
                p.x = i as f64;
                p.y = j as f64;
                p.z = 0.0;
                p.w = 1.0;
            }
        }
        let surface = BSplineSurface::from_nurb(raw, true);
        let direct = surface
            .evaluate_point(Point2::new(delta * sample, 0.25))
            .unwrap();
        let normalized = surface
            .copy()
            .evaluate_point(Point2::new(delta * sample, 0.25))
            .unwrap();
        assert!((normalized - direct).length() > 0.5);
        let mut raw_uv = allocate_nurb_curve(1, 1, 3).unwrap();
        raw_uv.knots_mut().copy_from_slice(&[0.0, 0.0, 1.0, 1.0]);
        for (p, u) in raw_uv.cpts_mut().iter_mut().zip([0.0, delta]) {
            p.x = u;
            p.y = 0.25;
            p.z = f64::MAX;
            p.w = -f64::MAX;
        }
        let mut curve = CrvOnSurf::new(
            &k,
            Curve::from(BSplineCurve::from_nurb(raw_uv, 2, false)),
            surface.into(),
            None,
            0,
        )
        .unwrap();
        for copy in [curve.copy(&k).unwrap(), curve.clone()] {
            assert_eq!(copy.evaluate_point(&k, sample).unwrap(), direct);
            assert_eq!(
                copy.evaluate(&k, sample, 1, true, false).unwrap(),
                curve.evaluate(&k, sample, 1, true, false).unwrap()
            );
        }
        // Native owned Crv copies still call virtual Surface::copy; an offset
        // with an unowned base must keep that base's original raw geometry.
        for owner_flag in 1..=3 {
            let offset = crate::surface::offset_surface::OffsetSurface::new(
                &k,
                0.15,
                curve.surface().clone_for_evaluation().unwrap(),
                false,
                0,
            )
            .unwrap();
            let owned = CrvOnSurf::new(
                &k,
                curve.uv_curve().clone_exact_bounded(),
                offset.into(),
                None,
                owner_flag,
            )
            .unwrap();
            let expected = direct + Vec3::Z * 0.15;
            for copy in [owned.clone(), owned.copy(&k).unwrap()] {
                assert!((copy.evaluate_point(&k, sample).unwrap() - expected).length() < 1e-12);
            }
        }
        let (approximation, gap) = curve
            .approximate_curve(&k, &[0.0, 1.0], 1e-6, true)
            .unwrap();
        assert!(gap <= 1e-6);
        assert!((approximation.evaluate_point(sample).unwrap() - direct).length() <= 1e-6);
    }

    #[test]
    fn construct_and_query() {
        let k = &Kernel::default();
        let c = CrvOnSurf::new(k, uv(), patch().into(), None, 1).unwrap();
        assert_eq!(c.owner_flag(), 1);
        assert!(c.need_breaks());
        assert!(c.is_kind_of(CrvOnSurf::TYPE) && c.is_kind_of(crate::curve::curve::p2::TYPE));
        assert_eq!(c.type_string(), "SmCrvOnSurf_TYPE");
        assert_eq!(c.natural_interval().max(), 1.0);
        assert!(c
            .approx_eq(k, &c.copy(&Kernel::default()).unwrap())
            .unwrap());
        let l = CrvOnSurf::from_uv_line(
            &Kernel::default(),
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 1.0),
            patch().into(),
            None,
            7,
        )
        .unwrap();
        assert_eq!(l.owner_flag(), 1);
        let p = l.evaluate_point(k, 0.5).unwrap();
        assert!((p.z - 0.25).abs() < 1e-12);
    }

    #[test]
    fn huge_derivative_orders_are_rejected() {
        // fuzz: `n = 2^31` sized the output allocation (abort)
        let c = CrvOnSurf::new(&Kernel::default(), uv(), patch().into(), None, 0).unwrap();
        for n in [crate::curve::MAX_EVAL_DERIVS + 1, 1 << 31, usize::MAX] {
            assert!(matches!(
                c.evaluate(&Kernel::default(), 0.3, n, true, false),
                Err(SolidError::InvalidInput { .. })
            ));
        }
    }

    #[test]
    fn evaluate_lifts_derivatives() {
        let k = &Kernel::default();
        let c = CrvOnSurf::new(k, uv(), patch().into(), None, 0).unwrap();
        let d = c.evaluate(k, 0.3, 5, true, false).unwrap();
        assert_eq!(d.len(), 6);
        let p = c.evaluate_point(k, 0.3).unwrap();
        assert!((d[0] - p).length() < 1e-12);
        // finite difference of the tangent
        let h = 1e-6;
        let a = c.evaluate_point(k, 0.3 - h).unwrap();
        let b = c.evaluate_point(k, 0.3 + h).unwrap();
        let fd = (b - a) / (2.0 * h);
        assert!((fd - d[1]).length() < 1e-6);
        assert_eq!(d[4], Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn cached_knots_and_edits() {
        let k = Kernel::default();
        let mut c = CrvOnSurf::new(&Kernel::default(), uv(), patch().into(), None, 0).unwrap();
        assert_eq!(c.knots(&k, None).unwrap(), (vec![0.0, 1.0], vec![4, 4]));
        assert!(!c.need_breaks());
        c.set_breaks_cache(false, vec![0.0, 0.25, 1.0]);
        let (kn, m) = c.knots(&k, None).unwrap();
        assert_eq!(kn, vec![0.0, 0.25, 1.0]);
        assert_eq!(m, vec![4, 2, 4]);
        assert_eq!(c.number_natural_knots(&k).unwrap(), 10);
        let (min, conts) = c.calculate_continuities(&k, CONTINUITY_ANGLE).unwrap();
        assert_eq!(conts.len(), 3);
        assert_ne!(min, ContinuityType::Undefined);
        c.edit_parameterization(&Kernel::default(), &Extent1d::from_min_max(0.0, 2.0))
            .unwrap();
        assert_eq!(c.breaks(), &[0.0, 0.5, 2.0]);
        let mut ivl = Extent1d::from_min_max(0.5, 1.5);
        assert!(c.trim(&k, &mut ivl, TrimMode::Notify).is_err());
        assert!(c
            .reverse_parameterization(&Kernel::default(), &ivl)
            .is_err());
    }

    #[test]
    fn approximation_cache() {
        let mut c = CrvOnSurf::new(&Kernel::default(), uv(), patch().into(), None, 1).unwrap();
        let (bs, gap) = c
            .approximate_curve(&Kernel::default(), &[0.0, 1.0], 1e-6, false)
            .unwrap();
        assert!(gap <= 1e-6);
        assert!(bs.number_control_points().unwrap() >= 4);
        assert!(c.bs_approx().is_some());
        assert!(!c.parametrization_matches());
        let ivl = c.natural_interval();
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: false,
            polar: false,
        };
        // the default approximation matches the parameterisation: rebuilt
        let b = c
            .calculate_bounding_box(&Kernel::default(), &ivl, req, true)
            .unwrap();
        assert!(b.normal.is_some());
        assert!(c.parametrization_matches());
    }

    #[test]
    fn lifts_match_the_formulas() {
        let cs = Vec3::new(1.0, 2.0, 0.0);
        let css = Vec3::new(3.0, 4.0, 0.0);
        let e = Vec3::new(1.0, 0.0, 0.0);
        let v = lift_second_derivative(cs, css, e, e, [e, e, e]);
        assert_eq!(v.x, 3.0 + 4.0 + 1.0 + 4.0 + 4.0);
        let w = lift_third_derivative([cs, css, css], [e, e], [e, e, e], [e, e, e, e]);
        assert_eq!(
            w.x,
            3.0 + 4.0 + 9.0 + 3.0 * 10.0 + 24.0 + 1.0 + 6.0 + 12.0 + 8.0
        );
    }

    #[test]
    fn read_stops_at_the_surface() {
        use crate::topology::database_io::InputStream;
        let text = "0 0 1 1 SmCrvOnSurf Domain of interest \n1 SmCrvOnSurf Has breaks flag \n";
        let mut db = DatabaseReader::with_stream(FileType::Ascii, InputStream::new(text.into()));
        assert!(CrvOnSurf::read_from_db(&Kernel::default(), &mut db, 0).is_err());
    }

    #[test]
    fn offset_approximation_keeps_duplicate_supplied_breaks_and_reuses_the_cache() {
        use crate::surface::offset_surface::OffsetSurface;

        let k = Kernel::default();
        let surface = OffsetSurface::new(&k, -0.1, patch().into(), true, 0).unwrap();
        let mut curve = CrvOnSurf::new(&k, uv(), surface.into(), None, 0).unwrap();
        let breaks = [0.0, 0.5, 0.5, 1.0];
        let (approximation, gap) = curve.approximate_curve(&k, &breaks, 1e-4, true).unwrap();
        assert_eq!(curve.breaks(), &breaks);
        let point = approximation.evaluate_point(0.4).unwrap();
        assert!(point.distance(curve.evaluate_point(&k, 0.4).unwrap()) <= 1e-4);
        let reused = curve
            .get_or_create_bs_approx(&k, gap, Some(&breaks), Some(true))
            .unwrap();
        assert_eq!(reused.evaluate_point(0.4).unwrap(), point);
        assert_eq!(curve.bs_approx_max_gap_3d(), gap);
        assert_eq!(curve.breaks(), &breaks);
    }
}
