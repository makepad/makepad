//! Proto topology, part 1: the containers of the Boolean's intersect-insert-relate
//! phase (vertex and edge definitions, proto vertices, edges and faces, and their
//! manager), their accessors and list management, evaluation, and the static helpers
//! of `SmProtoTopology.cpp`.
//!
//! Deviation: deleting a proto edge removes every incident entry at its endpoints
//! (#374), including the two entries of a closed edge. Upstream removes only one.

use crate::curve::curve::p2::Curve;
use crate::error::{ignore_status, ignored, EntityRef, Result, SolidError};
use crate::geom::attribute::ObjectKey;
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{xsect_tol3d, zone_tol3d};
use crate::intersect::curve_class::{src_zone_tol_3d_of, CurveClassification, CurveInterval};
use crate::intersect::point_class::{
    object_zone_tol_3d, ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{max2, min2, Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO};
use crate::query::face::p2 as face_p2;
use crate::topology::brep::p2 as brep_p2;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::topology as topo;
use crate::topology::{edge, vertex, vertexuse, Brep, EdgeId, VertexId};

use crate::curve::curve::p1::GlobalPointSolveOptions;

const OP: &str = "topology::proto_topology::p1";
const OP_POSITION: &str = "topology::proto_topology::p1::find_position_from_pcs";
const OP_EVALUATE: &str = "topology::proto_topology::p1::ProtoTopologyManager::proto_edge_evaluate";
const OP_SRC_TOPO: &str = "topology::proto_topology::p1::VertexDefinition::set_src_topo";
const OP_DEGEN: &str = "topology::proto_topology::p1::find_ed_on_degen_face";
const OP_END: &str = "topology::proto_topology::p1::ProtoTopologyManager::end_position";
const OP_COMBINE: &str =
    "topology::proto_topology::p1::ProtoTopologyManager::can_combine_edge_definitions";

/// Upstream `SmStatus` value `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

/// Upstream `SmStatus` value `SM_ERR_ASSERT_FAILURE`.
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// `SmProtoTopologyManager_TYPE` (`TOPO_BASE_TYPE + 680`).
pub const MANAGER_TYPE: i64 = 16680;

/// `SmProtoTopologyManager::GetTypeString`.
pub const MANAGER_TYPE_STRING: &str = "SmProtoTopologyManager_TYPE";

/// `SmProtoTopologyManager::GetClassString`.
pub const MANAGER_CLASS_STRING: &str = "SmProtoTopologyManager";

/// `SmProtoTopologyManager::IsKindOf`: [`MANAGER_TYPE`], then the `SmTopology` chain.
#[must_use]
pub const fn manager_is_kind_of(t: i64) -> bool {
    if t == MANAGER_TYPE {
        true
    } else {
        topo::is_kind_of(t)
    }
}

macro_rules! proto_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            /// The creation index of the object in its manager.
            #[must_use]
            pub const fn index(self) -> u32 {
                self.0
            }

            /// The id with creation index `index`.
            #[must_use]
            pub const fn from_index(index: u32) -> Self {
                Self(index)
            }
        }
    };
}

proto_id!(
    /// Identifies a [`VertexDefinition`] of a [`ProtoTopologyManager`].
    VertexDefId
);
proto_id!(
    /// Identifies an [`EdgeDefinition`] of a [`ProtoTopologyManager`].
    EdgeDefId
);
proto_id!(
    /// Identifies a [`ProtoVertex`] of a [`ProtoTopologyManager`].
    ProtoVertexId
);
proto_id!(
    /// Identifies a [`ProtoEdge`] of a [`ProtoTopologyManager`].
    ProtoEdgeId
);
proto_id!(
    /// Identifies a [`ProtoFace`] of a [`ProtoTopologyManager`].
    ProtoFaceId
);

/// Mutable access to the curves behind handles, for the edits of an
/// [`EdgeDefinition`]'s curves (reversal, reparameterisation).
pub trait ProtoGeometry<O>: ClassGeometry<O> {
    /// The curve of a [`ClassObjectKind::Curve`] handle.
    fn curve_mut(&mut self, o: O) -> Option<&mut Curve>;
}

/// The B-rep topology walks the proto topology makes over the handles' vertices,
/// edges and faces (`sm_AreConnected`, `SmProtoVertex::GetFinalVertex`,
/// `sm_FindEDOnDegenFace`). [`Brep`] answers them for [`EntityRef`] handles of its
/// own entities.
pub trait ProtoTopology<O> {
    /// `SmVertex::GetVertexuses`, each use's face (`SmVertexuse::GetFaceuse`, then
    /// `SmFaceuse::GetFace`): `None` for a use without face use or face.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn vertex_use_faces(&self, vertex: O) -> Result<Vec<Option<O>>>;
    /// `SmEdge::GetEdgeuses`, each use's face (`SmEdgeuse::GetFace`).
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_use_faces(&self, edge: O) -> Result<Vec<Option<O>>>;
    /// `SmEdge::GetStartVertex`.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk (an edge without primary edge use
    /// is a `NULL` dereference upstream).
    fn edge_start_vertex(&self, edge: O) -> Result<Option<O>>;
    /// `SmEdge::GetOtherVertex`: `None` when `vertex` is not an end of the edge.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_other_vertex(&self, edge: O, vertex: Option<O>) -> Result<Option<O>>;
    /// `SmVertex::GetPoint`.
    ///
    /// # Errors
    ///
    /// Those of the implementation's lookup.
    fn vertex_point(&self, vertex: O) -> Result<Point3>;
    /// `SmEdge::GetBrep`: the identity of the edge's B-rep, `None` without one.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_brep(&self, edge: O) -> Result<Option<ObjectKey>>;
    /// `SmEdge::GetFaces`.
    ///
    /// # Errors
    ///
    /// Those of the implementation's topology walk.
    fn edge_faces(&self, edge: O) -> Result<Vec<O>>;
}

/// An [`EntityRef`] of `kind`, else an invalid-input error (a failed `dynamic_cast`
/// that upstream never reaches).
fn vertex_id(o: EntityRef) -> Result<VertexId> {
    match o {
        EntityRef::Vertex(v) => Ok(v),
        _ => Err(dangling("handle is not a vertex")),
    }
}

fn edge_id(o: EntityRef) -> Result<EdgeId> {
    match o {
        EntityRef::Edge(e) => Ok(e),
        _ => Err(dangling("handle is not an edge")),
    }
}

/// The walks over one B-rep's own entities, through units `topology.vertex`,
/// `topology.vertexuse`, `topology.edge` and `topology.edgeuse_p1`.
impl ProtoTopology<EntityRef> for Brep {
    fn vertex_use_faces(&self, vertex: EntityRef) -> Result<Vec<Option<EntityRef>>> {
        let mut out = Vec::new();
        for vu in vertex::vertexuses(self, vertex_id(vertex)?)? {
            let face = match vertexuse::faceuse(self, vu)? {
                Some(fu) => eu1::faceuse_face(self, fu)?,
                None => None,
            };
            out.push(face.map(EntityRef::Face));
        }
        Ok(out)
    }

    fn edge_use_faces(&self, edge: EntityRef) -> Result<Vec<Option<EntityRef>>> {
        let mut out = Vec::new();
        for eu in edge::edgeuses(self, edge_id(edge)?, None, None)? {
            out.push(eu1::face(self, eu)?.map(EntityRef::Face));
        }
        Ok(out)
    }

    fn edge_start_vertex(&self, edge: EntityRef) -> Result<Option<EntityRef>> {
        Ok(edge::start_vertex(self, edge_id(edge)?)?.map(EntityRef::Vertex))
    }

    fn edge_other_vertex(
        &self,
        edge: EntityRef,
        vertex: Option<EntityRef>,
    ) -> Result<Option<EntityRef>> {
        let v = match vertex {
            Some(v) => Some(vertex_id(v)?),
            None => None,
        };
        Ok(edge::other_vertex(self, edge_id(edge)?, v)?.map(EntityRef::Vertex))
    }

    fn vertex_point(&self, vertex: EntityRef) -> Result<Point3> {
        vertex::point(self, vertex_id(vertex)?)
    }

    fn edge_brep(&self, edge: EntityRef) -> Result<Option<ObjectKey>> {
        Ok(eu1::edge_brep(self, edge_id(edge)?)?.map(Brep::key))
    }

    fn edge_faces(&self, edge: EntityRef) -> Result<Vec<EntityRef>> {
        Ok(edge::faces(self, edge_id(edge)?, None)?
            .into_iter()
            .map(EntityRef::Face)
            .collect())
    }
}

/// The handle the curve point solves name (only the solution count is used).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// An upstream status other than `SM_ERR` / `SM_ERR_NULL_POINTER`.
const fn status(operation: &'static str, code: i32) -> SolidError {
    SolidError::Upstream {
        operation,
        code,
        entity: None,
    }
}

/// A dereference of a null pointer or a deleted object upstream (undefined
/// behaviour).
const fn dangling(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// `SM_CAST_PTR(SmTopology, obj)`: the object when it is a topology object.
fn as_topology<O: ClassObject>(o: Option<O>) -> Option<O> {
    o.filter(|o| {
        matches!(
            o.class_kind(),
            ClassObjectKind::Vertex
                | ClassObjectKind::Edge
                | ClassObjectKind::Face
                | ClassObjectKind::Loop
                | ClassObjectKind::Region
                | ClassObjectKind::Vertexuse
                | ClassObjectKind::Edgeuse
                | ClassObjectKind::Faceuse
                | ClassObjectKind::Loopuse
        )
    })
}

/// The curve behind `o` (a `NULL` dereference upstream for none).
fn curve_of<O: Copy>(geometry: &impl ClassGeometry<O>, o: Option<O>) -> Result<&Curve> {
    o.and_then(|o| geometry.curve(o))
        .ok_or(dangling("curve handle is null or unknown"))
}

/// A status upstream ignores, except the errors that cannot be ignored here: unported
/// callees and undefined behaviour upstream.
fn ignore_soft<T>(r: Result<T>) -> Result<()> {
    if let Err(
        e @ (SolidError::Unported { .. }
        | SolidError::InvalidInput { .. }
        | SolidError::PortInvariant { .. }),
    ) = r
    {
        return Err(e);
    }
    Ok(())
}

/// `FindObjPoint3d` with its status ignored: the object point, `SetUninitialized`
/// on failure.
fn obj_point<O: ClassObject>(
    k: &Kernel,
    pc: &PointClassification<O>,
    geometry: &impl ClassGeometry<O>,
) -> Result<Point3> {
    Ok(ignored(pc.find_obj_point_3d(k, geometry))?.unwrap_or(Vec3::UNDEF))
}

/// `EvaluatePoint` with its status ignored: the point, `SM_UNDEF` on failure.
fn eval_point(k: &Kernel, curve: &Curve, t: f64) -> Result<Point3> {
    Ok(ignored(curve.evaluate_point(k, t))?.unwrap_or(Vec3::UNDEF))
}

// ---- static helpers ------------------------------------------------------------------

/// Whether a classification of type `t` can be imprinted (`sm_CanImprint`): regions,
/// faces, edges and vertices.
#[must_use]
pub fn can_imprint(t: PointClassType) -> bool {
    t == PointClassType::Region
        || t == PointClassType::Face
        || t == PointClassType::Edge
        || t == PointClassType::Vertex
}

/// The dimension of a topology object (`sm_TopoDimension`): 0 for a vertex, 1 for an
/// edge, 2 for a face, -1 otherwise.
#[must_use]
pub fn topo_dimension<O: ClassObject>(topo: O) -> i32 {
    match topo.class_kind() {
        ClassObjectKind::Vertex => 0,
        ClassObjectKind::Edge => 1,
        ClassObjectKind::Face => 2,
        _ => -1,
    }
}

/// The result of [`find_position_from_pcs`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionFromPcs {
    /// `rPosition`: the position the two classifications agree on.
    pub position: Point3,
    /// `rdDeviation`: the distance between the two object points (0 when only one
    /// was known at the start).
    pub deviation: f64,
}

/// The position two point classifications define (`sm_FindPositionFromPCs`). A
/// classification without parameters gets them from the other's object point when
/// possible. With both points known, the lower-dimensional object wins; at equal
/// dimension the points are blended towards the one with the smaller gap.
///
/// # Errors
///
/// `SM_ERR` when neither classification has parameters; those of
/// [`PointClassification::find_obj_point_3d`] and of the objects' zone tolerances
/// ([`SolidError::Unported`] for topology objects).
pub fn find_position_from_pcs<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    pc1: &mut PointClassification<O>,
    pc2: &mut PointClassification<O>,
) -> Result<PositionFromPcs> {
    let mut pt1 = Vec3::UNDEF;
    let mut pt2 = Vec3::UNDEF;

    // These should have been set in the SmVertexDefinition constructor.
    let mut found1 = pc1.are_parameters_set();
    let mut found2 = pc2.are_parameters_set();

    // error - both Parameters are not Set
    if !found1 && !found2 {
        return Err(SolidError::sm_err(OP_POSITION));
    }

    if found1 {
        pt1 = pc1.find_obj_point_3d(k, geometry)?;
    }
    if found2 {
        pt2 = pc2.find_obj_point_3d(k, geometry)?;
    }

    // (SmTol::GetXSectTol3d of the two objects only feeds a diagnostic message when
    // the deviation exceeds it; it is not computed here.)

    // deviation = dist between PtClassifyObjects when both point classifies have
    // ParametersSet, else 0.0
    let deviation = if found1 && found2 {
        pt1.distance(pt2)
    } else {
        0.0
    };

    let mut position = Vec3::UNDEF;
    if !found1 {
        // See if we can find a position for rPC1 from sPt2.
        let ok = ignored(pc1.compute_point_parameters(k, geometry, pt2))?.is_some();
        if ok && pc1.are_parameters_set() {
            pt1 = pc1.find_obj_point_3d(k, geometry)?;
            found1 = true;
        } else {
            position = pt2;
        }
    } else if !found2 {
        // See if we can find a position for rPC2 from sPt1.
        let ok = ignored(pc2.compute_point_parameters(k, geometry, pt1))?.is_some();
        if ok && pc2.are_parameters_set() {
            pt2 = pc2.find_obj_point_3d(k, geometry)?;
            found2 = true;
        } else {
            position = pt1;
        }
    }

    // when both PCs have Parameters
    if found1 && found2 {
        // high dimension means low weight
        let dim1 = as_topology(pc1.object()).map_or(3, topo_dimension);
        let dim2 = as_topology(pc2.object()).map_or(3, topo_dimension);

        // Give more weight to the one with lower dimension.
        position = match dim1.cmp(&dim2) {
            core::cmp::Ordering::Less => pt1,
            core::cmp::Ordering::Greater => pt2,
            core::cmp::Ordering::Equal => {
                // Same dimension. Give more weight to the one with the tighter gap.
                let gap1 = pc1.gap_3d(k, geometry, None)?;
                let gap2 = pc2.gap_3d(k, geometry, None)?;
                let sum = gap1 + gap2;
                let frac = if sum > EFF_ZERO { gap1 / sum } else { 0.5 };
                (1.0 - frac) * pt1 + frac * pt2
            }
        };
    }

    Ok(PositionFromPcs {
        position,
        deviation,
    })
}

/// Whether two intersection curves are meant to be the same (`sm_AreSameCurve`): the
/// same curve over the same interval, or the midpoint of the first drops onto the
/// second (an intersecting drop, guessed at the second interval's middle) closer than
/// `tol`.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a missing curve that is dereferenced (undefined
/// upstream); [`SolidError::Unported`] from the drop.
pub fn are_same_curve<O: Copy + PartialEq>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    crv1: Option<O>,
    ivl1: &Extent1d,
    crv2: Option<O>,
    ivl2: &Extent1d,
    tol: f64,
) -> Result<bool> {
    // low work - same curve.
    if crv1 == crv2 {
        // return TRUE when intervals are the same
        return Ok(ivl1.are_same(ivl2));
    }

    // locals
    let param1 = ivl1.mid();
    let guess = ivl2.mid();

    // Crv1 mid point
    let mid_pt1 = eval_point(k, curve_of(geometry, crv1)?, param1)?;

    // Drop Crv1 mid point to Crv2 - get drop distance
    let dropped = ignored(curve_of(geometry, crv2)?.drop_point(
        k,
        ivl2,
        mid_pt1,
        None,
        tol,
        Some(guess),
        SolverOperation::Intersect,
    ))?;

    // return TRUE when DropPoint() succeeded and DropDist < dTol
    Ok(dropped.is_some_and(|d| d.success && d.distance < tol))
}

/// `sm_AreConnected(const SmVertex*, const SmFace*)`.
fn are_connected_vertex_face<O: Copy + PartialEq>(
    topology: &impl ProtoTopology<O>,
    vtx: O,
    face: O,
) -> Result<bool> {
    for f in topology.vertex_use_faces(vtx)? {
        if f == Some(face) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// `sm_AreConnected(const SmEdge*, const SmFace*)`.
fn are_connected_edge_face<O: Copy + PartialEq>(
    topology: &impl ProtoTopology<O>,
    edge: O,
    face: O,
) -> Result<bool> {
    for f in topology.edge_use_faces(edge)? {
        if f == Some(face) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// `sm_AreConnected(const SmVertex*, const SmEdge*)`.
fn are_connected_vertex_edge<O: Copy + PartialEq>(
    topology: &impl ProtoTopology<O>,
    vertex: O,
    edge: O,
) -> Result<bool> {
    let vtx = topology.edge_start_vertex(edge)?;
    if vtx == Some(vertex) {
        return Ok(true);
    }

    let vtx = topology.edge_other_vertex(edge, vtx)?;
    if vtx == Some(vertex) {
        return Ok(true);
    }

    Ok(false)
}

/// Whether two topology objects are neighbours in their B-rep
/// (`sm_AreConnected(const SmTopology*, const SmTopology*)`): the same object, or a
/// face and a boundary edge or vertex, or an edge and a boundary vertex; any other
/// pair is not connected.
///
/// # Errors
///
/// Those of the `topology` walks.
pub fn are_connected<O: ClassObject>(
    topology: &impl ProtoTopology<O>,
    topo1: Option<O>,
    topo2: Option<O>,
) -> Result<bool> {
    use ClassObjectKind as K;
    if topo1 == topo2 {
        return Ok(true);
    }
    let of_kind = |o: Option<O>, k: K| o.filter(|o| o.class_kind() == k);

    if let Some(face1) = of_kind(topo1, K::Face) {
        if let Some(edge2) = of_kind(topo2, K::Edge) {
            return are_connected_edge_face(topology, edge2, face1);
        }
        if let Some(vertex2) = of_kind(topo2, K::Vertex) {
            return are_connected_vertex_face(topology, vertex2, face1);
        }
    }
    if let Some(edge1) = of_kind(topo1, K::Edge) {
        if let Some(face2) = of_kind(topo2, K::Face) {
            return are_connected_edge_face(topology, edge1, face2);
        }
        if let Some(vertex2) = of_kind(topo2, K::Vertex) {
            return are_connected_vertex_edge(topology, vertex2, edge1);
        }
    }
    if let Some(vertex1) = of_kind(topo1, K::Vertex) {
        if let Some(face2) = of_kind(topo2, K::Face) {
            return are_connected_vertex_face(topology, vertex1, face2);
        }
        if let Some(edge2) = of_kind(topo2, K::Edge) {
            return are_connected_vertex_edge(topology, vertex1, edge2);
        }
    }
    Ok(false)
}

/// Whether `param` is strictly inside `domain` of `curve`, also in 3d
/// (`sm_IsInterior`): inside the domain, and its point farther than `zone_tol_3d`
/// from both end points.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a missing curve; [`SolidError::Unported`] from
/// the evaluations.
pub fn is_interior<O: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    curve: Option<O>,
    domain: &Extent1d,
    param: f64,
    zone_tol_3d: f64,
) -> Result<bool> {
    // low work - When Param is outside of Curve domain - it's not interior
    if !domain.contains_value(param, 0.0) {
        return Ok(false);
    }

    // next: Check 3d Pts. if this param's Pt3d is within ZoneTol3d dist of a crv
    // endpoint it on the boundary not the interior
    let c = curve_of(geometry, curve)?;
    let crv_pt = eval_point(k, c, param)?;
    let start_pt = eval_point(k, c, domain.min())?;
    let end_pt = eval_point(k, c, domain.max())?;

    if crv_pt.closer_than(zone_tol_3d, start_pt) {
        return Ok(false);
    }
    if crv_pt.closer_than(zone_tol_3d, end_pt) {
        return Ok(false);
    }
    Ok(true)
}

/// Drops `point` onto `curve` over `domain` (intersecting within `tol`, guessed at
/// `guess`) and tests the drop parameter with [`is_interior`].
fn drop_is_interior<O: Copy>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    curve: Option<O>,
    domain: &Extent1d,
    point: Point3,
    tol: f64,
    guess: f64,
) -> Result<bool> {
    let c = curve_of(geometry, curve)?;
    let dropped = ignored(c.drop_point(
        k,
        domain,
        point,
        None,
        tol,
        Some(guess),
        SolverOperation::Intersect,
    ))?;
    match dropped {
        Some(d) if d.success => is_interior(k, geometry, curve, domain, d.parameter, tol),
        _ => Ok(false),
    }
}

/// Whether a curve interval and an edge definition overlap over a segment
/// (`sm_AreOverlapping`): an end of either curve drops (within the larger of the
/// interval's source zone tolerance and the definition's tolerance) to the interior
/// of the other. `classification` is the classification `interval` belongs to (its
/// curve and tolerance; upstream reaches it through the interval's back pointer).
///
/// # Errors
///
/// [`SolidError::InvalidInput`] for a missing curve; [`SolidError::Unported`] from
/// the drops.
pub fn are_overlapping<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    classification: &CurveClassification<O>,
    interval: &CurveInterval<O>,
    ed: &EdgeDefinition<O>,
) -> Result<bool> {
    // locals
    let ci_curve = classification.curve();
    let ci_domain = *interval.interval();
    let ed_curve = ed.xsect_curve_3d();
    let ed_domain = ed.xsect_interval();

    // tolerance
    let zone_tol_3d = zone_tol3d(
        k,
        max2(src_zone_tol_3d_of(k, Some(classification)), ed.tolerance(k)),
    );

    // Drop ED->Curve Start and End Pts to CI->CurveClassification->Curve
    {
        let start_pt = eval_point(k, curve_of(geometry, ed_curve)?, ed_domain.min())?;
        if drop_is_interior(
            k,
            geometry,
            ci_curve,
            &ci_domain,
            start_pt,
            zone_tol_3d,
            ci_domain.evaluate(0.1),
        )? {
            return Ok(true);
        }

        let end_pt = eval_point(k, curve_of(geometry, ed_curve)?, ed_domain.max())?;
        if drop_is_interior(
            k,
            geometry,
            ci_curve,
            &ci_domain,
            end_pt,
            zone_tol_3d,
            ci_domain.evaluate(0.9),
        )? {
            return Ok(true);
        }
    }

    // Drop CI->Curve->EndPts to ED->Curve
    {
        let start_pt = eval_point(k, curve_of(geometry, ci_curve)?, ci_domain.min())?;
        if drop_is_interior(
            k,
            geometry,
            ed_curve,
            &ed_domain,
            start_pt,
            zone_tol_3d,
            ci_domain.evaluate(0.1),
        )? {
            return Ok(true);
        }

        let end_pt = eval_point(k, curve_of(geometry, ci_curve)?, ci_domain.max())?;
        if drop_is_interior(
            k,
            geometry,
            ed_curve,
            &ed_domain,
            end_pt,
            zone_tol_3d,
            ed_domain.evaluate(0.9),
        )? {
            return Ok(true);
        }
    }

    // none of the CI or CD curve endpoints drop to the interior of the other curve
    Ok(false)
}

/// Which of two coincident edges of one B-rep lies on a degenerate face and can be
/// deleted (`sm_FindEDOnDegenFace`; 0 neither, 1 the first, 2 the second): 0 when the
/// edges share no face.
///
/// # Errors
///
/// `SM_ERR_ASSERT_FAILURE` for edges of different B-reps; those of the `topology`
/// walks; [`SolidError::Unported`] for a shared face (`SmFace::GetEdges`, unit
/// `topology.face_p4`).
pub fn find_ed_on_degen_face<O: Copy + PartialEq>(
    topology: &impl ProtoTopology<O>,
    edge1: O,
    edge2: O,
) -> Result<u64> {
    // Check inputs
    let brep1 = topology.edge_brep(edge1)?;
    let brep2 = topology.edge_brep(edge2)?;
    if brep1 != brep2 {
        return Err(status(OP_DEGEN, SM_ERR_ASSERT_FAILURE));
    }

    // Find shared faces (FindCommonElements: the second list's faces found in the first)
    let ed1_fs = topology.edge_faces(edge1)?;
    let ed2_fs = topology.edge_faces(edge2)?;
    let common: Vec<O> = ed2_fs.into_iter().filter(|f| ed1_fs.contains(f)).collect();

    // Degenerate face has 2 edges within tol of edge on other brep
    if let Some(face) = common.first() {
        let _ = face;
        // UNPORTED(topology.face_p4): SmFace::GetEdges, then SmBrep::DeleteFace(pFace,
        // FALSE, TRUE, &sTopos)
        // Proof (unreachable): the only caller, `CanCombineEdgeDefinitions`, gets here
        // with two different edges of one B-rep, which `sm_AreConnected` (no edge-edge
        // case, upstream and `are_connected`) reports unconnected, so it has already
        // returned 0. Upstream's `DeleteFace` without `bDeleteEdgesVertices` also leaves
        // `sTopos` empty, so even a reached call would answer 0 (its only effect being
        // the face deletion).
        return Err(SolidError::unported("SmFace::GetEdges", "topology.face_p4"));
    }

    // If no common faces, or deleted common face w/out deleting edge, then we have
    // coincident unstitched edges
    Ok(0)
}

/// `lCanDelete` after `sm_FindEDOnDegenFace`, whose status `CanCombineEdgeDefinitions`
/// ignores: 0 on an error return (set before any error past the B-rep check). Unported
/// callees and undefined behaviour upstream still fail.
fn degen_face_choice(r: Result<u64>) -> Result<u64> {
    match r {
        Ok(n) => Ok(n),
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }),
        ) => Err(e),
        Err(_) => Ok(0),
    }
}

// ---- SmVertexDefinition -----------------------------------------------------------------

/// One point intersection between a topology object of each B-rep
/// (`SmVertexDefinition`).
#[derive(Debug, Clone, PartialEq)]
pub struct VertexDefinition<O> {
    /// `m_sPC1`: the B-rep 1 classification of the point.
    pc1: PointClassification<O>,
    /// `m_sPC2`: the B-rep 2 classification of the point.
    pc2: PointClassification<O>,
    /// `m_sPosition`.
    position: Point3,
    /// `m_dTol`.
    tol: f64,
    /// `m_dGap`.
    gap: f64,
    /// `m_pMyProtoVertex`.
    proto_vertex: Option<ProtoVertexId>,
    /// `m_pFinalVtx1`.
    final_vtx1: Option<O>,
    /// `m_pFinalVtx2`.
    final_vtx2: Option<O>,
    /// Not upstream (WR-410, source issue #1059; the source fork's
    /// `m_dCrossingSine`): the sine of the angle at which the intersection curve that
    /// placed this definition crosses its source edge; `-1` when no such crossing placed
    /// it ([`VertexDefinition::place_by_crossing`]).
    crossing_sine: f64,
}

/// Not upstream (WR-410; the source fork's `sm_CrossingError`): how far along its
/// edge a crossing at angle `asin(crossing_sine)`, whose points lie `gap` apart, may
/// misplace a vertex definition: the gap over the sine, or `BIG_DOUBLE` when the curve
/// runs along the edge (within the angular tolerance) or the angle is unknown.
fn crossing_error(gap: f64, crossing_sine: f64) -> f64 {
    // (upstream's `!(sine >= min)`: a NaN sine is unknown too)
    if crossing_sine.partial_cmp(&crate::geom::tol::ang_tol_rad().sin())
        == Some(core::cmp::Ordering::Less)
        || crossing_sine.is_nan()
    {
        return BIG_DOUBLE;
    }
    max2(gap, 0.0) / crossing_sine
}

impl<O: ClassObject> VertexDefinition<O> {
    /// `SmVertexDefinition(pCtx, cpPC1, cpPC2, rPosition, dTol, dGap, pOwner)`: copies
    /// the classifications and computes their parameters at `position` when unset
    /// (statuses ignored); a non-positive `gap` becomes the distance between the two
    /// object points, at least either classification's gap.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the classifications (topology objects, surface
    /// point solves).
    pub fn new(
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pc1: &PointClassification<O>,
        pc2: &PointClassification<O>,
        position: Point3,
        tol: f64,
        gap: f64,
        owner: Option<ProtoVertexId>,
    ) -> Result<Self> {
        let mut vd = Self {
            pc1: *pc1,
            pc2: *pc2,
            position,
            tol,
            gap,
            proto_vertex: owner,
            final_vtx1: None,
            final_vtx2: None,
            crossing_sine: -1.0,
        };

        if !vd.pc1.are_parameters_set() {
            ignore_status(vd.pc1.compute_point_parameters(k, geometry, position))?;
        }
        if !vd.pc2.are_parameters_set() {
            ignore_status(vd.pc2.compute_point_parameters(k, geometry, position))?;
        }

        if gap <= 0.0 {
            let pt1 = obj_point(k, &vd.pc1, geometry)?;
            let pt2 = obj_point(k, &vd.pc2, geometry)?;
            vd.gap = pt1.distance(pt2);
            vd.gap = max2(vd.gap, pc1.gap_3d(k, geometry, None)?);
            vd.gap = max2(vd.gap, pc2.gap_3d(k, geometry, None)?);
        }
        Ok(vd)
    }

    /// The position (`GetPosition`).
    #[must_use]
    pub const fn position(&self) -> Point3 {
        self.position
    }

    /// Not upstream (WR-410, source issue #1059; the source fork's
    /// `SmVertexDefinition::PlaceByCrossing`): places this definition by another
    /// intersection curve's crossing of its source edge (`pc1`, `pc2`, the crossing's
    /// classifications against this definition's objects, at angle
    /// `asin(crossing_sine)`) when that crossing fixes it more closely. The first
    /// crossing placed it. A later one replaces the point parameters on the source
    /// objects (an edge's t, a face's uv), the gap, the position and the crossing angle
    /// only when its points lie within both objects' tolerance of each other (a farther
    /// one is another point the proto vertex took in) and its misplacement bound
    /// ([`crossing_error`]) is smaller. Returns whether it was placed again.
    ///
    /// # Errors
    ///
    /// Those of `FindObjPoint3d`, the objects' zone tolerances and
    /// [`find_position_from_pcs`].
    pub fn place_by_crossing(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pc1: &PointClassification<O>,
        pc2: &PointClassification<O>,
        crossing_sine: f64,
    ) -> Result<bool> {
        if self.crossing_sine < 0.0 {
            // The first crossing to reach this VertexDefinition placed it.
            self.crossing_sine = crossing_sine;
            return Ok(false);
        }

        // This VertexDefinition with the crossing's point parameters.
        let mut pcs = [self.pc1, self.pc2];
        for (pc, new) in pcs.iter_mut().zip([pc1, pc2]) {
            if !new.are_parameters_set() {
                return Ok(false);
            }
            match pc.point_class() {
                PointClassType::Edge => pc.set_t_param(new.t_param(), false),
                PointClassType::Face => pc.set_uv_param(new.uv_param()),
                _ => continue,
            }
            pc.set_gap_3d(new.gap_3d(k, geometry, None)?);
        }
        let [mut pc1, mut pc2] = pcs;
        let pt1 = pc1.find_obj_point_3d(k, geometry)?;
        let pt2 = pc2.find_obj_point_3d(k, geometry)?;
        let gap = max2(
            max2(pt1.distance(pt2), pc1.gap_3d(k, geometry, None)?),
            pc2.gap_3d(k, geometry, None)?,
        );

        // Points farther apart than both source objects' tolerances are no point on
        // both: the crossing is another point that the ProtoVertex's containment radius
        // took in (the far end of a short curve it absorbs), not this one placed again.
        let obj_tol = max2(
            pc1.obj_zone_tol_3d(k, geometry)?,
            pc2.obj_zone_tol_3d(k, geometry)?,
        );
        // (upstream's `!(gap <= tol)` and `!(new < old)`: NaN keeps the placement)
        if !matches!(
            gap.partial_cmp(&obj_tol),
            Some(core::cmp::Ordering::Less | core::cmp::Ordering::Equal)
        ) {
            return Ok(false);
        }
        let (new_error, old_error) = (
            crossing_error(gap, crossing_sine),
            crossing_error(self.gap, self.crossing_sine),
        );
        if new_error.partial_cmp(&old_error) != Some(core::cmp::Ordering::Less) {
            return Ok(false);
        }

        let position = find_position_from_pcs(k, geometry, &mut pc1, &mut pc2)?;
        self.pc1 = pc1;
        self.pc2 = pc2;
        self.position = position.position;
        self.tol = max2(self.tol, position.deviation);
        self.gap = gap;
        self.crossing_sine = crossing_sine;
        Ok(true)
    }

    /// The tolerance (`GetTolerance`, as an `SmZoneTol3d`).
    #[must_use]
    pub fn tolerance(&self, k: &Kernel) -> f64 {
        zone_tol3d(k, self.tol)
    }

    /// The gap between the two source objects (`GetGap`).
    #[must_use]
    pub const fn gap(&self) -> f64 {
        self.gap
    }

    /// The vertex made in B-rep `which` (1, else 2) by the imprint
    /// (`GetFinalVertex`).
    #[must_use]
    pub const fn final_vertex(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.final_vtx1
        } else {
            self.final_vtx2
        }
    }

    /// The owning proto vertex (`GetProtoVertex`).
    #[must_use]
    pub const fn proto_vertex(&self) -> Option<ProtoVertexId> {
        self.proto_vertex
    }

    /// The B-rep 1 topology object (`GetBrep1Topo`: the classification object when
    /// it is topology).
    #[must_use]
    pub fn brep1_topo(&self) -> Option<O> {
        as_topology(self.pc1.object())
    }

    /// The B-rep 2 topology object (`GetBrep2Topo`).
    #[must_use]
    pub fn brep2_topo(&self) -> Option<O> {
        as_topology(self.pc2.object())
    }

    /// The B-rep 1 classification type (`GetBrep1TopoType`).
    #[must_use]
    pub const fn brep1_topo_type(&self) -> PointClassType {
        self.pc1.point_class()
    }

    /// The B-rep 2 classification type (`GetBrep2TopoType`).
    #[must_use]
    pub const fn brep2_topo_type(&self) -> PointClassType {
        self.pc2.point_class()
    }

    /// The topology object in B-rep `which` (1, else 2) (`GetSrcTopo`).
    #[must_use]
    pub fn src_topo(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.brep1_topo()
        } else {
            self.brep2_topo()
        }
    }

    /// The classification type in B-rep `which` (`GetSrcTopo_TYPE`).
    #[must_use]
    pub const fn src_topo_type(&self, which: i32) -> PointClassType {
        if which == 1 {
            self.brep1_topo_type()
        } else {
            self.brep2_topo_type()
        }
    }

    /// The edge parameter in B-rep `which` (`GetBrepTParam`).
    #[must_use]
    pub const fn brep_t_param(&self, which: i32) -> f64 {
        if which == 1 {
            self.pc1.t_param()
        } else {
            self.pc2.t_param()
        }
    }

    /// The face parameters in B-rep `which` (`GetBrepUVParam`).
    #[must_use]
    pub const fn brep_uv_param(&self, which: i32) -> Point2 {
        if which == 1 {
            self.pc1.uv_param()
        } else {
            self.pc2.uv_param()
        }
    }

    /// The classification in B-rep `which` (`GetBrepPointClassif`).
    #[must_use]
    pub const fn brep_point_classif(&self, which: i32) -> &PointClassification<O> {
        if which == 1 {
            &self.pc1
        } else {
            &self.pc2
        }
    }

    /// Mutable access to the classification in B-rep `which` (`GetBrepPointClassif`).
    pub fn brep_point_classif_mut(&mut self, which: i32) -> &mut PointClassification<O> {
        if which == 1 {
            &mut self.pc1
        } else {
            &mut self.pc2
        }
    }

    /// Sets the position (`SetPosition`).
    pub fn set_position(&mut self, position: Point3) {
        self.position = position;
    }

    /// Sets the owning proto vertex (`SetProtoVertex`).
    pub fn set_proto_vertex(&mut self, pv: Option<ProtoVertexId>) {
        self.proto_vertex = pv;
    }

    /// Sets the vertex of B-rep `which` (1, else 2) (`SetFinalVertex`).
    pub fn set_final_vertex(&mut self, which: i32, vertex: Option<O>) {
        if which == 1 {
            self.final_vtx1 = vertex;
        } else {
            self.final_vtx2 = vertex;
        }
    }

    /// Sets the classification object of B-rep `which` (`SetSrcTopo`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `which` other than 1 and 2.
    pub fn set_src_topo(&mut self, which: i32, t: PointClassType, topo: Option<O>) -> Result<()> {
        if which == 1 {
            self.pc1.set_class_object(t, topo);
        } else if which == 2 {
            self.pc2.set_class_object(t, topo);
        } else {
            return Err(SolidError::sm_err(OP_SRC_TOPO));
        }
        Ok(())
    }

    /// Recomputes both classifications' parameters at the position
    /// (`SetPointParameters`; statuses ignored).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the classifications.
    pub fn set_point_parameters(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
    ) -> Result<()> {
        ignore_status(
            self.pc1
                .compute_point_parameters(k, geometry, self.position),
        )?;
        ignore_status(
            self.pc2
                .compute_point_parameters(k, geometry, self.position),
        )?;
        Ok(())
    }
}

// ---- SmEdgeDefinition -------------------------------------------------------------------

/// The `SM_TYPE` of a source topology object, as an [`EdgeDefinition`] caches it
/// (`m_pSrcTopo1_TYPE`).
fn src_type<O: ClassObject>(topo: O) -> ClassObjectKind {
    topo.class_kind()
}

/// One curve intersection between a topology object of each B-rep
/// (`SmEdgeDefinition`). Its curves are handles to the caller's geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeDefinition<O> {
    /// `m_pXSectCurve3d`.
    xsect_curve_3d: Option<O>,
    /// `m_pXSectUVCurve1`.
    xsect_uv_curve1: Option<O>,
    /// `m_pXSectUVCurve2`.
    xsect_uv_curve2: Option<O>,
    /// `m_pXSectInterval`.
    xsect_interval: Extent1d,
    /// `m_pSrcTopo1`.
    src_topo1: Option<O>,
    /// `m_pSrcTopo2`.
    src_topo2: Option<O>,
    /// `m_pSrcTopo1_TYPE`.
    src_topo1_type: ClassObjectKind,
    /// `m_pSrcTopo2_TYPE`.
    src_topo2_type: ClassObjectKind,
    /// `m_pProtoEdge`.
    proto_edge: Option<ProtoEdgeId>,
    /// `m_dTol`.
    tol: f64,
    /// `m_pFinalEdge1`.
    final_edge1: Option<O>,
    /// `m_pFinalEdge2`.
    final_edge2: Option<O>,
}

impl<O: ClassObject> EdgeDefinition<O> {
    /// `SmEdgeDefinition(pXSectCurve3d, pXSectInterval, pSrcTopo1, pSrcTopo2, dTol,
    /// pXSectUVCurve1, pXSectUVCurve2, pMyProtoEdge)`; the source objects' types are
    /// cached.
    #[must_use]
    pub fn new(
        xsect_curve_3d: Option<O>,
        xsect_interval: Extent1d,
        src_topo1: O,
        src_topo2: O,
        tol: f64,
        xsect_uv_curve1: Option<O>,
        xsect_uv_curve2: Option<O>,
        proto_edge: Option<ProtoEdgeId>,
    ) -> Self {
        Self {
            xsect_curve_3d,
            xsect_uv_curve1,
            xsect_uv_curve2,
            xsect_interval,
            src_topo1: Some(src_topo1),
            src_topo2: Some(src_topo2),
            src_topo1_type: src_type(src_topo1),
            src_topo2_type: src_type(src_topo2),
            proto_edge,
            tol,
            final_edge1: None,
            final_edge2: None,
        }
    }

    /// The copy constructor `SmEdgeDefinition(const SmEdgeDefinition&)`: everything
    /// but the final edges (left null).
    #[must_use]
    pub fn copy_definition(&self) -> Self {
        Self {
            final_edge1: None,
            final_edge2: None,
            ..self.clone()
        }
    }

    /// The destructor `~SmEdgeDefinition`: the curves it deletes, those not consumed
    /// by an imprint (the 3d and first UV curve when B-rep 1's source is an edge, the
    /// second UV curve when B-rep 2's is).
    #[must_use]
    pub fn destroy(mut self) -> Vec<O> {
        let mut deleted = Vec::new();
        if self.src_topo_type(1) == PointClassType::Edge {
            deleted.extend(self.xsect_curve_3d.take());
            deleted.extend(self.xsect_uv_curve1.take());
        }
        if self.src_topo_type(2) == PointClassType::Edge {
            deleted.extend(self.xsect_uv_curve2.take());
        }
        deleted
    }

    /// Deletes all three curves (`Destruct`): the handles deleted.
    pub fn destruct(&mut self) -> Vec<O> {
        let mut deleted = Vec::new();
        deleted.extend(self.xsect_curve_3d.take());
        deleted.extend(self.xsect_uv_curve1.take());
        deleted.extend(self.xsect_uv_curve2.take());
        deleted
    }

    /// The 3d intersection curve (`GetXSectCurve3d`).
    #[must_use]
    pub const fn xsect_curve_3d(&self) -> Option<O> {
        self.xsect_curve_3d
    }

    /// The curve interval (`GetXSectInterval`).
    #[must_use]
    pub const fn xsect_interval(&self) -> Extent1d {
        self.xsect_interval
    }

    /// The tolerance (`GetTolerance`, as an `SmZoneTol3d`).
    #[must_use]
    pub fn tolerance(&self, k: &Kernel) -> f64 {
        zone_tol3d(k, self.tol)
    }

    /// The raw tolerance `m_dTol` (read directly by `Imprint`).
    #[must_use]
    pub const fn raw_tolerance(&self) -> f64 {
        self.tol
    }

    /// The UV curve on B-rep `which`'s face (1, else 2) (`GetXSectUVCurve`).
    #[must_use]
    pub const fn xsect_uv_curve(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.xsect_uv_curve1
        } else {
            self.xsect_uv_curve2
        }
    }

    /// The source object in B-rep `which` (`GetSrcTopo`).
    #[must_use]
    pub const fn src_topo(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.src_topo1
        } else {
            self.src_topo2
        }
    }

    /// The classification type of the cached source type in B-rep `which`
    /// (`GetSrcTopo_TYPE`): vertex, edge, face, region, else unknown.
    #[must_use]
    pub fn src_topo_type(&self, which: i32) -> PointClassType {
        let t = if which == 1 {
            self.src_topo1_type
        } else {
            self.src_topo2_type
        };
        match t {
            ClassObjectKind::Vertex => PointClassType::Vertex,
            ClassObjectKind::Edge => PointClassType::Edge,
            ClassObjectKind::Face => PointClassType::Face,
            ClassObjectKind::Region => PointClassType::Region,
            _ => PointClassType::Unknown,
        }
    }

    /// The owning proto edge (`GetProtoEdge`).
    #[must_use]
    pub const fn proto_edge(&self) -> Option<ProtoEdgeId> {
        self.proto_edge
    }

    /// The edge made in B-rep `which` (`GetFinalEdge`).
    #[must_use]
    pub const fn final_edge(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.final_edge1
        } else {
            self.final_edge2
        }
    }

    /// Sets the edge made in B-rep `which` (1, else 2) (the imprint's write of
    /// `m_pFinalEdge`).
    pub fn set_final_edge(&mut self, which: i32, edge: Option<O>) {
        if which == 1 {
            self.final_edge1 = edge;
        } else {
            self.final_edge2 = edge;
        }
    }

    /// Sets the 3d curve (`SetXSectCurve3d`).
    pub fn set_xsect_curve_3d(&mut self, curve: Option<O>) {
        self.xsect_curve_3d = curve;
    }

    /// Sets the UV curve of B-rep `which` (1, else 2) (`SetXSectCurveUV`).
    pub fn set_xsect_curve_uv(&mut self, which: i32, curve: Option<O>) {
        if which == 1 {
            self.xsect_uv_curve1 = curve;
        } else {
            self.xsect_uv_curve2 = curve;
        }
    }

    /// Sets the owning proto edge (`SetProtoEdge`).
    pub fn set_proto_edge(&mut self, pe: Option<ProtoEdgeId>) {
        self.proto_edge = pe;
    }

    /// Sets the curve interval (`SetXSectInterval(const SmExtent1d&)`).
    pub fn set_xsect_interval(&mut self, interval: Extent1d) {
        self.xsect_interval = interval;
    }

    /// Sets the source object of B-rep `which` (1, else 2) and caches its type
    /// (`SetSrcTopo`; the classification type is unused).
    pub fn set_src_topo(&mut self, which: i32, _t: PointClassType, topo: O) {
        if which == 1 {
            self.src_topo1 = Some(topo);
            self.src_topo1_type = src_type(topo);
        } else {
            self.src_topo2 = Some(topo);
            self.src_topo2_type = src_type(topo);
        }
    }

    /// The 3d curve's point at the start (or end) of the interval
    /// (`GetEndPosition(SmBoolean, SmPoint3d&)`; the evaluation status is ignored).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a curve; [`SolidError::Unported`] from
    /// the evaluation.
    pub fn end_position(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        at_start: bool,
    ) -> Result<Point3> {
        // pick end param
        let param = if at_start {
            self.xsect_interval.min()
        } else {
            self.xsect_interval.max()
        };

        // evaluate Pos3d
        eval_point(k, curve_of(geometry, self.xsect_curve_3d)?, param)
    }

    /// The point and/or tangent of the 3d curve at `param` (`Evaluate`): `None` for
    /// what was not asked.
    ///
    /// # Errors
    ///
    /// Those of the curve's evaluation; [`SolidError::InvalidInput`] without a curve.
    pub fn evaluate(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        param: f64,
        want_pos: bool,
        want_tan: bool,
    ) -> Result<(Option<Point3>, Option<Vec3>)> {
        if !want_pos && !want_tan {
            return Ok((None, None));
        }

        let curve = curve_of(geometry, self.xsect_curve_3d)?;
        if !want_tan {
            return Ok((Some(curve.evaluate_point(k, param)?), None));
        }

        // Caller wants the deriv vector.
        let pt_vec = curve.evaluate(k, param, 1, true, true)?;
        let pos = pt_vec
            .first()
            .copied()
            .ok_or(dangling("evaluation result"))?;
        let tan = pt_vec
            .get(1)
            .copied()
            .ok_or(dangling("evaluation result"))?;
        Ok((want_pos.then_some(pos), Some(tan)))
    }

    /// [`EdgeDefinition::evaluate`] at the middle of the interval (`EvaluateMid`).
    ///
    /// # Errors
    ///
    /// Those of [`EdgeDefinition::evaluate`].
    pub fn evaluate_mid(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        want_pos: bool,
        want_tan: bool,
    ) -> Result<(Option<Point3>, Option<Vec3>)> {
        let param = self.xsect_interval.mid();
        self.evaluate(k, geometry, param, want_pos, want_tan)
    }

    /// Reverses the parameterisation of the 3d and UV curves, each updating the
    /// interval in turn (`ReverseOrientation`).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::reverse_parameterization`] (the first failure stops);
    /// [`SolidError::InvalidInput`] for an unknown curve handle.
    pub fn reverse_orientation(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoGeometry<O>,
    ) -> Result<()> {
        for c in [
            self.xsect_curve_3d,
            self.xsect_uv_curve1,
            self.xsect_uv_curve2,
        ]
        .into_iter()
        .flatten()
        {
            let curve = geometry
                .curve_mut(c)
                .ok_or(dangling("curve handle is unknown"))?;
            self.xsect_interval = curve.reverse_parameterization(k, &self.xsect_interval)?;
        }
        Ok(())
    }

    /// Sets the interval from two values, reversing the curves first when
    /// `min > max` (`SetXSectInterval(double, double)`; the reversal's status is
    /// ignored).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the reversal.
    pub fn set_xsect_interval_min_max(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoGeometry<O>,
        min: f64,
        max: f64,
    ) -> Result<()> {
        if min <= max {
            ignore_status(self.xsect_interval.set_min_max(min, max))?;
        } else {
            ignore_soft(self.reverse_orientation(k, geometry))?;
            ignore_status(self.xsect_interval.set_min_max(max, min))?;
        }
        Ok(())
    }

    /// Reparameterises the curves to `new_domain`, reversing them first when asked
    /// (`Reparameterize`; the reversal's status is ignored). Nothing is edited when
    /// the domain is already the interval (to the scaled zero).
    ///
    /// # Errors
    ///
    /// Those of [`Curve::edit_parameterization`]; [`SolidError::InvalidInput`]
    /// without a 3d curve.
    pub fn reparameterize(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoGeometry<O>,
        new_domain: &Extent1d,
        reverse: bool,
    ) -> Result<()> {
        if reverse {
            ignore_soft(self.reverse_orientation(k, geometry))?;
        }

        if new_domain.are_same(&self.xsect_interval) {
            return Ok(());
        }

        self.xsect_interval = *new_domain;

        let c3 = self
            .xsect_curve_3d
            .and_then(|c| geometry.curve_mut(c))
            .ok_or(dangling("curve handle is null or unknown"))?;
        c3.edit_parameterization(k, new_domain)?;

        for c in [self.xsect_uv_curve1, self.xsect_uv_curve2]
            .into_iter()
            .flatten()
        {
            geometry
                .curve_mut(c)
                .ok_or(dangling("curve handle is unknown"))?
                .edit_parameterization(k, new_domain)?;
        }
        Ok(())
    }

    /// Whether `position` is an interior point of the definition near its start (or
    /// end) (`IsInteriorPoint`): farther than `tol` from that end, dropping onto the
    /// 3d curve (minimising, guessed at that end) within `tol` of it, in that half of
    /// the interval, with the drop point farther than `tol` from the end. The parameter is the drop
    /// parameter when the result is `true` (or the end parameter when the position
    /// is at the end).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a curve; [`SolidError::Unported`] from
    /// the drop.
    pub fn is_interior_point(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        position: Point3,
        tol: f64,
        at_start: bool,
    ) -> Result<(bool, f64)> {
        let guess = if at_start {
            self.xsect_interval.min()
        } else {
            self.xsect_interval.max()
        };
        let mut param = guess;

        let end_pos = self.end_position(k, geometry, at_start)?;
        let dist = position.distance(end_pos);
        if dist < tol {
            return Ok((false, param));
        }

        // Drop the point to our 3d curve.
        let dropped = ignored(curve_of(geometry, self.xsect_curve_3d)?.drop_point(
            k,
            &self.xsect_interval,
            position,
            None,
            tol,
            Some(guess),
            SolverOperation::Minimize,
        ))?;
        let Some(d) = dropped else {
            return Ok((false, param));
        };
        param = d.parameter;
        if !d.success {
            return Ok((false, param));
        }

        // SM_SO_MINIMIZE succeeds for any converged closest point, however far away.
        // A true result lets UpdateEdges() trim this EdgeDef to the parameter, so the
        // position must actually lie on the curve. (`!(dDropDist <= sTol3d)`: a NaN
        // distance is rejected.)
        if d.distance.is_nan() || d.distance > tol {
            return Ok((false, param));
        }

        // Sometimes for very short Edges, the point will be at or near the far end.
        if at_start && param > self.xsect_interval.mid() {
            return Ok((false, param));
        }
        if !at_start && param < self.xsect_interval.mid() {
            return Ok((false, param));
        }

        // See whether the split point is interior to this EdgeDef (in 3d).
        let edge_pt = match self.evaluate(k, geometry, param, true, false) {
            Ok((Some(p), _)) => p,
            Err(e) if e.is_unported() => return Err(e),
            _ => Vec3::UNDEF,
        };
        let dist = edge_pt.distance(end_pos);
        Ok((dist > tol, param))
    }
}

// ---- SmProtoVertex ----------------------------------------------------------------------

/// A set of vertex definitions at about the same position (`SmProtoVertex`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProtoVertex {
    /// `m_sPosition`.
    position: Point3,
    /// `m_dTol`.
    tol: f64,
    /// `m_sVertexDefinitions`.
    vertex_defs: Vec<VertexDefId>,
    /// `m_sProtoEdges` (a closed proto edge is listed twice).
    proto_edges: Vec<ProtoEdgeId>,
    /// The SMLib fork's `m_bAbsorbedProtoEdge`: a degenerate proto edge of ours was dropped,
    /// so our vertices stand for its curve (WR-306, opt-in).
    absorbed_proto_edge: bool,
}

impl ProtoVertex {
    /// Whether a degenerate proto edge of this proto vertex was dropped (the fork's
    /// `AbsorbedProtoEdge`, WR-306).
    #[must_use]
    pub const fn absorbed_proto_edge(&self) -> bool {
        self.absorbed_proto_edge
    }

    /// Marks a degenerate proto edge of this proto vertex as dropped (the fork's
    /// `SetAbsorbedProtoEdge`, WR-306).
    pub fn set_absorbed_proto_edge(&mut self) {
        self.absorbed_proto_edge = true;
    }

    /// The position (`GetPosition`).
    #[must_use]
    pub const fn position(&self) -> Point3 {
        self.position
    }

    /// Sets the position (through `GetPosition`'s reference).
    pub fn set_position(&mut self, position: Point3) {
        self.position = position;
    }

    /// The tolerance (`GetTolerance`, as an `SmZoneTol3d`).
    #[must_use]
    pub fn tolerance(&self, k: &Kernel) -> f64 {
        zone_tol3d(k, self.tol)
    }

    /// The raw tolerance `m_dTol`.
    #[must_use]
    pub const fn raw_tolerance(&self) -> f64 {
        self.tol
    }

    /// Sets the raw tolerance `m_dTol` (the writes of `AddVertexDefinition`).
    pub fn set_raw_tolerance(&mut self, tol: f64) {
        self.tol = tol;
    }

    /// `GetNumVertexDefinitions`.
    #[must_use]
    pub fn num_vertex_definitions(&self) -> usize {
        self.vertex_defs.len()
    }

    /// `GetNumProtoEdges`.
    #[must_use]
    pub fn num_proto_edges(&self) -> usize {
        self.proto_edges.len()
    }

    /// The vertex definitions (`GetVertexDefinitions`).
    #[must_use]
    pub fn vertex_definitions(&self) -> &[VertexDefId] {
        &self.vertex_defs
    }

    /// Mutable access to the vertex definitions (`GetVertexDefinitions`).
    pub fn vertex_definitions_mut(&mut self) -> &mut Vec<VertexDefId> {
        &mut self.vertex_defs
    }

    /// The connected proto edges (`GetProtoEdges`).
    #[must_use]
    pub fn proto_edges(&self) -> &[ProtoEdgeId] {
        &self.proto_edges
    }

    /// Mutable access to the connected proto edges (`GetProtoEdges`).
    pub fn proto_edges_mut(&mut self) -> &mut Vec<ProtoEdgeId> {
        &mut self.proto_edges
    }

    /// Forgets the vertex definitions (`ClearVertexDefinitions`).
    pub fn clear_vertex_definitions(&mut self) {
        self.vertex_defs.clear();
    }

    /// Lists `pe`, even when already listed (`AddProtoEdge`, design decision DD1).
    pub fn add_proto_edge(&mut self, pe: ProtoEdgeId) {
        self.proto_edges.push(pe);
    }

    /// Unlists the first entry `vd` (`RemoveVertexDefinition`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `vd` is not listed.
    pub fn remove_vertex_definition(&mut self, vd: VertexDefId) -> Result<()> {
        match self.vertex_defs.iter().position(|&x| x == vd) {
            Some(i) => {
                self.vertex_defs.remove(i);
                Ok(())
            }
            None => Err(SolidError::sm_err(OP)),
        }
    }

    /// Unlists the first entry `pe` (`RemoveProtoEdge`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `pe` is not listed.
    pub fn remove_proto_edge(&mut self, pe: ProtoEdgeId) -> Result<()> {
        match self.proto_edges.iter().position(|&x| x == pe) {
            Some(i) => {
                self.proto_edges.remove(i);
                Ok(())
            }
            None => Err(SolidError::sm_err(OP)),
        }
    }
}

// ---- SmProtoEdge ------------------------------------------------------------------------

/// A set of edge definitions along about the same curve between two proto vertices
/// (`SmProtoEdge`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProtoEdge {
    /// `m_pXSectInterval`.
    xsect_interval: Extent1d,
    /// `m_pStartPV`.
    start_pv: Option<ProtoVertexId>,
    /// `m_pEndPV`.
    end_pv: Option<ProtoVertexId>,
    /// `m_sEdgeDefs`.
    edge_defs: Vec<EdgeDefId>,
    /// `m_dTol`.
    tol: f64,
    /// `m_sOverlappingPEs`.
    overlapping_pes: Vec<ProtoEdgeId>,
    /// `m_sCoincPFs`.
    coinc_pfs: Vec<ProtoFaceId>,
}

impl ProtoEdge {
    /// The interval (`GetXSectInterval`).
    #[must_use]
    pub const fn xsect_interval(&self) -> Extent1d {
        self.xsect_interval
    }

    /// The tolerance (`GetTolerance`, as an `SmZoneTol3d`).
    #[must_use]
    pub fn tolerance(&self, k: &Kernel) -> f64 {
        zone_tol3d(k, self.tol)
    }

    /// The raw tolerance `m_dTol`.
    #[must_use]
    pub const fn raw_tolerance(&self) -> f64 {
        self.tol
    }

    /// Sets the raw tolerance `m_dTol` (the write of `CombineProtoEdge`).
    pub fn set_raw_tolerance(&mut self, tol: f64) {
        self.tol = tol;
    }

    /// The start proto vertex (`GetStartProtoVertex`).
    #[must_use]
    pub const fn start_proto_vertex(&self) -> Option<ProtoVertexId> {
        self.start_pv
    }

    /// The end proto vertex (`GetEndProtoVertex`).
    #[must_use]
    pub const fn end_proto_vertex(&self) -> Option<ProtoVertexId> {
        self.end_pv
    }

    /// The overlapping proto edges (`GetOverlappingPEs`).
    #[must_use]
    pub fn overlapping_pes(&self) -> &[ProtoEdgeId] {
        &self.overlapping_pes
    }

    /// Empties the overlapping proto edges (`GetOverlappingPEs().ReSet()`).
    pub fn clear_overlapping_pes(&mut self) {
        self.overlapping_pes.clear();
    }

    /// `GetNumEdgeDefinitions`.
    #[must_use]
    pub fn num_edge_definitions(&self) -> usize {
        self.edge_defs.len()
    }

    /// The edge definitions (`GetEdgeDefinitions`).
    #[must_use]
    pub fn edge_definitions(&self) -> &[EdgeDefId] {
        &self.edge_defs
    }

    /// Mutable access to the edge definitions (`GetEdgeDefinitions`).
    pub fn edge_definitions_mut(&mut self) -> &mut Vec<EdgeDefId> {
        &mut self.edge_defs
    }

    /// The coincident proto faces this edge bounds (`GetCoincProtoFaces`).
    #[must_use]
    pub fn coinc_proto_faces(&self) -> &[ProtoFaceId] {
        &self.coinc_pfs
    }

    /// The coincident proto faces, for the edits upstream makes to `m_sCoincPFs`.
    pub fn coinc_proto_faces_mut(&mut self) -> &mut Vec<ProtoFaceId> {
        &mut self.coinc_pfs
    }

    /// Sets the interval (`SetXSectInterval`).
    pub fn set_xsect_interval(&mut self, interval: Extent1d) {
        self.xsect_interval = interval;
    }

    /// Sets the start proto vertex (`SetStartProtoVertex`).
    pub fn set_start_proto_vertex(&mut self, pv: Option<ProtoVertexId>) {
        self.start_pv = pv;
    }

    /// Sets the end proto vertex (`SetEndProtoVertex`).
    pub fn set_end_proto_vertex(&mut self, pv: Option<ProtoVertexId>) {
        self.end_pv = pv;
    }

    /// Lists an overlapping proto edge (`AddOverlappingPE`).
    pub fn add_overlapping_pe(&mut self, pe: ProtoEdgeId) {
        self.overlapping_pes.push(pe);
    }

    /// Lists overlapping proto edges (`AppendOverlappingPEs`).
    pub fn append_overlapping_pes(&mut self, pes: &[ProtoEdgeId]) {
        self.overlapping_pes.extend_from_slice(pes);
    }

    /// Forgets the edge definitions (`ClearEdgeDefinitions`).
    pub fn clear_edge_definitions(&mut self) {
        self.edge_defs.clear();
    }

    /// Replaces `old_pv` by `new_pv` at either end (`ReplaceProtoVertex`).
    pub fn replace_proto_vertex(
        &mut self,
        old_pv: Option<ProtoVertexId>,
        new_pv: Option<ProtoVertexId>,
    ) {
        if self.start_pv == old_pv {
            self.start_pv = new_pv;
        }
        if self.end_pv == old_pv {
            self.end_pv = new_pv;
        }
    }

    /// Unlists the first entry `ed` (`RemoveEdgeDefinition`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `ed` is not listed.
    pub fn remove_edge_definition(&mut self, ed: EdgeDefId) -> Result<()> {
        match self.edge_defs.iter().position(|&x| x == ed) {
            Some(i) => {
                self.edge_defs.remove(i);
                Ok(())
            }
            None => Err(SolidError::sm_err(OP)),
        }
    }

    /// Clears the ends equal to `pv` (`RemoveProtoVertex`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when neither end is `pv`.
    pub fn remove_proto_vertex(&mut self, pv: Option<ProtoVertexId>) -> Result<()> {
        let mut deleted = false;
        if pv == self.start_pv {
            self.start_pv = None;
            deleted = true;
        }
        if pv == self.end_pv {
            self.end_pv = None;
            deleted = true;
        }
        if deleted {
            Ok(())
        } else {
            Err(SolidError::sm_err(OP))
        }
    }
}

// ---- SmProtoFace ------------------------------------------------------------------------

/// A coincident face pair of the two B-reps and the proto edges bounding it
/// (`SmProtoFace`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProtoFace<O> {
    /// `m_pBrep1Face`.
    brep1_face: Option<O>,
    /// `m_pBrep2Face`.
    brep2_face: Option<O>,
    /// `m_sCoincPEs`.
    coinc_pes: Vec<ProtoEdgeId>,
}

impl<O: ClassObject> ProtoFace<O> {
    /// The face of B-rep `which` (1, else 2) (`GetSrcTopo`).
    #[must_use]
    pub const fn src_topo(&self, which: i32) -> Option<O> {
        if which == 1 {
            self.brep1_face
        } else {
            self.brep2_face
        }
    }

    /// Always [`PointClassType::Face`] (`GetSrcTopo_TYPE`).
    #[must_use]
    pub const fn src_topo_type(&self, _which: i32) -> PointClassType {
        PointClassType::Face
    }

    /// The bounding proto edges (`GetCoincProtoEdges`).
    #[must_use]
    pub fn coinc_proto_edges(&self) -> &[ProtoEdgeId] {
        &self.coinc_pes
    }

    /// The bounding proto edges, for the edits of the list upstream makes through the
    /// pointer `GetCoincProtoEdges` returns.
    pub fn coinc_proto_edges_mut(&mut self) -> &mut Vec<ProtoEdgeId> {
        &mut self.coinc_pes
    }

    /// Sets the face of B-rep `which` (1, else 2): the object when it is a face
    /// (`SetSrcTopo`, `SM_CAST_PTR(SmFace, pTopo)`).
    pub fn set_src_topo(&mut self, which: i32, _t: PointClassType, topo: Option<O>) {
        let face = topo.filter(|t| t.class_kind() == ClassObjectKind::Face);
        if which == 1 {
            self.brep1_face = face;
        } else {
            self.brep2_face = face;
        }
    }
}

// ---- SmProtoTopologyManager -------------------------------------------------------------

/// The proto topology of one Boolean (`SmProtoTopologyManager`): the lists of proto
/// vertices, edges and faces, and the store every proto object and definition of
/// the operation lives in (upstream allocates them with `new (pOwner)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProtoTopologyManager<O> {
    /// `m_pBrep1`.
    brep1: O,
    /// `m_pBrep2`.
    brep2: O,
    /// `m_pTI`.
    ti: O,
    /// `m_dTol`.
    tol: f64,
    /// `m_dCaptureDistance`.
    capture_distance: f64,
    /// `m_sPVs`.
    pvs: Vec<ProtoVertexId>,
    /// `m_sPEs`.
    pes: Vec<ProtoEdgeId>,
    /// `m_sPFs`.
    pfs: Vec<ProtoFaceId>,
    /// Every vertex definition made (`None` once deleted).
    vertex_def_store: Vec<Option<VertexDefinition<O>>>,
    /// Every edge definition made.
    edge_def_store: Vec<Option<EdgeDefinition<O>>>,
    /// Every proto vertex made.
    pv_store: Vec<Option<ProtoVertex>>,
    /// Every proto edge made.
    pe_store: Vec<Option<ProtoEdge>>,
    /// Every proto face made.
    pf_store: Vec<Option<ProtoFace<O>>>,
    /// Not upstream: `SmProtoEdge::Resolve` splits a self-loop proto edge even when
    /// its vertex definition is on a vertex (`true`, the default and upstream's
    /// behaviour); `false` drops such a proto edge instead (source issue #480,
    /// see `topology::proto_topology::p2`).
    split_self_loops_on_vertices: bool,
}

fn push_store<T>(store: &mut Vec<Option<T>>, item: T) -> Result<u32> {
    let index = u32::try_from(store.len()).map_err(|_| dangling("store is full"))?;
    store.push(Some(item));
    Ok(index)
}

impl<O: ClassObject> ProtoTopologyManager<O> {
    /// `SmProtoTopologyManager(pBrep1, pBrep2, pTI, dCaptureDistance,
    /// dOverrideTol)`. Each B-rep is passed as its handle and its [`Brep`]. The
    /// tolerance is the topology intersector's approximation tolerance
    /// (`pTI->GetThisApproxTol3d()`, passed as `ti_approx_tol_3d`; the override is
    /// overwritten by it, as upstream); a tolerance below `SM_EFF_ZERO` becomes
    /// `min(1.25 * (tol1 + tol2), 0.5 * min(edge1, edge2))` from the B-reps'
    /// tolerances and smallest edge boxes ([`brep_p2::min_edge_bbox_size`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the edge box computation's unported callees;
    /// [`SolidError::NotFound`] for a dangling edge list.
    pub fn new(
        k: &Kernel,
        brep1: (O, &Brep),
        brep2: (O, &Brep),
        ti: O,
        ti_approx_tol_3d: f64,
        capture_distance: f64,
        override_tol: f64,
    ) -> Result<Self> {
        let _ = override_tol;
        let (brep1, b1) = brep1;
        let (brep2, b2) = brep2;
        // Get tol from pTI, which is from the SmMerge constructor.
        let mut tol = ti_approx_tol_3d;
        if tol < EFF_ZERO {
            let brep_tol = b1.tolerance() + b2.tolerance();
            // the edges found are unused, as upstream
            let (edge1_size, _) = brep_p2::min_edge_bbox_size(k, b1)?;
            let (edge2_size, _) = brep_p2::min_edge_bbox_size(k, b2)?;
            let edge_size = min2(edge1_size, edge2_size);
            tol = min2(brep_tol * 1.25, edge_size * 0.5);
        }
        Ok(Self {
            brep1,
            brep2,
            ti,
            tol,
            capture_distance,
            pvs: Vec::new(),
            pes: Vec::new(),
            pfs: Vec::new(),
            vertex_def_store: Vec::new(),
            edge_def_store: Vec::new(),
            pv_store: Vec::new(),
            pe_store: Vec::new(),
            pf_store: Vec::new(),
            split_self_loops_on_vertices: true,
        })
    }

    /// Whether resolving a proto edge from a proto vertex back to itself splits it
    /// even when its vertex definition is on a vertex in either B-rep (`true`, the
    /// default, as upstream). Not upstream.
    #[must_use]
    pub const fn split_self_loops_on_vertices(&self) -> bool {
        self.split_self_loops_on_vertices
    }

    /// Sets [`ProtoTopologyManager::split_self_loops_on_vertices`]; the Boolean's
    /// topology intersector sets it to `false` when
    /// `Kernel::fixes.proto_edge_self_loop_on_vertex_degenerate` is on
    /// (source issue #480, WR-214). Not upstream.
    pub fn set_split_self_loops_on_vertices(&mut self, split: bool) {
        self.split_self_loops_on_vertices = split;
    }

    /// The destructor `~SmProtoTopologyManager`: first cuts the links between the
    /// listed proto edges and proto vertices (each listed edge's ends are cleared and
    /// each listed vertex's edge list emptied, so no destructor below unlists across
    /// them), then deletes the listed proto faces, then the listed proto edges (with their edge definitions, which delete the
    /// curves they own), then the listed proto vertices (with their vertex
    /// definitions). Returns the curve handles deleted. References to objects
    /// already deleted (a use after free upstream) are skipped. Objects that are not
    /// listed stay in the store (upstream allocates them from the context, not from
    /// the manager); the manager must not be used for anything else afterwards.
    #[must_use]
    pub fn destroy(&mut self) -> Vec<O> {
        // Cut the ProtoVertex <-> ProtoEdge links first: a ProtoVertex can still list a
        // ProtoEdge that has moved to another ProtoVertex, and its destructor would then
        // write into a deleted ProtoEdge.
        for &pe in &self.pes {
            if let Some(Some(e)) = self.pe_store.get_mut(pe.0 as usize) {
                e.set_start_proto_vertex(None);
                e.set_end_proto_vertex(None);
            }
        }
        for &pv in &self.pvs {
            if let Some(Some(v)) = self.pv_store.get_mut(pv.0 as usize) {
                v.proto_edges_mut().clear();
            }
        }

        let mut deleted = Vec::new();
        for pf in std::mem::take(&mut self.pfs) {
            if let Some(slot) = self.pf_store.get_mut(pf.0 as usize) {
                *slot = None;
            }
        }
        for pe in std::mem::take(&mut self.pes) {
            deleted.extend(self.delete_proto_edge(pe));
        }
        for pv in std::mem::take(&mut self.pvs) {
            self.delete_proto_vertex(pv);
        }
        deleted
    }

    /// `delete pPE` (`~SmProtoEdge`): deletes its edge definitions, unlists it from
    /// its end proto vertices (all occurrences, including closed edges); the curves
    /// deleted.
    fn delete_proto_edge(&mut self, pe: ProtoEdgeId) -> Vec<O> {
        let mut deleted = Vec::new();
        let Some(edge) = self.pe_store.get_mut(pe.0 as usize).and_then(Option::take) else {
            return deleted;
        };
        for ed in &edge.edge_defs {
            if let Some(d) = self
                .edge_def_store
                .get_mut(ed.0 as usize)
                .and_then(Option::take)
            {
                deleted.extend(d.destroy());
            }
        }
        for endpoint in [edge.start_pv, edge.end_pv].into_iter().flatten() {
            if let Some(Some(vertex)) = self.pv_store.get_mut(endpoint.0 as usize) {
                vertex.proto_edges.retain(|&incident| incident != pe);
            }
        }
        deleted
    }

    /// `delete pPV` (`~SmProtoVertex`): deletes its vertex definitions and clears it
    /// from the ends of the proto edges it lists.
    fn delete_proto_vertex(&mut self, pv: ProtoVertexId) {
        let Some(vertex) = self.pv_store.get_mut(pv.0 as usize).and_then(Option::take) else {
            return;
        };
        for vd in &vertex.vertex_defs {
            if let Some(slot) = self.vertex_def_store.get_mut(vd.0 as usize) {
                *slot = None;
            }
        }
        for pe in &vertex.proto_edges {
            if let Some(Some(e)) = self.pe_store.get_mut(pe.0 as usize) {
                let _ = e.remove_proto_vertex(Some(pv));
            }
        }
    }

    /// Empties the three lists without deleting anything (`Reset`).
    pub fn reset(&mut self) {
        self.pvs.clear();
        self.pes.clear();
        self.pfs.clear();
    }

    // ---- simple access ---------------------------------------------------------------

    /// B-rep `which` (1, else 2) (`GetWhichBrep`).
    #[must_use]
    pub const fn which_brep(&self, which: i32) -> O {
        if which == 1 {
            self.brep1
        } else {
            self.brep2
        }
    }

    /// The topology intersector (`m_pTI`).
    #[must_use]
    pub const fn topology_intersector(&self) -> O {
        self.ti
    }

    /// The tolerance (`GetTolerance`, as an `SmZoneTol3d`).
    #[must_use]
    pub fn tolerance(&self, k: &Kernel) -> f64 {
        zone_tol3d(k, self.tol)
    }

    /// The raw tolerance `m_dTol` (read directly by the manager's own members).
    #[must_use]
    pub const fn raw_tolerance(&self) -> f64 {
        self.tol
    }

    /// Sets the tolerance (`SetTolerance`; the flags are unused).
    pub fn set_tolerance(&mut self, zone_tol_3d: f64) {
        self.tol = zone_tol_3d;
    }

    /// The capture distance (`GetCaptureDistance`).
    #[must_use]
    pub const fn capture_distance(&self) -> f64 {
        self.capture_distance
    }

    /// Sets the capture distance (`SetCaptureDistance`).
    pub fn set_capture_distance(&mut self, capture_distance: f64) {
        self.capture_distance = capture_distance;
    }

    /// The listed proto vertices (`GetProtoVertices`).
    #[must_use]
    pub fn proto_vertices(&self) -> &[ProtoVertexId] {
        &self.pvs
    }

    /// The listed proto edges (`GetProtoEdges`).
    #[must_use]
    pub fn proto_edges(&self) -> &[ProtoEdgeId] {
        &self.pes
    }

    /// The listed proto faces (`GetProtoFaces`).
    #[must_use]
    pub fn proto_faces(&self) -> &[ProtoFaceId] {
        &self.pfs
    }

    /// Lists `pv` (`AddProtoVertex`).
    pub fn add_proto_vertex(&mut self, pv: ProtoVertexId) {
        self.pvs.push(pv);
    }

    /// Lists `pe` (`AddProtoEdge`).
    pub fn add_proto_edge(&mut self, pe: ProtoEdgeId) {
        self.pes.push(pe);
    }

    /// Lists `pf` (`AddProtoFace`).
    pub fn add_proto_face(&mut self, pf: ProtoFaceId) {
        self.pfs.push(pf);
    }

    // ---- the store -------------------------------------------------------------------

    /// The vertex definition `vd`, if it exists.
    #[must_use]
    pub fn vertex_def(&self, vd: VertexDefId) -> Option<&VertexDefinition<O>> {
        self.vertex_def_store
            .get(vd.0 as usize)
            .and_then(Option::as_ref)
    }

    /// Mutable access to the vertex definition `vd`.
    pub fn vertex_def_mut(&mut self, vd: VertexDefId) -> Option<&mut VertexDefinition<O>> {
        self.vertex_def_store
            .get_mut(vd.0 as usize)
            .and_then(Option::as_mut)
    }

    /// The edge definition `ed`, if it exists.
    #[must_use]
    pub fn edge_def(&self, ed: EdgeDefId) -> Option<&EdgeDefinition<O>> {
        self.edge_def_store
            .get(ed.0 as usize)
            .and_then(Option::as_ref)
    }

    /// Mutable access to the edge definition `ed`.
    pub fn edge_def_mut(&mut self, ed: EdgeDefId) -> Option<&mut EdgeDefinition<O>> {
        self.edge_def_store
            .get_mut(ed.0 as usize)
            .and_then(Option::as_mut)
    }

    /// The proto vertex `pv`, if it exists.
    #[must_use]
    pub fn proto_vertex(&self, pv: ProtoVertexId) -> Option<&ProtoVertex> {
        self.pv_store.get(pv.0 as usize).and_then(Option::as_ref)
    }

    /// Mutable access to the proto vertex `pv`.
    pub fn proto_vertex_mut(&mut self, pv: ProtoVertexId) -> Option<&mut ProtoVertex> {
        self.pv_store
            .get_mut(pv.0 as usize)
            .and_then(Option::as_mut)
    }

    /// The proto edge `pe`, if it exists.
    #[must_use]
    pub fn proto_edge(&self, pe: ProtoEdgeId) -> Option<&ProtoEdge> {
        self.pe_store.get(pe.0 as usize).and_then(Option::as_ref)
    }

    /// Mutable access to the proto edge `pe`.
    pub fn proto_edge_mut(&mut self, pe: ProtoEdgeId) -> Option<&mut ProtoEdge> {
        self.pe_store
            .get_mut(pe.0 as usize)
            .and_then(Option::as_mut)
    }

    /// The proto face `pf`, if it exists.
    #[must_use]
    pub fn proto_face(&self, pf: ProtoFaceId) -> Option<&ProtoFace<O>> {
        self.pf_store.get(pf.0 as usize).and_then(Option::as_ref)
    }

    /// Mutable access to the proto face `pf`.
    pub fn proto_face_mut(&mut self, pf: ProtoFaceId) -> Option<&mut ProtoFace<O>> {
        self.pf_store
            .get_mut(pf.0 as usize)
            .and_then(Option::as_mut)
    }

    fn vd(&self, vd: VertexDefId) -> Result<&VertexDefinition<O>> {
        self.vertex_def(vd)
            .ok_or(dangling("vertex definition is deleted"))
    }

    fn ed(&self, ed: EdgeDefId) -> Result<&EdgeDefinition<O>> {
        self.edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))
    }

    fn pv(&self, pv: Option<ProtoVertexId>) -> Result<&ProtoVertex> {
        pv.and_then(|pv| self.proto_vertex(pv))
            .ok_or(dangling("proto vertex is null or deleted"))
    }

    fn pv_mut(&mut self, pv: ProtoVertexId) -> Result<&mut ProtoVertex> {
        self.proto_vertex_mut(pv)
            .ok_or(dangling("proto vertex is deleted"))
    }

    fn pe(&self, pe: Option<ProtoEdgeId>) -> Result<&ProtoEdge> {
        pe.and_then(|pe| self.proto_edge(pe))
            .ok_or(dangling("proto edge is null or deleted"))
    }

    fn pe_mut(&mut self, pe: ProtoEdgeId) -> Result<&mut ProtoEdge> {
        self.proto_edge_mut(pe)
            .ok_or(dangling("proto edge is deleted"))
    }

    /// Stores a vertex definition (`new (pOwner) SmVertexDefinition(..)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn insert_vertex_definition(&mut self, vd: VertexDefinition<O>) -> Result<VertexDefId> {
        push_store(&mut self.vertex_def_store, vd).map(VertexDefId)
    }

    /// Stores an edge definition (`new (pOwner) SmEdgeDefinition(..)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn insert_edge_definition(&mut self, ed: EdgeDefinition<O>) -> Result<EdgeDefId> {
        push_store(&mut self.edge_def_store, ed).map(EdgeDefId)
    }

    /// Stores a copy of the edge definition `ed` (the copy constructor
    /// `SmEdgeDefinition(const SmEdgeDefinition&)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted definition or a full store.
    pub fn copy_edge_definition(&mut self, ed: EdgeDefId) -> Result<EdgeDefId> {
        let copy = self.ed(ed)?.copy_definition();
        self.insert_edge_definition(copy)
    }

    /// Deletes the vertex definition `vd` (`delete pVD`; `~SmVertexDefinition` does
    /// nothing else).
    pub fn delete_vertex_definition(&mut self, vd: VertexDefId) {
        if let Some(slot) = self.vertex_def_store.get_mut(vd.0 as usize) {
            *slot = None;
        }
    }

    /// Deletes the edge definition `ed` (`delete pED`): the curves deleted.
    pub fn delete_edge_definition(&mut self, ed: EdgeDefId) -> Vec<O> {
        self.edge_def_store
            .get_mut(ed.0 as usize)
            .and_then(Option::take)
            .map(EdgeDefinition::destroy)
            .unwrap_or_default()
    }

    /// Deletes the proto edge `pe` (`delete pPE`, `~SmProtoEdge`): the curves deleted.
    pub fn delete_proto_edge_object(&mut self, pe: ProtoEdgeId) -> Vec<O> {
        self.delete_proto_edge(pe)
    }

    /// Drops every reference the listed proto faces (coincident proto edges) and
    /// proto edges (overlapping proto edges) hold to `pe`, before it is deleted.
    ///
    /// source fix source fork 93efd6d (UB-239), which does this in
    /// `RemoveProtoEdge` because every library caller deletes the edge next. Upstream
    /// keeps the references, and later passes over those lists
    /// (`GlueCoincidentBrepVertices`, `Resolve`) read the edge after the free. The port
    /// calls it at those library deletes, so `remove_proto_edge` and
    /// `delete_proto_edge_object` stay upstream's on their own.
    pub fn forget_proto_edge(&mut self, pe: ProtoEdgeId) {
        for pf in self.pfs.clone() {
            if let Some(face) = self.proto_face_mut(pf) {
                face.coinc_proto_edges_mut().retain(|&x| x != pe);
            }
        }
        for other in self.pes.clone() {
            if let Some(edge) = self.proto_edge_mut(other) {
                edge.overlapping_pes.retain(|&x| x != pe);
            }
        }
    }

    /// Deletes the proto vertex `pv` (`delete pPV`, `~SmProtoVertex`).
    pub fn delete_proto_vertex_object(&mut self, pv: ProtoVertexId) {
        self.delete_proto_vertex(pv);
    }

    // ---- constructors of the proto objects -------------------------------------------

    /// A proto vertex without vertex definitions (`SmProtoVertex(crPos, dTol,
    /// pOwner)`), not listed yet.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn new_proto_vertex(&mut self, position: Point3, tol: f64) -> Result<ProtoVertexId> {
        push_store(
            &mut self.pv_store,
            ProtoVertex {
                position,
                tol,
                vertex_defs: Vec::new(),
                proto_edges: Vec::new(),
                absorbed_proto_edge: false,
            },
        )
        .map(ProtoVertexId)
    }

    /// A proto vertex with one new vertex definition of `pc1`, `pc2` at `position`
    /// (gap computed) (`SmProtoVertex(crPC1, crPC2, rPosition, dTol, pOwner)`), not
    /// listed yet.
    ///
    /// # Errors
    ///
    /// Those of [`VertexDefinition::new`].
    pub fn new_proto_vertex_with_pcs(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pc1: &PointClassification<O>,
        pc2: &PointClassification<O>,
        position: Point3,
        tol: f64,
    ) -> Result<ProtoVertexId> {
        let id = ProtoVertexId(
            u32::try_from(self.pv_store.len()).map_err(|_| dangling("store is full"))?,
        );
        let vd = VertexDefinition::new(k, geometry, pc1, pc2, position, tol, -1.0, Some(id))?;
        let vd_id = self.insert_vertex_definition(vd)?;
        push_store(
            &mut self.pv_store,
            ProtoVertex {
                position,
                tol,
                vertex_defs: vec![vd_id],
                proto_edges: Vec::new(),
                absorbed_proto_edge: false,
            },
        )
        .map(ProtoVertexId)
    }

    /// A proto edge from `start` to `end` over `interval` with the manager's
    /// tolerance (`SmProtoEdge(pStartPV, pEndPV, crIvl, pOwner)`), not listed yet.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn new_proto_edge(
        &mut self,
        k: &Kernel,
        start: Option<ProtoVertexId>,
        end: Option<ProtoVertexId>,
        interval: Extent1d,
    ) -> Result<ProtoEdgeId> {
        let tol = self.tolerance(k);
        push_store(
            &mut self.pe_store,
            ProtoEdge {
                xsect_interval: interval,
                start_pv: start,
                end_pv: end,
                edge_defs: Vec::new(),
                tol,
                overlapping_pes: Vec::new(),
                coinc_pfs: Vec::new(),
            },
        )
        .map(ProtoEdgeId)
    }

    /// A copy of the proto edge `pe` without definitions, overlaps or proto faces
    /// (the copy constructor `SmProtoEdge(const SmProtoEdge&)`), not listed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted proto edge or a full store.
    pub fn copy_proto_edge(&mut self, pe: ProtoEdgeId) -> Result<ProtoEdgeId> {
        let src = self.pe(Some(pe))?;
        let copy = ProtoEdge {
            xsect_interval: src.xsect_interval,
            start_pv: src.start_pv,
            end_pv: src.end_pv,
            edge_defs: Vec::new(),
            tol: src.tol,
            overlapping_pes: Vec::new(),
            coinc_pfs: Vec::new(),
        };
        push_store(&mut self.pe_store, copy).map(ProtoEdgeId)
    }

    /// A proto face of the coincident faces `face1`, `face2` bounded by `coinc_pes`
    /// (`SmProtoFace(pBrep1Face, pBrep2Face, crCoincPEs, pOwner)`), not listed.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn new_proto_face(
        &mut self,
        face1: Option<O>,
        face2: Option<O>,
        coinc_pes: &[ProtoEdgeId],
    ) -> Result<ProtoFaceId> {
        push_store(
            &mut self.pf_store,
            ProtoFace {
                brep1_face: face1,
                brep2_face: face2,
                coinc_pes: coinc_pes.to_vec(),
            },
        )
        .map(ProtoFaceId)
    }

    // ---- SmVertexDefinition members that reach the manager --------------------------

    /// The number of proto edges at the vertex definition's proto vertex
    /// (`GetNumProtoEdges`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a proto vertex.
    pub fn vertex_def_num_proto_edges(&self, vd: VertexDefId) -> Result<usize> {
        Ok(self.pv(self.vd(vd)?.proto_vertex)?.num_proto_edges())
    }

    /// Resolves the vertex definition `vd` (`Resolve`): when its two object points
    /// are farther apart than the manager's tolerance and either classification has
    /// a gap, the position moves between them, proportionally closer to the object
    /// with the smaller gap but within both objects' zone tolerances. The status
    /// output is always 0.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a proto vertex; [`SolidError::Unported`]
    /// from the classifications (topology objects).
    pub fn resolve_vertex_definition(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        vd: VertexDefId,
    ) -> Result<i32> {
        let v = self.vd(vd)?;
        let dist1 = v.pc1.gap_3d(k, geometry, None)?;
        let dist2 = v.pc2.gap_3d(k, geometry, None)?;

        let pt1 = obj_point(k, &v.pc1, geometry)?;
        let pt2 = obj_point(k, &v.pc2, geometry)?;

        let dist = pt1.distance(pt2);

        // the manager's tolerance (through the proto vertex back pointer)
        self.pv(v.proto_vertex)?;
        let tol = self.tolerance(k);

        if dist > tol && dist1 + dist2 > EFF_ZERO {
            // Put it proportionally closer to the topo object with the smaller gap,
            // but not out of Tolerance
            let vec = pt2 - pt1;
            let vec_length = vec.length();
            let tol1 = object_zone_tol_3d(k, geometry, v.pc1.object())?;
            let tol2 = object_zone_tol_3d(k, geometry, v.pc2.object())?;
            let frac_min = 1. - tol2 / vec_length;
            let frac_max = tol1 / vec_length;
            let mut frac_ivl = Extent1d::from_min_max(0.0, 1.0);
            let disjoint = frac_max < frac_min;
            if !disjoint {
                ignore_status(frac_ivl.set_min_max(frac_min, frac_max))?;
            }

            let frac = dist1 / (dist1 + dist2);
            let frac = frac_ivl.clamp_value(frac);
            let position = pt1 + frac * vec;
            if let Some(v) = self.vertex_def_mut(vd) {
                v.position = position;
            }
        }
        Ok(0)
    }

    // ---- SmEdgeDefinition members that reach the manager ----------------------------

    /// The start proto vertex of the definition's proto edge
    /// (`SmEdgeDefinition::GetStartProtoVertex`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a proto edge.
    pub fn edge_def_start_proto_vertex(&self, ed: EdgeDefId) -> Result<Option<ProtoVertexId>> {
        Ok(self.pe(self.ed(ed)?.proto_edge)?.start_pv)
    }

    /// The end proto vertex of the definition's proto edge
    /// (`SmEdgeDefinition::GetEndProtoVertex`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a proto edge.
    pub fn edge_def_end_proto_vertex(&self, ed: EdgeDefId) -> Result<Option<ProtoVertexId>> {
        Ok(self.pe(self.ed(ed)?.proto_edge)?.end_pv)
    }

    /// The 3d curve's point at the end of the definition bounded by `which_pv`
    /// (`SmEdgeDefinition::GetEndPosition(SmProtoVertex*, SmPoint3d&)`; the
    /// evaluation status is ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `which_pv` bounds neither end of the proto edge;
    /// [`SolidError::InvalidInput`] without a proto edge or curve.
    pub fn edge_def_end_position_at(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        ed: EdgeDefId,
        which_pv: Option<ProtoVertexId>,
    ) -> Result<Point3> {
        let d = self.ed(ed)?;
        let pe = self.pe(d.proto_edge)?;
        let t = if which_pv == pe.start_pv {
            d.xsect_interval.min()
        } else if which_pv == pe.end_pv {
            d.xsect_interval.max()
        } else {
            return Err(SolidError::sm_err(OP_END));
        };
        eval_point(k, curve_of(geometry, d.xsect_curve_3d)?, t)
    }

    /// Whether the intersection interval `tgt_curve` over `tgt_domain` between
    /// `tgt_start`, `tgt_end` has the shape of the definition `ed`
    /// (`IsSameEdgeDefinition`), and whether it also comes from the same source
    /// objects (the midpoint classifications' objects).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a proto edge or curve;
    /// [`SolidError::Unported`] from the curve comparison.
    pub fn is_same_edge_definition(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        ed: EdgeDefId,
        tgt_start: Option<ProtoVertexId>,
        tgt_end: Option<ProtoVertexId>,
        mid_pc1: &PointClassification<O>,
        mid_pc2: &PointClassification<O>,
        tgt_curve: Option<O>,
        tgt_domain: &Extent1d,
    ) -> Result<(bool, bool)> {
        let d = self.ed(ed)?;
        let pe = self.pe(d.proto_edge)?;
        let (start, end) = (pe.start_pv, pe.end_pv);

        // First, it would have to have the same two ProtoVertices.
        let forward =
            (start == tgt_start && end == tgt_end) || (end == tgt_start && start == tgt_end);
        if !forward {
            return Ok((false, false));
        }

        // The 3d curves have to be roughly the same (use the stored larger
        // CaptureDistance).
        let tol = self.capture_distance;
        if !are_same_curve(
            k,
            geometry,
            d.xsect_curve_3d,
            &d.xsect_interval,
            tgt_curve,
            tgt_domain,
            tol,
        )? {
            return Ok((false, false));
        }

        // when ED->SrcTopos are the same as the TgtInterval->MidPt->Objects
        let identical = mid_pc1.object() == d.src_topo1 && mid_pc2.object() == d.src_topo2;
        Ok((true, identical))
    }

    // ---- SmProtoVertex members that reach the manager -------------------------------

    /// Lists the vertex definition `vd` at `pv` and makes `pv` its proto vertex
    /// (`AddVertexDefinition(SmVertexDefinition*)`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn add_vertex_definition(&mut self, pv: ProtoVertexId, vd: VertexDefId) -> Result<()> {
        self.pv_mut(pv)?.vertex_defs.push(vd);
        self.vertex_def_mut(vd)
            .ok_or(dangling("vertex definition is deleted"))?
            .proto_vertex = Some(pv);
        Ok(())
    }

    /// The first vertex definition at `pv` with source objects `topo1`, `topo2`
    /// (`FindMatchingVertexDefinition`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn find_matching_vertex_definition(
        &self,
        pv: ProtoVertexId,
        topo1: Option<O>,
        topo2: Option<O>,
    ) -> Result<Option<VertexDefId>> {
        for &vd in &self.pv(Some(pv))?.vertex_defs {
            let v = self.vd(vd)?;
            if v.src_topo(1) == topo1 && v.src_topo(2) == topo2 {
                return Ok(Some(vd));
            }
        }
        Ok(None)
    }

    /// Not upstream (WR-410, source issue #1059; the source fork's
    /// `SmProtoVertex::AddCrossing`): an intersection curve crosses an edge at `pv`
    /// (`pc1`, `pc2`, the crossing's classifications, with their parameters set, at angle
    /// `asin(crossing_sine)`). The vertex definition of the crossing's source objects
    /// takes its position from the crossing that fixes it best
    /// ([`VertexDefinition::place_by_crossing`]); placed again, it moves `pv` (the
    /// average of its definitions) with it and widens `pv`'s tolerance to its own.
    /// Nothing for an unknown angle or no matching definition.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects; those of
    /// [`VertexDefinition::place_by_crossing`].
    pub fn add_crossing(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pv: ProtoVertexId,
        pc1: &PointClassification<O>,
        pc2: &PointClassification<O>,
        crossing_sine: f64,
    ) -> Result<()> {
        // (upstream's `!(sine >= 0)`: a NaN sine is unknown)
        if crossing_sine.is_nan() || crossing_sine < 0.0 {
            return Ok(());
        }
        let topo1 = as_topology(pc1.object());
        let topo2 = as_topology(pc2.object());
        let Some(vd) = self.find_matching_vertex_definition(pv, topo1, topo2)? else {
            return Ok(());
        };

        // Placed again by a crossing that fixes it more closely, it moves this
        // ProtoVertex, the average of its VertexDefinitions, with it.
        let old = self.vd(vd)?.position;
        let moved = self
            .vertex_def_mut(vd)
            .ok_or(dangling("vertex definition is deleted"))?
            .place_by_crossing(k, geometry, pc1, pc2, crossing_sine)?;
        if moved {
            let (new, vd_tol) = {
                let v = self.vd(vd)?;
                (v.position, v.tolerance(k))
            };
            let count = self.pv(Some(pv))?.vertex_defs.len() as f64;
            let p = self.pv_mut(pv)?;
            p.position += (new - old) / count;
            p.tol = max2(p.tol, vd_tol);
        }
        Ok(())
    }

    /// The vertex definition at `pv` closest to `position`
    /// (`GetClosestVertexDefinition`; the first of equally close ones).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn closest_vertex_definition(
        &self,
        pv: ProtoVertexId,
        position: Point3,
    ) -> Result<Option<VertexDefId>> {
        let mut ret = None;
        let mut min_dist = BIG_DOUBLE;
        for &vd in &self.pv(Some(pv))?.vertex_defs {
            let dist = position.distance_squared(self.vd(vd)?.position);
            if dist < min_dist {
                ret = Some(vd);
                min_dist = dist;
            }
        }
        Ok(ret)
    }

    /// The B-rep `which` vertex at `pv` connected to the source of the edge
    /// definition `ed` and closest to its start (or end) (`GetFinalVertex`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or a missing curve; those of
    /// the `geometry` topology walks.
    pub fn proto_vertex_final_vertex(
        &self,
        k: &Kernel,
        geometry: &(impl ClassGeometry<O> + ProtoTopology<O>),
        pv: ProtoVertexId,
        which: i32,
        ed: EdgeDefId,
        at_start: bool,
    ) -> Result<Option<O>> {
        let vds = &self.pv(Some(pv))?.vertex_defs;
        if vds.is_empty() {
            return Ok(None);
        }

        let mut ret_val = None;
        let mut max_dist = BIG_DOUBLE;

        let d = self.ed(ed)?;
        let brep_topo_ed = d.src_topo(which);
        let ed_pos = d.end_position(k, geometry, at_start)?;

        for &vd in vds {
            let Some(vtx) = self.vd(vd)?.final_vertex(which) else {
                continue;
            };
            if are_connected(geometry, brep_topo_ed, Some(vtx))? {
                let dist = ed_pos.distance(geometry.vertex_point(vtx)?);
                if dist < max_dist {
                    max_dist = dist;
                    ret_val = Some(vtx);
                }
            }
        }
        Ok(ret_val)
    }

    /// Merges `other` into `pv` (`CombineProtoVertex`): the larger tolerance, all of
    /// `other`'s vertex definitions (re-owned, still listed at `other`), the position
    /// averaged over the definitions (`other` contributing its own position once per
    /// definition), and `other`'s proto edges re-pointed and listed at `pv`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn combine_proto_vertex(&mut self, pv: ProtoVertexId, other: ProtoVertexId) -> Result<()> {
        // If they're the same, do nothing.
        if other == pv {
            return Ok(());
        }
        let o = self.pv(Some(other))?.clone();
        let this = self.pv_mut(pv)?;
        this.tol = max2(this.tol, o.tol);
        this.absorbed_proto_edge = this.absorbed_proto_edge || o.absorbed_proto_edge;

        // the average of all VDs, starting with what is here already
        let mut ave_pos = (this.vertex_defs.len() as f64) * this.position;

        for &vd in &o.vertex_defs {
            self.add_vertex_definition(pv, vd)?;
            ave_pos += o.position;
        }
        let this = self.pv_mut(pv)?;
        let num_vds = this.vertex_defs.len();
        this.position = ave_pos / num_vds as f64;

        // Replace pOtherPV with 'this' in all of pOtherPV's ProtoEdges.
        for &pe in &o.proto_edges {
            self.pe_mut(pe)?.replace_proto_vertex(Some(other), Some(pv));
        }

        self.pv_mut(pv)?
            .proto_edges
            .extend_from_slice(&o.proto_edges);
        Ok(())
    }

    // ---- SmProtoEdge members that reach the manager ---------------------------------

    /// The unique group of proto edges overlapping `pe`, directly or through other
    /// members (`GetOverlappingPEGroup`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn overlapping_pe_group(&self, pe: ProtoEdgeId) -> Result<Vec<ProtoEdgeId>> {
        // start with this PE's overlapping PEs
        let mut group = self.pe(Some(pe))?.overlapping_pes.clone();
        let mut this_iter_cnt = 0usize;

        // chase Overlapping PE associations until the whole group is found
        while this_iter_cnt < group.len() {
            let last_iter_cnt = this_iter_cnt;
            this_iter_cnt = group.len();

            for ii in last_iter_cnt..this_iter_cnt {
                let member = *group.get(ii).ok_or(dangling("group index"))?;
                for &o in &self.pe(Some(member))?.overlapping_pes {
                    if !group.contains(&o) {
                        group.push(o);
                    }
                }
            }
        }
        Ok(group)
    }

    /// The position of the end of `pe` bounded by `which_pv`
    /// (`SmProtoEdge::GetEndPosition(SmProtoVertex*, SmPoint3d&)`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `which_pv` bounds neither end; those of
    /// [`ProtoTopologyManager::proto_edge_end_position`].
    pub fn proto_edge_end_position_at(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        which_pv: Option<ProtoVertexId>,
    ) -> Result<Point3> {
        let e = self.pe(Some(pe))?;
        if which_pv == e.start_pv {
            self.proto_edge_end_position(k, geometry, pe, true)
        } else if which_pv == e.end_pv {
            self.proto_edge_end_position(k, geometry, pe, false)
        } else {
            Err(SolidError::sm_err(OP_END))
        }
    }

    /// The start (or end) position of `pe` (`SmProtoEdge::GetEndPosition(SmBoolean,
    /// SmPoint3d&)`): the average of its definitions' end points, or its proto
    /// vertex's position without definitions.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a missing proto vertex, definition or curve.
    pub fn proto_edge_end_position(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        at_start: bool,
    ) -> Result<Point3> {
        let e = self.pe(Some(pe))?;
        if e.edge_defs.is_empty() {
            let pv = if at_start { e.start_pv } else { e.end_pv };
            return Ok(self.pv(pv)?.position);
        }

        // average position of all EdgeDefs
        let mut ave_pt = Vec3::new(0.0, 0.0, 0.0);
        for &ed in &e.edge_defs {
            ave_pt += self.ed(ed)?.end_position(k, geometry, at_start)?;
        }
        Ok(ave_pt / e.edge_defs.len() as f64)
    }

    /// The average point and/or tangent of the definitions of `pe` at `param`
    /// (`SmProtoEdge::Evaluate`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` without definitions; those of
    /// [`EdgeDefinition::evaluate`].
    pub fn proto_edge_evaluate(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        param: f64,
        want_pos: bool,
        want_tan: bool,
    ) -> Result<(Option<Point3>, Option<Vec3>)> {
        let e = self.pe(Some(pe))?;
        let num_eds = e.edge_defs.len();
        if num_eds == 0 {
            return Err(status(OP_EVALUATE, SM_ERR_INVALID_INPUT));
        }

        let mut sum_pts = Vec3::new(0.0, 0.0, 0.0);
        let mut sum_tans = Vec3::new(0.0, 0.0, 0.0);
        for &ed in &e.edge_defs {
            let (p, t) = self
                .ed(ed)?
                .evaluate(k, geometry, param, want_pos, want_tan)?;
            if let Some(p) = p {
                sum_pts += p;
            }
            if let Some(t) = t {
                sum_tans += t;
            }
        }

        Ok((
            want_pos.then(|| sum_pts / num_eds as f64),
            want_tan.then(|| sum_tans / num_eds as f64),
        ))
    }

    /// [`ProtoTopologyManager::proto_edge_evaluate`] at the middle of the interval
    /// (`SmProtoEdge::EvaluateMid`).
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::proto_edge_evaluate`].
    pub fn proto_edge_evaluate_mid(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        want_pos: bool,
        want_tan: bool,
    ) -> Result<(Option<Point3>, Option<Vec3>)> {
        let param = self.pe(Some(pe))?.xsect_interval.mid();
        self.proto_edge_evaluate(k, geometry, pe, param, want_pos, want_tan)
    }

    /// Whether the intersection interval has the shape of a definition of `pe`
    /// (`IsSameProtoEdge`); the second value (always `false` when the first is not
    /// `true` with an identical definition) tells whether that definition also comes
    /// from the same source objects.
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::is_same_edge_definition`].
    pub fn is_same_proto_edge(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        tgt_start: Option<ProtoVertexId>,
        tgt_end: Option<ProtoVertexId>,
        mid_pc1: &PointClassification<O>,
        mid_pc2: &PointClassification<O>,
        tgt_curve: Option<O>,
        tgt_domain: &Extent1d,
    ) -> Result<(bool, bool)> {
        let e = self.pe(Some(pe))?;
        let mut found_same_shape_only = false;

        // does this PE run between the Tgt PVs
        let forward = (tgt_start == e.start_pv && tgt_end == e.end_pv)
            || (tgt_start == e.end_pv && tgt_end == e.start_pv);
        if !forward {
            return Ok((false, false));
        }

        for &ed in &e.edge_defs {
            let (same, identical) = self.is_same_edge_definition(
                k, geometry, ed, tgt_start, tgt_end, mid_pc1, mid_pc2, tgt_curve, tgt_domain,
            )?;
            if same {
                if identical {
                    return Ok((true, true));
                }
                found_same_shape_only = true;
            }
        }
        Ok((found_same_shape_only, false))
    }

    /// Relates `pe` and the coincident proto face `pf` both ways, each without
    /// duplicates (`SmProtoEdge::RelateCoincidentPF`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects.
    pub fn relate_coincident_pf(&mut self, pe: ProtoEdgeId, pf: ProtoFaceId) -> Result<()> {
        let e = self.pe_mut(pe)?;
        if !e.coinc_pfs.contains(&pf) {
            e.coinc_pfs.push(pf);
        }
        let f = self
            .proto_face_mut(pf)
            .ok_or(dangling("proto face is deleted"))?;
        if !f.coinc_pes.contains(&pe) {
            f.coinc_pes.push(pe);
        }
        Ok(())
    }

    // ---- SmProtoTopologyManager -----------------------------------------------------

    /// The vertex definitions of all listed proto vertices, in list order
    /// (`GetAllVertexDefinitions`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted listed proto vertex.
    pub fn all_vertex_definitions(&self) -> Result<Vec<VertexDefId>> {
        let mut out = Vec::new();
        for &pv in &self.pvs {
            out.extend_from_slice(&self.pv(Some(pv))?.vertex_defs);
        }
        Ok(out)
    }

    /// The edge definitions of all listed proto edges, in list order
    /// (`GetAllEdgeDefinitions`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted listed proto edge.
    pub fn all_edge_definitions(&self) -> Result<Vec<EdgeDefId>> {
        let mut out = Vec::new();
        for &pe in &self.pes {
            out.extend_from_slice(&self.pe(Some(pe))?.edge_defs);
        }
        Ok(out)
    }

    /// The closest listed proto vertex containing `position`, and its distance
    /// ([`BIG_DOUBLE`] for none) (`FindProtoVertex`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted listed proto vertex.
    pub fn find_proto_vertex(&self, position: Point3) -> Result<(Option<ProtoVertexId>, f64)> {
        // Init outputs.
        let mut ret_val = None;
        let mut min_dist = BIG_DOUBLE;

        // for every ProtoVertex - find the closest one to rPosition that's within
        // max(m_dTol, GetCaptureDistance())
        for &id in &self.pvs {
            let pv = self.pv(Some(id))?;
            if face_p2::proto_vertex_contains_point(self, pv, position) {
                let this_dist = face_p2::proto_vertex_distance_to(pv, position);
                if this_dist < min_dist {
                    ret_val = Some(id);
                    min_dist = this_dist;
                }
            }
        }
        Ok((ret_val, min_dist))
    }

    /// The first edge definition (of the listed proto edges) whose B-rep `which`
    /// source or final edge is `target_edge` (`FindEdgeDefinitionFromEdge`).
    ///
    /// # Errors
    ///
    /// Those of [`ProtoTopologyManager::all_edge_definitions`].
    pub fn find_edge_definition_from_edge(
        &self,
        target_edge: Option<O>,
        which: i32,
    ) -> Result<Option<EdgeDefId>> {
        for ed in self.all_edge_definitions()? {
            let d = self.ed(ed)?;
            if d.src_topo(which) == target_edge {
                return Ok(Some(ed));
            }
            if d.final_edge(which) == target_edge {
                return Ok(Some(ed));
            }
        }
        Ok(None)
    }

    /// Unlists the vertex definition `vd` from its proto vertex (not deleted), and
    /// with `remove_owner_if_empty` unlists and deletes that proto vertex when it has
    /// no definitions left (`RemoveVertexDefinition`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or no proto vertex.
    pub fn remove_vertex_definition(
        &mut self,
        vd: VertexDefId,
        remove_owner_if_empty: bool,
    ) -> Result<()> {
        let owner = self
            .vd(vd)?
            .proto_vertex
            .ok_or(dangling("vertex definition has no proto vertex"))?;

        // status ignored upstream
        let _ = self.pv_mut(owner)?.remove_vertex_definition(vd);

        if remove_owner_if_empty && self.pv(Some(owner))?.vertex_defs.is_empty() {
            let _ = self.remove_proto_vertex(owner);
            self.delete_proto_vertex(owner);
        }
        Ok(())
    }

    /// Unlists the first entry `pv` (`RemoveProtoVertex`; nothing is deleted).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `pv` is not listed.
    pub fn remove_proto_vertex(&mut self, pv: ProtoVertexId) -> Result<()> {
        match self.pvs.iter().position(|&x| x == pv) {
            Some(i) => {
                self.pvs.remove(i);
                Ok(())
            }
            None => Err(SolidError::sm_err(OP)),
        }
    }

    /// Unlists the first entry `pe` and unlists it at its end proto vertices
    /// (`RemoveProtoEdge`; nothing is deleted).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `pe` is not listed; [`SolidError::InvalidInput`] for deleted
    /// objects.
    pub fn remove_proto_edge(&mut self, pe: ProtoEdgeId) -> Result<()> {
        match self.pes.iter().position(|&x| x == pe) {
            Some(i) => {
                self.pes.remove(i);
            }
            None => return Err(SolidError::sm_err(OP)),
        }

        // remove End PV->PE back pointers (statuses ignored)
        let e = self.pe(Some(pe))?;
        let (start, end) = (e.start_pv, e.end_pv);
        if let Some(s) = start {
            let _ = self.pv_mut(s)?.remove_proto_edge(pe);
        }
        if let Some(e) = end {
            let _ = self.pv_mut(e)?.remove_proto_edge(pe);
        }
        Ok(())
    }

    /// Adds `survivor` to the coincident proto edges of every listed proto face that
    /// has `removed` there and not `survivor` yet: the faces bounded by a proto edge
    /// combined away are bounded by the one it was combined into (source change,
    /// source fork 93efd6d, in `SplitIntersectingProtoEdges`).
    pub fn hand_over_coincident_proto_edge(&mut self, survivor: ProtoEdgeId, removed: ProtoEdgeId) {
        for pf in self.pfs.clone() {
            if let Some(face) = self.proto_face_mut(pf) {
                let list = face.coinc_proto_edges_mut();
                if list.contains(&removed) && !list.contains(&survivor) {
                    list.push(survivor);
                }
            }
        }
    }

    /// The proto edges with a definition from the same source objects as the
    /// interval pair's midpoints that overlaps `interval1` over a segment
    /// (`CheckOverlappingProtoEdges`; upstream appends them to `rOverlappingPEs`).
    /// Only intervals on a B-rep edge are checked. `classification1` is the
    /// classification `interval1` belongs to.
    ///
    /// # Errors
    ///
    /// Those of [`are_overlapping`]; [`SolidError::InvalidInput`] for deleted
    /// objects.
    pub fn check_overlapping_proto_edges(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        classification1: &CurveClassification<O>,
        interval1: &CurveInterval<O>,
        interval2: &CurveInterval<O>,
    ) -> Result<Vec<Option<ProtoEdgeId>>> {
        let mut out = Vec::new();
        let mid1 = &interval1.mid;
        let mid2 = &interval2.mid;
        if mid1.edge_object().is_none() && mid2.edge_object().is_none() {
            return Ok(out);
        }

        let obj1 = as_topology(mid1.object());
        let obj2 = as_topology(mid2.object());
        for ed in self.all_edge_definitions()? {
            let d = self.ed(ed)?;
            if d.src_topo(1) != obj1 || d.src_topo(2) != obj2 {
                continue;
            }
            if are_overlapping(k, geometry, classification1, interval1, d)? {
                out.push(d.proto_edge);
            }
        }
        Ok(out)
    }

    /// Makes and lists a proto face (`CreateProtoFace`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the store is full.
    pub fn create_proto_face(
        &mut self,
        face1: Option<O>,
        face2: Option<O>,
        coinc_pes: &[ProtoEdgeId],
    ) -> Result<ProtoFaceId> {
        let pf = self.new_proto_face(face1, face2, coinc_pes)?;
        self.add_proto_face(pf);
        Ok(pf)
    }

    /// Whether two definitions of one proto edge can be combined, and which to
    /// delete (`CanCombineEdgeDefinitions`; 0 neither, 1 the first, 2 the second):
    /// the second when both come from the same source objects; otherwise, for
    /// connected source pairs with the same curve, the one on higher-dimensional
    /// topology.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for a definition without a source object; those of the
    /// `geometry` topology walks and of [`find_ed_on_degen_face`] but its statuses.
    pub fn can_combine_edge_definitions(
        &self,
        k: &Kernel,
        geometry: &(impl ClassGeometry<O> + ProtoTopology<O>),
        ed1: EdgeDefId,
        ed2: EdgeDefId,
    ) -> Result<u64> {
        let d1 = self.ed(ed1)?;
        let d2 = self.ed(ed2)?;
        let null = || SolidError::null_pointer(OP_COMBINE);
        let t11 = d1.src_topo1.ok_or_else(null)?;
        let t12 = d1.src_topo2.ok_or_else(null)?;
        let t21 = d2.src_topo1.ok_or_else(null)?;
        let t22 = d2.src_topo2.ok_or_else(null)?;

        // If they're sitting on the same topology in both Breps, combine.
        if t11 == t21 && t12 == t22 {
            return Ok(2);
        }

        // If either pair is not connected in their Brep, can't combine.
        if !are_connected(geometry, Some(t11), Some(t21))?
            || !are_connected(geometry, Some(t12), Some(t22))?
        {
            return Ok(0);
        }

        // Check distance: if they're too far apart, we can't combine them. (Upstream
        // compares the second curve over the first definition's interval.)
        let domain1 = d1.xsect_interval;
        let tol = (d1.tolerance(k) + d2.tolerance(k)) * 5.0;
        if !are_same_curve(
            k,
            geometry,
            d1.xsect_curve_3d,
            &domain1,
            d2.xsect_curve_3d,
            &domain1,
            tol,
        )? {
            return Ok(0);
        }

        // Check dimensionality - [vertex:0, edge:1, face:2]
        let dim11 = topo_dimension(t11);
        let dim12 = topo_dimension(t12);
        let dim21 = topo_dimension(t21);
        let dim22 = topo_dimension(t22);

        let mut can_delete = 0;
        if dim11 < dim21 && dim12 <= dim22 {
            can_delete = 2;
        }
        if dim21 < dim11 && dim22 <= dim12 {
            can_delete = 1;
        }
        if dim12 < dim22 && dim11 <= dim21 {
            can_delete = 2;
        }
        if dim22 < dim12 && dim21 <= dim11 {
            can_delete = 1;
        }

        // check for coincident edges (e.g. unstitched edges or degenerate faces)
        let is_edge = |t: O| t.class_kind() == ClassObjectKind::Edge;
        if is_edge(t11) && is_edge(t12) && is_edge(t21) && is_edge(t22) {
            if t11 != t21 && t12 == t22 {
                let n = degen_face_choice(find_ed_on_degen_face(geometry, t11, t21))?;
                if n != 0 {
                    return Ok(n);
                }
            } else if t11 == t21 && t12 != t22 {
                let n = degen_face_choice(find_ed_on_degen_face(geometry, t12, t22))?;
                if n != 0 {
                    return Ok(n);
                }
            }
        }
        Ok(can_delete)
    }

    /// Removes the proto vertex `pv` when it has no proto edges and lies on the curve
    /// of `pe` (the first definition's curve over the proto edge's interval, within
    /// twice the proto edge's tolerance) (`RemoveCoincidentNoEdgeProtoVertex`; the
    /// proto vertex is unlisted, not deleted). As upstream, `true` is also returned,
    /// with nothing removed, when the point solve fails or finds nothing (upstream
    /// returns `SM_SUCCESS` from its `SmBoolean` function there), unless
    /// `Kernel::fixes.proto_vertex_miss_is_false` is set (WR-73, then `false`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for deleted objects or a missing curve;
    /// [`SolidError::Unported`] from the point solve.
    pub fn remove_coincident_no_edge_proto_vertex(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        pe: ProtoEdgeId,
        pv: ProtoVertexId,
    ) -> Result<bool> {
        let e = self.pe(Some(pe))?;

        // no work - ProtoEdge is already terminated by ProtoVertex.
        if e.start_pv == Some(pv) || e.end_pv == Some(pv) {
            return Ok(false);
        }

        // no work - ProtoEdge does not have any EdgeDefinitions
        let Some(&first_ed) = e.edge_defs.first() else {
            return Ok(false);
        };

        let p = self.pv(Some(pv))?;
        let vertex_pos = p.position;
        let edge_zone_tol = e.tolerance(k);
        let xsect_tol = xsect_tol3d(k, edge_zone_tol + edge_zone_tol);

        let crv_handle = self.ed(first_ed)?.xsect_curve_3d;
        let crv = curve_of(geometry, crv_handle)?;
        let crv_ivl = e.xsect_interval;

        // XSect ProtoEdgeCurve/ProtoVertexPosition
        let opts = GlobalPointSolveOptions {
            handle: Some(CurveHandle(crv.natural_interval())),
            ..GlobalPointSolveOptions::default()
        };
        let sols = ignored(crv.global_point_solve(
            k,
            &crv_ivl,
            SolverOperation::Intersect,
            vertex_pos,
            xsect_tol,
            SolutionRequested::All,
            &opts,
        ))?;
        // upstream returns SM_SUCCESS here, which reads as TRUE (the proto vertex is
        // not removed): plan/upstream-issues.md WR-73; FALSE with the opt-in
        // `Kernel::fixes.proto_vertex_miss_is_false` (source issue #848)
        let missed = !k.fixes.proto_vertex_miss_is_false;
        let Some(sols) = sols else {
            return Ok(missed);
        };
        if sols.is_empty() {
            return Ok(missed);
        }

        // When the ProtoVertex is Edgeless and XSects a ProtoEdge - remove it
        if p.proto_edges.is_empty() {
            let _ = self.remove_proto_vertex(pv);
            return Ok(true);
        }
        Ok(false)
    }

    /// The proto vertex to combine at a proto edge split (`sm_WhichPVToCombine`):
    /// `split_pv` when the edge was split, else the end proto vertex containing the
    /// edge's point at `split_param`, if any.
    ///
    /// # Errors
    ///
    /// Without `split_pv`: [`SolidError::InvalidInput`] for a deleted proto edge or a
    /// proto edge without start or end proto vertex (upstream dereferences them), and
    /// those of [`ProtoTopologyManager::proto_edge_evaluate`] upstream does not ignore
    /// ([`SolidError::Unported`], [`SolidError::InvalidInput`]).
    pub fn which_pv_to_combine(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        orig_pe: ProtoEdgeId,
        split_param: f64,
        split_pv: Option<ProtoVertexId>,
    ) -> Result<Option<ProtoVertexId>> {
        if split_pv.is_some() {
            return Ok(split_pv);
        }

        // locals
        let e = self.pe(Some(orig_pe))?;
        let start_pv = e.start_pv;
        let end_pv = e.end_pv;

        // Eval PE at SplitParam (status ignored: the point stays `SmPoint3d()`)
        let split_pt =
            match self.proto_edge_evaluate(k, geometry, orig_pe, split_param, true, false) {
                Ok((Some(p), _)) => p,
                Err(
                    e @ (SolidError::Unported { .. }
                    | SolidError::InvalidInput { .. }
                    | SolidError::PortInvariant { .. }),
                ) => return Err(e),
                _ => Vec3::UNDEF,
            };

        // return matching End PV or NULL for none
        if face_p2::proto_vertex_contains_point(self, self.pv(start_pv)?, split_pt) {
            Ok(start_pv)
        } else if face_p2::proto_vertex_contains_point(self, self.pv(end_pv)?, split_pt) {
            Ok(end_pv)
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    fn manager(k: &Kernel) -> ProtoTopologyManager<H> {
        let b = H(100, ClassObjectKind::Other);
        let e = Brep::new();
        ProtoTopologyManager::new(k, (b, &e), (b, &e), b, 1e-5, 1e-3, -1.0).unwrap()
    }

    #[test]
    fn deleting_a_closed_proto_edge_unlinks_both_incident_entries() {
        let k = Kernel::default();
        let mut m = manager(&k);
        let vertex = m.new_proto_vertex(Point3::ZERO, 1e-5).unwrap();
        let interval = Extent1d::from_raw(0.0, 1.0);
        let gone = m
            .new_proto_edge(&k, Some(vertex), Some(vertex), interval)
            .unwrap();
        let kept = m.new_proto_edge(&k, Some(vertex), None, interval).unwrap();
        let incident = m.proto_vertex_mut(vertex).unwrap();
        incident.add_proto_edge(gone);
        incident.add_proto_edge(kept);
        incident.add_proto_edge(gone);

        assert!(m.delete_proto_edge_object(gone).is_empty());
        assert!(m.proto_edge(gone).is_none());
        assert_eq!(m.proto_vertex(vertex).unwrap().proto_edges(), &[kept]);
        assert!(m.proto_edge(kept).is_some());
        // Repeated cleanup cannot disturb the remaining live edge.
        assert!(m.delete_proto_edge_object(gone).is_empty());
        assert_eq!(m.proto_vertex(vertex).unwrap().proto_edges(), &[kept]);
        m.delete_proto_vertex_object(vertex);
        assert_eq!(m.proto_edge(kept).unwrap().start_proto_vertex(), None);
    }

    #[test]
    fn deleting_a_proto_edge_after_vertex_combination_unlinks_every_entry() {
        let k = Kernel::default();
        let mut m = manager(&k);
        let start = m.new_proto_vertex(Point3::ZERO, 1e-5).unwrap();
        let end = m.new_proto_vertex(Point3::X, 1e-5).unwrap();
        let gone = m
            .new_proto_edge(&k, Some(start), Some(end), Extent1d::from_raw(0.0, 1.0))
            .unwrap();
        m.proto_vertex_mut(start).unwrap().add_proto_edge(gone);
        m.proto_vertex_mut(end).unwrap().add_proto_edge(gone);
        m.combine_proto_vertex(start, end).unwrap();
        assert_eq!(m.proto_vertex(start).unwrap().proto_edges(), &[gone, gone]);

        m.delete_proto_edge_object(gone);
        assert!(m.proto_vertex(start).unwrap().proto_edges().is_empty());
    }

    #[test]
    fn a_forgotten_proto_edge_leaves_every_face_and_overlap_list() {
        // 93efd6d (#369): SplitIntersectingProtoEdges deleted a combined-away proto
        // edge that proto faces and other proto edges still listed
        let k = Kernel::default();
        let mut m = manager(&k);
        let ivl = Extent1d::from_raw(0.0, 1.0);
        let [kept, gone, other] = [(); 3].map(|()| m.new_proto_edge(&k, None, None, ivl).unwrap());
        for pe in [kept, gone, other] {
            m.add_proto_edge(pe);
        }
        let f1 = m.new_proto_face(None, None, &[gone, other, gone]).unwrap();
        let f2 = m.new_proto_face(None, None, &[kept, gone]).unwrap();
        m.add_proto_face(f1);
        m.add_proto_face(f2);
        m.proto_edge_mut(other).unwrap().add_overlapping_pe(gone);
        m.proto_edge_mut(kept).unwrap().add_overlapping_pe(gone);
        m.proto_edge_mut(kept).unwrap().add_overlapping_pe(other);
        let coinc = |m: &ProtoTopologyManager<H>, pf| {
            m.proto_face(pf).unwrap().coinc_proto_edges().to_vec()
        };

        // the faces gone bounded are bounded by kept, once
        m.hand_over_coincident_proto_edge(kept, gone);
        assert_eq!(coinc(&m, f1), vec![gone, other, gone, kept]);
        assert_eq!(coinc(&m, f2), vec![kept, gone]);
        // unlisting alone keeps upstream's lists
        m.remove_proto_edge(gone).unwrap();
        assert_eq!(coinc(&m, f2), vec![kept, gone]);
        assert_eq!(m.proto_edges(), &[kept, other]);
        // the library's delete drops every reference first
        m.forget_proto_edge(gone);
        let _ = m.delete_proto_edge_object(gone);
        assert_eq!(coinc(&m, f1), vec![other, kept]);
        assert_eq!(coinc(&m, f2), vec![kept]);
        assert!(m.proto_edge(other).unwrap().overlapping_pes().is_empty());
        assert_eq!(m.proto_edge(kept).unwrap().overlapping_pes(), &[other]);
    }
}
