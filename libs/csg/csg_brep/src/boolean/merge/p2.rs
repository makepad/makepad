//! The merge constructor, copying an other-operand edge or face use into the primary
//! B-rep, and merging freshly added topology into the rest of the B-rep.
//!
//! Upstream: `source/SMLib/src/SmMerge.cpp`, `source/SMLib/inc/SmMerge.h`.
//!
//! | Rust                               | Upstream                                       |
//! |------------------------------------|------------------------------------------------|
//! | [`MergeState::with_breps`]         | `SmMerge::SmMerge(crContext, pBrep, pOtherBrep, ...)` |
//! | [`merge_edge`], [`merge_edge_with`], [`MergeContext`] | `SmMerge::MergeEdge`              |
//! | [`make_faceuse`], [`make_faceuse_with`], [`MadeFaceuse`] | `SmMerge::MakeFaceuse`        |
//! | [`TopologyCopy`]                   | the `Notify(SM_NO_COPY, ...)` identities       |
//! | [`MergeOperand`]                   | the two B-rep pointers of a concrete merge     |
//! | `reverse_orientation` (private)    | `SM_REVERSE_ORIENTATION`                       |
//! | [`MergeState::merge_added_face`]   | `SmMerge::MergeAddedFace`                      |
//! | [`MergeState::merge_added_topology`], [`MergeAddedOptions`], [`MergedTopology`] | `SmMerge::MergeAddedTopology` |
//! | [`MergeBrep`], [`StitchSettings`], [`StitchedVertices`], [`SplitEdges`], [`StitchedEdges`] | what `MergeAddedTopology` reads from and does to its B-rep and `SmStitch` |
//!
//! Deviations:
//! - **Operand trims (issue #488, opt-in `Kernel::fixes.keep_operand_uv_trims`).** After
//!   `MakeFaceuse` makes a face from the other operand, its edge uses get copies of the
//!   other edge uses' UV trims where the new edge runs on `MergeEdge`'s copy of the
//!   other edge's curve over the same interval (the fork's
//!   `sm_CopyUVTrimCurvesOfOtherFace`). Curves have no mates here, so "a copy of its
//!   curve" is checked as the same points at the interval's ends and middle. Upstream
//!   (and the default) builds the face without them.
//! - Operands, mates and notification storage are borrowed explicitly, as in
//!   [`p1`] and [`p5`](super::p5); arena ids replace pointers.
//!   `MergeEdge` and `MakeFaceuse` work on concrete [`Brep`] operands and the
//!   [`Relations`] mates, which gain face mates and edge/face `Relate` here.
//! - **Curve and surface mates.** An arena edge owns its curve and a face its
//!   surface, so neither can be shared by two edges or faces and neither has a mate
//!   of its own. `GetBrepMate(pCurve)` (`MergeEdge`) and `GetBrepMate(pOldSurface)`
//!   (`MakeFaceuse`) are reached only for an edge or face without a mate, so they
//!   are always null: the curve is always copied (`SmCurve::Copy`) and the surface
//!   always copied with `CopyAndAddAnalytics`. Their `Relate` calls record nothing
//!   observable and are not kept.
//! - Out-params become return values; on an error the outputs are not returned
//!   (upstream leaves `pOldRegion` null and `pNewRegion` as `MakeFace` left it).
//!   `MakeFaceuse`'s optional new-face list is cleared on entry and receives the new
//!   face on success, as upstream.
//! - Undefined behaviour upstream (a null mate faceuse, shell, edge, curve, vertex
//!   use or vertex dereferenced) is [`SolidError::InvalidInput`]; upstream's `NER`
//!   checks are `SM_ERR_NULL_POINTER`.
//! - `MakeFaceuse` treats a failed `FindRadialSector` as `SM_ERR`, as upstream,
//!   except [`SolidError::Unported`] and [`SolidError::InvalidInput`], which
//!   propagate.
//! - The infinite region `MakeFaceuse` clears around `MakeFace` is restored before
//!   `MakeFace`'s error propagates, as upstream.
//! - **`MergeAddedTopology`** drives the intersector, so it works on handles `O`
//!   like [`TopologyIntersector`]: the B-rep is read and edited through
//!   [`MergeBrep`] (an [`IntersectorBrepP2`] plus the B-rep, face and `SmStitch`
//!   calls the merge makes, each method naming the function it stands for). Its
//!   `SmTemporaryChangeValue`s of the intersector tolerance and the B-rep's
//!   `m_bEditingEnabled` are restored on every exit. The header declares
//!   `bDoGluing` before the four flags but the definition takes it after them;
//!   [`MergeAddedOptions::default`] is what the declared defaults pass to the
//!   definition (`plan/upstream-issues.md`).
//! - **Ignored statuses.** `MergeAddedTopology` ignores the statuses of the bounding
//!   boxes, the stitch and intersection calls, `RemoveRelationship` / `Relate`, and
//!   the region and orientation fix-ups, as upstream, except
//!   [`SolidError::Unported`] and [`SolidError::InvalidInput`], which propagate. A
//!   failed box query leaves the previous box, as upstream's reused local. In/out
//!   lists keep the edits made before a failure.
//! - **Partial outputs of a failed callee.** The ported stitch and intersector calls
//!   return no out-lists or gaps with an error, so after an ignored failure the merge
//!   continues with the values upstream sets on entry (empty, `0`, `SM_BIG_DOUBLE`).
//!   Upstream keeps what the callee appended before it failed: `VertexFaceIntersect`,
//!   `SplitEdgesWithVertices` and the edge/face intersectors `SER` mid-loop when
//!   `bIgnoreProblems` is `FALSE` (reachable through `SmFeatureExecutive`'s
//!   `m_bIgnoreProblems`), after appending processed vertices and raising the gaps.
//!   Those cases can differ until the callees hand back their partial outputs
//!   (`plan/findings.md`).
//! - **`SmStitch`.** The merge's one `SmStitch` (with a default, stateless
//!   `SmStitchCallback`) is [`StitchSettings`]; an implementation builds a stitch from
//!   them for each of the three calls, which is equivalent because the stitch carries
//!   nothing else between them.
//! - **Null processed vertices.** `sProcessedVerts` holds the null entries the edge
//!   intersectors return, as upstream; `FaceFaceIntersect`'s non-null vertex list
//!   drops them, which changes nothing because `FindCommonElements` never matches a
//!   null against a face's vertices.
//! - `MergeEdge` calls `topology::brep::p3::make_wire_edge` with the primary's
//!   mutable kernel and destination identities. `MergeContext::Lent` carries its
//!   existing notification lease; otherwise construction borrows the primary's
//!   callbacks. Copy events use the source context.
//!   The
//!   [`MergeBrep`] defaults do the same for `SmFace::GetEdges` / `GetVertices` /
//!   `GetBrep` (`topology.face_p4`). `OrientTrimmedSurfaces` is a required
//!   [`MergeBrep`] method (`topology::brep::p3`).
//!   The code after each stub is ported in full. `SmBrep::GetMinEdgeBBoxSize` is
//!   `topology::brep::p2::min_edge_bbox_size`. The callees ported on arena B-reps are
//!   required [`MergeBrep`] methods (`SmEdge::GetVertices`, `SmFace::GetFaceuses`,
//!   `SmFaceuse::GetShell`, `SmBrep::Notify` / `IsManifoldSolid` (`topology.brep_p2`),
//!   `SmFace::CalculateBoundingBox` (`bTight = FALSE`), the three
//!   `SmStitch` calls, `TestClosureSplitRegion`, `FindAndSetInfiniteRegion`) or come
//!   from the intersector traits (`SmBrep::GetEdges` / `GetVertices` / `GetFaces`);
//!   `SmBrep::GetTolerance` is [`Brep::tolerance`], `SmBrep::MakeFace`
//!   `topology::brep::p7::make_face`, and `RemoveRelationship` / `EdgeEdgeIntersect` /
//!   `EdgeFaceCoincidence` the intersector's (`boolean.topology_intersector_p2`).
//! - The `SM_DEBUG_CODE` / `SM_VALIDATE_TOPOLOGY` blocks are compiled out upstream
//!   and not ported. `SM_USE_OLDTOL` is the reference build's setting.
//! - **Recursion.** None.

use crate::boolean::merge::p1::{self, MergeOptions, MergeState, VertexCopy};
use crate::boolean::topology_intersector::p1::{IntersectorBrep, Relations, TopologyIntersector};
use crate::boolean::topology_intersector::p2::{
    EdgeEdgeIntersect, EdgeFaceCoincidence, IntersectorBrepP2,
};
use crate::curve::curve::p2::Curve;
use crate::error::{Result, SolidError};
use crate::geom::context::Context;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::object::{NotifyOperation, ObjectRef};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::intersect::point_class::{ClassObject, ClassObjectKind};
use crate::kernel::Kernel;
use crate::math::{max2, min2, BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::brep::p7::{self as brep_p7, MakeFaceOptions};
use crate::topology::edgeuse::{p1 as eu, p3 as eu3};
use crate::topology::topology as topo;
use crate::topology::{
    edge, loopuse, vertexuse, Brep, EdgeId, EdgeuseId, FaceId, FaceuseId, RegionId, ShellId,
    VertexId,
};

const OP: &str = "boolean::merge::p2";
const OP_EDGE: &str = "boolean::merge::p2::merge_edge";
const OP_FACEUSE: &str = "boolean::merge::p2::make_faceuse";

/// A dereference of a null pointer upstream (undefined behaviour).
const fn dangling(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// `SM_REVERSE_ORIENTATION(a)`: `SM_OT_SAME` becomes `SM_OT_OPPOSITE`, anything
/// else (including `SM_OT_UNKNOWN`) becomes `SM_OT_SAME`.
const fn reverse_orientation(o: OrientType) -> OrientType {
    match o {
        OrientType::Same => OrientType::Opposite,
        OrientType::Opposite | OrientType::Unknown => OrientType::Same,
    }
}

// ---- the constructor ---------------------------------------------------------------

/// `SmBrep::GetMinEdgeBBoxSize(rpEdge)` (`topology::brep::p2::min_edge_bbox_size`); the
/// constructor drops the edge.
fn min_edge_bbox_size(k: &Kernel, brep: &Brep) -> Result<f64> {
    Ok(crate::topology::brep::p2::min_edge_bbox_size(k, brep)?.0)
}

impl<O: ClassObject + SolutionObject + Ord> MergeState<TopologyIntersector<O>> {
    /// `SmMerge(crContext, pBrep, pOtherBrep, dApproxTol3d, dAngleTolRadians,
    /// pOptMergeOptions)`: the intersector over both B-reps with the given
    /// tolerances, the default controls, then the options (a null pointer keeps the
    /// defaults). A tolerance below `SM_EFF_ZERO` is replaced by
    /// `min(0.5 * min(brep tolerances), 0.33 * min edge bounding-box size)`; an angle
    /// tolerance of exactly `0.0` by 20 degrees. `brep` and `other` give each B-rep's
    /// arena and the handle the intersector refers to it by; `other` may be `None`
    /// (the stitch entry points) unless the tolerance must be computed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a computed tolerance without other B-rep
    /// (upstream dereferences null); those of `SmBrep::GetMinEdgeBBoxSize`.
    pub fn with_breps(
        k: &Kernel,
        brep: (&Brep, O),
        other: Option<(&Brep, O)>,
        approx_tol_3d: f64,
        ang_tol_rad: f64,
        options: Option<&MergeOptions>,
    ) -> Result<Self> {
        let intersector =
            TopologyIntersector::new(Some(brep.1), other.map(|o| o.1), approx_tol_3d, ang_tol_rad);
        // m_bManifoldBoolean(TRUE), m_bCookieCutter(FALSE), m_bImprinting(FALSE),
        // m_bBooleanPostProcess(TRUE), m_bIntersectionLoopClosed(TRUE),
        // m_bImprintAndClassifyFaces(FALSE), m_bKeepOtherBrep(FALSE), m_lTagID(0)
        let mut state = Self::new(intersector);
        state.manifold_boolean = Some(true);
        state.intersection_loop_closed = Some(true);
        state.set_merge_options(options);

        // The user can ask us to calculate a tolerance based on the geometry by
        // specifying a zero tolerance.
        if approx_tol_3d < EFF_ZERO {
            // Get the minimum Edge length.
            let edge1_size = min_edge_bbox_size(k, brep.0)?;
            let (other_brep, _) =
                other.ok_or(dangling(OP, "computed tolerance without other B-rep"))?;
            let edge2_size = min_edge_bbox_size(k, other_brep)?;
            let edge_size = min2(edge1_size, edge2_size);

            // siWhichTol == 3: 1/2 of Min of two Brep's tols.
            let brep_tol = 0.5 * min2(brep.0.tolerance(), other_brep.tolerance());

            state
                .intersector
                .set_this_approx_tol_3d(min2(brep_tol, edge_size * 0.33));
        }

        // Use 20 degrees as a default angle tolerance
        #[allow(clippy::float_cmp)] // upstream's exact `== 0.0`
        if ang_tol_rad == 0.0 {
            state.intersector.set_this_ang_tol_rad(20.0 * (PI / 180.0));
        }
        Ok(state)
    }
}

/// The intersector's identities of the two operands of a merge built on concrete
/// [`Brep`]s ([`MergeState::with_breps`]): the only objects its constructor names.
/// Callers that walk the operands' topology through the intersector choose a richer
/// handle type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MergeOperand {
    /// The B-rep receiving the merge (`pBrep`).
    Primary,
    /// The B-rep merged into it (`pOtherBrep`).
    Other,
}

impl ClassObject for MergeOperand {
    fn class_kind(&self) -> ClassObjectKind {
        ClassObjectKind::Other
    }
}

impl SolutionObject for MergeOperand {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Other
    }
}

// ---- copying topology from the other operand -----------------------------------------

/// The notification storage and identities of the topology copied from the other
/// operand (`Notify(SM_NO_COPY, pCopy, ...)` of vertices, edges and faces). Keys must
/// be distinct across operands even when their arena ids coincide.
pub struct TopologyCopy<'a> {
    /// Records, attributes and vertex identities ([`p1::merge_vertex`]).
    pub vertices: VertexCopy<'a>,
    /// Notification identity of each other-operand edge.
    pub edge_source_key: &'a dyn Fn(EdgeId) -> ObjectRef,
    /// Notification identity of each primary edge, including freshly created ids.
    pub edge_destination_key: &'a dyn Fn(EdgeId) -> ObjectRef,
    /// Notification identity of each other-operand face.
    pub face_source_key: &'a dyn Fn(FaceId) -> ObjectRef,
    /// Notification identity of each primary face, including freshly created ids.
    pub face_destination_key: &'a dyn Fn(FaceId) -> ObjectRef,
}

impl TopologyCopy<'_> {
    /// `pSource->Notify(SM_NO_COPY, pCopy, SM_NO_GET_BREP(pCopy),
    /// SM_NO_GET_BREP(pSource))`: `SM_NO_GET_BREP` is null in the pinned source.
    fn notify(
        &mut self,
        context: Option<&mut Context>,
        source: ObjectRef,
        copy: ObjectRef,
    ) -> Result<()> {
        topo::notify(
            self.vertices.records,
            self.vertices.attributes,
            context,
            NotifyOperation::Copy,
            source,
            Some(copy),
            None,
            None,
        )
    }
}

fn make_wire_edge(
    ctx: &mut MergeContext<'_>,
    brep: &mut Brep,
    region: Option<RegionId>,
    vertices: [VertexId; 2],
    curve: Curve,
    interval: Extent1d,
    copy: &mut TopologyCopy<'_>,
) -> Result<EdgeId> {
    let [start, end] = vertices;
    let vertex_key = copy.vertices.destination_key;
    let edge_key = copy.edge_destination_key;
    let face_key = copy.face_destination_key;
    let key_of = |entity| match entity {
        crate::EntityRef::Edge(edge) => edge_key(edge),
        crate::EntityRef::Face(face) => face_key(face),
        other => vertex_key(other),
    };
    let construct = |k: &mut Kernel, context: &mut Context| {
        let mut env = edge::NotifyEnv {
            records: &mut *copy.vertices.records,
            attributes: &mut *copy.vertices.attributes,
            context: Some(context),
            key_of: &key_of,
        };
        crate::topology::brep::p3::make_wire_edge(
            k,
            brep,
            &mut env,
            region,
            start,
            end,
            curve,
            &interval,
            OrientType::Same,
        )
    };
    match ctx {
        MergeContext::Lent(k, context) => construct(k, context),
        MergeContext::Split(k, _) | MergeContext::Shared(k) => {
            Context::with_notification_context(k, construct)?
        }
    }
}

/// Merge an edge of the other operand into the primary B-rep (`MergeEdge`). An edge
/// without a primary mate becomes a new primary wire edge in `region` (`None`: the
/// infinite region) over a copy of its curve, between the primary mates of its
/// vertices (merged in as shell vertices when they have none), and is related to it.
/// The primary edge then gets the larger tolerance and the source edge is notified
/// of the copy. A null edge use succeeds with no edge, as upstream.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` when a vertex has no primary mate after merging;
/// [`SolidError::InvalidInput`] for a null edge, curve, vertex use or vertex of the
/// edge use (upstream dereferences them); those of the curve copy, `Relate` and
/// [`p1::merge_vertex`]; [`SolidError::Unported`] (module docs).
pub fn merge_edge(
    k: &mut Kernel,
    brep: &mut Brep,
    other: &mut Brep,
    edgeuse: Option<EdgeuseId>,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    context: Option<&mut Context>,
) -> Result<Option<EdgeId>> {
    merge_edge_with(
        MergeContext::Split(k, context),
        brep,
        other,
        edgeuse,
        region,
        relations,
        copy,
    )
}

/// The kernel `MergeEdge` copies the curve with (the primary B-rep's context) and the
/// context the source edge and vertices are notified through.
pub enum MergeContext<'a> {
    /// A kernel and a separate notification context (or none).
    Split(&'a mut Kernel, Option<&'a mut Context>),
    /// A kernel whose notification context is already lent to its caller.
    Lent(&'a mut Kernel, &'a mut Context),
    /// One kernel whose own context is notified (both B-reps share it).
    Shared(&'a mut Kernel),
}

impl MergeContext<'_> {
    pub(crate) fn kernel(&self) -> &Kernel {
        match self {
            Self::Split(k, _) | Self::Lent(k, _) | Self::Shared(k) => k,
        }
    }

    pub(crate) fn kernel_mut(&mut self) -> &mut Kernel {
        match self {
            Self::Split(k, _) | Self::Shared(k) | Self::Lent(k, _) => k,
        }
    }

    fn context(&mut self) -> Option<&mut Context> {
        match self {
            Self::Split(_, context) => context.as_deref_mut(),
            Self::Lent(_, context) => Some(context),
            Self::Shared(k) => Some(&mut k.context),
        }
    }

    pub(crate) fn reborrow(&mut self) -> MergeContext<'_> {
        match self {
            Self::Split(k, context) => MergeContext::Split(k, context.as_deref_mut()),
            Self::Lent(k, context) => MergeContext::Lent(k, context),
            Self::Shared(k) => MergeContext::Shared(k),
        }
    }
}

/// [`merge_edge`] with the kernel and notification context given as a
/// [`MergeContext`] (for callers whose two B-reps share one kernel). The curve is
/// copied before any notification, so the two uses never overlap.
///
/// # Errors
///
/// As [`merge_edge`].
pub fn merge_edge_with(
    mut ctx: MergeContext<'_>,
    brep: &mut Brep,
    other: &mut Brep,
    edgeuse: Option<EdgeuseId>,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
) -> Result<Option<EdgeId>> {
    // The caller knows better whether this is a problem.
    let Some(edgeuse) = edgeuse else {
        return Ok(None);
    };

    // get target edge
    let other_edge = eu::edge(other, edgeuse)?.ok_or(dangling(OP_EDGE, "edge use without edge"))?;

    // get Brep matching edge (probably NULL)
    let brep_edge = if let Some(brep_edge) = relations.edges.primary(other_edge) {
        brep_edge
    } else {
        // copy the curve (its Brep mate is always NULL; module docs)
        let source = other.edge(other_edge)?;
        let interval = source.interval;
        let new_curve = source
            .curve
            .as_ref()
            .ok_or(dangling(OP_EDGE, "edge without curve"))?
            .copy(ctx.kernel())?;

        // get OtherBrep edge->vertices
        let vu = eu::vertexuse(other, edgeuse)?
            .ok_or(dangling(OP_EDGE, "edge use without vertex use"))?;
        let mut v = vertexuse::vertex(other, vu)?;
        let mut v2 = edge::other_vertex(other, other_edge, v)?;

        // Swap vertices if the edgeuse is oriented opposite to the edge.
        if eu::orientation(other, edgeuse)? == OrientType::Opposite {
            core::mem::swap(&mut v, &mut v2);
        }

        // Get or make matching Brep vertices
        let mut mates = [None, None];
        for (slot, v) in mates.iter_mut().zip([v, v2]) {
            let mut mate = v.and_then(|v| relations.vertices.primary(v));
            if mate.is_none() {
                mate = p1::merge_vertex(
                    brep,
                    other,
                    v,
                    region,
                    relations,
                    &mut copy.vertices,
                    ctx.context(),
                )?;
            }
            *slot = Some(mate.ok_or(SolidError::null_pointer(OP_EDGE))?);
        }
        let [Some(bv), Some(bv2)] = mates else {
            return Err(SolidError::null_pointer(OP_EDGE));
        };

        // make the new Brep wire edge
        let brep_edge =
            make_wire_edge(&mut ctx, brep, region, [bv, bv2], new_curve, interval, copy)?;

        // mate new edge with input otherBrep edge
        relations.relate_edges(brep, brep_edge, other, other_edge)?;
        brep_edge
    };

    // match tolerances for the mated edges
    let other_tolerance = other.edge(other_edge)?.tolerance;
    if other_tolerance > brep.edge(brep_edge)?.tolerance {
        edge::set_tolerance(brep, brep_edge, other_tolerance, true, true)?;
    }

    // notify OtherBrepEdge of the copy (so it can propagate attributes as needed)
    let (source, dest) = (
        (copy.edge_source_key)(other_edge),
        (copy.edge_destination_key)(brep_edge),
    );
    copy.notify(ctx.context(), source, dest)?;

    Ok(Some(brep_edge))
}

/// What [`make_faceuse`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MadeFaceuse {
    /// `pOldRegion`: the primary region receiving the new face (`None` when the face
    /// already had a mate).
    pub old_region: Option<RegionId>,
    /// `pNewRegion`: the region made when the new face splits `old_region`.
    pub new_region: Option<RegionId>,
}

/// What [`make_faceuse_attempt`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct MakeFaceuseAttempt {
    /// The status of the operation: `Ok(())` or the upstream error.
    pub status: Result<()>,
    /// `pOldRegion`: the primary region receiving the new face (`None` on error or when
    /// the face already had a mate).
    pub old_region: Option<RegionId>,
    /// `pNewRegion`: the region made when the new face splits `old_region`.
    pub new_region: Option<RegionId>,
}

fn shell_region_of(b: &Brep, fu: FaceuseId) -> Result<Option<RegionId>> {
    let shell: ShellId =
        eu::faceuse_shell(b, fu)?.ok_or(dangling(OP_FACEUSE, "face use without shell"))?;
    eu::shell_region(b, shell)
}

fn vertex_of_vertexuse(b: &Brep, vu: Option<crate::topology::VertexuseId>) -> Result<VertexId> {
    let vu = vu.ok_or(dangling(OP_FACEUSE, "null vertex use"))?;
    vertexuse::vertex(b, vu)?.ok_or(dangling(OP_FACEUSE, "vertex use without vertex"))
}

/// The orientation of the primary edge use to look for in the radial sector of
/// `brep_edge` for the other-operand edge use `edgeuse` (opposite to it relative to
/// the vertices, since it will end up radial to it).
fn orientation_to_find(
    k: &Kernel,
    brep: &mut Brep,
    other: &mut Brep,
    edgeuse: EdgeuseId,
    other_edge: EdgeId,
    brep_edge: EdgeId,
    relations: &Relations,
) -> Result<OrientType> {
    let v = vertex_of_vertexuse(other, eu::vertexuse(other, edgeuse)?)?;
    let v2 = edge::other_vertex(other, other_edge, Some(v))?;
    let prim = eu::primary_edgeuse(brep, brep_edge)?
        .ok_or(dangling(OP_FACEUSE, "edge without primary edge use"))?;
    let vb = vertex_of_vertexuse(brep, eu::vertexuse(brep, prim)?)?;
    let vb2 = edge::other_vertex(brep, brep_edge, Some(vb))?;
    if Some(v) == v2 {
        // Here need to do additional work for closed loop stuff
        let tangent = |k: &Kernel, b: &mut Brep, use_: EdgeuseId, t: f64| -> Result<_> {
            eu3::normalized_evaluate(k, b, use_, t, false, true, true)?
                .first_deriv
                .ok_or(dangling(OP_FACEUSE, "no first derivative"))
        };
        let tan1 = tangent(k, other, edgeuse, 0.0)?;
        // Pointing from the vertex into the curve...
        let tan2 = -tangent(k, other, edgeuse, 1.0)?;
        let tan_ref = tangent(k, brep, prim, 0.0)?;
        let tan1 = tan1.unitize()?;
        let tan2 = tan2.unitize()?;
        let tan_ref = tan_ref.unitize()?;
        let orient = eu::orientation(brep, prim)?;
        if tan1.dot(tan_ref) > tan2.dot(tan_ref) {
            Ok(orient)
        } else {
            Ok(reverse_orientation(orient))
        }
    } else {
        let vb_rel = relations
            .vertices
            .primary(v)
            .ok_or(SolidError::null_pointer(OP_FACEUSE))?;
        if vb == vb_rel {
            eu::orientation(brep, prim)
        } else if vb2 == Some(vb_rel) {
            Ok(reverse_orientation(eu::orientation(brep, prim)?))
        } else {
            Err(SolidError::sm_err(OP_FACEUSE))
        }
    }
}

/// Make the face of an other-operand face use in the primary B-rep
/// (`MakeFaceuse`). The face use on the side of the other operand's infinite region
/// is used. A face that already has a primary mate is only notified of the copy.
/// Otherwise the containing primary region is `region` or found with
/// [`p1::find_brep_region_of_other_face`]; every vertex and edge of the face is
/// merged in ([`p1::merge_vertex`], [`merge_edge`]), the radial sector of each
/// primary edge chosen, the surface copied and the face made, related and notified.
/// `new_faces`, when given, is cleared and receives the new face.
///
/// # Errors
///
/// `SM_ERR` when no region contains the face, a vertex orientation cannot be matched
/// or no radial sector is found; `SM_ERR_NULL_POINTER` for a vertex without mate;
/// [`SolidError::InvalidInput`] for missing topology upstream dereferences or a zero
/// tangent (`Unitize`); those of the merges, evaluations, copies and `Relate`;
/// [`SolidError::Unported`] (module docs).
pub fn make_faceuse(
    k: &mut Kernel,
    brep: &mut Brep,
    other: &mut Brep,
    faceuse: FaceuseId,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    context: Option<&mut Context>,
    new_faces: Option<&mut Vec<FaceId>>,
) -> Result<MadeFaceuse> {
    make_faceuse_with(
        MergeContext::Split(k, context),
        brep,
        other,
        faceuse,
        region,
        relations,
        copy,
        new_faces,
    )
}

/// [`make_faceuse`] attempt variant preserving partial outputs (such as `new_region` on split).
pub fn make_faceuse_attempt(
    k: &mut Kernel,
    brep: &mut Brep,
    other: &mut Brep,
    faceuse: FaceuseId,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    context: Option<&mut Context>,
    new_faces: Option<&mut Vec<FaceId>>,
) -> Result<MakeFaceuseAttempt> {
    make_faceuse_attempt_with(
        MergeContext::Split(k, context),
        brep,
        other,
        faceuse,
        region,
        relations,
        copy,
        new_faces,
    )
}

/// The attributes of the copy `MakeFaceuse` makes of a face's surface: upstream's
/// `CopyAndAddAnalytics` copies the surface through its copy constructor (or
/// `IsNurbSurfacePlane`'s `SM_NO_COPY`), which copies every attribute of the other
/// face's surface onto the new face's, a reference attribute (`SM_AI_SURFACEMAP`)
/// gaining the copy as a user. A surface goes by its owner's name (the arena name
/// without one), as the merges' surface identities.
fn copy_surface_attributes(
    attributes: &mut crate::geom::attribute::AttributeStore,
    other: &Brep,
    from_face: FaceId,
    brep: &Brep,
    to_face: FaceId,
) -> Result<()> {
    let surface_key = |b: &Brep, f: FaceId| -> Result<ObjectRef> {
        let surface = b.face(f)?.surface.as_ref();
        let surface = surface.ok_or(dangling(OP_FACEUSE, "face without surface"))?;
        Ok(surface.owner().unwrap_or_else(|| {
            crate::topology::brep_cache::arena_identity(
                crate::topology::brep_cache::CacheObject::Face(f),
            )
        }))
    };
    let (from, to) = (surface_key(other, from_face)?, surface_key(brep, to_face)?);
    for h in attributes.attributes(from.key) {
        attributes.copy(h, from.key, to.key)?;
    }
    Ok(())
}

/// [`make_faceuse`] with the kernel and notification context given as a
/// [`MergeContext`] (for callers whose two B-reps share one kernel).
///
/// # Errors
///
/// As [`make_faceuse`].
pub fn make_faceuse_with(
    ctx: MergeContext<'_>,
    brep: &mut Brep,
    other: &mut Brep,
    faceuse: FaceuseId,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    new_faces: Option<&mut Vec<FaceId>>,
) -> Result<MadeFaceuse> {
    let attempt = make_faceuse_attempt_with(
        ctx, brep, other, faceuse, region, relations, copy, new_faces,
    )?;
    attempt.status?;
    Ok(MadeFaceuse {
        old_region: attempt.old_region,
        new_region: attempt.new_region,
    })
}

/// [`make_faceuse_with`] attempt variant preserving partial outputs.
pub fn make_faceuse_attempt_with(
    mut ctx: MergeContext<'_>,
    brep: &mut Brep,
    other: &mut Brep,
    faceuse: FaceuseId,
    region: Option<RegionId>,
    relations: &mut Relations,
    copy: &mut TopologyCopy<'_>,
    mut new_faces: Option<&mut Vec<FaceId>>,
) -> Result<MakeFaceuseAttempt> {
    // init output
    if let Some(list) = new_faces.as_deref_mut() {
        list.clear();
    }

    // If the Face of the input Faceuse bounds the other Brep's infinite region, then
    // set it up so that the face use is the one on the side of the infinite region.
    let mut faceuse = faceuse;
    let mate =
        eu::faceuse_mate(other, faceuse)?.ok_or(dangling(OP_FACEUSE, "face use without mate"))?;
    let other_inf = other.infinite_region();
    if shell_region_of(other, mate)? == other_inf && shell_region_of(other, faceuse)? != other_inf {
        faceuse = mate;
    }

    // Check to see if face is already mated to a Brep face
    let face_to_make =
        eu::faceuse_face(other, faceuse)?.ok_or(dangling(OP_FACEUSE, "face use without face"))?;
    if let Some(face) = relations.faces.primary(face_to_make) {
        // low work - notify of copy to propagate attributes and return
        let (source, dest) = (
            (copy.face_source_key)(face_to_make),
            (copy.face_destination_key)(face),
        );
        copy.notify(ctx.context(), source, dest)?;
        return Ok(MakeFaceuseAttempt {
            status: Ok(()),
            old_region: None,
            new_region: None,
        });
    }

    // when not given a Brep Region try to find the containing Brep Region
    let region = match region {
        Some(r) => Some(r),
        None => p1::find_brep_region_of_other_face(
            ctx.kernel(),
            brep,
            other,
            faceuse,
            relations.edges.other_to_primary(),
        )?,
    };
    // error - unable to classify input OtherBrep faceuse into a Brep region
    let region = region.ok_or(SolidError::sm_err(OP_FACEUSE))?;

    // Now merge all unrelated input face->vertices into Brep's Region as shell
    // vertices, and pick up the radial sector edge uses.
    let mut face_edgeuses = Vec::new();
    let mut loop_vertices = Vec::new();
    let mut edge_loops = Vec::new();

    // for every Faceuse->Loopuse
    for lu in eu::faceuse_loopuses(other, faceuse, false, None)? {
        // get this loopuse's edgeuses
        let edgeuses = loopuse::edgeuses(other, lu, None, None)?;

        // load edge counts into sEdgeLoops array
        let vertex_loop = loopuse::is_vertex_loopuse(other, lu)?;
        if !vertex_loop {
            edge_loops.push(edgeuses.len());
        }

        // for every Loopuse->Vertexuse
        for (kk, vu) in loopuse::vertexuses(other, lu, None)?
            .into_iter()
            .enumerate()
        {
            let v = vertex_of_vertexuse(other, vu)?;

            // Merge the Vertex
            let brep_vertex = p1::merge_vertex(
                brep,
                other,
                Some(v),
                Some(region),
                relations,
                &mut copy.vertices,
                ctx.context(),
            )?;

            // load VertexLoopuses into sLoopVertices
            if kk == 0 && vertex_loop {
                loop_vertices.push(brep_vertex.ok_or(SolidError::null_pointer(OP_FACEUSE))?);
            }
        }

        // for every Loopuse->Edgeuse
        for eu_other in edgeuses {
            let other_edge =
                eu::edge(other, eu_other)?.ok_or(dangling(OP_FACEUSE, "edge use without edge"))?;

            // merge the edge
            let brep_edge = merge_edge_with(
                ctx.reborrow(),
                brep,
                other,
                Some(eu_other),
                Some(region),
                relations,
                copy,
            )?
            .ok_or(dangling(OP_FACEUSE, "merged edge use without edge"))?;

            // Find the orientation of the Edgeuse to look for in the radial sector.
            let orient = orientation_to_find(
                ctx.kernel(),
                brep,
                other,
                eu_other,
                other_edge,
                brep_edge,
                relations,
            )?;

            // classify Edgeuse->Face against BrepEdge
            let found = match edge::find_radial_sector_between(
                ctx.kernel(),
                brep,
                brep_edge,
                Some(&mut *other),
                eu_other,
                Some(region),
                orient,
                None,
            ) {
                Ok(sector) => sector.radial_edgeuse,
                Err(
                    e @ (SolidError::Unported { .. }
                    | SolidError::InvalidInput { .. }
                    | SolidError::PortInvariant { .. }),
                ) => return Err(e),
                Err(_) => None,
            };
            // add the classified radial sector's Edgeuse to the sFaceEdgeuses list
            face_edgeuses.push(found.ok_or(SolidError::sm_err(OP_FACEUSE))?);
        }
    }

    // Copy the surface, when possible as an analytic surface (its Brep mate is always
    // NULL; module docs)
    let face_record = other.face(face_to_make)?;
    let uv_domain = face_record.uv_domain;
    let new_surface = face_record
        .surface
        .as_ref()
        .ok_or(dangling(OP_FACEUSE, "face without surface"))?
        .copy_and_add_analytics(ctx.kernel())?;

    // If we have the case of a faceuse bordering an infinite region in the other
    // brep being merged into the infinite region in this brep, temporarily set Brep's
    // infinite region to NULL.
    let brep_inf = brep.infinite_region();
    let mate =
        eu::faceuse_mate(other, faceuse)?.ok_or(dangling(OP_FACEUSE, "face use without mate"))?;
    let saved_infinite_region = if Some(region) == brep_inf
        && shell_region_of(other, faceuse)? == other_inf
        && shell_region_of(other, mate)? != other_inf
    {
        brep.set_infinite_region_record(None);
        brep_inf
    } else {
        None
    };

    // make the new Brep Face within the classified Brep region
    let orientation = other.faceuse(faceuse)?.orientation;
    // bVerifyRadialSectors = FALSE, bCalculateTrims = FALSE, cpOptUVCurves = NULL
    // MakeFace's own topology records and notifications under the primary
    // identities: the copy's face and edge keys, its entity keys for the rest (no
    // context callbacks: the kernel is borrowed for the face's mark lock)
    let (face_key, edge_key, entity_key) = (
        copy.face_destination_key,
        copy.edge_destination_key,
        copy.vertices.destination_key,
    );
    let key_of = |e: crate::EntityRef| match e {
        crate::EntityRef::Face(f) => face_key(f),
        crate::EntityRef::Edge(x) => edge_key(x),
        other => entity_key(other),
    };
    let mut env = edge::NotifyEnv {
        records: &mut *copy.vertices.records,
        attributes: &mut *copy.vertices.attributes,
        context: None,
        key_of: &key_of,
    };
    let attempt = brep_p7::make_face_attempt(
        ctx.kernel_mut(),
        brep,
        Some(region),
        &edge_loops,
        &face_edgeuses,
        &loop_vertices,
        Some(new_surface),
        &uv_domain,
        orientation,
        &MakeFaceOptions::default(),
        copy.face_destination_key,
        &mut env,
    )?;
    if saved_infinite_region.is_some() {
        brep.set_infinite_region_record(saved_infinite_region);
    }
    if let Err(err) = attempt.status {
        return Ok(MakeFaceuseAttempt {
            status: Err(err),
            old_region: None,
            new_region: attempt.region,
        });
    }
    let new_region = attempt.region;
    // SER(m_vTI.Relate(pNewFace, ...)) dereferences a null new face.
    let new_face = attempt
        .face
        .ok_or(dangling(OP_FACEUSE, "MakeFace made no face"))?;

    // the surface copy's attributes (`CopyAndAddAnalytics`)
    copy_surface_attributes(
        &mut *copy.vertices.attributes,
        other,
        face_to_make,
        brep,
        new_face,
    )?;

    relations.relate_faces(brep, new_face, other, face_to_make)?;

    // Not upstream (source issue #488; opt-in): MakeFace does not take the
    // other face's UV trims over; carry the exact ones across
    if ctx.kernel().fixes.keep_operand_uv_trims {
        copy_uv_trim_curves_of_other_face(ctx.kernel(), brep, other, faceuse, new_face, relations)?;
    }
    let (source, dest) = (
        (copy.face_source_key)(face_to_make),
        (copy.face_destination_key)(new_face),
    );
    copy.notify(ctx.context(), source, dest)?;

    // set output
    if let Some(list) = new_faces {
        list.push(new_face);
    }
    Ok(MakeFaceuseAttempt {
        status: Ok(()),
        old_region: Some(region),
        new_region,
    })
}

/// Not upstream (source issue #488, the fork's
/// `sm_CopyUVTrimCurvesOfOtherFace`; opt-in `Kernel::fixes.keep_operand_uv_trims`): gives the
/// edge uses of `new_face`, just made in `brep` from the other operand's face use
/// `other_fu`, the UV trim curves of the other edge uses they stand for. A trim is
/// parameterised by its edge's curve on its face's surface, so it stays exact where
/// the new edge is the primary mate of the other edge, runs on a copy of its curve
/// (the same points at its interval's ends and middle) over the same interval, and the
/// new face lies on the copy `MakeFaceuse` made of the other face's surface (always,
/// here). The new face's face use of the other face use's orientation lists its edge
/// uses in the other's order (as `MakeFace` assigns `cpOptUVCurves`). Elsewhere the
/// new edge use keeps what `MakeFace` gave it.
///
/// # Errors
///
/// `SM_ERR_NULL_POINTER` for a new face without both face uses; `SM_ERR` when the two
/// face uses list different numbers of edge uses; those of the topology reads, the
/// curve copies and evaluations and `SetUVTrimCurve`.
fn copy_uv_trim_curves_of_other_face(
    k: &Kernel,
    brep: &mut Brep,
    other: &Brep,
    other_fu: FaceuseId,
    new_face: FaceId,
    relations: &Relations,
) -> Result<()> {
    let (fu1, fu2) = crate::topology::face::p2::faceuses(brep, new_face)?;
    let (Some(fu1), Some(fu2)) = (fu1, fu2) else {
        return Err(SolidError::null_pointer(OP_FACEUSE));
    };
    let orientation = other.faceuse(other_fu)?.orientation;
    let new_fu = if brep.faceuse(fu1)?.orientation == orientation {
        fu1
    } else {
        fu2
    };
    let other_uses = eu::faceuse_edgeuses(other, other_fu, false, None)?;
    let new_uses = eu::faceuse_edgeuses(brep, new_fu, false, None)?;
    if other_uses.len() != new_uses.len() {
        return Err(SolidError::sm_err(OP_FACEUSE));
    }
    for (other_eu, new_eu) in other_uses.into_iter().zip(new_uses) {
        let Some(trim) = eu::uv_trim_curve(other, other_eu)?.0 else {
            continue;
        };
        let other_edge = eu::edge(other, other_eu)?;
        let new_edge = eu::edge(brep, new_eu)?;
        let (Some(other_edge), Some(new_edge)) = (other_edge, new_edge) else {
            continue;
        };
        if relations.edges.primary(other_edge) != Some(new_edge) {
            continue;
        }
        let (other_record, new_record) = (other.edge(other_edge)?, brep.edge(new_edge)?);
        let interval = other_record.interval;
        let new_interval = new_record.interval;
        if new_interval.min() != interval.min() || new_interval.max() != interval.max() {
            continue;
        }
        let (Some(other_curve), Some(new_curve)) =
            (other_record.curve.as_ref(), new_record.curve.as_ref())
        else {
            continue;
        };
        // (MergeEdge's copy of the other edge's curve: the same points)
        let mut same = other_curve.dim() == new_curve.dim()
            && other_curve.natural_interval() == new_curve.natural_interval();
        for t in [interval.min(), interval.mid(), interval.max()] {
            if !same {
                break;
            }
            same = other_curve.evaluate_point(k, t)? == new_curve.evaluate_point(k, t)?;
        }
        if !same {
            continue;
        }
        let copy = trim.copy(k)?;
        // TRUE = delete a trim MakeFace made for sectoring
        eu::set_uv_trim_curve(
            brep,
            new_eu,
            eu::UvTrimCurveArg::New(Box::new(copy)),
            0.0,
            true,
            None,
        )?;
    }
    Ok(())
}

// ---- merging added topology ------------------------------------------------------------

/// What [`MergeBrep::stitch_vertex_pairs`] returns (`SmStitch::StitchVertexPairs`,
/// two-list overload).
#[derive(Debug, Clone, PartialEq)]
pub struct StitchedVertices<O> {
    /// `rProcessedVerts`: the surviving glued vertices (from the first list).
    pub processed: Vec<O>,
    /// `rDeletedVerts`: the glued vertices deleted, in step with `processed`.
    pub deleted: Vec<O>,
    /// `rdMaxVertexGap`.
    pub max_vertex_gap: f64,
    /// `rdMinUnstitchedVertGap`.
    pub min_unstitched_vert_gap: f64,
}

impl<O> StitchedVertices<O> {
    /// The outputs as initialised on entry (what an ignored failure leaves).
    #[must_use]
    pub const fn initial() -> Self {
        Self {
            processed: Vec::new(),
            deleted: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        }
    }
}

/// What [`MergeBrep::split_edges_with_vertices`] returns
/// (`SmStitch::SplitEdgesWithVertices`).
#[derive(Debug, Clone, PartialEq)]
pub struct SplitEdges<O> {
    /// `rProcessedVerts`: the vertices that split an edge.
    pub processed_verts: Vec<O>,
    /// `rSplitEdges`: the edges that were split.
    pub split_edges: Vec<O>,
    /// `rNewEdges`: the edges the splits created (also appended to the edge list).
    pub new_edges: Vec<O>,
    /// `rdMaxVertexGap`.
    pub max_vertex_gap: f64,
    /// `rdMinUnstitchedVertGap`.
    pub min_unstitched_vert_gap: f64,
}

impl<O> SplitEdges<O> {
    /// The outputs as initialised on entry (what an ignored failure leaves).
    #[must_use]
    pub const fn initial() -> Self {
        Self {
            processed_verts: Vec::new(),
            split_edges: Vec::new(),
            new_edges: Vec::new(),
            max_vertex_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
        }
    }
}

/// What [`MergeBrep::stitch_edges_of_vertices`] returns
/// (`SmStitch::StitchEdgesOfVertices`).
#[derive(Debug, Clone, PartialEq)]
pub struct StitchedEdges<O> {
    /// `rGluedEdges`: the edges that survived being glued.
    pub glued: Vec<O>,
    /// `rDeletedEdges`: the edges gluing deleted, in step with `glued`.
    pub deleted: Vec<O>,
    /// `rdMaxEdgeGap`.
    pub max_edge_gap: f64,
    /// `rdMinUnstitchedEdgeGap`.
    pub min_unstitched_edge_gap: f64,
}

impl<O> StitchedEdges<O> {
    /// The outputs as initialised on entry (what an ignored failure leaves; the
    /// caller's glued and deleted lists start empty).
    #[must_use]
    pub const fn initial() -> Self {
        Self {
            glued: Vec::new(),
            deleted: Vec::new(),
            max_edge_gap: 0.0,
            min_unstitched_edge_gap: BIG_DOUBLE,
        }
    }
}

/// The `SmStitch` [`MergeState::merge_added_topology`] builds (`SmStitch
/// sStitcher(sCB, dThisTol)` with a default `SmStitchCallback`) and the members it
/// sets; the stitch tolerance changes (`SetStitchTol3d`) between the three calls.
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StitchSettings {
    /// `m_dStitchTol3d`.
    pub stitch_tol_3d: f64,
    /// `m_bMakingManifoldSolid`.
    pub making_manifold_solid: bool,
    /// `m_bIgnoreProblems`.
    pub ignore_problems: bool,
    /// `m_bDoRegionNesting`.
    pub do_region_nesting: bool,
    /// `m_bFastEdgeCompare`.
    pub fast_edge_compare: bool,
    /// `m_bValidateResult` (`FALSE`).
    pub validate_result: bool,
    /// `m_bSqueezeSmallEdges` (`FALSE`).
    pub squeeze_small_edges: bool,
    /// `m_bSplitEdgesWithVertices` (`TRUE`).
    pub split_edges_with_vertices: bool,
    /// `m_bRemoveLaminarSlivers` (`FALSE`).
    pub remove_laminar_slivers: bool,
}

/// What the merge reads from and does to the B-rep holding the added topology,
/// beyond what the intersector needs ([`IntersectorBrep`], [`IntersectorBrepP2`];
/// `SmBrep::GetEdges` / `GetVertices` are its `brep_edges` / `brep_vertices`). Each
/// method names the upstream function it stands for. The callees ported on arena
/// B-reps are required methods (an implementation routes them to the named
/// function); those whose unit is not ported yet default to [`SolidError::Unported`]
/// (tagged `UNPORTED(<unit>)`) until it lands.
pub trait MergeBrep<O>: IntersectorBrepP2<O> {
    /// `SmEdge::GetVertices(rVertices)` (`topology::edge::vertices`, no attribute).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn edge_vertices(&self, edge: O) -> Result<Vec<O>>;

    /// `SmFace::GetFaceuses(rpFU1, rpFU2)` (`topology::face::p2::faceuses`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn face_faceuses(&self, face: O) -> Result<(Option<O>, Option<O>)>;

    /// `SmFaceuse::GetShell` (`topology::edgeuse::p1::faceuse_shell`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn faceuse_shell(&self, faceuse: O) -> Result<Option<O>>;

    /// `SmFace::CalculateBoundingBox(rBox)` with its default `bTight = FALSE`
    /// (`query::face::p1::calculate_bounding_box(.., false)`; the merge ignores its
    /// status).
    ///
    /// # Errors
    ///
    /// Those of the face box.
    fn face_bounding_box(&self, k: &Kernel, face: O) -> Result<Extent3d>;

    /// `SmStitch::StitchVertexPairs(rVerts1, rVerts2, ...)` (two-list overload,
    /// `heal::stitch::p1::Stitch::stitch_vertex_pairs` of a `Stitch` built from
    /// `stitch` with a default callback) on the vertices of `brep`: glued entries of
    /// both lists become `None`.
    ///
    /// # Errors
    ///
    /// Those of the stitch.
    fn stitch_vertex_pairs(
        &mut self,
        k: &mut Kernel,
        brep: O,
        stitch: &StitchSettings,
        list1: &mut [Option<O>],
        list2: &mut [Option<O>],
    ) -> Result<StitchedVertices<O>>;

    /// `SmStitch::SplitEdgesWithVertices(rVerts, rEdges, ...)`
    /// (`heal::stitch::p2`, as [`MergeBrep::stitch_vertex_pairs`]): used vertices
    /// become `None`, processed edges `None`, and the new edges are appended to
    /// `edges`.
    ///
    /// # Errors
    ///
    /// Those of the stitch.
    fn split_edges_with_vertices(
        &mut self,
        k: &mut Kernel,
        brep: O,
        stitch: &StitchSettings,
        verts: &mut [Option<O>],
        edges: &mut Vec<Option<O>>,
    ) -> Result<SplitEdges<O>>;

    /// `SmStitch::StitchEdgesOfVertices(rVerts, ...)` (`heal::stitch::p1`, as
    /// [`MergeBrep::stitch_vertex_pairs`]; null entries are skipped).
    ///
    /// # Errors
    ///
    /// Those of the stitch.
    fn stitch_edges_of_vertices(
        &mut self,
        k: &mut Kernel,
        brep: O,
        stitch: &StitchSettings,
        verts: &[Option<O>],
    ) -> Result<StitchedEdges<O>>;

    /// `SmBrep::TestClosureSplitRegion(bTestClosure, pFaceuse, rpNewShell,
    /// rpNewRegion)` (`topology::brep::p6::test_closure_split_region`): the new shell
    /// and region.
    ///
    /// # Errors
    ///
    /// Those of the region split.
    fn test_closure_split_region(
        &mut self,
        k: &mut Kernel,
        brep: O,
        test_closure: bool,
        faceuse: O,
    ) -> Result<(Option<O>, Option<O>)>;

    /// `SmBrep::FindAndSetInfiniteRegion()` with its defaults: no shell to test,
    /// `SM_VS_INFINITEREGION_SWAP`, no bounding box
    /// (`topology::brep::p5::find_and_set_infinite_region`).
    ///
    /// # Errors
    ///
    /// Those of the region search.
    fn find_and_set_infinite_region(&mut self, k: &mut Kernel, brep: O) -> Result<()>;

    /// `SmBrep::Notify(eOperation, pBrep, NULL, NULL)` (`topology::brep::p2::notify`
    /// with the B-rep as caller).
    ///
    /// # Errors
    ///
    /// Those of the notification.
    fn notify_brep(&mut self, k: &mut Kernel, brep: O, operation: NotifyOperation) -> Result<()>;

    /// `SmBrep::IsManifoldSolid` (`topology::brep::p2::is_manifold_solid`).
    ///
    /// # Errors
    ///
    /// Those of the topology walk.
    fn is_manifold_solid(&self, brep: O) -> Result<bool>;

    /// `SmBrep::OrientTrimmedSurfaces(bArg1, rbMaybeNotClosed, bArg3)` (no face
    /// list): returns `rbMaybeNotClosed`.
    ///
    /// The owner of the B-rep implements it with
    /// [`crate::topology::brep::p3::orient_trimmed_surfaces`].
    ///
    /// # Errors
    ///
    /// Those of the orientation.
    fn orient_trimmed_surfaces(
        &mut self,
        k: &mut Kernel,
        brep: O,
        arg1: bool,
        arg3: bool,
    ) -> Result<bool>;
}

/// The four flags of [`MergeState::merge_added_topology`], in the order of
/// upstream's definition.
///
/// The header declares `bDoGluing` (unused) *before* the four flags, but the
/// definition takes it *after* them, so a caller relying on the declared defaults
/// `(TRUE, FALSE, TRUE, TRUE, TRUE)` passes `bMakingManifoldSolid = TRUE` and
/// `bFastEdgeCompare = FALSE`. [`Default`] is that effective result (see
/// `plan/upstream-issues.md`); both upstream callers pass all four explicitly.
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean arguments
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeAddedOptions {
    /// `m_bMakingManifoldSolid`.
    pub making_manifold_solid: bool,
    /// `m_bFastEdgeCompare`.
    pub fast_edge_compare: bool,
    /// `m_bDoRegionNesting`.
    pub do_region_nesting: bool,
    /// `m_bIgnoreProblems`.
    pub ignore_problems: bool,
}

impl Default for MergeAddedOptions {
    /// The declared defaults as the definition receives them: `(TRUE, FALSE, TRUE,
    /// TRUE)`.
    fn default() -> Self {
        Self {
            making_manifold_solid: true,
            fast_edge_compare: false,
            do_region_nesting: true,
            ignore_problems: true,
        }
    }
}

/// `MergeAddedFace`'s fixed flags: `(FALSE, TRUE, TRUE, TRUE)`.
pub const MERGE_ADDED_FACE_OPTIONS: MergeAddedOptions = MergeAddedOptions {
    making_manifold_solid: false,
    fast_edge_compare: true,
    do_region_nesting: true,
    ignore_problems: true,
};

/// What [`MergeState::merge_added_topology`] returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MergedTopology {
    /// `rnNumEdgesStitched`: the glued and imprinted edges (null entries included).
    pub num_edges_stitched: usize,
    /// `rdMaxVertGap`.
    pub max_vert_gap: f64,
    /// `rdMaxEdgeGap`.
    pub max_edge_gap: f64,
    /// `rdMinUnstitchedVertGap`.
    pub min_unstitched_vert_gap: f64,
    /// `rdMinUnstitchedEdgeGap`.
    pub min_unstitched_edge_gap: f64,
}

const OP_ADDED: &str = "boolean::merge::p2::merge_added_topology";

/// `SmTArray::FindElement` + `AddUnique`.
fn add_unique<O: PartialEq>(list: &mut Vec<O>, item: O) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// `if (list.FindElement(item, lIdx)) list[lIdx] = NULL;` (the first occurrence).
fn null_first<O: PartialEq + Copy>(list: &mut [Option<O>], item: O) {
    if let Some(slot) = list.iter_mut().find(|s| **s == Some(item)) {
        *slot = None;
    }
}

/// A status upstream ignores, except the errors that cannot be ignored here:
/// unported callees and undefined behaviour upstream.
fn ignore_soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }),
        ) => Err(e),
        Err(_) => Ok(None),
    }
}

fn tolerance_of<O, B: IntersectorBrep<O>>(brep: &B, topo: O) -> Result<f64> {
    brep.topology_tolerance(topo)
        .ok_or(dangling(OP_ADDED, "topology handle is unknown"))
}

/// The other mate of `entity` when it is of `kind` (`SM_CAST_PTR`).
fn other_mate_of_kind<O: ClassObject + SolutionObject + Ord>(
    ti: &TopologyIntersector<O>,
    entity: O,
    kind: ClassObjectKind,
) -> Option<O> {
    ti.other_mate(Some(entity))
        .filter(|o| o.class_kind() == kind)
}

impl<O: ClassObject + SolutionObject + Ord> MergeState<TopologyIntersector<O>> {
    /// `MergeAddedFace`: [`MergeState::merge_added_topology`] of one face of the
    /// primary B-rep with its edges and vertices, with
    /// [`MERGE_ADDED_FACE_OPTIONS`]. `bDoGluing` is unused upstream.
    ///
    /// # Errors
    ///
    /// Those of [`MergeState::merge_added_topology`] and of the face queries.
    pub fn merge_added_face<B: MergeBrep<O>>(
        &mut self,
        k: &mut Kernel,
        brep: &mut B,
        face: O,
    ) -> Result<MergedTopology> {
        // Wrap up a call to MergeAddedTopology().
        let faces = [face];
        let edges = brep.face_edges(face)?;
        let vertices = brep.face_vertices(face)?;
        self.merge_added_topology(
            k,
            brep,
            Some(&faces),
            Some(&edges),
            Some(&vertices),
            MERGE_ADDED_FACE_OPTIONS,
        )
    }

    /// `MergeAddedTopology`: glue topology just added to the primary B-rep to the
    /// rest of it, then intersect, insert and merge what crosses. Lists that are
    /// `None` are gathered from the added faces (edges) or faces and edges
    /// (vertices). Both lists are culled to each other's loose bounding boxes; then
    /// coincident vertices are stitched, vertices split edges, coincident edges are
    /// glued, vertices imprint faces, edges cross edges and faces, edges imprint
    /// faces, faces cross and coincident faces are glued, re-mating deleted
    /// topology each time; finally regions are split and nested and a manifold
    /// result oriented. The intersector's tolerance and the B-rep's editing flag are
    /// restored on every exit.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) when an added face,
    /// edge or vertex is not in the primary B-rep, and where upstream dereferences
    /// null or indexes past a list; those of the B-rep queries; statuses of the
    /// stitch, intersection and region calls are ignored as upstream, except
    /// [`SolidError::Unported`] and [`SolidError::InvalidInput`].
    pub fn merge_added_topology<B: MergeBrep<O>>(
        &mut self,
        k: &mut Kernel,
        brep: &mut B,
        faces: Option<&[O]>,
        edges: Option<&[O]>,
        vertices: Option<&[O]>,
        options: MergeAddedOptions,
    ) -> Result<MergedTopology> {
        let master = self
            .intersector
            .primary_brep()
            .ok_or(dangling(OP_ADDED, "no primary B-rep"))?;

        // We reset our tol in this method. Make sure it ends up where it started.
        let tol = self.intersector.this_approx_tol_3d();
        // set Brep to allow modifications
        let editing = brep.set_editing_enabled(master, true)?;
        let result = self.merge_added_body(k, brep, master, tol, faces, edges, vertices, options);
        // ~SmTemporaryChangeValue, in reverse construction order
        let restored = brep.set_editing_enabled(master, editing);
        self.intersector.set_this_approx_tol_3d(tol);
        let out = result?;
        restored?;
        Ok(out)
    }

    fn merge_added_body<B: MergeBrep<O>>(
        &mut self,
        k: &mut Kernel,
        brep: &mut B,
        master: O,
        d3d_tol: f64,
        faces: Option<&[O]>,
        edges: Option<&[O]>,
        vertices: Option<&[O]>,
        options: MergeAddedOptions,
    ) -> Result<MergedTopology> {
        brep.notify_brep(k, master, NotifyOperation::PreEdit)?;

        // init some output.
        let mut out = MergedTopology {
            num_edges_stitched: 0,
            max_vert_gap: 0.0,
            max_edge_gap: 0.0,
            min_unstitched_vert_gap: BIG_DOUBLE,
            min_unstitched_edge_gap: BIG_DOUBLE,
        };

        // Collect vertices, edges, and faces to be merged in.
        let mut added_faces: Vec<O> = faces.map(<[O]>::to_vec).unwrap_or_default();
        let added_edges: Vec<O> = if let Some(edges) = edges {
            edges.to_vec()
        } else {
            let mut list = Vec::new();
            for &f in &added_faces {
                for e in brep.face_edges(f)? {
                    add_unique(&mut list, e);
                }
            }
            list
        };
        let added_verts: Vec<O> = if let Some(vertices) = vertices {
            vertices.to_vec()
        } else {
            // Collect from both Faces and Edges.
            let mut list = Vec::new();
            for &f in &added_faces {
                for v in brep.face_vertices(f)? {
                    add_unique(&mut list, v);
                }
            }
            for &e in &added_edges {
                for v in brep.edge_vertices(e)? {
                    add_unique(&mut list, v);
                }
            }
            list
        };

        if added_faces.len() + added_edges.len() + added_verts.len() < 1 {
            return Ok(out); // Nothing to add in.
        }

        // Double check that all added topology is in pMasterBrep.
        for &f in &added_faces {
            if brep.face_brep(f)? != Some(master) {
                return Err(SolidError::invalid_input(
                    OP_ADDED,
                    "added face not in the B-rep",
                ));
            }
        }
        for &e in &added_edges {
            if brep.edge_brep_handle(e)? != Some(master) {
                return Err(SolidError::invalid_input(
                    OP_ADDED,
                    "added edge not in the B-rep",
                ));
            }
        }
        for &v in &added_verts {
            if brep.vertex_brep(v)? != Some(master) {
                return Err(SolidError::invalid_input(
                    OP_ADDED,
                    "added vertex not in the B-rep",
                ));
            }
        }

        // Collect vertices, edges, and faces from original Master (before merging),
        // and remove those in the Added arrays.
        let mut master_faces = brep.brep_faces(master)?;
        master_faces.retain(|f| !added_faces.contains(f));
        let mut master_edges = brep.brep_edges(master)?;
        master_edges.retain(|e| !added_edges.contains(e));
        let mut master_verts = brep.brep_vertices(master)?;
        master_verts.retain(|v| !added_verts.contains(v));

        if master_faces.len() + master_edges.len() + master_verts.len() < 1 {
            return Ok(out); // Nothing to add to.
        }

        // Bounding Box checks: approx, just do verts and edges, don't worry about
        // interiors of Faces. A failed box query leaves the previous box.
        let mut master_box = Extent3d::default();
        let mut this_box = Extent3d::default();
        let mut temp_box = Extent3d::default();
        for (faces, edges, verts, bx) in [
            (&master_faces, &master_edges, &master_verts, &mut master_box),
            (&added_faces, &added_edges, &added_verts, &mut this_box),
        ] {
            for &f in faces {
                if let Some(b) = ignore_soft(brep.face_bounding_box(k, f))? {
                    temp_box = b;
                }
                *bx = bx.union(&temp_box);
            }
            for &e in edges {
                if let Some(b) = ignore_soft(brep.edge_bounding_box(k, e))? {
                    temp_box = b;
                }
                *bx = bx.union(&temp_box);
            }
            for &v in verts {
                bx.add_point(brep.vertex_point(v)?);
            }
        }

        // We did cheap (loose) BBox checks, so expand them a bit:
        this_box.expand_absolute(10.0 * d3d_tol);
        master_box.expand_absolute(10.0 * d3d_tol);

        if master_box.are_disjoint(&this_box, 0.0) {
            return Ok(out);
        }

        // Cull all topology lists to the other's bounding box.
        let mut bbox = Extent3d::default();
        let cull_verts = |brep: &B, verts: Vec<O>, bx: &Extent3d| -> Result<Vec<O>> {
            let mut kept = Vec::new();
            for v in verts {
                if bx.contains_point(brep.vertex_point(v)?, 0.0) {
                    kept.push(v);
                }
            }
            Ok(kept)
        };
        let added_verts = cull_verts(brep, added_verts, &master_box)?;
        let mut added_e = Vec::new();
        for e in added_edges {
            if let Some(b) = ignore_soft(brep.edge_bounding_box(k, e))? {
                bbox = b;
            }
            if !master_box.are_disjoint(&bbox, tolerance_of(brep, e)?) {
                added_e.push(Some(e));
            }
        }
        let mut kept = Vec::new();
        for f in added_faces {
            if let Some(b) = ignore_soft(brep.face_bounding_box(k, f))? {
                bbox = b;
            }
            if !master_box.are_disjoint(&bbox, tolerance_of(brep, f)?) {
                kept.push(f);
            }
        }
        added_faces = kept;
        let master_verts = cull_verts(brep, master_verts, &this_box)?;
        let mut master_e = Vec::new();
        for e in master_edges {
            if let Some(b) = ignore_soft(brep.edge_bounding_box(k, e))? {
                bbox = b;
            }
            if !this_box.are_disjoint(&bbox, tolerance_of(brep, e)?) {
                master_e.push(Some(e));
            }
        }
        let mut kept = Vec::new();
        for f in master_faces {
            if let Some(b) = ignore_soft(brep.face_bounding_box(k, f))? {
                bbox = b;
            }
            if !this_box.are_disjoint(&bbox, tolerance_of(brep, f)?) {
                kept.push(f);
            }
        }
        master_faces = kept;
        let mut master_v: Vec<Option<O>> = master_verts.into_iter().map(Some).collect();
        let mut added_v: Vec<Option<O>> = added_verts.into_iter().map(Some).collect();

        // ****    Vert-Vert Coinc:    ****
        // cbiToleranceSetting == 1: the small tolerances are 4x, the big ones 8x.
        let small_tol = d3d_tol * 4.0;
        let big_tol = d3d_tol * 8.0;
        let mut stitch = StitchSettings {
            stitch_tol_3d: small_tol,
            making_manifold_solid: options.making_manifold_solid,
            ignore_problems: options.ignore_problems,
            do_region_nesting: options.do_region_nesting,
            fast_edge_compare: options.fast_edge_compare,
            validate_result: false,
            squeeze_small_edges: false,
            split_edges_with_vertices: true,
            remove_laminar_slivers: false,
        };
        let pairs =
            ignore_soft(brep.stitch_vertex_pairs(k, master, &stitch, &mut master_v, &mut added_v))?
                .unwrap_or_else(StitchedVertices::initial);
        out.max_vert_gap = pairs.max_vertex_gap;
        out.min_unstitched_vert_gap = pairs.min_unstitched_vert_gap;
        // sProcessedVerts (upstream appends null entries the intersectors return).
        let mut processed_verts: Vec<Option<O>> =
            pairs.processed.iter().copied().map(Some).collect();

        // A glued New vertex has been deleted: switch its mapping to the vertex (in
        // Master) that it was glued to.
        for (ii, &del) in pairs.deleted.iter().enumerate() {
            if let Some(this_v) =
                other_mate_of_kind(&self.intersector, del, ClassObjectKind::Vertex)
            {
                ignore_soft(
                    self.intersector
                        .remove_relationship(Some(del), Some(this_v)),
                )?;
                let survivor = *pairs
                    .processed
                    .get(ii)
                    .ok_or(dangling(OP_ADDED, "fewer processed than deleted vertices"))?;
                ignore_soft(
                    self.intersector
                        .relate(k, brep, Some(survivor), Some(this_v)),
                )?;
            }
        }

        // ****    Vert-Edge xSect:    ****
        // First see about This vertices splitting Master edges.
        stitch.stitch_tol_3d = small_tol;
        let split = ignore_soft(brep.split_edges_with_vertices(
            k,
            master,
            &stitch,
            &mut added_v,
            &mut master_e,
        ))?
        .unwrap_or_else(SplitEdges::initial);
        out.max_vert_gap = max2(out.max_vert_gap, split.max_vertex_gap);
        out.min_unstitched_vert_gap =
            min2(out.min_unstitched_vert_gap, split.min_unstitched_vert_gap);
        processed_verts.extend(split.processed_verts.into_iter().map(Some));

        // Now see about Master vertices splitting This edges (the new edges are added
        // to the edge list in the routine).
        let split = ignore_soft(brep.split_edges_with_vertices(
            k,
            master,
            &stitch,
            &mut master_v,
            &mut added_e,
        ))?
        .unwrap_or_else(SplitEdges::initial);
        out.max_vert_gap = max2(out.max_vert_gap, split.max_vertex_gap);
        out.min_unstitched_vert_gap =
            min2(out.min_unstitched_vert_gap, split.min_unstitched_vert_gap);
        processed_verts.extend(split.processed_verts.into_iter().map(Some));

        // ****    Edge-Edge Coinc:    ****
        // Corresponding vertices are already glued, so a bigger tol is in order.
        stitch.stitch_tol_3d = big_tol;
        let glued =
            ignore_soft(brep.stitch_edges_of_vertices(k, master, &stitch, &processed_verts))?
                .unwrap_or_else(StitchedEdges::initial);
        out.max_edge_gap = max2(out.max_edge_gap, glued.max_edge_gap);
        out.min_unstitched_edge_gap =
            min2(out.min_unstitched_edge_gap, glued.min_unstitched_edge_gap);

        // Re-mate deleted edges, and take deleted and glued edges out of the
        // Master/Added lists.
        for (ii, &del) in glued.deleted.iter().enumerate() {
            let survivor = *glued
                .glued
                .get(ii)
                .ok_or(dangling(OP_ADDED, "fewer glued than deleted edges"))?;
            if let Some(this_e) = other_mate_of_kind(&self.intersector, del, ClassObjectKind::Edge)
            {
                ignore_soft(
                    self.intersector
                        .remove_relationship(Some(del), Some(this_e)),
                )?;
                ignore_soft(
                    self.intersector
                        .relate(k, brep, Some(survivor), Some(this_e)),
                )?;
            }
            null_first(&mut master_e, del);
            null_first(&mut added_e, del);
            null_first(&mut master_e, survivor);
            null_first(&mut added_e, survivor);
        }
        let mut glued_edges: Vec<Option<O>> = glued.glued.into_iter().map(Some).collect();

        // ****    Vertex-Face xSect:    ****
        let master_f: Vec<Option<O>> = master_faces.iter().copied().map(Some).collect();
        let added_f: Vec<Option<O>> = added_faces.iter().copied().map(Some).collect();
        self.intersector.set_this_approx_tol_3d(small_tol);
        for (verts, faces) in [(&mut added_v, &master_f), (&mut master_v, &added_f)] {
            let (gap, min_unstitched, processed) =
                match ignore_soft(self.intersector.vertex_face_intersect(
                    k,
                    brep,
                    verts,
                    faces,
                    options.making_manifold_solid,
                    options.ignore_problems,
                ))? {
                    Some(vf) => (
                        vf.max_vertex_gap,
                        vf.min_unstitched_vert_gap,
                        vf.processed_verts,
                    ),
                    None => (0.0, BIG_DOUBLE, Vec::new()),
                };
            out.max_vert_gap = max2(out.max_vert_gap, gap);
            out.min_unstitched_vert_gap = min2(out.min_unstitched_vert_gap, min_unstitched);
            processed_verts.extend(processed.into_iter().map(Some));
        }
        // Done with the vertex lists (upstream resets them against stale pointers).
        let _ = (master_v, added_v);

        // ****    Edge-Edge xSect:    ****
        self.intersector.set_this_approx_tol_3d(small_tol);
        let ee = ignore_soft(self.intersector.edge_edge_intersect(
            k,
            brep,
            &master_e,
            &added_e,
            options.making_manifold_solid,
            options.ignore_problems,
        ))?
        .unwrap_or(EdgeEdgeIntersect {
            split_edges1: Vec::new(),
            new_edges1: Vec::new(),
            split_edges2: Vec::new(),
            new_edges2: Vec::new(),
            new_verts: Vec::new(),
            max_edge_gap: 0.0,
            min_unstitched_edge_gap: BIG_DOUBLE,
        });
        out.max_vert_gap = max2(out.max_vert_gap, ee.max_edge_gap);
        out.min_unstitched_vert_gap = min2(out.min_unstitched_vert_gap, ee.min_unstitched_edge_gap);
        processed_verts.extend(ee.new_verts.into_iter().map(Some));
        // Must add new (split) edges to sMasterEdges and sAddedEdges.
        master_e.extend(ee.new_edges1);
        added_e.extend(ee.new_edges2);

        // ****    Edge-Face xSect:    ****
        self.intersector.set_this_approx_tol_3d(small_tol);
        // First, Added edges intersecting Master faces.
        let ef = ignore_soft(self.intersector.edge_face_intersect(
            k,
            brep,
            &added_e,
            &master_f,
            options.making_manifold_solid,
            options.ignore_problems,
        ))?;
        let (gap, min_unstitched, new_verts, new_edges) = match ef {
            Some(ef) => (
                ef.max_vertex_gap,
                ef.min_unstitched_vert_gap,
                ef.new_verts,
                ef.new_edges,
            ),
            None => (0.0, BIG_DOUBLE, Vec::new(), Vec::new()),
        };
        out.max_edge_gap = max2(out.max_edge_gap, gap);
        out.min_unstitched_edge_gap = min2(out.min_unstitched_edge_gap, min_unstitched);
        processed_verts.extend(new_verts);
        // Must add new (split) edges to sAddedEdges.
        added_e.extend(new_edges);

        // Now, original Master edges intersecting Added faces.
        let ef = ignore_soft(self.intersector.edge_face_intersect(
            k,
            brep,
            &master_e,
            &added_f,
            options.making_manifold_solid,
            options.ignore_problems,
        ))?;
        let (gap, min_unstitched, new_verts, new_edges) = match ef {
            Some(ef) => (
                ef.max_vertex_gap,
                ef.min_unstitched_vert_gap,
                ef.new_verts,
                ef.new_edges,
            ),
            None => (0.0, BIG_DOUBLE, Vec::new(), Vec::new()),
        };
        out.max_edge_gap = max2(out.max_edge_gap, gap);
        out.min_unstitched_edge_gap = min2(out.min_unstitched_edge_gap, min_unstitched);
        processed_verts.extend(new_verts);
        // Must add new (split) edges to sMasterEdges.
        master_e.extend(new_edges);

        // Done with the edge lists (upstream resets them against stale pointers).
        let _ = (master_e, added_e);

        // ****    Edge-Face Coinc:    ****
        self.intersector.set_this_approx_tol_3d(big_tol);
        let efc = ignore_soft(self.intersector.edge_face_coincidence(
            k,
            brep,
            &processed_verts,
            &glued_edges,
            options.ignore_problems,
            options.do_region_nesting,
        ))?
        .unwrap_or(EdgeFaceCoincidence {
            imprinted_edges: Vec::new(),
            processed_faces: Vec::new(),
            new_faces: Vec::new(),
            split_faces: Vec::new(),
            max_edge_gap: 0.0,
            this_min_unstitched: BIG_DOUBLE,
        });
        out.max_edge_gap = max2(out.max_edge_gap, efc.max_edge_gap);
        out.min_unstitched_edge_gap = min2(out.min_unstitched_edge_gap, efc.this_min_unstitched);
        // Imprinted edges get added to GluedEdges list.
        glued_edges.extend(efc.imprinted_edges.into_iter().map(Some));
        let mut imprinted_faces = efc.processed_faces;
        // If a face was split, add the new face to sMasterFaces or sAddedFaces.
        for (ii, split_face) in efc.split_faces.iter().enumerate() {
            let new_face = *efc
                .new_faces
                .get(ii)
                .ok_or(dangling(OP_ADDED, "fewer new than split faces"))?;
            if master_faces.contains(split_face) {
                master_faces.push(new_face);
            } else if added_faces.contains(split_face) {
                added_faces.push(new_face);
            }
        }

        // ****    Face-Face xSect:    ****
        self.intersector.set_this_approx_tol_3d(small_tol);
        // FindCommonElements(rInputVerts, sFaceVerts) never matches a null entry, so
        // the nulls are dropped for the intersector's non-null list.
        let input_verts: Vec<O> = processed_verts.iter().flatten().copied().collect();
        let ff = ignore_soft(self.intersector.face_face_intersect(
            k,
            brep,
            &input_verts,
            &master_faces,
            &added_faces,
            options.ignore_problems,
            options.do_region_nesting,
        ))?;
        let (gap, min_unstitched, mut processed_faces, new_faces, new_edges) = match ff {
            Some(ff) => (
                ff.max_gap,
                ff.min_unstitched,
                ff.processed_faces,
                ff.new_faces,
                ff.new_edges,
            ),
            None => (0.0, BIG_DOUBLE, Vec::new(), Vec::new(), Vec::new()),
        };
        out.max_edge_gap = max2(out.max_edge_gap, gap);
        out.min_unstitched_edge_gap = min2(out.min_unstitched_edge_gap, min_unstitched);
        // Any new intersection edges go into sGluedEdges.
        glued_edges.extend(new_edges);
        // Add intersected faces to ImprintedFaces list.
        for &f in processed_faces.iter().chain(&new_faces) {
            add_unique(&mut imprinted_faces, f);
        }

        // ****    Face-Face Coinc:    ****
        self.intersector.set_this_approx_tol_3d(big_tol);
        let fc = ignore_soft(self.intersector.face_face_coincidence(
            k,
            brep,
            &glued_edges,
            &added_faces,
            options.ignore_problems,
            options.do_region_nesting,
        ))?;
        let (gap, min_unstitched, surviving, deleted) = match fc {
            Some(fc) => (
                fc.max_gap,
                fc.min_unstitched,
                fc.processed_faces,
                fc.deleted_faces,
            ),
            None => (0.0, BIG_DOUBLE, Vec::new(), Vec::new()),
        };
        out.max_edge_gap = max2(out.max_edge_gap, gap);
        out.min_unstitched_edge_gap = min2(out.min_unstitched_edge_gap, min_unstitched);
        for &f in &surviving {
            add_unique(&mut imprinted_faces, f);
        }
        // Also remove deleted faces from that list.
        for f in &deleted {
            if let Some(i) = imprinted_faces.iter().position(|x| x == f) {
                imprinted_faces.remove(i);
            }
        }
        // Update mapping: if the deleted and surviving sizes differ, skip the loop
        // (upstream asserts).
        let num_deleted = if deleted.len() == surviving.len() {
            deleted.len()
        } else {
            0
        };
        for (&del, &survivor) in deleted.iter().zip(&surviving).take(num_deleted) {
            if let Some(this_f) = other_mate_of_kind(&self.intersector, del, ClassObjectKind::Face)
            {
                ignore_soft(
                    self.intersector
                        .remove_relationship(Some(del), Some(this_f)),
                )?;
                ignore_soft(
                    self.intersector
                        .relate(k, brep, Some(survivor), Some(this_f)),
                )?;
            }
            // Also while we're here, add to ProcessedFace list.
            add_unique(&mut processed_faces, survivor);
        }

        // Done with combining topology. Fix up region nesting.
        for &f in &imprinted_faces {
            let (fu1, fu2) = brep.face_faceuses(f)?;
            let fu1 = fu1.ok_or(dangling(OP_ADDED, "face without face use"))?;
            let fu2 = fu2.ok_or(dangling(OP_ADDED, "face without mate face use"))?;
            // If they don't already separate two regions:
            if brep.faceuse_shell(fu1)? == brep.faceuse_shell(fu2)? {
                ignore_soft(brep.test_closure_split_region(k, master, true, fu1))?;
            }
        }
        ignore_soft(brep.find_and_set_infinite_region(k, master))?;

        // If the resulting Brep is Manifold, orient its faces.
        if brep.is_manifold_solid(master)? {
            ignore_soft(brep.orient_trimmed_surfaces(k, master, true, true))?;
        }

        out.num_edges_stitched = glued_edges.len();
        brep.notify_brep(k, master, NotifyOperation::PostEdit)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::solution_array::ObjectKind;

    /// A handle: a kind and an index.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct H(u8, u32);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Other
        }
    }

    impl SolutionObject for H {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Other
        }
    }

    const B: H = H(9, 0);
    const OTHER: H = H(9, 1);

    #[test]
    fn reverse_orientation_maps_unknown_to_same_as_the_macro() {
        assert_eq!(reverse_orientation(OrientType::Same), OrientType::Opposite);
        assert_eq!(reverse_orientation(OrientType::Opposite), OrientType::Same);
        assert_eq!(reverse_orientation(OrientType::Unknown), OrientType::Same);
    }

    #[test]
    fn merge_added_defaults_follow_the_definition_order() {
        let d = MergeAddedOptions::default();
        assert!(d.making_manifold_solid && !d.fast_edge_compare);
        assert!(d.do_region_nesting && d.ignore_problems);
        let f = MERGE_ADDED_FACE_OPTIONS;
        assert!(!f.making_manifold_solid && f.fast_edge_compare);
        assert!(f.do_region_nesting && f.ignore_problems);
    }

    #[test]
    fn constructor_keeps_given_tolerances_and_defaults_the_angle() {
        let brep = Brep::new();
        let other = Brep::new();
        let options = MergeOptions::new(true, false, false, true, true);
        let m = MergeState::with_breps(
            &Kernel::default(),
            (&brep, B),
            Some((&other, OTHER)),
            0.25,
            0.5,
            Some(&options),
        )
        .unwrap();
        assert_eq!(m.intersector.primary_brep(), Some(B));
        assert_eq!(m.intersector.other_brep(), Some(OTHER));
        assert_eq!(
            m.intersector.this_approx_tol_3d().to_bits(),
            0.25f64.to_bits()
        );
        assert_eq!(m.intersector.this_ang_tol_rad().to_bits(), 0.5f64.to_bits());
        assert_eq!(m.options, options);
        assert_eq!(m.tag_id, 0);
        // m_bManifoldBoolean(TRUE), m_bIntersectionLoopClosed(TRUE)
        assert_eq!(m.manifold_boolean, Some(true));
        assert_eq!(m.intersection_loop_closed, Some(true));
        let context_only = MergeState::new(());
        assert_eq!(context_only.manifold_boolean, None);
        assert_eq!(context_only.intersection_loop_closed, None);

        // A zero angle tolerance becomes 20 degrees; a null options pointer keeps the
        // defaults; the other B-rep may be null (the stitch entry points).
        let m = MergeState::with_breps(&Kernel::default(), (&brep, B), None, EFF_ZERO, 0.0, None)
            .unwrap();
        assert_eq!(m.intersector.other_brep(), None);
        assert_eq!(
            m.intersector.this_ang_tol_rad().to_bits(),
            (20.0 * (PI / 180.0)).to_bits()
        );
        assert_eq!(m.options, MergeOptions::default());
        // `-0.0 == 0.0`, so a negative zero is replaced too; other angles are kept.
        let m =
            MergeState::with_breps(&Kernel::default(), (&brep, B), None, 1.0, -0.0, None).unwrap();
        assert_eq!(
            m.intersector.this_ang_tol_rad().to_bits(),
            (20.0 * (PI / 180.0)).to_bits()
        );
        let m =
            MergeState::with_breps(&Kernel::default(), (&brep, B), None, 1.0, -1.0, None).unwrap();
        assert_eq!(
            m.intersector.this_ang_tol_rad().to_bits(),
            (-1.0f64).to_bits()
        );
    }

    #[test]
    fn computed_tolerance_takes_the_smaller_of_brep_and_edge_sizes() {
        // Edgeless B-reps: GetMinEdgeBBoxSize gives the standard 0.001 for both.
        let brep = Brep::new();
        let expected = min2(0.5 * min2(brep.tolerance(), brep.tolerance()), 0.001 * 0.33);
        for tol in [0.0, -1.0, EFF_ZERO / 2.0] {
            let m = MergeState::<TopologyIntersector<H>>::with_breps(
                &Kernel::default(),
                (&brep, B),
                Some((&brep, OTHER)),
                tol,
                0.0,
                None,
            )
            .unwrap();
            assert_eq!(
                m.intersector.this_approx_tol_3d().to_bits(),
                expected.to_bits()
            );
            // Without other B-rep upstream dereferences null.
            let e = MergeState::<TopologyIntersector<H>>::with_breps(
                &Kernel::default(),
                (&brep, B),
                None,
                tol,
                0.0,
                None,
            )
            .unwrap_err();
            assert!(matches!(e, SolidError::InvalidInput { .. }));
        }
    }

    #[test]
    fn wire_construction_uses_the_edge_copy_identity() {
        use crate::curve::line::Line;
        use crate::geom::attribute::{AttributeStore, ObjectKey};
        use crate::math::Vec3;
        use crate::topology::topology::Topology;
        use crate::topology::vertex;

        let mut k = Kernel::default();
        let mut b = Brep::new_with_infinite_region().unwrap();
        let (_, start) =
            crate::topology::brep::p2::make_shell_vertex_topology(&mut b, None).unwrap();
        let (_, end) = crate::topology::brep::p2::make_shell_vertex_topology(&mut b, None).unwrap();
        b.vertex_mut(end).unwrap().point = Vec3::X;
        let vertex_key = |e| ObjectRef {
            key: ObjectKey(match e {
                crate::EntityRef::Vertex(v) => u64::from(v.index()),
                crate::EntityRef::Vertexuse(v) => 6_000_000 + u64::from(v.index()),
                _ => 9_000_000,
            }),
            type_code: crate::topology::topology::TYPE,
        };
        let source_key = |_: VertexId| ObjectRef {
            key: ObjectKey(1_000_000),
            type_code: vertex::TYPE,
        };
        let edge_key = |e: EdgeId| ObjectRef {
            key: ObjectKey(2_000_000 + u64::from(e.index())),
            type_code: edge::TYPE,
        };
        let face_key = |_: FaceId| ObjectRef {
            key: ObjectKey(4_000_000),
            type_code: crate::topology::face::p2::TYPE,
        };
        let mut records = crate::topology::topology::NotifyRecords::new();
        let source = ObjectRef {
            key: ObjectKey(3_000_000),
            type_code: edge::TYPE,
        };
        let mut state = Topology::new();
        state.set_all_flags(17);
        records.insert(source.key, state);
        let mut attributes = AttributeStore::new();
        let mut copy = TopologyCopy {
            vertices: VertexCopy {
                records: &mut records,
                attributes: &mut attributes,
                source_key: &source_key,
                destination_key: &vertex_key,
            },
            edge_source_key: &edge_key,
            edge_destination_key: &edge_key,
            face_source_key: &face_key,
            face_destination_key: &face_key,
        };
        let curve = Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap();
        let edge = make_wire_edge(
            &mut MergeContext::Shared(&mut k),
            &mut b,
            None,
            [start, end],
            Curve::from(curve),
            Extent1d::from_min_max(0.0, 1.0),
            &mut copy,
        )
        .unwrap();
        copy.notify(None, source, edge_key(edge)).unwrap();
        assert_eq!(copy.vertices.records[&edge_key(edge).key].all_flags(), 17);
    }

    /// `MakeFaceuse`'s surface copy carries the source surface's attributes: a reference
    /// `SM_AI_SURFACEMAP` lists both the source surface and its copy as users (as
    /// `PiecewiseMerge`'s `RegisterExistingSSI` needs), and an owner-less surface goes by
    /// its face's arena name.
    #[test]
    fn surface_copy_carries_the_surface_map_attribute_to_a_second_user() {
        use crate::geom::attribute::{ids, Attribute, AttributeStore, Behavior, ObjectKey};
        use crate::math::Point3;
        use crate::primitive::api_primitives::create_box;
        use crate::topology::brep_cache::{arena_identity, CacheObject};

        let mut k = Kernel::default();
        let mut other = create_box(&mut k, Point3::ZERO, 1.0, 2.0, 3.0).unwrap();
        let mut brep = create_box(&mut k, Point3::ZERO, 1.0, 2.0, 3.0).unwrap();
        let from_face = other.faces().ids().next().unwrap();
        let to_face = brep.faces().ids().nth(1).unwrap();
        let face_type = crate::topology::face::p2::TYPE;
        let from = ObjectRef {
            key: ObjectKey(7_000_001),
            type_code: face_type,
        };
        let to = ObjectRef {
            key: ObjectKey(8_000_002),
            type_code: face_type,
        };
        let _ = other
            .face_mut(from_face)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .set_owner(Some(from));
        let _ = brep
            .face_mut(to_face)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .set_owner(Some(to));

        let mut attributes = AttributeStore::new();
        let map = attributes
            .insert(Attribute::pointer(ids::SURFACEMAP, 0, Behavior::Reference))
            .unwrap();
        attributes.add_attribute(from.key, map).unwrap();

        copy_surface_attributes(&mut attributes, &other, from_face, &brep, to_face).unwrap();
        assert_eq!(
            attributes.find_attribute(to.key, ids::SURFACEMAP),
            Some(map)
        );
        assert_eq!(attributes.users(map).unwrap(), vec![from.key, to.key]);
        // a second copy onto the same surface (the face notification under the same
        // name) adds no second reference
        copy_surface_attributes(&mut attributes, &other, from_face, &brep, to_face).unwrap();
        assert_eq!(attributes.users(map).unwrap(), vec![from.key, to.key]);

        // an owner-less copy goes by its face's arena name
        let _ = brep
            .face_mut(to_face)
            .unwrap()
            .surface
            .as_mut()
            .unwrap()
            .set_owner(None);
        copy_surface_attributes(&mut attributes, &other, from_face, &brep, to_face).unwrap();
        let arena = arena_identity(CacheObject::Face(to_face));
        assert_eq!(
            attributes.find_attribute(arena.key, ids::SURFACEMAP),
            Some(map)
        );
        assert_eq!(
            attributes.users(map).unwrap(),
            vec![from.key, to.key, arena.key]
        );
    }

    #[test]
    fn null_edgeuse_merges_nothing() {
        let mut brep = Brep::new();
        let mut other = Brep::new();
        let mut records = crate::topology::topology::NotifyRecords::new();
        let mut attributes = crate::geom::attribute::AttributeStore::new();
        let key = |_: VertexId| ObjectRef {
            key: crate::geom::attribute::ObjectKey(1),
            type_code: 0,
        };
        let edge_key = |_: EdgeId| ObjectRef {
            key: crate::geom::attribute::ObjectKey(2),
            type_code: 0,
        };
        let face_key = |_: FaceId| ObjectRef {
            key: crate::geom::attribute::ObjectKey(3),
            type_code: 0,
        };
        let mut copy = TopologyCopy {
            vertices: VertexCopy {
                records: &mut records,
                attributes: &mut attributes,
                source_key: &key,
                destination_key: &|_| ObjectRef {
                    key: crate::geom::attribute::ObjectKey(1),
                    type_code: 0,
                },
            },
            edge_source_key: &edge_key,
            edge_destination_key: &edge_key,
            face_source_key: &face_key,
            face_destination_key: &face_key,
        };
        let r = merge_edge(
            &mut Kernel::default(),
            &mut brep,
            &mut other,
            None,
            None,
            &mut Relations::default(),
            &mut copy,
            None,
        )
        .unwrap();
        assert_eq!(r, None);
    }
}
