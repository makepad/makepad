//! `SmSurfaceIntersector`, part 3: the start points on the natural boundaries
//! ([`SurfaceIntersector::find_boundary_start_points`]), the tracing step
//! ([`SurfaceIntersector::compute_next_point`]), the end-of-trace test with its
//! snapping to through, start and end points ([`SurfaceIntersector::test_point`],
//! [`SurfaceIntersector::update_tsect_point`]) and the fillet-law span split
//! ([`SurfaceIntersector::split_span`]).
//!
//! Upstream: `source/SMLib/src/SmSurfaceIntersector.cpp`.
//!
//! | Rust                                                  | Upstream                                   |
//! |-------------------------------------------------------|--------------------------------------------|
//! | [`SurfaceIntersector::find_boundary_start_points`], [`SurfaceIntersector::find_boundary_start_points_with_caches`], [`SurfaceIntersector::find_boundary_start_points_with_queries`] | `FindBoundaryStartPoints` |
//! | [`SurfaceIntersector::compute_next_point`], [`NextPoint`] | `ComputeNextPoint`                     |
//! | [`SurfaceIntersector::update_tsect_point`], [`update_tsect_point_with`] | `sm_UpdateTsectPoint` (static) |
//! | [`SurfaceIntersector::test_point`], [`test_point_with`] | `TestPoint`                              |
//! | [`SurfaceIntersector::split_span`], [`split_span_with`] | `SplitSpan`                              |
//! | [`TsectPnt::zeroed`]                                  | a point fresh from `m_vTSPntMgr` (zero-filled) |
//!
//! Deviations:
//! - **Virtual calls.** `TestPoint` and `sm_UpdateTsectPoint` are generic over the
//!   intersector (`intersect::surface_intersector::p4::IntersectorVirtuals`), so the
//!   `ComputePointValues` of `sm_UpdateTsectPoint` is the derived intersector's when
//!   `TraceCurve` runs on one ([`test_point_with`], [`update_tsect_point_with`]); the
//!   methods run them on the basic intersector. `SplitSpan` is generic the same way
//!   ([`split_span_with`]), for the virtual calls of `ComputeLawPointValues`.
//! - **Points.** Points are arena ids ([`TsectId`]), as in part 1: `rNextTSP`, the
//!   previous point and the snap targets of `TestPoint` / `sm_UpdateTsectPoint` are
//!   points of the arena (`TraceCurve` takes `rNextTSP` from `m_vTSPntMgr` too). A next
//!   point that is also the previous point (the last curve point) is rejected with
//!   [`SolidError::InvalidInput`]: upstream would evaluate a point onto itself
//!   through two aliases. Every other alias (a snap target that is the next point)
//!   behaves as upstream: the arena is re-read after each write.
//! - **Null previous point.** `TestPoint` dereferences the last curve point
//!   whenever there are through points, a start point (`m_pStartPoint`) or an end
//!   point; with an empty curve list that is a null dereference upstream and
//!   [`SolidError::InvalidInput`] here.
//! - **Outputs.** Out-params become return values. `TestPoint`'s `rbDone` is
//!   `None` when upstream leaves it unchanged (a through point was hit);
//!   `ComputeNextPoint`'s deviation and angle are `None` when upstream leaves them
//!   unset. `SplitSpan` returns the new point's id. `FindBoundaryStartPoints`'
//!   output arrays are a [`TsectCurves`] (part 2).
//! - **Surface caches.** `smsurf_GetSurfaceCache` ([`surface_cache`]) builds a
//!   fresh cache per call (the cache store keeps none), so the one of each surface
//!   is fetched once per call (once for a self intersection, where upstream gets
//!   the same cache twice); a surface without one is `SM_ERR_NULL_POINTER` (`NER`).
//!   Their `process_boundary_curves` flags are set as upstream's
//!   `SmTemporaryChangeValue`s set them; a callee that fetches its own cache does not
//!   see them. [`SurfaceIntersector::find_boundary_start_points_with_caches`] takes
//!   the caches from the caller instead. Their iso boundary curves are read, and
//!   `GlobalCurveIntersect`, which upstream runs on the other surface's one cache
//!   (`smsurf_GetSurfaceCache` returns the intersector's), runs on a copy of the
//!   caller's cache whose handles name the curve and surface objects of the solve
//!   ([`Surface::global_curve_intersect_in`], `SurfaceCache::map_handles`), so its
//!   line and tree solves see the same tree. The temporary removal of the other surface's owner (point validation)
//!   has no counterpart: Rust surfaces have no owner.
//! - **Points of `FindBoundaryStartPoints`.** Upstream takes a point from the block
//!   manager for every solution and leaks those it does not keep; here a solution's
//!   point is a local [`TsectPnt::zeroed`] value that gets its arena slot when it
//!   becomes a start point.
//! - **Dead code.** The face edge branch of `FindBoundaryStartPoints`
//!   (`static constexpr bGWCAddEdgeThroughPoints = FALSE`: target curves beyond the
//!   four natural boundaries, `ApproximateCurve` of non-B-spline targets, through
//!   points, `RefinePoint` of edge crossings) and the parts of `TestPoint` behind
//!   `static bStopAtSingularity = FALSE` (stop at singular points, the final through
//!   and end point snapping of a boundary point) are unreachable upstream and not
//!   ported. The cycle back-edges of the plan (`topology.edge`, `topology.edgeuse_p1`,
//!   `topology.edgeuse_p3`, `topology.face_p4`) are all in the face edge branch.
//! - **Previous UVs.** `ComputeNextPoint` hands `RefinePoint` the address of the
//!   previous point's first UV position, which the callee may read as the array
//!   `m_vUVCurvePV` (`[uv0, duv0, uv1, duv1]`); that array is passed.
//! - **Unported callees.**
//!   `SmSurface::DropAndTrimCurve`'s trace (`intersect.surface_drop_curve`, through
//!   [`Surface::drop_and_trim_curve`]) return [`SolidError::Unported`], which always propagates (also where upstream
//!   ignores the status).
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks (graphics, dumps), the
//!   `SM_ASSERT`s (no-ops in release builds) and the user pointers
//!   (`m_apUserPointer`, not modelled by [`TsectPnt`]).

use crate::blend::fillet_standard_solver::p1::FilletLaw;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError, SM_ERR_NULL_POINTER};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionEnd, SolutionObject, SolutionType,
};
use crate::geom::tol::{is_zero, ZONE_TOL_3D};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p2::surface_cache;
use crate::surface::surface::p2::TsectCurveType;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::surface_cache::face_context::FaceQueries;
use crate::surface::surface_cache::SurfaceCache;
use crate::topology::ObjectHandle;

use super::p1::{
    compute_curve_type, is_point_on_tsect_curve, IntersectionPointType, SrfIndex,
    SurfaceIntersector, TsectId, TsectPnt, MAX_USER_DOUBLES,
};
use super::p2::{LawPointOptions, TsectCurves};

const OP_FBSP: &str =
    "intersect::surface_intersector::p3::SurfaceIntersector::find_boundary_start_points";
const OP_NEXT: &str = "intersect::surface_intersector::p3::SurfaceIntersector::compute_next_point";
const OP_UPDATE: &str =
    "intersect::surface_intersector::p3::SurfaceIntersector::update_tsect_point";
const OP_TEST: &str = "intersect::surface_intersector::p3::SurfaceIntersector::test_point";

/// The outputs of [`SurfaceIntersector::compute_next_point`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NextPoint {
    /// `rbFoundGoodPoint`: the next point passed the direction, angle and deviation
    /// tests.
    pub found_good_point: bool,
    /// `rdDeviationFound`: the largest curve/surface distance of the span test
    /// points; `None` when upstream leaves it unset.
    pub deviation_found: Option<f64>,
    /// `rdAngleFoundRad`: the angle between the two curve tangents; `None` when
    /// upstream leaves it unset.
    pub angle_found_rad: Option<f64>,
    /// `rbClipped`: the step was shortened to keep the next point in both domains.
    pub clipped: bool,
    /// `rbBoundaryHit`: the step leaves a domain (or the next point could not be
    /// evaluated); the trace ends.
    pub boundary_hit: bool,
}

impl TsectPnt {
    /// A point as `m_vTSPntMgr.GetNewElement()` hands it out: zero-filled memory,
    /// never constructed (type `SM_IP_UNDEFINED`, parameter types `SM_EP_START`,
    /// every vector and value zero).
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            point_type: IntersectionPointType::Undefined,
            uv_param_type: [[ExtentPointType::Start; 2]; 2],
            uv_curve_pv: [[Vec2::new(0.0, 0.0); 2]; 2],
            surface_pv: [[[Vec3::ZERO; 3]; 3]; 2],
            curve_pv: [Vec3::ZERO; 2],
            curve_parameter: 0.0,
            tangent_plane_angle_rad: 0.0,
            deviation: 0.0,
            user_doubles: [0.0; MAX_USER_DOUBLES],
            user_pointer: [None, None],
        }
    }
}

/// The last output 3D curve (the one being added).
fn last_curve(out: &mut TsectCurves) -> Result<&mut Curve> {
    out.curves_3d
        .last_mut()
        .ok_or_else(|| SolidError::invalid_input(OP_FBSP, "no output curve"))
}

/// `rSol.m_vStart[i]` / `m_vEnd[i]`.
fn par(e: &SolutionEnd, i: usize) -> f64 {
    e.parameters.get(i).copied().unwrap_or(0.0)
}

/// Removes `id` from a point list (`SmTList::Remove`).
fn unlink(list: &mut Vec<TsectId>, id: TsectId) {
    if let Some(pos) = list.iter().position(|&x| x == id) {
        list.remove(pos);
    }
}

/// A status upstream ignores (`Unported` propagates).
fn ignored<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// `a[srf]`.
const fn at<T>(a: &[T; 2], s: SrfIndex) -> &T {
    match s {
        SrfIndex::First => &a[0],
        SrfIndex::Second => &a[1],
    }
}

/// `a[srf]`, mutable.
fn at_mut<T>(a: &mut [T; 2], s: SrfIndex) -> &mut T {
    match s {
        SrfIndex::First => &mut a[0],
        SrfIndex::Second => &mut a[1],
    }
}

/// `SrfIndex` of `m_cpSurface[i]`.
const fn srf(i: usize) -> SrfIndex {
    if i == 0 {
        SrfIndex::First
    } else {
        SrfIndex::Second
    }
}

/// A boundary start point waiting for classification: the point, the boundary curve
/// point and the index of the other surface (`sDegeneratePoints`, `s3DPoints`,
/// `sSurfIndex`).
struct DegeneratePoint {
    point: TsectPnt,
    curve_point: Point3,
    surf_index: SrfIndex,
}

/// The working lists of [`SurfaceIntersector::find_boundary_start_points`].
#[derive(Default)]
struct BoundaryWork {
    /// `sEdgeCrossings`.
    edge_crossings: Vec<TsectPnt>,
    /// `sDegeneratePoints`, `s3DPoints`, `sSurfIndex`.
    degenerate: Vec<DegeneratePoint>,
}

impl SurfaceIntersector<'_> {
    // ---- boundary start points ---------------------------------------------------------

    /// Finds intersection start points on the natural boundaries
    /// (`FindBoundaryStartPoints`): for each surface whose edges are used
    /// (`use_surface_edges`), each non-degenerate boundary iso curve of its domain
    /// (minimum u, minimum v, maximum u, maximum v, within `this_approx_tol_3d / 3` of
    /// the surface) whose box, expanded by `this_approx_tol_3d`, meets
    /// `intersection_box` is intersected with the other surface.
    ///
    /// A point solution (or a range shorter than a hundredth of the boundary, refined
    /// by a local curve solve) off the surface singularities is evaluated; a
    /// degenerate one (touching, [`SurfaceIntersector::classify_boundary_start_point_intersection`])
    /// becomes a degenerate output curve of type touching unless it is on an output
    /// curve or a start point; else, unless it is on an output curve, it becomes a
    /// start point (crossings on the other surface's boundary last). A longer range
    /// whose end deviations differ by less than a factor of 100 and whose middle is on
    /// no output curve becomes an output curve: the boundary curve trimmed to it and
    /// dropped onto both surfaces, kept when within `this_approx_tol_3d` of both, with
    /// its type; start points and crossings on it are then dropped.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a surface without a surface cache; those of the iso
    /// curves, the bounding boxes, the evaluations, the trims and the curve solves;
    /// [`SolidError::Unported`] from unported callees.
    pub fn find_boundary_start_points(
        &mut self,
        k: &Kernel,
        intersection_box: &Extent3d,
        out: &mut TsectCurves,
    ) -> Result<()> {
        // get surface caches (NER: no cache is SM_ERR_NULL_POINTER)
        let get = |i: SrfIndex| match surface_cache::<SrfIndex>(k, self.surface(i), Some(i)) {
            Ok(c) => Ok(c),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Err(SolidError::Upstream {
                operation: OP_FBSP,
                code: SM_ERR_NULL_POINTER,
                entity: None,
            }),
        };
        let mut caches = vec![get(SrfIndex::First)?];
        if !self.doing_self_intersection {
            caches.push(get(SrfIndex::Second)?);
        }

        self.find_boundary_start_points_with_caches(k, &mut caches, intersection_box, out)
    }

    /// [`SurfaceIntersector::find_boundary_start_points`] with the given surface
    /// caches: `caches[0]` of the first surface, `caches[1]` (or `caches[0]` when only
    /// one is given, as for a self intersection) of the second. Only their iso boundary
    /// curves are used; their `process_boundary_curves` flags are set for the call and
    /// restored on every exit (`SmTemporaryChangeValue`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without a cache; else as
    /// [`SurfaceIntersector::find_boundary_start_points`].
    pub fn find_boundary_start_points_with_caches<O: Copy + PartialEq>(
        &mut self,
        k: &Kernel,
        caches: &mut [SurfaceCache<O>],
        intersection_box: &Extent3d,
        out: &mut TsectCurves,
    ) -> Result<()> {
        self.find_boundary_start_points_with_queries(k, caches, intersection_box, out, None)
    }

    /// [`SurfaceIntersector::find_boundary_start_points_with_caches`] with the face
    /// classifications of the caller's context for trimmed caches: the boundary
    /// curve intersections' point solves test their fast-drop solutions against the
    /// face of a trimmed cache with point testing on (`GlobalPointSolve` reaching
    /// `pFace->PointClassify`).
    ///
    /// # Errors
    ///
    /// As [`SurfaceIntersector::find_boundary_start_points_with_caches`], and those of
    /// the classifications.
    pub fn find_boundary_start_points_with_queries<O: Copy + PartialEq>(
        &mut self,
        k: &Kernel,
        caches: &mut [SurfaceCache<O>],
        intersection_box: &Extent3d,
        out: &mut TsectCurves,
        queries: Option<&dyn FaceQueries<O>>,
    ) -> Result<()> {
        if caches.is_empty() {
            return Err(SolidError::Upstream {
                operation: OP_FBSP,
                code: SM_ERR_NULL_POINTER,
                entity: None,
            });
        }

        // Turn on boundary curve processing to force LocalSolve to look for drop
        // points on boundary curves (restored on every exit, last change first).
        let saved: Vec<bool> = caches
            .iter()
            .take(2)
            .map(|c| c.process_boundary_curves)
            .collect();
        for c in caches.iter_mut().take(2) {
            c.process_boundary_curves = true;
        }
        let r = self.find_boundary_start_points_body(k, caches, intersection_box, out, queries);
        for (c, s) in caches.iter_mut().zip(saved).rev() {
            c.process_boundary_curves = s;
        }
        r
    }

    #[allow(clippy::too_many_lines)]
    fn find_boundary_start_points_body<O: Copy + PartialEq>(
        &mut self,
        k: &Kernel,
        caches: &mut [SurfaceCache<O>],
        intersection_box: &Extent3d,
        out: &mut TsectCurves,
        queries: Option<&dyn FaceQueries<O>>,
    ) -> Result<()> {
        let mut work = BoundaryWork::default();
        // sIntervals keeps its values from surface to surface (SetMinMax failures)
        let mut intervals = [Extent1d::new(); 4];

        // for both surfaces - gather and intersect surf NaturalBoundaryCurves with
        // the other surface
        for l_srf in 0..2_usize {
            let this = srf(l_srf);
            let other = srf(1 - l_srf);

            // skip surfaces marked as not using Surface edges
            if !*at(&self.use_surface_edges, this) {
                continue;
            }

            let domain = *at(&self.uv_domains, this);
            let other_domain = *at(&self.uv_domains, other);

            // Get thisSurface->3DBoundaryCurves: [0] min u, [1] min v, [2] max u,
            // [3] max v
            let cache_index = l_srf.min(caches.len() - 1);
            let cache = caches.get_mut(cache_index).ok_or(SolidError::Upstream {
                operation: OP_FBSP,
                code: SM_ERR_NULL_POINTER,
                entity: None,
            })?;
            let iso = cache.iso_boundary_curves(k, &domain, self.this_approx_tol_3d / 3.0)?;

            // add the intervals of the iso curves (statuses ignored)
            for (ii, ivl) in intervals.iter_mut().enumerate() {
                if ii % 2 == 1 {
                    let _ = ivl.set_min_max(domain.min().x, domain.max().x);
                } else {
                    let _ = ivl.set_min_max(domain.min().y, domain.max().y);
                }
            }

            // for every TargetCurve - intersect ThisSurface->BoundaryCurves with
            // OtherSurface
            for (l_crv, tgt) in iso.curves.iter().enumerate() {
                let norm_param = if l_crv < 2 { 0.0 } else { 1.0 };
                let l_coord = l_crv % 2;
                let l_side = usize::from(l_crv >= 2);
                let bsc_ivl = intervals.get(l_crv).copied().unwrap_or_default();

                // skip degenerate TargetCurves
                if tgt.is_degenerate(k, EFF_ZERO, Some(&bsc_ivl))? {
                    continue;
                }

                // BoundaryCurve BBox + Tol
                let boxes = tgt.calculate_bounding_box(
                    k,
                    &bsc_ivl,
                    BoundingBoxRequest {
                        normal: true,
                        pseudo: false,
                        polar: false,
                    },
                    true,
                )?;
                let mut bbox = boxes.normal.unwrap_or_default();
                bbox.expand_absolute(self.this_approx_tol_3d);

                // when the intersection box meets the boundary curve box, intersect
                // OtherSurface with ThisSurface->BoundaryCurve
                let tgt_curve = tgt.clone();
                let solutions = if bbox.are_disjoint(intersection_box, 0.0) {
                    Vec::new()
                } else {
                    // on OtherSurface's cache: upstream's GlobalCurveIntersect fetches
                    // that same cache (smsurf_GetSurfaceCache) and solves on its tree
                    let other_cache = caches.get((1 - l_srf).min(caches.len() - 1)).ok_or(
                        SolidError::Upstream {
                            operation: OP_FBSP,
                            code: SM_ERR_NULL_POINTER,
                            entity: None,
                        },
                    )?;
                    surface_global_curve_intersect(
                        k,
                        self.surface(other),
                        other_cache,
                        &other_domain,
                        &tgt_curve,
                        &bsc_ivl,
                        self.this_approx_tol_3d,
                        queries,
                    )?
                };

                // for every intersection solution
                for sol in &solutions {
                    let mut sol = *sol;
                    let iso_uv = domain.evaluate(norm_param, norm_param);
                    // the uv point on this surface of curve parameter t
                    let this_uv = |t: f64| {
                        if l_coord == 0 {
                            Point2::new(iso_uv.x, t)
                        } else {
                            Point2::new(t, iso_uv.y)
                        }
                    };

                    // a point for this solution (from the block manager upstream)
                    let mut tsp = TsectPnt::zeroed();
                    tsp.point_type = IntersectionPointType::Unknown;

                    // For single intersection points
                    if sol.solution_type == SolutionType::SingleValue {
                        let mut uvs = [Point2::new(0.0, 0.0); 2];
                        *at_mut(&mut uvs, other) =
                            Point2::new(par(&sol.start, 1), par(&sol.start, 2));
                        *at_mut(&mut uvs, this) = this_uv(par(&sol.start, 0));

                        // skip start points which are on surface singularities
                        if surface_is_singularity(k, self.surface(this), *at(&uvs, this))?
                            || surface_is_singularity(k, self.surface(other), *at(&uvs, other))?
                        {
                            continue;
                        }

                        // Compute xsect point values for the mated UVPoints (a start
                        // point: no previous point)
                        if ignored(self.compute_point_values(k, uvs, &mut tsp, None, None))?
                            .is_none()
                        {
                            continue;
                        }

                        self.use_boundary_point(
                            k,
                            tgt,
                            &sol,
                            uvs,
                            tsp,
                            other,
                            &other_domain,
                            out,
                            &mut work,
                        )?;
                    }
                    // For interval intersections
                    else if sol.solution_type == SolutionType::RangeOfValues {
                        let size = par(&sol.end, 0) - par(&sol.start, 0);
                        let ivl_size = bsc_ivl.max() - bsc_ivl.min();

                        if size > ivl_size / 100.0 {
                            // Large intersection: a coincident boundary curve
                            if sol.end.solution_value < EFF_ZERO {
                                sol.end.solution_value = EFF_ZERO;
                            }
                            if sol.start.solution_value < EFF_ZERO {
                                sol.start.solution_value = EFF_ZERO;
                            }
                            let size_ratio = sol.start.solution_value / sol.end.solution_value;

                            // Skip curves whose deviation changes too much
                            if size_ratio > 100.00 || size_ratio < 0.01 {
                                continue;
                            }

                            // skip curves that are already found
                            let test_param = (par(&sol.start, 0) + par(&sol.end, 0)) / 2.0;
                            let test_pnt = tgt.evaluate_point(k, test_param)?;
                            if self.is_point_on_curve(k, test_pnt, &out.curves_3d, None)? {
                                continue;
                            }

                            self.add_boundary_curve(k, caches, tgt, &sol, l_side, out, &mut work)?;
                        } else {
                            // Small intersection - just use as start points
                            let mut t = par(&sol.start, 0);
                            let mut uv = Point2::new(par(&sol.start, 1), par(&sol.start, 2));
                            let mut uv_ivl2 =
                                Extent1d::from_min_max(par(&sol.start, 0), par(&sol.end, 0));
                            uv_ivl2.expand_absolute(ivl_size * EFF_ZERO_SQRT);
                            uv_ivl2 = uv_ivl2
                                .intersect(&tgt.natural_interval())
                                .unwrap_or_default();

                            // use the better end unless the start is on a boundary
                            if sol.start.solution_value > sol.end.solution_value
                                && !other_domain.is_point_on_boundary(uv, EFF_ZERO).0
                            {
                                sol.start = sol.end;
                            }

                            // refine the point
                            let found = self.surface(other).local_curve_solve(
                                k,
                                &other_domain,
                                &tgt_curve,
                                &uv_ivl2,
                                crate::geom::global_solver::SolverOperation::Normalize,
                                self.this_approx_tol_3d,
                                &[],
                                uv,
                                t,
                            )?;
                            if let Some(s) = found {
                                if s.distance < sol.start.solution_value {
                                    sol.start.solution_value = s.distance;
                                    if let Some(p) = sol.start.parameters.get_mut(0) {
                                        *p = s.t;
                                    }
                                    if let Some(p) = sol.start.parameters.get_mut(1) {
                                        *p = s.uv.x;
                                    }
                                    if let Some(p) = sol.start.parameters.get_mut(2) {
                                        *p = s.uv.y;
                                    }
                                }
                            }
                            t = par(&sol.start, 0);
                            uv = Point2::new(par(&sol.start, 1), par(&sol.start, 2));

                            let mut uvs = [Point2::new(0.0, 0.0); 2];
                            *at_mut(&mut uvs, this) = this_uv(t);
                            *at_mut(&mut uvs, other) = uv;

                            // skip start points on surface singularities
                            if surface_is_singularity(k, self.surface(SrfIndex::First), uvs[0])?
                                || surface_is_singularity(
                                    k,
                                    self.surface(SrfIndex::Second),
                                    uvs[1],
                                )?
                            {
                                continue;
                            }

                            if ignored(self.compute_point_values(k, uvs, &mut tsp, None, None))?
                                .is_some()
                            {
                                self.use_boundary_point(
                                    k,
                                    tgt,
                                    &sol,
                                    uvs,
                                    tsp,
                                    other,
                                    &other_domain,
                                    out,
                                    &mut work,
                                )?;
                            }
                        }
                    }
                }
            }
        }

        // Add the surface-edge crossings last (statuses ignored)
        for p in core::mem::take(&mut work.edge_crossings) {
            let id = self.alloc_point(p);
            ignored(self.add_start_point(k, id))?;
        }

        // make degenerate curves of the touching points
        for d in &work.degenerate {
            let tsp = &d.point;
            let degenerate_point = (d.curve_point + tsp.srf_pos(d.surf_index)) / 2.0;

            // skip points that are on a curve or a start point already
            let starts = self.start_points().to_vec();
            if self.is_point_on_curve(k, degenerate_point, &out.curves_3d, None)?
                || self.is_point_on_start_point(tsp, &starts, None)?
            {
                continue;
            }

            // 3D curve, then the UV curves
            let c3 = BSplineCurve::create_degenerate_curve(3, degenerate_point)?;
            out.curves_3d.push(Curve::from(c3));
            let c0 =
                BSplineCurve::create_degenerate_curve(2, Vec3::from(tsp.uv_pos(SrfIndex::First)))?;
            out.uv_curves[0].push(Curve::from(c0));
            let c1 =
                BSplineCurve::create_degenerate_curve(2, Vec3::from(tsp.uv_pos(SrfIndex::Second)))?;
            out.uv_curves[1].push(Curve::from(c1));

            // deviation and type
            let dist = tsp
                .srf_pos(SrfIndex::First)
                .distance(tsp.srf_pos(SrfIndex::Second));
            out.deviations.push(dist);
            out.curve_types.push(TsectCurveType::Touching);
        }
        Ok(())
    }

    /// The tail of a boundary point solution (single value or small range): the
    /// degenerate classification and the start point lists.
    #[allow(clippy::too_many_arguments)]
    fn use_boundary_point(
        &mut self,
        k: &Kernel,
        tgt: &Curve,
        sol: &Solution<()>,
        uvs: [Point2; 2],
        mut tsp: TsectPnt,
        other: SrfIndex,
        other_domain: &Extent2d,
        out: &TsectCurves,
        work: &mut BoundaryWork,
    ) -> Result<()> {
        // boundary curve point (status ignored: an uninitialised point)
        let pt = ignored(tgt.evaluate(k, par(&sol.start, 0), 1, true, true))?
            .and_then(|v| v.first().copied())
            .unwrap_or(Vec3::UNDEF);

        // Set bIsDegenerate = TRUE: a degenerate point xsect curve (status ignored)
        let is_degenerate =
            ignored(self.classify_boundary_start_point_intersection(k, &tsp))?.unwrap_or(false);

        // when a degenerate point not on an output curve yet: classify it at the end
        if is_degenerate && !self.is_point_on_curve(k, tsp.crv_pos(), &out.curves_3d, None)? {
            work.degenerate.push(DegeneratePoint {
                point: tsp,
                curve_point: pt,
                surf_index: other,
            });
            return Ok(());
        }

        // when the point is not on an output curve already
        if !self.is_point_on_curve(k, tsp.crv_pos(), &out.curves_3d, None)? {
            tsp.deviation = sol.start.solution_value;
            if other_domain
                .is_point_on_boundary(*at(&uvs, other), EFF_ZERO * 100.0)
                .0
            {
                // a crossing on the other surface's edge: added last
                work.edge_crossings.push(tsp);
            } else {
                // add the point to the list of start points (status ignored)
                let id = self.alloc_point(tsp);
                ignored(self.add_start_point(k, id))?;
            }
        }
        Ok(())
    }

    /// A long range solution of a boundary curve: the curve trimmed to it, dropped
    /// onto both surfaces, becomes an output curve when close to both.
    #[allow(clippy::too_many_arguments)]
    fn add_boundary_curve<O: Copy + PartialEq>(
        &mut self,
        k: &Kernel,
        caches: &[SurfaceCache<O>],
        tgt: &Curve,
        sol: &Solution<()>,
        l_side: usize,
        out: &mut TsectCurves,
        work: &mut BoundaryWork,
    ) -> Result<()> {
        let tol = self.this_approx_tol_3d;

        // copy and trim the boundary curve (may snap the interval to knots)
        // (`new SmBSplineCurve(*pTgtBSC)`: a plain B-spline copy)
        let mut result = crate::surface::surface_cache::p1::copy_as_bspline(tgt)?;
        let mut trim_ivl = Extent1d::from_min_max(par(&sol.start, 0), par(&sol.end, 0));
        result.trim(k, &mut trim_ivl, TrimMode::Notify)?;
        out.curves_3d.push(result);

        // Get UVCurves on surface 1
        let drop1 = {
            let c = last_curve(out)?;
            let ivl = c.natural_interval();
            surface_drop_and_trim_curve(
                k,
                self.surface(SrfIndex::First),
                caches.first(),
                &self.uv_domains[0],
                c,
                &ivl,
                tol,
            )?
        };
        match drop1.uv_curves.len() {
            2 => out.uv_curves[0].extend(drop1.uv_curves.get(l_side).cloned()),
            1 => out.uv_curves[0].extend(drop1.uv_curves.first().cloned()),
            _ => {
                out.curves_3d.pop();
                return Ok(());
            }
        }

        // Trim the 3D curve to the UV curve's interval
        let uv_ivl = out.uv_curves[0]
            .last()
            .map(Curve::natural_interval)
            .unwrap_or_default();
        if !trim_ivl.is_contained_by(&uv_ivl, 0.0) {
            trim_ivl = uv_ivl;
            last_curve(out)?.trim(k, &mut trim_ivl, TrimMode::Notify)?;
        }

        // Get UVCurves on surface 2
        let drop2 = {
            let c = last_curve(out)?;
            let ivl = c.natural_interval();
            surface_drop_and_trim_curve(
                k,
                self.surface(SrfIndex::Second),
                caches.get(1.min(caches.len().saturating_sub(1))),
                &self.uv_domains[1],
                c,
                &ivl,
                tol,
            )?
        };
        match drop2.uv_curves.len() {
            2 => out.uv_curves[1].extend(drop2.uv_curves.get(l_side).cloned()),
            1 => out.uv_curves[1].extend(drop2.uv_curves.first().cloned()),
            _ => {
                out.curves_3d.pop();
                out.uv_curves[0].pop();
                return Ok(());
            }
        }

        // Trim the 3D curve and the first UV curve to the second UV curve's interval
        let uv_ivl2 = out.uv_curves[1]
            .last()
            .map(Curve::natural_interval)
            .unwrap_or_default();
        if !trim_ivl.is_contained_by(&uv_ivl2, 0.0) {
            trim_ivl = uv_ivl2;
            last_curve(out)?.trim(k, &mut trim_ivl, TrimMode::Notify)?;
            if let Some(c) = out.uv_curves[0].last_mut() {
                c.trim(k, &mut trim_ivl, TrimMode::Notify)?;
            }
        }

        if drop1.dist_to_surf > tol || drop2.dist_to_surf > tol {
            // don't use this curve
            out.uv_curves[0].pop();
            out.uv_curves[1].pop();
            out.curves_3d.pop();
            return Ok(());
        }

        // set the curve deviation and type
        out.deviations
            .push(max2(drop1.dist_to_surf, drop2.dist_to_surf));
        let curve_type = match (out.uv_curves[0].last(), out.uv_curves[1].last()) {
            (Some(c0), Some(c1)) => compute_curve_type(
                k,
                self.surface(SrfIndex::First),
                self.surface(SrfIndex::Second),
                c0,
                c1,
            )?,
            _ => return Err(SolidError::invalid_input(OP_FBSP, "no uv curve")),
        };
        if matches!(
            curve_type,
            TsectCurveType::Coincident | TsectCurveType::RegionBoundary
        ) {
            self.check_for_interior_curves = false;
        }
        out.curve_types.push(curve_type);

        // remove start points on the new curve
        for id in self.start_points().to_vec() {
            let p = self.point(id)?.crv_pos();
            if self.is_point_on_curve(k, p, &out.curves_3d, None)? {
                unlink(&mut self.start_points, id);
            }
        }

        // remove edge crossings on the new curve
        let mut kept = Vec::with_capacity(work.edge_crossings.len());
        for p in core::mem::take(&mut work.edge_crossings) {
            if !self.is_point_on_curve(k, p.crv_pos(), &out.curves_3d, None)? {
                kept.push(p);
            }
        }
        work.edge_crossings = kept;
        Ok(())
    }

    // ---- tracing step --------------------------------------------------------------------

    /// Steps from the point `prev` by `step_size` along its UV tangents to the next
    /// intersection point, evaluated into `next` (`ComputeNextPoint`).
    ///
    /// The step is clipped to both domains (`clipped`); a zero step is a boundary
    /// hit. The guesses are refined ([`SurfaceIntersector::refine_point`] with
    /// boundary testing, then without when the step collapses below a hundredth),
    /// the step scaled by the refinement's UV step on the first surface. A failed
    /// evaluation is a boundary hit; a tangent or coincident next point, or one behind
    /// `prev` on the first surface's UV tangent line (a singular one becomes a
    /// crossing), is not good. Else the curve parameter advances by the 3D distance
    /// and [`SurfaceIntersector::test_span_accuracy`] decides.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `prev` is outside a domain (by `SM_EFF_ZERO`); `InvalidInput`
    /// when `next` is `prev` or an id is unknown; those of the span test;
    /// [`SolidError::Unported`] from unported callees.
    pub fn compute_next_point(
        &mut self,
        k: &Kernel,
        prev: TsectId,
        step_size: f64,
        next: TsectId,
    ) -> Result<NextPoint> {
        if prev == next {
            return Err(SolidError::invalid_input(
                OP_NEXT,
                "the next point is the previous point",
            ));
        }
        let prev_tsp = self.point(prev)?.clone();
        self.point(next)?;
        let s0 = SrfIndex::First;
        let s1 = SrfIndex::Second;

        // init output
        let mut res = NextPoint {
            found_good_point: false,
            deviation_found: None,
            angle_found_rad: None,
            clipped: false,
            boundary_hit: false,
        };

        // check state - both surfaces contain rPrevTSP
        if !self.uv_domains[0].contains_point(prev_tsp.uv_pos(s0), EFF_ZERO)
            || !self.uv_domains[1].contains_point(prev_tsp.uv_pos(s1), EFF_ZERO)
        {
            return Err(SolidError::sm_err(OP_NEXT));
        }

        // if needed: clip stepsize to keep nextTSP in both surfaces
        let unclipped_step_size = step_size;
        let mut step_size =
            self.uv_domains[0].clip_line(prev_tsp.uv_pos(s0), prev_tsp.uv_deriv(s0), step_size);
        step_size =
            self.uv_domains[1].clip_line(prev_tsp.uv_pos(s1), prev_tsp.uv_deriv(s1), step_size);

        // remember clipping when it happens
        res.clipped = step_size < unclipped_step_size - EFF_ZERO * (1.0 + unclipped_step_size);

        // when StepSize is clipped to zero - return a boundary hit with no next point
        if is_zero(step_size) {
            res.boundary_hit = true;
            return Ok(res);
        }

        // Set NextStep UVs for Surface0 and Surface1
        let guess_uv0 = prev_tsp.uv_pos(s0) + prev_tsp.uv_deriv(s0) * step_size;
        let guess_uv1 = prev_tsp.uv_pos(s1) + prev_tsp.uv_deriv(s1) * step_size;

        // this safety step should no longer fire since StepSize has already been clipped
        let guess_uv0 = self.uv_domains[0].clamp_point(guess_uv0);
        let guess_uv1 = self.uv_domains[1].clamp_point(guess_uv1);

        // the previous point's m_vUVCurvePV, as RefinePoint may read it
        let previous_uv = [
            prev_tsp.uv_pos(s0),
            prev_tsp.uv_deriv(s0),
            prev_tsp.uv_pos(s1),
            prev_tsp.uv_deriv(s1),
        ];

        // Find surf/surf xSectPoint near the guess points (a failure is no answer)
        let refine = |do_boundary_testing: bool| -> Result<Option<[Point2; 2]>> {
            match self.refine_point(
                k,
                guess_uv0,
                guess_uv1,
                prev_tsp.point_type,
                Some(prev_tsp.crv_deriv()),
                Some(&previous_uv),
                do_boundary_testing,
            ) {
                Ok(r) => Ok(r),
                Err(e) if e.is_unported() => Err(e),
                Err(_) => Ok(None),
            }
        };
        let Some(mut next_uvs) = refine(true)? else {
            return Ok(res);
        };

        // compute actual step to guess step
        let guess_step = guess_uv0 - prev_tsp.uv_pos(s0);
        let actual_step = next_uvs[0] - prev_tsp.uv_pos(s0);
        let mut guess_step_size = guess_step.length();
        let mut actual_step_size = actual_step.length();
        if guess_step_size > EFF_ZERO * actual_step_size {
            step_size *= actual_step_size / guess_step_size;
        }

        // Step size may go to zero again so test it again.
        if is_zero(step_size) || actual_step_size < guess_step_size / 100.0 {
            // Before bailing out try local intersection again without boundary
            // testing.
            let Some(uvs) = refine(false)? else {
                return Ok(res);
            };
            next_uvs = uvs;

            // update StepSize for this refined point
            let guess_step = guess_uv0 - prev_tsp.uv_pos(s0);
            let actual_step = next_uvs[0] - prev_tsp.uv_pos(s0);
            guess_step_size = guess_step.length();
            actual_step_size = actual_step.length();
            if guess_step_size > EFF_ZERO * actual_step_size {
                step_size *= actual_step_size / guess_step_size;
            }

            // if this 2nd refined point also ends up with a zero stepSize - quit
            if is_zero(step_size) {
                res.boundary_hit = true;
                return Ok(res);
            }
        }

        // set NextTSP values given its surface UV values (a next point: with the
        // previous point)
        let mut next_tsp = self.point(next)?.clone();
        let r =
            self.compute_point_values(k, next_uvs, &mut next_tsp, Some(&prev_tsp), Some(step_size));
        *self.point_mut(next)? = next_tsp.clone();
        if ignored(r)?.is_none() {
            res.boundary_hit = true; // causes termination of current tracing
            return Ok(res);
        }

        // end iteration for tangent and coincident points
        if matches!(
            next_tsp.point_type,
            IntersectionPointType::TangentPoint | IntersectionPointType::Coincidence
        ) {
            return Ok(res);
        }

        // Also make sure that the point is going in the same direction
        let line_param = line_closest_point(
            Vec3::from(prev_tsp.uv_pos(s0)),
            Vec3::from(prev_tsp.uv_deriv(s0)),
            Vec3::from(next_tsp.uv_pos(s0)),
        );
        // when next point lies behind TsectPnt
        if line_param < 0.0 {
            // mark NextTSP as SM_IP_CROSSING and quit
            if next_tsp.point_type == IntersectionPointType::Singularity {
                self.point_mut(next)?.point_type = IntersectionPointType::Crossing;
            }
            return Ok(res);
        }

        // let NextTSP param = TsectPnt.Param + 3DCurve ArcLength
        let dist_moved = next_tsp.crv_pos().distance(prev_tsp.crv_pos());
        self.point_mut(next)?.curve_parameter = prev_tsp.curve_parameter + dist_moved;

        // the angle and deviation tests of the span
        let acc = self.test_span_accuracy(k, prev, next)?;
        res.found_good_point = acc.found_good_point;
        res.deviation_found = acc.deviation_found;
        res.angle_found_rad = Some(acc.angle_found_rad);
        Ok(res)
    }

    // ---- end-of-trace tests ----------------------------------------------------------------

    /// Trims the traced span from `prev` to `next` to the point `new`
    /// (`sm_UpdateTsectPoint`): `next` takes `new`'s user values and is evaluated at
    /// `new`'s UV points (as a next point of `prev`); its curve parameter is scaled
    /// with the chord length; `new` takes that parameter when its own is negative.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `next` is `prev` or an id is unknown; those of
    /// [`SurfaceIntersector::compute_point_values`] (`next` keeps what was computed).
    pub fn update_tsect_point(
        &mut self,
        k: &Kernel,
        prev: TsectId,
        next: TsectId,
        new: TsectId,
    ) -> Result<()> {
        update_tsect_point_with(self, k, prev, next, new)
    }

    /// Tests the next traced point `next` against the targets of the trace and the
    /// domain boundaries (`TestPoint`), snapping it
    /// ([`SurfaceIntersector::update_tsect_point`]) to the first target on the span
    /// from the last curve point to `next` ([`is_point_on_tsect_curve`]):
    ///
    /// 1. the first through point: it is removed (the last one sets
    ///    `last_through_parameter` when extension clipping), `clipped_by_through_point`
    ///    is set, and the done flag is left unchanged (`None`);
    /// 2. a start point, newest first (skipped when the last curve point is the start
    ///    of the trace, or is at the start point without crossing a seam): a crossing
    ///    start point with a non-parallel tangent plane is removed; the trace is done
    ///    unless the start point is a crossing on a domain boundary the curve runs
    ///    along;
    /// 3. the start of the trace (a closed curve, at least two spans on);
    /// 4. the end point (unless the span starts the curve at it, a seam).
    ///
    /// Without an end point, extension clipping ends the trace past
    /// `last_through_parameter + extension_distance`. Else the trace is done when
    /// `next` is on a domain boundary with its UV tangent pointing across it; the
    /// start points at `next` are then removed.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `next` is the last curve point or an id is unknown, or
    /// when the curve list is empty and the last curve point is needed (upstream
    /// dereferences null); those of the snapping; [`SolidError::Unported`] from
    /// unported callees.
    pub fn test_point(&mut self, k: &Kernel, next: TsectId) -> Result<Option<bool>> {
        test_point_with(self, k, next)
    }

    // ---- fillet law spans ------------------------------------------------------------------

    /// Splits the fillet-law span from `curr` to `next` (`SplitSpan`): a new crossing
    /// point at the mean law parameter (`user_doubles[1]`), UV points and curve
    /// parameter, evaluated by [`SurfaceIntersector::compute_law_point_values`]; its
    /// curve parameter becomes `curr`'s plus the chord when the chord exceeds
    /// `SM_ZONE_TOL_3D` times the law parameter step. Returns the new point's id.
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceIntersector::compute_law_point_values`] (the basic
    /// intersector's `ComputePointValuesDelta` always fails); `InvalidInput` for an
    /// unknown id.
    pub fn split_span(
        &mut self,
        k: &Kernel,
        curr: TsectId,
        next: TsectId,
        delta_step: f64,
        law: &LawPointOptions<'_>,
        law_curve: &mut FilletLaw,
    ) -> Result<TsectId> {
        split_span_with(self, k, curr, next, delta_step, law, law_curve)
    }
}

/// [`SurfaceIntersector::split_span`] on the intersector of `v`, the virtual calls of
/// `ComputeLawPointValues` dispatched to `v`.
///
/// # Errors
///
/// As [`SurfaceIntersector::split_span`], with the errors of `v`'s overrides.
pub fn split_span_with<'s, V: super::p4::IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    curr: TsectId,
    next: TsectId,
    delta_step: f64,
    law: &LawPointOptions<'_>,
    law_curve: &mut FilletLaw,
) -> Result<TsectId> {
    let curr_tsp = v.intersector().point(curr)?.clone();
    let next_tsp = v.intersector().point(next)?.clone();

    let mut new_tsp = TsectPnt::zeroed();
    new_tsp.point_type = IntersectionPointType::Crossing;

    // the law parameters of the span ends and their mean
    let curr_param = curr_tsp.user_doubles[1];
    let next_param = next_tsp.user_doubles[1];
    let mid_param = (curr_param + next_param) / 2.0;
    new_tsp.user_doubles[1] = mid_param;

    let uv0 = (curr_tsp.uv_pos(SrfIndex::First) + next_tsp.uv_pos(SrfIndex::First)) / 2.0;
    let uv1 = (curr_tsp.uv_pos(SrfIndex::Second) + next_tsp.uv_pos(SrfIndex::Second)) / 2.0;

    new_tsp.curve_parameter = (curr_tsp.curve_parameter + next_tsp.curve_parameter) / 2.0;

    super::p2::compute_law_point_values_with(
        v,
        k,
        mid_param,
        super::p2::LawGuess::Given([uv0, uv1]),
        delta_step,
        law,
        law_curve,
        false,
        &mut new_tsp,
    )?;

    let dist = curr_tsp.crv_pos().distance(new_tsp.crv_pos());
    if dist > ZONE_TOL_3D * (next_param - curr_param) {
        new_tsp.curve_parameter = curr_tsp.curve_parameter + dist;
    }
    Ok(v.intersector_mut().alloc_point(new_tsp))
}

/// [`SurfaceIntersector::update_tsect_point`] on the intersector of `v`, its
/// `ComputePointValues` dispatched to `v` (`sm_UpdateTsectPoint` calls the virtual).
///
/// # Errors
///
/// As [`SurfaceIntersector::update_tsect_point`].
pub fn update_tsect_point_with<'s, V: super::p4::IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    prev: TsectId,
    next: TsectId,
    new: TsectId,
) -> Result<()> {
    if prev == next {
        return Err(SolidError::invalid_input(
            OP_UPDATE,
            "the next point is the previous point",
        ));
    }
    let prev_tsp = v.intersector().point(prev)?.clone();
    let new_tsp = v.intersector().point(new)?.clone();
    let mut next_tsp = v.intersector().point(next)?.clone();

    // get pPrev1/rNextTSP 3DCurve arcLength distance
    let dist = prev_tsp.crv_pos().distance(next_tsp.crv_pos());

    // get parameter Step between pPrev1 and rNextTSP
    let step = next_tsp.curve_parameter - prev_tsp.curve_parameter;

    // get NewTSP UV values
    let uvs = [
        new_tsp.uv_pos(SrfIndex::First),
        new_tsp.uv_pos(SrfIndex::Second),
    ];

    // copy rNextTSP user values from pNewTSP
    next_tsp.user_doubles = new_tsp.user_doubles;
    next_tsp.user_pointer.clone_from(&new_tsp.user_pointer);

    // ComputePointValues with pNewTSP UV values and store results in rNextTSP
    let r = v.compute_point_values(k, uvs, &mut next_tsp, Some(&prev_tsp), None);
    *v.intersector_mut().point_mut(next)? = next_tsp.clone();
    r?;

    // set parameter step size based on current curve parameterization
    let new_dist = prev_tsp.crv_pos().distance(next_tsp.crv_pos());
    let new_step = new_dist / dist * step;
    let next_param = prev_tsp.curve_parameter + new_step;
    v.intersector_mut().point_mut(next)?.curve_parameter = next_param;

    // set pNewTSP parameter value so that it knows if/where it was used
    let p = v.intersector_mut().point_mut(new)?;
    if p.curve_parameter < 0.0 {
        p.curve_parameter = next_param;
    }
    Ok(())
}

/// [`SurfaceIntersector::test_point`] on the intersector of `v`, the snapping
/// ([`update_tsect_point_with`]) dispatched to `v`.
///
/// # Errors
///
/// As [`SurfaceIntersector::test_point`].
#[allow(clippy::too_many_lines)]
pub fn test_point_with<'s, V: super::p4::IntersectorVirtuals<'s> + ?Sized>(
    v: &mut V,
    k: &Kernel,
    next: TsectId,
) -> Result<Option<bool>> {
    v.intersector().point(next)?;
    let prev1 = v.intersector().curve_points.last().copied();
    if prev1 == Some(next) {
        return Err(SolidError::invalid_input(
            OP_TEST,
            "the next point is the last curve point",
        ));
    }
    let need_prev = || prev1.ok_or_else(|| SolidError::invalid_input(OP_TEST, "no curve point"));
    let tol = v.intersector().this_approx_tol_3d;

    // If we are given through points check the first one: they are in order
    if let Some(&through) = v.intersector().through_points.first() {
        let p1 = need_prev()?;

        // if through point is on hermite curve from pPrev1 point to rNextTSP point
        if is_point_on_tsect_curve(
            k,
            v.intersector().point(through)?,
            v.intersector().point(p1)?,
            v.intersector().point(next)?,
            tol,
        )? {
            // modify the rNextTSP to correspond to the through point
            update_tsect_point_with(v, k, p1, next, through)?;

            // remove the through point from the list - we're done with it
            unlink(&mut v.intersector_mut().through_points, through);

            // mark the last throughParameter distance when extension clipping
            if v.intersector().doing_extension_clipping && v.intersector().through_points.is_empty()
            {
                let param = v.intersector().point(through)?.curve_parameter;
                v.intersector_mut().last_through_parameter = param;
            }

            // remember that a through point was hit
            v.intersector_mut().clipped_by_through_point = true;
            return Ok(None);
        }
    }

    // Check against Start Points, the better ones (at the end) first.
    let starts = v.intersector().start_points.clone();
    for &start in starts.iter().rev() {
        // Skip pPrev1 if it's the same as m_pStartPoint.
        if prev1 == v.intersector().start_point {
            continue;
        }
        let p1 = need_prev()?;

        // Also skip it if it's at the same place as pStartTSP, unless a seam
        {
            let prev_tsp = v.intersector().point(p1)?;
            let start_tsp = v.intersector().point(start)?;
            let dist = prev_tsp.crv_pos().distance(start_tsp.crv_pos());
            if dist < tol {
                let dom1_size = v.intersector().uv_domains[0].size().length();
                let dom2_size = v.intersector().uv_domains[1].size().length();
                let dist1 = prev_tsp
                    .uv_pos(SrfIndex::First)
                    .distance(start_tsp.uv_pos(SrfIndex::First));
                let dist2 = prev_tsp
                    .uv_pos(SrfIndex::Second)
                    .distance(start_tsp.uv_pos(SrfIndex::Second));
                if dist1 < dom1_size / 2.0 && dist2 < dom2_size / 2.0 {
                    continue;
                }
            }
        }

        // if StartTsp is on xsect curve segment
        if is_point_on_tsect_curve(
            k,
            v.intersector().point(start)?,
            v.intersector().point(p1)?,
            v.intersector().point(next)?,
            tol,
        )? {
            // Modify the rNextTSP to correspond to that start point
            update_tsect_point_with(v, k, p1, next, start)?;

            // Remove the start point we just crossed (not singular or tangent ones)
            let next_type = v.intersector().point(next)?.point_type;
            let start_tsp = v.intersector().point(start)?.clone();
            if next_type == IntersectionPointType::Crossing
                && start_tsp.tangent_plane_angle_rad > v.intersector().stepping_parallel_tol_radians
            {
                unlink(&mut v.intersector_mut().start_points, start);
            }

            // a crossing start point on a boundary the curve runs along is a
            // through point
            let done = if start_tsp.point_type == IntersectionPointType::Crossing {
                let along = |on: ExtentPointType, across: ExtentPointType, d: f64| {
                    (on == ExtentPointType::Start || on == ExtentPointType::End)
                        && across == ExtentPointType::Inside
                        && is_zero(d)
                };
                let u0 = along(
                    start_tsp.u_param_type(SrfIndex::First),
                    start_tsp.v_param_type(SrfIndex::First),
                    start_tsp.uv_deriv(SrfIndex::First).x,
                );
                let u1 = along(
                    start_tsp.u_param_type(SrfIndex::Second),
                    start_tsp.v_param_type(SrfIndex::Second),
                    start_tsp.uv_deriv(SrfIndex::Second).x,
                );
                let v0 = along(
                    start_tsp.v_param_type(SrfIndex::First),
                    start_tsp.u_param_type(SrfIndex::First),
                    start_tsp.uv_deriv(SrfIndex::First).y,
                );
                let v1 = along(
                    start_tsp.v_param_type(SrfIndex::Second),
                    start_tsp.u_param_type(SrfIndex::Second),
                    start_tsp.uv_deriv(SrfIndex::Second).y,
                );
                !(u0 || u1 || v0 || v1)
            } else {
                true
            };
            return Ok(Some(done));
        }
    }

    // If we have a start point check for a closed curve situation
    if let Some(sp) = v.intersector().start_point {
        // Don't check if we are only starting the curve
        if prev1 != Some(sp) {
            let p1 = need_prev()?;
            // the node before pPrev1 (circular list: itself when alone)
            let n = v.intersector().curve_points.len();
            let prev2 = v
                .intersector()
                .curve_points
                .get(n.saturating_sub(2))
                .copied();
            if prev2 != Some(sp)
                && is_point_on_tsect_curve(
                    k,
                    v.intersector().point(sp)?,
                    v.intersector().point(p1)?,
                    v.intersector().point(next)?,
                    tol,
                )?
            {
                // Have found loop around
                update_tsect_point_with(v, k, p1, next, sp)?;
                v.intersector_mut().curve_is_closed = true;
                return Ok(Some(true));
            }
        }
    }

    // If we are given an ending point - check it
    if let Some(ep) = v.intersector().end_point {
        let p1 = need_prev()?;
        if is_point_on_tsect_curve(
            k,
            v.intersector().point(ep)?,
            v.intersector().point(p1)?,
            v.intersector().point(next)?,
            tol,
        )? {
            let mut on_seam = false;
            if v.intersector().curve_points.first() == Some(&p1) {
                // a single span: the end point at its start is a seam
                let dist1 = v
                    .intersector()
                    .point(p1)?
                    .crv_pos()
                    .distance(v.intersector().point(ep)?.crv_pos());
                if dist1 < tol {
                    on_seam = true;
                }
            }
            if !on_seam {
                update_tsect_point_with(v, k, p1, next, ep)?;
                return Ok(Some(true));
            }
        }
    } else if v.intersector().doing_extension_clipping
        && v.intersector().last_through_parameter != 0.0
        && v.intersector().point(next)?.curve_parameter
            > v.intersector().last_through_parameter + v.intersector().extension_distance
    {
        return Ok(Some(true));
    }

    // Check whether the point is inside of the domain of the surfaces.
    let next_tsp = v.intersector().point(next)?.clone();
    let mut on_boundary = false;
    for s in [SrfIndex::First, SrfIndex::Second] {
        // let tol scale to surface domain size
        let domain = at(&v.intersector().uv_domains, s);
        let size = domain.max_dimension();
        let tol = EFF_ZERO * (1.0 + size) * 100.0;
        let ang_tol = EFF_ZERO_SQRT * (1.0 + size) * 10.0;

        // check NextTSP to see if its crossing a boundary
        let (on, bi_norm) = domain.is_point_on_boundary(next_tsp.uv_pos(s), tol);
        let d = next_tsp.uv_deriv(s);
        if on
            && ((bi_norm.x != 0.0 && d.x.abs() > ang_tol)
                || (bi_norm.y != 0.0 && d.y.abs() > ang_tol))
        {
            on_boundary = true;
        }

        // done when point is on and heading out of any boundary
        if on_boundary {
            break;
        }
    }

    // mark points not crossing boundaries as not done and return
    if !on_boundary {
        return Ok(Some(false));
    }

    // else this point is crossing a boundary: remove the start points at it
    for id in v.intersector().start_points.clone() {
        let tsp = v.intersector().point(id)?;
        let mut same = true;
        for s in [SrfIndex::First, SrfIndex::Second] {
            let size = at(&v.intersector().uv_domains, s).max_dimension();
            let tol = EFF_ZERO * (1.0 + size) * 100.0;
            let dist = tsp.uv_pos(s).distance_squared(next_tsp.uv_pos(s));
            if dist > tol {
                same = false;
                break;
            }
        }
        if same {
            unlink(&mut v.intersector_mut().start_points, id);
        }
    }
    Ok(Some(true))
}

// ---- callees ----------------------------------------------------------------------------------

/// `surface.IsSingularity(uv, eSingDir)` (default tolerance): the plane override
/// (never singular); other kinds run `SmSurface::IsSingularity`.
fn surface_is_singularity(k: &Kernel, surface: &Surface, uv: Point2) -> Result<bool> {
    match surface.kind() {
        SurfaceKind::Plane(p) => Ok(p.is_singularity().0),
        SurfaceKind::Cone(_)
        | SurfaceKind::Sphere(_)
        | SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::SurfOfRevolution(_)
        | SurfaceKind::SurfOfExtrusion(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {
            Ok(surface.is_singularity(k, uv, EFF_ZERO, false)?.singular)
        }
    }
}

/// The objects of the solve of [`surface_global_curve_intersect`]: the boundary curve
/// and the surface, carrying the natural interval / domain that the solver's
/// identical-solution test reads through upstream's object pointers
/// (`GetNaturalInterval`, `GetNaturalUVDomain`). The solutions returned name no objects.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SolveObject {
    Curve(Extent1d),
    Surface(Extent2d),
}

impl SolutionObject for SolveObject {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Curve(_) => ObjectKind::Curve,
            Self::Surface(_) => ObjectKind::Surface,
        }
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Curve(ivl) => Some(*ivl),
            Self::Surface(_) => None,
        }
    }

    fn natural_uv_domain(&self) -> Option<Extent2d> {
        match self {
            Self::Curve(_) => None,
            Self::Surface(domain) => Some(*domain),
        }
    }
}

/// The owning-face service of a trimmed cache for [`surface_global_curve_intersect`]'s
/// solve, on its [`SolveObject`] handles: the cache (the surface's, renamed) and the
/// caller's classifications of the cache's face.
struct TrimmedFaceContext<'q, O> {
    cache: SurfaceCache<SolveObject>,
    face: O,
    queries: &'q dyn FaceQueries<O>,
}

impl<O> core::fmt::Debug for TrimmedFaceContext<'_, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TrimmedFaceContext").finish_non_exhaustive()
    }
}

impl<O: Copy + PartialEq> SurfaceSolveContext<SolveObject> for TrimmedFaceContext<'_, O> {
    fn build_surface_cache(
        &mut self,
        _: &Kernel,
        _: &Surface,
        _: Option<SolveObject>,
    ) -> Result<SurfaceCache<SolveObject>> {
        self.cache.map_handles(Ok)
    }

    fn face_queries(&self) -> Option<&dyn FaceQueries<SolveObject>> {
        Some(self)
    }
}

impl<O: Copy + PartialEq> FaceQueries<SolveObject> for TrimmedFaceContext<'_, O> {
    fn face_zone_tol_3d(&self, _: SolveObject) -> Result<f64> {
        self.queries.face_zone_tol_3d(self.face)
    }

    fn face_point_classify(
        &self,
        k: &Kernel,
        _: SolveObject,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<SolveObject>)> {
        // (the classification object does not name a solve object)
        Ok((
            self.queries
                .face_point_classify(k, self.face, uv, src_tol)?
                .0,
            None,
        ))
    }

    fn face_point_classify_fast_drop(
        &self,
        k: &Kernel,
        _: SolveObject,
        uv: Point2,
        src_tol: f64,
    ) -> Result<(i32, Option<SolveObject>)> {
        Ok((
            self.queries
                .face_point_classify_fast_drop(k, self.face, uv, src_tol)?
                .0,
            None,
        ))
    }
}

/// `surface.GlobalCurveIntersect(domain, curve, interval, tol, solutions)`, which
/// upstream runs on the surface's one cache: here on a copy of `cache` (the
/// intersector's cache of `surface`; its handles, which name only the surface, become
/// the [`SolveObject`] of the surface; a cache naming no surface cannot serve the leaf
/// solves, so the solve then builds its own). The solutions, `[t, u, v]` each (their
/// objects and nodes are not kept).
#[allow(clippy::too_many_arguments)]
fn surface_global_curve_intersect<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    cache: &SurfaceCache<O>,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
    queries: Option<&dyn FaceQueries<O>>,
) -> Result<Vec<Solution<()>>> {
    let surface_object = SolveObject::Surface(surface.natural_uv_domain_with_nurb(k)?);
    let curve_object = SolveObject::Curve(curve.natural_interval());
    // a trimmed cache of a face's surface: the solve's point solves classify their
    // fast-drop solutions on that face (GlobalPointSolve -> pFace->PointClassify)
    let face_context = match queries {
        Some(queries) if cache.surface_handle().is_some() && cache.is_trimmed() => cache
            .face()?
            .map(|face| -> Result<TrimmedFaceContext<'_, O>> {
                Ok(TrimmedFaceContext {
                    cache: cache.map_handles(|_| Ok(surface_object))?,
                    face,
                    queries,
                })
            })
            .transpose()?,
        _ => None,
    };
    let found = if let Some(mut face_context) = face_context {
        let mut solve_cache = cache.map_handles(|_| Ok(surface_object))?;
        surface.global_curve_intersect_with_context::<SolveObject>(
            k,
            [Some(curve_object), Some(surface_object)],
            Some(&mut solve_cache),
            &mut face_context,
            domain,
            curve,
            interval,
            tol,
        )?
    } else if cache.surface_handle().is_some() {
        let mut solve_cache = cache.map_handles(|_| Ok(surface_object))?;
        surface.global_curve_intersect_in::<SolveObject>(
            k,
            [Some(curve_object), Some(surface_object)],
            Some(&mut solve_cache),
            domain,
            curve,
            interval,
            tol,
        )?
    } else {
        // a cache that does not name its surface cannot serve the leaf solves
        // (sm_GetOriginalSurface reads the tree's owner): the solve builds its own
        surface.global_curve_intersect::<SolveObject>(
            k,
            [curve_object, surface_object],
            domain,
            curve,
            interval,
            tol,
        )?
    };
    Ok(found
        .as_slice()
        .iter()
        .map(|s| {
            let mut out = Solution::<()>::new();
            out.solution_type = s.solution_type;
            out.num_variables = s.num_variables;
            out.start = s.start;
            out.end = s.end;
            out
        })
        .collect())
}

/// The outputs of `SmSurface::DropAndTrimCurve` used here.
struct DropAndTrim {
    /// `rdDistToSurf`.
    dist_to_surf: f64,
    /// `rUVCurves`.
    uv_curves: Vec<Curve>,
}

/// `surface.DropAndTrimCurve(ctx, domain, curve, interval, tol, dist, deviation,
/// uv_curves, TRUE, TRUE)`. Its trace (`smsurf_GetSurfaceCache` inside `DoTrace`)
/// runs on a copy of `cache`, the surface's cache the intersector holds, as
/// [`surface_global_curve_intersect`] does; with no cache, or one that does not name
/// its surface, the trace builds its own.
fn surface_drop_and_trim_curve<O: Copy + PartialEq>(
    k: &Kernel,
    surface: &Surface,
    cache: Option<&SurfaceCache<O>>,
    domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    tol: f64,
) -> Result<DropAndTrim> {
    let r = match cache {
        Some(c) if c.surface_handle().is_some() => {
            let surface_object = SolveObject::Surface(surface.natural_uv_domain_with_nurb(k)?);
            let mut trace_cache = c.map_handles(|_| Ok(surface_object))?;
            surface.drop_and_trim_curve_in::<ObjectHandle, SolveObject>(
                k,
                Some(&mut trace_cache),
                domain,
                curve,
                interval,
                tol,
                true,
                true,
                None,
            )?
        }
        _ => surface.drop_and_trim_curve::<ObjectHandle>(
            k, domain, curve, interval, tol, true, true, None,
        )?,
    };
    Ok(DropAndTrim {
        dist_to_surf: r.max_drop_to_surf,
        uv_curves: r.uv_curves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector3d::Vec3;
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

    fn two_planes() -> (Surface, Surface) {
        // z = 0 and y = 0.5: they meet along x
        (
            plane([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            plane([0.0, 0.5, -0.5], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        )
    }

    fn intersector<'s>(k: &Kernel, a: &'s Surface, b: &'s Surface) -> SurfaceIntersector<'s> {
        let d = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        SurfaceIntersector::new(k, a, &d, b, &d, false).expect("intersector")
    }

    /// The crossing point of the two planes at `x`.
    fn crossing(si: &SurfaceIntersector<'_>, x: f64, param: f64) -> TsectPnt {
        let mut p = TsectPnt::new();
        si.compute_point_values(
            &Kernel::default(),
            [Point2::new(x, 0.5), Point2::new(x, 0.5)],
            &mut p,
            None,
            None,
        )
        .expect("cpv");
        p.curve_parameter = param;
        p
    }

    #[test]
    fn zeroed_point() {
        let p = TsectPnt::zeroed();
        assert_eq!(p.point_type, IntersectionPointType::Undefined);
        assert_eq!(p.uv_param_type, [[ExtentPointType::Start; 2]; 2]);
        assert_eq!(p.crv_pos(), Vec3::ZERO);
    }

    #[test]
    fn next_point_from_a_tangent_point_type_is_not_refined() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let mut prev = crossing(&si, 0.2, 0.0);
        prev.point_type = IntersectionPointType::Unknown;
        let p = si.alloc_point(prev);
        let n = si.alloc_point(TsectPnt::new());
        // an unknown type is not refined: no good point
        let r = si
            .compute_next_point(&Kernel::default(), p, 0.1, n)
            .expect("next");
        assert!(!r.found_good_point);
        assert!(!r.boundary_hit);
        assert!(si
            .compute_next_point(&Kernel::default(), p, 0.1, p)
            .is_err());
    }

    #[test]
    fn next_point_from_a_tangent_point_back_to_the_corner_is_good() {
        let k = &Kernel::default();
        // z = 0 and y = 0 meet along x; both planes' corner (0, 0) is the origin
        let a = plane([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let b = plane([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let mut si = intersector(k, &a, &b);
        let mut prev = TsectPnt::new();
        si.compute_point_values(
            &Kernel::default(),
            [Point2::new(0.5, 0.0), Point2::new(0.5, 0.0)],
            &mut prev,
            None,
            None,
        )
        .expect("cpv");
        prev.point_type = IntersectionPointType::TangentPoint;
        prev.uv_curve_pv[0][1] = Vec2::new(-1.0, 0.0);
        prev.uv_curve_pv[1][1] = Vec2::new(-1.0, 0.0);
        prev.curve_pv[1] = Vec3::new(-1.0, 0.0, 0.0);
        let p = si.alloc_point(prev);
        let n = si.alloc_point(TsectPnt::new());
        // a tangent point is not refined: the next point is the corner
        let r = si
            .compute_next_point(&Kernel::default(), p, 0.1, n)
            .expect("next");
        assert!(r.found_good_point, "{r:?}");
        let next = si.point(n).expect("next");
        assert!(next.crv_pos().length() < 1e-12);
        assert!((next.curve_parameter - 0.5).abs() < 1e-12);
    }

    #[test]
    fn next_point_outside_the_domain_is_sm_err() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let mut prev = crossing(&si, 0.2, 0.0);
        prev.uv_curve_pv[0][0] = Point2::new(2.0, 0.5);
        let p = si.alloc_point(prev);
        let n = si.alloc_point(TsectPnt::new());
        assert!(si
            .compute_next_point(&Kernel::default(), p, 0.1, n)
            .is_err());
    }

    #[test]
    fn next_point_clipped_to_zero_is_a_boundary_hit() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let mut prev = crossing(&si, 1.0, 0.0);
        // heading out of both domains
        prev.uv_curve_pv[0][1] = Vec2::new(1.0, 0.0);
        prev.uv_curve_pv[1][1] = Vec2::new(1.0, 0.0);
        let p = si.alloc_point(prev);
        let n = si.alloc_point(TsectPnt::new());
        let r = si
            .compute_next_point(&Kernel::default(), p, 0.1, n)
            .expect("next");
        assert!(r.boundary_hit);
        assert!(r.clipped);
    }

    #[test]
    fn test_point_snaps_to_end_point() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let p0 = si.alloc_point(crossing(&si, 0.2, 0.0));
        let end = si.alloc_point(crossing(&si, 0.5, -1.0));
        let next = si.alloc_point(crossing(&si, 0.6, 0.4));
        si.add_point_to_curve(p0).expect("curve");
        si.end_point = Some(end);
        let done = si.test_point(&Kernel::default(), next).expect("test");
        assert_eq!(done, Some(true));
        let n = si.point(next).expect("next");
        assert!((n.crv_pos().x - 0.5).abs() < 1e-12);
        // the end point learns its parameter
        assert!(si.point(end).expect("end").curve_parameter >= 0.0);
    }

    #[test]
    fn test_point_through_point_leaves_done_unset() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let p0 = si.alloc_point(crossing(&si, 0.2, 0.0));
        let th = si.alloc_point(crossing(&si, 0.4, -1.0));
        let next = si.alloc_point(crossing(&si, 0.6, 0.4));
        si.add_point_to_curve(p0).expect("curve");
        si.add_through_point(th).expect("through");
        assert_eq!(si.test_point(&Kernel::default(), next).expect("test"), None);
        assert!(si.through_points().is_empty());
        assert!(si.clipped_by_through_point);
    }

    #[test]
    fn test_point_needs_a_previous_point() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let next = si.alloc_point(crossing(&si, 0.6, 0.4));
        let end = si.alloc_point(crossing(&si, 0.5, 0.0));
        // nothing to test against: inside the domain, not done
        assert_eq!(
            si.test_point(&Kernel::default(), next).expect("test"),
            Some(false)
        );
        si.end_point = Some(end);
        assert!(si.test_point(&Kernel::default(), next).is_err());
    }

    #[test]
    fn test_point_on_boundary_is_done() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let p0 = si.alloc_point(crossing(&si, 0.8, 0.0));
        let s = si.alloc_point(crossing(&si, 1.0, 0.0));
        si.add_point_to_curve(p0).expect("curve");
        let next = si.alloc_point(crossing(&si, 1.0, 0.2));
        si.add_start_point(k, s).expect("start");
        assert_eq!(
            si.test_point(&Kernel::default(), next).expect("test"),
            Some(true)
        );
        assert!(si.start_points().is_empty());
    }

    #[test]
    fn update_rejects_alias() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        let p0 = si.alloc_point(crossing(&si, 0.2, 0.0));
        assert!(si
            .update_tsect_point(&Kernel::default(), p0, p0, p0)
            .is_err());
    }

    #[test]
    fn find_boundary_start_points_with_a_far_box_finds_nothing() {
        let k = &Kernel::default();
        let (a, b) = two_planes();
        let mut si = intersector(k, &a, &b);
        si.set_use_surface_edges(false, false);
        let k = Kernel::default();
        let mut out = TsectCurves::default();
        let far = Extent3d::from_bounds(50.0, 50.0, 50.0, 51.0, 51.0, 51.0);
        // (the surface cache tree build is not ported: no fetched cache)
        let mut caches = vec![SurfaceCache::<()>::new(
            a.clone(),
            a.natural_uv_domain_if_built(),
            crate::surface::surface_cache::SurfaceCacheOptions::default(),
        )];
        si.find_boundary_start_points_with_caches(&k, &mut caches, &far, &mut out)
            .expect("fbsp");
        assert!(si
            .find_boundary_start_points_with_caches::<()>(&k, &mut [], &far, &mut out)
            .is_err());
        assert!(out.curves_3d.is_empty());
        assert!(si.start_points().is_empty());
    }
}
