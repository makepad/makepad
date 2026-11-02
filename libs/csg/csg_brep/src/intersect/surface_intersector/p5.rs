//! `SmSurfaceIntersector`, part 5: the top level surface/surface intersection
//! ([`do_intersection`]) and its grazing-box shortcut
//! ([`SurfaceIntersector::grazing_box_process`]).
//!
//! Upstream: `source/SMLib/src/SmSurfaceIntersector.cpp`.
//!
//! | Rust                                                  | Upstream                      |
//! |-------------------------------------------------------|-------------------------------|
//! | [`do_intersection`], [`AdvSurfaceIntersector::do_intersection_with_caches`] | `DoIntersection` |
//! | [`intersection_surface_caches`], [`IntersectionSurface`] | the `smsurf_GetSurfaceCache` / `GetTree` calls at the top of `DoIntersection` |
//! | [`SurfaceIntersector::grazing_box_process`], [`GrazingBox`] | `GrazingBoxProcess`     |
//!
//! Deviations:
//! - **Dispatch.** `DoIntersection` is a base class method, but every upstream caller
//!   (`SmSurface::GlobalSurfaceIntersect`, `SmSurface::ParallelProjectCurve`) runs it
//!   on an `SmAdvSurfaceIntersector`, whose `SolveBranch` / `LocalSolve` overrides
//!   answer the `SM_SO_INTERSECTION_TEST` solve and whose `FindInteriorCurves`
//!   override finds the interior loops. It is therefore ported on
//!   [`AdvSurfaceIntersector`] (on a plain `SmSurfaceIntersector` the base
//!   `LocalSolve` stores node-only solutions whose box and domain values are never
//!   set, a case no caller reaches). `ComputePointValues` (part 2, called by
//!   `FindBoundaryStartPoints` and part 4's `TraceCurve`) reaches the advanced
//!   `ComputeSingularityPoint` override through the base's `advanced` flag, which
//!   [`AdvSurfaceIntersector::new`] sets.
//! - **Surface caches.** Rust surfaces keep no cache, so the caller hands the caches
//!   in ([`intersection_surface_caches`] builds them as `smsurf_GetSurfaceCache` /
//!   `GetTree` would; [`do_intersection`] does both). One
//!   cache per surface, one for a self intersection (upstream gets the same cache
//!   twice). The temporary flags (`m_bPointTestEnabled = FALSE`,
//!   `m_bProcessBoundaryCurves = TRUE`) are set before the first solve rather than
//!   after it (that `SM_SO_INTERSECTION_TEST` solve and `GrazingBoxProcess` read
//!   neither) and stay set on the caller's caches (upstream restores them on return; here
//!   the caches stay borrowed by the solver for `'t`, so they cannot be written
//!   back on exit; [`do_intersection`]'s caches are local, so it is unaffected).
//!   The boundary bounding planes are computed on copies of the caches' (each lazily
//!   computed plane is the same), and `FindBoundaryStartPoints` runs on copies of the
//!   caches, flags included.
//! - **Outputs.** The out-params become one returned [`SurfaceIntersection`] (a
//!   `NULL` UV curve is `None`); on an error nothing is returned. `GrazingBoxProcess`
//!   returns its curve instead of appending it to the out-params.
//! - **Statuses.** `TraceCurve`'s status, ignored upstream (no curve is flushed), is
//!   ignored except [`SolidError::Unported`], which always propagates; so is
//!   `EvaluatePoint`'s in the degenerate curve sweep (an unset point is
//!   [`Vec3::UNDEF`]). `IsDegenerate()` (default tolerance `SM_EFF_ZERO`) and
//!   `IsPointOnCurve` return `SmBoolean` upstream and cannot fail; their Rust ports
//!   return `Result`, and an error from them (unreachable today) propagates instead
//!   of reading as `FALSE`. Mismatched curve arrays in the degenerate
//!   curve sweep (an `SM_ASSERT`, then a read past the array upstream) are
//!   [`SolidError::InvalidInput`].
//! - **Bounded loops.** The start point loop pops one start point per pass; no pass
//!   adds start points (`TraceCurve` adds none), so it ends after at most as many
//!   passes as `FindBoundaryStartPoints` made points.
//! - **Opt-in fix (WR-76, #392).** With
//!   [`UpstreamFixes::ssi_trace_tangent_curves_once`](crate::kernel::UpstreamFixes::ssi_trace_tangent_curves_once)
//!   a traced curve removes, before it is flushed, the start points left in a tangent
//!   cluster with either of its ends (`SurfaceIntersector::remove_end_tangent_clusters`);
//!   the source fork's source fork@0ce09f2. Off by default (upstream
//!   behaviour).
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks (graphics, dumps, the start
//!   point reordering experiment), `SmCacheCheckOutIn` (the caches are owned by the
//!   caller), the unused context argument and the `SM_ASSERT`s.

use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::ExtentPointType;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::{GlobalSolve, SolutionRequested, SolveRequest, SolverOperation};
use crate::geom::solution_array::{SolutionArray, SolutionObject};
use crate::geom::tol::is_zero_to_tol;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::adv_surface_intersector::{AdvSurfaceIntersector, InteriorCurves};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::knots::Side;
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p2::{surface_cache, SurfaceIntersection, TsectCurveType};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::p1::{PatchBoundaryBoundingPlanes, SurfaceCache};

use super::p1::{SrfIndex, SurfaceIntersector, TsectPnt};
use super::p2::{BoundaryPlaneTestResult, TsectCurves};

const OP_DI: &str = "intersect::surface_intersector::p5::AdvSurfaceIntersector::do_intersection";
const OP_CACHES: &str = "intersect::surface_intersector::p5::intersection_surface_caches";

/// The point intersection [`SurfaceIntersector::grazing_box_process`] found: a
/// degenerate 3D curve, a degenerate UV curve on each surface (all degree 1, both
/// control points at the point), type touching and the distance between the two
/// surface points as deviation.
#[derive(Debug, Clone)]
pub struct GrazingBox {
    /// `p3DCurves` entry.
    pub curve_3d: Curve,
    /// `pSurface1UVCurves`, `pSurface2UVCurves` entries.
    pub uv_curves: [Curve; 2],
    /// `pCurveTypes` entry (always [`TsectCurveType::Touching`]).
    pub curve_type: TsectCurveType,
    /// `pDeviations` entry.
    pub deviation: f64,
}

/// `smsurf_GetSurfaceCache(m_cpSurface[i])` and its `GetTree()`, as `DoIntersection`
/// fetches them: the cache of the first surface and, unless the intersector
/// intersects a surface with itself, of the second, each with its decomposition tree
/// built. `handles` name the two surfaces in their caches (upstream's caches and
/// trees name their surface; the leaf solves of `GlobalCurveIntersect`, which runs on
/// these caches, read it).
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a surface without cache or tree (`NER`);
/// [`SolidError::Unported`] from the cache and tree builds.
pub fn intersection_surface_caches<O: Copy + PartialEq>(
    k: &Kernel,
    si: &SurfaceIntersector<'_>,
    handles: [O; 2],
) -> Result<Vec<SurfaceCache<O>>> {
    intersection_surface_caches_in(k, si, handles.map(Some), None)
}

/// [`intersection_surface_caches`] through the caller's face-aware cache context:
/// the context builds each cache with the surface handle that names it, so a surface
/// owned by a face reaches that face (upstream's `GetFace()`).
///
/// # Errors
///
/// Those of [`intersection_surface_caches`] and of the context's cache preparation.
pub fn intersection_surface_caches_with_context<O: Copy + PartialEq>(
    k: &Kernel,
    si: &SurfaceIntersector<'_>,
    handles: [Option<O>; 2],
    context: &mut dyn SurfaceSolveContext<O>,
) -> Result<Vec<SurfaceCache<O>>> {
    intersection_surface_caches_in(k, si, handles, Some(context))
}

fn intersection_surface_caches_in<O: Copy + PartialEq>(
    k: &Kernel,
    si: &SurfaceIntersector<'_>,
    handles: [Option<O>; 2],
    mut context: Option<&mut dyn SurfaceSolveContext<O>>,
) -> Result<Vec<SurfaceCache<O>>> {
    let ner = |e: SolidError| {
        if e.is_unported() {
            e
        } else {
            SolidError::null_pointer(OP_CACHES)
        }
    };
    let [h1, h2] = handles;
    let mut get = |s, h| -> Result<SurfaceCache<O>> {
        let (built, from_context) = match context.as_mut() {
            Some(ctx) => (ctx.build_surface_cache(k, s, h), true),
            None => (surface_cache::<O>(k, s, h), false),
        };
        let mut c = built.map_err(ner)?;
        // A context builds the cache with its tree ready (`GetTree` through the face
        // that owns the surface, `SurfaceSolveContext::build_surface_cache`): calling
        // `GetTree` again here, in the same state, would rebuild the same tree.
        if !(from_context && c.tree().is_some()) {
            c.get_tree(k).map_err(ner)?;
        }
        Ok(c)
    };
    let mut caches = vec![get(si.surface(SrfIndex::First), h1)?];
    if !si.doing_self_intersection {
        caches.push(get(si.surface(SrfIndex::Second), h2)?);
    }
    Ok(caches)
}

/// The surface handles of the caches [`do_intersection`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntersectionSurface {
    /// The first surface.
    First,
    /// The second surface.
    Second,
}

impl SolutionObject for IntersectionSurface {
    fn kind(&self) -> crate::geom::solution_array::ObjectKind {
        crate::geom::solution_array::ObjectKind::Surface
    }
}

impl SurfaceIntersector<'_> {
    /// The intersection of surfaces whose common box is thin (`GrazingBoxProcess`):
    /// for each corner of the first target domain inside `uv_domain1` and each
    /// corner of the second inside `uv_domain2` (u then v, minimum first), whose
    /// surface points are closer than `this_approx_tol_3d` and whose corner tangents
    /// (pointing into the domains) are not degenerate: when both tangents of the
    /// second surface, projected onto the first surface's tangent plane, are tiny,
    /// or one of them points into the first surface's corner sector, more work is
    /// needed; otherwise the corner is a touching point and no more work is needed.
    /// The first such corner pair decides; without one, more work is needed.
    ///
    /// Returns whether more work is needed and the touching point, if any.
    ///
    /// # Errors
    ///
    /// Those of the evaluations, unit vectors and degenerate curves.
    pub fn grazing_box_process(
        &self,
        k: &Kernel,
        uv_domain1: &Extent2d,
        uv_domain2: &Extent2d,
    ) -> Result<(bool, Option<GrazingBox>)> {
        // locals
        let approx_tol_3d = self.this_approx_tol_3d;

        // for all 4 Surface1->UVcorners of the crUVDomain1
        for iu1 in 0..2_u32 {
            for iv1 in 0..2_u32 {
                // skip UVcorner[iu1,uv1] not contained by crUVDomain1
                let uv1 = self.uv_domains[0].evaluate(f64::from(iu1), f64::from(iv1));
                if !uv_domain1.contains_point(uv1, 0.0) {
                    continue;
                }

                // for all 4 Surface2->UVCorners of the crUVDomain2
                for iu2 in 0..2_u32 {
                    for iv2 in 0..2_u32 {
                        // skip UVcorner[iu2,iv2] not contained by crUVDomain2
                        let uv2 = self.uv_domains[1].evaluate(f64::from(iu2), f64::from(iv2));
                        if !uv_domain2.contains_point(uv2, 0.0) {
                            continue;
                        }

                        // a corner inside both intersection domains: compare the
                        // 3D points
                        let (pnt1, mut du1, mut dv1) =
                            evaluate_1st_derivatives(k, self.surface(SrfIndex::First), uv1)?;
                        let (pnt2, mut du2, mut dv2) =
                            evaluate_1st_derivatives(k, self.surface(SrfIndex::Second), uv2)?;
                        if iu1 == 1 {
                            du1 = -du1;
                        }
                        if iv1 == 1 {
                            dv1 = -dv1;
                        }
                        if iu2 == 1 {
                            du2 = -du2;
                        }
                        if iv2 == 1 {
                            dv2 = -dv2;
                        }

                        // when 3D Points are within tolerance
                        if pnt1.distance(pnt2) < approx_tol_3d {
                            // skip singular surfaces
                            if du1.length_squared() < EFF_ZERO
                                || dv1.length_squared() < EFF_ZERO
                                || du2.length_squared() < EFF_ZERO
                                || dv2.length_squared() < EFF_ZERO
                            {
                                continue;
                            }

                            // normalize the tangents
                            let du1 = du1.unitize()?;
                            let dv1 = dv1.unitize()?;
                            let du2 = du2.unitize()?;
                            let dv2 = dv2.unitize()?;

                            // get Surface1 Normal
                            let norm1 = du1.cross(dv1);
                            if norm1.length_squared() < EFF_ZERO {
                                continue;
                            }

                            // sBin: orthogonalized sDV1, in the sDU1/sDV1 plane
                            let bin = norm1.cross(du1);

                            // project both sDU2 and sDV2 onto Surface1 plane
                            let proj_du2 = norm1.cross(du2).cross(norm1);
                            let proj_dv2 = norm1.cross(dv2).cross(norm1);
                            let mut u_inside = false;
                            let mut v_inside = false;

                            // both derivative vectors of the second surface are
                            // perpendicular to the first tangent plane: no good read
                            if proj_du2.length_squared() < EFF_ZERO_SQ
                                && proj_dv2.length_squared() < EFF_ZERO_SQ
                            {
                                return Ok((true, None));
                            }

                            // does the Surface2 domain corner extend into the
                            // Surface1 domain corner sector
                            if proj_du2.length_squared() > EFF_ZERO_SQ {
                                u_inside = proj_du2.is_inside_sector(du1, bin, dv1);
                            }
                            if proj_dv2.length_squared() > EFF_ZERO_SQ {
                                v_inside = proj_dv2.is_inside_sector(du1, bin, dv1);
                            }

                            if !u_inside && !v_inside {
                                // a point intersection - mark it with degenerate curves
                                let curve_3d =
                                    Curve::from(BSplineCurve::create_degenerate_curve(3, pnt1)?);
                                let uv_curve1 = Curve::from(BSplineCurve::create_degenerate_curve(
                                    2,
                                    Vec3::from(uv1),
                                )?);
                                let uv_curve2 = Curve::from(BSplineCurve::create_degenerate_curve(
                                    2,
                                    Vec3::from(uv2),
                                )?);
                                let dist = pnt1.distance(pnt2);
                                return Ok((
                                    false,
                                    Some(GrazingBox {
                                        curve_3d,
                                        uv_curves: [uv_curve1, uv_curve2],
                                        curve_type: TsectCurveType::Touching,
                                        deviation: dist,
                                    }),
                                ));
                            }

                            // further work is needed
                            return Ok((true, None));
                        }
                    }
                }
            }
        }
        Ok((true, None))
    }
}

/// `DoIntersection` as `SmSurface::GlobalSurfaceIntersect` runs it: an
/// [`AdvSurfaceIntersector`] of `surface1` over `uv_domain1` and `surface2` over
/// `uv_domain2` (not from a fillet intersector) using the boundaries of the surfaces
/// flagged in `use_surface_edges`, run on the surface caches
/// ([`intersection_surface_caches`], built for the call) with the optional
/// tolerances ([`AdvSurfaceIntersector::do_intersection_with_caches`]).
///
/// # Errors
///
/// Those of [`AdvSurfaceIntersector::new`], [`intersection_surface_caches`] and
/// [`AdvSurfaceIntersector::do_intersection_with_caches`].
pub fn do_intersection(
    k: &Kernel,
    surface1: &Surface,
    uv_domain1: &Extent2d,
    surface2: &Surface,
    uv_domain2: &Extent2d,
    use_surface_edges: [bool; 2],
    opt_approx_tol_3d: Option<f64>,
    opt_ang_tol_rad: Option<f64>,
) -> Result<SurfaceIntersection> {
    let mut caches: Vec<SurfaceCache<IntersectionSurface>>;
    let mut ssi: AdvSurfaceIntersector<'_, '_, IntersectionSurface> =
        AdvSurfaceIntersector::new(k, surface1, uv_domain1, surface2, uv_domain2, false)?;
    ssi.base
        .set_use_surface_edges(use_surface_edges[0], use_surface_edges[1]);
    caches = intersection_surface_caches(
        k,
        &ssi.base,
        [IntersectionSurface::First, IntersectionSurface::Second],
    )?;
    ssi.do_intersection_with_caches(k, &mut caches, opt_approx_tol_3d, opt_ang_tol_rad)
}

/// [`do_intersection`] through the caller's face-aware cache context. `handles` name
/// the two surfaces in their caches; the context builds each cache (and so reaches
/// the face that owns the surface) instead of the handle-less default.
///
/// # Errors
///
/// Those of [`do_intersection`] and of the context's cache preparation.
pub fn do_intersection_with_context<O: SolutionObject + Copy + PartialEq>(
    k: &Kernel,
    surface1: &Surface,
    uv_domain1: &Extent2d,
    surface2: &Surface,
    uv_domain2: &Extent2d,
    use_surface_edges: [bool; 2],
    opt_approx_tol_3d: Option<f64>,
    opt_ang_tol_rad: Option<f64>,
    context: &mut dyn SurfaceSolveContext<O>,
    handles: [Option<O>; 2],
) -> Result<SurfaceIntersection> {
    let mut caches: Vec<SurfaceCache<O>>;
    let mut ssi: AdvSurfaceIntersector<'_, '_, O> =
        AdvSurfaceIntersector::new(k, surface1, uv_domain1, surface2, uv_domain2, false)?;
    ssi.base
        .set_use_surface_edges(use_surface_edges[0], use_surface_edges[1]);
    caches = intersection_surface_caches_with_context(k, &ssi.base, handles, context)?;
    ssi.do_intersection_in(
        k,
        &mut caches,
        opt_approx_tol_3d,
        opt_ang_tol_rad,
        context.face_queries(),
        true,
    )
}

impl<'t, O: SolutionObject + Copy + PartialEq + 't> AdvSurfaceIntersector<'_, 't, O> {
    /// The surface/surface intersection (`DoIntersection`), on `caches`: the surface
    /// cache of the first surface and (unless it is the only one, for a self
    /// intersection) of the second; their trees are built when missing and become
    /// the solver trees.
    ///
    /// `opt_approx_tol_3d` / `opt_ang_tol_rad` replace `this_approx_tol_3d` /
    /// `this_ang_tol_rad` when given. Then:
    ///
    /// 1. Root boxes apart by more than the tolerance: no intersection. Else the
    ///    `SM_SO_INTERSECTION_TEST` solve collects the leaf pairs whose patches may
    ///    meet; none: no intersection. Their box (union of the leaf box overlaps)
    ///    and domains are the intersection region.
    /// 2. One leaf pair whose box is thinner than twice the tolerance in some
    ///    dimension: [`SurfaceIntersector::grazing_box_process`]; a touching corner
    ///    ends the intersection with that point.
    /// 3. The boundary bounding plane test
    ///    ([`SurfaceIntersector::do_boundary_plane_test`]): disjoint surfaces have no
    ///    intersection; grazing surfaces intersect in the boundary curve
    ///    intersections it found (without UV curves).
    /// 4. Unless intersecting a surface with itself, the boundary start points
    ///    ([`SurfaceIntersector::find_boundary_start_points`], which may add
    ///    boundary curves). From the last start point on, each is traced
    ///    ([`SurfaceIntersector::trace_curve`]; for a start point whose UV tangents
    ///    run along every boundary it is on, also backwards when the curve is not
    ///    closed) and flushed into a curve; a start point that gives no curve becomes
    ///    a through point.
    /// 5. The interior curves ([`AdvSurfaceIntersector::find_interior_curves`]).
    /// 6. Degenerate curves (`IsDegenerate()`, tolerance `SM_EFF_ZERO`) whose point
    ///    is on another curve are removed with their UV curves, type and deviation.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a cache or tree; those of the solves, the
    /// boundary tests, the flushes, the reversals, the backward traces and the curve
    /// predicates; [`SolidError::InvalidInput`] for curve arrays of different
    /// lengths; [`SolidError::Unported`] from unported callees.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub fn do_intersection_with_caches(
        &mut self,
        k: &Kernel,
        caches: &'t mut [SurfaceCache<O>],
        opt_approx_tol_3d: Option<f64>,
        opt_ang_tol_rad: Option<f64>,
    ) -> Result<SurfaceIntersection> {
        self.do_intersection_with_caches_and_queries(
            k,
            caches,
            opt_approx_tol_3d,
            opt_ang_tol_rad,
            None,
        )
    }

    /// [`AdvSurfaceIntersector::do_intersection_with_caches`] with the face
    /// classifications of the context that made the caches: the boundary start
    /// points' solves on a face's trimmed cache classify on that face
    /// ([`SurfaceIntersector::find_boundary_start_points_with_queries`]).
    ///
    /// # Errors
    ///
    /// As [`AdvSurfaceIntersector::do_intersection_with_caches`].
    pub fn do_intersection_with_caches_and_queries(
        &mut self,
        k: &Kernel,
        caches: &'t mut [SurfaceCache<O>],
        opt_approx_tol_3d: Option<f64>,
        opt_ang_tol_rad: Option<f64>,
        queries: Option<&dyn FaceQueries<O>>,
    ) -> Result<SurfaceIntersection> {
        self.do_intersection_in(
            k,
            caches,
            opt_approx_tol_3d,
            opt_ang_tol_rad,
            queries,
            false,
        )
    }

    /// [`AdvSurfaceIntersector::do_intersection_with_caches_and_queries`]; with
    /// `trees_ready` the caches came from a face-aware context with their trees ready
    /// (`GetTree` just ran through their faces), so a cache with a tree is not asked
    /// for it again: in the same state that `GetTree` would rebuild the same tree, and
    /// the two flags its rebuild sets are overwritten below.
    #[allow(clippy::too_many_lines)] // one upstream function
    fn do_intersection_in(
        &mut self,
        k: &Kernel,
        caches: &'t mut [SurfaceCache<O>],
        opt_approx_tol_3d: Option<f64>,
        opt_ang_tol_rad: Option<f64>,
        queries: Option<&dyn FaceQueries<O>>,
        trees_ready: bool,
    ) -> Result<SurfaceIntersection> {
        let mut result = SurfaceIntersection::default();
        // filled by FindInteriorCurves, read by the tangential curve pass (WR-75)
        self.tangent_curve_seeds.clear();

        if let Some(t) = opt_approx_tol_3d {
            self.base.this_approx_tol_3d = t;
        }
        if let Some(a) = opt_ang_tol_rad {
            self.base.this_ang_tol_rad = a;
        }
        let tol = self.base.this_approx_tol_3d;

        // get surface caches and their decomposition trees (NER)
        if caches.is_empty() {
            return Err(SolidError::null_pointer(OP_DI));
        }
        for c in caches.iter_mut().take(2) {
            if !(trees_ready && c.tree().is_some()) {
                c.get_tree(k).map_err(|e| {
                    if e.is_unported() {
                        e
                    } else {
                        SolidError::null_pointer(OP_DI)
                    }
                })?;
            }

            // Turn off point testing so GlobalPointSolve() keeps all point solutions;
            // turn on boundary curve processing so LocalSolve looks for drop points on
            // boundary curves (set here, before the first solve, which reads neither).
            c.point_test_enabled = false;
            c.process_boundary_curves = true;
        }

        // the boundary bounding planes of both caches (copies: lazily computed)
        let planes: Vec<PatchBoundaryBoundingPlanes> = caches
            .iter_mut()
            .take(2)
            .map(|c| c.patch_boundary_bounding_planes().clone())
            .collect();
        let mut planes1 = planes
            .first()
            .cloned()
            .ok_or(SolidError::null_pointer(OP_DI))?;
        let mut planes2 = planes.get(1).cloned().unwrap_or_else(|| planes1.clone());

        let caches: &'t [SurfaceCache<O>] = caches;
        let cache1 = caches.first().ok_or(SolidError::null_pointer(OP_DI))?;
        let cache2 = caches.get(1).unwrap_or(cache1);
        self.set_surface_caches(cache1, cache2)?;

        // no work - root treeNode bounding boxes are disjoint
        let root_box = |c: &SurfaceCache<O>| -> Result<Extent3d> {
            let tree = c.tree().ok_or(SolidError::null_pointer(OP_DI))?;
            let top = tree
                .top_node()
                .ok_or(SolidError::invalid_input(OP_DI, "tree without top node"))?;
            Ok(tree.node(top)?.bbox)
        };
        let root1 = root_box(cache1)?;
        let root2 = root_box(cache2)?;
        let mut bbox = root1;
        bbox.expand_absolute(tol);
        if bbox.are_disjoint(&root2, 0.0) {
            return Ok(result);
        }

        // get bounding box intersection - the region containing a possible
        // intersection (status ignored)
        if let Ok(b) = bbox.intersect(&root2) {
            bbox = b;
        }
        bbox.expand_absolute(tol);

        // Find node pairs in the tree where intersections might exist
        let mut solutions = SolutionArray::with_capacity(10);
        let request = SolveRequest {
            operation: SolverOperation::IntersectionTest,
            solution_requested: SolutionRequested::All,
            tolerance_3d: tol,
            best_answer_so_far_sq: BIG_DOUBLE,
            opt_vectors: &[],
        };
        self.solve_it(&request, &mut solutions)?;

        // no work - no intersecting leafNode bounding boxes
        let Some(sol) = solutions.get(0) else {
            return Ok(result);
        };
        let s = sol.start.parameters;
        let e = sol.end.parameters;

        // refine the intersection box from the leaf box intersections (status ignored)
        let _ = bbox.set_min_max(Vec3::new(s[0], s[1], s[2]), Vec3::new(s[3], s[4], s[5]));

        // a single leafNode box intersection - may be a point
        if s[6] < 2.0 {
            let size = bbox.size();

            // count the number of small dimensions: 3 = a point intersection
            let small_count = usize::from(size.x < 2.0 * tol)
                + usize::from(size.y < 2.0 * tol)
                + usize::from(size.z < 2.0 * tol);

            // when any dimension is smaller than 2*tolerance
            if small_count > 0 {
                let uv_domain1 =
                    Extent2d::from_points(Point2::new(e[0], e[1]), Point2::new(e[2], e[3]));
                let uv_domain2 =
                    Extent2d::from_points(Point2::new(e[4], e[5]), Point2::new(e[6], e[7]));

                // resolve the intersection
                let (more_work_required, touching) =
                    self.base.grazing_box_process(k, &uv_domain1, &uv_domain2)?;
                if !more_work_required {
                    if let Some(g) = touching {
                        let [uv1, uv2] = g.uv_curves;
                        result.curves_3d.push(g.curve_3d);
                        result.surface1_uv_curves.push(Some(uv1));
                        result.surface2_uv_curves.push(Some(uv2));
                        result.curve_types.push(g.curve_type);
                        result.deviations.push(g.deviation);
                    }
                    return Ok(result);
                }
            }
        }

        // arrive here when we need to do full intersections
        let mut out = TsectCurves::default();

        // check boundary curve bounding planes for possible disjointness or grazing
        let boundary_test_result = self.base.do_boundary_plane_test(
            k,
            &mut planes1,
            &mut planes2,
            tol,
            &bbox,
            &mut out,
        )?;

        // set output with any graze intersections
        if boundary_test_result == BoundaryPlaneTestResult::Graze && !out.curves_3d.is_empty() {
            let n = out.curves_3d.len();
            result.curves_3d = core::mem::take(&mut out.curves_3d);
            result.curve_types = core::mem::take(&mut out.curve_types);
            result.deviations = core::mem::take(&mut out.deviations);
            result.surface1_uv_curves = vec![None; n];
            result.surface2_uv_curves = vec![None; n];
        }

        // Return if the boundary plane test detected disjointness or grazing
        if matches!(
            boundary_test_result,
            BoundaryPlaneTestResult::Graze | BoundaryPlaneTestResult::Disjoint
        ) {
            return Ok(result);
        }

        // get StartPoints = Surf1->NaturalBoundaryCurve/Surf2 Intersections and
        // Surf1/Surf2->NaturalBoundaryCurve Intersections (on copies of the caches:
        // exact ones, since upstream searches its own caches)
        if !self.base.doing_self_intersection {
            let mut boundary_caches = caches
                .iter()
                .map(SurfaceCache::clone_for_evaluation)
                .collect::<Result<Vec<_>>>()?;
            self.base.find_boundary_start_points_with_queries(
                k,
                &mut boundary_caches,
                &bbox,
                &mut out,
                queries,
            )?;
        }

        // while there are boundaryCurve/Surface xsect startPoints, use them to trace
        // out intersection curves (each pass pops one; none adds any)
        while let Some(&start) = self.base.start_points.last() {
            // keep a copy of the start point (operator= into a fresh element)
            let mut tmp = TsectPnt::zeroed();
            tmp.assign(self.base.point(start)?);
            let tmp_id = self.base.alloc_point(tmp);
            self.base.curve_trace_direction = 1.0;
            let num_curves = out.curves_3d.len();

            // Remove Last StartPoint from m_vStartPoint array
            self.base.start_points.pop();

            // Trace an intersection curve onto m_vCurvePoints.
            let input_trace_direction = self.base.curve_trace_direction;
            if ignored(self.base.trace_curve(k, start, &out.curves_3d))?.is_some() {
                // StartPoint's UVTangent is tangent to the Surface domain boundary:
                // trace the curve in both directions
                let sp = self.base.point(start)?;
                let along = |srf: SrfIndex| {
                    (sp.u_param_type(srf) == ExtentPointType::Inside
                        || is_zero_to_tol(sp.uv_deriv(srf).x, EFF_ZERO))
                        && (sp.v_param_type(srf) == ExtentPointType::Inside
                            || is_zero_to_tol(sp.uv_deriv(srf).y, EFF_ZERO))
                };
                // (opt-in, WR-80: a trace that added no point, its start already on a
                // found curve, has nothing to trace back from; upstream reverses the empty
                // list and fails with SM_ERR_NULL_POINTER)
                let traced =
                    !k.fixes.ssi_no_reverse_of_empty_trace || !self.base.curve_points.is_empty();
                if traced
                    && !self.base.curve_is_closed
                    && input_trace_direction == self.base.curve_trace_direction
                    && along(SrfIndex::First)
                    && along(SrfIndex::Second)
                {
                    // trace the curve in the opposite direction
                    self.base.reverse_curve_direction()?;
                    self.base.curve_trace_direction *= -1.0;
                    self.base.trace_curve(k, start, &out.curves_3d)?;

                    // restore the curve direction (upstream multiplies by 1.0)
                    self.base.curve_trace_direction *= 1.0;
                    self.base.reverse_curve_direction()?;
                }

                // opt-in fix (WR-76, #392): a traced curve that ends on a boundary
                // inside a tangential contact consumes the start points of that
                // contact's boundary cluster
                if k.fixes.ssi_trace_tangent_curves_once {
                    self.base.remove_end_tangent_clusters(k)?;
                }

                // create the curves from the points in m_vCurvePoints
                self.base.flush_curve(k, &mut out)?;
            }

            // when an intersection curve was not generated from this startPoint
            if num_curves == out.curves_3d.len() {
                self.base.through_points.push(tmp_id);
            }
        }

        // Now that we have found all boundary intersections - look for non boundary
        // intersections (the SmAdvSurfaceIntersector override)
        let mut interior = InteriorCurves {
            curves_3d: core::mem::take(&mut out.curves_3d),
            surface1_uv_curves: core::mem::take(&mut out.uv_curves[0]),
            surface2_uv_curves: core::mem::take(&mut out.uv_curves[1]),
            curve_types: core::mem::take(&mut out.curve_types),
            deviations: core::mem::take(&mut out.deviations),
        };
        self.find_interior_curves(k, &mut interior)?;

        // not upstream (WR-75, opt-in; the fork's FindTangentCurves): trace the
        // tangential curves the marcher above cannot follow
        let mut tangent_nodes = Vec::new();
        if k.fixes.ssi_tangent_curve_tracer {
            let size = crate::math::min2(root1.size().length(), root2.size().length());
            let seeds = core::mem::take(&mut self.tangent_curve_seeds);
            let mut curves = TsectCurves {
                curves_3d: core::mem::take(&mut interior.curves_3d),
                uv_curves: [
                    core::mem::take(&mut interior.surface1_uv_curves),
                    core::mem::take(&mut interior.surface2_uv_curves),
                ],
                curve_types: core::mem::take(&mut interior.curve_types),
                deviations: core::mem::take(&mut interior.deviations),
            };
            let traced = super::tangent_curves::find_tangent_curves(
                k,
                &mut self.base,
                size,
                &seeds,
                &mut curves,
            );
            let TsectCurves {
                curves_3d,
                uv_curves: [surface1_uv_curves, surface2_uv_curves],
                curve_types,
                deviations,
            } = curves;
            interior = InteriorCurves {
                curves_3d,
                surface1_uv_curves,
                surface2_uv_curves,
                curve_types,
                deviations,
            };
            tangent_nodes = traced?;
        }
        let InteriorCurves {
            mut curves_3d,
            mut surface1_uv_curves,
            mut surface2_uv_curves,
            mut curve_types,
            mut deviations,
        } = interior;

        // remove degenerate Curves whose points lie on nonDegenerate intersections
        let mut ii = 0;
        while let Some(curve) = curves_3d.get(ii) {
            if curve.is_degenerate(k, EFF_ZERO, None)? {
                // status ignored: an unset point
                let point = ignored(curve.evaluate_point(k, 0.0))?.unwrap_or(Vec3::UNDEF);
                if self
                    .base
                    .is_point_on_curve(k, point, &curves_3d, Some(ii))?
                {
                    if ii >= surface1_uv_curves.len()
                        || ii >= surface2_uv_curves.len()
                        || ii >= deviations.len()
                        || ii >= curve_types.len()
                    {
                        return Err(SolidError::invalid_input(
                            OP_DI,
                            "curve arrays of different lengths (upstream reads past them)",
                        ));
                    }
                    curves_3d.remove(ii);
                    surface1_uv_curves.remove(ii);
                    surface2_uv_curves.remove(ii);
                    deviations.remove(ii);
                    curve_types.remove(ii);
                    continue;
                }
            }
            ii += 1;
        }

        // not upstream (WR-75, opt-in; the fork's AddTangentNodes): report the singular
        // points where crossing branches meet
        if k.fixes.ssi_tangent_curve_tracer {
            super::tangent_curves::add_tangent_nodes(
                k,
                &self.base,
                &tangent_nodes,
                &mut curves_3d,
                &mut surface1_uv_curves,
                &mut surface2_uv_curves,
                &mut curve_types,
                &mut deviations,
            )?;
        }

        // set outputs
        result.curves_3d.extend(curves_3d);
        result
            .surface1_uv_curves
            .extend(surface1_uv_curves.into_iter().map(Some));
        result
            .surface2_uv_curves
            .extend(surface2_uv_curves.into_iter().map(Some));
        result.deviations.extend(deviations);
        result.curve_types.extend(curve_types);
        Ok(result)
    }
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`.
fn evaluate_1st_derivatives(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
) -> Result<(Point3, Vec3, Vec3)> {
    crate::surface::surface::p1::evaluate_1st_derivatives(k, surface, uv, Side::Left, Side::Left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SM_ERR_NULL_POINTER;
    use crate::intersect::adv_surface_intersector::NoSurfaceHandle;
    use crate::surface::plane::Plane;

    fn plane(o: [f64; 3], x: [f64; 3], y: [f64; 3]) -> Surface {
        Surface::from(Plane::new(
            Vec3::from_array(o),
            Vec3::from_array(x),
            Vec3::from_array(y),
            Point2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    fn unit() -> Extent2d {
        Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
    }

    fn corner(u: f64, v: f64) -> Extent2d {
        Extent2d::from_bounds(u - 0.1, v - 0.1, u + 0.1, v + 0.1)
    }

    #[test]
    fn grazing_corner_leaving_the_other_surface_is_a_touching_point() {
        let k = &Kernel::default();
        // B starts at A's corner (1, 1, 0) and leaves A (along (1, 1, 0) and z)
        let s = 0.5_f64.sqrt();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([1.0, 1.0, 0.0], [s, s, 0.0], [0.0, 0.0, 1.0]);
        let si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let (more, touching) = si
            .grazing_box_process(k, &corner(1.0, 1.0), &corner(0.0, 0.0))
            .unwrap();
        assert!(!more);
        let g = touching.unwrap();
        assert_eq!(g.curve_type, TsectCurveType::Touching);
        assert_eq!(g.deviation, 0.0);
        let p = g.curve_3d.evaluate_point(k, 0.5).unwrap();
        assert!(p.distance(Vec3::new(1.0, 1.0, 0.0)) < 1e-15);
        let uv2 = g.uv_curves[1].evaluate_point(k, 0.0).unwrap();
        assert!(uv2.distance(Vec3::ZERO) < 1e-15);
    }

    #[test]
    fn grazing_corner_entering_the_other_surface_needs_more_work() {
        let k = &Kernel::default();
        // B's u axis runs back over A from its corner
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([1.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        let (more, touching) = si
            .grazing_box_process(k, &corner(1.0, 1.0), &corner(0.0, 0.0))
            .unwrap();
        assert!(more);
        assert!(touching.is_none());
    }

    #[test]
    fn grazing_without_coincident_corners_needs_more_work() {
        let k = &Kernel::default();
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let si = SurfaceIntersector::new(k, &a, &unit(), &b, &unit(), false).unwrap();
        // corners in both domains, 1 apart
        let (more, touching) = si.grazing_box_process(k, &unit(), &unit()).unwrap();
        assert!(more);
        assert!(touching.is_none());
        // no corner inside the intersection domains
        let inner = Extent2d::from_bounds(0.4, 0.4, 0.6, 0.6);
        let (more, _) = si.grazing_box_process(k, &inner, &inner).unwrap();
        assert!(more);
    }

    #[test]
    fn no_cache_is_a_null_pointer() {
        let a = plane([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let k = Kernel::default();
        let mut ssi: AdvSurfaceIntersector<'_, '_, NoSurfaceHandle> =
            AdvSurfaceIntersector::new(&k, &a, &unit(), &b, &unit(), false).unwrap();
        let mut caches: Vec<SurfaceCache<NoSurfaceHandle>> = Vec::new();
        let e = ssi
            .do_intersection_with_caches(&k, &mut caches, Some(0.5), Some(0.1))
            .unwrap_err();
        assert!(matches!(
            e,
            SolidError::Upstream {
                code: SM_ERR_NULL_POINTER,
                ..
            }
        ));
        // the tolerances are set first
        assert_eq!(ssi.base.this_approx_tol_3d, 0.5);
        assert_eq!(ssi.base.this_ang_tol_rad, 0.1);
    }
}
