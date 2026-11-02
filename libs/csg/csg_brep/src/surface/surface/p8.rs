//! Surface coincidence, continuity knots, blends, line/curve intersection and the
//! point inversion near a singularity.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`Surface::find_knots_by_max_continuity`] | `SmSurface::FindKnotsByMaxContinuity` |
//! | [`Surface::coincidence_check`], [`Surface::base_coincidence_check`] | `SmSurface::CoincidenceCheck` (virtual; `SmPlane` overrides it) |
//! | [`Surface::create_blend_surface`] | `SmSurface::CreateBlendSurface` |
//! | [`Surface::global_curve_intersect`], [`Surface::global_curve_intersect_in`] | `SmSurface::GlobalCurveIntersect` |
//! | [`Surface::global_line_intersect`], [`Surface::global_line_intersect_in`], [`Surface::global_line_intersect_held`], [`Surface::global_line_intersect_built`], [`Surface::base_global_line_intersect`], [`Surface::base_global_line_intersect_in`] | `SmSurface::GlobalLineIntersect` (virtual; `SmPlane` overrides it) |
//! | [`SpGlobalSolver::local_solve_branch`] | `SmSPGlobalSolver::LocalSolve` |
//! | [`Surface::invert_point_near_singularity`] | `SmSurface::InvertPointNearSingularity` |
//! | [`Surface::local_line_intersect`] | `SmSurface::LocalLineIntersect` |
//! | [`extract_iso_curve`] | `sm_ExtractIsoCurve` (static) |
//! | [`intersect_srf_bndry_crv`] | `sm_IntersectSrfBndryCrv` (static) |
//!
//! Deviations (not upstream, beyond the port): `CoincidenceCheck` keeps the surfaces'
//! natural boundary curves in a memo in each surface value (`BoundaryMemo`), where
//! upstream makes them again for every check. The curves are a function of the surface
//! value, the domain and the kernel's tolerances, so a hit gives what a fresh build
//! gives; `Kernel::memoize_curve_caches` turns it off with the curve cache memo.
//!
//! Deviations: out-parameters become return values. The caller's object handles
//! (`O`, the objects named in solutions) replace the `this` and `&crCurve` pointers
//! upstream stores in them. Solvers read the patches of the surface cache they were
//! built over, which is built fresh for each call
//! ([`crate::surface::surface::p1::get_surface_cache`]); upstream shares it (one
//! cache per surface, `smsurf_GetSurfaceCache`). A caller that holds the surface's
//! cache passes it to [`Surface::global_curve_intersect_in`] /
//! [`Surface::global_line_intersect_in`], so the line and tree solves run on that
//! cache's tree as upstream's do, and so do the coincident `DropAndTrimCurve` traces
//! ([`Surface::drop_and_trim_curve_in`]; the degenerate-curve point solve and the
//! `DropCurve` fallback still fetch their own). A caller that makes many line solves
//! on one unchanged surface keeps its cache in a [`HeldSolveCache`]
//! ([`Surface::global_line_intersect_held`]), built on the first solve and reused as
//! upstream reuses the surface's cache (its coincident traces fetch their own, as
//! without a cache); a failed build is kept as well and its error returned again
//! (upstream retries the build, which fails the same way: the build is
//! deterministic). A caller that can reach the face owning the surface gives
//! [`Surface::global_line_intersect_built`] the build of upstream's cache (the trimmed
//! cache of the face, [`crate::surface::surface::p1::get_surface_cache_with_face`]),
//! run when the solve fetches its cache (`smsurf_GetSurfaceCache`), so the tree is
//! the face's and the coincident traces share it. The
//! optional `SmDerivSurfDefinition` of `CreateBlendSurface` is copied, so its caller
//! does not see the fields upstream overwrites. `CreateBlendSurface` retains the
//! sampler's partial, zero-filled arrays when ignoring an ordinary sampling error,
//! as upstream does; `Unported` and count-safety failures still propagate.
//! `CreateBlendSurface` of a curve that is not a B-spline kind
//! returns `InvalidInput` (upstream signals the error and casts anyway).
//! `SmSPGlobalSolver::LocalSolve` returns `InvalidInput` when the original surface of
//! the leaf node is not the surface of the cache the solver was given (upstream reads
//! the patch through the node's owner whatever it is). `GlobalPointSolve` gained the
//! out-parameter form [`Surface::global_point_solve_into`] so that a caller ignoring
//! its status keeps what was found before the failure. `CoincidenceCheck` keeps the
//! curve cache of each boundary curve for all its intersections, as upstream keeps
//! it on the curve, so each boundary curve builds one cache per check.
//! Unported callees return [`SolidError::Unported`], also where upstream ignores the
//! status. Recursion: none.

use super::p1::{
    get_surface_cache, is_node_valid, is_point_valid_with as is_point_valid, OperandDomain,
    SolveOperands, SurfaceSolveContext,
};
use super::p2::{
    original_surface, CrossSectionValues, DerivSurfDefinition, LsiGlobalSolver, Surface,
    SurfaceEval, SurfaceKind, TangencyType,
};
use super::p4::{is_point_on_normal, NoObject};
use super::p5::DropPointOptions;
use super::p6::SampleOptions;
use crate::curve::curve::p5::CurveMemoKey;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::curve_cache::CurveCache;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{line_closest_point, line_plane_intersect_uv};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{GlobalSolve, SolutionRequested, SolveRequest, SolverOperation};
use crate::geom::local_solve_nd::TerminationReason;
use crate::geom::polar_box::{PolarBox, PolarBoxState};
use crate::geom::solution_array::{
    OperandObject, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::are_same;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::kernel::Kernel;
use crate::math::{
    max2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::{EvaluateFlags, OrientType};
use crate::nurbs::knots::Side;
use crate::surface::plane::Coincidence;
use crate::surface::surface::p7::SpGlobalSolver;
use crate::surface::surface_cache::p1::SurfaceCache;
use crate::surface::surface_cache::BezierPatch;
use std::borrow::Cow;
use std::sync::OnceLock;

/// A surface's solve cache kept by a caller across [`Surface::global_line_intersect_held`]
/// calls (upstream keeps one cache on each surface): `None` until the first call
/// builds it, then the outcome of that build.
pub type HeldSolveCache<O> = Option<Result<SurfaceCache<O>>>;

/// Where `GlobalLineIntersect` takes its surface cache from.
enum LineCacheSource<'c, O: Copy + PartialEq> {
    /// Built for this call.
    Fresh,
    /// The caller's cache of the surface (its tree is built if it has none).
    Caller(&'c mut SurfaceCache<O>),
    /// Built on the first call and kept by the caller.
    Held(&'c mut HeldSolveCache<O>),
    /// Built by the caller, which can reach the surface's face.
    Face(&'c mut (dyn FaceSolveCache<O> + 'c)),
}

/// The cache of a face's surface as a caller holding the face gives it to a solve:
/// upstream's `smsurf_GetSurfaceCache` returns the face's trimmed cache
/// (`SmTrimSrfCache`), whose virtual `PointTest` classifies a potential solution
/// against the face.
pub trait FaceSolveCache<O> {
    /// `smsurf_GetSurfaceCache` of the face's surface, its tree built: the trimmed
    /// cache with every handle naming the solve's surface object (the line solve
    /// reads only node classes, boxes and patches).
    ///
    /// # Errors
    ///
    /// Those of the cache build.
    fn build(&mut self, k: &Kernel) -> Result<SurfaceCache<O>>;

    /// `SmTrimSrfCache::PointTest(uv, ok, NULL, NULL)` of the built cache: whether
    /// `uv` classifies to the face, one of its edges or vertices (true when point
    /// testing is off or the face was modified).
    ///
    /// # Errors
    ///
    /// Those of `SmFace::PointClassify`.
    fn point_test(&mut self, k: &Kernel, uv: Point2) -> Result<bool>;
}

const OP_KNOTS: &str = "surface::surface::p8::Surface::find_knots_by_max_continuity";
const OP_BNDRY: &str = "surface::surface::p8::intersect_srf_bndry_crv";
const OP_COINC: &str = "surface::surface::p8::Surface::base_coincidence_check";
const OP_BLEND: &str = "surface::surface::p8::Surface::create_blend_surface";
const OP_CURVE: &str = "surface::surface::p8::Surface::global_curve_intersect";
const OP_LINE: &str = "surface::surface::p8::Surface::base_global_line_intersect";
const OP_SP: &str = "surface::surface::p8::SpGlobalSolver::local_solve_branch";
const OP_INVERT: &str = "surface::surface::p8::Surface::invert_point_near_singularity";
const OP_LOCAL_LINE: &str = "surface::surface::p8::Surface::local_line_intersect";

/// The 6 x 6 grid of `CoincidenceCheck` (`lNumSamples`).
const NUM_SAMPLES: usize = 6;

// ---- calls into other units (the only unported one is DropCurve, below) ---------------------

/// `smsurf_CheckShapeMatch(sPVV0, sPVV1, tol, angTolRad, bSuccess)`: `bSuccess`. The
/// evaluation matrices are modified in place, as upstream's arrays are.
fn check_shape_match(
    pvv0: &mut [[Vec3; 3]; 3],
    pvv1: &mut [[Vec3; 3]; 3],
    tol_3d: f64,
    ang_tol_rad: f64,
) -> Result<bool> {
    super::p3::check_shape_match(pvv0, pvv1, tol_3d, ang_tol_rad)
}

/// `SmSurface::DropPoint(pt, domain, guess, bSuccess, uv, dist, bIsMulti, op)`: success,
/// the UV point (the guess when none was found) and the distance.
fn surface_drop_point(
    k: &Kernel,
    surface: &Surface,
    pt: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
    op: SolverOperation,
) -> Result<(bool, Point2, f64)> {
    let r = surface.drop_point::<NoObject>(
        k,
        pt,
        domain,
        &DropPointOptions {
            guess_uv: guess,
            operation: op,
            ..DropPointOptions::default()
        },
    )?;
    Ok((r.success, r.result_uv, r.gap))
}

/// `SmSurface::LocalPointSolve(domain, op, pt, guess, bSuccess, sol)`:
/// success and the solution.
fn surface_local_point_solve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    op: SolverOperation,
    pt: Point3,
    guess: Point2,
) -> Result<(bool, Solution<O>)> {
    let r = surface.local_point_solve::<O>(k, domain, op, pt, guess)?;
    Ok((r.found, r.solution))
}

/// `sm_SpLocalSolve(pSurfaceNode, pBezPatch, uvDomain, uConts, vConts, testPnt, tol,
/// op, bSolveForSurface, bProcessBoundaryCurves, sSolutions)`: the solutions `[u, v]`
/// of the leaf. `eval_surface` is `sm_GetSurfaceToEvaluate(node, patch surface)`, the
/// original surface of the node's cache (the patch itself is not read by the port).
fn sp_local_solve<O: SolutionObject>(
    k: &Kernel,
    _patch: &BezierPatch<O>,
    eval_surface: &Surface,
    domain: &Extent2d,
    u_conts: [ContinuityType; 2],
    v_conts: [ContinuityType; 2],
    point: Point3,
    tol_3d: f64,
    operation: SolverOperation,
    solve_for_surface: bool,
    process_boundary_curves: bool,
) -> Result<SolutionArray<O>> {
    super::p5::sp_local_solve::<O>(
        k,
        eval_surface,
        domain,
        u_conts,
        v_conts,
        point,
        tol_3d,
        operation,
        solve_for_surface,
        process_boundary_curves,
    )
}

/// `SmSurface::DropCurve(ctx, domain, curve, interval, tol, .., keepAll, NULL, FALSE)`
/// (virtual): the UV curves of the drop, the largest drop distance and the deviation.
fn surface_drop_curve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    keep_all: bool,
    context: Option<DropSolveContext<'_, O>>,
) -> Result<(Vec<Curve>, f64, f64)> {
    let d = match context {
        Some(context) => surface.drop_curve_with_context::<O>(
            k, domain, curve, interval, tol, keep_all, None, false, context,
        )?,
        None => surface.drop_curve::<O>(k, domain, curve, interval, tol, keep_all, None, false)?,
    };
    Ok((d.uv_curves, d.max_drop_to_surf, d.max_approx_dev))
}

// ---- helpers --------------------------------------------------------------------------------

/// `Evaluate(uv, 2, 2, TRUE, TRUE, TRUE (bOnlyUpperHalf), &sPVV[0][0])` whose status upstream ignores: on
/// failure `out` keeps what it held (upstream's array is left as it was).
fn evaluate_3x3_into(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    out: &mut [[Vec3; 3]; 3],
) -> Result<()> {
    // bOnlyUpperHalf = TRUE: only the entries with i + j <= 2 are written
    let flags = EvaluateFlags {
        only_upper_half: true,
        ..EvaluateFlags::default()
    };
    let Some(m) = ignored(surface.evaluate(k, uv, 2, 2, flags))? else {
        return Ok(());
    };
    for (i, row) in out.iter_mut().enumerate() {
        for (j, slot) in row.iter_mut().enumerate() {
            if i + j > 2 {
                continue;
            }
            if let Some(v) = m.get(i).and_then(|r| r.get(j)) {
                *slot = *v;
            }
        }
    }
    Ok(())
}

/// `EvaluatePoint(uv, pt)` whose status upstream ignores: `Vec3::UNDEF` on failure.
fn point_or_undef(k: &Kernel, surface: &Surface, uv: Point2) -> Result<Point3> {
    Ok(ignored(surface.evaluate_point(k, uv))?.unwrap_or(Vec3::UNDEF))
}

/// The B-spline part of a curve of a kind derived from `SmBSplineCurve`.
fn b_spline_part(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        _ => None,
    }
}

/// The 3D side curves of the natural UV trim loop of `surface` over `domain`
/// (`CreateNaturalUVTrimCurves(ctx, domain, SM_SP_V, FALSE, ..)`); the status is
/// not checked upstream.
/// The key of a memoised [`trim_curves`] result (not upstream): the kernel inputs of the
/// build ([`CurveMemoKey::of_kernel`]) and the domain's bits.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BoundaryMemoKey {
    kernel: CurveMemoKey,
    domain: [u64; 4],
}

impl BoundaryMemoKey {
    /// The key of `trim_curves(k, surface, domain)`, or `None` when it must not be
    /// memoised: the memo is off, or the surface is not a pure value. B-spline, plane,
    /// cone, sphere and torus surfaces build their sides from their own value (their
    /// `CreateIsoParametricCurve` overrides read nothing else); the other kinds hold
    /// curves, other surfaces or volumes, or approximate through the base body.
    fn of(k: &Kernel, surface: &Surface, domain: &Extent2d) -> Option<Self> {
        let pure_kind = matches!(
            surface.kind(),
            SurfaceKind::BSpline(_)
                | SurfaceKind::Plane(_)
                | SurfaceKind::Cone(_)
                | SurfaceKind::Sphere(_)
                | SurfaceKind::Torus(_)
        );
        if !pure_kind {
            return None;
        }
        Some(Self {
            kernel: CurveMemoKey::of_kernel(k)?,
            domain: [
                domain.min().x.to_bits(),
                domain.min().y.to_bits(),
                domain.max().x.to_bits(),
                domain.max().y.to_bits(),
            ],
        })
    }
}

/// One memoised [`trim_curves`] result.
#[derive(Debug)]
struct BoundaryMemoEntry {
    key: BoundaryMemoKey,
    curves: Vec<Option<Curve>>,
}

/// The memo of a surface's natural boundary curves for `CoincidenceCheck` (not
/// upstream: upstream makes the four curves again for every check, here every face
/// pair of a boolean). It lives in the [`Surface`] value and is filled once; a copy
/// starts an empty one and an edit through `&mut Surface` replaces it. The curves are a
/// function of the surface value and the key, so a hit gives what a fresh build gives,
/// and the curves carry their own curve cache memos, so their caches are built once
/// too. Lent by reference: the check only reads the curves. One entry per live surface,
/// freed with it; no `unsafe`, and the surface stays `Send`. The entry is boxed so the
/// memo adds only a small cell, not the whole entry, to every `Surface` value.
#[derive(Default)]
pub(crate) struct BoundaryMemo(OnceLock<Box<BoundaryMemoEntry>>);

impl core::fmt::Debug for BoundaryMemo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // the memo is not part of the surface's observable state
        f.write_str("BoundaryMemo")
    }
}

impl BoundaryMemo {
    /// An empty memo.
    pub(crate) const fn new() -> Self {
        Self(OnceLock::new())
    }
}

/// [`trim_curves`] through the surface's [`BoundaryMemo`]: borrowed from the memo on a
/// hit (or after filling it), built and owned when the key differs or the surface is
/// not memoised.
fn boundary_curves<'s>(
    k: &Kernel,
    surface: &'s Surface,
    domain: &Extent2d,
) -> Result<Cow<'s, [Option<Curve>]>> {
    let Some(key) = BoundaryMemoKey::of(k, surface, domain) else {
        return trim_curves(k, surface, domain).map(Cow::Owned);
    };
    let memo = &surface.boundary_memo().0;
    if let Some(entry) = memo.get() {
        if entry.key == key {
            return Ok(Cow::Borrowed(&entry.curves));
        }
        return trim_curves(k, surface, domain).map(Cow::Owned);
    }
    let curves = trim_curves(k, surface, domain)?;
    if let Err(entry) = memo.set(Box::new(BoundaryMemoEntry { key, curves })) {
        return Ok(Cow::Owned(entry.curves));
    }
    match memo.get() {
        Some(entry) => Ok(Cow::Borrowed(&entry.curves)),
        None => trim_curves(k, surface, domain).map(Cow::Owned),
    }
}

fn trim_curves(k: &Kernel, surface: &Surface, domain: &Extent2d) -> Result<Vec<Option<Curve>>> {
    let t = ignored(surface.create_natural_uv_trim_curves(k, domain, SurfParamType::V, false))?;
    Ok(t.map(|t| t.curves_3d.into_iter().map(Some).collect())
        .unwrap_or_default())
}

/// The UV point and the inward UV binormal of parameter `t` on boundary `index` of
/// `domain` (0: v min, 1: u max, 2: v max, 3: u min).
fn boundary_uv(domain: &Extent2d, index: usize, t: f64) -> (Point2, Vec2) {
    match index {
        0 => (Point2::new(t, domain.min().y), Vec2::new(0.0, 1.0)),
        1 => (Point2::new(domain.max().x, t), Vec2::new(-1.0, 0.0)),
        2 => (Point2::new(t, domain.max().y), Vec2::new(0.0, -1.0)),
        3 => (Point2::new(domain.min().x, t), Vec2::new(1.0, 0.0)),
        // the switch upstream leaves the values unset; there are never more than 4 sides
        _ => (Point2::new(t, domain.min().y), Vec2::new(0.0, 0.0)),
    }
}

/// `eStat = GlobalCurveIntersect(..)`: the intersections of two boundary curves, none
/// when the call fails. `cache0` and `cache1` hold the curves' caches across the
/// calls of one coincidence check, as upstream keeps them on the boundary curves.
#[allow(clippy::too_many_arguments)]
fn boundary_intersections(
    k: &Kernel,
    crv0: &Curve,
    dom0: &Extent1d,
    crv1: &Curve,
    dom1: &Extent1d,
    tol: f64,
    cache0: &mut Option<CurveCache<OperandObject>>,
    cache1: &mut Option<CurveCache<OperandObject>>,
) -> Result<SolutionArray<NoObject>> {
    Ok(ignored(
        crv0.global_curve_intersect_unnamed_cached(k, dom0, crv1, dom1, tol, false, cache0, cache1),
    )?
    .unwrap_or_default())
}

// ---- SmSurface members ------------------------------------------------------------------------

impl Surface {
    /// The interior knots of both directions where the surface continuity is at most
    /// `max_continuity` (exactly, with `at_continuity`): `(u knots, v knots)`
    /// (`FindKnotsByMaxContinuity`).
    ///
    /// # Errors
    ///
    /// Those of `CalculateContinuities` and `GetKnots`.
    pub fn find_knots_by_max_continuity(
        &self,
        k: &Kernel,
        max_continuity: ContinuityType,
        at_continuity: bool,
    ) -> Result<(Vec<f64>, Vec<f64>)> {
        let mut found = [Vec::new(), Vec::new()];
        for (dir, out) in [SurfParamType::U, SurfParamType::V]
            .into_iter()
            .zip(found.iter_mut())
        {
            let (_, continuities) = self.calculate_continuities(k, dir)?;
            let (knots, _) = self.knots_with_nurb(k, dir, None)?;
            // note: can't say knots.len() - 1
            let mut i = 1;
            while i + 1 < knots.len() {
                let c = continuities
                    .get(i)
                    .copied()
                    .ok_or(SolidError::sm_err(OP_KNOTS))?;
                if (!at_continuity && (c as i32) <= (max_continuity as i32))
                    || (at_continuity && c == max_continuity)
                {
                    out.push(knots.get(i).copied().ok_or(SolidError::sm_err(OP_KNOTS))?);
                }
                i += 1;
            }
        }
        let [u, v] = found;
        Ok((u, v))
    }

    /// Whether `other` coincides with this surface within `tol_3d` over both whole
    /// domains (`whole_surface`) or over any overlap (`CoincidenceCheck`): the plane
    /// override for a plane, else [`Surface::base_coincidence_check`].
    ///
    /// # Errors
    ///
    /// Those of the kind's body.
    pub fn coincidence_check(
        &self,
        k: &Kernel,
        other: &Self,
        tol_3d: f64,
        whole_surface: bool,
    ) -> Result<Coincidence> {
        match self.kind() {
            SurfaceKind::Plane(p) => p.coincidence_check(k, other, tol_3d, whole_surface),
            _ => self.base_coincidence_check(k, other, tol_3d, whole_surface),
        }
    }

    /// `SmSurface::CoincidenceCheck` (the base body): a plane `other` runs its own
    /// check. The same object is coincident. Disjoint boxes are not. For whole
    /// surfaces the four corners must match (in either orientation of the
    /// parameterisation, which is then worked out) and a 6x6 grid of this surface's
    /// points must drop within tolerance onto `other`. For overlaps a boundary curve
    /// of one must meet a boundary curve of the other where the surfaces agree in
    /// second order shape, or the middle of one must drop onto the other where they
    /// agree. `max_distance` is the largest drop distance of a whole surface check.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a surface without domain; [`SolidError::Unported`] from the
    /// callees of the point drops, the evaluations and the shape checks.
    #[allow(clippy::too_many_lines)]
    pub fn base_coincidence_check(
        &self,
        k: &Kernel,
        other: &Self,
        tol_3d: f64,
        whole_surface: bool,
    ) -> Result<Coincidence> {
        // init output
        let mut res = Coincidence {
            are_coincident: false,
            max_distance: BIG_DOUBLE,
        };

        // For evaluation (upstream's sPVV0/sPVV1 start undefined)
        let mut pvv0 = [[Vec3::UNDEF; 3]; 3];
        let mut pvv1 = [[Vec3::UNDEF; 3]; 3];

        // low work - surfaces are the same
        if core::ptr::eq(self, other) {
            res.are_coincident = true;
            res.max_distance = 0.0;
            return Ok(res);
        }

        // If the other surface is a plane, use its method.
        if let SurfaceKind::Plane(p) = other.kind() {
            return p.coincidence_check(k, self, tol_3d, whole_surface);
        }

        // arrive here when both surfaces are not of type SmPlane_TYPE
        let domain0 = self.natural_uv_domain_with_nurb(k)?;
        let domain1 = other.natural_uv_domain_with_nurb(k)?;

        // Bad input - Surface types that don't have domains.
        if domain0.size().length_squared() < EFF_ZERO_SQ
            || domain1.size().length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_COINC));
        }

        // Check bounding boxes (statuses ignored)
        let box_of = |s: &Self, d: &Extent2d| -> Result<Extent3d> {
            Ok(
                ignored(s.calculate_bounding_box(k, d, [true, false, false], true))?
                    .and_then(|b| b.normal_box)
                    .unwrap_or_default(),
            )
        };
        let bbox0 = box_of(self, &domain0)?;
        let bbox1 = box_of(other, &domain1)?;
        if bbox0.are_disjoint(&bbox1, 0.0) {
            return Ok(res);
        }

        if whole_surface {
            return self.whole_surface_coincidence(k, other, &domain0, &domain1, tol_3d, res);
        }

        // Now we're looking for any partial overlap.

        // Create two sets of boundary curves.
        // (not upstream: kept on the surfaces across checks, see BoundaryMemo)
        let all0 = boundary_curves(k, self, &domain0)?;
        let all1 = boundary_curves(k, other, &domain1)?;
        let mut bdrys0: Vec<Option<&Curve>> = all0.iter().map(Option::as_ref).collect();
        let mut bdrys1: Vec<Option<&Curve>> = all1.iter().map(Option::as_ref).collect();

        // We check angles a lot, so get a good angular tolerance.
        let bbox = bbox0.union(&bbox1);
        let ang_tol_rad = tol_3d / bbox.size().length();

        // The inner loop gets called four times for each curve.
        // Factor out some things that are better done just once for each.
        for slot in &mut bdrys1 {
            let degenerate = match slot {
                Some(c) => c.is_degenerate(k, EFF_ZERO, None)?,
                None => false,
            };
            if degenerate {
                *slot = None;
            }
        }

        // The boundary curves' caches, built at their first intersection and kept
        // for the rest of the check (upstream keeps them on the curves).
        let mut caches0: Vec<Option<CurveCache<OperandObject>>> = Vec::new();
        caches0.resize_with(bdrys0.len(), || None);
        let mut caches1: Vec<Option<CurveCache<OperandObject>>> = Vec::new();
        caches1.resize_with(bdrys1.len(), || None);

        // Set this True as default, so that we can just return if found.
        res.are_coincident = true;

        // Double loop over the two surface's boundary curves.
        for ((ii, crv0), cache0) in bdrys0.iter_mut().enumerate().zip(caches0.iter_mut()) {
            let Some(crv0) = crv0.as_ref() else { continue };
            if crv0.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }
            let crv_dom0 = if ii == 0 || ii == 2 {
                domain0.u_interval()
            } else {
                domain0.v_interval()
            };

            for ((jj, crv1), cache1) in bdrys1.iter().enumerate().zip(caches1.iter_mut()) {
                let Some(crv1) = crv1.as_ref() else { continue };
                let crv_dom1 = if jj == 0 || jj == 2 {
                    domain1.u_interval()
                } else {
                    domain1.v_interval()
                };

                // Check for intersections.
                let sols = boundary_intersections(
                    k, crv0, &crv_dom0, crv1, &crv_dom1, tol_3d, cache0, cache1,
                )?;

                // Check each boundary intersection for surface coincidence.
                for sol in sols.as_slice() {
                    // Note, for range sols we might think to use the midpoints.
                    // However, the parameterizations of the curves might well be
                    // different enough to make that invalid. So just use an end point.
                    let t0 = sol.start.param(0)?;
                    let t1 = sol.start.param(1)?;

                    // Get the surface uv's from the edge curve params.
                    let (uv0, uv_binorm0) = boundary_uv(&domain0, ii, t0);
                    let (uv1, uv_binorm1) = boundary_uv(&domain1, jj, t1);

                    // Now check the surfaces for coincidence at this point.
                    evaluate_3x3_into(k, self, uv0, &mut pvv0)?;
                    evaluate_3x3_into(k, other, uv1, &mut pvv1)?;

                    // Now check 2nd order terms.
                    let matched =
                        ignored(check_shape_match(&mut pvv0, &mut pvv1, tol_3d, ang_tol_rad))?
                            .unwrap_or(false);
                    if !matched {
                        continue;
                    }

                    if sol.solution_type == SolutionType::SingleValue {
                        // If this hits the interior of both boundaries
                        if !crv_dom0.is_value_on_boundary(t0, EFF_ZERO).0
                            && !crv_dom1.is_value_on_boundary(t1, EFF_ZERO).0
                        {
                            return Ok(res);
                        }
                    } else {
                        // Range solution: edge-to-edge. Need to check binormals.
                        let binorm0 =
                            uv_binorm0.x * at3(&pvv0, 1, 0) + uv_binorm0.y * at3(&pvv0, 0, 1);
                        let binorm1 =
                            uv_binorm1.x * at3(&pvv1, 1, 0) + uv_binorm1.y * at3(&pvv1, 0, 1);
                        if binorm0.dot(binorm1) > 0.0 {
                            return Ok(res);
                        }
                    }
                }
            }
        }

        // Ok, no edge/edge intersections.
        // If there is coincidence, then one surface completely contains the other.
        // Drop any point from each to the other.
        let mut uv1 = domain1.mid();
        let pt1 = point_or_undef(k, other, uv1)?;
        let in_box = bbox0.contains_point(pt1, 0.0);
        let dropped = if in_box {
            ignored(surface_drop_point(
                k,
                self,
                pt1,
                &domain0,
                None,
                SolverOperation::Intersect,
            ))?
        } else {
            None
        };
        if let Some((true, uv0, dist)) = dropped {
            if dist <= tol_3d {
                // Point coincidence, check normals and curvature.
                evaluate_3x3_into(k, self, uv0, &mut pvv0)?;
                evaluate_3x3_into(k, other, uv1, &mut pvv1)?;
                if ignored(check_shape_match(&mut pvv0, &mut pvv1, tol_3d, ang_tol_rad))?
                    .unwrap_or(false)
                {
                    return Ok(res);
                }
            }
        }

        // Repeat the other way around.
        let uv0 = domain0.mid();
        let pt0 = point_or_undef(k, self, uv0)?;
        let in_box = bbox1.contains_point(pt0, 0.0);
        let dropped = if in_box {
            ignored(surface_drop_point(
                k,
                other,
                pt0,
                &domain1,
                None,
                SolverOperation::Intersect,
            ))?
        } else {
            None
        };
        if let Some((true, found_uv1, dist)) = dropped {
            uv1 = found_uv1;
            if dist <= tol_3d {
                // Point coincidence, check normals and curvature.
                evaluate_3x3_into(k, self, uv0, &mut pvv0)?;
                evaluate_3x3_into(k, other, uv1, &mut pvv1)?;
                if ignored(check_shape_match(&mut pvv0, &mut pvv1, tol_3d, ang_tol_rad))?
                    .unwrap_or(false)
                {
                    return Ok(res);
                }
            }
        }

        // If we arrive here, we haven't found coincidence.
        res.are_coincident = false;
        Ok(res)
    }

    /// The whole surface branch of [`Surface::base_coincidence_check`]: matches the
    /// corners to find how the parameterisations map onto each other, then drops a
    /// 6x6 grid of this surface's points to `other`.
    #[allow(clippy::too_many_lines)]
    fn whole_surface_coincidence(
        &self,
        k: &Kernel,
        other: &Self,
        domain0: &Extent2d,
        domain1: &Extent2d,
        tol_3d: f64,
        mut res: Coincidence,
    ) -> Result<Coincidence> {
        // First collect the corner points: Eval Surfaces at normalized domain corners
        let mut pts1 = [[Vec3::UNDEF; 2]; 2];
        let mut pts2 = [[Vec3::UNDEF; 2]; 2];
        for ii in 0..2 {
            for jj in 0..2 {
                let (fi, fj) = (f64::from(ii as u8), f64::from(jj as u8));
                // surface1 corners
                if let Some(slot) = pts1.get_mut(ii).and_then(|r| r.get_mut(jj)) {
                    *slot = point_or_undef(k, self, domain0.evaluate(fi, fj))?;
                }
                // surface2 corners
                if let Some(slot) = pts2.get_mut(ii).and_then(|r| r.get_mut(jj)) {
                    *slot = point_or_undef(k, other, domain1.evaluate(fi, fj))?;
                }
            }
        }
        let p1 = |i: usize, j: usize| {
            pts1.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(Vec3::UNDEF)
        };
        let p2 = |i: usize, j: usize| {
            pts2.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(Vec3::UNDEF)
        };

        // Find out which other corner matches this->corner[0][0].
        let mut idx00: i32 = -1;
        let mut jdx00: i32 = -1;
        'search: for ii in 0..2 {
            for jj in 0..2 {
                if p1(0, 0).closer_than(tol_3d, p2(ii, jj)) {
                    idx00 = ii as i32;
                    jdx00 = jj as i32;
                    break 'search;
                }
            }
        }

        // not coincident - when no other->corner matches this->corner[0][0]
        if idx00 < 0 {
            return Ok(res);
        }
        let (i00, j00) = (idx00 as usize, jdx00 as usize);

        // not coincident - When opposite corners don't match
        if !p1(1, 1).closer_than(tol_3d, p2(1 - i00, 1 - j00)) {
            return Ok(res);
        }

        // There are now 2 possible this->corners can match other->Corner[idx00][1-jdx00]
        let (idx10, jdx10): (i32, i32);
        if p1(1, 0).closer_than(tol_3d, p2(i00, 1 - j00)) {
            // not coincident - when this->corner[0][1] does not match opposite
            // other->corner[1-idx00][jdx00]
            if !p1(0, 1).closer_than(tol_3d, p2(1 - i00, j00)) {
                return Ok(res);
            }
            idx10 = idx00;
            jdx10 = 1 - jdx00;
        } else if p1(1, 0).closer_than(tol_3d, p2(1 - i00, j00)) {
            // not coincident - when this->corner[1][0] does not match opposite
            // other->corner[idx00][1-jdx00]
            if !p1(0, 1).closer_than(tol_3d, p2(i00, 1 - j00)) {
                return Ok(res);
            }
            idx10 = 1 - idx00;
            jdx10 = jdx00;
        } else {
            // not coincident - when other->Corner[idx00][1-jdx00] has no this->corner
            // matches
            return Ok(res);
        }

        // arrive here when 4 thisSurface->Corners are coincident with some
        // otherSurface->corners.

        // Set eOrient, dDirU, dDirV.
        let mut dir_u = 1.0;
        let mut dir_v = 1.0;
        let match00 = 2 * idx00 + jdx00;
        let match10 = 2 * idx10 + jdx10;
        let orient = if super::p2::is_even(match00 + match10) {
            OrientType::Same
        } else {
            OrientType::Opposite
        };
        if match00 > match10 {
            dir_u = -1.0;
        }
        if match00 > 0 && match10 > 0 {
            dir_v = -1.0;
        }

        // Now check V seams - periodic in U (this will work for singularities with
        // nonZeroTangent evaluations)
        if p1(0, 0).closer_than(tol_3d, p1(1, 0)) {
            // Closed in u (on surf 1).
            let uv1 = domain0.evaluate(0.0, 0.0);
            let (_, su0, _) = ignored(self.evaluate_1st_derivatives(k, uv1, true, true))?
                .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
            let uv2 = domain1.evaluate(f64::from(idx00 as u8), f64::from(jdx00 as u8));
            let (_, su1, sv1) = ignored(other.evaluate_1st_derivatives(k, uv2, true, true))?
                .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
            let other_dir = if orient == OrientType::Same { su1 } else { sv1 };
            if su0.dot(other_dir) < 0.0 {
                dir_u = -1.0;
            }
        }

        // Now check U seams - periodic in V
        if p1(0, 0).closer_than(tol_3d, p1(0, 1)) {
            // Closed in v (on surf 1).
            let uv1 = domain0.evaluate(0.0, 0.0);
            let (_, _, sv0) = ignored(self.evaluate_1st_derivatives(k, uv1, true, true))?
                .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
            let uv2 = domain1.evaluate(f64::from(idx00 as u8), f64::from(jdx00 as u8));
            let (_, su1, sv1) = ignored(other.evaluate_1st_derivatives(k, uv2, true, true))?
                .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
            let other_dir = if orient == OrientType::Same { sv1 } else { su1 };
            if sv0.dot(other_dir) < 0.0 {
                dir_v = -1.0;
            }
        }

        // arrive here when - parameterization-mapping info is complete: eOrient, dDirU,
        // dDirV. Now just check a grid of sample points. No reason to do boundaries
        // specially. We'll evaluate our surface, and DropPoint to the other, using
        // guess points derived from the parameterization-mapping info.
        res.max_distance = 0.0;

        // For a grid of 6x6 sample points
        let denom = (NUM_SAMPLES - 1) as f64;
        for ii in 0..NUM_SAMPLES {
            let u = ii as f64 / denom;
            for jj in 0..NUM_SAMPLES {
                // skip corners - already did the corners
                if (ii == 0 || ii == NUM_SAMPLES - 1) && (jj == 0 || jj == NUM_SAMPLES - 1) {
                    continue;
                }
                let v = jj as f64 / denom;

                // this surface(dU,dV) point
                let uv1 = domain0.evaluate(u, v);
                let pt1 = point_or_undef(k, self, uv1)?;

                // sUV1 =map=> sUV2
                let normalized_uv2 = super::p2::map_uv(u, v, orient, dir_u, dir_v);
                let guess_uv2 = domain1.evaluate(normalized_uv2.x, normalized_uv2.y);

                let dropped = ignored(surface_drop_point(
                    k,
                    other,
                    pt1,
                    domain1,
                    Some(guess_uv2),
                    SolverOperation::Minimize,
                ))?;

                // Not Coincident - when DropPoint can't find an otherSurface point
                // within dThisXSectTol3d.
                let Some((success, _, dist)) = dropped else {
                    return Ok(res);
                };
                if !success || dist > tol_3d {
                    return Ok(res);
                }

                // Save max dist between surfaces
                if dist > res.max_distance {
                    res.max_distance = dist;
                }
            }
        }

        // all done - Everything matches.
        res.are_coincident = true;
        Ok(res)
    }
}

/// `m[i][j]` of an evaluation matrix.
fn at3(m: &[[Vec3; 3]; 3], i: usize, j: usize) -> Vec3 {
    m.get(i)
        .and_then(|r| r.get(j))
        .copied()
        .unwrap_or(Vec3::UNDEF)
}

// ---- CreateBlendSurface -------------------------------------------------------------------------

impl Surface {
    /// The G1/G2/G3 blend between boundary curve `curve1` of this surface and boundary
    /// curve `curve2` of `surface2` (`CreateBlendSurface`): cross derivatives sampled
    /// along both curves (each with the other as tangent reference), interpolated
    /// into cross sections that are skinned with the curves as rails.
    /// `off_to_the_right` picks the side of `curve1`, `in_from_the_left` that of
    /// `curve2`; `tolerance` `0` interpolates the samples, else approximates. The
    /// optional definition must describe a blend from surface boundary curves; its
    /// tangency type and start/end angles are overridden as upstream does.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a curve that is not a B-spline kind; `SM_ERR` for an invalid
    /// definition or a failing skin; count-safety failures from the sampler;
    /// [`SolidError::Unported`] from the evaluations (`surface.surface_p5`,
    /// `DropPoint`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_blend_surface(
        &self,
        k: &Kernel,
        curve1: &Curve,
        ivl1: &Extent1d,
        off_to_the_right: bool,
        surface2: &Self,
        curve2: &Curve,
        ivl2: &Extent1d,
        in_from_the_left: bool,
        tolerance: f64,
        opt_ds_def: Option<&DerivSurfDefinition<'_>>,
    ) -> Result<Self> {
        // check assumptions - curves currently must be BSplineCurves
        let (Some(rail1), Some(rail2)) = (b_spline_part(curve1), b_spline_part(curve2)) else {
            return Err(SolidError::invalid_input(
                OP_BLEND,
                "input curves must currently be SmBSplineCurves",
            ));
        };

        // select blending options
        let mut def = if let Some(d) = opt_ds_def {
            let mut def = *d;
            if !def.is_from_surface_boundary_curve() {
                return Err(SolidError::sm_err(OP_BLEND));
            }
            def.tangency_type = TangencyType::DirToReferenceCurve;
            def.start_angle_deg = 0.0; // no rotations about boundary curves wanted
            def.end_angle_deg = 0.0; // no rotations about boundary curves wanted
            def
        } else {
            DerivSurfDefinition {
                num_samples: 100,
                tangency_type: TangencyType::DirToReferenceCurve,
                start_deriv_dir_angle_deg: 0.0,
                end_deriv_dir_angle_deg: 0.0,
                tangent_reference: None,
                start_tan_length: 1.0,
                end_tan_scale: 1.0,
                start_angle_deg: 0.0,
                end_angle_deg: 0.0,
                vector: Vec3::new(0.0, 0.0, 0.0),
                curvature_type: super::p2::CurvatureType::G2FromSurface,
                blend_end_type: super::p2::BlendEndType::NoCusp,
                start_radius: 0.0,
                end_rad_scale: 0.0,
                curvature_reference: None,
                flip_curvature: false,
            }
        };

        // Sample locals
        let num_derivs = def.num_derivs();
        let want_higher = [num_derivs >= 2, num_derivs >= 3];

        // Get Sample Values from Face1/Edge1 using edge2 as tangentReference
        def.tangent_reference = Some(curve2);
        let (start, start_status) = def.sample_values_with_status(
            k,
            self,
            curve1,
            ivl1,
            SampleOptions {
                off_to_the_right,
                start_at_boundary: true, // blend starts at crCurve1 and moves to the reference
                want_higher,
            },
        )?;
        ignore_status(start_status)?;

        // Get Sample Values from Face2/Edge2 using edge1 as tangentReference
        def.tangent_reference = Some(curve1);
        // Native reuses sParams, but GetSampleValues first calls ReSet, which
        // zeroes its previous contents. The independent end.params matches that.
        let (end, end_status) = def.sample_values_with_status(
            k,
            surface2,
            curve2,
            ivl2,
            SampleOptions {
                off_to_the_right: in_from_the_left,
                start_at_boundary: false, // blend stops at crCurve2 starting from the reference
                want_higher,
            },
        )?;
        ignore_status(end_status)?;

        // use samples to build cross-section curves that capture the G1/G2/G3 shapes
        // (the status is not checked upstream; the curves built before a failure stay in
        // its array, which the port cannot recover, so a failure leaves it empty)
        let cross_sections = ignored(def.create_cross_section_curves(&CrossSectionValues {
            start_pos: &start.positions,
            start_1st_derivs: &start.first_derivs,
            start_2nd_derivs: start.second_derivs.as_deref(),
            start_3rd_derivs: start.third_derivs.as_deref(),
            end_pos: Some(&end.positions),
            end_1st_derivs: Some(&end.first_derivs),
            end_2nd_derivs: end.second_derivs.as_deref(),
            end_3rd_derivs: end.third_derivs.as_deref(),
        }))?
        .unwrap_or_default();

        // Skin cross sections with blend endCurves as Rail constraints to build the
        // G1/G2/G3 blend
        let mut start_rail = rail1.copy();
        let mut end_rail = rail2.copy();
        // (the status is not checked upstream: a NULL blend surface is SM_ERR below)
        let blend = match def.skin_deriv_surface(
            k,
            &cross_sections,
            &end.params,
            &mut start_rail,
            Some(&mut end_rail),
            tolerance,
        ) {
            Ok(blend) => blend,
            // too few intersection parameters: upstream reads and writes past the end of
            // its array here (UB-157), which the port does not reproduce
            Err(e @ (SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })) => {
                return Err(e)
            }
            Err(e) if e.is_unported() => return Err(e),
            // a NULL blend surface
            Err(_) => return Err(SolidError::sm_err(OP_BLEND)),
        };
        Ok(Self::from(blend))
    }
}

// ---- GlobalLineIntersect / GlobalCurveIntersect -----------------------------------------------------

impl Surface {
    /// The cache of this surface for a solve (`smsurf_GetSurfaceCache`, `NER`).
    pub(super) fn solve_cache<O: SolutionObject>(
        &self,
        k: &Kernel,
        handle: O,
        op: &'static str,
    ) -> Result<SurfaceCache<O>> {
        self.solve_cache_of(k, Some(handle), op)
    }

    /// The caller's cache of this surface with its tree built (`GetTree`, `NER`).
    fn held_cache<'c, O: SolutionObject>(
        k: &Kernel,
        cache: &'c mut SurfaceCache<O>,
        op: &'static str,
    ) -> Result<&'c SurfaceCache<O>> {
        match cache.get_tree(k) {
            Ok(_) => Ok(cache),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Err(SolidError::null_pointer(op)),
        }
    }

    /// [`Surface::solve_cache`] for an optional handle.
    fn solve_cache_of<O: SolutionObject>(
        &self,
        k: &Kernel,
        handle: Option<O>,
        op: &'static str,
    ) -> Result<SurfaceCache<O>> {
        let mut cache = match get_surface_cache::<O>(k, self, handle) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(op)),
        };
        // the solves' GetTree (NER): a reused cache whose tree is not built yet
        // (the surface's cache slot) builds it now
        if cache.tree().is_none() {
            Self::held_cache(k, &mut cache, op)?;
        }
        Ok(cache)
    }

    fn solve_cache_from_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        handle: Option<O>,
        op: &'static str,
        context: Option<&mut dyn SurfaceSolveContext<O>>,
    ) -> Result<SurfaceCache<O>> {
        match context {
            None => self.solve_cache_of(k, handle, op),
            Some(context) => context.build_surface_cache(k, self, handle),
        }
    }

    /// Intersections of the line `line_point + t * line_vector` with the surface over
    /// `uv_domain` (`GlobalLineIntersect`): the plane override for a plane, else
    /// [`Surface::base_global_line_intersect`]. `surface` is this surface's handle in
    /// the solutions' objects.
    ///
    /// # Errors
    ///
    /// Those of the kind's body.
    #[allow(clippy::too_many_arguments)]
    pub fn global_line_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_line_intersect_in(
            k,
            Some(surface),
            None,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    /// [`Surface::global_line_intersect`] over `cache`, this surface's cache when the
    /// caller holds it (upstream's `smsurf_GetSurfaceCache` hands every caller the
    /// one cache of the surface, with whatever tree and flags its holder set); `None`
    /// builds one. `surface` (optional) is this surface's handle.
    ///
    /// # Errors
    ///
    /// Those of the kind's body.
    #[allow(clippy::too_many_arguments)]
    pub fn global_line_intersect_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&mut SurfaceCache<O>>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        match self.kind() {
            SurfaceKind::Plane(p) => p.global_line_intersect_in(
                k,
                surface,
                cache,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
            _ => self.base_global_line_intersect_in(
                k,
                surface,
                cache,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
        }
    }

    /// [`Surface::global_line_intersect`] with caller-supplied topology prepared
    /// only if the inherited base solver requests a surface cache. Analytic
    /// overrides keep their existing dispatch and early-return behavior.
    ///
    /// # Errors
    /// Those of the line solve, cache preparation and trimmed point classification.
    #[allow(clippy::too_many_arguments)]
    pub fn global_line_intersect_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&mut SurfaceCache<O>>,
        context: &mut dyn SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        match self.kind() {
            // (the plane's base solve runs on this surface, its face's, with the
            // caller's cache and context)
            SurfaceKind::Plane(p) => p.global_line_intersect_with_base(
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
                |sub_interval| {
                    self.base_global_line_intersect_from(
                        k,
                        surface,
                        match cache {
                            Some(cache) => LineCacheSource::Caller(cache),
                            None => LineCacheSource::Fresh,
                        },
                        Some(context),
                        uv_domain,
                        line_point,
                        line_vector,
                        Some(sub_interval),
                        fire_ray,
                        distance_tolerance,
                    )
                },
            ),
            _ => self.base_global_line_intersect_from(
                k,
                surface,
                match cache {
                    Some(cache) => LineCacheSource::Caller(cache),
                    None => LineCacheSource::Fresh,
                },
                Some(context),
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
        }
    }

    /// `SmSurface::GlobalLineIntersect` (the base body): the line can be a ray
    /// (`fire_ray`, only the closest intersection is kept), an infinite line or a
    /// segment (`line_interval`). The solutions are `[line t, surface u, surface v]`,
    /// in increasing `t`. The line-surface leaf solves of the surface cache's tree are
    /// merged (duplicates dropped, solution pairs that bracket a stretch of line on
    /// the surface become ranges), and a line coincident with the surface becomes the
    /// range(s) of its drop onto it.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a zero line vector; `SM_ERR_NULL_POINTER` when the cache
    /// cannot be built; [`SolidError::Unported`]: `LocalPointSolve`
    /// (`surface.surface_p5`), `DropCurve` (`intersect.surface_drop_lib_p3`) and the
    /// callees of the leaf solves.
    #[allow(clippy::too_many_arguments)]
    pub fn base_global_line_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.base_global_line_intersect_in(
            k,
            Some(surface),
            None,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    /// [`Surface::base_global_line_intersect`] over `cache` (see
    /// [`Surface::global_line_intersect_in`]).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::base_global_line_intersect`].
    #[allow(clippy::too_many_arguments)]
    pub fn base_global_line_intersect_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&mut SurfaceCache<O>>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        let source = match cache {
            Some(c) => LineCacheSource::Caller(c),
            None => LineCacheSource::Fresh,
        };
        self.base_global_line_intersect_from(
            k,
            surface,
            source,
            None,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    /// [`Surface::global_line_intersect`] with the surface's cache kept in `held`
    /// across calls, as upstream keeps it on the surface (`smsurf_GetSurfaceCache`):
    /// the first call builds it ([`get_surface_cache`]) and stores the outcome, later
    /// calls reuse it. A failed build is kept too: the build is deterministic, so
    /// building again from the same surface fails the same way (upstream retries it).
    /// A plane builds its own cache (only its coincident-line branch needs one).
    /// `held` must only ever be used with this surface, unchanged, and `surface`.
    ///
    /// # Errors
    ///
    /// As [`Surface::global_line_intersect`]; a kept failed build returns its error
    /// again.
    #[allow(clippy::too_many_arguments)]
    pub fn global_line_intersect_held<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        held: &mut HeldSolveCache<O>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_line_intersect_held_from(
            k,
            surface,
            held,
            None,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    /// [`Self::global_line_intersect_held`] with an independent topology service.
    /// A retained trimmed cache receives classification even when no build occurs.
    /// `held` and the context must name the same unchanged geometry and topology.
    ///
    /// # Errors
    /// Those of the solve, cache preparation and face classification.
    pub fn global_line_intersect_held_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        held: &mut HeldSolveCache<O>,
        context: &mut dyn SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_line_intersect_held_from(
            k,
            surface,
            held,
            Some(context),
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    fn global_line_intersect_held_from<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        held: &mut HeldSolveCache<O>,
        context: Option<&mut dyn SurfaceSolveContext<O>>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        match self.kind() {
            SurfaceKind::Plane(p) => p.global_line_intersect_in(
                k,
                Some(surface),
                None,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
            _ => self.base_global_line_intersect_from(
                k,
                Some(surface),
                LineCacheSource::Held(held),
                context,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
        }
    }

    /// [`Surface::global_line_intersect`] whose surface cache, when the solve fetches
    /// one (`smsurf_GetSurfaceCache`), is `face_cache`'s: the caller that holds the
    /// face owning this surface builds the face's trimmed cache, and the solve's
    /// `sm_IsPointValid` runs its point test. A plane builds its own cache (only its
    /// coincident-line branch needs one), as in [`Surface::global_line_intersect_held`].
    /// The cache is built at most once.
    ///
    /// # Errors
    ///
    /// As [`Surface::global_line_intersect`]; a failed build is `SM_ERR_NULL_POINTER`
    /// (upstream's `NER`), [`SolidError::Unported`] passes through.
    #[allow(clippy::too_many_arguments)]
    pub fn global_line_intersect_built<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        face_cache: &mut dyn FaceSolveCache<O>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        match self.kind() {
            SurfaceKind::Plane(p) => p.global_line_intersect_in(
                k,
                Some(surface),
                None,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
            _ => self.base_global_line_intersect_from(
                k,
                Some(surface),
                LineCacheSource::Face(face_cache),
                None,
                uv_domain,
                line_point,
                line_vector,
                line_interval,
                fire_ray,
                distance_tolerance,
            ),
        }
    }

    /// The body of [`Surface::base_global_line_intersect_in`] with the cache from
    /// `source`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn base_global_line_intersect_from<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        source: LineCacheSource<'_, O>,
        mut context: Option<&mut dyn SurfaceSolveContext<O>>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        // check input - LineVector must be NonZero
        if line_vector.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::invalid_input(OP_LINE, "zero line vector"));
        }

        // init output
        let mut sols: SolutionArray<O> = SolutionArray::new();

        // local Surface Cache
        // (`held`: the caller's cache, which the coincident traces below share)
        let mut held: Option<&mut SurfaceCache<O>> = None;
        let mut built;
        let mut owned;
        // the trimmed cache's point test (`sm_IsPointValid` -> `PointTest`)
        let mut face_cache: Option<&mut (dyn FaceSolveCache<O> + '_)> = None;
        let cache = match source {
            LineCacheSource::Face(source) => {
                owned = match source.build(k) {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Err(SolidError::null_pointer(OP_LINE)),
                };
                face_cache = Some(source);
                // the face's source ran the trimmed cache's own GetTree: the base
                // cache's cannot rebuild a face-owned tree
                let has_tree = owned.tree().is_some();
                held = Some(&mut owned);
                match held.as_deref_mut() {
                    Some(c) if has_tree => &*c,
                    Some(c) => Self::held_cache(k, c, OP_LINE)?,
                    None => return Err(SolidError::null_pointer(OP_LINE)),
                }
            }
            LineCacheSource::Caller(c) => {
                // A caller solving through a face-aware context holds a cache that
                // context built, which already ran the owning face's virtual GetTree;
                // the bare base cache cannot rebuild a face-owned tree.
                let ready = context.is_some() && c.tree().is_some();
                held = Some(c);
                match held.as_deref_mut() {
                    Some(c) if ready => &*c,
                    Some(c) => Self::held_cache(k, c, OP_LINE)?,
                    None => return Err(SolidError::null_pointer(OP_LINE)),
                }
            }
            LineCacheSource::Fresh => {
                built = self.solve_cache_from_context(
                    k,
                    surface,
                    OP_LINE,
                    context
                        .as_mut()
                        .map(|context| &mut **context as &mut dyn SurfaceSolveContext<O>),
                )?;
                // pSurfaceCache->GetTree() (NER): a cache made while a boolean operation
                // delays its tree builds it now
                // The context already performed the owning face's virtual GetTree.
                // Re-entering the bare base cache cannot rebuild a face-owned tree.
                if context.is_some() && built.tree().is_some() {
                    &built
                } else {
                    Self::held_cache(k, &mut built, OP_LINE)?
                }
            }
            LineCacheSource::Held(kept) => {
                match kept.get_or_insert_with(|| {
                    self.solve_cache_from_context(
                        k,
                        surface,
                        OP_LINE,
                        context
                            .as_mut()
                            .map(|context| &mut **context as &mut dyn SurfaceSolveContext<O>),
                    )
                }) {
                    Ok(c) => &*c,
                    Err(e) => return Err(e.clone()),
                }
            }
        };
        let face_queries = context
            .as_deref()
            .and_then(SurfaceSolveContext::face_queries);
        let tree = cache.tree().ok_or(SolidError::null_pointer(OP_LINE))?;
        let top = tree.top_node().ok_or(SolidError::null_pointer(OP_LINE))?;
        let tree_box = tree.node(top)?.bbox;

        // scale LineVec so that its length is about 1 to improve solution tolerances
        let scale = max2(line_vector.length(), 0.1);
        let vec = line_vector / scale;
        let scaled_ivl = Extent1d::from_min_max(
            line_interval.map_or(0.0, |i| i.min() * scale),
            line_interval.map_or(1.0, |i| i.max() * scale),
        );
        // instance the Solver
        let mut gs = LsiGlobalSolver::new(
            k,
            tree,
            Some(cache.surface()),
            *uv_domain,
            line_point,
            vec, // preScale line: crLinePoint, crLineVector
            line_interval.map(|_| scaled_ivl),
            fire_ray,
            distance_tolerance,
        );
        gs.surface_cache = Some(cache);
        gs.face_queries = face_queries;
        gs.face_cache = face_cache;

        // traverse curve and surface sub-division trees
        // for each sub-division leaf pair that has intersecting boundaries
        //   call SmLSIGlobalSolver::LocalSolve()
        gs.solve_it(
            &SolveRequest {
                operation: SolverOperation::Intersect,
                solution_requested: SolutionRequested::All,
                tolerance_3d: distance_tolerance,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            },
            &mut sols,
        )?;
        // restore the original scaling to solution parameters
        for ii in 0..sols.len() {
            let sol = sols.get_mut(ii).ok_or(SolidError::null_pointer(OP_LINE))?;
            *sol.start.param_mut(0)? /= scale;
            if sol.solution_type == SolutionType::RangeOfValues {
                *sol.end.param_mut(0)? /= scale;
            }
        }

        // If we are rayfiring then we need only the solution closest to the start of
        // the line.
        if fire_ray && !sols.is_empty() {
            sols.set_size(1);
        }

        // Post process checking for coincidences over the intervals between solutions.
        // (This is generally the same code as in GlobalCurveIntersect(), encapsulated in
        // sm_GlobalCurveIntersectCombineSolutions().)

        // Remove duplicate solutions: re-execute the same solution check using the
        // line parameter length test.
        let param_tol = [
            max2(
                EFF_ZERO_SQRT * (1.0 + line_interval.map_or(1.0, Extent1d::max)).abs(),
                line_interval.map_or(1.0, Extent1d::length) * 0.0001,
            ),
            max2(
                EFF_ZERO_SQRT * (1.0 + uv_domain.max().x).abs(),
                (uv_domain.max().x - uv_domain.min().x) * 0.0001,
            ),
            max2(
                EFF_ZERO_SQRT * (1.0 + uv_domain.max().y).abs(),
                (uv_domain.max().y - uv_domain.min().y) * 0.0001,
            ),
        ];

        // for every pair of solutions - find and remove repeated solutions
        let mut ii = 0;
        while ii < sols.len() {
            let mut jj = ii + 1;
            while jj < sols.len() {
                let si = sols.get_at(ii)?;
                let sj = sols.get_at(jj)?;
                let mut same = si.solution_type == sj.solution_type;
                for (kk, tol) in param_tol.iter().enumerate() {
                    if !same {
                        break;
                    }
                    // check type and params to see if they are the same
                    if (si.start.param(kk)? - sj.start.param(kk)?).abs() > *tol
                        || (si.solution_type == SolutionType::RangeOfValues
                            && (si.end.param(kk)? - sj.end.param(kk)?).abs() > *tol)
                    {
                        same = false;
                    }
                }

                // remove duplicates
                if same {
                    // save the best solution
                    if si.start.solution_value > sj.start.solution_value {
                        sols.set_at(ii, &sj)?;
                    }

                    // remove the worst duplicate
                    sols.remove_at(jj, 1);
                } else {
                    jj += 1;
                }
            }
            ii += 1;
        }
        // Now classify SolutionPoints to exclude a total coincident case.
        //  Points are not coincident when
        //   1. line tangent is not perp to surface normal,
        //   2. Surface's NormalSection curvature in direction of line is far from linear.
        let num_sols = sols.len();
        let mut might_be_coincident = num_sols > 0;
        let mut sr_eval = SurfaceEval::with_uv_and_sect_dir(k, Some(self), 0.0, 0.0, line_vector);

        // for every solution - classify solution points as Coincident/NotCoincident
        for ii in 0..num_sols {
            let sol = sols.get_at(ii)?;
            for (end, check) in [
                (&sol.start, true),
                (&sol.end, sol.solution_type == SolutionType::RangeOfValues),
            ] {
                if !check {
                    continue;
                }
                sr_eval.set_uv_coords(end.param(1)?, end.param(2)?);

                // When line tangent not perp to surfNormal - can't be coincident
                ignore_status(sr_eval.eval_normal(k))?;
                let normal = sr_eval.values()[2][2];
                if line_vector.is_perpendicular_to(normal, EFF_ZERO_RAD) {
                    // if SurfaceNormalSection curvature is far from linear - can't be
                    // coincident
                    ignore_status(sr_eval.eval_normal_section(k))?;
                    let curvature = sr_eval.stored_section().1;
                    let surf_size = 2.0 * tree_box.max_dimension();
                    if curvature.abs() > (1.0 / surf_size) {
                        might_be_coincident = false;
                    }
                } else {
                    might_be_coincident = false;
                }
            }
        }

        // when all solution points might be coincident, check for total coincidence
        if might_be_coincident {
            // Do a really fast although not extremely robust check for coincidence

            // create a SmBSplineLine segment with current line interval parameterization
            let mut line_start = line_point;
            let mut line_end = line_point + line_vector;
            let mut line_interval_used = Extent1d::from_min_max(0.0, 1.0);
            if let Some(i) = line_interval {
                line_start = line_point + i.min() * line_vector;
                line_end = line_point + i.max() * line_vector;
                line_interval_used = *i;
            }
            let Ok(mut line) = BSplineCurve::create_line_segment(3, line_start, line_end) else {
                return Ok(sols);
            };
            ignore_status(line.edit_parameterization(&line_interval_used))?;
            let line = Curve::from(line);

            // Check Surface/LineSegment Coincidence (its point solve reaches the
            // surface's face through the caller's context, when there is one)
            let found = if let Some(context) = context.as_deref_mut() {
                self.simple_coincidence_checker_with_context::<O>(
                    k,
                    uv_domain,
                    &line,
                    &line_interval_used,
                    distance_tolerance,
                    true, // bAllowPartialCoinc [B122]
                    [None, surface],
                    Some(cache),
                    context,
                )?
            } else {
                self.simple_coincidence_checker::<O>(
                    k,
                    uv_domain,
                    &line,
                    &line_interval_used,
                    distance_tolerance,
                    true, // bAllowPartialCoinc [B122]
                    [None, surface],
                )?
            };

            // Put the end points of the intersection into separate solutions
            if found.is_some() {
                sols.reset();

                // (the drop's trace gets the surface's cache, `smsurf_GetSurfaceCache`:
                // the caller's, else the face's through a face-aware context)
                let mut from_context;
                let held = match (held, context.as_deref_mut()) {
                    (Some(c), _) => Some(c),
                    (None, Some(context)) if self.face()?.is_some() => {
                        from_context = context.build_surface_cache(k, self, surface)?;
                        Some(&mut from_context)
                    }
                    (None, _) => None,
                };
                let dropped = self.drop_and_trim_curve_in::<NoObject, O>(
                    k,
                    held,
                    uv_domain,
                    &line,
                    &line_interval_used,
                    distance_tolerance,
                    false,
                    true,
                    None,
                )?;
                let mut dist_to_surface = dropped.max_drop_to_surf;
                let mut uv_curves: Vec<Curve> = dropped.uv_curves;

                // If the curve is entirely interior to the surface, then
                // DropAndTrimCurve won't find it; in that case use DropCurve. (Its trace
                // gets the surface's cache as above: the face's through a face-aware
                // context, whose point test classifies against the face.)
                if uv_curves.is_empty() {
                    let (curves, dist, _deviation) = surface_drop_curve(
                        k,
                        self,
                        uv_domain,
                        &line,
                        &line_interval_used,
                        distance_tolerance,
                        false,
                        context.as_mut().map(|solver| DropSolveContext {
                            solver: &mut **solver,
                            handle: surface,
                        }),
                    )?;
                    uv_curves = curves;
                    dist_to_surface = dist;
                }

                // for every UVCurve - add a range solution to rSolutions array
                for uv_curve in &uv_curves {
                    let ivl = uv_curve.natural_interval();
                    let mut sol = Solution::<O>::new();
                    sol.num_variables = 3;
                    sol.solution_type = SolutionType::RangeOfValues;

                    let pnt = uv_curve.evaluate_point(k, ivl.min())?;
                    sol.start.solution_value = dist_to_surface;
                    *sol.start.param_mut(0)? = ivl.min();
                    *sol.start.param_mut(1)? = pnt.x;
                    *sol.start.param_mut(2)? = pnt.y;

                    let pnt = uv_curve.evaluate_point(k, ivl.max())?;
                    sol.end.solution_value = dist_to_surface;
                    *sol.end.param_mut(0)? = ivl.max();
                    *sol.end.param_mut(1)? = pnt.x;
                    *sol.end.param_mut(2)? = pnt.y;

                    sols.add(&sol);
                }
                return Ok(sols);
            }
        }
        // arrive here when: 1. line is not totally coincident with surface and
        //                   2. all rSolutions are ordered with increasing LineParam values.
        //    note: Repeated equal LineParam value rSolutions are expected when the
        //          curve intersects a surface seam or singularity.

        // next: Find and combine solution pairs that bracket line intervals that are
        // coincident with the surface.

        // lProcessEnd :== Index of LastSolution. Increments as solutions are processed
        let mut processed_end = 0;

        // for every unprocessed solution - see if it and LastSolution bracket a
        // coincident line interval
        let mut ii = 1;
        while ii < sols.len() {
            // part 1. Set bDoCombine for LastSolution to CurrentSolution BetweenIvl
            let mut do_combine = false;

            // LastSol locals
            let last_sol = sols.get_at(processed_end)?;
            let last_is_range = last_sol.solution_type == SolutionType::RangeOfValues;
            let last_end = if last_is_range {
                last_sol.end
            } else {
                last_sol.start
            };
            let last_uv = Point2::new(last_end.param(1)?, last_end.param(2)?);

            // When multiple sols have SameLineParam due to curve/SurfSeam or
            // curve/SurfSingularity xsects - use closest to LastEnd in SurfParam space
            let mut jj = ii + 1;
            while jj < sols.len() {
                let si = sols.get_at(ii)?;
                let sj = sols.get_at(jj)?;
                // when next solution has SameLineParam, set Closest = Sol closest to
                // LastEnd in SurfParam space
                if (si.start.param(0)? - sj.start.param(0)?).abs() < EFF_ZERO {
                    // get surfParam space distances to the LastEnd
                    let u_diff1 = (si.start.param(1)? - last_end.param(1)?).abs();
                    let v_diff1 = (si.start.param(2)? - last_end.param(2)?).abs();
                    let u_diff2 = (sj.start.param(1)? - last_end.param(1)?).abs();
                    let v_diff2 = (sj.start.param(2)? - last_end.param(2)?).abs();

                    // target the solution closest to LastEnd in SurfParam space
                    if u_diff2 + v_diff2 < u_diff1 + v_diff1 {
                        // swap solution ordering placing closest solution into ii slot
                        sols.swap(ii, jj)?;
                    }
                } else {
                    // quit at the first NotSameLineParam value sol
                    break;
                }
                jj += 1;
            }

            // Current Solution Locals
            let curr_sol = sols.get_at(ii)?;
            let curr_start = curr_sol.start;
            let curr_uv = Point2::new(curr_start.param(1)?, curr_start.param(2)?);

            // Between Last/Current Solution Locals
            let mut between_ivl = Extent1d::from_value(last_end.param(0)?);
            between_ivl.add_value(curr_start.param(0)?);
            let mut between_uv_domain = Extent2d::new();
            between_uv_domain.add_point(last_uv);
            between_uv_domain.add_point(curr_uv);

            'decide: {
                // NoCoincidentIvlTest: lastEnd LineParamValue > CurrentStart
                // LineParamValue + tol
                if last_end.param(0)? > curr_start.param(0)? + EFF_ZERO_SQRT {
                    // Have a case of range including the next solution (probably a
                    // point solution). That's ok to have a range with a closest point
                    // solution so save both. Or have a case of curve falling along seam
                    // or something nasty like that - don't combine
                    break 'decide;
                }

                // NoCoincidentIvlTest: Solutions across seams, same LineParams, very
                // different SurfParams.
                if between_ivl.length() < EFF_ZERO_SQRT
                    && ((last_uv.x - curr_uv.x).abs() > 0.99 * uv_domain.x_length()
                        || (last_uv.y - curr_uv.y).abs() > 0.99 * uv_domain.y_length())
                {
                    break 'decide;
                }

                // NoCoincidentIvlTest: BetweenUVDomain adjoint to intersection's
                // surfaceTargetUVDomain (disjoint extents return an error)
                match between_uv_domain.intersect(uv_domain) {
                    Ok(d) => between_uv_domain = d,
                    Err(_) => break 'decide,
                }

                // NoCoincidentIvlTest: Line's BetweenIvl MidPoint too far from surface
                {
                    // expand BetweenUVDomain for upcoming LocalPointSolve call.
                    let expand_dist = 1.1
                        * EFF_ZERO
                        * (1.0
                            + max2(
                                max2(last_end.param(1)?.abs(), curr_start.param(1)?.abs()),
                                max2(last_end.param(2)?.abs(), curr_start.param(2)?.abs()),
                            ));
                    between_uv_domain.expand_absolute(expand_dist);

                    // However, the domain must not spill out of the given domain.
                    match between_uv_domain.intersect(uv_domain) {
                        Ok(d) => between_uv_domain = d,
                        Err(_) => break 'decide,
                    }

                    let curve_ivl_mid_pt = line_point + between_ivl.evaluate(0.5) * line_vector;
                    let uv_guess = between_uv_domain.evaluate(0.5, 0.5);

                    // when Line's BetweenIvl MidPoint distance to surface is bigger
                    // than tolerance
                    let (success, solution) = surface_local_point_solve::<O>(
                        k,
                        self,
                        &between_uv_domain,
                        SolverOperation::Minimize,
                        curve_ivl_mid_pt,
                        uv_guess,
                    )?;
                    if !success || solution.start.solution_value > distance_tolerance {
                        break 'decide;
                    }
                }

                // If make it to here we assume the Line's BetweenIvl is coincident to
                // the surface because the Line's BetweenIvl mid point is within
                // tolerance of the surface.
                do_combine = true;
            }

            // Part 2: Arrive here when Line BetweenIvl is classified as
            // Coincident/NotCoincident
            if do_combine {
                // BetweenIvl Solution locals
                let mut last_sol = last_sol;
                let last_original_type = last_sol.solution_type;

                // Set last solution to range
                last_sol.solution_type = SolutionType::RangeOfValues;

                // if CurrSol is a range
                if curr_sol.solution_type == SolutionType::RangeOfValues {
                    // When LastEnd < rCurrStart - set LastEnd = CurrStart
                    //  note: need to check because may have opposite sorting due to
                    //  tolerances
                    if last_end.param(0)? < curr_start.param(0)? {
                        last_sol.end = curr_sol.end;
                        if last_sol.start.param(0)? > last_sol.end.param(0)? {
                            return Err(SolidError::sm_err(OP_LINE));
                        }
                    }
                } else {
                    // if CurrSol is a point

                    // shortIvl tolerance: Note, SM_EFF_ZERO is too small to use here.
                    let tol =
                        line_interval.map_or(EFF_ZERO * 100.0, |i| EFF_ZERO * 100.0 * i.length());

                    // when interval is too short - don't create really short range
                    // solution
                    if (last_sol.start.param(0)? - curr_sol.start.param(0)?).abs() < tol {
                        // restore last interval
                        last_sol.solution_type = last_original_type;
                    } else {
                        // set LastSolution end value
                        last_sol.end = curr_sol.start;

                        // If have opposite sorting because of tolerancing, swap the
                        // ends of rLastSol.
                        if last_sol.start.param(0)? > last_sol.end.param(0)? {
                            std::mem::swap(&mut last_sol.end, &mut last_sol.start);
                        }
                    }
                }
                sols.set_at(processed_end, &last_sol)?;
            } else {
                // Move current solution down to end of processed list - increment
                // processed count
                processed_end += 1;
                if ii != processed_end {
                    sols.set_at(processed_end, &curr_sol)?;
                }
            }
            ii += 1;
        }

        // Shrink solutions down to the proper size after all of resizing has been done.
        if sols.len() > processed_end + 1 {
            sols.set_size(processed_end + 1);
        }

        Ok(sols)
    }

    /// The intersections of `curve` over `interval` with the surface over `uv_domain`
    /// within `tol_3d` (`GlobalCurveIntersect`), sorted along the curve: solutions are
    /// `[curve t, surface u, surface v]`, naming `handles` (the curve, the surface).
    /// A degenerate curve is a point solve; a straight B-spline is a line
    /// intersection; a curve coincident with the surface becomes the range(s) of its
    /// drop onto it; otherwise the curve and surface cache trees are solved and the
    /// solutions combined ([`super::p10::global_curve_intersect_combine_solutions`]).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a cache cannot be built; those of the evaluations,
    /// the sub-solves and the drops; [`SolidError::Unported`] from unported callees
    /// (`surface.surface_p5`, `intersect.surface_drop_lib_p3`, ...).
    pub fn global_curve_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        handles: [O; 2],
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        let [curve_handle, surface_handle] = handles;
        self.global_curve_intersect_in(
            k,
            [Some(curve_handle), Some(surface_handle)],
            None,
            uv_domain,
            curve,
            interval,
            distance_tolerance,
        )
    }

    /// [`Surface::global_curve_intersect`] over `cache`, this surface's cache when the
    /// caller holds it (upstream's `smsurf_GetSurfaceCache` hands every caller the
    /// one cache of the surface, with whatever tree and flags its holder set; here the
    /// line, point and tree solves use it); `None` builds one. The `handles` (curve,
    /// surface) are optional.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::global_curve_intersect`].
    #[allow(clippy::too_many_lines)]
    pub fn global_curve_intersect_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        handles: [Option<O>; 2],
        cache: Option<&mut SurfaceCache<O>>,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_curve_intersect_from(
            k,
            handles,
            cache,
            None,
            uv_domain,
            curve,
            interval,
            distance_tolerance,
        )
    }

    /// [`Surface::global_curve_intersect`] using real owning-face cache context
    /// supplied by the caller. The context is not consulted before the existing
    /// numerical fast paths reach their cache acquisition point.
    ///
    /// # Errors
    /// Those of the intersection, topology preparation and face classification.
    #[allow(clippy::too_many_arguments)]
    pub fn global_curve_intersect_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        handles: [Option<O>; 2],
        cache: Option<&mut SurfaceCache<O>>,
        context: &mut dyn SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_curve_intersect_from(
            k,
            handles,
            cache,
            Some(context),
            uv_domain,
            curve,
            interval,
            distance_tolerance,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn global_curve_intersect_from<O: SolutionObject>(
        &self,
        k: &Kernel,
        handles: [Option<O>; 2],
        cache: Option<&mut SurfaceCache<O>>,
        mut context: Option<&mut dyn SurfaceSolveContext<O>>,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        let [curve_handle, surface_handle] = handles;

        // Special case DegenerateCurve/Surface intersections
        if curve.is_degenerate(k, EFF_ZERO, Some(interval))? {
            let pnt = curve.evaluate_point(k, interval.min())?;

            // pass call to global surface/point intersect (status not checked: what the
            // call found before failing stays in the solutions)
            let mut sols: SolutionArray<O> = SolutionArray::new();
            let solve = if let Some(context) = context {
                self.global_point_solve_into_with_context(
                    k,
                    surface_handle,
                    cache.as_deref(),
                    context,
                    uv_domain,
                    SolverOperation::Intersect,
                    pnt,
                    distance_tolerance,
                    None,
                    SolutionRequested::Single,
                    &mut sols,
                )
            } else {
                self.global_point_solve_into::<O>(
                    k,
                    surface_handle,
                    uv_domain,
                    SolverOperation::Intersect,
                    pnt,
                    distance_tolerance,
                    None,
                    SolutionRequested::Single,
                    &mut sols,
                )
            };
            let _ = super::p1::ignore_native_classification(solve)?;

            // set output values
            for ii in 0..sols.len() {
                let sol = sols.get_mut(ii).ok_or(SolidError::null_pointer(OP_CURVE))?;
                sol.num_objects = 2;
                sol.objects[0] = curve_handle;
                sol.objects[1] = surface_handle;
                *sol.start.param_mut(2)? = sol.start.param(1)?;
                *sol.start.param_mut(1)? = sol.start.param(0)?;
                *sol.start.param_mut(0)? = interval.min();
            }
            return Ok(sols);
        }

        // Special case Line/Surface intersections
        if b_spline_part(curve).is_some() {
            // Note: cannot use IsLinear() here: for GlobalLineIntersect(), we also
            // require constant speed. Use IsLine(). [B196] Tolerance: whatever we use
            // here can result in errors of that magnitude, so use a very tight one.
            if curve.is_line(k, 10, EFF_ZERO)?.is_some() {
                // get Line value at param=0.0 avoiding interval boundary problems
                let pv = curve.evaluate(k, interval.min(), 1, true, false)?;
                let p0 = pv.first().copied().ok_or(SolidError::sm_err(OP_CURVE))?;
                let d1 = pv.get(1).copied().ok_or(SolidError::sm_err(OP_CURVE))?;
                let line_point = p0 - interval.min() * d1;

                // pass call to global surface/line intersect
                let mut sols = if let Some(context) = context {
                    self.global_line_intersect_with_context(
                        k,
                        surface_handle,
                        cache,
                        context,
                        uv_domain,
                        line_point,
                        d1,
                        Some(interval),
                        false,
                        distance_tolerance,
                    )?
                } else {
                    self.global_line_intersect_in(
                        k,
                        surface_handle,
                        cache,
                        uv_domain,
                        line_point,
                        d1,
                        Some(interval),
                        false,
                        distance_tolerance,
                    )?
                };

                // for every solution set the object values
                for ii in 0..sols.len() {
                    let sol = sols.get_mut(ii).ok_or(SolidError::null_pointer(OP_CURVE))?;
                    sol.num_objects = 2;
                    sol.objects[0] = curve_handle;
                    sol.objects[1] = surface_handle;
                }
                return Ok(sols);
            }
        }

        // init output
        let mut sols: SolutionArray<O> = SolutionArray::new();

        // Run quick not robust coincidence check.
        let coincidence = if let Some(context) = context.as_mut() {
            self.simple_coincidence_checker_with_context(
                k,
                uv_domain,
                curve,
                interval,
                distance_tolerance,
                true,
                [curve_handle, surface_handle],
                cache.as_deref(),
                &mut **context,
            )?
        } else {
            self.simple_coincidence_checker::<O>(
                k,
                uv_domain,
                curve,
                interval,
                distance_tolerance,
                true,
                [curve_handle, surface_handle],
            )?
        };
        let coincident = coincidence.is_some();

        // When Coincident - make 3Dcurve projection an interval solution
        //                   more than 1 when curve wanders in and out of surface
        //                   boundary by more than dDistanceTolerance
        if coincident {
            // drop curve to surface (the drop's trace gets the surface's cache,
            // `smsurf_GetSurfaceCache`: the face's, through a face-aware context)
            let mut from_context;
            let cache = match (cache, context.as_mut()) {
                (Some(c), _) => Some(c),
                (None, Some(context)) if self.face()?.is_some() => {
                    from_context = context.build_surface_cache(k, self, surface_handle)?;
                    Some(&mut from_context)
                }
                (None, _) => None,
            };
            let dropped = self.drop_and_trim_curve_in_context::<NoObject, O>(
                k,
                cache,
                uv_domain,
                curve,
                interval,
                distance_tolerance,
                true,
                true,
                None,
                context.as_mut().map(|solver| DropSolveContext {
                    solver: &mut **solver,
                    handle: surface_handle,
                }),
            )?;
            let mut dist_to_surface = dropped.max_drop_to_surf;
            let mut uv_curves: Vec<Curve> = dropped.uv_curves;

            // DropAndTrimCurve() will miss curves that lie entirely within the
            // boundaries of the surface. Use DropCurve() for that. [090827]
            if uv_curves.is_empty() {
                // To find totally interior curves, try dropping the whole curve to the
                // surface asking only for those curves within dDistanceTolerance of the
                // surface in the output.
                if let Some((curves, dist, _deviation)) = ignored(surface_drop_curve(
                    k,
                    self,
                    uv_domain,
                    curve,
                    interval,
                    distance_tolerance,
                    false,
                    context.as_mut().map(|solver| DropSolveContext {
                        solver: &mut **solver,
                        handle: surface_handle,
                    }),
                ))? {
                    uv_curves = curves;
                    dist_to_surface = dist;
                }
            }

            // Set up a Solution for each dropped curve: Range solutions.
            for uv_curve in &uv_curves {
                let ivl = uv_curve.natural_interval();
                let mut sol = Solution::<O>::new();
                sol.num_variables = 3;
                sol.solution_type = SolutionType::RangeOfValues;
                sol.num_objects = 2;
                sol.objects[0] = curve_handle;
                sol.objects[1] = surface_handle;

                let pnt = uv_curve.evaluate_point(k, ivl.min())?;
                sol.start.solution_value = dist_to_surface;
                *sol.start.param_mut(0)? = ivl.min();
                *sol.start.param_mut(1)? = pnt.x;
                *sol.start.param_mut(2)? = pnt.y;

                let pnt = uv_curve.evaluate_point(k, ivl.max())?;
                sol.end.solution_value = dist_to_surface;
                *sol.end.param_mut(0)? = ivl.max();
                *sol.end.param_mut(1)? = pnt.x;
                *sol.end.param_mut(2)? = pnt.y;

                sols.add(&sol);
            }
            return Ok(sols);
        }

        // arrive here - need to do a general surf/curve intersection

        // Get surface cache before the curve cache. (SmSurfaceCache::GetTree() can
        // cause lots of additions to the curve cache, which could push this curve's
        // cache right out of the queue. So get the curve's cache afterwards.)
        let built;
        let surface_cache = if let Some(c) = cache {
            // (a cache of a face-aware context already ran the face's GetTree, as in
            // the line intersection)
            if context.is_some() && c.tree().is_some() {
                &*c
            } else {
                Self::held_cache(k, c, OP_CURVE)?
            }
        } else {
            built = self.solve_cache_from_context(
                k,
                surface_handle,
                OP_CURVE,
                context
                    .as_mut()
                    .map(|context| &mut **context as &mut dyn SurfaceSolveContext<O>),
            )?;
            &built
        };
        let face_queries = context
            .as_deref()
            .and_then(SurfaceSolveContext::face_queries);
        let surface_tree = surface_cache
            .tree()
            .ok_or(SolidError::null_pointer(OP_CURVE))?;
        let curve_cache =
            match crate::curve::curve::p5::make_curve_cache::<O>(k, curve, curve_handle) {
                Ok(c) => c,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Err(SolidError::null_pointer(OP_CURVE)),
            };
        let curve_tree = curve_cache
            .tree()
            .ok_or(SolidError::null_pointer(OP_CURVE))?;

        // instance the Solver
        let mut gs = super::p1::CsiGlobalSolver::new(
            k,
            curve_tree,
            surface_tree,
            Some(surface_cache.surface()),
            coincident,
            *interval,
            *uv_domain,
        );
        gs.curve = Some(curve);
        gs.surface_cache = Some(surface_cache);
        gs.face_queries = face_queries;
        // the solutions name the trees' owners (LocalSolve: the curve, sm_GetOriginalSurface)
        let surface_domain = surface_cache.surface().natural_uv_domain_with_nurb(k)?;
        gs.operands = Some(SolveOperands {
            operands: [
                curve_tree
                    .owner_object()?
                    .map(|h| (h, OperandDomain::Curve(curve.natural_interval()))),
                surface_tree
                    .owner_object()?
                    .map(|h| (h, OperandDomain::Surface(surface_domain))),
            ],
        });

        // traverse curve and surface sub-division trees
        // for each sub-division leaf pair that has intersecting boundaries
        //   call SmCSIGlobalSolver::LocalSolve()
        gs.solve_it(
            &SolveRequest {
                operation: SolverOperation::Intersect,
                solution_requested: SolutionRequested::All,
                tolerance_3d: distance_tolerance,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            },
            &mut sols,
        )?;

        // combine duplicate solutions - find coincident regions (the call is not
        // checked upstream, which returns SM_SUCCESS)
        ignore_status(super::p10::global_curve_intersect_combine_solutions(
            k,
            self,
            uv_domain,
            curve,
            interval,
            distance_tolerance,
            &super::p10::CombineContext {
                curve_tree: Some(curve_tree),
                surface_cache: Some(surface_cache),
                curve_handle,
                surface_handle,
                edge_tolerance: super::p9::curve_edge_tolerance(curve, curve_handle, face_queries)?,
            },
            &mut sols,
        ))?;
        Ok(sols)
    }
}

// ---- SmSPGlobalSolver::LocalSolve --------------------------------------------------------------------

impl<O: SolutionObject> SpGlobalSolver<'_, '_, O> {
    /// `SmSPGlobalSolver::LocalSolve`: solves the point against the Bezier patch of the
    /// surface leaf node `branch[1]` and adds the solutions `[u, v]` kept (inside the
    /// solver's domain and passing the cache's point test) to `base.solutions`.
    /// Patches whose domain misses the solver's, or that are too far from the point
    /// (or, for the normal-based operations, whose normal cone cannot reach it), are
    /// skipped. Returns the request for more subdivision (always false).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without `surface_cache`; `InvalidInput` when the node is
    /// not a patch node of that cache; [`SolidError::Unported`]: `sm_SpLocalSolve`
    /// (`surface.surface_p5`).
    #[allow(clippy::too_many_lines)]
    pub fn local_solve_branch(
        &mut self,
        branch: &crate::geom::global_solver::Branch,
    ) -> Result<bool> {
        let k = self.kernel;
        // locals
        let point_node = *self.base.branch_node(branch, 0)?;
        let surface_node = *self.base.branch_node(branch, 1)?;
        let surface_id = branch
            .get(1)
            .copied()
            .flatten()
            .ok_or(SolidError::null_pointer(OP_SP))?;
        let cache = self.surface_cache.ok_or(SolidError::null_pointer(OP_SP))?;
        let patch = cache.patch(surface_id)?.clone();
        let test_pnt = point_node.bbox.min();
        let tree1 = self.base.tree(1)?;

        // If the classification of the node is outside then we can quit here
        if !is_node_valid(tree1.owner(), cache, &surface_node)? {
            return Ok(false);
        }

        // when BezierPatch is approximate get original Surface else use BezierPatch
        let original = original_surface(tree1, &surface_node)?;
        if Some(original) != cache.surface_handle() {
            return Err(SolidError::invalid_input(
                OP_SP,
                "surface cache does not own the tree's surface",
            ));
        }
        let mut uv_domain = patch.aux.uv_domain;

        // skip nodes whose domains are not in the Solver domain limit
        if uv_domain.are_disjoint(&self.uv_domain, 0.0) {
            return Ok(false);
        }

        // copy patch decomposition continuities, set problem boundaries to
        // DISCONTINUOUS
        let mut u_conts = patch.u_curve_conts;
        let mut v_conts = patch.v_curve_conts;
        if self.uv_domain.min().x >= uv_domain.min().x {
            u_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().x <= uv_domain.max().x {
            u_conts[1] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.min().y >= uv_domain.min().y {
            v_conts[0] = ContinuityType::Discontinuous;
        }
        if self.uv_domain.max().y <= uv_domain.max().y {
            v_conts[1] = ContinuityType::Discontinuous;
        }

        // Do interval tests and reject if disjoint or adjust if sUVDomain is not
        // completely inside of m_crUVDomain
        if !uv_domain.is_contained_by(&self.uv_domain, EFF_ZERO) {
            // Need to adjust continuities (the status is ignored)
            if let Some(d) = ignored(uv_domain.intersect(&self.uv_domain))? {
                uv_domain = d;
            }
            // Check for zero in either domain bound
            if are_same(uv_domain.max().x, uv_domain.min().x)
                || are_same(uv_domain.max().y, uv_domain.min().y)
            {
                return Ok(false);
            }
        }

        let op = self.base.operation;
        let tol = self.base.tolerance_3d;

        // skip solving for points too far away to generate a solution
        if op == SolverOperation::Minimize || op == SolverOperation::Normalize {
            let mut dist = patch.pseudo_box.distance_to_point(test_pnt) - tol;
            if dist < 0.0 {
                dist = 0.0;
            }
            if dist * dist > self.base.best_answer_so_far_sq {
                return Ok(false);
            }
        }

        // skip solving for points too far away for an intersection
        if op == SolverOperation::Intersect && !patch.pseudo_box.contains_point(test_pnt, tol) {
            return Ok(false);
        }

        // assume that we will be solving for SurfacePoints
        let mut solve_for_surface = true;

        // skip solving for SurfacePoints that can't project to surface along surface
        // normals (we can still solve for surfaceBoundary CurvePoints)
        if (op == SolverOperation::Minimize
            || op == SolverOperation::Maximize
            || op == SolverOperation::Normalize)
            && !patch.pseudo_box.contains_point(test_pnt, tol)
        {
            // build a polar box of TestPoint/PseudoBox Vector directions
            let corners = patch.pseudo_box.corners();
            let mut tmp_polar = PolarBox::default();
            for corner in corners {
                if tmp_polar.state() == PolarBoxState::Unbounded {
                    break;
                }
                tmp_polar.add_vector(corner - test_pnt, None);
            }

            // when SurfacePatch/Temp PolarBoxes are disjoint
            if tmp_polar.state() != PolarBoxState::Unbounded
                && tmp_polar.are_disjoint(&cache.polar_box_unstored(k, surface_id)?)
            {
                // no solution possible based on polar box check
                solve_for_surface = false;
            }
        }

        // solve
        let solutions = sp_local_solve::<O>(
            k,
            &patch,
            cache.surface(),
            &uv_domain,
            u_conts,
            v_conts,
            test_pnt,
            tol,
            op,
            solve_for_surface,
            cache.process_boundary_curves(),
        )?;

        // For every solution, set some solution parameters, and then check whether to
        // keep the point, based on whether it's within the trimmed surface boundaries
        // (i.e., the surface's Face boundaries), if the surface is trimmed.
        let owner0 = self.base.tree(0)?.owner_object()?;
        for found in solutions.as_slice() {
            let mut solution = *found;
            let uv_found = Point2::new(solution.start.param(0)?, solution.start.param(1)?);

            // set solution parameters
            solution.nodes[0] = branch.first().copied().flatten();
            solution.nodes[1] = branch.get(1).copied().flatten();
            solution.num_variables = 2;
            solution.num_objects = 2;
            solution.objects[0] = owner0;
            solution.objects[1] = Some(original);

            // Keep this solution if: Solution Point is within surface->Owner
            // trimBoundaries, or: surface has no owner trimBoundaries, or: Point
            // testing is off.
            if is_point_valid(
                k,
                self.base.tree(1)?.owner(),
                cache,
                uv_found,
                None,
                self.face_queries,
            )? {
                let key = if op == SolverOperation::Minimize || op == SolverOperation::Maximize {
                    SortKey::ByValueWithParameter
                } else {
                    SortKey::ByFirstTwoParameters
                };
                // (the status is not checked upstream)
                ignore_status(self.add_sorted_solution(&solution, key, true))?;
            }
        }
        Ok(false)
    }
}

// ---- InvertPointNearSingularity ----------------------------------------------------------------------

/// The result of [`Surface::invert_point_near_singularity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SingularityInversion {
    /// `rUVGuess`: the parameters of the closest point.
    pub uv: Point2,
    /// `reReason`: always `FoundAnswerConverged` on success.
    pub reason: TerminationReason,
}

impl Surface {
    /// The parameters of the closest point of the surface to `test_pt` near a
    /// singularity (`InvertPointNearSingularity`): Newton steps do not work there, so
    /// the two directions are solved separately. The guess itself, or the pole next
    /// to it, may already be the answer; else the degenerate parameter is found where
    /// the surviving tangent points to the test point
    /// ([`Surface::find_degen_param_for_direction`]), and the other parameter by
    /// dropping the point to the iso curve there.
    ///
    /// # Errors
    ///
    /// The status of a failing degenerate parameter search (`SM_ERR` when it does not
    /// converge) or iso curve creation or curve drop; [`SolidError::Unported`] from
    /// the evaluations and drops.
    pub fn invert_point_near_singularity(
        &self,
        k: &Kernel,
        test_pt: Point3,
        uv_domain: &Extent2d,
        uv_guess: Point2,
        tol: f64,
    ) -> Result<SingularityInversion> {
        let reason = TerminationReason::FoundAnswerConverged;
        let undef3 = (Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF);

        let (srf_pt, su, sv) =
            ignored(self.evaluate_1st_derivatives(k, uv_guess, false, false))?.unwrap_or(undef3);
        if srf_pt.closer_than(tol, test_pt) {
            return Ok(SingularityInversion {
                uv: uv_guess,
                reason,
            });
        }

        let sing_dir;
        let mut degen_param; // the one that doesn't move the point
        let mut good_param; // the one leaving the singularity
        let good_domain;
        let mut sing_uv = Point2::UNDEF;

        if su.length_squared() < sv.length_squared() {
            sing_dir = SurfParamType::U;
            degen_param = uv_guess.x;
            good_param = uv_guess.y;
            good_domain = uv_domain.v_interval();
            sing_uv.x = degen_param;
            sing_uv.y = if good_param < good_domain.mid() {
                good_domain.min()
            } else {
                good_domain.max()
            };
        } else {
            sing_dir = SurfParamType::V;
            degen_param = uv_guess.y;
            good_param = uv_guess.x;
            good_domain = uv_domain.u_interval();
            sing_uv.x = if good_param < good_domain.mid() {
                good_domain.min()
            } else {
                good_domain.max()
            };
            sing_uv.y = degen_param;
        }

        // test crTestPt/SurfaceNormal(sSingUV) dist against dTol (status ignored)
        let on_normal = ignored(is_point_on_normal(
            k,
            self,
            Some(uv_domain),
            sing_uv,
            test_pt,
            None,
            tol,
        ))?;

        // Don't trust this routine's bIsSolution alone: near a singularity, it might
        // return true when it's not (because of dotting with a near-zero deriv).
        // Call it good only if the distance is also small.
        if let Some(n) = on_normal {
            if n.good_point && (n.drop_to_surf < tol || n.drop_to_surf <= EFF_ZERO) {
                return Ok(SingularityInversion {
                    uv: sing_uv,
                    reason,
                });
            }
        }

        // Ok, neither the guess uv nor the pole is a solution. Find the degenerate
        // param, at the pole, where the non-degenerate tangent points towards the
        // test point.
        let pole_pt = point_or_undef(k, self, sing_uv)?;
        let diff_vec = test_pt - pole_pt; // we know this isn't zero.
        let degen =
            self.find_degen_param_for_direction(k, sing_uv, diff_vec, true, sing_dir, tol)?;
        if !degen.converged {
            // Probably out of bounds (SM_TR_OUT_OF_BOUNDS / SM_TR_UNABLE_TO_CONVERGE).
            return Err(SolidError::sm_err(OP_INVERT));
        }
        sing_uv = degen.uv;

        // Now find the non-degenerate param: drop to iso-curve.
        degen_param = if sing_dir == SurfParamType::U {
            sing_uv.x
        } else {
            sing_uv.y
        };
        // (CreateIsoParametricCurve with the tolerance dTol, status checked)
        let mut iso_curve = self
            .create_iso_parametric_curve(k, sing_dir, degen_param, tol, None, false)?
            .curve;
        enable_out_of_bounds(&mut iso_curve);

        // Get a guess param. First re-evaluate at the param that's going in the right
        // direction.
        let (_, su, sv) =
            ignored(self.evaluate_1st_derivatives(k, sing_uv, false, false))?.unwrap_or(undef3);
        let good_deriv = if sing_dir == SurfParamType::U { sv } else { su };
        let mut guess_t = diff_vec.dot(good_deriv) / good_deriv.dot(good_deriv);
        guess_t = good_domain.clamp_value(guess_t);

        // same-point tol for close solutions
        let dropped = iso_curve.drop_point(
            k,
            &good_domain,
            test_pt,
            None,
            tol,
            Some(guess_t),
            SolverOperation::Minimize,
        )?;
        // DropPoint always writes rdDroppedParameter (initialised to 0.0), used as is
        good_param = dropped.parameter;

        let uv = if sing_dir == SurfParamType::U {
            Point2::new(degen_param, good_param)
        } else {
            Point2::new(good_param, degen_param)
        };
        Ok(SingularityInversion { uv, reason })
    }
}

/// `SM_CURVE_ENABLE_OUTOFBOUNDS` for the B-spline part of `curve` (kept for the rest
/// of the scope upstream; this copy of the curve is local).
fn enable_out_of_bounds(curve: &mut Curve) {
    match curve.kind_mut() {
        CurveKind::BSpline(c) => c.set_out_of_bounds_enabled(true),
        CurveKind::Line(c) => c.base_mut().set_out_of_bounds_enabled(true),
        CurveKind::Ellipse(c) => c.base_mut().set_out_of_bounds_enabled(true),
        CurveKind::Hyperbola(c) => c.base_mut().set_out_of_bounds_enabled(true),
        CurveKind::Parabola(c) => c.base_mut().set_out_of_bounds_enabled(true),
        _ => {}
    }
}

// ---- LocalLineIntersect ------------------------------------------------------------------------------

/// The outputs of [`Surface::local_line_intersect`] (`SmSurface::LocalLineIntersect`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineIntersection {
    /// `rbFound`.
    pub found: bool,
    /// `rUVFound`: the surface parameters of the intersection.
    pub uv: Point2,
    /// `rdTFound`: the line parameter.
    pub t: f64,
    /// `rdDeviation`: the distance between the final line and surface points.
    pub deviation: f64,
    /// `rPoint`: the intersection point.
    pub point: Point3,
    /// `rNormal`: the surface normal there (not unitized).
    pub normal: Vec3,
}

impl Surface {
    /// Local intersection of the line `line_point + t * line_vector` with the surface
    /// from the guess `uv_guess` (`LocalLineIntersect`): the line is projected to the
    /// tangent plane and the UV shifts taken from that (quadratic convergence in the
    /// general case, at most 50 steps, three boundary hits stop it). When that does
    /// not converge the general curve/surface local intersector runs, from the last UV
    /// and then from the guess. A result off the boundary is a boundary solution that
    /// may not be precise.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a zero line vector; those of the evaluations and of
    /// [`Surface::local_curve_intersect`].
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn local_line_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Point3,
        interval: &Extent1d,
        distance_tolerance: f64,
        accuracy_tolerance: f64,
        uv_guess: Point2,
    ) -> Result<LineIntersection> {
        let mut out = LineIntersection {
            found: false,
            uv: Point2::UNDEF,
            t: UNDEF_DOUBLE,
            deviation: UNDEF_DOUBLE,
            point: Vec3::UNDEF,
            normal: Vec3::UNDEF,
        };

        // Error if zero line vector.
        if line_vector.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_LOCAL_LINE));
        }

        let mut uv = uv_domain.clamp_point(uv_guess);
        let accuracy_squared = accuracy_tolerance * accuracy_tolerance;

        // Evaluate surface at starting guess (nonZeroTangent values, upper half only).
        let flags = EvaluateFlags {
            only_upper_half: true,
            ..Default::default()
        };
        let pvv = |s: &Self, uv: Point2| -> Result<(Vec3, Vec3, Vec3)> {
            let m = s.evaluate(k, uv, 1, 1, flags)?;
            let at = |i: usize, j: usize| {
                m.get(i)
                    .and_then(|r| r.get(j))
                    .copied()
                    .ok_or(SolidError::sm_err(OP_LOCAL_LINE))
            };
            Ok((at(0, 0)?, at(1, 0)?, at(0, 1)?))
        };
        let (mut srf_pnt, mut du, mut dv) = pvv(self, uv)?;

        // Get corresponding starting guess on line.
        let mut t = line_closest_point(line_point, line_vector, srf_pnt);
        t = interval.clamp_value(t);
        let orig_t = t;

        // Local progress variables.
        let mut num_boundary_hits = 0;
        let mut last_dist = BIG_DOUBLE;
        let uv_tol =
            EFF_ZERO * (1.0 + uv_domain.max().max_dimension() + uv_domain.min().max_dimension());

        let max_iters = 50;
        for i in 0..max_iters {
            // State at top of loop:
            // - Surface has been evaluated at latest uv: srf_pnt, du, dv.
            // - t is set to the current line-parameter guess.
            let line_pnt = line_point + t * line_vector;
            let dist_sq = srf_pnt.distance_squared(line_pnt);
            if dist_sq < accuracy_squared {
                out.found = true;
                out.uv = uv;
                out.t = t;
                out.deviation = crate::geom::vector3d::sqrt(dist_sq);
                out.point = srf_pnt;

                let mut normal = du.cross(dv);
                if normal.length_squared() < EFF_ZERO_SQ {
                    normal = self.evaluate_normal(k, uv, Side::Left, Side::Left)?;
                }
                out.normal = normal;
                return Ok(out);
            }

            // Make sure we are converging - if not just break and use general
            // algorithm.
            if dist_sq >= last_dist && i > 4 {
                break;
            }
            last_dist = dist_sq;

            // This version of LinePlaneIntersect() returns the uv shift explicitly.
            let Ok((new_t, shift_u, shift_v)) =
                line_plane_intersect_uv(line_point, line_vector, srf_pnt, du, dv)
            else {
                break;
            };
            t = new_t;

            uv += Point2::new(shift_u, shift_v);
            if uv_domain.contains_point(uv, uv_tol) {
                num_boundary_hits = 0;
            } else {
                num_boundary_hits += 1;
                if num_boundary_hits >= 3 {
                    return Ok(out);
                }
                uv = uv_domain.clamp_point(uv);
            }

            (srf_pnt, du, dv) = pvv(self, uv)?;
        }

        // If we make it to here then we did not converge. Try a more sophisticated
        // technique that involves curve solving.
        let tmp_line = Curve::from(Line::infinite(line_point, line_vector, 3, false));

        // First try it from the uv value that our iteration ended up at.
        let mut found = self.local_curve_intersect(
            k,
            uv_domain,
            &tmp_line,
            interval,
            distance_tolerance,
            uv,
            t,
        )?;
        if found.is_none() {
            // Try again, from the original uv guess.
            found = self.local_curve_intersect(
                k,
                uv_domain,
                &tmp_line,
                interval,
                distance_tolerance,
                uv_guess,
                orig_t,
            )?;
        }
        if let Some(f) = found {
            out.found = true;
            out.uv = f.uv;
            out.t = f.t;
            out.deviation = f.deviation;
            out.point = self.evaluate_point(k, f.uv)?;
            out.normal = self.evaluate_normal(k, f.uv, Side::Left, Side::Left)?;
        }
        Ok(out)
    }
}

// ---- sm_ExtractIsoCurve / sm_IntersectSrfBndryCrv ------------------------------------------------------

/// Lifts a straight UV curve that runs along an iso line of `surface` to the iso curve
/// (`sm_ExtractIsoCurve`): `None` when `uv_curve` is not a linear curve with two
/// control points along a constant `u` or `v` (a degenerate or diagonal curve), when
/// the iso curve cannot be made or has an end knot of too high multiplicity. The iso
/// curve is trimmed to the UV curve's extent, reversed when the UV curve runs
/// backwards and reparameterised to `interval`; the maximum approximation deviation
/// is always `0`. A degenerate iso curve is returned as is.
///
/// # Errors
///
/// [`SolidError::Unported`] from the iso curve creation (surface kinds without it) and
/// the evaluations; other failures return `None` as upstream.
pub fn extract_iso_curve(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    uv_curve: &BSplineCurve,
    interval: &Extent1d,
    this_approx_tol_3d: f64,
) -> Result<Option<Curve>> {
    // Check to see if the UV curve corresponds to an ISO curve on the surface. If it
    // does then extract it from the surface.
    if uv_curve.degree()? > 1 || uv_curve.number_control_points()? > 2 {
        return Ok(None);
    }

    // get UVCurve endPoints clamped to crUVDomain
    let ends = uv_curve.ends()?;
    let (mut start_pnt, end_pnt) = (ends.start, ends.end);
    let uv_start = uv_domain.clamp_point(Point2::new(start_pnt.x, start_pnt.y));
    let uv_end = uv_domain.clamp_point(Point2::new(end_pnt.x, end_pnt.y));

    // locals
    let x_tol = EFF_ZERO * (1.0 + uv_domain.size().x);
    let y_tol = EFF_ZERO * (1.0 + uv_domain.size().y);
    let reverse;
    let trim_ivl;
    let iso = if (start_pnt.x - end_pnt.x).abs() < x_tol && (start_pnt.y - end_pnt.y).abs() > y_tol
    {
        // when UVCurve is a constant U, varying V isoCurve: make sure UVEndPoints are
        // clamped in X direction
        if start_pnt.x < uv_domain.min().x {
            start_pnt.x = uv_domain.min().x;
        } else if start_pnt.x > uv_domain.max().x {
            start_pnt.x = uv_domain.max().x;
        }
        // get curve trim interval, remember if curve is running reversed
        if start_pnt.y > end_pnt.y {
            trim_ivl = Extent1d::from_min_max(uv_end.y, uv_start.y);
            reverse = true;
        } else {
            trim_ivl = Extent1d::from_min_max(uv_start.y, uv_end.y);
            reverse = false;
        }
        // set up UVSpace min/max points and UVDomain
        let mut min = uv_domain.min();
        let mut max = uv_domain.max();
        min.y = trim_ivl.min();
        max.y = trim_ivl.max();
        let trim_domain = Extent2d::from_points(min, max);

        // Create constant U isoParameterCurve in the trim domain to ApproxTol/3.0
        let Some(iso) = ignored(surface.create_iso_parametric_curve(
            k,
            SurfParamType::U,
            start_pnt.x,
            this_approx_tol_3d / 3.0,
            Some(&trim_domain),
            false,
        ))?
        else {
            return Ok(None);
        };
        iso.curve
    } else if (start_pnt.y - end_pnt.y).abs() < y_tol && (start_pnt.x - end_pnt.x).abs() > x_tol {
        // else if UVCurve varies in U direction: make sure curve is clamped in V
        if start_pnt.y < uv_domain.min().y {
            start_pnt.y = uv_domain.min().y;
        } else if start_pnt.y > uv_domain.max().y {
            start_pnt.y = uv_domain.max().y;
        }
        // get UVTrimCurve, remember when curve runs in negative direction
        if start_pnt.x > end_pnt.x {
            trim_ivl = Extent1d::from_min_max(uv_end.x, uv_start.x);
            reverse = true;
        } else {
            trim_ivl = Extent1d::from_min_max(uv_start.x, uv_end.x);
            reverse = false;
        }

        // set up UVSpace min/max points and UVDomain
        let mut min = uv_domain.min();
        let mut max = uv_domain.max();
        min.x = trim_ivl.min();
        max.x = trim_ivl.max();
        let trim_domain = Extent2d::from_points(min, max);

        // Create constant V isoParameterCurve in the trim domain to ApproxTol/3.0
        let Some(iso) = ignored(surface.create_iso_parametric_curve(
            k,
            SurfParamType::V,
            start_pnt.y,
            this_approx_tol_3d / 3.0,
            Some(&trim_domain),
            false,
        ))?
        else {
            return Ok(None);
        };
        iso.curve
    } else {
        // UVCurve is degenerate
        return Ok(None);
    };
    let mut iso = iso;

    // signal failures and catch degenerate 3dCurves
    if iso.is_degenerate(k, EFF_ZERO, None)? {
        return Ok(Some(iso));
    }

    // Check starting and ending knots for too high multiplicity.
    let Some((_, multiplicities)) = ignored(iso.knots(k, None))? else {
        return Ok(None);
    };
    let degree = iso.degree()?;
    if multiplicities.first().is_some_and(|m| *m > degree + 1)
        || multiplicities.last().is_some_and(|m| *m > degree + 1)
    {
        return Ok(None);
    }

    // Trim ISO curve Trim it to the length of the input UVTrimCurve line (may snap the
    // interval by tol to existing knots).
    let mut trim = trim_ivl;
    ignore_status(iso.trim(k, &mut trim, TrimMode::default()))?;

    // when needed - reverse isoParamCurve parameterization to equal input UVTrimcCurve
    // direction
    if reverse {
        let natural = iso.natural_interval();
        ignore_status(iso.reverse_parameterization(k, &natural))?;
    }

    // Set IsoCurve parameterization to equal input interval
    ignore_status(iso.edit_parameterization(k, interval))?;
    Ok(Some(iso))
}

/// The outputs of [`intersect_srf_bndry_crv`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryIntersection {
    /// `rbFoundSolution`.
    pub found: bool,
    /// `rUVFound`.
    pub uv: Point2,
    /// `rdTFound`.
    pub t: f64,
    /// `rdDeviation`.
    pub deviation: f64,
    /// `rbTangency`: the iso curve tangent is perpendicular to the surface normal at
    /// the solution (the surfaces may be tangent, or the curve coincident or tangent
    /// to the surface).
    pub tangency: bool,
}

/// Intersects `surface` with the iso-parameter curve of `curve_surface` at
/// `iso_parameter` in direction `param` near the guess (`sm_IntersectSrfBndryCrv`).
/// A degenerate iso curve, or one lying in the surface at the guess (its tangent
/// perpendicular to the surface normal within 0.05 degrees), has no solution.
///
/// # Errors
///
/// Those of the iso curve and surface evaluations and of
/// [`Surface::local_curve_intersect`]; [`SolidError::Unported`] for a curve surface
/// that is not a B-spline kind.
#[allow(clippy::too_many_arguments)]
pub fn intersect_srf_bndry_crv(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve_surface: &Surface,
    param: SurfParamType,
    iso_parameter: f64,
    tol_3d: f64,
    guess_uv: Point2,
    guess_t: f64,
) -> Result<BoundaryIntersection> {
    let mut out = BoundaryIntersection {
        found: false,
        uv: Point2::UNDEF,
        t: UNDEF_DOUBLE,
        deviation: UNDEF_DOUBLE,
        tangency: false,
    };

    // specify CurveSurface 3DIsoParameterCurve
    let iso = super::p6::boundary_iso_curve(k, curve_surface, param, iso_parameter, true)?;

    // skip degenerate curves
    if iso.is_degenerate(k, EFF_ZERO, None)? {
        return Ok(out);
    }

    // If the curve is in the surface, don't even try: the solution can slip all
    // around. When the boundary curve is in the surface, then the regular intersector
    // will work just fine, so just act like nothing happened.
    let pv = iso.evaluate(k, guess_t, 1, true, true)?;
    let tangent = pv.get(1).copied().ok_or(SolidError::sm_err(OP_BNDRY))?;
    let srf_norm = surface.evaluate_normal(k, guess_uv, Side::Right, Side::Right)?;
    if srf_norm.is_perpendicular_to(tangent, 0.05) {
        return Ok(out); // no solutions
    }

    // intersect the surface with specified isoParameterCurve
    let found = surface.local_curve_intersect(
        k,
        uv_domain,
        &iso,
        &iso.natural_interval(),
        tol_3d,
        guess_uv,
        guess_t,
    )?;

    // when a solution was found
    if let Some(f) = found {
        out.found = true;
        out.uv = f.uv;
        out.t = f.t;
        out.deviation = f.deviation;
        let pv = iso.evaluate(k, f.t, 1, true, true)?;
        let tangent = pv.get(1).copied().ok_or(SolidError::sm_err(OP_BNDRY))?;
        let normal = surface.evaluate_normal(k, f.uv, Side::Left, Side::Left)?;

        // when the angle curveTangent/SurfaceNormal angle is perpendicular to +/-5 deg
        let angle = tangent.angle_between(normal)?;
        if (angle - (PI / 2.0)).abs() < 0.01 {
            // ThisSurface/OtherSurface are tangent at this point
            out.tangency = true;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector3d::Vec3;
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn bilinear() -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(2.0, 3.0, 1.0),
            )
            .unwrap(),
        )
    }

    #[test]
    fn same_surface_is_coincident() {
        let k = Kernel::default();
        let s = bilinear();
        let c = s.base_coincidence_check(&k, &s, 1e-6, true).unwrap();
        assert!(c.are_coincident);
        assert_eq!(c.max_distance, 0.0);
    }

    #[test]
    fn disjoint_boxes_are_not_coincident() {
        let k = Kernel::default();
        let a = bilinear();
        let b = Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(10.0, 10.0, 10.0),
                Vec3::new(12.0, 10.0, 10.0),
                Vec3::new(10.0, 13.0, 10.0),
                Vec3::new(12.0, 13.0, 11.0),
            )
            .unwrap(),
        );
        let c = a.base_coincidence_check(&k, &b, 1e-6, true).unwrap();
        assert!(!c.are_coincident);
        assert_eq!(c.max_distance, BIG_DOUBLE);
    }

    #[test]
    fn bilinear_surface_has_no_interior_knots() {
        let k = Kernel::default();
        let s = bilinear();
        let (u, v) = s
            .find_knots_by_max_continuity(&k, ContinuityType::G1, false)
            .unwrap();
        assert!(u.is_empty() && v.is_empty());
    }

    #[test]
    fn zero_line_vector_is_rejected() {
        let k = Kernel::default();
        let s = bilinear();
        let r = s.base_global_line_intersect::<NoObject>(
            &k,
            NoObject,
            &s.natural_uv_domain_if_built(),
            Vec3::ZERO,
            Vec3::ZERO,
            None,
            false,
            1e-6,
        );
        assert!(r.is_err());
        assert!(s
            .local_line_intersect(
                &k,
                &s.natural_uv_domain_if_built(),
                Vec3::ZERO,
                Vec3::ZERO,
                &Extent1d::from_min_max(0.0, 1.0),
                1e-6,
                1e-9,
                Point2::new(0.5, 0.5),
            )
            .is_err());
    }

    struct RejectCachePreparation {
        calls: usize,
    }

    impl SurfaceSolveContext<NoObject> for RejectCachePreparation {
        fn build_surface_cache(
            &mut self,
            _k: &Kernel,
            _surface: &Surface,
            _handle: Option<NoObject>,
        ) -> Result<SurfaceCache<NoObject>> {
            self.calls += 1;
            Err(SolidError::invalid_input(
                OP_LINE,
                "unexpected eager cache preparation",
            ))
        }

        fn face_queries(
            &self,
        ) -> Option<&dyn crate::surface::surface_cache::face_context::FaceQueries<NoObject>>
        {
            None
        }
    }

    #[test]
    fn caller_context_does_not_prepare_an_analytic_fast_path() {
        let k = Kernel::default();
        let mut plane = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        // Real ownership with no available trims: analytic work must not request
        // mutable topology merely because an owner is present.
        plane.set_owner(Some(crate::geom::object::ObjectRef {
            type_code: crate::topology::face::p2::TYPE,
            key: crate::geom::attribute::ObjectKey(50_000),
        }));
        plane.set_owner_zone_tol_3d(Some(1e-5));
        let mut context = RejectCachePreparation { calls: 0 };
        let domain = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        let hit = plane
            .global_line_intersect_with_context(
                &k,
                Some(NoObject),
                None,
                &mut context,
                &domain,
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::Z,
                None,
                false,
                1e-6,
            )
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(context.calls, 0);

        let curve = Curve::from(
            BSplineCurve::create_line_segment(3, Vec3::new(0.0, 0.0, -1.0), Vec3::Z).unwrap(),
        );
        let hit = plane
            .global_curve_intersect_with_context(
                &k,
                [Some(NoObject), Some(NoObject)],
                None,
                &mut context,
                &domain,
                &curve,
                &curve.natural_interval(),
                1e-6,
            )
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(context.calls, 0);
    }

    #[test]
    fn invalid_line_is_rejected_before_context_preparation() {
        let k = Kernel::default();
        let surface = bilinear();
        let mut context = RejectCachePreparation { calls: 0 };
        assert!(surface
            .global_line_intersect_with_context(
                &k,
                Some(NoObject),
                None,
                &mut context,
                &surface.natural_uv_domain_if_built(),
                Vec3::ZERO,
                Vec3::ZERO,
                None,
                false,
                1e-6,
            )
            .is_err());
        assert_eq!(context.calls, 0);
    }

    #[test]
    fn degenerate_curve_propagates_malformed_cache_context() {
        let k = Kernel::default();
        let mut plane = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::Z,
        ));
        plane.set_owner(Some(crate::geom::object::ObjectRef {
            type_code: crate::topology::face::p2::TYPE,
            key: crate::geom::attribute::ObjectKey(50_002),
        }));
        plane.set_owner_zone_tol_3d(Some(1e-5));
        let curve = Curve::from(BSplineCurve::create_degenerate_curve(3, Vec3::ZERO).unwrap());
        let mut context = RejectCachePreparation { calls: 0 };
        let result = plane.global_curve_intersect_with_context(
            &k,
            [Some(NoObject), Some(NoObject)],
            None,
            &mut context,
            &Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0),
            &curve,
            &curve.natural_interval(),
            1e-6,
        );
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert_eq!(context.calls, 1);
    }

    #[derive(Debug)]
    struct NumericalTrimContext {
        builds: usize,
        tests: core::cell::Cell<usize>,
        inside: bool,
    }

    impl crate::surface::surface_cache::face_context::FaceQueries<NoObject> for NumericalTrimContext {
        fn face_zone_tol_3d(&self, _face: NoObject) -> Result<f64> {
            Ok(1e-5)
        }

        fn face_point_classify(
            &self,
            _k: &Kernel,
            _face: NoObject,
            _uv: Point2,
            _tol: f64,
        ) -> Result<(i32, Option<NoObject>)> {
            self.tests.set(self.tests.get() + 1);
            Ok(if self.inside {
                (102, Some(NoObject))
            } else {
                (100, None)
            })
        }

        fn face_point_classify_fast_drop(
            &self,
            k: &Kernel,
            face: NoObject,
            uv: Point2,
            tol: f64,
        ) -> Result<(i32, Option<NoObject>)> {
            self.face_point_classify(k, face, uv, tol)
        }
    }

    impl SurfaceSolveContext<NoObject> for NumericalTrimContext {
        fn build_surface_cache(
            &mut self,
            k: &Kernel,
            surface: &Surface,
            handle: Option<NoObject>,
        ) -> Result<SurfaceCache<NoObject>> {
            use crate::surface::surface_cache::p1::FaceContainment;
            use crate::surface::surface_cache::SurfaceCacheOptions;
            use crate::surface::trim_srf_cache::TrimSrfCache;
            self.builds += 1;
            let mut cache = TrimSrfCache::new(
                surface.clone_for_evaluation()?,
                SurfaceCacheOptions::default(),
            );
            cache.base_mut().set_surface_handle(handle);
            cache.bind_face(surface.face()?.unwrap(), NoObject, 1e-5)?;
            // This focused adapter isolates solver acceptance. Real topology and
            // containment are independently compared in native_face fixtures.
            cache.base_mut().build_tree(k, false)?;
            cache.base_mut().have_t_surface_cache = true;
            cache.base_mut().face_containment_done = FaceContainment::Done;
            Ok(cache.into_base())
        }

        fn face_queries(
            &self,
        ) -> Option<&dyn crate::surface::surface_cache::face_context::FaceQueries<NoObject>>
        {
            Some(self)
        }
    }

    #[test]
    fn numerical_trim_queries_survive_caller_and_held_cache_reuse() {
        let k = Kernel::default();
        let mut surface = bilinear();
        surface.set_owner(Some(crate::geom::object::ObjectRef {
            type_code: crate::topology::face::p2::TYPE,
            key: crate::geom::attribute::ObjectKey(50_001),
        }));
        surface.set_owner_zone_tol_3d(Some(1e-5));
        let domain = surface.natural_uv_domain_if_built();
        let mut context = NumericalTrimContext {
            builds: 0,
            tests: core::cell::Cell::new(0),
            inside: true,
        };
        let mut held = None;
        let hit = surface
            .global_line_intersect_held_with_context(
                &k,
                NoObject,
                &mut held,
                &mut context,
                &domain,
                Vec3::new(1.0, 1.5, -2.0),
                Vec3::Z,
                None,
                false,
                1e-6,
            )
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(context.builds, 1);
        assert!(context.tests.get() > 0);
        context.inside = false;
        let tested = context.tests.get();
        let hit = surface
            .global_line_intersect_held_with_context(
                &k,
                NoObject,
                &mut held,
                &mut context,
                &domain,
                Vec3::new(1.0, 1.5, -2.0),
                Vec3::Z,
                None,
                false,
                1e-6,
            )
            .unwrap();
        assert!(hit.is_empty());
        assert_eq!(context.builds, 1);
        assert!(context.tests.get() > tested);
        let cache = held.as_mut().unwrap().as_mut().unwrap();
        context.inside = true;
        let hit = surface
            .global_line_intersect_with_context(
                &k,
                Some(NoObject),
                Some(cache),
                &mut context,
                &domain,
                Vec3::new(1.0, 1.5, -2.0),
                Vec3::Z,
                None,
                false,
                1e-6,
            )
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(context.builds, 1);
    }

    #[test]
    fn local_line_intersect_hits_a_plane_like_patch() {
        let k = &Kernel::default();
        let s = Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(2.0, 3.0, 0.0),
            )
            .unwrap(),
        );
        let hit = s
            .local_line_intersect(
                k,
                &s.natural_uv_domain_if_built(),
                Vec3::new(1.0, 1.5, -2.0),
                Vec3::new(0.0, 0.0, 1.0),
                &Extent1d::from_min_max(-10.0, 10.0),
                1e-6,
                1e-9,
                Point2::new(0.4, 0.4),
            )
            .unwrap();
        assert!(hit.found);
        assert!((hit.t - 2.0).abs() < 1e-9);
        assert!((hit.uv.x - 0.5).abs() < 1e-9 && (hit.uv.y - 0.5).abs() < 1e-9);
    }

    #[test]
    /// A "surface" that is a line segment (rows of equal control points): the
    /// `Evaluate` failures of the overlap check are ignored, as upstream does.
    fn degenerate_line_surface_coincidence_check_does_not_fail() {
        use crate::nurbs::math::{CPoint, NOW};
        use crate::nurbs::srf_basic::Surface as NlSurface;
        let rows: [&[[f64; 3]]; 2] = [
            &[
                [-44.681590021410166, 67.02238503211525, 18.146048194526948],
                [-44.681590021410166, 67.02238503211525, 18.146048194526948],
                [-44.681590021410166, 67.02238503211525, 18.146048194526948],
                [-44.681590021410166, 67.02238503211525, 18.146048194526948],
            ],
            &[
                [89.36318004282033, 67.02238503211525, 0.0],
                [89.36318004282033, 67.02238503211525, 0.0],
                [89.36318004282033, 67.02238503211525, 0.0],
                [89.36318004282033, 67.02238503211525, 0.0],
            ],
        ];
        let net = rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|c| CPoint::new(c[0], c[1], c[2], NOW))
                    .collect()
            })
            .collect();
        let nl = NlSurface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.2902846541158647, 0.5446927402597225, 1.0, 1.0],
        )
        .unwrap();
        let s = Surface::from(BSplineSurface::from_nurb(nl, false));
        let k = Kernel::default();
        let r = s.base_coincidence_check(&k, &s.clone(), 1e-6, false);
        // (the point solve behind the drop is unported: surface.surface_point_solve_p3)
        assert!(
            r.is_ok() || matches!(&r, Err(e) if e.is_unported()),
            "{r:?}"
        );
    }

    /// Opaque solution handles, as a B-rep's: they cannot reach their geometry.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Opaque {
        Curve,
        Surface,
    }

    impl SolutionObject for Opaque {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            match self {
                Self::Curve => crate::geom::solution_array::ObjectKind::Curve,
                Self::Surface => crate::geom::solution_array::ObjectKind::Surface,
            }
        }
    }

    #[test]
    fn tree_intersection_compares_solutions_of_opaque_handles() {
        // A cubic dipping through the bilinear patch crosses it twice: the tree solve
        // compares the two solutions (IdenticalSolutions), whose parameter domains come
        // from the solve's curve and surface, not from the handles.
        let k = Kernel::default();
        let s = bilinear();
        let pts =
            [(0.5, 1.0), (0.8, -1.0), (1.2, -1.0), (1.5, 1.0)].map(|(x, z)| Vec3::new(x, 1.5, z));
        let c = Curve::from(crate::primitive::api_curves::create_curve(&pts).unwrap());
        let sols = s
            .global_curve_intersect(
                &k,
                [Opaque::Curve, Opaque::Surface],
                &s.natural_uv_domain_if_built(),
                &c,
                &c.natural_interval(),
                1e-6,
            )
            .unwrap();
        assert_eq!(sols.as_slice().len(), 2);
        for sol in sols.as_slice() {
            let t = sol.start.param(0).unwrap();
            let uv = Point2::new(sol.start.param(1).unwrap(), sol.start.param(2).unwrap());
            let on_curve = c.evaluate_point(&k, t).unwrap();
            let on_surface = s.evaluate_point(&k, uv).unwrap();
            assert!(
                on_curve.closer_than(1e-6, on_surface),
                "{on_curve:?} {on_surface:?}"
            );
        }
    }

    /// Two overlapping bilinear patches in the same plane, the second one shifted.
    fn overlapping_pair() -> (Surface, Surface) {
        let patch = |x: f64| {
            Surface::from(
                BSplineSurface::create_bilinear_surface(
                    Vec3::new(x, 0.0, 0.0),
                    Vec3::new(x + 2.0, 0.0, 0.0),
                    Vec3::new(x, 3.0, 0.0),
                    Vec3::new(x + 2.0, 3.0, 0.0),
                )
                .unwrap(),
            )
        };
        (patch(0.0), patch(1.0))
    }

    #[test]
    fn boundary_memo_hit_is_the_fresh_build() {
        let on = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        let off = Kernel {
            memoize_curve_caches: false,
            ..Kernel::default()
        };
        let (a, b) = overlapping_pair();
        let want = format!(
            "{:?}",
            a.base_coincidence_check(&off, &b, 1e-6, false).unwrap()
        );
        assert!(a.boundary_memo().0.get().is_none());
        let (mut a, b) = overlapping_pair();
        let first = format!(
            "{:?}",
            a.base_coincidence_check(&on, &b, 1e-6, false).unwrap()
        );
        assert!(a.boundary_memo().0.get().is_some());
        assert!(b.boundary_memo().0.get().is_some());
        let second = format!(
            "{:?}",
            a.base_coincidence_check(&on, &b, 1e-6, false).unwrap()
        );
        assert_eq!(first, want);
        assert_eq!(second, want);
        // a copy starts empty, an edit empties the memo
        assert!(a.clone().boundary_memo().0.get().is_none());
        let _ = a.kind_mut();
        assert!(a.boundary_memo().0.get().is_none());
        // other tolerances miss and build again, with the same result
        let mut loose = Kernel {
            memoize_curve_caches: true,
            ..Kernel::default()
        };
        loose.tolerances.zone_tol_3d *= 10.0;
        let (a2, b2) = overlapping_pair();
        let _ = a2.base_coincidence_check(&on, &b2, 1e-6, false).unwrap();
        let fresh = format!(
            "{:?}",
            overlapping_pair()
                .0
                .base_coincidence_check(&loose, &b2, 1e-6, false)
                .unwrap()
        );
        let missed = format!(
            "{:?}",
            a2.base_coincidence_check(&loose, &b2, 1e-6, false).unwrap()
        );
        assert_eq!(missed, fresh);
    }
}
