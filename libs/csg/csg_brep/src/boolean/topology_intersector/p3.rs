//! Topology intersector, part 3: merging a pair of curve classifications into the two
//! B-reps (`MergeCurveClasses`), merging a surface/surface intersection curve into the
//! faces of both surfaces (`MergeCurveOnSurfaces`), and making the faces of two
//! coincident surfaces compatible (`MergeCoincidentSurfaces`).
//!
//! Upstream: `source/SMLib/src/SmTopologyIntersector.cpp`,
//! `source/SMLib/inc/SmTopologyIntersector.h`.
//!
//! | Rust                                                  | Upstream                      |
//! |-------------------------------------------------------|-------------------------------|
//! | [`TopologyIntersector::merge_curve_classes`]          | `MergeCurveClasses`           |
//! | [`CurveClassesMerge`]                                 | its out-params                |
//! | [`TopologyIntersector::merge_curve_on_surfaces`]      | `MergeCurveOnSurfaces`        |
//! | [`CurveOnSurfaces`]                                   | its in-params                 |
//! | [`CurveOnSurfacesMerge`]                              | its out-params                |
//! | [`TopologyIntersector::merge_coincident_surfaces`]    | `MergeCoincidentSurfaces`     |
//! | [`CoincidentSurfacesMerge`]                           | its out-params                |
//! | [`IntersectorBrepP3`]                                 | the `SmSurface`, `SmEdge`, `SmLoopuse`, `SmEdgeuse` and `SmFaceuse` walks these use |
//!
//! Deviations:
//! - **Handles and B-reps** as in [`p1`](super::p1): objects are handles `O`, the
//!   B-reps are read and edited through [`IntersectorBrep`] plus [`IntersectorBrepP3`].
//!   `MergeCurveOnSurfaces` takes the handle of the intersector (`this`, passed to
//!   `SetIntersectData`) and, for `SetFilletData`, handles naming its two local curve
//!   classifications ([`CurveOnSurfaces::ti`], [`CurveOnSurfaces::classifications`]).
//! - **Out-params** become return values. `MergeCurveClasses` writes its deleted-edge
//!   and deleted-vertex lists only when it reaches its end; its early returns leave
//!   them untouched, which is `None` here ([`CurveClassesMerge`]). The B-rep's new faces
//!   and vertices of `MergeClassifications` are not asked for (upstream passes `NULL`).
//! - **Temporary curves.** The curve copies, the trimmed coincident curve and the
//!   dropped UV curves that upstream deletes on return (`SmObjDelete`, `SmObjsDelete`)
//!   are stored with [`ProtoBrep::insert_curve`](crate::topology::proto_topology::ProtoBrep::insert_curve) to get a handle; their handles are
//!   returned in `deleted_curves` for the caller to free. On an error return they are
//!   not reported.
//! - **Owners of the curve copies.** As upstream, the edge-curve copy each edge pass merges
//!   gets the edge curve's owner (`pECopy->SetOwner(pEdgeCurve->GetOwner())`, for the
//!   owner's tolerance; set again after the trim on the other B-rep's side), while the
//!   per-UV-curve copies keep none (a copy resets its owner, as `SmCurve::Copy` does).
//!   `SetOwner`'s `SM_NO_CHANGE_OWNER` notification of the temporary copy, which upstream
//!   passes only to the context's notify callbacks, is not delivered: the intersector
//!   hooks carry no notification environment.
//! - **Statuses.** `MergeCurveClasses` turns a failing `FixupClassification`,
//!   `Homogenize`, `CleanupAndValidate` or `FixupClassMates` into `SM_ERR` as upstream,
//!   and a failing `Relate` of two edges into its repair attempt; a failing
//!   `CheckForSqueezeVertices` skips the vertex `Relate`; a failing `FindRadialSector`
//!   or `DropAndTrimCurve` skips the edge. In each case [`SolidError::Unported`] and
//!   [`SolidError::InvalidInput`] propagate instead. Statuses upstream ignores
//!   (`RemoveRelationship` in the stale-mate and edge-repair steps) are ignored through
//!   `ignore_soft`.
//! - **Face queries** (`SmFace::Point3DClassify`, `SmFace::CalculateBoundingBox`, unit
//!   `query.face_p1`) go through [`IntersectorBrepP3`], whose implementation calls
//!   `query::face::p1` on the B-rep behind the handle. `dCurveTolerance` reaches the
//!   point classifications as `SmZoneTol3d` (`zone_tol3d`), as upstream's implicit
//!   conversion does.
//! - **B-rep edits and walks of other units** are [`IntersectorBrepP3`] hooks:
//!   `SmBrep::DeleteEdge` (`topology.brep_p4`) is [`IntersectorBrep::brep_delete_edge`],
//!   `SmFace::GetEdges` [`IntersectorBrep::face_edges`] and `GetUpwardLoopuses` an
//!   [`IntersectorBrepP3`] hook (`topology.face_p4`). `MergeClassifications` and the
//!   degenerate curve's `MergeIntoObject` edit the B-reps through the
//!   [`MergeIntervalBrep`] context [`IntersectorBrepP3`] requires
//!   (`merge_classifications_mut`, `merge_into_object_mut`).
//! - **Undefined behaviour upstream** (a null or deleted object dereferenced, a
//!   classification object of the wrong kind used as an edge or vertex, an index past
//!   a list) is [`SolidError::InvalidInput`].
//! - **Bound.** `MergeCoincidentSurfaces` restarts its edge passes while the merges
//!   change the topology; past [`MAX_COINCIDENT_SURFACE_PASSES`] passes it fails with
//!   `SM_ERR` (upstream is unbounded).
//! - `static int cbiTol = 0` in `MergeCoincidentSurfaces` is a compile-time constant
//!   upstream; only its `0` branch is ported. The `SM_DEBUG_CODE` and
//!   `SM_VALIDATE_TOPOLOGY` blocks are compiled out upstream and not ported.
//! - **Recursion.** None: `MergeCoincidentSurfaces` calls `MergeCurveOnSurfaces`,
//!   which calls `MergeCurveClasses`; none calls back.

use crate::curve::curve::p2::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::SolutionObject;
use crate::geom::tol::zone_tol3d;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p4::ClassificationInit;
use crate::intersect::curve_class::p6::{merge_into_object_mut, MergeIntervalBrep};
use crate::intersect::point_class::{
    ClassObject, ClassObjectKind, PointClassType, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, BIG_DOUBLE};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::SurfaceKind;

use super::p1::{
    add_unique, contains, dangling, ignore_soft, set_edge_class, IntersectorBrep,
    TopologyIntersector,
};
use super::p2::{IntersectorBrepP2, SqueezeVertices};

/// Whether a pair is backed by caller metadata or the actual two local records.
#[derive(Clone, Copy)]
enum PairLink {
    Metadata,
    LocalLinked,
}

struct ClassificationPair<'a, O> {
    first: &'a mut CurveClassification<O>,
    second: &'a mut CurveClassification<O>,
    link: PairLink,
}

const OP_CLASSES: &str = "boolean::topology_intersector::p3::merge_curve_classes";
const OP_ON_SURFACES: &str = "boolean::topology_intersector::p3::merge_curve_on_surfaces";
const OP_COINCIDENT: &str = "boolean::topology_intersector::p3::merge_coincident_surfaces";

/// Most passes over the two faces' edges [`TopologyIntersector::merge_coincident_surfaces`]
/// makes while its merges keep changing the topology; a bound of the port.
pub const MAX_COINCIDENT_SURFACE_PASSES: usize = 1 << 16;

// ---- what part 3 needs from the B-reps beyond part 1 -----------------------------------

/// The B-rep walks of this part, on top of [`IntersectorBrep`], and the mutable B-rep
/// context `MergeClassifications` imprints the classifications through
/// ([`MergeIntervalBrep`]).
pub trait IntersectorBrepP3<O>: IntersectorBrepP2<O> + MergeIntervalBrep<O> {
    /// `SmSurface::GetFace` (`surface::surface::p1`, `Surface::face`): the face that
    /// owns a surface, `None` for none.
    ///
    /// # Errors
    ///
    /// Those of the lookup.
    fn surface_face(&self, surface: O) -> Result<Option<O>>;
    /// `smsurf_GetSurfaceCache(pSurface)` of a face's surface, the cache
    /// `SmSurfaceDropCurve::DoTrace` traces on: the face's trimmed cache, made on its
    /// B-rep (`query::face::p2::face_surface_cache`). `None` (the default) is an
    /// adapter that cannot reach the face's B-rep mutably; the trace then builds the
    /// surface's cache itself.
    ///
    /// # Errors
    ///
    /// Those of the cache lookup.
    fn face_surface_trace_cache(
        &mut self,
        k: &Kernel,
        surface: O,
    ) -> Result<Option<crate::surface::surface_cache::SurfaceCache<O>>>
    where
        O: Copy + PartialEq,
    {
        let _ = (k, surface);
        Ok(None)
    }
    /// `SmEdge::IsManifold` (`topology::edge::is_manifold`).
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn edge_is_manifold(&self, edge: O) -> Result<bool>;
    /// `SmLoopuse::IsVertexLoopuse` (`topology::loopuse::is_vertex_loopuse`).
    ///
    /// # Errors
    ///
    /// Those of the query.
    fn loopuse_is_vertex_loopuse(&self, loopuse: O) -> Result<bool>;
    /// `SmLoopuse::GetEdgeuses` (`topology::loopuse::edgeuses`).
    ///
    /// # Errors
    ///
    /// Those of the walk.
    fn loopuse_edgeuses(&self, loopuse: O) -> Result<Vec<O>>;
    /// `SmEdgeuse::GetEdge`.
    ///
    /// # Errors
    ///
    /// Those of the lookup.
    fn edgeuse_edge(&self, edgeuse: O) -> Result<Option<O>>;
    /// `SmEdgeuse::GetFace`.
    ///
    /// # Errors
    ///
    /// Those of the lookup.
    fn edgeuse_face(&self, edgeuse: O) -> Result<Option<O>>;
    /// `SmFaceuse::GetFace`.
    ///
    /// # Errors
    ///
    /// Those of the lookup.
    fn faceuse_face(&self, faceuse: O) -> Result<Option<O>>;
    /// `SmEdge::FindRadialSector(pClassifyEU, NULL, eRequiredOrientation, rpRadialEU,
    /// rpCoincidentFU)` (`topology::edge::find_radial_sector_between`): the radial edge
    /// use and the coincident face use found, over all regions. Mutable: the
    /// classification may build missing UV trim curves.
    ///
    /// # Errors
    ///
    /// Those of `find_radial_sector` (`SM_ERR` when no sector is unique).
    fn edge_find_radial_sector(
        &mut self,
        k: &Kernel,
        edge: O,
        classify_edgeuse: O,
        required_orientation: OrientType,
    ) -> Result<(Option<O>, Option<O>)>;
    /// `SmFace::CalculateBoundingBox(rBox)` (`bTight = FALSE`; unit `query.face_p1`:
    /// `query::face::p1::calculate_bounding_box`).
    ///
    /// # Errors
    ///
    /// Those of `calculate_bounding_box`; [`SolidError::InvalidInput`] for a handle
    /// that is not a face of a known B-rep.
    fn face_bounding_box(&self, k: &Kernel, face: O) -> Result<Extent3d>;
    /// `SmFace::GetUpwardLoopuses(rLoopuses)` (all loops; unit `topology.face_p4`:
    /// `topology::face::p4::upward_loopuses`).
    ///
    /// # Errors
    ///
    /// Those of `upward_loopuses`; [`SolidError::InvalidInput`] for a handle that is
    /// not a face of a known B-rep.
    fn face_upward_loopuses(&self, face: O) -> Result<Vec<O>>;
}

// ---- small helpers ----------------------------------------------------------------------------

/// A status upstream tests with `!= SM_SUCCESS`: `Ok(true)` for success, `Ok(false)`
/// for a failure, except [`SolidError::Unported`] and [`SolidError::InvalidInput`],
/// which propagate.
fn succeeded(r: Result<()>) -> Result<bool> {
    Ok(ignore_soft(r)?.is_some())
}

/// `if (call != SM_SUCCESS) { return SM_ERR; }`.
fn or_sm_err(r: Result<()>, op: &'static str) -> Result<()> {
    if succeeded(r)? {
        Ok(())
    } else {
        Err(SolidError::sm_err(op))
    }
}

/// The topology tolerance of a handle (`GetTolerance`).
fn tol_of<O: Copy>(brep: &impl IntersectorBrep<O>, topo: O) -> Result<f64> {
    brep.topology_tolerance(topo)
        .ok_or(dangling("topology handle is unknown"))
}

/// The curve behind a handle (a null dereference upstream for none).
fn curve_of<O: Copy>(brep: &impl IntersectorBrep<O>, curve: Option<O>) -> Result<&Curve> {
    curve
        .and_then(|c| brep.curve(c))
        .ok_or(dangling("curve handle cannot be resolved"))
}

/// `pEdge->GetCurve()` and `pEdge->GetInterval()`.
fn edge_curve_interval<O: Copy>(
    brep: &impl IntersectorBrep<O>,
    edge: O,
) -> Result<(&Curve, Extent1d)> {
    let curve_h = brep
        .edge_curve(edge)
        .ok_or(dangling("edge handle is unknown"))?;
    let curve = curve_of(brep, curve_h)?;
    let interval = brep
        .edge_interval(edge)
        .ok_or(dangling("edge handle is unknown"))?;
    Ok((curve, interval))
}

/// `pE->GetCurve()->ApproximateLength(pE->GetInterval(), 5)`.
fn edge_length<O: Copy>(k: &Kernel, brep: &impl IntersectorBrep<O>, edge: O) -> Result<f64> {
    let (curve, interval) = edge_curve_interval(brep, edge)?;
    Ok(curve.approximate_length(k, &interval, 5, false)?.0)
}

/// `(SmEdge*)` / `(SmVertex*)` of a classification object: the object when it is of
/// `kind` (upstream uses whatever is there; another kind or none is undefined).
fn object_of<O: ClassObject>(pc: &PointClassification<O>, kind: ClassObjectKind) -> Result<O> {
    pc.object()
        .filter(|o| o.class_kind() == kind)
        .ok_or(dangling(
            "classification object is not of the expected kind",
        ))
}

/// `pFace->GetSurface()`.
fn face_surface_of<O: Copy>(brep: &impl IntersectorBrep<O>, face: O) -> Result<Option<O>> {
    brep.face_surface(face)
        .ok_or(dangling("face handle is unknown"))
}

/// `sFaces.GetSize() > 1 && sFaces[0]->GetSurface() == sFaces[1]->GetSurface()` for the
/// faces of a manifold edge.
fn faces_share_surface<O: Copy + PartialEq>(
    brep: &impl IntersectorBrep<O>,
    edge: O,
) -> Result<bool> {
    let faces = brep.edge_faces(edge)?;
    if let [f0, f1, ..] = faces.as_slice() {
        Ok(face_surface_of(brep, *f0)? == face_surface_of(brep, *f1)?)
    } else {
        Ok(false)
    }
}

/// The full `SmCurveClassification(cpCurve, crInterval, cpUVCurve, sSrcZoneTol3d)`
/// constructor with its other arguments at their defaults.
fn new_classification<O: ClassObject>(
    k: &Kernel,
    curve: O,
    interval: Extent1d,
    uv_curve: Option<O>,
    src_zone_tol_3d: f64,
) -> Result<CurveClassification<O>> {
    CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(curve),
            interval,
            uv_curve,
            src_zone_tol_3d,
            opt_data_size: 0,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )
}

/// `IsKindOf(SmPlane_TYPE)` of a surface handle.
fn is_plane<O: Copy>(brep: &impl IntersectorBrep<O>, surface: O) -> Result<bool> {
    let s = brep
        .surface(surface)
        .ok_or(dangling("surface handle cannot be resolved"))?;
    Ok(matches!(s.kind(), SurfaceKind::Plane(_)))
}

// ---- outputs and inputs ------------------------------------------------------------------------

/// What [`TopologyIntersector::merge_curve_classes`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveClassesMerge<O> {
    /// `rbTopologyWasDeleted`: edges or vertices were deleted (by squeezing or by the
    /// edge repair).
    pub topology_was_deleted: bool,
    /// `*paDeletedEdges`: `None` when upstream leaves the list untouched (an early
    /// return).
    pub deleted_edges: Option<Vec<O>>,
    /// `*paDeletedVertices`: `None` when upstream leaves the list untouched.
    pub deleted_vertices: Option<Vec<O>>,
}

impl<O> CurveClassesMerge<O> {
    /// The outputs as set on entry (`rbTopologyWasDeleted = FALSE`, lists untouched).
    const fn untouched() -> Self {
        Self {
            topology_was_deleted: false,
            deleted_edges: None,
            deleted_vertices: None,
        }
    }
}

/// The in-params of [`TopologyIntersector::merge_curve_on_surfaces`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveOnSurfaces<O> {
    /// `pSurface`: the 1st surface of the surface/surface intersection (may be `None`
    /// when `brep_edge` is given).
    pub surface: Option<O>,
    /// `pOtherSurface`: the 2nd surface (may be `None` when `other_edge` is given).
    pub other_surface: Option<O>,
    /// `dCurveTolerance`: the shortest non-degenerate curve; a shorter one is treated
    /// as a point.
    pub curve_tolerance: f64,
    /// `p3DCurve`: the 3D intersection curve.
    pub curve: O,
    /// `pUVCurve1`: its optional UV curve on the 1st surface.
    pub uv_curve1: Option<O>,
    /// `pUVCurve2`: its optional UV curve on the 2nd surface.
    pub uv_curve2: Option<O>,
    /// `pBrepEdge`: an existing edge of the B-rep corresponding to the curve.
    pub brep_edge: Option<O>,
    /// `pOtherEdge`: an existing edge of the other B-rep corresponding to the curve.
    pub other_edge: Option<O>,
    /// The intersector's own handle (`this`, passed to `SetIntersectData`).
    pub ti: Option<O>,
    /// Handles naming the two local curve classifications (`&sCrvClass2`,
    /// `&sCrvClass1` passed to `SetFilletData`); read only with `doing_fillets`.
    pub classifications: Option<(O, O)>,
}

/// What [`TopologyIntersector::merge_curve_on_surfaces`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveOnSurfacesMerge<O> {
    /// `rbDeletedTopology`: multiply mated vertices or edges were squeezed by the last
    /// `MergeCurveClasses`.
    pub deleted_topology: bool,
    /// `*paDeletedEdges`: `None` when no `MergeCurveClasses` wrote it.
    pub deleted_edges: Option<Vec<O>>,
    /// `*paDeletedVertices`: `None` when no `MergeCurveClasses` wrote it.
    pub deleted_vertices: Option<Vec<O>>,
    /// The handles of the temporary curves stored for the call (upstream deletes them).
    pub deleted_curves: Vec<O>,
}

/// Status and initialized/partial outputs of an actual linked merge attempt.
#[derive(Debug)]
pub struct CurveOnSurfacesAttempt<O> {
    /// Outputs written before the returned status, including cleanup curve handles.
    pub output: CurveOnSurfacesMerge<O>,
    /// The original status; output ownership is retained on error.
    pub status: Result<()>,
}

impl<O> CurveOnSurfacesMerge<O> {
    fn untouched() -> Self {
        Self {
            deleted_topology: false,
            deleted_edges: None,
            deleted_vertices: None,
            deleted_curves: Vec::new(),
        }
    }

    /// Takes the outputs of one `MergeCurveClasses` call: its flag replaces the last,
    /// its lists replace the last only when it wrote them.
    fn take(&mut self, m: CurveClassesMerge<O>) {
        self.deleted_topology = m.topology_was_deleted;
        if m.deleted_edges.is_some() {
            self.deleted_edges = m.deleted_edges;
        }
        if m.deleted_vertices.is_some() {
            self.deleted_vertices = m.deleted_vertices;
        }
    }
}

/// What [`TopologyIntersector::merge_coincident_surfaces`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoincidentSurfacesMerge<O> {
    /// `rFaces`: the B-rep surface's faces coincident with other-surface faces.
    pub faces: Vec<O>,
    /// `rOFaces`: the other surface's faces coincident with B-rep-surface faces.
    pub other_faces: Vec<O>,
    /// The handles of the temporary curves stored by the call and its merges.
    pub deleted_curves: Vec<O>,
}

/// Which face's edges a pass of `MergeCoincidentSurfaces` merges into the other face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Step 1: the B-rep face's edges into the other face.
    Brep,
    /// Step 2: the other face's edges into the B-rep face.
    Other,
}

impl<O: ClassObject + SolutionObject + Ord> TopologyIntersector<O> {
    // ---- MergeCurveClasses ------------------------------------------------------------------

    /// `MergeCurveClasses(rCrvClass1, rCrvClass2, bMergeSingularities,
    /// rbTopologyWasDeleted, paDeletedEdges, paDeletedVertices)`: fixes up,
    /// synchronises and cleans the two classifications, skips a single interval much
    /// shorter than the edge it is coincident with, merges both into their B-reps, then
    /// rebuilds the mates of the new edges (the nearest edge of the other B-rep within
    /// tolerance) and relates the classifications' end vertices (squeezing vertices
    /// with older mates) and mid edges (deleting an unneeded edge between faces of one
    /// surface when the relation conflicts).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a fix-up, the synchronisation, the clean-up or the class mates
    /// fail, or when a conflicting edge relation cannot be repaired; those of
    /// `MergeClassifications`, the drops, evaluations, `Relate` and `DeleteEdge`
    /// ([`SolidError::Unported`], `topology.brep_p4`); [`SolidError::InvalidInput`] for
    /// classifications of different sizes, a classification object of the wrong kind
    /// or an unknown handle.
    pub fn merge_curve_classes(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        crv_class1: &mut CurveClassification<O>,
        crv_class2: &mut CurveClassification<O>,
        merge_singularities: bool,
    ) -> Result<CurveClassesMerge<O>> {
        self.merge_curve_classes_using(
            k,
            brep,
            crv_class1,
            crv_class2,
            merge_singularities,
            PairLink::Metadata,
        )
    }

    fn merge_curve_classes_using(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        crv_class1: &mut CurveClassification<O>,
        crv_class2: &mut CurveClassification<O>,
        merge_singularities: bool,
        pair: PairLink,
    ) -> Result<CurveClassesMerge<O>> {
        let mut out = CurveClassesMerge::untouched();
        self.merge_curve_classes_into_using(
            k,
            brep,
            ClassificationPair {
                first: crv_class1,
                second: crv_class2,
                link: pair,
            },
            merge_singularities,
            &mut out,
        )?;
        Ok(out)
    }

    fn merge_curve_classes_into_using(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        curves: ClassificationPair<'_, O>,
        merge_singularities: bool,
        out: &mut CurveClassesMerge<O>,
    ) -> Result<()> {
        let ClassificationPair {
            first: crv_class1,
            second: crv_class2,
            link: pair,
        } = curves;
        out.topology_was_deleted = false;

        // For endPointClassifications
        //   1. if endPoint close to edge's vertex change classification from EDGE to VERTEX
        //   2. if endPoint separates a short interval from a long - remove the endPoint,
        //      merge the intervals
        or_sm_err(crv_class1.fixup_classification(k, &*brep), OP_CLASSES)?;
        or_sm_err(crv_class2.fixup_classification(k, &*brep), OP_CLASSES)?;

        // Syncronize the two curve classifications
        or_sm_err(crv_class1.homogenize(k, &*brep, crv_class2), OP_CLASSES)?;

        // Remove all intervals that pass the AreCoincidentMaybe() test -
        //   Short intervals whose mid and end-pt both classify to an edge.
        or_sm_err(
            crv_class1.cleanup_and_validate(k, &*brep, crv_class2),
            OP_CLASSES,
        )?;

        // For every classification PointPair: when one Point classifies to a mated
        // object, verify the other Point classifies to the mate.
        or_sm_err(
            self.fixup_class_mates(k, &*brep, crv_class1, crv_class2),
            OP_CLASSES,
        )?;

        // special case crvClass1 has 1 interval and m_vMid:[EDGE] and either m_vStart or
        // m_vEnd:[EDGE] (curve coincident to EDGE): skip curves much smaller than EDGE
        if crv_class1.size() == 1 {
            let ivl1 = crv_class1.at(0)?;
            let ivl2 = crv_class2.at(0)?;

            // when crvClass1 interval mid and at least one endPoint map to Edge
            if ivl1.mid.point_class() == PointClassType::Edge
                && (ivl1.start.point_class() == PointClassType::Edge
                    || ivl1.end.point_class() == PointClassType::Edge)
            {
                // when either crvClass2 interval endPoints are not vertices
                if ivl2.start.point_class() != PointClassType::Vertex
                    || ivl2.end.point_class() != PointClassType::Vertex
                {
                    // get curve length
                    let curve = curve_of(brep, crv_class1.curve())?;
                    let leng = curve.approximate_length(k, &ivl1.interval, 5, false)?.0;

                    // get the target edge and its length
                    let e = object_of(&ivl1.mid, ClassObjectKind::Edge)?;
                    let leng_edge1 = edge_length(k, brep, e)?;

                    // skip curves which are smaller than the edge
                    if leng < leng_edge1 / 20.0 {
                        return Ok(());
                    }

                    // when interval2 classifies to an edge
                    if ivl2.mid.point_class() == PointClassType::Edge {
                        let e2 = object_of(&ivl2.mid, ClassObjectKind::Edge)?;
                        let leng_edge2 = edge_length(k, brep, e2)?;

                        // skip curves smaller than the edge length
                        if leng < leng_edge2 / 20.0 {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // special case crvClass2 has 1 interval and m_vMid:[EDGE] and either m_vStart or
        // m_vEnd:[EDGE] (curve coincident to EDGE)
        if crv_class2.size() == 1 {
            let ivl1 = crv_class1.at(0)?;
            let ivl2 = crv_class2.at(0)?;

            // when crvClass2 interval mid and at least one endPoint map to Edge
            if ivl2.mid.point_class() == PointClassType::Edge
                && (ivl2.start.point_class() == PointClassType::Edge
                    || ivl2.end.point_class() == PointClassType::Edge)
            {
                // when either crvClass1 interval endPoints are not vertices
                if !(ivl1.start.point_class() == PointClassType::Vertex
                    && ivl1.end.point_class() == PointClassType::Vertex)
                {
                    let curve = curve_of(brep, crv_class2.curve())?;
                    let leng = curve.approximate_length(k, &ivl2.interval, 5, false)?.0;
                    let e = object_of(&ivl2.mid, ClassObjectKind::Edge)?;
                    let leng_edge1 = edge_length(k, brep, e)?;
                    if leng < leng_edge1 / 20.0 {
                        return Ok(());
                    }

                    if ivl1.mid.point_class() == PointClassType::Edge {
                        let e2 = object_of(&ivl1.mid, ClassObjectKind::Edge)?;
                        let leng_edge2 = edge_length(k, brep, e2)?;
                        if leng < leng_edge2 / 20.0 {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Merge results of two classifications into the SmBrep topology graphs.
        //    Only intervals classified to topology in both SmBreps will be inserted.
        let (merged, merged_other) = match pair {
            PairLink::Metadata => {
                crv_class1.merge_classifications_mut(k, brep, crv_class2, merge_singularities)?
            }
            PairLink::LocalLinked => crv_class1.merge_classifications_linked_mut(
                k,
                brep,
                crv_class2,
                merge_singularities,
            )?,
        };
        let new_edges = merged.edges;
        let new_edges_other = merged_other.edges;

        // Rebuild stale mate relationships for new edges (a split edge is reused as one
        // of the new children edges; it may have an old mapping that is no longer
        // valid). First remove any existing relationships from all new edges.
        for ii in 0..new_edges.len() + new_edges_other.len() {
            // look for a ThisEdge<->OtherEdge mapping for current target edge
            let (new_e, other_new_e) =
                self.new_edge_pair(ii, &new_edges, &new_edges_other, true)?;

            // when old pNewE<->pOtherNewE mapping exists remove it (status unchecked)
            if new_e.is_some() && other_new_e.is_some() {
                ignore_soft(self.remove_relationship(new_e, other_new_e))?;
            }
        }

        // Now update Relationships on all new edges.
        for ii in 0..new_edges.len() + new_edges_other.len() {
            // look for a ThisNewEdge<->OtherNewEdge mappings
            let (new_e, other_new_e) =
                self.new_edge_pair(ii, &new_edges, &new_edges_other, false)?;

            // skip NewE/OtherNewE referenced in the classifications - they're handled later
            let ref1 = crv_class1.is_referenced(new_e);
            let ref2 = crv_class2.is_referenced(other_new_e);
            if ref1 || ref2 {
                continue;
            }

            // skip OtherNewE when it already has a mapping
            if new_e.is_some() && other_new_e.is_some() {
                continue;
            }

            // make new mapping between ThisNewEdge and OtherNewEdge nearest in 3D space
            // to ThisNewEdge
            if let Some(ne) = new_e {
                let best = self.nearest_new_edge(k, brep, ne, &new_edges_other, crv_class2)?;
                if let Some(best_o_edge) = best {
                    self.relate(k, brep, Some(ne), Some(best_o_edge))?;
                }
            }

            // make new mapping between OtherEdge and ThisEdge nearest in 3D space to
            // OtherEdge
            if let Some(one) = other_new_e {
                let best = self.nearest_new_edge(k, brep, one, &new_edges, crv_class1)?;
                if let Some(best_edge) = best {
                    self.relate(k, brep, Some(best_edge), Some(one))?;
                }
            }
        }

        // Now update relationships between CurveClassification topology objects
        let mut squeezed = SqueezeVertices::default();
        let mut did_last_end = false;
        let mut squeeze_ok = true;

        // for every classification interval
        for ii in 0..crv_class1.size() {
            // See about relating Starts: Vertices.
            let start_both_vertices = crv_class1.at(ii)?.start.point_class()
                == PointClassType::Vertex
                && crv_class2.at(ii)?.start.point_class() == PointClassType::Vertex;
            if start_both_vertices && !did_last_end {
                let v1 = Self::surviving_vertex(&squeezed, &mut crv_class1.at_mut(ii)?.start)?;
                let v2 = Self::surviving_vertex(&squeezed, &mut crv_class2.at_mut(ii)?.start)?;

                // when both this and other Brep are present (the usual case): if
                // vertices have old mates - squeeze old and new mates
                if self.brep.is_some() && self.other.is_some() {
                    squeeze_ok = succeeded(self.check_for_squeeze_vertices_into(
                        k,
                        brep,
                        v1,
                        v2,
                        &mut squeezed,
                    ))?;
                }

                // remember if any edges were deleted by squeeze
                if !squeezed.deleted_edges.is_empty() {
                    out.topology_was_deleted = true;
                }

                // create the mate relationship between the two vertices
                if squeeze_ok {
                    self.relate(k, brep, Some(v1), Some(v2))?;
                }
            }

            // See about relating Mids: Edges.
            let mid_both_edges = crv_class1.at(ii)?.mid.point_class() == PointClassType::Edge
                && crv_class2.at(ii)?.mid.point_class() == PointClassType::Edge;
            if mid_both_edges {
                let e1 = object_of(&crv_class1.at(ii)?.mid, ClassObjectKind::Edge)?;
                let e2 = object_of(&crv_class2.at(ii)?.mid, ClassObjectKind::Edge)?;

                // when neither edge has been deleted
                if !contains(&squeezed.deleted_edges, &e1)
                    && !contains(&squeezed.deleted_edges, &e2)
                {
                    // create the Edge/Edge mate relationship
                    if !succeeded(self.relate(k, brep, Some(e1), Some(e2)))? {
                        self.repair_edge_relation(k, brep, e1, e2, &mut squeezed, out)?;
                    }
                }
            }

            // See about relating Ends: Vertices.
            let end_both_vertices = crv_class1.at(ii)?.end.point_class() == PointClassType::Vertex
                && crv_class2.at(ii)?.end.point_class() == PointClassType::Vertex;
            if end_both_vertices {
                let v1 = Self::surviving_vertex(&squeezed, &mut crv_class1.at_mut(ii)?.end)?;
                let v2 = Self::surviving_vertex(&squeezed, &mut crv_class2.at_mut(ii)?.end)?;

                // when both Brep and OtherBrep are present (the usual case)
                if self.brep.is_some() && self.other.is_some() {
                    squeeze_ok = succeeded(self.check_for_squeeze_vertices_into(
                        k,
                        brep,
                        v1,
                        v2,
                        &mut squeezed,
                    ))?;
                }

                // remember if edges were deleted by Squeeze
                if !squeezed.deleted_edges.is_empty() || !squeezed.deleted_vertices.is_empty() {
                    out.topology_was_deleted = true;
                }

                // create interval-end vertex/vertex mate relationship
                if squeeze_ok {
                    self.relate(k, brep, Some(v1), Some(v2))?;
                }

                // remember the event
                did_last_end = true;
            } else {
                // remember not creating interval-end vertex/vertex mate relationship
                did_last_end = false;
            }
        }

        // Pass back deleted topology.
        out.deleted_edges = Some(squeezed.deleted_edges);
        out.deleted_vertices = Some(squeezed.deleted_vertices);
        Ok(())
    }

    fn capture_curve_classes(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        curves: ClassificationPair<'_, O>,
        merge_singularities: bool,
        out: &mut CurveOnSurfacesMerge<O>,
    ) -> Result<()> {
        let mut output = CurveClassesMerge::untouched();
        let status =
            self.merge_curve_classes_into_using(k, brep, curves, merge_singularities, &mut output);
        out.take(output);
        status
    }

    /// The (`pNewE`, `pOtherNewE`) pair at index `ii` of the two mate-rebuilding loops
    /// over the new edges of both B-reps: a B-rep edge with its other mate
    /// (`mate_brep_edges`) or with none, then each other edge with its B-rep mate. The
    /// mates are looked up when the loop reaches the index, after the edits of the
    /// earlier indices, as upstream.
    fn new_edge_pair(
        &self,
        ii: usize,
        new_edges: &[O],
        new_edges_other: &[O],
        mate_brep_edges: bool,
    ) -> Result<(Option<O>, Option<O>)> {
        if let Some(&e) = new_edges.get(ii) {
            let other = if mate_brep_edges {
                self.other_mate(Some(e))
            } else {
                None
            };
            Ok((Some(e), other))
        } else {
            let o = *ii
                .checked_sub(new_edges.len())
                .and_then(|j| new_edges_other.get(j))
                .ok_or(dangling("new edge index past the lists"))?;
            Ok((self.brep_mate(Some(o)), Some(o)))
        }
    }

    /// The edge of `candidates` (the new edges of the B-rep that `edge` is not in) whose
    /// middle drops closest onto `edge`, skipping those referenced in `candidates_class`
    /// and those whose drop from `edge`'s middle is farther than the best so far; `None`
    /// when the best is farther than the sum of the two edge tolerances (`[B279]`).
    fn nearest_new_edge(
        &self,
        k: &Kernel,
        brep: &impl IntersectorBrepP3<O>,
        edge: O,
        candidates: &[O],
        candidates_class: &CurveClassification<O>,
    ) -> Result<Option<O>> {
        // evaluate edge->MidPoint
        let (curve, ivl) = edge_curve_interval(brep, edge)?;
        let mut best_dist = BIG_DOUBLE;
        let mut best_edge = None;
        let mid_pnt = curve.evaluate_point(k, ivl.evaluate(0.5))?;

        // for all candidate edges - find the closest to the midPoint
        for &cand in candidates {
            // skip edges referenced in the classification
            if candidates_class.is_referenced(Some(cand)) {
                continue;
            }

            // drop edge->midPoint to candidate->Curve
            let (cand_curve, cand_ivl) = edge_curve_interval(brep, cand)?;
            let dist_tol = 10.0 * (self.this_approx_tol_3d + tol_of(brep, cand)?);
            let drop = cand_curve.drop_point(
                k,
                &cand_ivl,
                mid_pnt,
                None,
                dist_tol,
                None,
                SolverOperation::Minimize,
            )?;

            // skip candidates too far away
            if !drop.success || drop.distance > best_dist {
                continue;
            }

            // drop the candidate->MidPoint back onto edge->Curve
            let mid_pnt2 = cand_curve.evaluate_point(k, cand_ivl.evaluate(0.5))?;
            let drop2 = curve.drop_point(
                k,
                &ivl,
                mid_pnt2,
                None,
                dist_tol,
                None,
                SolverOperation::Minimize,
            )?;

            // when candidate->MidPoint is closer to edge than all others
            if drop2.success && drop2.distance < best_dist {
                best_edge = Some(cand);
                best_dist = drop2.distance;
            }
        }

        // If the closest found edge is not within tolerance, do not relate it.  [B279]
        if let Some(best) = best_edge {
            if best_dist > tol_of(brep, edge)? + tol_of(brep, best)? {
                best_edge = None;
            }
        }
        Ok(best_edge)
    }

    /// Replaces a deleted vertex of a classification end by its surviving vertex
    /// (`sDeletedV.FindElement`, `SetClassObject(SM_PC_VERTEX, ..)`) and returns the
    /// end's vertex.
    fn surviving_vertex(
        squeezed: &SqueezeVertices<O>,
        pc: &mut PointClassification<O>,
    ) -> Result<O> {
        let mut v = object_of(pc, ClassObjectKind::Vertex)?;
        if let Some(found) = squeezed.deleted_vertices.iter().position(|&d| d == v) {
            v = *squeezed
                .surviving_vertices
                .get(found)
                .ok_or(dangling("deleted vertex without survivor"))?;
            pc.set_class_object(PointClassType::Vertex, Some(v));
        }
        Ok(v)
    }

    /// The repair of a failed `Relate(pE1, pE2)` in `MergeCurveClasses`: when exactly
    /// one of the two edges is mated, an unneeded manifold edge between two faces of
    /// one surface (the unmated edge, else the mated edge's mate) is deleted, after
    /// which the mated edge's mate is replaced by the new pair.
    fn repair_edge_relation(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        e1: O,
        e2: O,
        squeezed: &mut SqueezeVertices<O>,
        out: &mut CurveClassesMerge<O>,
    ) -> Result<()> {
        // when pE2 has a mate but pE1 does not - try to fix things by removing unneeded
        // edges
        if self.brep_mate(Some(e2)).is_some() && self.other_mate(Some(e1)).is_none() {
            let brep_h = brep
                .edge_brep_handle(e1)?
                .ok_or(dangling("edge without B-rep"))?;
            let mut fixed = false;

            // when pE1 is manifold and both its faces share the same surface
            if brep.edge_is_manifold(e1)? && faces_share_surface(brep, e1)? {
                // delete the unneeded edge
                fixed = true;
                squeezed.deleted_edges.push(e1);
                brep.brep_delete_edge(k, brep_h, e1)?;
                out.topology_was_deleted = true;
            }

            // when there is still a problem
            if !fixed {
                // when pE2 mate's is manifold and its faces share the same surface
                let e1_other = self.brep_mate(Some(e2));
                if let Some(e1o) = e1_other {
                    if brep.edge_is_manifold(e1o)? && faces_share_surface(brep, e1o)? {
                        // remove the unneeded edge
                        fixed = true;
                        ignore_soft(self.remove_relationship(Some(e1o), Some(e2)))?;
                        squeezed.deleted_edges.push(e1o);
                        brep.brep_delete_edge(k, brep_h, e1o)?;
                        out.topology_was_deleted = true;
                        self.relate(k, brep, Some(e1), Some(e2))?;
                    }
                }
            }

            // if there is still a problem - signal an error
            if !fixed {
                return Err(SolidError::sm_err(OP_CLASSES));
            }
        }
        // when pE1 has a mate but pE2 does not - try to fix things by deleting unneeded
        // edges
        else if self.brep_mate(Some(e2)).is_none() && self.other_mate(Some(e1)).is_some() {
            let brep_h = brep
                .edge_brep_handle(e2)?
                .ok_or(dangling("edge without B-rep"))?;
            let mut fixed = false;
            if brep.edge_is_manifold(e2)? && faces_share_surface(brep, e2)? {
                fixed = true;
                squeezed.deleted_edges.push(e2);
                brep.brep_delete_edge(k, brep_h, e2)?;
                out.topology_was_deleted = true;
            }
            if !fixed {
                // (upstream asks the B-rep mate of the B-rep edge pE1)
                let e2_other = self.brep_mate(Some(e1));
                if let Some(e2o) = e2_other {
                    if brep.edge_is_manifold(e2o)? && faces_share_surface(brep, e2o)? {
                        fixed = true;
                        ignore_soft(self.remove_relationship(Some(e1), Some(e2o)))?;
                        squeezed.deleted_edges.push(e2o);
                        brep.brep_delete_edge(k, brep_h, e2o)?;
                        out.topology_was_deleted = true;
                        self.relate(k, brep, Some(e1), Some(e2))?;
                    }
                }
            }
            if !fixed {
                return Err(SolidError::sm_err(OP_CLASSES));
            }
        }
        Ok(())
    }

    // ---- MergeCurveOnSurfaces ---------------------------------------------------------------

    /// `MergeCurveOnSurfaces(pSurface, pOtherSurface, dCurveTolerance, p3DCurve,
    /// pUVCurve1, pUVCurve2, pBrepEdge, pOtherEdge, rbDeletedTopology, paDeletedEdges,
    /// paDeletedVertices)`: inserts edges for every segment of the curve contained in
    /// both faces of the two surfaces (or, for a degenerate curve, relates the vertices
    /// its point merges into). A given edge stands for the curve on its side instead of
    /// classifying against the face; when the curve runs along two partially
    /// coincident edges, the coincident part is merged first and the curve classified
    /// again.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a surface without face; `SM_ERR` for a degenerate
    /// curve without both surfaces or a negative partial-coincidence gap; those of the
    /// classifications, `Point3DClassify` ([`SolidError::Unported`], `query.face_p1`),
    /// `MergeIntoObject`, `Relate` and [`TopologyIntersector::merge_curve_classes`];
    /// [`SolidError::InvalidInput`] for an unknown handle, a missing face where one is
    /// used, or `doing_fillets` without classification handles.
    pub fn merge_curve_on_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
    ) -> Result<CurveOnSurfacesMerge<O>> {
        self.merge_curve_on_surfaces_using(k, brep, input, PairLink::Metadata)
    }

    /// Merge using the two actual local classification records as a linked pair.
    /// No external classification labels or address registry are required.
    /// # Errors
    /// The original classification/merge errors, retaining their mutation order.
    pub fn merge_curve_on_surfaces_linked(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
    ) -> Result<CurveOnSurfacesMerge<O>> {
        self.merge_curve_on_surfaces_using(k, brep, input, PairLink::LocalLinked)
    }

    fn merge_curve_on_surfaces_using(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
        pair: PairLink,
    ) -> Result<CurveOnSurfacesMerge<O>> {
        let attempt = self.merge_curve_on_surfaces_attempt_using(k, brep, input, pair);
        attempt.status?;
        Ok(attempt.output)
    }

    /// A linked merge preserving output and temporary-curve ownership on error.
    /// The two local classifications are the actual linked pair, without external labels.
    pub fn merge_curve_on_surfaces_linked_attempt(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
    ) -> CurveOnSurfacesAttempt<O> {
        self.merge_curve_on_surfaces_attempt_using(k, brep, input, PairLink::LocalLinked)
    }

    fn merge_curve_on_surfaces_attempt_using(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
        pair: PairLink,
    ) -> CurveOnSurfacesAttempt<O> {
        let mut output = CurveOnSurfacesMerge::untouched();
        let status = self.merge_curve_on_surfaces_into_using(k, brep, input, pair, &mut output);
        CurveOnSurfacesAttempt { output, status }
    }

    fn merge_curve_on_surfaces_into_using(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
        pair: PairLink,
        out: &mut CurveOnSurfacesMerge<O>,
    ) -> Result<()> {
        let both_edges_null = input.brep_edge.is_none() && input.other_edge.is_none();

        // Get the face or composite face from the surface.
        let this_face = match input.surface {
            Some(s) => Some(
                brep.surface_face(s)?
                    .ok_or(SolidError::null_pointer(OP_ON_SURFACES))?,
            ),
            None => None,
        };
        let other_face = match input.other_surface {
            Some(s) => Some(
                brep.surface_face(s)?
                    .ok_or(SolidError::null_pointer(OP_ON_SURFACES))?,
            ),
            None => None,
        };

        // when 3D curve is short
        let natural = curve_of(brep, Some(input.curve))?.natural_interval();
        if curve_of(brep, Some(input.curve))?.is_degenerate(k, input.curve_tolerance, None)? {
            // Don't allow degenerate cases when using edges
            if input.surface.is_none() || input.other_surface.is_none() {
                return Err(SolidError::sm_err(OP_ON_SURFACES));
            }

            // get curve point
            let pnt = curve_of(brep, Some(input.curve))?.evaluate_point(k, natural.min())?;

            // classify point against both faces (in/out trimmed face region)
            // (dCurveTolerance converts to SmZoneTol3d at both)
            let curve_zone_tol = zone_tol3d(k, input.curve_tolerance);
            let mut point_class1 = PointClassification::new(curve_zone_tol);
            let mut point_class2 = PointClassification::new(curve_zone_tol);
            let tf = this_face.ok_or(dangling("no face"))?;
            let of = other_face.ok_or(dangling("no other face"))?;
            brep.face_point_3d_classify(k, tf, pnt, curve_zone_tol, true, &mut point_class1, None)?;
            brep.face_point_3d_classify(k, of, pnt, curve_zone_tol, true, &mut point_class2, None)?;

            // when point was classified on both faces
            if point_class1.point_class() != PointClassType::Unknown
                && point_class2.point_class() != PointClassType::Unknown
            {
                // merge the point classification (not the curve) onto both faces
                merge_into_object_mut(k, brep, &mut point_class1, pnt)?;
                merge_into_object_mut(k, brep, &mut point_class2, pnt)?;

                // each merge creates a vertex in its SmBrep - relate those vertices
                let v1 = point_class1.object();
                let v2 = point_class2.object();
                self.relate(k, brep, v1, v2)?;
            }
            return Ok(());
        }

        // next: Classify curve against thisFace (get curve segments inside trimmed face
        // boundaries)
        let mut crv_class1 = new_classification(
            k,
            input.curve,
            natural,
            input.uv_curve1,
            input.curve_tolerance,
        )?;
        let mut crv_class2 = new_classification(
            k,
            input.curve,
            natural,
            input.uv_curve2,
            input.curve_tolerance,
        )?;

        // when asked - set up fillet data
        if self.doing_fillets {
            match pair {
                PairLink::Metadata => {
                    let (h1, h2) = input
                        .classifications
                        .ok_or(dangling("fillet merge without classification handles"))?;
                    crv_class1.set_fillet_data(Some(h2), true);
                    crv_class2.set_fillet_data(Some(h1), true);
                }
                PairLink::LocalLinked => {
                    // The actual opposite record is borrowed by the linked merge below.
                    crv_class1.set_fillet_data(None, true);
                    crv_class2.set_fillet_data(None, true);
                }
            }
        }

        // Assume this is an intersection curve only when both pBrepEdge and pOtherEdge
        // are NULL (GWC:CHANGE_SSS)
        if both_edges_null {
            crv_class1.set_intersect_data(
                input.surface,
                input.other_surface,
                input.uv_curve2,
                true,
                input.ti,
            );
        } else {
            crv_class1.set_surface1(input.surface);
        }

        if self.ignore_tangency {
            crv_class1.set_ignore_tangency(true);
        }

        // when curve is coincident with an existing brep edge
        if let Some(be) = input.brep_edge {
            // Set sCrvClass1 as such {m_vStart=VERTEX, m_vMid=EDGE, m_vEnd=VERTEX}
            set_edge_class(k, &*brep, &mut crv_class1, be)?;
        } else {
            // Intersect this curve with the topology objects of this face.
            let tf = this_face.ok_or(dangling("no face"))?;
            brep.face_curve_on_classify_mut(k, tf, true, &mut crv_class1)?;
        }

        // If classification found no intersections, quit.
        if Self::found_nothing(&crv_class1)? {
            return Ok(());
        }

        // next: Classify the curve against otherFace
        if both_edges_null {
            crv_class2.set_intersect_data(
                input.other_surface,
                input.surface,
                input.uv_curve1,
                crv_class1.is_curve_current(),
                input.ti,
            );
        } else {
            crv_class2.set_surface1(input.other_surface);
        }

        if self.ignore_tangency {
            crv_class2.set_ignore_tangency(true);
        }

        // when curve is coincident with an existing brep edge
        if let Some(oe) = input.other_edge {
            // reuse the edge
            set_edge_class(k, &*brep, &mut crv_class2, oe)?;
        } else {
            // Intersect this curve with the topology objects of this OtherFace.
            let of = other_face.ok_or(dangling("no other face"))?;
            brep.face_curve_on_classify_mut(k, of, true, &mut crv_class2)?;

            // make sure the obsolete bit is the same in both classifications
            crv_class1.set_is_curve_current(crv_class2.is_curve_current());
        }

        // If classification found no intersection, quit.
        if Self::found_nothing(&crv_class2)? {
            return Ok(());
        }

        // when XSect curve lies along two edges from two faces that are partially
        // coincident - make a new curve that is the coincident sub-interval
        let partial = crv_class1.test_for_partial_coincident_edges(k, &*brep, &crv_class2)?;
        let new_tol = partial.max_partial_coin_gap;
        if new_tol < 0.0 {
            return Err(SolidError::sm_err(OP_ON_SURFACES));
        }

        // when a coincident sub-interval curve was made
        if let Some(new_bsc) = partial.new_curve_to_test {
            let new_curve = brep
                .insert_curve(Curve::from(new_bsc))
                .ok_or(dangling("curve store is full"))?;
            // (sCleanBSC: the curve is deleted on return)
            out.deleted_curves.push(new_curve);

            if input.surface.is_some() && input.other_surface.is_some() {
                self.merge_partial_coincidence(
                    k,
                    brep,
                    input,
                    new_curve,
                    new_tol,
                    &crv_class1,
                    out,
                )?;
                return Ok(());
            }
        }

        // arrive here after curve is classified against both Faces
        // 1. Create and insert an SmEdge into the Brep and the OBrep topology graphs for
        //    each interval classified to geometry in both Breps.
        // 2. Create a mate relationship between the objects of every matching end-point
        //    and mid-point CurveClassification.
        self.capture_curve_classes(
            k,
            brep,
            ClassificationPair {
                first: &mut crv_class1,
                second: &mut crv_class2,
                link: pair,
            },
            false,
            out,
        )?;
        Ok(())
    }

    /// `GetSize() == 1` and the single interval's start, mid and end all
    /// `SM_PC_UNKNOWN`: the classification found no intersections.
    fn found_nothing(cc: &CurveClassification<O>) -> Result<bool> {
        if cc.size() != 1 {
            return Ok(false);
        }
        let ivl = cc.at(0)?;
        Ok(ivl.start.point_class() == PointClassType::Unknown
            && ivl.mid.point_class() == PointClassType::Unknown
            && ivl.end.point_class() == PointClassType::Unknown)
    }

    /// The partial-coincidence branch of `MergeCurveOnSurfaces`: classifies the
    /// coincident sub-interval curve against both faces and merges it, then classifies
    /// the whole curve against both faces a second time and merges that.
    #[allow(clippy::too_many_arguments)] // one upstream block over its locals
    fn merge_partial_coincidence(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        input: &CurveOnSurfaces<O>,
        new_curve: O,
        new_tol: f64,
        crv_class1: &CurveClassification<O>,
        out: &mut CurveOnSurfacesMerge<O>,
    ) -> Result<()> {
        let both_edges_null = input.brep_edge.is_none() && input.other_edge.is_none();
        let this_face = match input.surface {
            Some(s) => brep.surface_face(s)?,
            None => None,
        }
        .ok_or(dangling("no face"))?;
        let other_face = match input.other_surface {
            Some(s) => brep.surface_face(s)?,
            None => None,
        }
        .ok_or(dangling("no other face"))?;

        // classify the coincident curve against face1
        let new_natural = curve_of(brep, Some(new_curve))?.natural_interval();
        let mut new_class1 = new_classification(k, new_curve, new_natural, None, new_tol)?;
        if both_edges_null {
            new_class1.set_intersect_data(
                input.surface,
                input.other_surface,
                input.uv_curve2,
                true,
                input.ti,
            );
        } else {
            new_class1.set_surface1(input.surface);
        }
        brep.face_curve_on_classify_mut(k, this_face, true, &mut new_class1)?;

        // classify the coincident curve against face2
        let mut new_class2 = new_classification(k, new_curve, new_natural, None, new_tol)?;
        if both_edges_null {
            new_class2.set_intersect_data(
                input.other_surface,
                input.surface,
                input.uv_curve1,
                crv_class1.is_curve_current(),
                input.ti,
            );
        } else {
            new_class2.set_surface1(input.other_surface);
        }
        brep.face_curve_on_classify_mut(k, other_face, true, &mut new_class2)?;

        // merge those curves into their respective SmBreps
        self.capture_curve_classes(
            k,
            brep,
            ClassificationPair {
                first: &mut new_class1,
                second: &mut new_class2,
                link: PairLink::Metadata,
            },
            false,
            out,
        )?;

        // We still have some 3D curve left over: classify again
        let natural = curve_of(brep, Some(input.curve))?.natural_interval();

        // classify the curve against face 1 for the 2nd time
        let mut crv_class12 = new_classification(
            k,
            input.curve,
            natural,
            input.uv_curve1,
            input.curve_tolerance,
        )?;
        if both_edges_null {
            crv_class12.set_intersect_data(
                input.surface,
                input.other_surface,
                input.uv_curve2,
                true,
                input.ti,
            );
        } else {
            crv_class12.set_surface1(input.surface);
        }
        if let Some(be) = input.brep_edge {
            set_edge_class(k, &*brep, &mut crv_class12, be)?;
        } else {
            brep.face_curve_on_classify_mut(k, this_face, true, &mut crv_class12)?;
        }

        // classify the curve against face 2 for the 2nd time
        let mut crv_class22 = new_classification(
            k,
            input.curve,
            natural,
            input.uv_curve2,
            input.curve_tolerance,
        )?;
        if both_edges_null {
            crv_class22.set_intersect_data(
                input.other_surface,
                input.surface,
                input.uv_curve1,
                crv_class1.is_curve_current(),
                input.ti,
            );
        } else {
            crv_class22.set_surface1(input.other_surface);
        }
        if let Some(oe) = input.other_edge {
            set_edge_class(k, &*brep, &mut crv_class22, oe)?;
        } else {
            brep.face_curve_on_classify_mut(k, other_face, true, &mut crv_class22)?;
        }

        self.capture_curve_classes(
            k,
            brep,
            ClassificationPair {
                first: &mut crv_class12,
                second: &mut crv_class22,
                link: PairLink::Metadata,
            },
            false,
            out,
        )?;
        Ok(())
    }

    // ---- MergeCoincidentSurfaces -------------------------------------------------------------

    /// `MergeCoincidentSurfaces(pBSurface, pOSurface, rFaces, rOFaces)`: makes the faces
    /// of two coincident surfaces compatible by merging every unmated edge of one face
    /// (inside the two faces' common box) into both faces, alternately, until no merge
    /// changes the topology; then lists the face pairs whose edges are all mated to
    /// each other (found through the radial sector of the first mated edge use of each
    /// loop of the B-rep face).
    ///
    /// # Errors
    ///
    /// `SM_ERR` past [`MAX_COINCIDENT_SURFACE_PASSES`] passes; those of the bounding
    /// boxes ([`SolidError::Unported`], `query.face_p1`), `GetEdges` /
    /// `GetUpwardLoopuses` ([`SolidError::Unported`], `topology.face_p4`), the curve
    /// copies, trims and [`TopologyIntersector::merge_curve_on_surfaces`];
    /// `SM_ERR_NULL_POINTER` for a coincident face use or edge use without face;
    /// [`SolidError::InvalidInput`] for an unknown handle or a surface without face.
    pub fn merge_coincident_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        b_surface: O,
        o_surface: O,
        ti: Option<O>,
    ) -> Result<CoincidentSurfacesMerge<O>> {
        // init output
        let mut out = CoincidentSurfacesMerge {
            faces: Vec::new(),
            other_faces: Vec::new(),
            deleted_curves: Vec::new(),
        };

        let b_face = Self::face_of_surface(brep, b_surface)?;
        let o_face = Self::face_of_surface(brep, o_surface)?;

        // detect special case - 2 planar surfaces
        let both_planes = is_plane(brep, b_surface)? && is_plane(brep, o_surface)?;

        // get surface bounding boxes
        let mut this_face_box = brep.face_bounding_box(k, b_face)?;
        let other_face_box = brep.face_bounding_box(k, o_face)?;
        this_face_box.expand_absolute(self.this_approx_tol_3d);

        // no work - bounding boxes don't intersect
        if this_face_box.are_disjoint(&other_face_box, 0.0) {
            return Ok(out);
        }

        // get FaceBBox/FaceBBox intersection
        let intersection_box = this_face_box.intersect(&other_face_box)?;

        // loop: project the curves of one face onto the other, merge the projected
        // curves into the surface faces, repeat until merge no longer causes topology
        // changes
        let mut topology_changed = true;
        let mut passes = 0usize;
        while topology_changed {
            passes += 1;
            if passes > MAX_COINCIDENT_SURFACE_PASSES {
                return Err(SolidError::sm_err(OP_COINCIDENT));
            }
            // step 1 project and merge BFace->edges into OFace (bTopologyChanged is
            // reset to FALSE, then set by the step)
            topology_changed = self.merge_face_edges(
                k,
                brep,
                Side::Brep,
                b_surface,
                o_surface,
                both_planes,
                &intersection_box,
                ti,
                &mut out.deleted_curves,
            )?;

            // when topology changed -- restart the outer loop
            if topology_changed {
                continue;
            }

            // step 2 project and merge OFace->edges into BFace
            topology_changed = self.merge_face_edges(
                k,
                brep,
                Side::Other,
                b_surface,
                o_surface,
                both_planes,
                &intersection_box,
                ti,
                &mut out.deleted_curves,
            )?;
        }

        // arrive here after all edges on faces within the surf/surf bounding box
        // intersection have been made compatible. Now look for and save coincident faces
        // in the output arrays - those get related by the calling method

        // refresh face pointers
        let b_face = Self::face_of_surface(brep, b_surface)?;

        // for every BFace->Loop
        for lu in brep.face_upward_loopuses(b_face)? {
            if brep.loopuse_is_vertex_loopuse(lu)? {
                continue;
            }

            // for every Loop->Edgeuse until finding the 1st Edgeuse with matching other
            // Brep Edge that can classify
            for eu in brep.loopuse_edgeuses(lu)? {
                // get Edgeuse's Edge and its matched Edge partner
                let b_edge = brep
                    .edgeuse_edge(eu)?
                    .ok_or(dangling("edge use without edge"))?;
                let Some(o_edge) = self.other_mate(Some(b_edge)) else {
                    // skip cases where edges don't match
                    continue;
                };

                // Classify this edgeuse (face) against the matching other Edge
                let sector =
                    ignore_soft(brep.edge_find_radial_sector(k, o_edge, eu, OrientType::Same))?;
                let Some((_found_eu, found_fu)) = sector else {
                    // too difficult an edge to sector - let it choose a different one
                    continue;
                };

                // when the classification found a coincident face to the edgeuse->Face
                // check to see if all their edges match
                if let Some(fu) = found_fu {
                    // get the coincident matching faces
                    let o_face_match = brep
                        .faceuse_face(fu)?
                        .ok_or(SolidError::null_pointer(OP_COINCIDENT))?;
                    let b_face_match = brep
                        .edgeuse_face(eu)?
                        .ok_or(SolidError::null_pointer(OP_COINCIDENT))?;

                    // get face->edges
                    let edges_o = brep.face_edges(o_face_match)?;
                    let edges_b = brep.face_edges(b_face_match)?;

                    // when edge counts are the same - note if any edges don't have a match
                    if edges_o.len() == edges_b.len() {
                        let all_edges_match = edges_b.iter().all(|&be| {
                            self.other_mate(Some(be))
                                .is_some_and(|eo| contains(&edges_o, &eo))
                        });

                        // when all edges match - add the face pair to the coincident list
                        if all_edges_match {
                            add_unique(&mut out.faces, b_face_match);
                            add_unique(&mut out.other_faces, o_face_match);
                        }
                    }
                }

                // done with Edgeuse search - its face is either coincident or not
                break;
            }
        }

        // all done
        Ok(out)
    }

    /// `pSurface->GetFace()` where upstream then uses the face unchecked.
    fn face_of_surface(brep: &impl IntersectorBrepP3<O>, surface: O) -> Result<O> {
        brep.surface_face(surface)?
            .ok_or(dangling("surface without face"))
    }

    /// One step of the `MergeCoincidentSurfaces` loop: merges each unmated edge of one
    /// face (`side`) inside `intersection_box` into both faces, through the curve itself
    /// for two planes, else through its drops onto the other surface. Returns
    /// `bTopologyChanged` (a merge deleted topology or the face's edge count changed),
    /// after which the step stops.
    #[allow(clippy::too_many_arguments)] // one upstream loop over its locals
    fn merge_face_edges(
        &mut self,
        k: &mut Kernel,
        brep: &mut impl IntersectorBrepP3<O>,
        side: Side,
        b_surface: O,
        o_surface: O,
        both_planes: bool,
        intersection_box: &Extent3d,
        ti: Option<O>,
        deleted_curves: &mut Vec<O>,
    ) -> Result<bool> {
        let mut topology_changed = false;

        // get surface->Face ptrs
        let mut b_face = Some(Self::face_of_surface(brep, b_surface)?);
        let mut o_face = Some(Self::face_of_surface(brep, o_surface)?);
        let target_surface = match side {
            Side::Brep => o_surface,
            Side::Other => b_surface,
        };
        let this_face_of = |b: Option<O>, o: Option<O>| match side {
            Side::Brep => b,
            Side::Other => o,
        };

        let mut edges =
            brep.face_edges(this_face_of(b_face, o_face).ok_or(dangling("no face"))?)?;
        let num_edges = edges.len();
        for ii in 0..num_edges {
            // skip edges already mated to the other side
            let edge = *edges.get(ii).ok_or(dangling("edge index past the list"))?;
            let mate = match side {
                Side::Brep => self.other_mate(Some(edge)),
                Side::Other => self.brep_mate(Some(edge)),
            };
            if mate.is_some() {
                continue;
            }

            // Get the edge's curve and bounding box
            let (edge_curve, edge_ivl) = edge_curve_interval(brep, edge)?;
            let boxes = edge_curve.calculate_bounding_box(
                k,
                &edge_ivl,
                BoundingBoxRequest {
                    normal: true,
                    pseudo: false,
                    polar: false,
                },
                true,
            )?;
            let mut edge_box = boxes
                .normal
                .ok_or(dangling("curve bounding box without the normal box"))?;
            edge_box.expand_absolute(tol_of(brep, edge)?);

            // no work - EdgeBBox does not intersect FaceBBox/FaceBBox intersectionBBox
            if edge_box.are_disjoint(intersection_box, 0.0) {
                continue;
            }

            // pick a tolerance
            let target_face = match side {
                Side::Brep => o_face,
                Side::Other => b_face,
            }
            .ok_or(dangling("no face"))?;
            let mut tol = tol_of(brep, target_face)? + tol_of(brep, edge)?;
            tol = max2(tol, self.this_approx_tol_3d);

            // make temporary edge->Curve copy so the original won't be modified, trimmed
            // to the edge interval
            let mut copy = edge_curve.clone();
            // Set owner for access to Tol value (its notification: see the module notes)
            let owner = edge_curve.owner();
            let _ = copy.set_owner(owner, None);
            let mut tmp_ivl = edge_ivl;
            copy.trim(k, &mut tmp_ivl, TrimMode::Notify)?;
            if side == Side::Other {
                let _ = copy.set_owner(owner, None);
            }

            let edges_count = edges.len();
            let (brep_edge, other_edge) = match side {
                Side::Brep => (Some(edge), None),
                Side::Other => (None, Some(edge)),
            };

            // when surfaces are planar - merge curve into Breps without making
            // UVTrimCurves first
            if both_planes {
                let copy_h = brep
                    .insert_curve(copy)
                    .ok_or(dangling("curve store is full"))?;
                deleted_curves.push(copy_h);

                // merge the EdgeCurve into both surfaces
                let input = CurveOnSurfaces {
                    surface: face_surface_of(brep, b_face.ok_or(dangling("no face"))?)?,
                    other_surface: face_surface_of(brep, o_face.ok_or(dangling("no face"))?)?,
                    curve_tolerance: tol,
                    curve: copy_h,
                    uv_curve1: None,
                    uv_curve2: None,
                    brep_edge,
                    other_edge,
                    ti,
                    classifications: None,
                };
                let m = self.merge_curve_on_surfaces(k, brep, &input)?;
                deleted_curves.extend(m.deleted_curves);
                topology_changed = m.deleted_topology;

                // if topology was deleted - refresh the face ptr
                if topology_changed {
                    match side {
                        Side::Brep => b_face = brep.surface_face(b_surface)?,
                        Side::Other => o_face = brep.surface_face(o_surface)?,
                    }
                }

                // if there is a face - note if its edge count has changed
                if let Some(f) = this_face_of(b_face, o_face) {
                    edges = brep.face_edges(f)?;
                    if edges.len() != edges_count {
                        topology_changed = true;
                    }
                }
            } else {
                // else for general surfaces - get the target surface's UVTrimCurves 1st,
                // then merge those into Breps
                let target_uv_domain = brep
                    .face_uv_domain(target_face)
                    .ok_or(dangling("face handle is unknown"))?;
                let mut target = brep
                    .surface(target_surface)
                    .ok_or(dangling("surface handle cannot be resolved"))?
                    .clone_for_evaluation()?;
                let _ = target.set_owner(None::<crate::geom::object::ObjectRef>);
                // DoTrace traces on the surface's cache (smsurf_GetSurfaceCache: the
                // face's trimmed cache); a cache that cannot be had fails the drop
                let mut cache = match brep.face_surface_trace_cache(k, target_surface) {
                    Ok(cache) => cache,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };
                let dropped = ignore_soft(target.drop_and_trim_curve_in::<O, O>(
                    k,
                    cache.as_mut(),
                    &target_uv_domain,
                    &copy,
                    &copy.natural_interval(),
                    tol / 4.0,
                    true,
                    true,
                    None,
                ))?;
                let Some(dropped) = dropped else {
                    continue;
                };

                // skip curves that did not project or that were too far from the surface
                if dropped.uv_curves.is_empty() {
                    continue;
                }
                if dropped.max_drop_to_surf > tol * 10.0 {
                    continue;
                }

                // expand Tol as needed (cbiTol == 0)
                tol = max2(tol, dropped.max_drop_to_surf);

                // for every target-surface UVTrimCurve
                for uv_curve in dropped.uv_curves {
                    let mut ivl = uv_curve.natural_interval();
                    let uv_h = brep
                        .insert_curve(uv_curve)
                        .ok_or(dangling("curve store is full"))?;
                    deleted_curves.push(uv_h);

                    // get temporary curve copy trimmed to UVTrimCurve extents
                    let (edge_curve, _) = edge_curve_interval(brep, edge)?;
                    let mut copy2 = edge_curve.clone();
                    copy2.trim(k, &mut ivl, TrimMode::Notify)?;
                    let copy2_h = brep
                        .insert_curve(copy2)
                        .ok_or(dangling("curve store is full"))?;
                    deleted_curves.push(copy2_h);

                    // merge the Projected Curve into both surfaces with tol set large
                    // enough to account for the project distance
                    let (uv_curve1, uv_curve2) = match side {
                        Side::Brep => (None, Some(uv_h)),
                        Side::Other => (Some(uv_h), None),
                    };
                    let input = CurveOnSurfaces {
                        surface: face_surface_of(brep, b_face.ok_or(dangling("no face"))?)?,
                        other_surface: face_surface_of(brep, o_face.ok_or(dangling("no face"))?)?,
                        curve_tolerance: tol,
                        curve: copy2_h,
                        uv_curve1,
                        uv_curve2,
                        brep_edge,
                        other_edge,
                        ti,
                        classifications: None,
                    };
                    let m = self.merge_curve_on_surfaces(k, brep, &input)?;
                    deleted_curves.extend(m.deleted_curves);
                    topology_changed = m.deleted_topology;

                    // if topology was deleted - refresh the face ptr
                    if topology_changed {
                        match side {
                            Side::Brep => b_face = brep.surface_face(b_surface)?,
                            Side::Other => o_face = brep.surface_face(o_surface)?,
                        }
                    }

                    // check the edge count: when it changed, the edge may no longer
                    // correspond to the geometry
                    if let Some(f) = this_face_of(b_face, o_face) {
                        let num_brep_edges = match side {
                            Side::Brep => edges.len(),
                            Side::Other => edges_count,
                        };
                        edges = brep.face_edges(f)?;
                        if edges.len() != num_brep_edges {
                            topology_changed = true;
                        }
                    }

                    // if topology has changed - break and restart outer loop
                    if topology_changed {
                        break;
                    }
                }
            }

            // quit edge-loop with changed topology - restart big loop sequence
            if topology_changed {
                break;
            }
        }
        Ok(topology_changed)
    }
}
