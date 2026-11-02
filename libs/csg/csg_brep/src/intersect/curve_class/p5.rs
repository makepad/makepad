//! Curve classification, part 5: intersection insertion, geometry extraction and merging.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`insert_solutions`], [`insert_solutions_mut`] | `sm_InsertSolutions` (static) |
//! | [`CurveClassification::extract_geometry`], [`CurveClassification::extract_geometry_mut`] | `SmCurveClassification::ExtractGeometry` |
//! | [`CurveClassification::fixup_classification`] | `SmCurveClassification::FixupClassification` |
//! | [`CurveClassification::insert_topology_intersections`] | `SmCurveClassification::InsertTopologyIntersections` |
//! | [`CurveClassification::intersect_vertices`], [`CurveClassification::intersect_vertices_mut`] | `SmCurveClassification::IntersectVertices` |
//! | [`CurveClassification::merge_classifications`] | `SmCurveClassification::MergeClassifications` |
//! | [`CurveClassification::merge_intervals`] | `SmCurveClassification::MergeIntervals` |
//! | [`ClassGeometryMut`] | the non-const objects behind `m_cpCurve`, `SmVertex*`, `SmEdgeuse*` |
//!
//! Deviations: output lists become owned results; the crate-private merge variant
//! retains completed intervals' partial output in its error for ignored-status
//! callers. Handles and geometry use the part-1 model. Optional [`ClassGeometry`]
//! callbacks resolve vertex points, topology tolerances and vertex/face gaps; opaque
//! handles still defer.
//!
//! **In-place edits.** Upstream edits objects it reaches through const pointers: the
//! edge branch of `sm_InsertSolutions` raises a vertex tolerance to the edge's
//! (`SmVertex::SetTolerance`, when `GetOKToSetTolerance`), and the surf/surf/surf block
//! of `IntersectVertices` (SmCurveClass.cpp:14826-15095) makes the classification
//! curve pass through a vertex (`sm_SetXSectCurveThroughPoint`), marks it not current
//! and deletes the UV trim curves of the vertex's edges and their Boolean mates. The
//! `_mut` entry points do these edits through a [`ClassGeometryMut`]. The read-only
//! entry points run the same code over a plain [`ClassGeometry`] and defer
//! (`SmBrep mutable owning context`, unit `topology.brep_p1`) where an edit is due:
//! before raising the tolerance and before `sm_SetXSectCurveThroughPoint` is called on
//! the classification curve.
//!
//! `IntersectVertices` clears the face surface's owner while it runs (no trimmed
//! surface cache): its projections onto that surface use an owner-free copy. The
//! surf/surf/surf block evaluates both classification surfaces out of bounds (copies,
//! as part 1), on the surface itself without its owner when it is that face surface.
//! Parts 2, 6 and 7 are wired. The edge branch of `insert_solutions` resolves the edge
//! through [`ClassGeometry::topology_entity`] (an adapter without it keeps the
//! `SmEdge::GetCurve` deferral): the edge-end vertex snap, then
//! `sm_RefineCCIToSurfaces` through its no-work gate (its large-gap tail defers, part
//! 2), then the edge classification. The fixup
//! restart loop is bounded by three times the initial interval count plus one: fixes remove an interval or
//! snap an edge classification to a vertex. Undefined
//! native null dereferences and unequal paired classification lengths return errors.
//! Debug drawing/assertions are omitted. Ignored ordinary Trim failures stay ignored;
//! failed Copy returns an error rather than dereferencing a null native result.
//! `insert_topology_intersections` rejects `object_index > 1`, which upstream accepts.
//! `p5_closed` errors on an unresolved curve, where upstream returns FALSE.
//! The differential runner's face zone tolerance of zero is not compared against C++.

use super::p1::{out_of_bounds, set_xsect_curve_through_point};
use super::p2::{refine_cci_to_surfaces_in, refine_cci_to_surfaces_mut, RefineCci};
use super::p6::{
    FilletPeer, Imprint, MergeIntervalBrep, Mutable as MutableImprint, ReadOnly as ReadOnlyImprint,
};
use super::{
    make_consistent_classifications, src_zone_tol_3d_of, CurveClassification, CurveInterval,
};
use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::{Curve, CurveKind};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tol::{
    map_to_1d_on_curve, xsect_tol3d, xsect_tol_3d_from_zones, ANG_TOL_DEG, USE_DEFAULT,
};
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max3, Point2, Point3, BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::Brep;
use crate::EntityRef;

/// Mutable access to what classification handles name, for the edits upstream makes
/// through them (see the module docs): the classification curve, and the B-reps of
/// vertices and edge uses.
pub trait ClassGeometryMut<O>: ClassGeometry<O> {
    /// The curve of a [`ClassObjectKind::Curve`] handle, mutably; `None` for a handle
    /// the adapter cannot resolve (an invalid-input error for the caller).
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve>;
    /// The B-rep and entity behind a topology handle, mutably (as
    /// [`ClassGeometry::topology_entity`]); `None` for a handle the adapter cannot
    /// resolve (an invalid-input error for the caller).
    fn topology_entity_mut(&mut self, o: O) -> Option<(&mut Brep, EntityRef)>;
    /// `sm_SetDoFilletingGeometry` replaces an edge's curve at its native setter
    /// position. The default preserves the older direct arena assignment;
    /// retained adapters override it to perform `SetCurve` invalidation and notify.
    ///
    /// # Errors
    /// Missing mutable edge context or the retained setter's original error.
    fn set_filleting_edge_curve(&mut self, edge: O, curve: Curve) -> Result<()> {
        match self.topology_entity_mut(edge) {
            Some((b, EntityRef::Edge(e))) => {
                b.edge_mut(e)?.curve = Some(curve);
                Ok(())
            }
            // UNPORTED(topology.edge): SmEdge::SetCurve
            // Proof (unreachable): the library ClassGeometry impls resolve the edge handle
            // (topology_entity/_mut); 0 hits in the green difftest sweep.
            _ => Err(SolidError::unported("SmEdge::SetCurve", "topology.edge")),
        }
    }
    /// [`ClassGeometryMut::topology_entity_mut`] for a query (a classification or a
    /// point solve on a face) that only makes caches and UV trim curves on the B-rep,
    /// as upstream's queries of a face make them without editing it. An adapter that
    /// keeps caches of the B-rep's faces keeps them over such a lend.
    fn topology_entity_for_query(&mut self, o: O) -> Option<(&mut Brep, EntityRef)> {
        self.topology_entity_mut(o)
    }
}

/// The edits of part 5 over either kind of adapter.
trait Access<O: ClassObject> {
    type Geometry: ClassGeometry<O>;
    fn geometry(&self) -> &Self::Geometry;
    /// `pVtx->SetTolerance(tol)` (upstream defaults: only if larger).
    fn set_vertex_tolerance(&mut self, vertex: O, tol: f64) -> Result<()>;
    /// The classification curve `m_cpCurve`, for an in-place edit.
    fn curve_mut(&mut self, curve: O) -> Result<&mut Curve>;
    /// `pEdgeuse->SetUVTrimCurve(NULL, 0.0, TRUE)`.
    fn delete_uv_trim_curve(&mut self, edgeuse: O) -> Result<()>;
    /// `sm_RefineCCIToSurfaces(pCurveClassification, rSol, pEdge, ..)` with a fresh
    /// recursion counter and an undefined `vSSSPoint`: the read-only adapter defers its
    /// large-gap edits, the mutable one makes them.
    fn refine_cci_to_surfaces(
        &mut self,
        k: &Kernel,
        classification: &mut CurveClassification<O>,
        solution: &mut Solution<O>,
        edge: O,
    ) -> Result<RefineCci>;
}

/// A read-only adapter: every edit defers.
struct ReadOnly<'g, G>(&'g G);

impl<O: ClassObject, G: ClassGeometry<O>> Access<O> for ReadOnly<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn set_vertex_tolerance(&mut self, _vertex: O, _tol: f64) -> Result<()> {
        // DISPATCH GAP: SmBrep mutable owning context (read-only adapter without a Brep)
        Err(SolidError::unported(
            "SmBrep mutable owning context (SmVertex::SetTolerance)",
            "topology.brep_p1",
        ))
    }
    fn curve_mut(&mut self, _curve: O) -> Result<&mut Curve> {
        // DISPATCH GAP: SmBrep mutable owning context (read-only adapter without a Brep)
        Err(SolidError::unported(
            "SmBrep mutable owning context (sm_SetXSectCurveThroughPoint)",
            "topology.brep_p1",
        ))
    }
    fn delete_uv_trim_curve(&mut self, _edgeuse: O) -> Result<()> {
        // DISPATCH GAP: SmBrep mutable owning context (read-only adapter without a Brep)
        Err(SolidError::unported(
            "SmBrep mutable owning context (SmEdgeuse::SetUVTrimCurve)",
            "topology.brep_p1",
        ))
    }
    fn refine_cci_to_surfaces(
        &mut self,
        k: &Kernel,
        classification: &mut CurveClassification<O>,
        solution: &mut Solution<O>,
        edge: O,
    ) -> Result<RefineCci> {
        refine_cci_to_surfaces_in(k, self.0, classification, solution, edge, Point3::UNDEF, 0)
    }
}

/// A mutable adapter: the edits are made.
struct Mutable<'g, G>(&'g mut G);

impl<O: ClassObject, G: ClassGeometryMut<O>> Access<O> for Mutable<'_, G> {
    type Geometry = G;
    fn geometry(&self) -> &G {
        self.0
    }
    fn set_vertex_tolerance(&mut self, vertex: O, tol: f64) -> Result<()> {
        match self.0.topology_entity_mut(vertex) {
            Some((b, EntityRef::Vertex(v))) => {
                crate::topology::vertex::set_tolerance(b, v, tol, true, true)
            }
            _ => Err(SolidError::invalid_input(OP, "unresolved mutable vertex")),
        }
    }
    fn curve_mut(&mut self, curve: O) -> Result<&mut Curve> {
        self.0
            .curve_mut(curve)
            .ok_or(SolidError::invalid_input(OP, "unresolved mutable curve"))
    }
    fn delete_uv_trim_curve(&mut self, edgeuse: O) -> Result<()> {
        match self.0.topology_entity_mut(edgeuse) {
            Some((b, EntityRef::Edgeuse(eu))) => {
                crate::topology::edgeuse::p1::delete_uv_trim_curve(b, eu)
            }
            _ => Err(SolidError::invalid_input(OP, "unresolved mutable edge use")),
        }
    }
    fn refine_cci_to_surfaces(
        &mut self,
        k: &Kernel,
        classification: &mut CurveClassification<O>,
        solution: &mut Solution<O>,
        edge: O,
    ) -> Result<RefineCci> {
        refine_cci_to_surfaces_mut(k, self.0, classification, solution, edge, Point3::UNDEF, 0)
    }
}

#[derive(Clone, Copy, PartialEq)]
struct NoObject;
impl SolutionObject for NoObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

#[derive(Clone, Copy)]
enum PairLink {
    Metadata,
    Linked,
}

const OP: &str = "intersect::curve_class::p5";

/// `sLooseCrvDropSize`: vertex drops onto an intersection curve within
/// `[LOOSE_CRV_DROP_SIZE, XSectTol3d]` are checked for surf/surf/surf refinement.
const LOOSE_CRV_DROP_SIZE: f64 = 1.0e-7;
/// `sLooseSrfDropSize`: a vertex dropping to both intersection surfaces within this
/// distance is interpolated by the intersection curve.
const LOOSE_SRF_DROP_SIZE: f64 = 1.0e-8;

/// One surface projection of the surf/surf/surf block of `IntersectVertices`: with a UV
/// curve, a local solve from its point at `param` (kept in `t_point`, which a failed
/// evaluation leaves as it was); else a global solve with point testing off. Writes
/// upstream's `bFoundAnswer` / `sSol` locals; `false` when the global solve found
/// nothing (upstream `continue`s with them unchanged). Solver statuses are ignored.
#[allow(clippy::too_many_arguments)] // upstream's locals
fn project_vertex_point<O>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    any_uv_curve: Option<O>,
    surface: &Surface,
    vertex_point: Point3,
    param: f64,
    t_point: &mut Point3,
    found: &mut bool,
    solution: &mut Solution<NoObject>,
) -> Result<bool> {
    let domain = surface.natural_uv_domain_with_nurb(k)?;
    if let Some(uv_curve) = any_uv_curve {
        // let good UVguess = UVTrimCurve(nearest param to vertex value)
        let uv_curve = geometry
            .curve(uv_curve)
            .ok_or(SolidError::invalid_input(OP, "unresolved UV curve"))?;
        if let Some(p) = ignored(uv_curve.evaluate_point(k, param))? {
            *t_point = p;
        }
        match ignored(surface.local_point_solve::<NoObject>(
            k,
            &domain,
            SolverOperation::Minimize,
            vertex_point,
            Point2::new(t_point.x, t_point.y),
        ))? {
            Some(r) => {
                *found = r.found;
                *solution = r.solution;
            }
            // (a failed solve reports no answer)
            None => *found = false,
        }
        return Ok(true);
    }
    // no good UVguess - a global point solve ignoring trim curve boundaries
    let mut solutions = SolutionArray::new();
    ignore_status(surface.global_point_solve_point_test_off_into::<NoObject>(
        k,
        None,
        &domain,
        SolverOperation::Minimize,
        vertex_point,
        0.01,
        None,
        SolutionRequested::Single,
        &mut solutions,
    ))?;
    let Some(first) = solutions.as_slice().first().copied() else {
        return Ok(false);
    };
    *solution = first;
    *found = true;
    Ok(true)
}

/// Geometry of common classified intervals and isolated boundary points.
#[derive(Debug, Default, Clone)]
pub struct ExtractedGeometry {
    /// Copies of the classification curve trimmed to common intervals.
    pub curves_3d: Vec<Curve>,
    /// Trimmed copies of this classification's first UV curve, when requested.
    pub uv_curves1: Vec<Curve>,
    /// Trimmed copies of the other classification's first UV curve, when requested.
    pub uv_curves2: Vec<Curve>,
    /// Isolated classified points, when requested.
    pub points_3d: Vec<Point3>,
}

/// Requested optional outputs of `ExtractGeometry` (upstream defaults: all false).
#[derive(Debug, Default, Clone, Copy)]
pub struct ExtractOptions {
    /// Request the first UV curves.
    pub uv_curves1: bool,
    /// Request the second UV curves.
    pub uv_curves2: bool,
    /// Request isolated points.
    pub points_3d: bool,
}

/// Topology created by a merge. Collection order is native insertion order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedTopology<O> {
    /// Newly created faces.
    pub faces: Vec<O>,
    /// Split edges (including reused edges).
    pub edges: Vec<O>,
    /// Newly created vertices.
    pub vertices: Vec<O>,
}

impl<O> Default for MergedTopology<O> {
    fn default() -> Self {
        Self {
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
        }
    }
}

/// A failed `MergeIntervals` pass, including lists from completed intervals.
#[derive(Debug)]
pub(crate) struct PartialMergeError<O> {
    /// The first failed interval's status.
    pub(crate) error: SolidError,
    /// Output appended before that failure (native `SER` skips the failing interval).
    pub(crate) merged: Box<MergedTopology<O>>,
}

fn contained_mid(c: PointClassType) -> bool {
    matches!(
        c,
        PointClassType::Edge | PointClassType::Face | PointClassType::Region
    )
}

fn contained_point(c: PointClassType) -> bool {
    contained_mid(c) || c == PointClassType::Vertex
}

fn copy_uv_bspline(curve: &Curve) -> Result<Curve> {
    // Native constructs SmBSplineCurve from the UV curve's B-spline base, slicing
    // analytic subclasses. A non-B-spline UV curve makes that native cast undefined.
    let base = match curve.kind() {
        CurveKind::BSpline(c) => c.clone(),
        CurveKind::Line(c) => c.base_with_nurb()?.into_owned(),
        CurveKind::Ellipse(c) => c.base().clone(),
        CurveKind::Hyperbola(c) => c.base().clone(),
        CurveKind::Parabola(c) => c.base().clone(),
        _ => {
            return Err(SolidError::invalid_input(
                OP,
                "UV curve has no B-spline base",
            ))
        }
    };
    Ok(Curve::from(base))
}

fn append_unique<O: PartialEq>(dst: &mut Vec<O>, src: impl IntoIterator<Item = O>) {
    for value in src {
        if !dst.contains(&value) {
            dst.push(value);
        }
    }
}

/// Native `sBigGapXSectTol3d`: a face gap overrides the default constructor only
/// when both large-gap processing and a face are present.
fn vertex_big_gap_tolerance<O: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    vertex: O,
    surface: &Surface,
    face: Option<O>,
    has_big_gaps: bool,
    tolerance: f64,
) -> Result<f64> {
    if !has_big_gaps {
        return Ok(tolerance);
    }
    let has_face = face.is_some()
        || surface
            .owner()
            .is_some_and(|owner| owner.type_code == crate::geom::assert_array::labels::FACE_TYPE);
    if !has_face {
        return Ok(xsect_tol3d(k, USE_DEFAULT));
    }
    let gap = geometry
        .vertex_face_gap(vertex, surface, face)
        .ok_or_else(|| {
            SolidError::invalid_input(OP, "unresolved classification vertex/face gap")
        })?;
    Ok(if gap > tolerance { gap } else { tolerance })
}

impl<O: ClassObject> CurveClassification<O> {
    fn p5_closed(&self, k: &Kernel, geometry: &impl ClassGeometry<O>) -> Result<bool> {
        let Some(handle) = self.curve() else {
            return Ok(false);
        };
        let curve = geometry
            .curve(handle)
            .ok_or(SolidError::invalid_input(OP, "unresolved curve"))?;
        let zone = src_zone_tol_3d_of(k, Some(self));
        curve.is_closed(
            k,
            &curve.natural_interval(),
            xsect_tol_3d_from_zones(k, zone, zone),
        )
    }

    // Thin wrappers over the part-2/6/7 members.
    fn p5_point_class_by_index(
        classification: &Self,
        index: usize,
    ) -> Result<(&PointClassification<O>, f64)> {
        classification.point_class_by_index(index)
    }

    fn p5_classify_crv_loc_by_index(
        k: &Kernel,
        classification: &Self,
        geometry: &impl ClassGeometry<O>,
        index: usize,
    ) -> Result<(Point3, f64)> {
        classification.classify_crv_loc_by_index(k, geometry, index)
    }

    fn p5_merge_interval(
        classification: &mut Self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        index: usize,
        vertices_only: bool,
    ) -> Result<MergedTopology<O>> {
        let merged = classification.merge_interval(k, geometry, index, vertices_only)?;
        Ok(MergedTopology {
            faces: merged.new_faces,
            edges: merged.new_edges,
            vertices: merged.new_vertices,
        })
    }

    fn p5_merge_interval_in(
        classification: &mut Self,
        a: &mut impl Imprint<O>,
        index: usize,
        vertices_only: bool,
        peer: FilletPeer<'_, O>,
    ) -> Result<MergedTopology<O>> {
        let merged = classification.merge_interval_using_peer(a, index, vertices_only, peer)?;
        Ok(MergedTopology {
            faces: merged.new_faces,
            edges: merged.new_edges,
            vertices: merged.new_vertices,
        })
    }

    /// Extracts common geometry after homogenizing and cleaning both classifications.
    ///
    /// # Errors
    /// Unresolvable handles or unfinished topology merging defer; invalid geometry fails.
    pub fn extract_geometry(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &mut Self,
        options: ExtractOptions,
    ) -> Result<ExtractedGeometry> {
        self.homogenize(k, geometry, other)?;
        self.extract_homogenized(k, geometry, other, options)
    }

    /// [`CurveClassification::extract_geometry`] over topology the homogenization's
    /// point drops may change ([`CurveClassification::homogenize_mut`]): a drop onto a
    /// face makes and keeps the face's trimmed surface cache on its B-rep, as upstream.
    ///
    /// # Errors
    /// Those of [`CurveClassification::extract_geometry`].
    pub fn extract_geometry_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        other: &mut Self,
        options: ExtractOptions,
    ) -> Result<ExtractedGeometry> {
        self.homogenize_mut(k, geometry, other)?;
        self.extract_homogenized(k, &*geometry, other, options)
    }

    /// `ExtractGeometry` after its `Homogenize`.
    fn extract_homogenized(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &mut Self,
        options: ExtractOptions,
    ) -> Result<ExtractedGeometry> {
        let mut out = ExtractedGeometry::default();
        self.cleanup_and_validate(k, geometry, other)?;
        if self.size() != other.size() {
            return Err(SolidError::sm_err(OP));
        }
        let mut found = Vec::new();
        for i in 0..self.size() {
            let this = self.at(i)?;
            let common = contained_mid(this.mid.point_class())
                && contained_mid(other.at(i)?.mid.point_class());
            if common {
                let mut curve = self.curve_geometry(geometry)?.copy(k)?;
                let mut interval = this.interval;
                ignore_status(curve.trim(k, &mut interval, TrimMode::default()))?;
                out.curves_3d.push(curve);
                for (requested, handle, curves) in [
                    (options.uv_curves1, self.uv_curve1(), &mut out.uv_curves1),
                    (options.uv_curves2, other.uv_curve1(), &mut out.uv_curves2),
                ] {
                    if requested {
                        if let Some(handle) = handle {
                            let mut uv =
                                copy_uv_bspline(geometry.curve(handle).ok_or(
                                    SolidError::invalid_input(OP, "unresolved UV curve"),
                                )?)?;
                            ignore_status(uv.trim(k, &mut interval, TrimMode::default()))?;
                            curves.push(uv);
                        }
                    }
                }
            }
            found.push(common);
        }
        let mut found_last = self.p5_closed(k, geometry)? && found.last().copied().unwrap_or(false);
        for j in 0..self.size() {
            let (this, _) = Self::p5_point_class_by_index(self, j)?;
            let (that, _) = Self::p5_point_class_by_index(other, j)?;
            let found_current = found.get(j).copied().ok_or(SolidError::sm_err(OP))?;
            if contained_point(this.point_class())
                && contained_point(that.point_class())
                && !found_current
                && !found_last
            {
                // Upstream computes a tolerance-weighted point, then overwrites it with
                // GetClassifyCrvLoc(j). Retain the fallible lookups before that overwrite.
                let p1 = Self::p5_classify_crv_loc_by_index(k, self, geometry, j)?.0;
                let p2 = Self::p5_classify_crv_loc_by_index(k, other, geometry, j)?.0;
                if (p2 - p1).length() > EFF_ZERO {
                    object_zone_tol_3d(k, geometry, this.object())?;
                    object_zone_tol_3d(k, geometry, that.object())?;
                }
                let point = Self::p5_classify_crv_loc_by_index(k, self, geometry, j)?.0;
                if options.points_3d {
                    out.points_3d.push(point);
                }
            }
            found_last = found_current;
        }
        Ok(out)
    }

    /// Repairs classified interval endpoints, restarting after each interval removal.
    ///
    /// # Errors
    /// Evaluation errors, unresolved topology, or a repair loop that makes no progress.
    pub fn fixup_classification(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
    ) -> Result<()> {
        let mut restarts = self.size().saturating_mul(3).saturating_add(1);
        'restart: loop {
            for i in 0..self.size() {
                if self.at(i)?.mid.point_class() == PointClassType::Unknown {
                    continue;
                }
                let curve = self.curve_geometry(geometry)?;
                let interval = self.at(i)?.interval;
                let start = curve.evaluate_point(k, interval.min())?;
                let end = curve.evaluate_point(k, interval.max())?;
                if self.fix_end_point_class(k, geometry, i, start)?
                    || self.fix_end_point_class(k, geometry, i + 1, end)?
                {
                    restarts = restarts.checked_sub(1).ok_or(SolidError::sm_err(OP))?;
                    continue 'restart;
                }
            }
            break;
        }
        make_consistent_classifications(k, geometry, self)
    }

    /// Inserts topology solutions in four passes: nonface ranges, vertices, other
    /// points, then face ranges longer than one twentieth of the classified interval.
    ///
    /// # Errors
    /// Invalid object index, null point-solution objects, insertion errors or deferrals.
    pub fn insert_topology_intersections(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        solutions: &[Solution<O>],
        object_index: usize,
    ) -> Result<()> {
        if object_index > 1 {
            return Err(SolidError::invalid_input(OP, "object index must be 0 or 1"));
        }
        let other_index = usize::from(object_index == 0);
        let zone = src_zone_tol_3d_of(k, Some(self));
        for pass in 0..4 {
            for sol in solutions {
                let object = sol.objects.get(other_index).copied().flatten();
                let face = object.is_some_and(|o| o.class_kind() == ClassObjectKind::Face);
                if (pass == 0 || pass == 3)
                    && sol.solution_type == SolutionType::RangeOfValues
                    && ((pass == 0 && !face)
                        || (pass == 3
                            && face
                            && (sol.end.param(object_index)? - sol.start.param(object_index)?)
                                .abs()
                                > self.interval().length() / 20.0))
                {
                    let object =
                        object.ok_or(SolidError::invalid_input(OP, "null solution object"))?;
                    let (start, end) =
                        if sol.start.param(object_index)? <= sol.end.param(object_index)? {
                            (&sol.start, &sol.end)
                        } else {
                            (&sol.end, &sol.start)
                        };
                    let mut ivl = CurveInterval::with_interval(
                        k,
                        &Extent1d::from_min_max(
                            start.param(object_index)?,
                            end.param(object_index)?,
                        ),
                        Some(self),
                    );
                    ivl.start.load_from_intersection(
                        zone,
                        object,
                        start,
                        other_index,
                        start.param(object_index)?,
                    )?;
                    ivl.mid
                        .load_from_intersection(zone, object, start, other_index, BIG_DOUBLE)?;
                    ivl.end.load_from_intersection(
                        zone,
                        object,
                        end,
                        other_index,
                        end.param(object_index)?,
                    )?;
                    self.insert_curve_interval(k, geometry, &ivl)?;
                } else if (pass == 1 || pass == 2) && sol.solution_type == SolutionType::SingleValue
                {
                    let object = object
                        .ok_or(SolidError::invalid_input(OP, "null point-solution object"))?;
                    let vertex = object.class_kind() == ClassObjectKind::Vertex;
                    if (pass == 1 && vertex) || (pass == 2 && !vertex) {
                        let mut pc = PointClassification::new(zone);
                        pc.load_from_intersection(
                            zone,
                            object,
                            &sol.start,
                            other_index,
                            sol.start.param(object_index)?,
                        )?;
                        self.insert_point_class(
                            k,
                            geometry,
                            &pc,
                            sol.start.param(object_index)?,
                            BIG_DOUBLE,
                            false,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Intersects the curve with vertices (defaults: no large gaps, UV space allowed),
    /// over a read-only adapter: where the surf/surf/surf block would edit the
    /// classification curve, this defers ([`CurveClassification::intersect_vertices_mut`]
    /// makes the edits).
    ///
    /// # Errors
    /// Missing vertex points or vertex/face gaps are invalid input. Unported surface
    /// dependencies and the curve edit defer.
    pub fn intersect_vertices(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        vertices: &[O],
        surface: &Surface,
        has_big_gaps: bool,
        uv_space_okay: bool,
    ) -> Result<()> {
        self.intersect_vertices_in(
            k,
            &mut ReadOnly(geometry),
            vertices,
            surface,
            has_big_gaps,
            uv_space_okay,
            true,
        )
    }

    /// `IntersectVertices` with the edits of its surf/surf/surf block: the
    /// classification curve is made to pass through a vertex on both classification
    /// surfaces, and the UV trim curves of the edge uses of that vertex's edges (and of
    /// their Boolean mates) are deleted.
    ///
    /// # Errors
    /// As [`CurveClassification::intersect_vertices`], without the deferral of the
    /// edit; handles the adapter cannot resolve mutably are invalid input.
    pub fn intersect_vertices_mut(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ClassGeometryMut<O>,
        vertices: &[O],
        surface: &Surface,
        has_big_gaps: bool,
        uv_space_okay: bool,
    ) -> Result<()> {
        self.intersect_vertices_in(
            k,
            &mut Mutable(geometry),
            vertices,
            surface,
            has_big_gaps,
            uv_space_okay,
            true,
        )
    }

    /// The body of [`CurveClassification::intersect_vertices`]. With `keep_curve_cache`
    /// off every vertex drop builds its own curve cache (the reference the kept cache
    /// must reproduce).
    fn intersect_vertices_in(
        &mut self,
        k: &Kernel,
        access: &mut impl Access<O>,
        vertices: &[O],
        surface: &Surface,
        has_big_gaps: bool,
        uv_space_okay: bool,
        keep_curve_cache: bool,
    ) -> Result<()> {
        // work in 3d when asked
        let mut wrapped = {
            let original_curve = self.curve_geometry(access.geometry())?;
            if !uv_space_okay && original_curve.dim() == 2 {
                Some(Curve::from(CrvOnSurf::new(
                    k,
                    original_curve.clone_for_evaluation()?,
                    surface.clone_for_evaluation()?,
                    Some(surface.natural_uv_domain_with_nurb(k)?),
                    0,
                )?))
            } else {
                None
            }
        };
        let source_zone = src_zone_tol_3d_of(k, Some(self));
        // temporarily clear surface->Owner pointer to prevent building trimSrfCache for
        // crSurface (after the face is taken from it): an owner-free copy
        let detached = surface.clone();
        // the classification curve's cache, kept across the vertex drops as upstream keeps
        // it on the curve; dropped when the surf/surf/surf block may edit the curve
        let mut curve_cache = None;
        for &vertex in vertices {
            let geometry = access.geometry();
            let vertex_zone = object_zone_tol_3d(k, geometry, Some(vertex))?;
            let tolerance = xsect_tol_3d_from_zones(k, source_zone, vertex_zone);
            let point = geometry
                .classification_vertex_point(vertex)
                .ok_or_else(|| {
                    SolidError::invalid_input(OP, "unresolved classification vertex point")
                })?;
            let big_tolerance = vertex_big_gap_tolerance(
                k,
                geometry,
                vertex,
                surface,
                self.face(),
                has_big_gaps,
                tolerance,
            )?;
            let mut solutions = {
                let curve = match &wrapped {
                    Some(c) => c,
                    None => self.curve_geometry(geometry)?,
                };
                curve.global_point_solve_unnamed_cached::<NoObject>(
                    k,
                    &self.interval(),
                    SolverOperation::Minimize,
                    point,
                    tolerance,
                    SolutionRequested::All,
                    Some(0.0),
                    &mut curve_cache,
                )?
            };
            if !keep_curve_cache {
                curve_cache = None;
            }
            if solutions.is_empty() {
                continue;
            }
            // GWC:CHANGED_SSS: when the curve being classified is an intersection curve
            // and there are multiple vertex drop points or one out-of-tolerance one
            if self.surface1().is_some()
                && self.surface2().is_some()
                && (solutions.len() > 1
                    || (solutions.len() == 1
                        && solutions.as_slice().first().is_some_and(|s| {
                            s.start.solution_value > LOOSE_CRV_DROP_SIZE
                                && s.start.solution_value < tolerance
                        })))
            {
                curve_cache = None;
                self.refine_vertex_solutions(
                    k,
                    access,
                    &mut wrapped,
                    &mut solutions,
                    vertex,
                    point,
                    tolerance,
                    source_zone,
                    surface,
                )?;
            }
            // end GWC:CHANGED_SSS
            for solution in solutions.as_slice() {
                let geometry = access.geometry();
                let curve = match &wrapped {
                    Some(c) => c,
                    None => self.curve_geometry(geometry)?,
                };
                if solution.start.solution_value > tolerance {
                    if !has_big_gaps || solution.start.solution_value > big_tolerance {
                        continue;
                    }
                    let domain = detached.natural_uv_domain_with_nurb(k)?;
                    let vertex_solutions = detached.global_point_solve::<NoObject>(
                        k,
                        None,
                        &domain,
                        SolverOperation::Minimize,
                        point,
                        vertex_zone,
                        None,
                        SolutionRequested::Single,
                    )?;
                    let Some(projected) = vertex_solutions.as_slice().first() else {
                        continue;
                    };
                    let vertex_on_surface = detached.evaluate_point(
                        k,
                        Point2::new(projected.start.param(0)?, projected.start.param(1)?),
                    )?;
                    let curve_point = curve.evaluate_point(k, solution.start.param(0)?)?;
                    let curve_solutions = detached.global_point_solve::<NoObject>(
                        k,
                        None,
                        &domain,
                        SolverOperation::Minimize,
                        curve_point,
                        source_zone,
                        None,
                        SolutionRequested::Single,
                    )?;
                    let Some(projected) = curve_solutions.as_slice().first() else {
                        continue;
                    };
                    let curve_on_surface = detached.evaluate_point(
                        k,
                        Point2::new(projected.start.param(0)?, projected.start.param(1)?),
                    )?;
                    if curve_on_surface.distance(vertex_on_surface) > tolerance {
                        continue;
                    }
                }
                let mut pc = PointClassification::new(source_zone);
                pc.set_class_object(PointClassType::Vertex, Some(vertex));
                pc.set_gap_3d(solution.start.solution_value);
                pc.set_src_zone_tol_3d(source_zone);
                pc.set_pre_snap_param(solution.start.param(0)?);
                self.insert_point_class(
                    k,
                    geometry,
                    &pc,
                    solution.start.param(0)?,
                    BIG_DOUBLE,
                    false,
                )?;
            }
        }
        Ok(())
    }

    /// The surf/surf/surf block of `IntersectVertices` (SmCurveClass.cpp:14826-15095):
    /// culls drop points snapping to the same knot (keeping the closer), then, for each
    /// drop point in `[1e-7, tolerance]` whose vertex lies on both classification
    /// surfaces within `1e-8`, makes the curve pass through the surf/surf/surf point
    /// (`sm_SetXSectCurveThroughPoint`) and, when it changed, updates the solution,
    /// marks the curve not current and deletes the UV trim curves of the vertex's edges.
    #[allow(clippy::too_many_arguments)] // upstream's locals
    fn refine_vertex_solutions(
        &mut self,
        k: &Kernel,
        access: &mut impl Access<O>,
        wrapped: &mut Option<Curve>,
        solutions: &mut SolutionArray<NoObject>,
        vertex: O,
        vertex_point: Point3,
        tolerance: f64,
        source_zone: f64,
        face_surface: &Surface,
    ) -> Result<()> {
        // locals (declared once for the whole block, as upstream)
        let mut found1 = false;
        let mut found2 = false;
        let mut t_point = Point3::UNDEF;
        let mut sol1: Solution<NoObject> = Solution::new();
        let mut sol2: Solution<NoObject> = Solution::new();

        // the classification surfaces, out of bounds enabled (SmTemporaryChangeValue on
        // the SmBSplineSurface flag); the face surface runs without its owner here
        let (surface1, surface2) =
            {
                let geometry = access.geometry();
                let resolve =
                    |h: Option<O>| -> Result<Surface> {
                        let s = h.and_then(|h| geometry.surface(h)).ok_or(
                            SolidError::invalid_input(OP, "unresolved classification surface"),
                        )?;
                        Ok(if core::ptr::eq(s, face_surface) {
                            s.clone()
                        } else {
                            s.clone_keeping_owner()
                        })
                    };
                (resolve(self.surface1())?, resolve(self.surface2())?)
            };
        let solve1 = out_of_bounds(&surface1)?.into_owned();
        let solve2 = out_of_bounds(&surface2)?.into_owned();

        // ClassificationCurve's Param RangeSize
        let crv_param_range = {
            let curve = match wrapped {
                Some(c) => &*c,
                None => self.curve_geometry(access.geometry())?,
            };
            curve.natural_interval().length()
        };

        // cull multiple solutions of the same near point
        {
            let curve = match wrapped {
                Some(c) => &*c,
                None => self.curve_geometry(access.geometry())?,
            };
            let snap = |s: &Solution<NoObject>| -> Result<f64> {
                let param = s.start.param(0)?;
                // (the status is ignored; the output starts as the input parameter)
                Ok(ignored(curve.snap_to_knots(k, param, crv_param_range))?.unwrap_or(param))
            };
            let mut jj = 0;
            while jj + 1 < solutions.len() {
                let solution_jj = solutions.get_at(jj)?;
                let snap_jj = snap(&solution_jj)?;
                let mut restart = false;
                let mut kk = jj + 1;
                while kk < solutions.len() {
                    let solution_kk = solutions.get_at(kk)?;
                    let snap_kk = snap(&solution_kk)?;
                    // check for solutions near the same knot value
                    if snap_jj == snap_kk {
                        // remove the solution with the largest deviation
                        if solution_jj.start.solution_value > solution_kk.start.solution_value {
                            // remove solution jj and start the outer loop over at jj
                            solutions.remove_at(jj, 1);
                            restart = true;
                            break;
                        }
                        // remove solution kk and continue with the inner loop
                        solutions.remove_at(kk, 1);
                        continue;
                    }
                    kk += 1;
                }
                if !restart {
                    jj += 1;
                }
            }
        }

        // for every curve/vertex projection solution
        for jj in 0..solutions.len() {
            let value = solutions.get_at(jj)?.start.solution_value;
            // skip solutions that already intersect the vertex with tight tolerance or
            // don't pass the curve+vertex tolerance criterion
            if value < LOOSE_CRV_DROP_SIZE || value > tolerance {
                continue;
            }
            // curve param for this near point
            let mut param = solutions.get_at(jj)?.start.param(0)?;

            // project the vertex point onto surface1, then surface2
            if !project_vertex_point(
                k,
                access.geometry(),
                self.any_uv_curve1(),
                &solve1,
                vertex_point,
                param,
                &mut t_point,
                &mut found1,
                &mut sol1,
            )? {
                continue;
            }
            let uv1 = Point2::new(sol1.start.param(0)?, sol1.start.param(1)?);
            if !project_vertex_point(
                k,
                access.geometry(),
                self.any_uv_curve2(),
                &solve2,
                vertex_point,
                param,
                &mut t_point,
                &mut found2,
                &mut sol2,
            )? {
                continue;
            }
            let uv2 = Point2::new(sol2.start.param(0)?, sol2.start.param(1)?);

            // when the vertex is close to both surfaces it is a surf/surf xsect point and
            // the xsect curve should interpolate it
            if !(found1
                && sol1.start.solution_value < LOOSE_SRF_DROP_SIZE
                && found2
                && sol2.start.solution_value < LOOSE_SRF_DROP_SIZE)
            {
                continue;
            }
            let mut deviation = 0.0;
            // GWC:CHANGE_SSSUVTRIMCURVE: stop modifying UVTrimCurves (aUVCurve.ReSet())
            let curve = if let Some(c) = wrapped.as_mut() {
                c
            } else {
                let handle = self
                    .curve()
                    .ok_or(SolidError::invalid_input(OP, "no classification curve"))?;
                access.curve_mut(handle)?
            };
            // make the classification curve interpolate this vertex (its shape changes
            // by less than tolerance); the status is ignored
            let mut found = false;
            if let Some(r) = ignored(set_xsect_curve_through_point(
                k,
                curve,
                source_zone,
                &mut [],
                &[&surface1, &surface2],
                &[uv1, uv2],
                param,
                t_point,
            ))? {
                found = r.modified_curve;
                param = r.param;
                if let Some(d) = r.deviation {
                    deviation = d;
                }
                t_point = r.point_3d;
            }

            // update this solution after the curves are updated
            if found {
                let solution = solutions
                    .get_mut(jj)
                    .ok_or(SolidError::invalid_input(OP, "solution index"))?;
                *solution.start.param_mut(0)? = param;
                solution.start.solution_value = max3(
                    sol1.start.solution_value,
                    sol2.start.solution_value,
                    deviation,
                );

                // GWC:CHANGE_SSSUVTRIMCURVE: remove obsolete curve UVTrimCurves
                self.set_is_curve_current(false);

                // remove obsolete UVTrimCurves from the edges attached to this vertex
                let edgeuses = self.vertex_edge_edgeuses(access.geometry(), vertex)?;
                for edgeuse in edgeuses {
                    access.delete_uv_trim_curve(edgeuse)?;
                }
            }
        }
        Ok(())
    }

    /// The edge uses of `vertex`'s edges (`SmVertex::GetEdges`, `SmEdge::GetEdgeuses`),
    /// each edge followed by those of its Boolean mate when the classification has a
    /// topology intersector (`GetOtherMate`, else `GetBrepMate`).
    fn vertex_edge_edgeuses(&self, geometry: &impl ClassGeometry<O>, vertex: O) -> Result<Vec<O>> {
        let (b, v) = super::p7::vertex_of(geometry, vertex, "SmVertex::GetEdges")?;
        let handle = |b: &Brep, entity: EntityRef| {
            geometry
                .topology_handle_in(b, entity)
                .ok_or(SolidError::invalid_input(
                    OP,
                    "topology entity without handle",
                ))
        };
        let mut out = Vec::new();
        for e in crate::topology::vertex::edges(b, v, None)? {
            for eu in crate::topology::edge::edgeuses(b, e, None, None)? {
                out.push(handle(b, EntityRef::Edgeuse(eu))?);
            }
            // see if the edge has a boolean operation temporary mate
            if let Some(ti) = self.topology_intersector() {
                let edge = handle(b, EntityRef::Edge(e))?;
                let mate = geometry.topology_intersector_mate(ti, edge).ok_or(
                    // UNPORTED(boolean.topology_intersector_p1): SmTopologyIntersector::GetOtherMate
                    // (an adapter that cannot reach the intersector behind its handle)
                    // Proof (unreachable): only if topology_intersector() is Some and the
                    // adapter returns no mate; the library ClassGeometry impls resolve it;
                    // 0 hits in the green difftest sweep.
                    SolidError::unported(
                        "SmTopologyIntersector::GetOtherMate",
                        "boolean.topology_intersector_p1",
                    ),
                )?;
                if let Some(mate) = mate {
                    let (mb, me) = super::p7::edge_of(geometry, mate, "SmEdge::GetEdgeuses")?;
                    for eu in crate::topology::edge::edgeuses(mb, me, None, None)? {
                        out.push(handle(mb, EntityRef::Edgeuse(eu))?);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Merges common classified intervals, then optional singularities, into both B-reps.
    ///
    /// # Errors
    /// Unequal interval counts fail before mutation; missing merging operations defer.
    pub fn merge_classifications(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        other: &mut Self,
        merge_singularities: bool,
    ) -> Result<(MergedTopology<O>, MergedTopology<O>)> {
        self.merge_classifications_in(
            &mut ReadOnlyImprint { k, g: geometry },
            other,
            merge_singularities,
            PairLink::Metadata,
        )
    }

    /// [`CurveClassification::merge_classifications`] editing the B-reps behind the
    /// classifications' topology handles ([`CurveClassification::merge_interval_mut`],
    /// [`CurveClassification::merge_point_class_mut`]).
    ///
    /// # Errors
    /// Those of [`CurveClassification::merge_classifications`] and of the B-rep edits.
    pub fn merge_classifications_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        other: &mut Self,
        merge_singularities: bool,
    ) -> Result<(MergedTopology<O>, MergedTopology<O>)> {
        self.merge_classifications_in(
            &mut MutableImprint { k, g: geometry },
            other,
            merge_singularities,
            PairLink::Metadata,
        )
    }

    /// Merge an actual local pair whose native peer pointers are linked for this
    /// call. No opaque handle is assigned; each peer is borrowed directly. Existing
    /// metadata-based entrypoints retain their original absence semantics.
    ///
    /// # Errors
    /// Those of [`Self::merge_classifications_mut`].
    pub fn merge_classifications_linked_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        other: &mut Self,
        merge_singularities: bool,
    ) -> Result<(MergedTopology<O>, MergedTopology<O>)> {
        self.merge_classifications_in(
            &mut MutableImprint { k, g: geometry },
            other,
            merge_singularities,
            PairLink::Linked,
        )
    }

    fn merge_classifications_in(
        &mut self,
        a: &mut impl Imprint<O>,
        other: &mut Self,
        merge_singularities: bool,
        link: PairLink,
    ) -> Result<(MergedTopology<O>, MergedTopology<O>)> {
        if self.size() != other.size() {
            return Err(SolidError::sm_err(OP));
        }
        let mut out = MergedTopology::default();
        let mut out_other = MergedTopology::default();
        let mut scratch = MergedTopology::default();
        let mut scratch_other = MergedTopology::default();
        let closed = self.p5_closed(a.kernel(), a.geometry())?;
        for i in 0..self.size() {
            if !contained_mid(self.at(i)?.mid.point_class())
                || !contained_mid(other.at(i)?.mid.point_class())
            {
                continue;
            }
            scratch = Self::p5_merge_interval_in(
                self,
                a,
                i,
                false,
                match link {
                    PairLink::Metadata => FilletPeer::Metadata(Some(other)),
                    PairLink::Linked => FilletPeer::Linked(other),
                },
            )?;
            append_unique(&mut out.faces, scratch.faces.iter().copied());
            append_unique(&mut out.edges, scratch.edges.iter().copied());
            append_unique(&mut out.vertices, scratch.vertices.iter().copied());
            scratch_other = Self::p5_merge_interval_in(
                other,
                a,
                i,
                false,
                match link {
                    PairLink::Metadata => FilletPeer::Metadata(Some(self)),
                    PairLink::Linked => FilletPeer::Linked(self),
                },
            )?;
            append_unique(&mut out_other.faces, scratch_other.faces.iter().copied());
            append_unique(&mut out_other.edges, scratch_other.edges.iter().copied());
            append_unique(
                &mut out_other.vertices,
                scratch_other.vertices.iter().copied(),
            );
            if i == 0 && closed {
                let last = self.size() - 1;
                self.at_mut(last)?.end = self.at(0)?.start;
                other.at_mut(last)?.end = other.at(0)?.start;
            }
        }
        if merge_singularities && self.size() > 0 {
            for j in 0..=self.size() {
                if contained_point(Self::p5_point_class_by_index(self, j)?.0.point_class())
                    && contained_point(Self::p5_point_class_by_index(other, j)?.0.point_class())
                {
                    self.merge_point_class_in(
                        a,
                        j,
                        Some(&mut scratch.edges),
                        Some(&mut scratch.vertices),
                    )?;
                    // MergePointClass appends to the reused scratch lists. Native
                    // Append copies the whole lists, retaining repeated prefixes.
                    out.edges.extend_from_slice(&scratch.edges);
                    out.vertices.extend_from_slice(&scratch.vertices);
                    other.merge_point_class_in(
                        a,
                        j,
                        Some(&mut scratch_other.edges),
                        Some(&mut scratch_other.vertices),
                    )?;
                    out_other.edges.extend_from_slice(&scratch_other.edges);
                    out_other
                        .vertices
                        .extend_from_slice(&scratch_other.vertices);
                }
            }
        }
        Ok((out, out_other))
    }

    /// Imprints interval vertices first and edges second; `vertices_only` skips pass two.
    /// The upstream default is false. The debug-only original face argument is omitted.
    ///
    /// # Errors
    /// Unfinished topology merging defers when an interval requires graph mutation.
    pub fn merge_intervals(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        vertices_only: bool,
    ) -> Result<MergedTopology<O>> {
        self.merge_intervals_with_partial(k, geometry, vertices_only)
            .map_err(|e| e.error)
    }

    /// The merge status and completed-interval output, including on failure.
    /// Callers that ignore the native status must still consume its partial lists.
    pub(crate) fn merge_intervals_with_partial(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        vertices_only: bool,
    ) -> Result<MergedTopology<O>, PartialMergeError<O>> {
        self.merge_intervals_using(vertices_only, |c, j, v| {
            Self::p5_merge_interval(c, k, geometry, j, v)
        })
    }

    /// [`CurveClassification::merge_intervals`] editing the B-reps behind the
    /// classification's topology handles ([`CurveClassification::merge_interval_mut`]).
    ///
    /// # Errors
    /// Those of [`CurveClassification::merge_intervals`] and of the B-rep edits.
    pub fn merge_intervals_mut(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        vertices_only: bool,
    ) -> Result<MergedTopology<O>> {
        self.merge_intervals_mut_with_partial(k, geometry, vertices_only)
            .map_err(|e| e.error)
    }

    /// [`CurveClassification::merge_intervals_mut`] with the completed-interval output
    /// also on failure (upstream fills its output lists as the intervals merge).
    pub(crate) fn merge_intervals_mut_with_partial(
        &mut self,
        k: &mut Kernel,
        geometry: &mut impl MergeIntervalBrep<O>,
        vertices_only: bool,
    ) -> Result<MergedTopology<O>, PartialMergeError<O>> {
        let mut a = MutableImprint { k, g: geometry };
        self.merge_intervals_using(vertices_only, |c, j, v| {
            Self::p5_merge_interval_in(c, &mut a, j, v, FilletPeer::Metadata(None))
        })
    }

    fn merge_intervals_using(
        &mut self,
        vertices_only: bool,
        mut merge: impl FnMut(&mut Self, usize, bool) -> Result<MergedTopology<O>>,
    ) -> Result<MergedTopology<O>, PartialMergeError<O>> {
        let mut out = MergedTopology::default();
        let status = (|| {
            for pass in 0..if vertices_only { 1 } else { 2 } {
                for j in 0..self.size() {
                    let ivl = self.at(j)?;
                    if ivl.start.object().is_none() && ivl.end.object().is_none() {
                        continue;
                    }
                    let merged = merge(self, j, pass == 0)?;
                    out.faces.extend(merged.faces);
                    out.edges.extend(merged.edges);
                    out.vertices.extend(merged.vertices);
                }
            }
            Ok(())
        })();
        match status {
            Ok(()) => Ok(out),
            Err(error) => Err(PartialMergeError {
                error,
                merged: Box::new(out),
            }),
        }
    }
}

/// Converts sub-tolerance ranges to points and inserts curve/edge/plane intersections,
/// over a read-only adapter: where an edge-end vertex tolerance would be raised to the
/// edge's (`SmVertex::SetTolerance`), this defers ([`insert_solutions_mut`] raises it).
///
/// `xsect_tolerance` defaults to the sum of source and target zones. Solutions are
/// modified in place when a range collapses; their end records are preserved.
///
/// # Errors
/// Invalid target kinds fail. Opaque edge topology, unavailable insertion callees and
/// the tolerance edit defer. Native short-range tangency filtering without an edge
/// dereferences a null pointer; Rust returns an invalid-input error for that branch.
pub fn insert_solutions<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &mut CurveClassification<O>,
    solutions: &mut [Solution<O>],
    object: O,
    xsect_tolerance: Option<f64>,
) -> Result<()> {
    insert_solutions_in(
        k,
        &mut ReadOnly(geometry),
        classification,
        solutions,
        object,
        xsect_tolerance,
    )
}

/// [`insert_solutions`] raising an edge-end vertex's tolerance to the edge's when the
/// classification allows it (`GetOKToSetTolerance`), as upstream.
///
/// # Errors
/// As [`insert_solutions`], without the deferral of the edit; a vertex the adapter
/// cannot resolve mutably is invalid input.
pub fn insert_solutions_mut<O: ClassObject>(
    k: &Kernel,
    geometry: &mut impl ClassGeometryMut<O>,
    classification: &mut CurveClassification<O>,
    solutions: &mut [Solution<O>],
    object: O,
    xsect_tolerance: Option<f64>,
) -> Result<()> {
    insert_solutions_in(
        k,
        &mut Mutable(geometry),
        classification,
        solutions,
        object,
        xsect_tolerance,
    )
}

/// The edge curve of an edge target (`pEdge->GetCurve()`), or the target curve.
fn target_curve<O: ClassObject, G: ClassGeometry<O>>(
    geometry: &G,
    edge: Option<O>,
    object: O,
) -> Result<Option<&Curve>> {
    match edge {
        Some(e) => Ok(Some(
            super::p3::edge_curve(geometry, e)?
                .ok_or(SolidError::invalid_input(OP, "edge without a curve"))?,
        )),
        None => Ok(geometry.curve(object)),
    }
}

fn insert_solutions_in<O: ClassObject>(
    k: &Kernel,
    access: &mut impl Access<O>,
    classification: &mut CurveClassification<O>,
    solutions: &mut [Solution<O>],
    object: O,
    xsect_tolerance: Option<f64>,
) -> Result<()> {
    let geometry = access.geometry();
    classification.curve_geometry(geometry)?;
    let source_zone = src_zone_tol_3d_of(k, Some(classification));
    let target_zone = object_zone_tol_3d(k, geometry, Some(object))?;
    let tol = xsect_tolerance.map_or_else(
        || xsect_tol_3d_from_zones(k, source_zone, target_zone),
        |t| xsect_tol3d(k, t),
    );

    // when given an Edge - the target curve is Edge->Curve
    let edge = (object.class_kind() == ClassObjectKind::Edge).then_some(object);
    let has_target = target_curve(geometry, edge, object)?.is_some();
    let plane = edge.is_none()
        && geometry
            .surface(object)
            .is_some_and(|s| matches!(s.kind(), SurfaceKind::Plane(_)));
    if !has_target && !plane {
        return Err(SolidError::sm_err(OP));
    }
    for sol in solutions {
        collapse_short_range(k, access.geometry(), sol, tol)?;
        if sol.solution_type == SolutionType::SingleValue {
            // Check whether it should have classified to a vertex.
            if let Some(e) = edge {
                if edge_end_vertex_snap(k, access, classification, sol, e, source_zone, tol)? {
                    continue;
                }
            }
            // only try SSS for edge solutions: see if vertex should be placed at a
            // surf/surf/surf intersection (which may edit the curves and the solution)
            let refine = match edge {
                Some(e) => Some(access.refine_cci_to_surfaces(k, classification, sol, e)?),
                None => None,
            };
            let geometry = access.geometry();
            let source = classification.curve_geometry(geometry)?;
            let target = target_curve(geometry, edge, object)?;
            let sss_point_flag = refine.is_some_and(|r| r.sss_point_flag);

            let mut pc = PointClassification::new(source_zone);
            pc.set_class_object(
                if edge.is_some() {
                    PointClassType::Edge
                } else if target.is_some() {
                    PointClassType::Curve
                } else {
                    PointClassType::Surface
                },
                Some(object),
            );
            pc.set_gap_3d(match refine {
                Some(r) if r.sss_point_flag => r.deviation,
                _ => sol.start.solution_value,
            });
            pc.set_src_zone_tol_3d(source_zone);
            pc.set_pre_snap_param(sol.start.param(0)?);
            pc.set_t_param(sol.start.param(1)?, classification.doing_cross_hatch());
            if let (true, Some(r)) = (sss_point_flag, refine) {
                pc.set_sss_point(r.sss_point);
            }
            let mut do_insertion = true;
            if classification.ignore_tangency() {
                if let Some(target) = target {
                    let a = source.evaluate(k, sol.start.param(0)?, 1, true, true)?;
                    let b = target.evaluate(k, sol.start.param(1)?, 1, true, true)?;
                    let ta = a.get(1).copied().ok_or(SolidError::sm_err(OP))?;
                    let tb = b.get(1).copied().ok_or(SolidError::sm_err(OP))?;
                    let mut angle = tb.angle_between(ta).unwrap_or(1.0);
                    if angle > PI / 2.0 {
                        angle = PI - angle;
                    }
                    if angle < 5.0 * PI * ANG_TOL_DEG / 180.0 {
                        do_insertion = false;
                    }
                    if classification
                        .interval()
                        .is_value_on_boundary(sol.start.param(0)?, EFF_ZERO)
                        .0
                    {
                        do_insertion = true;
                    }
                }
            }
            if do_insertion {
                classification.insert_point_class(
                    k,
                    geometry,
                    &pc,
                    sol.start.param(0)?,
                    BIG_DOUBLE,
                    false,
                )?;
            }
        } else if sol.solution_type == SolutionType::RangeOfValues {
            if sol.start.param(0)? > sol.end.param(0)? {
                continue;
            }
            let geometry = access.geometry();
            let target = target_curve(geometry, edge, object)?;
            let mut ivl = CurveInterval::with_interval(
                k,
                &Extent1d::from_min_max(sol.start.param(0)?, sol.end.param(0)?),
                Some(classification),
            );
            // (with an edge the class object is the edge: upstream's 2D-curve case
            // needs a 2D target curve, which an edge target never sets)
            for pc in [&mut ivl.start, &mut ivl.mid, &mut ivl.end] {
                pc.set_class_object(
                    if edge.is_some() {
                        PointClassType::Edge
                    } else if target.is_some() {
                        PointClassType::Curve
                    } else {
                        PointClassType::Surface
                    },
                    Some(object),
                );
            }
            for (pc, end) in [(&mut ivl.start, &sol.start), (&mut ivl.end, &sol.end)] {
                pc.set_gap_3d(end.solution_value);
                pc.set_pre_snap_param(end.param(0)?);
                pc.set_t_param(end.param(1)?, classification.doing_cross_hatch());
            }
            let size_ratio = ivl.interval.length() / classification.interval().length();
            if classification.ignore_tangency() && size_ratio < 0.05 {
                // cancel the scheduled insertion; check for end point intersections and
                // insert them here (upstream maps the tolerance on pEdgeCurve, which a
                // curve or plane target leaves NULL)
                let (Some(_), Some(e_curve)) = (edge, target) else {
                    return Err(SolidError::invalid_input(
                        OP,
                        "native short-range tangency filtering requires an edge",
                    ));
                };
                let cc_ivl = classification.interval();
                let start_tol = map_to_1d_on_curve(k, tol, sol.start.param(0)?, e_curve)?;
                if cc_ivl
                    .is_value_on_boundary(sol.start.param(0)?, start_tol)
                    .0
                {
                    classification.insert_point_class(
                        k,
                        geometry,
                        &ivl.start,
                        sol.start.param(0)?,
                        BIG_DOUBLE,
                        false,
                    )?;
                }
                let end_tol = map_to_1d_on_curve(k, tol, sol.end.param(0)?, e_curve)?;
                if cc_ivl.is_value_on_boundary(sol.end.param(0)?, end_tol).0 {
                    classification.insert_point_class(
                        k,
                        geometry,
                        &ivl.end,
                        sol.end.param(0)?,
                        BIG_DOUBLE,
                        false,
                    )?;
                }
                continue;
            }
            classification.insert_curve_interval(k, geometry, &ivl)?;
        }
    }
    Ok(())
}

/// The vertex check of `sm_InsertSolutions` for an edge target: a point solution within
/// the intersection tolerance of an edge end whose vertex is within the classification
/// curve / vertex tolerance is inserted as a vertex classification (unless that would
/// make a sub-tolerance interval with both ends at the vertex). A vertex tolerance below
/// the edge's is raised to it first when the classification allows it. `true` when the
/// solution is done with (inserted or skipped).
fn edge_end_vertex_snap<O: ClassObject>(
    k: &Kernel,
    access: &mut impl Access<O>,
    classification: &mut CurveClassification<O>,
    sol: &Solution<O>,
    edge: O,
    source_zone: f64,
    xsect_tol: f64,
) -> Result<bool> {
    let geometry = access.geometry();
    let edge_curve = super::p3::edge_curve(geometry, edge)?
        .ok_or(SolidError::invalid_input(OP, "edge without a curve"))?;
    let param = sol.start.param(1)?;
    let edge_dom = super::p3::edge_interval(geometry, edge)?;
    let (eb, e) = super::p7::edge_of(geometry, edge, "SmEdge::GetStartVertex")?;
    let start_vertex = crate::topology::edge::start_vertex(eb, e)?;
    let edge_zone = object_zone_tol_3d(k, geometry, Some(edge))?;
    let param_xsect_tol = map_to_1d_on_curve(k, xsect_tol, edge_dom.min(), edge_curve)?;

    let near_start = (param - edge_dom.min()).abs() < param_xsect_tol
        && (param - edge_dom.min()).abs() < (param - edge_dom.max()).abs();
    let near_end = (param - edge_dom.max()).abs() < param_xsect_tol
        && (param - edge_dom.max()).abs() < (param - edge_dom.min()).abs();

    // if Param is near EdgeIvlStart (then EdgeIvlEnd) avoiding probs with closed curves
    for (near, at_end) in [(near_start, false), (near_end, true)] {
        if !near {
            continue;
        }
        let geometry = access.geometry();
        let (eb, e) = super::p7::edge_of(geometry, edge, "SmEdge::GetOtherVertex")?;
        let vertex = if at_end {
            crate::topology::edge::other_vertex(eb, e, start_vertex)?
        } else {
            start_vertex
        };
        let vertex = vertex.ok_or(SolidError::null_pointer(OP))?;
        let vtx = geometry
            .topology_handle_in(eb, EntityRef::Vertex(vertex))
            .ok_or(SolidError::invalid_input(OP, "vertex without handle"))?;
        let mut vtx_zone = object_zone_tol_3d(k, geometry, Some(vtx))?;

        if vtx_zone < edge_zone {
            if classification.ok_to_set_tolerance() {
                access.set_vertex_tolerance(vtx, edge_zone)?;
            }
            vtx_zone = edge_zone;
        }
        let geometry = access.geometry();
        let edge_curve = super::p3::edge_curve(geometry, edge)?
            .ok_or(SolidError::invalid_input(OP, "edge without a curve"))?;
        let source = classification.curve_geometry(geometry)?;

        // (GetXSectTol3d(src, vtx) at the start, (vtx, src) at the end: the same sum)
        let cc_vtx_xsect_tol = xsect_tol_3d_from_zones(k, source_zone, vtx_zone);

        // check the EdgeEnd/Vertex distance (EvaluatePoint's status is ignored)
        let edge_end_pt = ignored(edge_curve.evaluate_point(k, param))?.unwrap_or(Point3::UNDEF);
        let dist = edge_end_pt.distance(super::p3::vertex_point(geometry, vtx)?);

        // if EdgeEnd/Vertex distance < XSectTol3d(Vtx,ClassCurve) - snap to vertex
        if dist < cc_vtx_xsect_tol {
            // Don't add this vertex as a PointClassification if it would make a
            // sub-tolerant CurveInterval, where both ends are this vertex [B678]
            let touches = classification.point_classifies_connected_to(geometry, &[vtx])?;
            let vtx_vtx_zone = object_zone_tol_3d(k, geometry, Some(vtx))?;
            let xsect_vtx_vtx = xsect_tol_3d_from_zones(k, vtx_vtx_zone, vtx_vtx_zone);
            let s0 = sol.start.param(0)?;
            for touch in &touches {
                let lo = if touch.param < s0 { touch.param } else { s0 };
                let hi = if touch.param > s0 { touch.param } else { s0 };
                if source
                    .approximate_length(k, &Extent1d::from_min_max(lo, hi), 3, false)?
                    .0
                    < xsect_vtx_vtx
                {
                    return Ok(true);
                }
            }

            // Insert a Vertex classification.
            let mut pc = PointClassification::new(source_zone);
            pc.set_class_object(PointClassType::Vertex, Some(vtx));
            pc.set_gap_3d(sol.start.solution_value);
            pc.set_src_zone_tol_3d(source_zone);
            pc.set_pre_snap_param(s0);
            classification.insert_point_class(k, geometry, &pc, s0, BIG_DOUBLE, false)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn collapse_short_range<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    sol: &mut Solution<O>,
    tolerance: f64,
) -> Result<()> {
    if sol.solution_type != SolutionType::RangeOfValues {
        return Ok(());
    }
    // GetPoint(0) advances no parameters. A curve uses the first parameter and
    // reports ordinary evaluation failure through status; that leaves the range intact.
    let Some(handle) = sol.objects.first().copied().flatten() else {
        return Ok(());
    };
    let Some(curve) = geometry.curve(handle) else {
        return Ok(());
    };
    let (Some(start), Some(end)) = (
        ignored(curve.evaluate_point(k, sol.start.param(0)?))?,
        ignored(curve.evaluate_point(k, sol.end.param(0)?))?,
    ) else {
        return Ok(());
    };
    if !start.closer_than(tolerance, end) {
        return Ok(());
    }
    let mut interval = Extent1d::from_value(sol.start.param(0)?);
    interval.add_value(sol.end.param(0)?);
    if curve.approximate_length(k, &interval, 6, false)?.0 < tolerance {
        sol.solution_type = SolutionType::SingleValue;
        if sol.end.solution_value < sol.start.solution_value * 0.8 {
            sol.start.solution_value = sol.end.solution_value;
            for j in 0..2 {
                *sol.start.param_mut(j)? = sol.end.param(j)?;
            }
        } else if sol.end.solution_value < sol.start.solution_value * 1.2 {
            sol.start.solution_value = 0.5 * (sol.start.solution_value + sol.end.solution_value);
            for j in 0..2 {
                *sol.start.param_mut(j)? = 0.5 * (sol.start.param(j)? + sol.end.param(j)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::{BoundedLineOptions, Line};
    use crate::math::Vec3;
    use crate::surface::volume::Volume;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct H;
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Curve
        }
    }
    struct Geometry(Curve);
    impl ClassGeometry<H> for Geometry {
        fn curve(&self, _: H) -> Option<&Curve> {
            Some(&self.0)
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
    fn classification(k: &Kernel, count: usize) -> CurveClassification<H> {
        let mut cc = CurveClassification::new(k);
        cc.set_curve(Some(H), false);
        cc.set_src_zone_tol_3d(1e-6);
        cc.set_size(k, count).unwrap();
        for j in 0..count {
            cc.at_mut(j).unwrap().set_interval(&Extent1d::from_min_max(
                j as f64 / count as f64,
                (j + 1) as f64 / count as f64,
            ));
        }
        cc
    }
    fn geometry() -> Geometry {
        Geometry(Curve::from(Line::bounded(
            Vec3::ZERO,
            Vec3::new(1., 0., 0.),
            &Extent1d::from_min_max(0., 1.),
            &BoundedLineOptions::default(),
        )))
    }

    #[derive(Clone, Copy, PartialEq)]
    enum VertexTestObject {
        Curve,
        Vertex,
        Face,
    }

    impl ClassObject for VertexTestObject {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Curve => ClassObjectKind::Curve,
                Self::Vertex => ClassObjectKind::Vertex,
                Self::Face => ClassObjectKind::Face,
            }
        }
    }

    struct VertexGeometry {
        curve: Curve,
        point: Option<Point3>,
        gap_calls: core::cell::Cell<usize>,
    }

    impl ClassGeometry<VertexTestObject> for VertexGeometry {
        fn curve(&self, object: VertexTestObject) -> Option<&Curve> {
            (object == VertexTestObject::Curve).then_some(&self.curve)
        }
        fn surface(&self, _: VertexTestObject) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: VertexTestObject) -> Option<&Volume> {
            None
        }
        fn point(&self, _: VertexTestObject) -> Option<Point3> {
            None
        }
        fn classification_vertex_point(&self, _: VertexTestObject) -> Option<Point3> {
            self.point
        }
        fn topology_zone_tol_3d(&self, _: VertexTestObject) -> Option<f64> {
            Some(0.0)
        }
        fn vertex_face_gap(
            &self,
            _: VertexTestObject,
            _: &Surface,
            _: Option<VertexTestObject>,
        ) -> Option<f64> {
            self.gap_calls.set(self.gap_calls.get() + 1);
            None
        }
    }

    fn vertex_classification(k: &Kernel) -> CurveClassification<VertexTestObject> {
        let mut cc = CurveClassification::new(k);
        cc.set_curve(Some(VertexTestObject::Curve), false);
        cc.set_interval(&Extent1d::from_min_max(0., 1.)).unwrap();
        cc
    }

    #[test]
    fn unresolved_vertex_point_is_invalid_input() {
        let k = Kernel::default();
        let g = VertexGeometry {
            curve: geometry().0,
            point: None,
            gap_calls: core::cell::Cell::new(0),
        };
        let surface = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::new(0., 0., 1.),
        ));
        let result = vertex_classification(&k).intersect_vertices(
            &k,
            &g,
            &[VertexTestObject::Vertex],
            &surface,
            false,
            true,
        );
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert_eq!(g.gap_calls.get(), 0);
    }

    #[test]
    fn large_gap_adapter_is_only_required_when_a_face_exists() {
        use crate::geom::assert_array::labels::FACE_TYPE;
        use crate::geom::attribute::ObjectKey;
        use crate::geom::object::{ObjectRef, OBJECT_TYPE};

        let k = Kernel::default();
        for face_mode in 0..4 {
            let g = VertexGeometry {
                curve: geometry().0,
                // The vertex is far from the curve, so the ordinary limited solve
                // produces no intersection after the adapter preconditions pass.
                point: Some(Vec3::new(0., 10., 0.)),
                gap_calls: core::cell::Cell::new(0),
            };
            let mut cc = vertex_classification(&k);
            let mut surface = Surface::from(crate::surface::plane::Plane::from_normal(
                Vec3::ZERO,
                Vec3::new(0., 0., 1.),
            ));
            if face_mode == 1 {
                cc.set_face(Some(VertexTestObject::Face));
            } else if face_mode >= 2 {
                surface.set_owner(Some(ObjectRef {
                    type_code: if face_mode == 2 {
                        FACE_TYPE
                    } else {
                        OBJECT_TYPE
                    },
                    key: ObjectKey(1),
                }));
            }
            let result =
                cc.intersect_vertices(&k, &g, &[VertexTestObject::Vertex], &surface, true, true);
            if face_mode == 1 || face_mode == 2 {
                assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
                assert_eq!(g.gap_calls.get(), 1);
            } else {
                assert!(result.is_ok());
                assert_eq!(g.gap_calls.get(), 0);
            }
        }
    }

    #[test]
    fn no_face_large_gap_keeps_default_cutoff() {
        let mut k = Kernel::default();
        let g = VertexGeometry {
            curve: geometry().0,
            point: None,
            gap_calls: core::cell::Cell::new(0),
        };
        let surface = Surface::from(crate::surface::plane::Plane::from_normal(
            Vec3::ZERO,
            Vec3::new(0., 0., 1.),
        ));
        let ordinary = 1e-5;
        let cutoff = vertex_big_gap_tolerance(
            &k,
            &g,
            VertexTestObject::Vertex,
            &surface,
            None,
            true,
            ordinary,
        )
        .unwrap();
        assert_eq!(cutoff, 2e-5);
        assert!(1.5e-5 > ordinary && 1.5e-5 < cutoff);
        assert!(2.5e-5 > cutoff);
        assert_eq!(
            vertex_big_gap_tolerance(
                &k,
                &g,
                VertexTestObject::Vertex,
                &surface,
                None,
                false,
                ordinary,
            )
            .unwrap(),
            ordinary
        );
        k.tolerances.zone_tol_3d = 4e-5;
        assert_eq!(
            vertex_big_gap_tolerance(
                &k,
                &g,
                VertexTestObject::Vertex,
                &surface,
                None,
                true,
                ordinary,
            )
            .unwrap(),
            8e-5
        );
        assert_eq!(g.gap_calls.get(), 0);
    }

    #[test]
    fn read_only_edits_defer_to_the_mutable_owning_context() {
        let g = geometry();
        let mut access = ReadOnly(&g);
        for e in [
            access.set_vertex_tolerance(H, 1e-3).unwrap_err(),
            access.curve_mut(H).map(|_| ()).unwrap_err(),
            access.delete_uv_trim_curve(H).unwrap_err(),
        ] {
            assert!(
                matches!(
                    e,
                    SolidError::Unported {
                        unit: "topology.brep_p1",
                        ..
                    }
                ),
                "{e:?}"
            );
        }
    }

    #[test]
    fn paired_classification_lengths_fail_before_mutation() {
        let k = Kernel::default();
        let g = geometry();
        for other_count in [1, 3] {
            let mut cc = classification(&k, 2);
            let mut other = classification(&k, other_count);
            let before = cc.intervals().to_vec();
            let other_before = other.intervals().to_vec();
            assert!(matches!(
                cc.merge_classifications(&k, &g, &mut other, true),
                Err(SolidError::Upstream { code: 1001, .. })
            ));
            assert_eq!(cc.intervals(), before);
            assert_eq!(other.intervals(), other_before);
        }
        let mut cc = classification(&k, 2);
        let mut other = classification(&k, 2);
        let (a, b) = cc.merge_classifications(&k, &g, &mut other, false).unwrap();
        assert_eq!(a, MergedTopology::default());
        assert_eq!(b, MergedTopology::default());
    }

    #[test]
    fn collapsed_range_keeps_end_and_unrelated_parameters() {
        let k = &Kernel::default();
        let g = geometry();
        for (ratio, expected) in [(0.79, 0.6e-6), (0.8, 0.55e-6), (1.2, 0.5e-6)] {
            let mut sol = Solution::new();
            sol.solution_type = SolutionType::RangeOfValues;
            sol.objects[0] = Some(H);
            sol.num_objects = 1;
            sol.num_variables = 2;
            *sol.start.param_mut(0).unwrap() = 0.5e-6;
            *sol.end.param_mut(0).unwrap() = 0.6e-6;
            *sol.start.param_mut(1).unwrap() = 0.5e-6;
            *sol.end.param_mut(1).unwrap() = 0.6e-6;
            *sol.start.param_mut(2).unwrap() = 42.;
            sol.start.solution_value = 1.;
            sol.end.solution_value = ratio;
            let end = sol.end;
            collapse_short_range(k, &g, &mut sol, 1e-6).unwrap();
            assert_eq!(sol.solution_type, SolutionType::SingleValue);
            assert_eq!(sol.start.param(0).unwrap(), expected);
            assert_eq!(sol.start.param(2).unwrap(), 42.);
            assert_eq!(sol.end, end);
        }
    }
    #[test]
    fn failed_merge_retains_completed_intervals_in_native_visit_order() {
        for vertices_only in [true, false] {
            let k = Kernel::default();
            let mut cc = classification(&k, 3);
            for j in 0..3 {
                cc.at_mut(j)
                    .unwrap()
                    .start
                    .set_class_object(PointClassType::Curve, Some(H));
            }
            let mut visited = Vec::new();
            let failure = cc
                .merge_intervals_using(vertices_only, |_, index, vertex_pass| {
                    visited.push((index, vertex_pass));
                    if index == 1 && (vertices_only || !vertex_pass) {
                        return Err(SolidError::sm_err(OP));
                    }
                    Ok(MergedTopology {
                        faces: vec![H],
                        edges: vec![H, H],
                        vertices: vec![H, H, H],
                    })
                })
                .unwrap_err();
            let completed = if vertices_only { 1 } else { 4 };
            assert!(matches!(
                failure.error,
                SolidError::Upstream { code: 1001, .. }
            ));
            assert_eq!(failure.merged.faces.len(), completed);
            assert_eq!(failure.merged.edges.len(), 2 * completed);
            assert_eq!(failure.merged.vertices.len(), 3 * completed);
            let mut expected = vec![(0, true), (1, true)];
            if !vertices_only {
                expected = vec![(0, true), (1, true), (2, true), (0, false), (1, false)];
            }
            assert_eq!(visited, expected);
        }
    }

    /// Handles of [`a_kept_curve_cache_intersects_vertices_as_fresh_caches`].
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum V {
        Curve,
        Vertex(usize),
        Surface1,
        Surface2,
    }
    impl ClassObject for V {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Curve => ClassObjectKind::Curve,
                Self::Vertex(_) => ClassObjectKind::Vertex,
                Self::Surface1 | Self::Surface2 => ClassObjectKind::Surface,
            }
        }
    }
    struct Vertices {
        curve: Curve,
        points: Vec<Point3>,
        surfaces: [Surface; 2],
    }
    impl ClassGeometry<V> for Vertices {
        fn curve(&self, o: V) -> Option<&Curve> {
            (o == V::Curve).then_some(&self.curve)
        }
        fn surface(&self, o: V) -> Option<&Surface> {
            match o {
                V::Surface1 => self.surfaces.first(),
                V::Surface2 => self.surfaces.get(1),
                _ => None,
            }
        }
        fn volume(&self, _: V) -> Option<&Volume> {
            None
        }
        fn point(&self, _: V) -> Option<Point3> {
            None
        }
        fn classification_vertex_point(&self, o: V) -> Option<Point3> {
            match o {
                V::Vertex(i) => self.points.get(i).copied(),
                _ => None,
            }
        }
        fn topology_zone_tol_3d(&self, _: V) -> Option<f64> {
            Some(1e-4)
        }
        fn vertex_face_gap(&self, _: V, _: &Surface, _: Option<V>) -> Option<f64> {
            None
        }
    }

    /// The classification curve's cache kept across the vertex drops (dropped when a
    /// vertex enters the surf/surf/surf refinement, which may edit the curve) classifies
    /// the vertices as a fresh cache for each drop does.
    #[test]
    fn a_kept_curve_cache_intersects_vertices_as_fresh_caches() {
        use crate::nurbs::b_spline_curve::BSplineCurve;
        use crate::nurbs::math::{CPoint, NOW};
        use crate::nurbs::nurbs_crv::allocate_nurb_curve;
        use crate::surface::plane::Plane;

        let k = Kernel::default();
        // a planar cubic B-spline in z = 0 (not analytic, so the drops read the cache)
        let pts = [[0.0, 0.0], [1.0, 2.0], [3.0, -1.0], [4.0, 1.0], [6.0, 0.0]];
        let mut nurb = allocate_nurb_curve(4, 3, 8).unwrap();
        for (d, s) in nurb.cpts_mut().iter_mut().zip(pts) {
            *d = CPoint::new(s[0], s[1], 0.0, NOW);
        }
        nurb.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
        let curve = Curve::from(BSplineCurve::from_nurb(nurb, 3, false));
        let on = |t: f64| curve.evaluate_point(&k, t).unwrap();
        // vertex 1 lies 1e-6 off the curve (normal to its plane): one drop point in
        // [LOOSE_CRV_DROP_SIZE, tolerance], so the refinement runs (and the slot is
        // dropped); it is 1e-6 off surface 1 and 5 off surface 2, so the refinement
        // makes no edit. Vertices 0 and 3 miss, vertex 2 lies on the curve.
        let points = vec![
            Vec3::new(-20., 30., 0.),
            on(0.3) + Vec3::new(0., 0., 1e-6),
            on(0.7),
            Vec3::new(40., -30., 0.),
        ];
        let plane = |z: f64| Surface::from(Plane::from_normal(Vec3::new(0., 0., z), Vec3::Z));
        let g = Vertices {
            curve,
            points,
            surfaces: [plane(0.), plane(5.)],
        };
        let face_surface = plane(0.);
        let vertices: Vec<V> = (0..4).map(V::Vertex).collect();

        // vertex 1's drop lies in the refinement band of IntersectVertices: one drop
        // point farther than LOOSE_CRV_DROP_SIZE and within the curve+vertex tolerance
        let ivl = Extent1d::from_min_max(0., 1.);
        let tolerance = xsect_tol_3d_from_zones(&k, 1e-6, 1e-4);
        let band = g
            .curve
            .global_point_solve::<NoObject>(
                &k,
                &ivl,
                SolverOperation::Minimize,
                g.points[1],
                tolerance,
                SolutionRequested::All,
                &crate::curve::curve::p1::GlobalPointSolveOptions {
                    target_distance: Some(0.0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(band.len(), 1);
        let gap = band.as_slice().first().unwrap().start.solution_value;
        assert!(
            gap > LOOSE_CRV_DROP_SIZE && gap < tolerance,
            "{gap} {tolerance}"
        );

        let classify = |keep: bool| {
            // one unclassified interval over the curve, as the classifiers start
            let mut cc = CurveClassification::with_curve_interval(&k, V::Curve, ivl, 1e-6);
            cc.set_surface1(Some(V::Surface1));
            cc.set_surface2(Some(V::Surface2));
            let r = cc.intersect_vertices_in(
                &k,
                &mut ReadOnly(&g),
                &vertices,
                &face_surface,
                false,
                true,
                keep,
            );
            format!("{r:?} {:?}", cc.intervals())
        };
        let fresh = classify(false);
        assert!(fresh.starts_with("Ok"), "{fresh}");
        // the near and the on-curve vertex are classified (the refinement made no edit)
        assert!(
            fresh.contains("Vertex(1)") && fresh.contains("Vertex(2)"),
            "{fresh}"
        );
        assert_eq!(classify(true), fresh);
    }
}
