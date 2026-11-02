//! Curve classification against 3D edges and trimmed face boundaries.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`CurveClassification::intersect_edges`] | `SmCurveClassification::IntersectEdges` |
//! | [`CurveClassification::intersect_edges_mut`] | `SmCurveClassification::IntersectEdges` over a mutable adapter |
//! | [`CurveClassification::intersect_ts_bound`] | `SmCurveClassification::IntersectTSBound` |
//!
//! Deviations: topology is supplied as borrowed views resolved from arena ids, and
//! the caller supplies the face's trim cache (in its own handle space, mapped to the
//! caller's by [`BoundaryFace::cache_handle`]). The cached branch claims and locks a
//! context mark as upstream's `SmNewMarkAndLock` does ([`crate::geom::context::Context::claim_mark`]);
//! the vertex uses and edge uses it marks are a local ordered list of visited
//! handles. No input-driven recursion is added. Temporary surface-owner
//! clearing is represented by a private copy for a lifted 2D source. Concrete
//! curve-on-surface dispatch is preserved; [`CurveClassification::intersect_edges`]
//! inserts through a read-only adapter (its mutable edits defer) and
//! [`CurveClassification::intersect_edges_mut`] through a mutable one. Each target
//! edge's curve cache is built at that edge's live zone tolerance, as upstream's
//! `SmTol::GetApproxTol3d(&m_crCurve)` reads it from the curve's owning edge; the
//! classification curve's cache has no owner in reach through the caller's geometry and
//! still builds on the system tolerance. Ignored native evaluation
//! and drop statuses preserve initialized outputs; unported errors propagate. The
//! native checked-drop guess aliases its output and is reset to zero before solving;
//! the Rust call explicitly uses that zero guess. A 3D trim-boundary source returns
//! `InvalidInput` instead of upstream's debug-only assertion.

use super::p1::{cull_or_refine_cci_in_3d, src_zone_tol_3d_of};
use super::p5::{insert_solutions, insert_solutions_mut, ClassGeometryMut};
use super::CurveClassification;
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p5::make_curve_cache;
use crate::curve::curve::p6::DroppedPoint;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::curve::curve_cache::CurveCache;
use crate::error::{ignored, Result, SolidError};
use crate::geom::context::ALL_MARKS;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, SolutionType};
use crate::geom::tol::{xsect_tol3d, xsect_tol_3d_from_zones};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, min2, Point2, Point3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::surface::surface::p1::{map_to_2d, map_to_2d_on_curve};
use crate::surface::surface::Surface;
use crate::surface::surface_cache::p1::FaceContainment;
use crate::surface::trim_srf_cache::TrimSrfCache;

const OP: &str = "intersect::curve_class::p9";

/// Read-only `SmEdge` data, resolved through its owning B-rep.
#[derive(Debug, Clone, Copy)]
pub struct BoundaryEdge<O> {
    /// Edge classification handle.
    pub edge: O,
    /// Handle of the edge's actual curve (preserves pointer identity).
    pub curve: O,
    /// Edge parameter interval.
    pub interval: Extent1d,
    /// Stored edge zone tolerance (`SmEdge::GetTolerance`).
    pub zone_tol_3d: f64,
    /// Maximum gap for this face's edgeuse; `None` means no such edgeuse.
    pub face_gap: Option<f64>,
}

/// A vertexuse and its vertex, with its computed UV point.
#[derive(Debug, Clone, Copy)]
pub struct BoundaryVertex<O> {
    /// Vertexuse identity, used to deduplicate cached node lists.
    pub vertexuse: O,
    /// Vertex classification handle.
    pub vertex: O,
    /// `ComputeUVPoint` (fallback) or the cached UV point.
    pub uv: Point2,
    /// Vertex position (`SmVertex::GetPoint`).
    pub point: Point3,
    /// Stored vertex zone tolerance (`SmVertex::GetTolerance`).
    pub zone_tol_3d: f64,
}

/// Read-only edgeuse data, with the real UV trim curve.
#[derive(Debug, Clone, Copy)]
pub struct BoundaryEdgeuse<O> {
    /// Edgeuse identity, used to deduplicate cached node lists.
    pub edgeuse: O,
    /// Edge and its 3D curve.
    pub edge: BoundaryEdge<O>,
    /// UV trim curve handle. A missing lazy trim defers to its owning unit.
    pub trim_curve: Option<O>,
}

/// One upward loopuse's children in native traversal order.
#[derive(Debug, Clone)]
pub struct BoundaryLoop<O> {
    /// `GetVertexuses`, in native order (may repeat across loops).
    pub vertices: Vec<BoundaryVertex<O>>,
    /// `GetEdgeuses`, in native order.
    pub edges: Vec<BoundaryEdgeuse<O>>,
}

/// Resolved upward faceuse traversal and the face's actual surface and trim cache.
/// These views borrow geometry; they do not replace the B-rep arenas.
pub struct BoundaryFace<'a, O, C = O> {
    /// Face surface.
    pub surface: &'a Surface,
    /// Upward loopuses, in native order.
    pub loops: &'a [BoundaryLoop<O>],
    /// The face surface's trimmed surface cache (`smsurf_GetSurfaceCache`), whose
    /// vertex use and edge use handles are `C`.
    pub cache: &'a TrimSrfCache<C>,
    /// The caller's handle of a vertex use or edge use the cache's leaf nodes name
    /// (`None`: not one of the face's).
    pub cache_handle: &'a dyn Fn(C) -> Option<O>,
}

/// What the cached branch of `IntersectTSBound` visits
/// ([`CurveClassification::ts_bound_cached_visits`]).
#[derive(Debug, Clone)]
pub struct CachedVisits<O> {
    /// No cache node meets the source curve's UV box: nothing to do, and no mark is
    /// claimed.
    pub no_nodes: bool,
    /// The vertex uses of the leaf nodes, each once, with the cache's UV points.
    pub vertices: Vec<BoundaryVertex<O>>,
    /// The edge uses of the leaf nodes, each once.
    pub edgeuses: Vec<BoundaryEdgeuse<O>>,
}

/// `IntersectEdges` flags. `has_big_gaps` is required upstream; this Rust options
/// struct defaults it to false. `uv_space_okay` has the native default true.
#[derive(Debug, Clone, Copy)]
pub struct IntersectEdgesOptions {
    /// Increase intersection tolerance for known edge/face gaps.
    pub has_big_gaps: bool,
    /// Permit UV-space intersection; false lifts a 2D source onto the surface.
    pub uv_space_okay: bool,
}
impl Default for IntersectEdgesOptions {
    fn default() -> Self {
        Self {
            has_big_gaps: false,
            uv_space_okay: true,
        }
    }
}

/// The source and target curves of a curve/curve intersection here, with their
/// natural intervals (upstream's objects are the curves, whose natural intervals
/// `IdenticalSolutions` reads).
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Source(Extent1d),
    Target(Extent1d),
}
impl SolutionObject for Side {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        match self {
            Self::Source(ivl) | Self::Target(ivl) => Some(*ivl),
        }
    }
}

fn curve<O: ClassObject>(g: &impl ClassGeometry<O>, handle: O) -> Result<&Curve> {
    g.curve(handle).ok_or(SolidError::null_pointer(OP))
}
fn lifted(k: &Kernel, uv: &Curve, surface: &Surface) -> Result<Curve> {
    Ok(Curve::from(CrvOnSurf::new(
        k,
        uv.clone_for_evaluation()?,
        surface.clone_for_evaluation()?,
        Some(surface.natural_uv_domain_with_nurb(k)?),
        0,
    )?))
}
fn checked_vertex_drop(
    k: &Kernel,
    lift: &Curve,
    interval: &Extent1d,
    point: Point3,
    tolerance: f64,
) -> Result<DroppedPoint> {
    // IntersectTSBound passes &dParam as the guess and dParam as the output.
    // SmCrvOnSurf::DropPoint initializes its output to 0 before reading that
    // aliased guess. Preserve the observed zero seed, not the earlier UV result.
    Ok(ignored(lift.drop_point(
        k,
        interval,
        point,
        None,
        tolerance,
        Some(0.0),
        SolverOperation::Intersect,
    ))?
    .unwrap_or_default())
}
fn named<O: Copy>(s: &Solution<Side>, source: O, target: O) -> Solution<O> {
    let mut out = Solution::new();
    out.solution_type = s.solution_type;
    out.num_variables = s.num_variables;
    out.num_objects = s.num_objects;
    out.start = s.start;
    out.end = s.end;
    out.nodes = s.nodes;
    for (to, from) in out.objects.iter_mut().zip(s.objects) {
        *to = from.map(|x| match x {
            Side::Source(_) => source,
            Side::Target(_) => target,
        });
    }
    out
}

impl<O: ClassObject> CurveClassification<O> {
    /// Intersects the source curve with each edge, appending classifications in order.
    /// A shared curve uses the exact interval-overlap shortcut before insertion.
    ///
    /// # Errors
    /// Missing geometry fails; unavailable surface evaluation or mutable edge
    /// insertion defers to its dependency. Earlier successful insertions are retained.
    pub fn intersect_edges(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        edges: &[BoundaryEdge<O>],
        surface: &Surface,
        options: IntersectEdgesOptions,
    ) -> Result<()> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        let source = curve(geometry, source_handle)?;
        let lift;
        let working = if !options.uv_space_okay && source.dim() == 2 {
            let mut unowned = surface.clone_for_evaluation()?;
            let _ = unowned.set_owner(None);
            lift = lifted(k, source, &unowned)?;
            &lift
        } else {
            source
        };
        let source_zone = src_zone_tol_3d_of(k, Some(self));
        // the source curve's cache, kept across the edges (see `edge_solutions`)
        let mut source_cache = None;
        for edge in edges {
            let (mut solutions, tolerance) = self.edge_solutions(
                k,
                geometry,
                working,
                source_handle,
                source_zone,
                edge,
                options,
                &mut source_cache,
            )?;
            if !solutions.is_empty() {
                source_cache = None;
                insert_solutions(
                    k,
                    geometry,
                    self,
                    &mut solutions,
                    edge.edge,
                    Some(tolerance),
                )?;
            }
        }
        Ok(())
    }

    /// [`CurveClassification::intersect_edges`] over a mutable adapter: the solutions
    /// are inserted with [`insert_solutions_mut`], whose surface/surface/surface
    /// refinement may edit the classification curve and the edges in place; each edge
    /// is intersected with the curve as it is then (upstream's `m_cpCurve` pointer).
    /// `surface` (the face's surface) is needed only to lift a 2D source when
    /// `options.uv_space_okay` is off.
    ///
    /// # Errors
    /// As [`CurveClassification::intersect_edges`]; [`SolidError::InvalidInput`] for a 2D
    /// source to lift without `surface`.
    pub fn intersect_edges_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        edges: &[BoundaryEdge<O>],
        surface: Option<&Surface>,
        options: IntersectEdgesOptions,
    ) -> Result<()> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        let lift = {
            let source = curve(&*geometry, source_handle)?;
            if !options.uv_space_okay && source.dim() == 2 {
                let surface = surface.ok_or(SolidError::invalid_input(
                    OP,
                    "a 2D source to lift without the face's surface",
                ))?;
                let mut unowned = surface.clone_for_evaluation()?;
                let _ = unowned.set_owner(None);
                Some(lifted(k, source, &unowned)?)
            } else {
                None
            }
        };
        let source_zone = src_zone_tol_3d_of(k, Some(self));
        // the source curve's cache, kept across the edges (see `edge_solutions`)
        let mut source_cache = None;
        for edge in edges {
            let (mut solutions, tolerance) = {
                let working = match &lift {
                    Some(l) => l,
                    None => curve(&*geometry, source_handle)?,
                };
                self.edge_solutions(
                    k,
                    &*geometry,
                    working,
                    source_handle,
                    source_zone,
                    edge,
                    options,
                    &mut source_cache,
                )?
            };
            if !solutions.is_empty() {
                // the insertion's surf/surf/surf refinement may edit the source curve
                source_cache = None;
                insert_solutions_mut(
                    k,
                    geometry,
                    self,
                    &mut solutions,
                    edge.edge,
                    Some(tolerance),
                )?;
            }
        }
        Ok(())
    }

    /// The solutions of one edge of `IntersectEdges` and their intersection tolerance.
    /// `source_cache` keeps the source curve `working`'s cache across the edges, as
    /// upstream keeps it on the curve; the caller drops it whenever an insertion may
    /// have edited the curve.
    #[allow(clippy::too_many_arguments)]
    fn edge_solutions(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        working: &Curve,
        source_handle: O,
        source_zone: f64,
        edge: &BoundaryEdge<O>,
        options: IntersectEdgesOptions,
        source_cache: &mut Option<CurveCache<Side>>,
    ) -> Result<(Vec<Solution<O>>, f64)> {
        let target = curve(geometry, edge.curve)?;
        let edge_zone = edge.zone_tol_3d;
        let mut tolerance = xsect_tol_3d_from_zones(k, edge_zone, source_zone);
        if options.has_big_gaps {
            if let Some(gap) = edge.face_gap {
                if gap > tolerance {
                    tolerance = gap * 1.1;
                }
            }
        }
        let mut solutions = Vec::new();
        if core::ptr::eq(target, working) {
            let lo = max2(edge.interval.min(), self.interval().min());
            let hi = min2(edge.interval.max(), self.interval().max());
            if lo < hi {
                let mut sol = Solution::new();
                sol.solution_type = SolutionType::RangeOfValues;
                sol.num_variables = 2;
                sol.num_objects = 2;
                for slot in sol.objects.iter_mut().take(2) {
                    *slot = Some(edge.curve);
                }
                for i in 0..2 {
                    *sol.start.param_mut(i)? = lo;
                    *sol.end.param_mut(i)? = hi;
                }
                solutions.push(sol);
            }
        } else {
            // `GlobalCurveIntersect` caches the edge's own curve object, whose cache is
            // built at `SmTol::GetApproxTol3d(&m_crCurve)`: half the edge's tolerance
            // (`SmCurveCache.cpp:1496`, through the curve's owner). The caller's handle
            // geometry is not that owned object, so the live edge tolerance is supplied
            // to the lazy build; a cache is still built only where the solve needs it.
            let found = working.global_curve_intersect_cached_with_zone_tols(
                k,
                &self.interval(),
                target,
                &edge.interval,
                tolerance,
                &CurveIntersectOptions {
                    handles: [
                        Some(Side::Source(working.natural_interval())),
                        Some(Side::Target(target.natural_interval())),
                    ],
                    ..CurveIntersectOptions::default()
                },
                source_cache,
                &mut None,
                [None, Some(edge_zone)],
            )?;
            solutions.extend(
                found
                    .as_slice()
                    .iter()
                    .map(|s| named(s, source_handle, edge.curve)),
            );
        }
        Ok((solutions, tolerance))
    }

    /// Intersects a 2D source with a face's trim vertices and curves. A valid trim
    /// cache restricts work to intersecting leaf nodes; otherwise all loopuses run.
    /// `do_check_3d` also verifies vertex hits in 3D. Trim-curve hits always refine in 3D.
    ///
    /// # Errors
    /// Missing geometry/cache records fail; generic surface lifting and mutable
    /// edge insertion retain their existing dependency deferrals.
    pub fn intersect_ts_bound<C: Copy + PartialEq>(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        face: &BoundaryFace<'_, O, C>,
        do_check_3d: bool,
    ) -> Result<()> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        let source = curve(geometry, source_handle)?;
        if source.dim() != 2 {
            return Err(SolidError::invalid_input(
                OP,
                "trim boundary source must be 2D",
            ));
        }
        let Some(visits) = self.ts_bound_cached_visits(k, geometry, face)? else {
            for lp in face.loops {
                for vertex in &lp.vertices {
                    self.p9_vertex(k, geometry, source, face.surface, *vertex, do_check_3d)?;
                }
                for edgeuse in &lp.edges {
                    self.p9_edgeuse(k, geometry, source_handle, face.surface, *edgeuse, false)?;
                }
            }
            return Ok(());
        };
        // no work - curve is outside of the domain of the surface
        if visits.no_nodes {
            return Ok(());
        }
        // increment the mark on the context by 1 (the vertex uses and edge uses it
        // marks are the ordered visit lists)
        let _mark_lock = k.context.claim_mark(ALL_MARKS);
        for vertex in &visits.vertices {
            self.ts_bound_vertex(k, geometry, face.surface, *vertex, do_check_3d)?;
        }
        for edgeuse in &visits.edgeuses {
            self.ts_bound_edgeuse(k, geometry, face.surface, *edgeuse)?;
        }
        Ok(())
    }

    /// The cached branch of `IntersectTSBound`, up to its traversals: `None` when the
    /// face's trimmed cache is not up to date (the loop-use branch runs instead), else
    /// the vertex uses (with the cache's UV points) and then the edge uses of the
    /// cache's leaf nodes that meet the source curve's UV box, each once, in the order
    /// upstream visits (marks) them. The caller claims the context mark when there are
    /// nodes ([`CachedVisits::no_nodes`] is false), then intersects them with
    /// [`Self::ts_bound_vertex`] and [`Self::ts_bound_edgeuse`].
    ///
    /// # Errors
    /// Missing geometry or cache records; a source curve whose curve cache cannot be
    /// built (`SM_ERR_NULL_POINTER`).
    pub fn ts_bound_cached_visits<C: Copy + PartialEq>(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        face: &BoundaryFace<'_, O, C>,
    ) -> Result<Option<CachedVisits<O>>> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        let source = curve(geometry, source_handle)?;
        let base = face.cache.base();
        if base.face_was_modified
            || !base.have_t_surface_cache
            || base.face_containment_done == FaceContainment::NotDone
        {
            return Ok(None);
        }
        // `GetOrCreateObjectCache(SM_OC_CURVE, ..)` then `NER(pCC)`: a failed cache
        // build (e.g. a degenerate source) is a NULL cache, SM_ERR_NULL_POINTER.
        let cache = match make_curve_cache::<Side>(
            k,
            source,
            Some(Side::Source(source.natural_interval())),
        ) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(OP)),
        };
        let tree = cache.tree().ok_or(SolidError::null_pointer(OP))?;
        let top = tree.top_node().ok_or(SolidError::null_pointer(OP))?;
        let mut domain = Extent2d::from_extent3d(&tree.node(top)?.bbox);
        domain.expand_absolute(EFF_ZERO_SQRT);
        let nodes = base.find_uv_nodes_built(k, &domain, false)?;
        let mut visits = CachedVisits {
            no_nodes: nodes.is_empty(),
            vertices: Vec::new(),
            edgeuses: Vec::new(),
        };
        if visits.no_nodes {
            return Ok(Some(visits));
        }

        let tree = base.tree().ok_or(SolidError::null_pointer(OP))?;
        for &id in &nodes {
            // skip all but the leaf nodes
            if tree.node(id)?.child1.is_some() {
                continue;
            }
            for item in &base.aux(id)?.vertex_list {
                let vu = item.vertexuse.ok_or(SolidError::null_pointer(OP))?;
                let vu = (face.cache_handle)(vu).ok_or(SolidError::null_pointer(OP))?;
                // skip vertices that have already been processed
                if visits.vertices.iter().any(|v| v.vertexuse == vu) {
                    continue;
                }
                let mut vertex = face
                    .loops
                    .iter()
                    .flat_map(|lp| &lp.vertices)
                    .find(|v| v.vertexuse == vu)
                    .copied()
                    .ok_or(SolidError::null_pointer(OP))?;
                vertex.uv = item.uv_point;
                visits.vertices.push(vertex);
            }
        }
        for &id in &nodes {
            if tree.node(id)?.child1.is_some() {
                continue;
            }
            for &eu in &base.aux(id)?.edgeuse_list {
                let eu = (face.cache_handle)(eu).ok_or(SolidError::null_pointer(OP))?;
                // skip edgeuses already processed
                if visits.edgeuses.iter().any(|e| e.edgeuse == eu) {
                    continue;
                }
                let edgeuse = face
                    .loops
                    .iter()
                    .flat_map(|lp| &lp.edges)
                    .find(|e| e.edgeuse == eu)
                    .copied()
                    .ok_or(SolidError::null_pointer(OP))?;
                visits.edgeuses.push(edgeuse);
            }
        }
        Ok(Some(visits))
    }

    /// One vertex use of the cached branch of `IntersectTSBound` (see
    /// [`Self::ts_bound_cached_visits`]).
    ///
    /// # Errors
    /// As [`Self::intersect_ts_bound`].
    pub fn ts_bound_vertex(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        vertex: BoundaryVertex<O>,
        do_check_3d: bool,
    ) -> Result<()> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        let source = curve(geometry, source_handle)?;
        self.p9_vertex(k, geometry, source, surface, vertex, do_check_3d)
    }

    /// One edge use of the cached branch of `IntersectTSBound` (see
    /// [`Self::ts_bound_cached_visits`]); its UV trim curve (`GetOrCreateUVTrimCurve`)
    /// must be in `edgeuse`.
    ///
    /// # Errors
    /// As [`Self::intersect_ts_bound`].
    pub fn ts_bound_edgeuse(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        edgeuse: BoundaryEdgeuse<O>,
    ) -> Result<()> {
        let source_handle = self.curve().ok_or(SolidError::null_pointer(OP))?;
        self.p9_edgeuse(k, geometry, source_handle, surface, edgeuse, true)
    }

    fn p9_vertex(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        source: &Curve,
        surface: &Surface,
        vertex: BoundaryVertex<O>,
        do_check_3d: bool,
    ) -> Result<()> {
        let source_zone = src_zone_tol_3d_of(k, Some(self));
        let zone = vertex.zone_tol_3d;
        let tolerance = xsect_tol_3d_from_zones(k, source_zone, zone);
        let singular = surface
            .is_singularity(k, vertex.uv, EFF_ZERO, false)?
            .singular;
        let lift;
        let (drop_curve, point, distance_tol) = if singular {
            lift = lifted(k, source, surface)?;
            let point = ignored(surface.evaluate_point(k, vertex.uv))?.unwrap_or(Point3::UNDEF);
            (&lift, point, tolerance)
        } else {
            (
                source,
                Point3::from(vertex.uv),
                xsect_tol3d(k, map_to_2d(k, tolerance, vertex.uv, surface)?),
            )
        };
        let mut found = drop_curve.drop_point(
            k,
            &self.interval(),
            point,
            None,
            distance_tol,
            None,
            SolverOperation::Intersect,
        )?;
        if !found.success {
            return Ok(());
        }
        let mut distance_3d = 0.0;
        if do_check_3d {
            let lift = lifted(k, source, surface)?;
            let point = vertex.point;
            // Native ignores the status, but DropPoint initializes success to false.
            // A failed checked solve must not retain the earlier UV success.
            found = checked_vertex_drop(k, &lift, &self.interval(), point, tolerance)?;
            let on_surface =
                ignored(lift.evaluate_point(k, found.parameter))?.unwrap_or(Point3::UNDEF);
            distance_3d = point.distance(on_surface);
        }
        if found.success && (!do_check_3d || distance_3d < tolerance) {
            let mut pc = PointClassification::new(source_zone);
            pc.set_class_object(PointClassType::Vertex, Some(vertex.vertex));
            pc.set_gap_3d(found.distance);
            pc.set_src_zone_tol_3d(source_zone);
            pc.set_pre_snap_param(found.parameter);
            self.insert_point_class(k, geometry, &pc, found.parameter, BIG_DOUBLE, false)?;
        }
        Ok(())
    }

    fn p9_edgeuse(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        source_handle: O,
        surface: &Surface,
        eu: BoundaryEdgeuse<O>,
        cached: bool,
    ) -> Result<()> {
        let trim_handle = eu.trim_curve.ok_or_else(|| {
            if !cached {
                return SolidError::null_pointer(OP);
            }
            // UNPORTED(topology.edgeuse_p3): SmEdgeuse::GetOrCreateUVTrimCurve (justified: findings.md topology.edgeuse_p3)
            // the boundary views are read-only `ClassGeometry` handles built from a
            // borrowed `&Brep` (`query::face::p2::curve_on_classify`), so a trim curve
            // created here could be neither stored nor resolved
            // Proof (unreachable): only for an uncached trim curve met in a read-only
            // boundary view; 0 hits in the green difftest sweep.
            SolidError::unported("SmEdgeuse::GetOrCreateUVTrimCurve", "topology.edgeuse_p3")
        })?;
        let trim = curve(geometry, trim_handle)?;
        let source = curve(geometry, source_handle)?;
        let source_zone = src_zone_tol_3d_of(k, Some(self));
        let edge_zone = eu.edge.zone_tol_3d;
        let (tol3d, tol2d) = if !cached && self.doing_cross_hatch() {
            let tol = xsect_tol_3d_from_zones(k, source_zone, source_zone);
            (tol, tol)
        } else {
            let tol = xsect_tol_3d_from_zones(k, source_zone, edge_zone);
            (
                tol,
                xsect_tol3d(
                    k,
                    map_to_2d_on_curve(
                        k,
                        tol,
                        trim.natural_interval().evaluate(0.5),
                        trim,
                        surface,
                    )?,
                ),
            )
        };
        let mut solutions = source.global_curve_intersect(
            k,
            &self.interval(),
            trim,
            &trim.natural_interval(),
            tol2d,
            &CurveIntersectOptions {
                handles: [
                    Some(Side::Source(source.natural_interval())),
                    Some(Side::Target(trim.natural_interval())),
                ],
                ..CurveIntersectOptions::default()
            },
        )?;
        if !solutions.is_empty() {
            cull_or_refine_cci_in_3d(
                k,
                surface,
                source,
                &self.interval(),
                curve(geometry, eu.edge.curve)?,
                &eu.edge.interval,
                tol3d,
                &mut solutions,
            )?;
            let mut named_solutions: Vec<_> = solutions
                .as_slice()
                .iter()
                .map(|s| named(s, source_handle, trim_handle))
                .collect();
            insert_solutions(
                k,
                geometry,
                self,
                &mut named_solutions,
                eu.edge.edge,
                Some(tol2d),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::intersect::point_class::ClassObjectKind;
    use crate::math::Vec3;
    use crate::surface::plane::Plane;
    use crate::surface::surface_cache::p1::SurfaceCacheOptions;
    use crate::surface::volume::Volume;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum H {
        Source,
        Target,
        Edge,
    }
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            if *self == Self::Edge {
                ClassObjectKind::Edge
            } else {
                ClassObjectKind::Curve
            }
        }
    }
    struct Geometry {
        source: Curve,
        target: Curve,
    }
    impl ClassGeometry<H> for Geometry {
        fn topology_zone_tol_3d(&self, h: H) -> Option<f64> {
            (h == H::Edge).then_some(1e-6)
        }
        fn curve(&self, h: H) -> Option<&Curve> {
            match h {
                H::Source => Some(&self.source),
                H::Target => Some(&self.target),
                H::Edge => None,
            }
        }
        fn surface(&self, _: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _: H) -> Option<Point3> {
            None
        }
    }
    fn geometry() -> Geometry {
        let line = |a: Vec3, dir: Vec3| {
            Curve::from(Line::bounded(
                a,
                dir,
                &Extent1d::from_min_max(0., 1.),
                &BoundedLineOptions {
                    scale: 1.,
                    dim: 3,
                    ..Default::default()
                },
            ))
        };
        Geometry {
            source: line(Vec3::ZERO, Vec3::new(1., 0., 0.)),
            target: line(Vec3::new(0.5, -0.5, 2.1e-6), Vec3::new(0., 1., 0.)),
        }
    }
    fn classification(k: &Kernel) -> CurveClassification<H> {
        CurveClassification::with_curve_interval(k, H::Source, Extent1d::from_min_max(0., 1.), 1e-6)
    }
    #[test]
    fn face_gap_threshold_changes_the_intersection_tolerance() {
        let k = Kernel::default();
        let geometry = geometry();
        let surface = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0., 0., 1.)));
        let mut edge = BoundaryEdge {
            edge: H::Edge,
            curve: H::Target,
            interval: Extent1d::from_min_max(0., 1.),
            face_gap: Some(2e-6),
            zone_tol_3d: 1e-6,
        };
        let options = IntersectEdgesOptions {
            has_big_gaps: true,
            ..Default::default()
        };
        let mut cc = classification(&k);
        let original = cc.intervals().to_vec();
        cc.intersect_edges(&k, &geometry, &[edge], &surface, options)
            .unwrap();
        assert_eq!(cc.intervals(), original);
        edge.face_gap = Some(2.01e-6);
        let error = cc
            .intersect_edges(&k, &geometry, &[edge], &surface, options)
            .unwrap_err();
        assert!(
            matches!(
                error,
                SolidError::Unported {
                    upstream: "SmEdge::GetCurve",
                    unit: "topology.edge",
                }
            ),
            "{error:?}"
        );
        assert_eq!(cc.intervals(), original);
    }
    #[test]
    fn missing_trim_does_not_mean_lazy_creation_in_the_fallback() {
        let k = Kernel::default();
        let mut geometry = geometry();
        geometry.source = Curve::from(Line::bounded(
            Vec3::ZERO,
            Vec3::new(1., 0., 0.),
            &Extent1d::from_min_max(0., 1.),
            &BoundedLineOptions {
                dim: 2,
                ..Default::default()
            },
        ));
        let surface = Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0., 0., 1.)));
        let cache = TrimSrfCache::new(surface.clone(), SurfaceCacheOptions::default());
        let loops = [BoundaryLoop {
            vertices: Vec::new(),
            edges: vec![BoundaryEdgeuse {
                edgeuse: H::Edge,
                edge: BoundaryEdge {
                    edge: H::Edge,
                    curve: H::Target,
                    interval: Extent1d::from_min_max(0., 1.),
                    face_gap: None,
                    zone_tol_3d: 1e-6,
                },
                trim_curve: None,
            }],
        }];
        let mut cc = classification(&k);
        assert!(matches!(
            cc.intersect_ts_bound(
                &k,
                &geometry,
                &BoundaryFace {
                    surface: &surface,
                    loops: &loops,
                    cache: &cache,
                    cache_handle: &Some,
                },
                false
            ),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }
    #[test]
    fn checked_drop_preserves_the_native_aliased_zero_guess() {
        use crate::nurbs::b_spline_surface::BSplineSurface;
        use crate::nurbs::math::{CPoint, NOW};
        use crate::nurbs::srf_basic::Surface as NlSurface;
        let k = Kernel::default();
        let net = (0..2)
            .map(|u| {
                (0..2)
                    .map(|v| CPoint::new(1e6 * f64::from(u), 1e6 * f64::from(v), 0., NOW))
                    .collect()
            })
            .collect();
        let knots = vec![0., 0., 1., 1.];
        let surface = Surface::from(BSplineSurface::from_nurb(
            NlSurface::new(net, 1, 1, knots.clone(), knots).unwrap(),
            false,
        ));
        let source = Curve::from(Line::from_points(
            Vec3::new(0.2, 0.5, 0.),
            Vec3::new(0.8, 0.5, 0.),
            2,
        ));
        let lift = lifted(&k, &source, &surface).unwrap();
        // Native aliased-call probes return these exact parameter/gap pairs.
        // Keeping the UV guess instead returns gap 0 for both points.
        for (u, parameter) in [
            (0.6315271935871615, 0.7192119893119358),
            (0.7485310025864422, 0.9142183376440705),
        ] {
            let found = checked_vertex_drop(
                &k,
                &lift,
                &Extent1d::from_min_max(0., 1.),
                Vec3::new(1e6 * u, 5e5, 0.),
                0.19999999999999998,
            )
            .unwrap();
            assert!(found.success);
            assert_eq!(found.parameter, parameter);
            assert_eq!(found.distance, 1.1641532182693481e-10);
        }
    }

    /// Handles of [`a_kept_source_cache_intersects_edges_as_fresh_caches`].
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum E {
        Source,
        Target(usize),
        Edge(usize),
    }
    impl ClassObject for E {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Edge(_) => ClassObjectKind::Edge,
                _ => ClassObjectKind::Curve,
            }
        }
    }
    struct Edges {
        source: Curve,
        targets: Vec<Curve>,
    }
    impl ClassGeometry<E> for Edges {
        fn topology_zone_tol_3d(&self, h: E) -> Option<f64> {
            matches!(h, E::Edge(_)).then_some(1e-6)
        }
        fn curve(&self, h: E) -> Option<&Curve> {
            match h {
                E::Source => Some(&self.source),
                E::Target(i) => self.targets.get(i),
                E::Edge(_) => None,
            }
        }
        fn surface(&self, _: E) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: E) -> Option<&Volume> {
            None
        }
        fn point(&self, _: E) -> Option<Point3> {
            None
        }
    }

    /// The source curve's cache kept across the edges (and dropped after an edge with
    /// hits, as the edge loops do) gives the solutions of a fresh cache for each edge.
    #[test]
    fn a_kept_source_cache_intersects_edges_as_fresh_caches() {
        use crate::nurbs::b_spline_curve::BSplineCurve;
        use crate::nurbs::math::{CPoint, NOW};
        use crate::nurbs::nurbs_crv::allocate_nurb_curve;

        let k = Kernel::default();
        // a planar cubic B-spline (not analytic, so the intersection reads the caches)
        let pts = [[0.0, 0.0], [1.0, 2.0], [3.0, -1.0], [4.0, 1.0], [6.0, 0.0]];
        let mut nurb = allocate_nurb_curve(4, 3, 8).unwrap();
        for (d, s) in nurb.cpts_mut().iter_mut().zip(pts) {
            *d = CPoint::new(s[0], s[1], 0.0, NOW);
        }
        nurb.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
        let source = Curve::from(BSplineCurve::from_nurb(nurb, 3, false));
        let line = |a: Vec3, dir: Vec3, len: f64| {
            Curve::from(Line::bounded(
                a,
                dir,
                &Extent1d::from_min_max(0., len),
                &BoundedLineOptions {
                    scale: 1.,
                    dim: 3,
                    ..Default::default()
                },
            ))
        };
        let up = Vec3::new(0., 1., 0.);
        let far = |y: f64| line(Vec3::new(-10., y, 0.), Vec3::new(1., 0., 0.), 1.);
        // misses and crossings in turn: reuse after misses, a rebuild after each hit
        let targets = vec![
            far(20.),
            line(Vec3::new(1., -5., 0.), up, 10.),
            far(30.),
            far(40.),
            line(Vec3::new(2.5, -5., 0.), up, 10.),
            line(Vec3::new(4., -5., 0.), up, 10.),
            far(50.),
        ];
        let geometry = Edges { source, targets };
        let cc = CurveClassification::with_curve_interval(
            &k,
            E::Source,
            Extent1d::from_min_max(0., 1.),
            1e-6,
        );
        let source_zone = src_zone_tol_3d_of(&k, Some(&cc));
        let working = &geometry.source;
        let mut kept = None;
        let (mut hits, mut reused) = (0, 0);
        for (i, target) in geometry.targets.iter().enumerate() {
            let interval = target.natural_interval();
            let edge = BoundaryEdge {
                edge: E::Edge(i),
                curve: E::Target(i),
                interval,
                face_gap: None,
                zone_tol_3d: 1e-6,
            };
            let solve = |cache: &mut Option<CurveCache<Side>>| {
                cc.edge_solutions(
                    &k,
                    &geometry,
                    working,
                    E::Source,
                    source_zone,
                    &edge,
                    IntersectEdgesOptions::default(),
                    cache,
                )
                .unwrap()
            };
            let fresh = solve(&mut None);
            if kept.is_some() {
                reused += 1;
            }
            let with_kept = solve(&mut kept);
            assert!(kept.is_some());
            assert_eq!(format!("{with_kept:?}"), format!("{fresh:?}"), "edge {i}");
            if !with_kept.0.is_empty() {
                hits += 1;
                kept = None;
            }
        }
        assert_eq!(hits, 3);
        assert!(reused >= 3);
    }
}
