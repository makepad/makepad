//! Proto topology, part 3: finding or creating proto vertices and edges from point
//! and curve classifications, splitting proto edges and their definitions, gluing
//! coincident B-rep vertices, and replacing overlapping proto-edge groups.
//!
//! Upstream: `source/SMLib/src/SmProtoTopology.cpp`, `source/SMLib/inc/SmProtoTopology.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`ProtoTopologyManager::find_or_create_proto_vertex_pcs`] | `SmProtoTopologyManager::FindOrCreateProtoVertex(PC, PC, ..)` |
//! | [`ProtoTopologyManager::find_or_create_proto_edge`] | `SmProtoTopologyManager::FindOrCreateProtoEdge` |
//! | [`ProtoTopologyManager::edge_definition_split`] | `SmEdgeDefinition::Split` |
//! | [`ProtoTopologyManager::proto_edge_split`] | `SmProtoEdge::Split` |
//! | [`ProtoTopologyManager::split_intersecting_proto_edges`] | `SmProtoTopologyManager::SplitIntersectingProtoEdges` |
//! | [`ProtoTopologyManager::glue_coincident_brep_vertices`] | `SmProtoTopologyManager::GlueCoincidentBrepVertices` |
//! | [`ProtoTopologyManager::resolve_overlapping_proto_edges`] | `SmProtoTopologyManager::ResolveOverlappingProtoEdges` |
//!
//! Deviations:
//! - **Vertices placed by their best edge crossing** (not upstream, WR-410,
//!   source issue #1059, opt-in `Kernel::fixes.boolean_vertex_by_best_crossing`;
//!   the source fork's `FindOrCreateProtoEdge` change): after each end's proto vertex,
//!   [`ProtoTopologyManager::add_crossing`] with the angle at which the face pair's curve
//!   crosses the edge its end lies on (`crossing_sine`).
//! - **Removed proto edges** (source fix, source fork 93efd6d, UB-239).
//!   Upstream's `SplitIntersectingProtoEdges` deletes the combined-away proto edge while
//!   other proto edges (overlap lists) still name it (`CombineProtoEdge` hands its
//!   proto faces to the survivor), and later passes read it after the free. Here every
//!   reference to it is dropped before the delete ([`ProtoTopologyManager::forget_proto_edge`],
//!   part 1), here and in `ResolveOverlappingProtoEdges`. The fork does that inside
//!   `RemoveProtoEdge`; the port does it at these library deletes, so unlisting or
//!   deleting a proto edge on its own (the difftest's script ops) stays upstream's.
//!   Without it, the stale id names a deleted slot and the later passes fail with
//!   `InvalidInput`.
//! - **UB-241 in `GlueCoincidentBrepVertices`** (source fix, source fork
//!   da8d05e): use-after-free in `GlueCoincidentBrepVertices` on the `GlueVertices`
//!   path; the port stops before that call. The delete branch is defined and runs as
//!   upstream.
//! - **B-rep access.** Topology objects are the caller's handles `O`, as in
//!   [`p1`](super::p1). What these functions read from vertices and edges, and the
//!   store new curves go to, is [`ProtoBrep`] (of `topology.proto_topology_p2`, which
//!   gained `vertex_common_edges` and `face_curve_on_classify` (`SmFace::CurveOnClassify(TRUE, cc)`, implemented by the
//!   B-rep owner with `query::face::p2::curve_on_classify`) for these functions; `vertex_point`, `edge_faces` and `edge_other_vertex` are
//!   those of its supertrait [`ProtoTopology`](super::p1::proto::ProtoTopology)), which the owner of the B-reps implements. `SmTol::GetZoneTol3d` of a vertex is
//!   [`ProtoBrep::topology_tolerance`]; `SmTol::GetXSectTol3d(v1, v2)` is the
//!   sum of the two.
//! - **B-rep edits**: `SmBrep::SqueezeEdge` of `GetWhichBrep(which)` is
//!   [`ProtoBrep::brep_squeeze_edge`] (the owner of the B-reps calls
//!   `topology::brep::p3::squeeze_edge`), so
//!   [`ProtoTopologyManager::glue_coincident_brep_vertices`] takes `&mut Kernel` and
//!   the B-reps mutably.
//! - **Callees of other units** go through traits the B-reps' owner implements:
//!   `SmBrep::DeleteEdge` (`topology.brep_p4`) through [`BrepGlue::delete_edge`],
//!   `SmBrep::Point3DClassify` goes through [`ProtoBrep::brep_point_3d_classify`],
//!   which the B-reps' owner implements. `SmBrep::GlueVertices`
//!   goes through [`BrepGlue`], which the B-reps' owner implements with
//!   `topology::brep::p5` (so [`ProtoTopologyManager::glue_coincident_brep_vertices`]
//!   takes the kernel and the B-reps mutably).
//!   The intersector's `GetOtherMate` /
//!   `RemoveRelationship` are reached through [`Mates`] (ported in
//!   `boolean.topology_intersector_p1` / `_p2`; [`NoMates`] for no intersector maps). The
//!   `SmCurveClassification` constructor is part 4's ([`CurveClassification::with_curve`]). `ExtractGeometry` calls part 5 and
//!   currently reaches part 6's `Homogenize` dependency. The code
//!   after a stub is ported in full. The callees of `topology.proto_topology_p2` are
//!   its methods on [`ProtoTopologyManager`].
//! - **Deleted curves.** Functions that delete curves (through `Destruct` or the
//!   destructors) return the handles deleted, as in [`p1`](super::p1).
//! - **Undefined behaviour upstream** (dereferencing a null or deleted object, indexing
//!   an empty face array) is [`SolidError::InvalidInput`].
//! - **Out-params** become return values: the created flags and new objects of the
//!   find-or-create and split functions.
//! - `FindOrCreateProtoEdge` reads the topology intersector's `GetThisApproxTol3d`,
//!   passed as a value (`ti_approx_tol_3d`).
//! - A failing `eRet` of `GlueCoincidentBrepVertices` is returned as the error (the
//!   curves deleted before it are not reported).
//! - When `CreateProtoEdge` fails at `NER(pBrep1Topo)`, upstream returns the already
//!   allocated `rpNewPE` through the out-parameter; the port returns `None` there (the
//!   failing paths are not reachable from the callers ported so far).
//! - The `SM_DEBUG_CODE` blocks are not ported.
//! - **Recursion.** None.

use crate::curve::curve::p2::Curve;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::solution_array::{ObjectKind, SolutionObject, SolutionType};
use crate::geom::tol::{xsect_tol3d, zone_tol3d, USE_DEFAULT};
use crate::intersect::curve_class::p5::ExtractOptions;
use crate::intersect::curve_class::{ClassificationInit, CurveClassification, CurveInterval};
use crate::intersect::point_class::{
    ClassGeometry, ClassObject, ClassObjectKind, PointClassification,
};
use crate::kernel::Kernel;
use crate::math::{Vec3, BIG_DOUBLE, EFF_ZERO};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;

use super::p1::proto::{can_imprint, find_position_from_pcs, topo_dimension};
use super::p1::{
    EdgeDefId, ProtoEdgeId, ProtoFaceId, ProtoTopologyManager, ProtoVertexId, VertexDefId,
};
use super::p2::ProtoBrep;

const OP: &str = "topology::proto_topology::p3";
const OP_ED_SPLIT: &str = "topology::proto_topology::p3::edge_definition_split";
const OP_SPLIT_INT: &str = "topology::proto_topology::p3::split_intersecting_proto_edges";

/// The handle the curve intersections name (only the solution values are used).
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

/// A dereference of a null pointer or a deleted object upstream (undefined behaviour).
const fn dangling(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// The outcome of a status upstream ignores: the value, `None` for a failure other than
/// the errors that cannot be ignored here (unported callees, undefined behaviour).
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

/// `SM_CAST_PTR(<kind>, obj)` for a topology kind.
fn as_kind<O: ClassObject>(o: Option<O>, kind: ClassObjectKind) -> Option<O> {
    o.filter(|o| o.class_kind() == kind)
}

/// `SM_CAST_PTR(SmTopology, obj)`.
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

// ---- callees not ported yet ----------------------------------------------------------

/// `SmCurveClassification(pCurve, interval, NULL, tol)` (the remaining arguments default:
/// no interval storage, the curve not owned and current, no surfaces).
fn new_curve_classification<O: ClassObject>(
    k: &Kernel,
    curve: O,
    interval: Extent1d,
    tol: f64,
) -> Result<CurveClassification<O>> {
    CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(curve),
            interval,
            uv_curve: None,
            src_zone_tol_3d: zone_tol3d(k, tol),
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

/// The B-rep edit [`ProtoTopologyManager::glue_coincident_brep_vertices`] (and the
/// topology intersector) make on the B-reps behind the handles, implemented by their
/// owner with the mutable arenas (`topology::brep::p5`).
pub trait BrepGlue<O> {
    /// `SmBrep::GlueVertices(pV1, pV2, pOptWireEdgesInBrep)` of `brep`;
    /// `opt_wire_edges_in_brep` is `pOptWireEdgesInBrep` (`NULL` by default: every
    /// shell edge is checked).
    ///
    /// # Errors
    ///
    /// Those of `topology::brep::p5::glue_vertices`; [`SolidError::InvalidInput`] for
    /// handles that name no vertex (or edge) of `brep`.
    fn glue_vertices(
        &mut self,
        k: &mut Kernel,
        brep: O,
        v1: O,
        v2: O,
        opt_wire_edges_in_brep: Option<&[O]>,
    ) -> Result<()>;
    /// `SmBrep::DeleteEdge(pEdge)` of `brep` (`pOptSurfaceToKeep = NULL`;
    /// `topology::brep::p4::delete_edge`).
    ///
    /// # Errors
    ///
    /// Those of `topology::brep::p4::delete_edge`; [`SolidError::InvalidInput`] for a
    /// handle that names no edge of `brep`.
    fn delete_edge(&mut self, k: &mut Kernel, brep: O, edge: O) -> Result<()>;
}

/// What the manager's topology intersector (`m_pTI`) provides to
/// [`ProtoTopologyManager::glue_coincident_brep_vertices`]: `GetOtherMate` and
/// `RemoveRelationship` (implemented by
/// `boolean::topology_intersector::p2::MapMates` over the intersector's maps).
pub trait Mates<O> {
    /// `SmTopologyIntersector::GetOtherMate(pBrepEntity)`.
    fn other_mate(&self, brep_entity: O) -> Option<O>;
    /// `SmTopologyIntersector::RemoveRelationship(pBrepEntity, pOtherEntity)`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the two are not a mated pair.
    fn remove_relationship(&mut self, brep_entity: O, other_entity: O) -> Result<()>;
}

/// An intersector without relationships: no object has a mate.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoMates;

impl<O> Mates<O> for NoMates {
    fn other_mate(&self, _brep_entity: O) -> Option<O> {
        None
    }

    fn remove_relationship(&mut self, _brep_entity: O, _other_entity: O) -> Result<()> {
        Err(SolidError::sm_err(
            "topology::proto_topology::p3::remove_relationship",
        ))
    }
}

// ---- the manager's members ------------------------------------------------------------

impl<O: ClassObject> ProtoTopologyManager<O> {
    /// `SmProtoTopologyManager::FindOrCreateProtoVertex(rPC1, rPC2, rpNewPV,
    /// pbWasCreated)`: the proto vertex for a point two classifications agree on
    /// (found within the tolerance, else created) and whether it was created. `None`
    /// when either classification cannot be imprinted.
    ///
    /// # Errors
    ///
    /// Those of `FindProtoVertex` and `CreateProtoVertex`; [`SolidError::Unported`]
    /// for the callees listed in the module header.
    pub fn find_or_create_proto_vertex_pcs(
        &mut self,
        k: &Kernel,
        geometry: &impl ProtoBrep<O>,
        pc1: &mut PointClassification<O>,
        pc2: &mut PointClassification<O>,
    ) -> Result<(Option<ProtoVertexId>, bool)> {
        // Skip PtXSects that don't classify to Region, Face, Edge, or Vertex in both Breps
        if !can_imprint(pc1.point_class()) {
            return Ok((None, false));
        }
        if !can_imprint(pc2.point_class()) {
            return Ok((None, false));
        }

        // sm_FindPositionFromPCs, status ignored; the position stays SM_UNDEF on failure
        let position = ignore_soft(find_position_from_pcs(k, geometry, pc1, pc2))?
            .map_or(Vec3::UNDEF, |p| p.position);

        // find closest ProtoVertex within m_dTol to sPt
        let (found, _dist) = self.find_proto_vertex(position)?;

        // when nearby PV exists
        if let Some(pv) = found {
            // AddVertexDefinition(rPC1, rPC2, pNewVD): status ignored
            ignore_soft(self.add_vertex_definition_from_pcs(k, geometry, pv, pc1, pc2))?;
            return Ok((Some(pv), false));
        }

        // a new ProtoVertex has to be created
        let created = self.create_proto_vertex(k, geometry, pc1, pc2)?;
        Ok((created, created.is_some()))
    }

    /// `SmProtoTopologyManager::FindOrCreateProtoEdge(rCI1, rCI2, rpNewPE,
    /// pbWasCreated)`: the proto edge of a face pair's curve interval (found or
    /// created) and whether it was created. `classification1` is the classification
    /// `ci1` belongs to (its curve is the one the proto edge is made of), `classification2`
    /// that of `ci2`. The status of
    /// the non-degenerate path is always `SM_SUCCESS`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a missing curve; those of `IsDegenerate` and
    /// `FindOrCreateProtoVertex`; [`SolidError::Unported`] for the callees listed in
    /// the module header.
    pub fn find_or_create_proto_edge(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        ti_approx_tol_3d: f64,
        classification1: &CurveClassification<O>,
        ci1: &mut CurveInterval<O>,
        classification2: &CurveClassification<O>,
        ci2: &mut CurveInterval<O>,
    ) -> Result<(Option<ProtoEdgeId>, bool)> {
        let cc_curve = classification1.curve();
        let this_ivl = ci1.interval;
        let tol = ti_approx_tol_3d;

        // DD3: Check for overlapping PEs; the status is ignored.
        let overlapping: Vec<Option<ProtoEdgeId>> = ignore_soft(
            self.check_overlapping_proto_edges(k, geometry, classification1, ci1, ci2),
        )?
        .unwrap_or_default();

        // when CrvInterval is degenerate - find or create a ProtoVertex for Interval
        if curve_of(geometry, cc_curve)?.is_degenerate(k, tol, None)? {
            let start =
                can_imprint(ci1.start.point_class()) && can_imprint(ci2.start.point_class());
            let mid = can_imprint(ci1.mid.point_class()) && can_imprint(ci2.mid.point_class());
            let end = can_imprint(ci1.end.point_class()) && can_imprint(ci2.end.point_class());
            if start {
                self.find_or_create_proto_vertex_pcs(k, geometry, &mut ci1.start, &mut ci2.start)?;
            } else if mid {
                self.find_or_create_proto_vertex_pcs(k, geometry, &mut ci1.mid, &mut ci2.mid)?;
            } else if end {
                self.find_or_create_proto_vertex_pcs(k, geometry, &mut ci1.end, &mut ci2.end)?;
            }
            return Ok((None, false));
        }

        // NonDegen CrvIntervals: find or create PVs for the ends (statuses ignored)
        let mut start_pv = None;
        let mut end_pv = None;
        if can_imprint(ci1.start.point_class()) && can_imprint(ci2.start.point_class()) {
            start_pv = ignore_soft(self.find_or_create_proto_vertex_pcs(
                k,
                geometry,
                &mut ci1.start,
                &mut ci2.start,
            ))?
            .and_then(|r| r.0);
            // not upstream (WR-410, #1059, opt-in): placed by its best edge crossing
            if let (Some(pv), true) = (start_pv, k.fixes.boolean_vertex_by_best_crossing) {
                let sine = crossing_sine(
                    k,
                    &*geometry,
                    classification1,
                    ci1,
                    classification2,
                    ci2,
                    false,
                )?;
                self.add_crossing(k, &*geometry, pv, &ci1.start, &ci2.start, sine)?;
            }
        }
        if can_imprint(ci1.end.point_class()) && can_imprint(ci2.end.point_class()) {
            end_pv = ignore_soft(self.find_or_create_proto_vertex_pcs(
                k,
                geometry,
                &mut ci1.end,
                &mut ci2.end,
            ))?
            .and_then(|r| r.0);
            // not upstream (WR-410, #1059, opt-in): placed by its best edge crossing
            if let (Some(pv), true) = (end_pv, k.fixes.boolean_vertex_by_best_crossing) {
                let sine = crossing_sine(
                    k,
                    &*geometry,
                    classification1,
                    ci1,
                    classification2,
                    ci2,
                    true,
                )?;
                self.add_crossing(k, &*geometry, pv, &ci1.end, &ci2.end, sine)?;
            }
        }

        // Can't create a ProtoEdge without both PVs.
        let (Some(start_pv), Some(end_pv)) = (start_pv, end_pv) else {
            return Ok((None, false));
        };

        // quit - IntervalMid does not classify to topology in both SrcBreps
        if !can_imprint(ci1.mid.point_class()) || !can_imprint(ci2.mid.point_class()) {
            return Ok((None, false));
        }

        // quit - SrcFaces intersect at a single Vertex
        if ci1.mid.point_class() == crate::intersect::point_class::PointClassType::Vertex
            || ci2.mid.point_class() == crate::intersect::point_class::PointClassType::Vertex
        {
            return Ok((None, false));
        }

        // quit - degenerate interval. [B488]
        if curve_of(geometry, cc_curve)?.is_degenerate(k, tol, Some(&this_ivl))? {
            return Ok((None, false));
        }

        // See whether a ProtoEdge already exists in this location.
        let (found, is_identical) = self.find_proto_edge(
            k,
            &*geometry,
            Some(start_pv),
            Some(end_pv),
            &ci1.mid,
            &ci2.mid,
            cc_curve,
            &this_ivl,
        )?;
        let mut was_created = false;
        let mut new_pe = found;
        if new_pe.is_none() {
            // create new Protoedge and EdgeDefinition
            // The status is kept, not propagated: upstream returns SM_SUCCESS with the
            // out-parameter whatever it was (`Unported` still propagates).
            let created = ignore_soft(self.create_proto_edge(
                k,
                &mut *geometry,
                Some(start_pv),
                Some(end_pv),
                classification1,
                ci1,
                classification2,
                ci2,
            ))?;
            if let Some(pe) = created {
                was_created = true;
                let mut ids = Vec::with_capacity(overlapping.len());
                for o in &overlapping {
                    ids.push(o.ok_or(dangling("overlapping proto edge is null"))?);
                }
                self.proto_edge_mut(pe)
                    .ok_or(dangling("proto edge is deleted"))?
                    .append_overlapping_pes(&ids);
                for o in ids {
                    self.proto_edge_mut(o)
                        .ok_or(dangling("proto edge is deleted"))?
                        .add_overlapping_pe(pe);
                }
            }
            new_pe = created;
        } else if !is_identical {
            // Create new EdgeDef and add it to NewPE's EdgeDef list
            if let Some(pe) = new_pe {
                // (status ignored upstream)
                ignore_soft(self.add_edge_definition_from_intervals(
                    k,
                    &mut *geometry,
                    pe,
                    classification1,
                    ci1,
                    classification2,
                    ci2,
                ))?;
            }
        }
        Ok((new_pe, was_created))
    }

    /// `SmEdgeDefinition::Split(dParamPE, pMidPV, pTopPE, rpNewVD, rpNewED)`: splits
    /// the definition at `param_pe` after its proto edge was split. Returns the new
    /// vertex definition (added to `mid_pv`) and the new upper definition (added to
    /// `top_pe`); `(None, None)` when the parameter is not interior. This definition
    /// becomes the lower part.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted object or a missing curve;
    /// `SM_ERR_NULL_POINTER` for the `NER` checks; those of the end positions and of
    /// `SmCurve::Copy`; [`SolidError::Unported`] for the callees listed in the module
    /// header.
    pub fn edge_definition_split(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        ed: EdgeDefId,
        param_pe: f64,
        mid_pv: Option<ProtoVertexId>,
        top_pe: Option<ProtoEdgeId>,
    ) -> Result<(Option<VertexDefId>, Option<EdgeDefId>)> {
        /// `cbiFudge`.
        const CBI_FUDGE: f64 = 10.0;

        let d = self
            .edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))?;

        // Check that parameter is interior to this EdgeDef.
        if !d.xsect_interval().contains_value(param_pe, -EFF_ZERO) {
            return Ok((None, None));
        }
        let tol = d.raw_tolerance();

        // Takes reversal into account; status ignored.
        let crv_pt = ignore_soft(d.evaluate(k, &*geometry, param_pe, true, false))?
            .and_then(|r| r.0)
            .unwrap_or(Vec3::UNDEF);

        // Use distance to end of this EdgeDef, not distance to our VertexDefs.
        let pe = d
            .proto_edge()
            .and_then(|pe| self.proto_edge(pe))
            .ok_or(dangling("edge definition has no proto edge"))?;
        let start_pv = pe.start_proto_vertex();
        let end_pv = pe.end_proto_vertex();

        let pt = self.edge_def_end_position_at(k, &*geometry, ed, start_pv)?;
        if crv_pt.distance(pt) < tol * CBI_FUDGE {
            return Ok((None, None));
        }
        let pt = self.edge_def_end_position_at(k, &*geometry, ed, end_pv)?;
        if crv_pt.distance(pt) < tol * CBI_FUDGE {
            return Ok((None, None));
        }

        // Ok, go ahead and split. Need to classify point.
        let brep1 = self.which_brep(1);
        let brep2 = self.which_brep(2);
        let mut pc1 = PointClassification::new(zone_tol3d(k, tol));
        let mut pc2 = PointClassification::new(zone_tol3d(k, tol));
        // (statuses ignored: a failed classification keeps its cleared state)
        let tol_3d = zone_tol3d(k, tol);
        if let Some(pc) =
            ignore_soft(geometry.brep_point_3d_classify(k, brep1, crv_pt, tol_3d, true))?
        {
            pc1 = pc;
        }
        if let Some(pc) =
            ignore_soft(geometry.brep_point_3d_classify(k, brep2, crv_pt, tol_3d, true))?
        {
            pc2 = pc;
        }

        let mid_pv = mid_pv.ok_or(dangling("mid proto vertex is null"))?;
        let (new_vd, created) = ignore_soft(
            self.add_vertex_definition_from_pcs(k, &*geometry, mid_pv, &mut pc1, &mut pc2),
        )?
        .map_or((None, false), |(vd, created)| (Some(vd), created));
        let new_vd = new_vd.ok_or(SolidError::null_pointer(OP_ED_SPLIT))?;

        // For cleanup object: don't do it if it was 'found' (vs. created).
        let clean_vd = created.then_some(new_vd);

        let new_ed = match self.copy_edge_definition(ed) {
            Ok(x) => x,
            Err(e) => {
                if let Some(vd) = clean_vd {
                    self.delete_vertex_definition(vd);
                }
                return Err(e);
            }
        };
        // The split's new EdgeDefinition starts unowned (source issue #518).
        self.edge_def_mut(new_ed)
            .ok_or(dangling("edge definition is deleted"))?
            .set_proto_edge(None);

        // Get the parameterizations set correctly before calling AddEdgeDefinition().
        let mut this_ivl = self
            .edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_interval();
        ignore_soft(this_ivl.set_max(param_pe))?;
        self.set_ed_interval(ed, this_ivl)?;
        let mut new_ivl = self
            .edge_def(new_ed)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_interval();
        ignore_soft(new_ivl.set_min(param_pe))?;
        self.set_ed_interval(new_ed, new_ivl)?;

        let top = top_pe.ok_or(dangling("top proto edge is null"))?;
        ignore_soft(self.add_edge_definition(k, &mut *geometry, top, new_ed))?;

        // Copy the three curves; trim the copies and the originals.
        let this_crv3d = self
            .edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_curve_3d();
        let new_crv = curve_of(&*geometry, this_crv3d)?.copy(k)?;
        let new_handle = geometry
            .insert_curve(new_crv)
            .ok_or(SolidError::null_pointer(OP_ED_SPLIT))?;
        self.set_ed_curve_3d(new_ed, Some(new_handle))?;
        self.trim_ed_curve(k, geometry, ed, this_crv3d)?;
        self.trim_ed_curve(k, geometry, new_ed, Some(new_handle))?;

        for which in [1, 2] {
            let uv = self
                .edge_def(ed)
                .ok_or(dangling("edge definition is deleted"))?
                .xsect_uv_curve(which);
            if uv.is_some() {
                let copy = curve_of(&*geometry, uv)?.copy(k)?;
                let handle = geometry
                    .insert_curve(copy)
                    .ok_or(SolidError::null_pointer(OP_ED_SPLIT))?;
                self.edge_def_mut(new_ed)
                    .ok_or(dangling("edge definition is deleted"))?
                    .set_xsect_curve_uv(which, Some(handle));
                self.trim_ed_curve(k, geometry, new_ed, Some(handle))?;
                self.trim_ed_curve(k, geometry, ed, uv)?;
            }
        }

        Ok((Some(new_vd), Some(new_ed)))
    }

    /// Sets an edge definition's interval.
    fn set_ed_interval(&mut self, ed: EdgeDefId, ivl: Extent1d) -> Result<()> {
        self.edge_def_mut(ed)
            .ok_or(dangling("edge definition is deleted"))?
            .set_xsect_interval(ivl);
        Ok(())
    }

    /// Sets an edge definition's 3D curve.
    fn set_ed_curve_3d(&mut self, ed: EdgeDefId, curve: Option<O>) -> Result<()> {
        self.edge_def_mut(ed)
            .ok_or(dangling("edge definition is deleted"))?
            .set_xsect_curve_3d(curve);
        Ok(())
    }

    /// `curve->Trim(pED->m_pXSectInterval)`: the status is ignored and the interval
    /// may be snapped (it is passed by reference upstream).
    fn trim_ed_curve(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        ed: EdgeDefId,
        curve: Option<O>,
    ) -> Result<()> {
        let mut ivl = self
            .edge_def(ed)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_interval();
        let crv = curve
            .and_then(|c| geometry.curve_mut(c))
            .ok_or(dangling("curve handle is null or unknown"))?;
        ignore_soft(crv.trim(k, &mut ivl, TrimMode::Notify))?;
        self.set_ed_interval(ed, ivl)
    }

    /// `SmProtoEdge::Split(dT, rpNewPV, rpNewPE)`: splits the proto edge at `dt` into
    /// a lower part (this proto edge) and a new upper part, with a proto vertex at the
    /// split point, splitting every edge definition as well. Returns the new vertex
    /// and edge; `(None, None)` when `dt` is not interior.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted object; `SM_ERR_NULL_POINTER` when no
    /// vertex is found or created; those of the end positions;
    /// [`SolidError::Unported`] for the callees listed in the module header.
    pub fn proto_edge_split(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        pe: ProtoEdgeId,
        dt: f64,
    ) -> Result<(Option<ProtoVertexId>, Option<ProtoEdgeId>)> {
        let e = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?;

        // no work - dT is not interior to this ProtoEdge.
        if !e.xsect_interval().contains_value(dt, -EFF_ZERO) {
            return Ok((None, None));
        }
        let tol = e.raw_tolerance();

        let crv_pt = ignore_soft(self.proto_edge_evaluate(k, &*geometry, pe, dt, true, false))?
            .and_then(|r| r.0)
            .unwrap_or(Vec3::UNDEF);
        let start_pt = self.proto_edge_end_position(k, &*geometry, pe, true)?;
        let end_pt = self.proto_edge_end_position(k, &*geometry, pe, false)?;

        // no work - Split Pt is within tol of ProtoEdge end point
        if crv_pt.distance(start_pt) < tol {
            return Ok((None, None));
        }
        if crv_pt.distance(end_pt) < tol {
            return Ok((None, None));
        }

        // Split ourself first, then all of our EdgeDefs. The status is ignored.
        let (new_pv, created) = self.find_or_create_proto_vertex(crv_pt)?;

        // create new upper Split child ProtoEdge
        let new_pe = match self.copy_proto_edge(pe) {
            Ok(x) => x,
            Err(e) => {
                // the cleanup object deletes a created vertex
                if created {
                    self.delete_proto_vertex_object(new_pv);
                }
                return Err(e);
            }
        };

        // set Split child ProtoEdge intervals (the upper one is set from this
        // edge's interval after it was changed, as upstream; the upper child gets [dT, dT],
        // plan/upstream-issues.md WR-101)
        let mut lower = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .xsect_interval();
        ignore_soft(lower.set_min_max(lower.min(), dt))?;
        self.proto_edge_mut(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .set_xsect_interval(lower);
        let mut upper = self
            .proto_edge(new_pe)
            .ok_or(dangling("proto edge is deleted"))?
            .xsect_interval();
        ignore_soft(upper.set_min_max(dt, lower.max()))?;
        self.proto_edge_mut(new_pe)
            .ok_or(dangling("proto edge is deleted"))?
            .set_xsect_interval(upper);

        // add split child ProtoEdges to pNewPV connected ProtoEdge list
        {
            let v = self
                .proto_vertex_mut(new_pv)
                .ok_or(dangling("proto vertex is deleted"))?;
            v.add_proto_edge(pe);
            v.add_proto_edge(new_pe);
        }

        // set split child ProtoEdge End ProtoVertices
        let old_end = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .end_proto_vertex();
        {
            let n = self
                .proto_edge_mut(new_pe)
                .ok_or(dangling("proto edge is deleted"))?;
            n.set_end_proto_vertex(old_end);
            n.set_start_proto_vertex(Some(new_pv));
        }
        self.proto_edge_mut(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .set_end_proto_vertex(Some(new_pv));

        // edit upper split child -> EndProtoVertex -> ProtoEdge list
        let end = old_end.ok_or(dangling("proto edge has no end proto vertex"))?;
        {
            let v = self
                .proto_vertex_mut(end)
                .ok_or(dangling("proto vertex is deleted"))?;
            v.add_proto_edge(new_pe);
            ignore_soft(v.remove_proto_edge(pe))?;
        }

        // Add the new ProtoEdge to the ProtoTopoMgr ProtoTopo list
        self.add_proto_edge(new_pe);

        // Now split all of the original ProtoEdge->EdgeDefs.
        let eds: Vec<EdgeDefId> = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .edge_definitions()
            .to_vec();

        // clear new UpperSplitChild->EdgeDefinitions
        self.proto_edge_mut(new_pe)
            .ok_or(dangling("proto edge is deleted"))?
            .clear_edge_definitions();

        for ed in eds {
            ignore_soft(self.edge_definition_split(
                k,
                geometry,
                ed,
                dt,
                Some(new_pv),
                Some(new_pe),
            ))?;
        }

        // relate newPE to this_PE->CoincPFs
        let pfs: Vec<ProtoFaceId> = self
            .proto_edge(pe)
            .ok_or(dangling("proto edge is deleted"))?
            .coinc_proto_faces()
            .to_vec();
        for pf in pfs {
            self.relate_coincident_pf(new_pe, pf)?;
        }

        Ok((Some(new_pv), Some(new_pe)))
    }

    /// `SmProtoTopologyManager::SplitIntersectingProtoEdges(pPE1, pPE2)`: splits two
    /// proto edges (and their definitions) at their intersections. Returns the curves
    /// deleted.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when a proto edge has no definition; [`SolidError::InvalidInput`] for a
    /// deleted object or missing curve; [`SolidError::Unported`] for the callees
    /// listed in the module header.
    pub fn split_intersecting_proto_edges(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        pe1: ProtoEdgeId,
        pe2: ProtoEdgeId,
    ) -> Result<Vec<O>> {
        let mut deleted: Vec<O> = Vec::new();
        let e1 = self
            .proto_edge(pe1)
            .ok_or(dangling("proto edge is deleted"))?;
        let e2 = self
            .proto_edge(pe2)
            .ok_or(dangling("proto edge is deleted"))?;

        // no work - ProtoEdges already meet at a ProtoVertex.
        if e1.start_proto_vertex() == e2.start_proto_vertex()
            || e1.start_proto_vertex() == e2.end_proto_vertex()
            || e1.end_proto_vertex() == e2.start_proto_vertex()
            || e1.end_proto_vertex() == e2.end_proto_vertex()
        {
            return Ok(deleted);
        }

        // no work - ProtoEdge does not have any EdgeDefinitions
        let (Some(&first1), Some(&first2)) =
            (e1.edge_definitions().first(), e2.edge_definitions().first())
        else {
            return Err(SolidError::sm_err(OP_SPLIT_INT)); // [Reg_091215_simon_closed]
        };

        // no work - Skip the pair if they're known to overlap. Handled during Resolve
        if e1.overlapping_pes().contains(&pe2) {
            return Ok(deleted);
        }

        // the curves of the first EdgeDefs
        let crv1 = self
            .edge_def(first1)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_curve_3d();
        let crv2 = self
            .edge_def(first2)
            .ok_or(dangling("edge definition is deleted"))?
            .xsect_curve_3d();
        let dom1 = e1.xsect_interval();
        let dom2 = e2.xsect_interval();
        let tol = self.raw_tolerance();

        // XSect ProtoEdge curves
        let c1 = curve_of(&*geometry, crv1)?;
        let c2 = curve_of(&*geometry, crv2)?;
        let sols = match c1.global_curve_intersect::<CurveHandle>(
            k,
            &dom1,
            c2,
            &dom2,
            tol,
            &CurveIntersectOptions::default(),
        ) {
            Ok(s) => s,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(deleted),
        };
        if sols.is_empty() {
            return Ok(deleted);
        }

        // Found an intersection. If it's interior to either ProtoEdge, split it.
        for sol in sols.as_slice() {
            // Point intersection branch
            if sol.solution_type == SolutionType::SingleValue {
                let t1 = sol.start.param(0)?;
                let t2 = sol.start.param(1)?;

                let iv1 = self
                    .proto_edge(pe1)
                    .ok_or(dangling("proto edge is deleted"))?
                    .xsect_interval();
                if iv1.contains_value(t1, -EFF_ZERO) {
                    ignore_soft(self.proto_edge_split(k, geometry, pe1, t1))?;
                }
                let iv2 = self
                    .proto_edge(pe2)
                    .ok_or(dangling("proto edge is deleted"))?
                    .xsect_interval();
                if iv2.contains_value(t2, -EFF_ZERO) {
                    ignore_soft(self.proto_edge_split(k, geometry, pe2, t2))?;
                }
            } else if sol.solution_type == SolutionType::RangeOfValues {
                // Check whether the two PEs run in the same direction or opposite.
                let same_dir = sol.start.param(1)? < sol.end.param(1)?;

                // T1:
                let t11 = sol.start.param(0)?;
                let (npv11, npe11) = ignore_soft(self.proto_edge_split(k, geometry, pe1, t11))?
                    .unwrap_or((None, None));
                let t21 = sol.start.param(1)?;
                let (npv21, npe21) = ignore_soft(self.proto_edge_split(k, geometry, pe2, t21))?
                    .unwrap_or((None, None));

                // Combine PVs:
                let pv1 = self.which_pv_to_combine(k, &*geometry, pe1, t11, npv11)?;
                let pv2 = self.which_pv_to_combine(k, &*geometry, pe1, t21, npv21)?;
                self.combine_and_delete_pvs(pv1, pv2)?;

                // T2: first decide which pieces to split.
                let pe1_to_split = npe11.unwrap_or(pe1);
                let pe2_to_split = if same_dir { npe21.unwrap_or(pe2) } else { pe2 };

                let t12 = sol.end.param(0)?;
                let (npv12, npe12) =
                    ignore_soft(self.proto_edge_split(k, geometry, pe1_to_split, t12))?
                        .unwrap_or((None, None));
                let t22 = sol.end.param(1)?;
                let (npv22, npe22) =
                    ignore_soft(self.proto_edge_split(k, geometry, pe2_to_split, t22))?
                        .unwrap_or((None, None));
                let _ = npe12;

                // Combine PVs:
                let pv1 = self.which_pv_to_combine(k, &*geometry, pe1_to_split, t12, npv12)?;
                let pv2 = self.which_pv_to_combine(k, &*geometry, pe2_to_split, t22, npv22)?;
                self.combine_and_delete_pvs(pv1, pv2)?;

                // Combine ProtoEdges
                let pe1_to_combine = npe11.unwrap_or(pe1);
                let pe2_to_combine = if same_dir {
                    npe21.unwrap_or(pe2)
                } else {
                    npe22.unwrap_or(pe2)
                };
                ignore_soft(self.combine_proto_edge(
                    k,
                    &mut *geometry,
                    pe1_to_combine,
                    pe2_to_combine,
                ))?;

                ignore_soft(self.remove_proto_edge(pe2_to_combine))?;
                self.forget_proto_edge(pe2_to_combine);
                deleted.extend(self.delete_proto_edge_object(pe2_to_combine));
            }
        }
        Ok(deleted)
    }

    /// The combine-and-delete step of `SplitIntersectingProtoEdges`: when both proto
    /// vertices exist and differ, combines the second into the first and deletes it.
    fn combine_and_delete_pvs(
        &mut self,
        pv1: Option<ProtoVertexId>,
        pv2: Option<ProtoVertexId>,
    ) -> Result<()> {
        if let (Some(a), Some(b)) = (pv1, pv2) {
            if a != b {
                ignore_soft(self.combine_proto_vertex(a, b))?;
                self.proto_vertex_mut(b)
                    .ok_or(dangling("proto vertex is deleted"))?
                    .clear_vertex_definitions();
                ignore_soft(self.remove_proto_vertex(b))?;
                self.delete_proto_vertex_object(b);
            }
        }
        Ok(())
    }

    /// `SmProtoTopologyManager::GlueCoincidentBrepVertices()`: glues the final
    /// vertices of the vertex definitions of each proto vertex, squeezing or deleting
    /// the short edges between them, and drops the definitions that referred to what
    /// was deleted. Returns the curves deleted. `mates` is the intersector (`m_pTI`):
    /// its `GetOtherMate` and `RemoveRelationship` drop the relationships of deleted
    /// topology ([`Mates`]).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted or unknown object; the failing status
    /// (`eRet`) of an edge deletion; [`SolidError::Unported`] for the callees listed
    /// in the module header.
    pub fn glue_coincident_brep_vertices(
        &mut self,
        k: &mut Kernel,
        brep: &mut (impl ProtoBrep<O> + BrepGlue<O>),
        mates: &mut dyn Mates<O>,
    ) -> Result<Vec<O>> {
        let mut deleted_curves: Vec<O> = Vec::new();
        let pvs: Vec<ProtoVertexId> = self.proto_vertices().to_vec();

        // We need to check whether these are in any other ProtoVertices, ProtoEdges, or
        // ProtoFaces:
        let mut deleted_edges1: Vec<O> = Vec::new();
        let mut deleted_edges2: Vec<O> = Vec::new();
        let mut ret: Result<()> = Ok(());

        for &pv in &pvs {
            let mut vds: Vec<Option<VertexDefId>> = self
                .proto_vertex(pv)
                .ok_or(dangling("proto vertex is deleted"))?
                .vertex_definitions()
                .iter()
                .map(|&v| Some(v))
                .collect();
            let num_vds = vds.len();
            if num_vds < 2 {
                continue;
            }

            // Glue the first VD to all others.
            let vd1 = vds
                .first()
                .copied()
                .flatten()
                .ok_or(dangling("vertex definition is null"))?;

            // Keep track deleted Vertices and corresponding VD
            let mut deleted_v1: Vec<Option<O>> = vec![None; num_vds];
            let mut deleted_v2: Vec<Option<O>> = vec![None; num_vds];

            for jj in 1..num_vds {
                let vd2 = vds
                    .get(jj)
                    .copied()
                    .flatten()
                    .ok_or(dangling("vertex definition is null"))?;

                // If there is no Edge connecting the two Vertices, then Glue them.
                let final_of = |s: &Self, vd: VertexDefId, which: i32| -> Result<Option<O>> {
                    Ok(s.vertex_def(vd)
                        .ok_or(dangling("vertex definition is deleted"))?
                        .final_vertex(which))
                };
                let in_deleted = |list: &[Option<O>], v: O| list.contains(&Some(v));

                let Some(b1v1) = final_of(self, vd1, 1)? else {
                    continue;
                };
                if in_deleted(&deleted_v1, b1v1) {
                    continue;
                }
                let Some(b1v2) = final_of(self, vd2, 1)? else {
                    continue;
                };
                if in_deleted(&deleted_v1, b1v2) {
                    continue;
                }
                let Some(b2v1) = final_of(self, vd1, 2)? else {
                    continue;
                };
                if in_deleted(&deleted_v2, b2v1) {
                    continue;
                }
                let Some(b2v2) = final_of(self, vd2, 2)? else {
                    continue;
                };
                if in_deleted(&deleted_v2, b2v2) {
                    continue;
                }

                let mut dist1 = BIG_DOUBLE;
                let mut dist2 = BIG_DOUBLE;
                let mut xs1 = xsect_tol3d(k, USE_DEFAULT);
                let mut xs2 = xsect_tol3d(k, USE_DEFAULT);
                let mut was_deleted1 = false;
                let mut was_deleted2 = false;

                if in_deleted(&deleted_v1, b1v2) {
                    self.vertex_def_mut(vd2)
                        .ok_or(dangling("vertex definition is deleted"))?
                        .set_final_vertex(1, None);
                    was_deleted1 = true;
                } else {
                    dist1 = vertex_distance(brep, b1v1, b1v2)?;
                    xs1 = xsect_tol(brep, b1v1, b1v2)?;
                }
                if in_deleted(&deleted_v2, b2v2) {
                    self.vertex_def_mut(vd2)
                        .ok_or(dangling("vertex definition is deleted"))?
                        .set_final_vertex(2, None);
                    was_deleted2 = true;
                } else {
                    dist2 = vertex_distance(brep, b2v1, b2v2)?;
                    xs2 = xsect_tol(brep, b2v1, b2v2)?;
                }

                let edges1 = brep.vertex_common_edges(b1v1, b1v2)?;
                let edges2 = brep.vertex_common_edges(b2v1, b2v2)?;

                // Brep 1 then Brep 2.
                for which in [1, 2] {
                    let (v1, v2, dist, xs, edges, deleted_v, deleted_e) = if which == 1 {
                        (
                            b1v1,
                            b1v2,
                            dist1,
                            xs1,
                            &edges1,
                            &mut deleted_v1,
                            &mut deleted_edges1,
                        )
                    } else {
                        (
                            b2v1,
                            b2v2,
                            dist2,
                            xs2,
                            &edges2,
                            &mut deleted_v2,
                            &mut deleted_edges2,
                        )
                    };
                    let flag = if which == 1 {
                        &mut was_deleted1
                    } else {
                        &mut was_deleted2
                    };
                    if dist > xs {
                    } else if edges.is_empty() {
                        if v1 != v2 {
                            ignore_soft(brep.glue_vertices(
                                k,
                                self.which_brep(which),
                                v1,
                                v2,
                                None,
                            ))?;
                            set_slot(deleted_v, jj, v2);
                            self.vertex_def_mut(vd2)
                                .ok_or(dangling("vertex definition is deleted"))?
                                .set_final_vertex(which, None);
                            *flag = true;
                        }
                    } else {
                        for &edge in edges {
                            // UB-241 (source fix, source fork da8d05e): stop
                            // before the GlueVertices path; the delete branch runs as
                            // upstream.
                            let v2_deleted = deleted_v.get(jj).copied().flatten() == Some(v2);
                            let curve_handle =
                                brep.edge_curve(edge).ok_or(dangling("edge is unknown"))?;
                            let ivl = brep
                                .edge_interval(edge)
                                .ok_or(dangling("edge is unknown"))?;
                            let len = curve_of(brep, curve_handle)?
                                .approximate_length(k, &ivl, 5, false)?
                                .0;
                            if len < xs {
                                if v2_deleted && brep.edge_other_vertex(edge, Some(v1))? != Some(v1)
                                {
                                    break;
                                }
                                // If any to-be-deleted topology is mated, remove the
                                // relationship when done.
                                let edge_mate = mates
                                    .other_mate(edge)
                                    .filter(|m| m.class_kind() == ClassObjectKind::Edge);
                                let vertex_mate = mates
                                    .other_mate(v2)
                                    .filter(|m| m.class_kind() == ClassObjectKind::Vertex);
                                let other_vtx = brep.edge_other_vertex(edge, Some(v1))?;
                                let stat = if other_vtx == Some(v1) {
                                    // Can't call SqueezeEdge() in this case.
                                    brep.delete_edge(k, self.which_brep(which), edge)
                                } else {
                                    let r =
                                        brep.brep_squeeze_edge(k, self.which_brep(which), edge, v1); // deletes v2
                                    set_slot(deleted_v, jj, v2);
                                    r
                                };
                                deleted_e.push(edge);
                                match stat {
                                    Ok(()) => {
                                        *flag = true;
                                        if let Some(m) = edge_mate {
                                            ignore_soft(mates.remove_relationship(edge, m))?;
                                        }
                                        if let Some(m) = vertex_mate {
                                            ignore_soft(mates.remove_relationship(v2, m))?;
                                        }
                                        let found =
                                            self.find_edge_definition_from_edge(Some(edge), which)?;
                                        if let Some(ed) = found {
                                            let d = self
                                                .edge_def_mut(ed)
                                                .ok_or(dangling("edge definition is deleted"))?;
                                            if which == 1 {
                                                // the curve may have been deleted [B673]
                                                if curve_handle == d.xsect_curve_3d() {
                                                    d.set_xsect_curve_3d(None);
                                                }
                                            } else {
                                                // IntCurve3d is a Brep1 object [B673]
                                                d.set_xsect_curve_3d(None);
                                            }
                                            d.set_xsect_curve_uv(1, None);
                                            d.set_xsect_curve_uv(2, None);
                                            // Don't remove the PE if empty.
                                            ignore_soft(self.remove_edge_definition(ed, false))?;
                                            deleted_curves.extend(
                                                self.edge_def_mut(ed)
                                                    .ok_or(dangling("edge definition is deleted"))?
                                                    .destruct(),
                                            );
                                            deleted_curves.extend(self.delete_edge_definition(ed));
                                        }
                                    }
                                    Err(e) if e.is_unported() => return Err(e),
                                    Err(e) => ret = Err(e),
                                }
                            } else {
                                // UB-241: stop before GlueVertices
                                if v2_deleted {
                                    break;
                                }
                                // coincident Vertices connected by a long Edge: Glue.
                                if v1 != v2 {
                                    ignore_soft(brep.glue_vertices(
                                        k,
                                        self.which_brep(which),
                                        v1,
                                        v2,
                                        None,
                                    ))?;
                                    set_slot(deleted_v, jj, v2);
                                    self.vertex_def_mut(vd2)
                                        .ok_or(dangling("vertex definition is deleted"))?
                                        .set_final_vertex(which, None);
                                    *flag = true;
                                }
                            }
                        }
                    }
                }

                if was_deleted1 || was_deleted2 {
                    ignore_soft(
                        self.proto_vertex_mut(pv)
                            .ok_or(dangling("proto vertex is deleted"))?
                            .remove_vertex_definition(vd2),
                    )?;
                    if let Some(slot) = vds.get_mut(jj) {
                        *slot = None;
                    }
                    self.delete_vertex_definition(vd2);
                }
            }

            // Check for stale Brep topology: we might have deleted some Final SmVerts
            // that are pointed to by other PVs.
            let del1: Vec<O> = deleted_v1.iter().flatten().copied().collect();
            let del2: Vec<O> = deleted_v2.iter().flatten().copied().collect();
            if !del1.is_empty() || !del2.is_empty() {
                for &this_pv in &pvs {
                    let defs: Vec<VertexDefId> = self
                        .proto_vertex(this_pv)
                        .ok_or(dangling("proto vertex is deleted"))?
                        .vertex_definitions()
                        .to_vec();
                    let mut keep = Vec::with_capacity(defs.len());
                    for vd in defs {
                        let d = self
                            .vertex_def(vd)
                            .ok_or(dangling("vertex definition is deleted"))?;
                        let hit = d.final_vertex(1).is_some_and(|v| del1.contains(&v))
                            || d.final_vertex(2).is_some_and(|v| del2.contains(&v));
                        if hit {
                            self.delete_vertex_definition(vd);
                        } else {
                            keep.push(vd);
                        }
                    }
                    *self
                        .proto_vertex_mut(this_pv)
                        .ok_or(dangling("proto vertex is deleted"))?
                        .vertex_definitions_mut() = keep;
                }
            }

            // Also have to check deleted SmEdges.
            if !deleted_edges1.is_empty() || !deleted_edges2.is_empty() {
                let pes: Vec<ProtoEdgeId> = self.proto_edges().to_vec();
                for &this_pe in &pes {
                    let mut mm = 0;
                    loop {
                        let list = self
                            .proto_edge(this_pe)
                            .ok_or(dangling("proto edge is deleted"))?
                            .edge_definitions();
                        let Some(&ed) = list.get(mm) else { break };
                        let d = self
                            .edge_def(ed)
                            .ok_or(dangling("edge definition is deleted"))?;
                        let hit = d.final_edge(1).is_some_and(|e| deleted_edges1.contains(&e))
                            || d.final_edge(2).is_some_and(|e| deleted_edges2.contains(&e));
                        if hit {
                            ignore_soft(
                                self.proto_edge_mut(this_pe)
                                    .ok_or(dangling("proto edge is deleted"))?
                                    .remove_edge_definition(ed),
                            )?;
                            // No Destruct(): this EdgeDefinition was imprinted, so its
                            // curves belong to the Breps' Edges (the deleted Edge took
                            // its own along). The destructor deletes only the curves no
                            // Brep took.
                            deleted_curves.extend(self.delete_edge_definition(ed));
                        }
                        // (the index advances past the element that moved down)
                        mm += 1;
                    }
                }

                // ... and ProtoFaces have EdgeDefinitions too.
                let pfs: Vec<ProtoFaceId> = self.proto_faces().to_vec();
                for &this_pf in &pfs {
                    let coinc: Vec<ProtoEdgeId> = self
                        .proto_face(this_pf)
                        .ok_or(dangling("proto face is deleted"))?
                        .coinc_proto_edges()
                        .to_vec();
                    for this_pe in coinc {
                        let mut nn = 0;
                        loop {
                            let list = self
                                .proto_edge(this_pe)
                                .ok_or(dangling("proto edge is deleted"))?
                                .edge_definitions();
                            let Some(&ed) = list.get(nn) else { break };
                            let d = self
                                .edge_def(ed)
                                .ok_or(dangling("edge definition is deleted"))?;
                            let hit = d.final_edge(1).is_some_and(|e| deleted_edges1.contains(&e))
                                || d.final_edge(2).is_some_and(|e| deleted_edges2.contains(&e));
                            if hit {
                                ignore_soft(self.remove_edge_definition(ed, false))?;
                            }
                            nn += 1;
                        }
                    }
                }
            }
        }

        ret.map(|()| deleted_curves)
    }

    /// `SmProtoTopologyManager::ResolveOverlappingProtoEdges()`: replaces each group of
    /// overlapping proto edges with the non-overlapping proto edges of the coincident
    /// curve classified against the source faces. Returns the curves deleted.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a deleted or null object or an edge without
    /// faces or curve; [`SolidError::Unported`] for the callees listed in the module
    /// header.
    pub fn resolve_overlapping_proto_edges(
        &mut self,
        k: &Kernel,
        geometry: &mut impl ProtoBrep<O>,
        ti_approx_tol_3d: f64,
    ) -> Result<Vec<O>> {
        let mut deleted_curves: Vec<O> = Vec::new();
        let tol = self.raw_tolerance();

        // For each PE (at least one of the SrcTopos should be an edge.)
        let mut ii = 0usize;
        // repeat the index when the list was changed by removing a group
        while let Some(&pe) = self.proto_edges().get(ii) {
            let group = self.overlapping_pe_group(pe)?;
            let found_group = !group.is_empty();

            // skip PEs not in a Overlapping PE Group
            if !found_group {
                ii += 1;
                continue;
            }

            // lowest topology search locals
            let mut topo1: Option<O> = None;
            let mut topo2: Option<O> = None;
            let mut dim1: u64 = 3;
            let mut dim2: u64 = 3;

            // For every Overlap PE: save lowest order SrcTopology
            for &overlap in &group {
                let eds = self
                    .proto_edge(overlap)
                    .ok_or(dangling("proto edge is deleted"))?
                    .edge_definitions();
                // Empty ProtoEdges are not a problem. Arbitrary: use the 1st EdgeDef.
                let Some(&first) = eds.first() else { continue };
                let d = self
                    .edge_def(first)
                    .ok_or(dangling("edge definition is deleted"))?;
                let t1 = as_topology(d.src_topo(1));
                let t2 = as_topology(d.src_topo(2));
                let this_dim1 = dimension(t1)?;
                let this_dim2 = dimension(t2)?;
                if this_dim1 == 0 {
                } else if this_dim1 < dim1 {
                    dim1 = this_dim1;
                    topo1 = t1;
                }
                if this_dim2 == 0 {
                } else if this_dim2 < dim2 {
                    dim2 = this_dim2;
                    topo2 = t2;
                }
            }

            // PFs keep a list of coincident PEs. Check every ProtoFace: the group can be
            // coincident with more than one, and its PEs are deleted below.
            let mut replace_coin_pe = false;
            // every ProtoFace that listed a PE of the group
            let mut replace_pfs: Vec<ProtoFaceId> = Vec::new();
            let pfs: Vec<ProtoFaceId> = self.proto_faces().to_vec();
            for &pf in &pfs {
                let face = self
                    .proto_face_mut(pf)
                    .ok_or(dangling("proto face is deleted"))?;
                let list = face.coinc_proto_edges_mut();
                let size = list.len();
                list.retain(|p| !group.contains(p));
                if list.len() != size {
                    replace_coin_pe = true;
                    replace_pfs.push(pf);
                }
            }

            // Delete all OverlappingPE->EdgeDef and any PV that has no remaining PEs
            let mut overlap_pvs: Vec<ProtoVertexId> = Vec::new();
            for &overlap in &group {
                let (start, end, eds) = {
                    let e = self
                        .proto_edge(overlap)
                        .ok_or(dangling("proto edge is deleted"))?;
                    (
                        e.start_proto_vertex(),
                        e.end_proto_vertex(),
                        e.edge_definitions().to_vec(),
                    )
                };
                let start = start.ok_or(dangling("proto edge has no start proto vertex"))?;
                let end = end.ok_or(dangling("proto edge has no end proto vertex"))?;
                if !overlap_pvs.contains(&start) {
                    overlap_pvs.push(start);
                }
                if !overlap_pvs.contains(&end) {
                    overlap_pvs.push(end);
                }

                // Remove and delete all EdgeDefs from this OverlappingPE
                for ed in eds {
                    ignore_soft(self.remove_edge_definition(ed, false))?;
                    deleted_curves.extend(
                        self.edge_def_mut(ed)
                            .ok_or(dangling("edge definition is deleted"))?
                            .destruct(),
                    );
                    deleted_curves.extend(self.delete_edge_definition(ed));
                }

                // remove PE from End and Start ProtoVertex->PE list
                ignore_soft(
                    self.proto_vertex_mut(start)
                        .ok_or(dangling("proto vertex is deleted"))?
                        .remove_proto_edge(overlap),
                )?;
                ignore_soft(
                    self.proto_vertex_mut(end)
                        .ok_or(dangling("proto vertex is deleted"))?
                        .remove_proto_edge(overlap),
                )?;

                // remove the OverlappingPEs from this PE; clear the end vertices
                {
                    let e = self
                        .proto_edge_mut(overlap)
                        .ok_or(dangling("proto edge is deleted"))?;
                    e.clear_overlapping_pes();
                    e.set_start_proto_vertex(None);
                    e.set_end_proto_vertex(None);
                }

                // remove OverlapPE from ProtoTopoMgr, then delete it
                ignore_soft(self.remove_proto_edge(overlap))?;
                self.forget_proto_edge(overlap);
                deleted_curves.extend(self.delete_proto_edge_object(overlap));
            }

            // for every accumulated end ProtoVertex with no other PEs, delete it after
            // removing and deleting all its VertexDefs
            for &opv in &overlap_pvs {
                let v = self
                    .proto_vertex(opv)
                    .ok_or(dangling("proto vertex is deleted"))?;
                if v.num_proto_edges() == 0 {
                    let vds: Vec<VertexDefId> = v.vertex_definitions().to_vec();
                    for vd in vds {
                        // TRUE = delete ProtoVertexOwner when its count goes to zero
                        ignore_soft(self.remove_vertex_definition(vd, true))?;
                        self.delete_vertex_definition(vd);
                    }
                }
            }

            // classify the Edge->Curve against the SrcFaces
            let e1 = as_kind(topo1, ClassObjectKind::Edge);
            let e2 = as_kind(topo2, ClassObjectKind::Edge);
            let mut f1 = as_kind(topo1, ClassObjectKind::Face);
            let mut f2 = as_kind(topo2, ClassObjectKind::Face);

            let mut cc1s: Vec<CurveClassification<O>> = Vec::new();
            let mut cc2s: Vec<CurveClassification<O>> = Vec::new();

            if let (Some(edge1), Some(edge2)) = (e1, e2) {
                // SrcTopo Edge-Edge branch
                let faces1 = geometry.edge_faces(edge1)?;
                let faces2 = geometry.edge_faces(edge2)?;
                let h1 = geometry
                    .edge_curve(edge1)
                    .ok_or(dangling("edge is unknown"))?;
                let h2 = geometry
                    .edge_curve(edge2)
                    .ok_or(dangling("edge is unknown"))?;
                let ivl1 = geometry
                    .edge_interval(edge1)
                    .ok_or(dangling("edge is unknown"))?;
                let ivl2 = geometry
                    .edge_interval(edge2)
                    .ok_or(dangling("edge is unknown"))?;

                // arbitrary - pick XSectFace as first Edge Face
                f1 = Some(*faces1.first().ok_or(dangling("edge has no faces"))?);
                f2 = Some(*faces2.first().ok_or(dangling("edge has no faces"))?);

                // Find the Edge->Curve coincidences; the status is ignored.
                let sols = {
                    let c1 = curve_of(&*geometry, h1)?;
                    let c2 = curve_of(&*geometry, h2)?;
                    ignore_soft(c1.global_coincidence_checker::<CurveHandle>(
                        k,
                        &ivl1,
                        c2,
                        &ivl2,
                        tol,
                        true,
                        [None, None],
                    ))?
                    .map(|c| c.solutions)
                    .unwrap_or_default()
                };

                for sol in &sols {
                    if sol.solution_type == SolutionType::RangeOfValues {
                        // PEs expect the same curve classified against each face
                        let ivl = Extent1d::from_min_max(sol.start.param(0)?, sol.end.param(0)?);
                        let c1h = h1.ok_or(dangling("edge has no curve"))?;
                        let mut cc1 = new_curve_classification(k, c1h, ivl, tol)?;
                        let mut cc2 = new_curve_classification(k, c1h, ivl, tol)?;

                        // Classify the coincident curves against their faces
                        let face1 = f1.ok_or(dangling("face is null"))?;
                        let face2 = f2.ok_or(dangling("face is null"))?;
                        ignore_soft(geometry.face_curve_on_classify_mut(k, face1, true, &mut cc1))?;
                        ignore_soft(geometry.face_curve_on_classify_mut(k, face2, true, &mut cc2))?;

                        // a curve for every common interval classified in both
                        let _ = cc1.extract_geometry(
                            k,
                            &*geometry,
                            &mut cc2,
                            ExtractOptions::default(),
                        )?;

                        // Make sure that a parameter is set in each PointClass object.
                        ignore_soft(cc1.set_point_class_parameters(k, &*geometry, false))?;
                        ignore_soft(cc2.set_point_class_parameters(k, &*geometry, false))?;

                        cc1s.push(cc1);
                        cc2s.push(cc2);
                    }
                }
            } else if (e1.is_some() && f2.is_some()) || (f1.is_some() && e2.is_some()) {
                // SrcTopo Edge-Face branch: use the Edge->Curve
                let edge = e1.or(e2).ok_or(dangling("edge is null"))?;
                let handle = geometry
                    .edge_curve(edge)
                    .ok_or(dangling("edge is unknown"))?
                    .ok_or(dangling("edge has no curve"))?;
                let ivl = geometry
                    .edge_interval(edge)
                    .ok_or(dangling("edge is unknown"))?;
                let mut cc1 = new_curve_classification(k, handle, ivl, tol)?;
                let mut cc2 = new_curve_classification(k, handle, ivl, tol)?;

                // Pick a SrcFace object for the SrcEdge object
                let faces = geometry.edge_faces(edge)?;
                let first = *faces.first().ok_or(dangling("edge has no faces"))?;
                if e1.is_some() {
                    f1 = Some(first);
                } else {
                    f2 = Some(first);
                }

                // Classify Edge->Curve against both SrcFaces
                ignore_soft(geometry.face_curve_on_classify_mut(
                    k,
                    f1.ok_or(dangling("face is null"))?,
                    true,
                    &mut cc1,
                ))?;
                ignore_soft(geometry.face_curve_on_classify_mut(
                    k,
                    f2.ok_or(dangling("face is null"))?,
                    true,
                    &mut cc2,
                ))?;

                let _ = cc1.extract_geometry(k, &*geometry, &mut cc2, ExtractOptions::default())?;
                ignore_soft(cc1.set_point_class_parameters(k, &*geometry, false))?;
                ignore_soft(cc2.set_point_class_parameters(k, &*geometry, false))?;
                cc1s.push(cc1);
                cc2s.push(cc2);
            }

            // For each curve classification, create the new PEs
            for (cc1, cc2) in cc1s.iter_mut().zip(cc2s.iter_mut()) {
                for kk in 0..cc1.size() {
                    let mut ci1 = *cc1.at(kk)?;
                    let mut ci2 = *cc2.at(kk)?;
                    // the status is ignored
                    let result = ignore_soft(self.find_or_create_proto_edge(
                        k,
                        &mut *geometry,
                        ti_approx_tol_3d,
                        cc1,
                        &mut ci1,
                        cc2,
                        &mut ci2,
                    ))?;
                    *cc1.at_mut(kk)? = ci1;
                    *cc2.at_mut(kk)? = ci2;

                    // when any OverlappingPE was a CoinPE in a PF - add all New or Found
                    // PEs back into each of those PF->CoincPE lists
                    if let Some((Some(new_pe), _)) = result {
                        if replace_coin_pe {
                            for &pf in &replace_pfs {
                                let list = self
                                    .proto_face_mut(pf)
                                    .ok_or(dangling("proto face is deleted"))?
                                    .coinc_proto_edges_mut();
                                if !list.contains(&new_pe) {
                                    list.push(new_pe);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(deleted_curves)
    }
}

/// The dimension `sm_TopoDimension` returns, as the `ULONG` upstream stores it
/// (`-1` wraps to a huge value).
fn dimension<O: ClassObject>(topo: Option<O>) -> Result<u64> {
    let t = topo.ok_or(dangling("source topology is null"))?;
    let d = topo_dimension(t);
    Ok(u64::try_from(d).unwrap_or(u64::MAX))
}

/// `pV1->GetPoint().DistanceBetween(pV2->GetPoint())`.
fn vertex_distance<O: Copy>(brep: &impl ProtoBrep<O>, v1: O, v2: O) -> Result<f64> {
    let p1 = brep.vertex_point(v1)?;
    let p2 = brep.vertex_point(v2)?;
    Ok(p1.distance(p2))
}

/// `SmTol::GetXSectTol3d(pV1, pV2)`: the sum of the two zone tolerances.
fn xsect_tol<O: Copy>(brep: &impl ProtoBrep<O>, v1: O, v2: O) -> Result<f64> {
    let t1 = brep
        .topology_tolerance(v1)
        .ok_or(dangling("vertex is unknown"))?;
    let t2 = brep
        .topology_tolerance(v2)
        .ok_or(dangling("vertex is unknown"))?;
    Ok(t1 + t2)
}

/// `sDeletedVertices[jj] = vertex`.
fn set_slot<O: Copy>(list: &mut [Option<O>], index: usize, vertex: O) {
    if let Some(slot) = list.get_mut(index) {
        *slot = Some(vertex);
    }
}

/// Not upstream (WR-410; the source fork's `sm_EdgeCrossingSine`): the sine of the
/// angle at which a curve interval's curve (`curve`, over `interval`) crosses the edge its
/// end classification `pc` lies on, at the interval's end (`at_end`) or start; `-1` when
/// that end is not on an edge with a curve, its parameters are unset, an evaluation
/// fails or a tangent vanishes.
///
/// # Errors
///
/// [`SolidError::Unported`] from the evaluations.
fn edge_crossing_sine<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ProtoBrep<O>,
    curve: Option<O>,
    interval: Extent1d,
    pc: &PointClassification<O>,
    at_end: bool,
) -> Result<f64> {
    let edge_curve = pc
        .edge_object()
        .and_then(|e| geometry.edge_curve(e))
        .flatten()
        .and_then(|c| geometry.curve(c));
    let curve = curve.and_then(|c| geometry.curve(c));
    let (Some(edge_curve), Some(curve)) = (edge_curve, curve) else {
        return Ok(-1.0);
    };
    if !pc.are_parameters_set() {
        return Ok(-1.0);
    }
    let mut t = pc.pre_snap_param();
    if t == BIG_DOUBLE || t == crate::math::UNDEF_DOUBLE {
        t = if at_end {
            interval.max()
        } else {
            interval.min()
        };
    }
    let tangent = |r: Result<Vec<Vec3>>| -> Result<Option<Vec3>> {
        match r {
            Ok(d) => Ok(d.get(1).copied()),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(None),
        }
    };
    let (Some(c), Some(e)) = (
        tangent(curve.evaluate(k, t, 1, !at_end, false))?,
        tangent(edge_curve.evaluate(k, pc.t_param(), 1, true, false))?,
    ) else {
        return Ok(-1.0);
    };
    let len = c.length() * e.length();
    if len.is_nan() || len <= 0.0 {
        return Ok(-1.0);
    }
    Ok(c.cross(e).length() / len)
}

/// Not upstream (WR-410; the source fork's `sm_CrossingSine`): the sine of the angle
/// at which a face pair's intersection curve crosses an edge at one end (`at_end`, else
/// the start) of the curve interval pair `ci1`, `ci2`: the smaller of the two when both
/// ends lie on edges, `-1` when neither does.
///
/// # Errors
///
/// Those of [`edge_crossing_sine`].
fn crossing_sine<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ProtoBrep<O>,
    classification1: &CurveClassification<O>,
    ci1: &CurveInterval<O>,
    classification2: &CurveClassification<O>,
    ci2: &CurveInterval<O>,
    at_end: bool,
) -> Result<f64> {
    let pick = |ci: &CurveInterval<O>| if at_end { ci.end } else { ci.start };
    let sine1 = edge_crossing_sine(
        k,
        geometry,
        classification1.curve(),
        ci1.interval,
        &pick(ci1),
        at_end,
    )?;
    let sine2 = edge_crossing_sine(
        k,
        geometry,
        classification2.curve(),
        ci2.interval,
        &pick(ci2),
        at_end,
    )?;
    if sine1 < 0.0 {
        return Ok(sine2);
    }
    if sine2 < 0.0 {
        return Ok(sine1);
    }
    Ok(sine1.min(sine2))
}

#[cfg(test)]
mod tests {
    use super::super::p1::proto::{ProtoGeometry, ProtoTopology, VertexDefinition};
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::extent2d::Extent2d;
    use crate::intersect::point_class::PointClassType;
    use crate::math::{Point2, Point3};
    use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use crate::surface::surface::Surface;
    use crate::surface::volume::Volume;
    use crate::topology::Brep;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    const fn vtx(i: usize) -> H {
        H(i, ClassObjectKind::Vertex)
    }

    const fn edg(i: usize) -> H {
        H(i, ClassObjectKind::Edge)
    }

    /// Vertices 0 and 1 of B-rep 1 coincide and are joined by the long edges 0 and 1
    /// (both on curve 0); vertices 2 and 3 of B-rep 2 are far apart. Records every
    /// `GlueVertices` call and refuses one of a vertex already glued away; records
    /// every `SqueezeEdge` and `DeleteEdge` (after a squeeze, `edge_other_vertex` answers
    /// vertex 0).
    struct Pool {
        curves: Vec<Curve>,
        glued: Vec<(H, H)>,
        squeezed: Vec<H>,
        deleted: Vec<H>,
    }

    impl Pool {
        fn new() -> Self {
            Self::with_length(10.0)
        }

        /// As [`Pool::new`] with edges `length` long.
        fn with_length(length: f64) -> Self {
            let line = Line::create_line_segment(
                3,
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(length, 0.0, 0.0),
                None,
            )
            .unwrap();
            Self {
                curves: vec![Curve::from(line)],
                glued: Vec::new(),
                squeezed: Vec::new(),
                deleted: Vec::new(),
            }
        }
    }

    impl ClassGeometry<H> for Pool {
        fn curve(&self, o: H) -> Option<&Curve> {
            (o.1 == ClassObjectKind::Curve)
                .then(|| self.curves.get(o.0))
                .flatten()
        }
        fn surface(&self, _o: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _o: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _o: H) -> Option<Point3> {
            None
        }
    }

    impl ProtoGeometry<H> for Pool {
        fn curve_mut(&mut self, o: H) -> Option<&mut Curve> {
            (o.1 == ClassObjectKind::Curve)
                .then(|| self.curves.get_mut(o.0))
                .flatten()
        }
    }

    impl ProtoTopology<H> for Pool {
        fn vertex_use_faces(&self, _vertex: H) -> Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_use_faces(&self, _edge: H) -> Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_start_vertex(&self, _edge: H) -> Result<Option<H>> {
            Ok(Some(vtx(0)))
        }
        fn edge_other_vertex(&self, _edge: H, _vertex: Option<H>) -> Result<Option<H>> {
            Ok(Some(if self.squeezed.is_empty() {
                vtx(1)
            } else {
                vtx(0)
            }))
        }
        fn vertex_point(&self, vertex: H) -> Result<Point3> {
            Ok(match vertex.0 {
                2 => Point3::new(0.0, 5.0, 0.0),
                3 => Point3::new(0.0, -5.0, 0.0),
                _ => Point3::new(0.0, 0.0, 0.0),
            })
        }
        fn edge_brep(&self, _edge: H) -> Result<Option<crate::geom::attribute::ObjectKey>> {
            Ok(None)
        }
        fn edge_faces(&self, _edge: H) -> Result<Vec<H>> {
            Ok(Vec::new())
        }
    }

    impl ProtoBrep<H> for Pool {
        fn edge_interval(&self, _edge: H) -> Option<Extent1d> {
            Some(self.curves.first()?.natural_interval())
        }
        fn edge_curve(&self, _edge: H) -> Option<Option<H>> {
            Some(Some(H(0, ClassObjectKind::Curve)))
        }
        fn topology_tolerance(&self, _topo: H) -> Option<f64> {
            Some(1e-5)
        }
        fn face_uv_domain(&self, _face: H) -> Option<Extent2d> {
            None
        }
        fn set_face_uv_domain(&mut self, _face: H, _domain: Extent2d) -> Option<()> {
            None
        }
        fn face_surface(&self, _face: H) -> Option<Option<H>> {
            None
        }
        fn edge_edgeuse_of_face(&self, _edge: H, _face: H) -> Result<Option<H>> {
            Ok(None)
        }
        fn edge_is_connected_to_face(&self, _edge: H, _face: H) -> Result<bool> {
            Ok(false)
        }
        fn insert_curve(&mut self, curve: Curve) -> Option<H> {
            self.curves.push(curve);
            Some(H(self.curves.len() - 1, ClassObjectKind::Curve))
        }
        fn vertex_common_edges(&self, v1: H, v2: H) -> Result<Vec<H>> {
            Ok(if (v1, v2) == (vtx(0), vtx(1)) {
                vec![edg(0), edg(1)]
            } else {
                Vec::new()
            })
        }
        fn face_curve_on_classify(
            &self,
            _k: &Kernel,
            _face: H,
            _do_point_classify: bool,
            _cc: &mut CurveClassification<H>,
        ) -> Result<bool> {
            Ok(false)
        }
        fn face_trim_uv_domain(&mut self, _k: &Kernel, _face: H) -> Result<Extent2d> {
            Err(SolidError::invalid_input("test", "unused"))
        }
        fn face_point_classify_uv(
            &mut self,
            _k: &Kernel,
            _face: H,
            _uv: Point2,
            _source_tolerance: f64,
        ) -> Result<PointClassType> {
            Err(SolidError::invalid_input("test", "unused"))
        }
        fn brep_squeeze_edge(
            &mut self,
            _k: &mut Kernel,
            _brep: H,
            edge: H,
            _survivor: H,
        ) -> Result<()> {
            self.squeezed.push(edge);
            Ok(())
        }
        fn brep_point_3d_classify(
            &mut self,
            _k: &Kernel,
            _brep: H,
            _point: Point3,
            _tolerance: f64,
            _boundary_intersections: bool,
        ) -> Result<PointClassification<H>> {
            Err(SolidError::invalid_input("test", "unused"))
        }
        fn brep_make_edge_in_face(
            &mut self,
            _k: &Kernel,
            _brep: H,
            _face: H,
            _start: H,
            _end: H,
            _curve: Curve,
            _uv_curve: Option<Curve>,
            _interval: Extent1d,
            _orientation: OrientType,
            _tolerance: f64,
        ) -> Result<(Option<H>, Option<H>, Option<H>)> {
            Err(SolidError::invalid_input("test", "unused"))
        }
        fn edgeuse_evaluate_binormal(&mut self, _k: &Kernel, _eu: H, _param: f64) -> Result<Vec3> {
            Err(SolidError::invalid_input("test", "unused"))
        }
    }

    impl BrepGlue<H> for Pool {
        fn delete_edge(&mut self, _k: &mut Kernel, _brep: H, edge: H) -> Result<()> {
            self.deleted.push(edge);
            Ok(())
        }
        fn glue_vertices(
            &mut self,
            _k: &mut Kernel,
            _brep: H,
            v1: H,
            v2: H,
            _opt_wire_edges_in_brep: Option<&[H]>,
        ) -> Result<()> {
            let again = self.glued.iter().any(|&(_, gone)| gone == v2);
            self.glued.push((v1, v2));
            if again {
                return Err(SolidError::invalid_input(
                    "test",
                    "vertex already glued away",
                ));
            }
            Ok(())
        }
    }

    fn region_pc(o: H) -> PointClassification<H> {
        let mut pc = PointClassification::new(1e-5);
        pc.set_class_object(PointClassType::Region, Some(o));
        pc
    }

    #[test]
    fn glue_stops_once_the_second_vertex_is_glued_away() {
        // da8d05e (#371): two coincident vertices joined by two long common edges;
        // gluing for the first edge deletes the second vertex, which the second
        // edge's step glued again (upstream: a use-after-free)
        let mut k = Kernel::default();
        let mut pool = Pool::new();
        let b = H(100, ClassObjectKind::Other);
        let e = Brep::new();
        let mut m = ProtoTopologyManager::new(&k, (b, &e), (b, &e), b, 1e-5, 1e-3, -1.0).unwrap();
        let p = Point3::new(0.0, 0.0, 0.0);
        let pv = m.new_proto_vertex(p, 1e-5).unwrap();
        m.add_proto_vertex(pv);
        let r = H(50, ClassObjectKind::Region);
        for (v1, v2) in [(vtx(0), vtx(2)), (vtx(1), vtx(3))] {
            let mut vd =
                VertexDefinition::new(&k, &pool, &region_pc(r), &region_pc(r), p, 1e-5, 1e-6, None)
                    .unwrap();
            vd.set_final_vertex(1, Some(v1));
            vd.set_final_vertex(2, Some(v2));
            let id = m.insert_vertex_definition(vd).unwrap();
            m.add_vertex_definition(pv, id).unwrap();
        }

        m.glue_coincident_brep_vertices(&mut k, &mut pool, &mut NoMates)
            .unwrap();
        assert_eq!(pool.glued, vec![(vtx(0), vtx(1))]);
        // the second definition, whose B-rep 1 vertex is gone, is deleted
        assert_eq!(m.proto_vertex(pv).unwrap().vertex_definitions().len(), 1);
    }

    #[test]
    fn glue_deletes_a_second_short_common_edge_after_the_squeeze() {
        // the delete branch runs as upstream after a squeeze;
        // boolean.api_brep crossing_creases_union_d1e-05
        let mut k = Kernel::default();
        let mut pool = Pool::with_length(1e-6);
        let b = H(100, ClassObjectKind::Other);
        let e = Brep::new();
        let mut m = ProtoTopologyManager::new(&k, (b, &e), (b, &e), b, 1e-5, 1e-3, -1.0).unwrap();
        let p = Point3::new(0.0, 0.0, 0.0);
        let pv = m.new_proto_vertex(p, 1e-5).unwrap();
        m.add_proto_vertex(pv);
        let r = H(50, ClassObjectKind::Region);
        for (v1, v2) in [(vtx(0), vtx(2)), (vtx(1), vtx(3))] {
            let mut vd =
                VertexDefinition::new(&k, &pool, &region_pc(r), &region_pc(r), p, 1e-5, 1e-6, None)
                    .unwrap();
            vd.set_final_vertex(1, Some(v1));
            vd.set_final_vertex(2, Some(v2));
            let id = m.insert_vertex_definition(vd).unwrap();
            m.add_vertex_definition(pv, id).unwrap();
        }

        m.glue_coincident_brep_vertices(&mut k, &mut pool, &mut NoMates)
            .unwrap();
        assert_eq!(pool.squeezed, vec![edg(0)]);
        assert_eq!(pool.deleted, vec![edg(1)]);
        assert!(pool.glued.is_empty());
        assert_eq!(m.proto_vertex(pv).unwrap().vertex_definitions().len(), 1);
    }

    /// A vertex definition first placed by a grazing crossing is placed again by a
    /// steeper one, whose misplacement bound along the edge (gap over sine) is smaller,
    /// and not by a later, more grazing one (WR-410, #1059).
    #[test]
    fn issue_1059_a_steeper_crossing_places_the_vertex_definition() {
        let k = Kernel::default();
        let pool = Pool::new();
        let curve = H(0, ClassObjectKind::Curve);
        let on_curve = |t: f64| {
            let mut pc = PointClassification::new(1e-6);
            pc.set_class_object(PointClassType::Curve, Some(curve));
            pc.set_t_param(t, false);
            pc
        };
        // two points 1e-8 apart, within the curve's zone tolerance
        let (pc1, pc2) = (on_curve(0.5), on_curve(0.5 + 1e-9));
        let mut vd = VertexDefinition::new(
            &k,
            &pool,
            &pc1,
            &pc2,
            Vec3::new(5.0, 0.0, 0.0),
            1e-6,
            0.0,
            None,
        )
        .unwrap();
        // the first crossing to arrive places it, here at a grazing angle
        assert!(!vd.place_by_crossing(&k, &pool, &pc1, &pc2, 0.02).unwrap());
        // a steeper crossing fixes it more closely
        assert!(vd.place_by_crossing(&k, &pool, &pc1, &pc2, 0.9).unwrap());
        // a more grazing one than that does not
        assert!(!vd.place_by_crossing(&k, &pool, &pc1, &pc2, 0.5).unwrap());
    }
}
