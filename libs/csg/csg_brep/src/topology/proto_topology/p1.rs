//! Proto topology, part 1: owning-topology lists, assemblies, the copy-B-rep map, the
//! proto-topology containers with their accessors, list management and evaluation,
//! the static helpers of `SmProtoTopology.cpp`, and the API transformations.
//!
//! Upstream: `source/SMLib/inc/SmOwningTopology.h`, `source/SMLib/src/SmOwningTopology.cpp`,
//! `source/SMLib/inc/SmAssembly.h`, `source/SMLib/src/SmAssembly.cpp`,
//! `source/SMLib/inc/SmAssemblyInstance.h`, `source/SMLib/src/SmAssemblyInstance.cpp`,
//! `source/SMLib/inc/SmCopyBrepMap.h`, `source/SMLib/src/SmCopyBrepMap.cpp`,
//! `source/SMLib/inc/SmProtoTopology.h`, `source/SMLib/src/SmProtoTopology.cpp`,
//! `source/SM_API/src/SmApiGeneral.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`OwningTopology`], [`OwningTopology::new`], [`OwningTopology::destroy`] | `SmOwningTopology`, its constructor and destructor |
//! | [`OwningTopology::size`], [`OwningTopology::list`], [`OwningTopology::set_list`] | `GetSize`, `GetList`, `SetList` |
//! | [`OwningTopology::update_list_size`] | `SmOwningTopology::UpdateListSize` |
//! | [`OwningTopology::insert_after`] | `SmOwningTopology::InsertAfter` |
//! | [`OwningTopology::pre_insert`], [`OwningTopology::post_insert`] | `PreInsert`, `PostInsert` |
//! | [`OwningTopology::remove`] | `SmOwningTopology::Remove` |
//! | [`OwningTopology::is_in_list`] | `SmOwningTopology::IsInList` |
//! | [`OwningTopology::get_all`], [`OwningTopology::get_all_ignoring_status`] | `SmOwningTopology::GetAll` |
//! | [`OwningTopology::reverse_list`] | `SmOwningTopology::ReverseList` |
//! | [`OwningRecords`], [`destroy_topology`] | the `SmOwningTopology*` list owner, `SmTopology::~SmTopology` |
//! | [`owning_topology::TYPE`], [`owning_topology::TYPE_STRING`], [`owning_topology::CLASS_STRING`], [`owning_topology::is_kind_of`] | `SmOwningTopology::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`Assembly`], [`Assembly::new`], [`Assembly::destroy`] | `SmAssembly`, its constructor, `~SmAssembly` |
//! | [`Assembly::assembly_instances`] | `SmAssembly::GetAssemblyInstances` |
//! | [`Assembly::add_instance`], [`Assembly::remove_instance`] | `SmAssembly::AddInstance`, `RemoveInstance` |
//! | [`assembly::ASSEMBLY_TYPE`], [`assembly::ASSEMBLY_TYPE_STRING`], [`assembly::ASSEMBLY_CLASS_STRING`], [`assembly::assembly_is_kind_of`] | `SmAssembly::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`AssemblyInstance`], [`AssemblyInstance::new`], [`AssemblyInstance::destroy`] | `SmAssemblyInstance`, its constructor, `~SmAssemblyInstance` |
//! | [`AssemblyInstance::parent_assembly`] | `SmAssemblyInstance::GetParentAssembly` |
//! | [`assembly::ASSEMBLY_INSTANCE_TYPE`], [`assembly::ASSEMBLY_INSTANCE_TYPE_STRING`], [`assembly::ASSEMBLY_INSTANCE_CLASS_STRING`], [`assembly::assembly_instance_is_kind_of`] | `SmAssemblyInstance::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`CopyBrepMap`], [`CopyBrepMap::new`], [`CopyBrepMap::destroy`] | `SmCopyBrepMap`, its constructor and destructor |
//! | [`CopyBrepMap::set_at`], [`CopyBrepMap::get_at`] | `SmCopyBrepMap::SetAt`, `GetAt` |
//! | [`proto::can_imprint`] | `sm_CanImprint` (static) |
//! | [`proto::topo_dimension`] | `sm_TopoDimension` (static) |
//! | [`proto::find_position_from_pcs`], [`proto::PositionFromPcs`] | `sm_FindPositionFromPCs` (static) |
//! | [`proto::are_same_curve`] | `sm_AreSameCurve` (static) |
//! | [`proto::are_connected`] | the four `sm_AreConnected` overloads (static) |
//! | [`ProtoTopology`] | the B-rep walks those helpers make (see Deviations) |
//! | [`ProtoTopologyManager::which_pv_to_combine`] | `sm_WhichPVToCombine` (static) |
//! | [`proto::is_interior`] | `sm_IsInterior` (static) |
//! | [`proto::are_overlapping`] | `sm_AreOverlapping` (static) |
//! | [`proto::find_ed_on_degen_face`] | `sm_FindEDOnDegenFace` |
//! | [`VertexDefinition`], [`VertexDefinition::new`], [`ProtoTopologyManager::delete_vertex_definition`] | `SmVertexDefinition`, its constructor and destructor |
//! | [`VertexDefinition::position`], [`VertexDefinition::tolerance`], [`VertexDefinition::gap`], [`VertexDefinition::final_vertex`], [`VertexDefinition::proto_vertex`] | `GetPosition`, `GetTolerance`, `GetGap`, `GetFinalVertex`, `GetProtoVertex` |
//! | [`VertexDefinition::brep1_topo`], [`VertexDefinition::brep2_topo`], [`VertexDefinition::brep1_topo_type`], [`VertexDefinition::brep2_topo_type`] | `GetBrep1Topo`, `GetBrep2Topo`, `GetBrep1TopoType`, `GetBrep2TopoType` |
//! | [`VertexDefinition::src_topo`], [`VertexDefinition::src_topo_type`], [`VertexDefinition::brep_t_param`], [`VertexDefinition::brep_uv_param`], [`VertexDefinition::brep_point_classif`] | `GetSrcTopo`, `GetSrcTopo_TYPE`, `GetBrepTParam`, `GetBrepUVParam`, `GetBrepPointClassif` |
//! | [`VertexDefinition::set_position`], [`VertexDefinition::set_proto_vertex`], [`VertexDefinition::set_final_vertex`] | `SetPosition`, `SetProtoVertex`, `SetFinalVertex` |
//! | [`VertexDefinition::set_src_topo`], [`VertexDefinition::set_point_parameters`] | `SmVertexDefinition::SetSrcTopo`, `SetPointParameters` |
//! | [`ProtoTopologyManager::vertex_def_num_proto_edges`] | `SmVertexDefinition::GetNumProtoEdges` |
//! | [`ProtoTopologyManager::resolve_vertex_definition`] | `SmVertexDefinition::Resolve` |
//! | [`EdgeDefinition`], [`EdgeDefinition::new`], [`EdgeDefinition::copy_definition`], [`EdgeDefinition::destroy`] | `SmEdgeDefinition`, its constructors, `~SmEdgeDefinition` |
//! | [`EdgeDefinition::destruct`] | `SmEdgeDefinition::Destruct` |
//! | [`EdgeDefinition::xsect_curve_3d`], [`EdgeDefinition::xsect_interval`], [`EdgeDefinition::tolerance`], [`EdgeDefinition::xsect_uv_curve`] | `GetXSectCurve3d`, `GetXSectInterval`, `GetTolerance`, `GetXSectUVCurve` |
//! | [`EdgeDefinition::src_topo`], [`EdgeDefinition::src_topo_type`], [`EdgeDefinition::proto_edge`], [`EdgeDefinition::final_edge`] | `GetSrcTopo`, `GetSrcTopo_TYPE`, `GetProtoEdge`, `GetFinalEdge` |
//! | [`EdgeDefinition::set_xsect_curve_3d`], [`EdgeDefinition::set_xsect_curve_uv`], [`EdgeDefinition::set_proto_edge`], [`EdgeDefinition::set_src_topo`] | `SetXSectCurve3d`, `SetXSectCurveUV`, `SetProtoEdge`, `SetSrcTopo` |
//! | [`EdgeDefinition::set_xsect_interval_min_max`] | `SmEdgeDefinition::SetXSectInterval(double, double)` |
//! | [`ProtoTopologyManager::edge_def_start_proto_vertex`], [`ProtoTopologyManager::edge_def_end_proto_vertex`] | `SmEdgeDefinition::GetStartProtoVertex`, `GetEndProtoVertex` |
//! | [`ProtoTopologyManager::edge_def_end_position_at`], [`EdgeDefinition::end_position`] | the two `SmEdgeDefinition::GetEndPosition` |
//! | [`EdgeDefinition::evaluate`], [`EdgeDefinition::evaluate_mid`] | `SmEdgeDefinition::Evaluate`, `EvaluateMid` |
//! | [`EdgeDefinition::reverse_orientation`], [`EdgeDefinition::reparameterize`] | `SmEdgeDefinition::ReverseOrientation`, `Reparameterize` |
//! | [`ProtoTopologyManager::is_same_edge_definition`] | `SmEdgeDefinition::IsSameEdgeDefinition` |
//! | [`EdgeDefinition::is_interior_point`] | `SmEdgeDefinition::IsInteriorPoint` |
//! | [`ProtoVertex`], [`ProtoTopologyManager::new_proto_vertex`], [`ProtoTopologyManager::new_proto_vertex_with_pcs`], [`ProtoTopologyManager::delete_proto_vertex_object`] | `SmProtoVertex`, two of its constructors, `~SmProtoVertex` |
//! | [`ProtoVertex::position`], [`ProtoVertex::tolerance`], [`ProtoVertex::num_vertex_definitions`], [`ProtoVertex::num_proto_edges`], [`ProtoVertex::vertex_definitions`], [`ProtoVertex::proto_edges`] | `GetPosition`, `GetTolerance`, `GetNumVertexDefinitions`, `GetNumProtoEdges`, `GetVertexDefinitions`, `GetProtoEdges` |
//! | [`ProtoVertex::clear_vertex_definitions`], [`ProtoVertex::add_proto_edge`] | `ClearVertexDefinitions`, `AddProtoEdge` |
//! | [`ProtoVertex::remove_vertex_definition`], [`ProtoVertex::remove_proto_edge`] | `SmProtoVertex::RemoveVertexDefinition`, `RemoveProtoEdge` |
//! | [`ProtoTopologyManager::add_vertex_definition`] | `SmProtoVertex::AddVertexDefinition(SmVertexDefinition*)` |
//! | [`ProtoTopologyManager::find_matching_vertex_definition`] | `SmProtoVertex::FindMatchingVertexDefinition` |
//! | [`ProtoTopologyManager::closest_vertex_definition`] | `SmProtoVertex::GetClosestVertexDefinition` |
//! | [`ProtoTopologyManager::proto_vertex_final_vertex`] | `SmProtoVertex::GetFinalVertex` |
//! | [`ProtoTopologyManager::combine_proto_vertex`] | `SmProtoVertex::CombineProtoVertex` |
//! | [`ProtoEdge`], [`ProtoTopologyManager::new_proto_edge`], [`ProtoTopologyManager::copy_proto_edge`], [`ProtoTopologyManager::delete_proto_edge_object`] | `SmProtoEdge`, its constructors, `~SmProtoEdge` |
//! | [`ProtoEdge::xsect_interval`], [`ProtoEdge::tolerance`], [`ProtoEdge::start_proto_vertex`], [`ProtoEdge::end_proto_vertex`], [`ProtoEdge::overlapping_pes`] | `GetXSectInterval`, `GetTolerance`, `GetStartProtoVertex`, `GetEndProtoVertex`, both `GetOverlappingPEs` |
//! | [`ProtoEdge::num_edge_definitions`], [`ProtoEdge::edge_definitions`] | `GetNumEdgeDefinitions`, `GetEdgeDefinitions` |
//! | [`ProtoEdge::set_start_proto_vertex`], [`ProtoEdge::set_end_proto_vertex`], [`ProtoEdge::add_overlapping_pe`], [`ProtoEdge::append_overlapping_pes`], [`ProtoEdge::clear_edge_definitions`] | `SetStartProtoVertex`, `SetEndProtoVertex`, `AddOverlappingPE`, `AppendOverlappingPEs`, `ClearEdgeDefinitions` |
//! | [`ProtoEdge::replace_proto_vertex`], [`ProtoEdge::remove_edge_definition`], [`ProtoEdge::remove_proto_vertex`] | `SmProtoEdge::ReplaceProtoVertex`, `RemoveEdgeDefinition`, `RemoveProtoVertex` |
//! | [`ProtoTopologyManager::overlapping_pe_group`] | `SmProtoEdge::GetOverlappingPEGroup` |
//! | [`ProtoTopologyManager::proto_edge_end_position_at`], [`ProtoTopologyManager::proto_edge_end_position`] | the two `SmProtoEdge::GetEndPosition` |
//! | [`ProtoTopologyManager::relate_coincident_pf`] | `SmProtoEdge::RelateCoincidentPF` |
//! | [`ProtoTopologyManager::proto_edge_evaluate`], [`ProtoTopologyManager::proto_edge_evaluate_mid`] | `SmProtoEdge::Evaluate`, `EvaluateMid` |
//! | [`ProtoTopologyManager::is_same_proto_edge`] | `SmProtoEdge::IsSameProtoEdge` |
//! | [`ProtoFace`], [`ProtoTopologyManager::new_proto_face`] | `SmProtoFace`, its constructor and destructor |
//! | [`ProtoFace::src_topo`], [`ProtoFace::coinc_proto_edges`], [`ProtoFace::set_src_topo`] | `SmProtoFace::GetSrcTopo`, both `GetCoincProtoEdges`, `SetSrcTopo` |
//! | [`ProtoTopologyManager`], [`ProtoTopologyManager::new`], [`ProtoTopologyManager::destroy`], [`ProtoTopologyManager::reset`] | `SmProtoTopologyManager`, its constructor, destructor, `Reset` |
//! | [`ProtoTopologyManager::which_brep`], [`ProtoTopologyManager::tolerance`], [`ProtoTopologyManager::proto_vertices`], [`ProtoTopologyManager::proto_edges`], [`ProtoTopologyManager::proto_faces`] | `GetWhichBrep`, `GetTolerance`, `GetProtoVertices`, `GetProtoEdges`, `GetProtoFaces` |
//! | [`ProtoTopologyManager::add_proto_vertex`], [`ProtoTopologyManager::add_proto_edge`], [`ProtoTopologyManager::add_proto_face`] | `AddProtoVertex`, `AddProtoEdge`, `AddProtoFace` |
//! | [`ProtoTopologyManager::find_proto_vertex`], [`ProtoTopologyManager::find_edge_definition_from_edge`] | `FindProtoVertex`, `FindEdgeDefinitionFromEdge` |
//! | [`ProtoTopologyManager::all_vertex_definitions`], [`ProtoTopologyManager::all_edge_definitions`] | `GetAllVertexDefinitions`, `GetAllEdgeDefinitions` |
//! | [`ProtoTopologyManager::remove_vertex_definition`], [`ProtoTopologyManager::remove_proto_vertex`], [`ProtoTopologyManager::remove_proto_edge`] | `SmProtoTopologyManager::RemoveVertexDefinition`, `RemoveProtoVertex`, `RemoveProtoEdge` |
//! | [`ProtoTopologyManager::check_overlapping_proto_edges`] | `SmProtoTopologyManager::CheckOverlappingProtoEdges` |
//! | [`ProtoTopologyManager::create_proto_face`] | `SmProtoTopologyManager::CreateProtoFace` |
//! | [`ProtoTopologyManager::can_combine_edge_definitions`] | `SmProtoTopologyManager::CanCombineEdgeDefinitions` |
//! | [`ProtoTopologyManager::remove_coincident_no_edge_proto_vertex`] | `SmProtoTopologyManager::RemoveCoincidentNoEdgeProtoVertex` |
//! | [`proto::MANAGER_TYPE`], [`proto::MANAGER_TYPE_STRING`], [`proto::MANAGER_CLASS_STRING`], [`proto::manager_is_kind_of`] | `SmProtoTopologyManager::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`api_transform::transform_independent_object`], [`api_transform::TransformObject`] | `sm_TransformIndependentObject` (anonymous namespace) |
//! | [`api_transform::PolyBrepTransform`], [`api_transform::PolyBrepTarget`] | the `SmPolyBrep*` it transforms (`SmPolyBrep::Transform`) |
//! | [`api_transform::api_transform`], [`api_transform::api_scale`], [`api_transform::api_scale_by_pt`], [`api_transform::api_translate`], [`api_transform::api_rotate`] | `SmApiTransform`, `SmApiScale`, `SmApiScaleByPt`, `SmApiTranslate`, `SmApiRotate` |
//!
//! Deviations:
//! - **Lists.** An owner's `m_pList` / `m_lListSize` is an [`OwningTopology`]; the
//!   members' links stay in their [`Topology`](crate::topology::topology::Topology)
//!   records, edited through a [`TopologyRecords`](crate::topology::topology::TopologyRecords)
//!   store as the friend class edits them. Operations that dereference a null link or
//!   walk a list that never returns to its head (undefined or non-terminating
//!   upstream) return [`SolidError::InvalidInput`](crate::SolidError) instead; the
//!   walks keep a visited set for that. `GetAll`'s partial array on its `SM_ERR` path
//!   is [`OwningTopology::get_all_ignoring_status`]. `FixBackPointers`, `UpdateList`,
//!   `ConcatenateList`, `TypicalListOwnerUse`, `AssertValid` and `Dump` are not part of
//!   this unit (unused in the call graph).
//! - **Destructors.** `~SmTopology`'s removal from the owner's list is
//!   [`destroy_topology`] (the owner's list data comes from an [`OwningRecords`] store).
//!   The assembly destructors and constructors take the store and the object's key.
//!   Names are plain strings (the `SM_TBLOCK_SIZE` buffer is not modelled).
//! - **Proto objects** live in the [`ProtoTopologyManager`]'s store, addressed by
//!   typed ids; upstream allocates them with `new (pOwner)`. Members that reach the
//!   manager through a back pointer are manager methods taking the object's id. A
//!   deleted object's slot is empty; dereferencing a deleted or null object (undefined
//!   upstream) is [`SolidError::InvalidInput`](crate::SolidError), except in the
//!   destructors, where such references (a use after free upstream) are skipped.
//! - **Handles.** Topology objects, curves, B-reps and the topology intersector are
//!   caller-chosen handles `O` ([`ClassObject`](crate::intersect::point_class::ClassObject)),
//!   as in the point classification; the curves behind them come from a
//!   [`ClassGeometry`](crate::intersect::point_class::ClassGeometry), or a
//!   [`ProtoGeometry`] for the curve edits. Destructors that delete curves return the
//!   handles deleted. `SM_CAST_PTR(SmTopology, ..)` of a point object (undefined
//!   upstream) treats it as non-topology.
//! - **Constructor tolerance.** `SmProtoTopologyManager` only reads the topology
//!   intersector's `GetThisApproxTol3d`, passed as a value; the override tolerance is
//!   overwritten by it, as upstream. Each B-rep is passed as its handle and its
//!   [`Brep`](crate::topology::Brep), read only by the fallback tolerance.
//! - **Uninitialised members** of objects upstream allocates with `smos_Calloc` (the
//!   final edges of an edge definition) start as `None`.
//! - **Out-params** become return values: `IsInList`'s index, `FindProtoVertex`'s
//!   distance, `IsSameEdgeDefinition`'s identity flag, `IsInteriorPoint`'s parameter
//!   (meaningful when the result is true), `CanCombineEdgeDefinitions`' choice, the
//!   positions and tangents of the evaluations (`None` for what was not asked),
//!   `CheckOverlappingProtoEdges`' proto edges (upstream appends them).
//! - **Ignored statuses** of evaluations leave the point `SM_UNDEF` (upstream's
//!   `SetUninitialized`), and are otherwise dropped, except [`SolidError::Unported`](crate::SolidError).
//! - `sm_FindPositionFromPCs` does not compute `SmTol::GetXSectTol3d` of the two
//!   objects: it only feeds a diagnostic message.
//! - `CanCombineEdgeDefinitions` keeps upstream's comparison of the second curve over
//!   the first definition's interval. Its degenerate-face branch
//!   (`sm_FindEDOnDegenFace`) is unreachable from it (two different edges are never
//!   connected) but is ported.
//! - **B-rep walks.** The topology walks of `sm_AreConnected`, `GetFinalVertex` and
//!   `sm_FindEDOnDegenFace` (`SmVertex::GetVertexuses`, `SmVertexuse::GetFaceuse`,
//!   `SmFaceuse::GetFace`, `SmEdge::GetEdgeuses`, `SmEdgeuse::GetFace`,
//!   `SmEdge::GetStartVertex`, `GetOtherVertex`, `GetBrep`, `GetFaces`,
//!   `SmVertex::GetPoint`) go through a [`ProtoTopology`] for the handles;
//!   [`Brep`](crate::topology::Brep) implements it over its own entities with units
//!   `topology.vertex`, `topology.vertexuse`, `topology.edge` and
//!   `topology.edgeuse_p1`. `sm_FindEDOnDegenFace`'s status, which
//!   `CanCombineEdgeDefinitions` ignores, leaves the choice 0.
//! - **Poly B-reps** reach `SmApiTransform` as an [`api_transform::PolyBrepTransform`]
//!   (an [`api_transform::PolyBrepTarget`]: the poly B-rep with its notification
//!   environment).
//! - **Callees in units not ported yet** return [`SolidError::Unported`](crate::SolidError):
//!   `SmBrep::GetTolerance` (`topology.brep_p1`) in the manager's fallback tolerance,
//!   `SmFace::GetEdges` (`topology.face_p4`) for edges sharing a face in
//!   `sm_FindEDOnDegenFace` (unreachable, see the degenerate-face branch above), and
//!   part 5's `SmBrep::Transform` reaches `SmBrep::Notify` (`topology.brep_p2`).
//!   `SmProtoVertex::ContainsPoint` / `DistanceTo` (unit `query.face_p2`) are
//!   [`crate::query::face::p2`]'s.
//! - **Copy map.** `SetAt` takes two objects and `GetAt` of an object without an entry
//!   is [`SolidError::InvalidInput`](crate::SolidError): upstream's release `SM_ASSERT`s
//!   call `GetType()` on null objects there (plan/upstream-issues.md UB-98).
//! - **Manager destructor** leaves the objects it does not list in the store (upstream
//!   allocates them from the context).
//! - `RemoveCoincidentNoEdgeProtoVertex` keeps upstream's `return SM_SUCCESS` (read as
//!   `TRUE`) when the point solve finds nothing (plan/upstream-issues.md WR-73); the
//!   opt-in `Kernel::fixes.proto_vertex_miss_is_false` returns `FALSE` there
//!   (source issue #848).
//! - `Dump`, `Draw` and the `SM_DEBUG_CODE` blocks are not ported.
//! - **Recursion.** None.

pub mod api_transform;
pub mod assembly;
pub mod copy_brep_map;
pub mod owning_topology;
pub mod proto;

pub use assembly::{Assembly, AssemblyInstance};
pub use copy_brep_map::CopyBrepMap;
pub use owning_topology::{destroy_topology, OwningRecords, OwningTopology};
pub use proto::{
    EdgeDefId, EdgeDefinition, ProtoEdge, ProtoEdgeId, ProtoFace, ProtoFaceId, ProtoGeometry,
    ProtoTopology, ProtoTopologyManager, ProtoVertex, ProtoVertexId, VertexDefId, VertexDefinition,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::api_transform::{self, TransformObject};
    use super::assembly::{assembly_instance_is_kind_of, assembly_is_kind_of, ASSEMBLY_TYPE};
    use super::*;
    use crate::curve::curve::p2::Curve;
    use crate::curve::line::Line;
    use crate::error::SolidError;
    use crate::geom::attribute::{Attribute, AttributeStore, Behavior, ObjectKey};
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::geom::extent1d::Extent1d;
    use crate::geom::tol::zone_tol3d;
    use crate::intersect::point_class::{
        ClassGeometry, ClassObject, ClassObjectKind, PointClassType,
    };
    use crate::kernel::Kernel;
    use crate::math::{min2, Point3, Vec3, EFF_ZERO};
    use crate::surface::surface::Surface;
    use crate::surface::volume::Volume;
    use crate::topology::topology::{Topology, TopologyRecords};
    use crate::topology::Brep;

    /// A store of owning objects keyed by index.
    #[derive(Default)]
    struct Store {
        recs: crate::topology::topology::NotifyRecords,
        lists: BTreeMap<ObjectKey, OwningTopology>,
    }

    impl TopologyRecords for Store {
        fn topology(&self, key: ObjectKey) -> Option<&Topology> {
            self.recs.get(&key)
        }
        fn topology_mut(&mut self, key: ObjectKey) -> Option<&mut Topology> {
            self.recs.get_mut(&key)
        }
    }

    impl OwningRecords for Store {
        fn owning(&self, key: ObjectKey) -> Option<OwningTopology> {
            self.lists.get(&key).copied()
        }
        fn set_owning(&mut self, key: ObjectKey, owning: OwningTopology) -> Option<()> {
            *self.lists.get_mut(&key)? = owning;
            Some(())
        }
    }

    fn store(n: u64) -> Store {
        let mut s = Store::default();
        for i in 0..n {
            s.recs.insert(ObjectKey(i), Topology::new());
            s.lists.insert(ObjectKey(i), OwningTopology::new());
        }
        s
    }

    fn members(s: &Store, owner: u64) -> Vec<u64> {
        let l = s.owning(ObjectKey(owner)).unwrap();
        l.get_all(s, &AttributeStore::new(), None, None)
            .unwrap()
            .iter()
            .map(|k| k.0)
            .collect()
    }

    #[test]
    fn owning_list_insert_reverse_remove() {
        let mut s = store(4);
        let o = ObjectKey(0);
        let mut l = s.owning(o).unwrap();
        l.post_insert(o, &mut s, ObjectKey(1)).unwrap();
        l.post_insert(o, &mut s, ObjectKey(2)).unwrap();
        l.pre_insert(o, &mut s, ObjectKey(3)).unwrap();
        s.set_owning(o, l).unwrap();
        assert_eq!(members(&s, 0), vec![3, 1, 2]);
        assert_eq!(l.size(), 3);
        assert_eq!(l.is_in_list(&s, Some(ObjectKey(2))).unwrap(), (true, 2));
        assert_eq!(l.is_in_list(&s, None).unwrap(), (false, 3));

        l.reverse_list(&mut s).unwrap();
        assert_eq!(members(&s, 0), vec![3, 2, 1]);

        // removing the head moves it on; removing an object not owned is SM_ERR
        l.remove(o, &mut s, ObjectKey(3)).unwrap();
        s.set_owning(o, l).unwrap();
        assert_eq!(l.list(), Some(ObjectKey(2)));
        assert!(l.remove(o, &mut s, ObjectKey(3)).is_err());
        let mut l2 = l;
        l2.update_list_size(&s).unwrap();
        assert_eq!(l2.size(), 2);
    }

    #[test]
    fn owning_list_get_all_filters() {
        let mut s = store(3);
        let o = ObjectKey(0);
        let mut l = s.owning(o).unwrap();
        l.post_insert(o, &mut s, ObjectKey(1)).unwrap();
        l.post_insert(o, &mut s, ObjectKey(2)).unwrap();
        let mut attrs = AttributeStore::new();
        let h = attrs
            .insert(Attribute::new(7, Behavior::StandaloneCopy))
            .unwrap();
        attrs.add_attribute(ObjectKey(2), h).unwrap();
        let with7 = l.get_all(&s, &attrs, Some(7), None).unwrap();
        assert_eq!(with7, vec![ObjectKey(2)]);
        let target = l.get_all(&s, &attrs, None, Some(ObjectKey(1))).unwrap();
        assert_eq!(target, vec![ObjectKey(1)]);
        // a count lower than the list overruns: SM_ERR, the members walked so far
        let mut short = l;
        short.set_list(l.list());
        let mut bad = OwningTopology::new();
        bad.set_list(l.list());
        assert!(bad.get_all(&s, &attrs, None, None).is_err());
        assert_eq!(
            bad.get_all_ignoring_status(&s, &attrs, None, None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn owning_list_broken_walk_is_invalid_input() {
        let mut s = store(3);
        // a cycle that never returns to the head
        s.recs.get_mut(&ObjectKey(1)).unwrap().set_list_links(
            Some(ObjectKey(0)),
            Some(ObjectKey(2)),
            Some(ObjectKey(2)),
        );
        s.recs.get_mut(&ObjectKey(2)).unwrap().set_list_links(
            Some(ObjectKey(0)),
            Some(ObjectKey(2)),
            Some(ObjectKey(1)),
        );
        let mut l = OwningTopology::new();
        l.set_list(Some(ObjectKey(1)));
        assert!(matches!(
            l.is_in_list(&s, Some(ObjectKey(0))),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            l.reverse_list(&mut s),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn assembly_instances_and_destructors() {
        let mut s = store(4);
        let a = ObjectKey(0);
        let i1 = AssemblyInstance::new(
            &mut s,
            ObjectKey(1),
            Some("i1"),
            a,
            None,
            Axis2Placement::default(),
            Vec3::new(1.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        let _i2 = AssemblyInstance::new(
            &mut s,
            ObjectKey(2),
            None,
            a,
            Some(ObjectKey(3)),
            Axis2Placement::default(),
            Vec3::new(1.0, 1.0, 1.0),
            true,
        )
        .unwrap();
        assert_eq!(
            Assembly::assembly_instances(&s, a).unwrap(),
            vec![ObjectKey(1), ObjectKey(2)]
        );
        assert_eq!(
            AssemblyInstance::parent_assembly(&s, ObjectKey(1)).unwrap(),
            Some(a)
        );
        i1.destroy(&mut s, ObjectKey(1)).unwrap();
        assert_eq!(
            Assembly::assembly_instances(&s, a).unwrap(),
            vec![ObjectKey(2)]
        );
        Assembly::new(Some("a")).destroy(&mut s, a).unwrap();
        assert_eq!(
            AssemblyInstance::parent_assembly(&s, ObjectKey(2)).unwrap(),
            None
        );
        assert!(assembly_is_kind_of(ASSEMBLY_TYPE));
        assert!(!assembly_is_kind_of(owning_topology::TYPE));
        assert!(!assembly_instance_is_kind_of(ASSEMBLY_TYPE));
        assert!(owning_topology::is_kind_of(crate::topology::topology::TYPE));
    }

    #[test]
    fn copy_map_requires_entries() {
        let mut m: CopyBrepMap<u32> = CopyBrepMap::new();
        m.set_at(1, 10);
        m.set_at(1, 11);
        assert_eq!(m.get_at(Some(1)).unwrap(), Some(11));
        assert_eq!(m.get_at(None).unwrap(), None);
        assert!(m.get_at(Some(2)).is_err());
        m.set_breps(Some(5), Some(6));
        assert_eq!((m.orig_brep(), m.copy_brep()), (Some(5), Some(6)));
        m.destroy();
        assert_eq!(m.orig_brep(), None);
        assert_eq!(m.len(), 1);
    }

    /// Pool handles: an index and a kind.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(usize, ClassObjectKind);

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            self.1
        }
    }

    struct Geo(Vec<Curve>);

    impl ClassGeometry<H> for Geo {
        fn curve(&self, o: H) -> Option<&Curve> {
            self.0.get(o.0)
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

    impl ProtoGeometry<H> for Geo {
        fn curve_mut(&mut self, o: H) -> Option<&mut Curve> {
            self.0.get_mut(o.0)
        }
    }

    /// The bare stand-ins: no uses, no B-rep; an edge's start vertex dereferences its
    /// missing primary edge use.
    impl ProtoTopology<H> for Geo {
        fn vertex_use_faces(&self, _vertex: H) -> crate::Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_use_faces(&self, _edge: H) -> crate::Result<Vec<Option<H>>> {
            Ok(Vec::new())
        }
        fn edge_start_vertex(&self, _edge: H) -> crate::Result<Option<H>> {
            Err(SolidError::invalid_input("test", "no primary edge use"))
        }
        fn edge_other_vertex(&self, _edge: H, _vertex: Option<H>) -> crate::Result<Option<H>> {
            Ok(None)
        }
        fn vertex_point(&self, _vertex: H) -> crate::Result<Point3> {
            Ok(Point3::new(0.0, 0.0, 0.0))
        }
        fn edge_brep(&self, _edge: H) -> crate::Result<Option<ObjectKey>> {
            Ok(None)
        }
        fn edge_faces(&self, _edge: H) -> crate::Result<Vec<H>> {
            Ok(Vec::new())
        }
    }

    fn line(p: [f64; 3], q: [f64; 3]) -> Curve {
        Curve::from(
            Line::create_line_segment(3, Vec3::from_array(p), Vec3::from_array(q), None).unwrap(),
        )
    }

    const C0: H = H(0, ClassObjectKind::Curve);
    const R0: H = H(10, ClassObjectKind::Region);
    const L0: H = H(11, ClassObjectKind::Loop);

    fn manager() -> ProtoTopologyManager<H> {
        let b = H(100, ClassObjectKind::Other);
        let e = Brep::new();
        ProtoTopologyManager::new(&Kernel::default(), (b, &e), (b, &e), b, 1e-5, 1e-3, -1.0)
            .unwrap()
    }

    #[test]
    fn manager_tolerance_falls_back_to_the_breps() {
        let k = Kernel::default();
        let b = H(100, ClassObjectKind::Other);
        let e1 = Brep::new();
        let mut e2 = Brep::new();
        e2.set_tolerance(1e-4);
        // no edges: both edge sizes are 0.001, so min(1.25 * tol, 0.0005)
        let expect = (e1.tolerance() + e2.tolerance()) * 1.25;
        for approx in [0.0, -1.0, 1e-13] {
            let m = ProtoTopologyManager::new(&k, (b, &e1), (b, &e2), b, approx, 0.0, 0.5).unwrap();
            assert_eq!(m.tolerance(&k), zone_tol3d(&k, min2(expect, 0.0005)));
        }
        let m = ProtoTopologyManager::new(&k, (b, &e1), (b, &e2), b, EFF_ZERO, 0.0, 0.5).unwrap();
        assert_eq!(m.tolerance(&k), zone_tol3d(&k, EFF_ZERO));
        e2.set_tolerance(1.0);
        let m = ProtoTopologyManager::new(&k, (b, &e1), (b, &e2), b, 0.0, 0.0, 0.5).unwrap();
        assert_eq!(m.tolerance(&k), zone_tol3d(&k, 0.0005));
    }

    #[test]
    fn proto_edge_definitions_evaluate_and_combine() {
        let k = Kernel::default();
        let geo = Geo(vec![line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0])]);
        let mut m = manager();
        let ivl = geo.0[0].natural_interval();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v0);
        m.add_proto_vertex(v1);
        let pe = m.new_proto_edge(&k, Some(v0), Some(v1), ivl).unwrap();
        m.add_proto_edge(pe);
        let ed1 = m
            .insert_edge_definition(EdgeDefinition::new(
                Some(C0),
                ivl,
                R0,
                L0,
                1e-5,
                None,
                None,
                Some(pe),
            ))
            .unwrap();
        let ed2 = m
            .insert_edge_definition(EdgeDefinition::new(
                Some(C0),
                ivl,
                R0,
                L0,
                1e-5,
                None,
                None,
                Some(pe),
            ))
            .unwrap();
        m.proto_edge_mut(pe)
            .unwrap()
            .edge_definitions_mut()
            .extend([ed1, ed2]);

        let (pos, tan) = m.proto_edge_evaluate_mid(&k, &geo, pe, true, true).unwrap();
        assert!((pos.unwrap() - Point3::new(1.0, 0.0, 0.0)).length() < 1e-12);
        assert!(tan.is_some());
        let end = m
            .proto_edge_end_position_at(&k, &geo, pe, Some(v1))
            .unwrap();
        assert!((end - Point3::new(2.0, 0.0, 0.0)).length() < 1e-12);
        assert!(m.proto_edge_end_position_at(&k, &geo, pe, None).is_err());
        // same sources: drop the second
        assert_eq!(
            m.can_combine_edge_definitions(&k, &geo, ed1, ed2).unwrap(),
            2
        );
        assert_eq!(m.all_edge_definitions().unwrap(), vec![ed1, ed2]);
        assert_eq!(m.edge_def_start_proto_vertex(ed1).unwrap(), Some(v0));

        let (inside, t) = m
            .edge_def(ed1)
            .unwrap()
            .is_interior_point(&k, &geo, Point3::new(0.5, 0.0, 0.0), 1e-5, true)
            .unwrap();
        assert!(inside);
        assert!(t > ivl.min() && t < ivl.mid());
    }

    #[test]
    fn proto_vertices_combine_and_remove() {
        let k = Kernel::default();
        let geo = Geo(vec![line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0])]);
        let mut m = manager();
        let ivl = geo.0[0].natural_interval();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 2e-5)
            .unwrap();
        let pe = m.new_proto_edge(&k, Some(v1), Some(v1), ivl).unwrap();
        m.proto_vertex_mut(v1).unwrap().add_proto_edge(pe);
        m.combine_proto_vertex(v0, v1).unwrap();
        let e = m.proto_edge(pe).unwrap();
        assert_eq!(
            (e.start_proto_vertex(), e.end_proto_vertex()),
            (Some(v0), Some(v0))
        );
        assert_eq!(m.proto_vertex(v0).unwrap().proto_edges(), &[pe]);
        assert!((m.proto_vertex(v0).unwrap().tolerance(&k) - 2e-5).abs() < 1e-18);

        // an edgeless proto vertex on the curve is unlisted
        let v2 = m
            .new_proto_vertex(Point3::new(1.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v2);
        let ed = m
            .insert_edge_definition(EdgeDefinition::new(
                Some(C0),
                ivl,
                R0,
                L0,
                1e-5,
                None,
                None,
                Some(pe),
            ))
            .unwrap();
        m.proto_edge_mut(pe)
            .unwrap()
            .edge_definitions_mut()
            .push(ed);
        assert!(m
            .remove_coincident_no_edge_proto_vertex(&k, &geo, pe, v2)
            .unwrap());
        assert!(m.proto_vertices().is_empty());
        // off the curve: no solution, and upstream's SM_SUCCESS reads as TRUE
        let v3 = m
            .new_proto_vertex(Point3::new(1.0, 5.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v3);
        assert!(m
            .remove_coincident_no_edge_proto_vertex(&k, &geo, pe, v3)
            .unwrap());
        assert_eq!(m.proto_vertices(), &[v3]);
    }

    /// source issue #848 (WR-73): with the opt-in, a proto vertex the proto
    /// edge does not reach is reported as not removed, so the caller goes on testing it
    /// against the remaining proto edges.
    #[test]
    fn issue_848_no_hit_is_false() {
        let mut k = Kernel::default();
        k.fixes.proto_vertex_miss_is_false = true;
        let geo = Geo(vec![line([0.0, 0.0, 0.0], [2.0, 0.0, 0.0])]);
        let mut m = manager();
        let ivl = geo.0[0].natural_interval();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(2.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let pe = m.new_proto_edge(&k, Some(v0), Some(v1), ivl).unwrap();
        let ed = m
            .insert_edge_definition(EdgeDefinition::new(
                Some(C0),
                ivl,
                R0,
                L0,
                1e-5,
                None,
                None,
                Some(pe),
            ))
            .unwrap();
        m.proto_edge_mut(pe)
            .unwrap()
            .edge_definitions_mut()
            .push(ed);
        // off the curve: not removed, and reported so
        let off = m
            .new_proto_vertex(Point3::new(1.0, 5.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(off);
        assert!(!m
            .remove_coincident_no_edge_proto_vertex(&k, &geo, pe, off)
            .unwrap());
        assert_eq!(m.proto_vertices(), &[off]);
        // on the curve: still removed
        let on = m
            .new_proto_vertex(Point3::new(1.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(on);
        assert!(m
            .remove_coincident_no_edge_proto_vertex(&k, &geo, pe, on)
            .unwrap());
        assert_eq!(m.proto_vertices(), &[off]);
        // the default keeps upstream's TRUE
        k.fixes.proto_vertex_miss_is_false = false;
        assert!(m
            .remove_coincident_no_edge_proto_vertex(&k, &geo, pe, off)
            .unwrap());
    }

    #[test]
    fn manager_destroy_keeps_unlisted_objects() {
        let k = Kernel::default();
        let mut m = manager();
        let v0 = m
            .new_proto_vertex(Point3::new(0.0, 0.0, 0.0), 1e-5)
            .unwrap();
        let v1 = m
            .new_proto_vertex(Point3::new(1.0, 0.0, 0.0), 1e-5)
            .unwrap();
        m.add_proto_vertex(v0);
        let pe = m
            .new_proto_edge(&k, Some(v1), Some(v0), Extent1d::from_min_max(0.0, 1.0))
            .unwrap();
        m.proto_vertex_mut(v0).unwrap().add_proto_edge(pe);
        let deleted = m.destroy();
        assert!(deleted.is_empty());
        assert!(m.proto_vertex(v0).is_none());
        assert!(m.proto_vertex(v1).is_some());
        // v1.0.1's ~SmProtoTopologyManager (3fd5b95) empties the listed proto vertices'
        // edge lists before deleting them, so the deleted vertex no longer clears itself
        // from the unlisted proto edge: its ends stay as they were
        assert_eq!(m.proto_edge(pe).unwrap().end_proto_vertex(), Some(v0));
        assert_eq!(m.proto_edge(pe).unwrap().start_proto_vertex(), Some(v1));
    }

    #[test]
    fn static_helpers() {
        assert!(proto::can_imprint(PointClassType::Edge));
        assert!(!proto::can_imprint(PointClassType::Curve));
        assert_eq!(proto::topo_dimension(R0), -1);
        assert_eq!(proto::topo_dimension(H(1, ClassObjectKind::Face)), 2);
        let bare = Geo(Vec::new());
        assert!(proto::are_connected(&bare, Some(R0), Some(R0)).unwrap());
        assert!(!proto::are_connected(&bare, Some(R0), Some(L0)).unwrap());
        assert!(proto::are_connected(
            &bare,
            Some(H(1, ClassObjectKind::Edge)),
            Some(H(2, ClassObjectKind::Vertex))
        )
        .is_err());
        let (e1, f1) = (H(1, ClassObjectKind::Edge), H(3, ClassObjectKind::Face));
        assert!(!proto::are_connected(&bare, Some(f1), Some(e1)).unwrap());
        assert!(
            !proto::are_connected(&bare, Some(H(2, ClassObjectKind::Vertex)), Some(f1)).unwrap()
        );
        assert_eq!(
            proto::find_ed_on_degen_face(&bare, e1, H(4, ClassObjectKind::Edge)).unwrap(),
            0
        );

        let k = Kernel::default();
        let geo = Geo(vec![
            line([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            line([0.0, 1e-9, 0.0], [1.0, 1e-9, 0.0]),
        ]);
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        let c1 = H(1, ClassObjectKind::Curve);
        assert!(proto::are_same_curve(&k, &geo, Some(C0), &ivl, Some(c1), &ivl, 1e-6).unwrap());
        assert!(!proto::are_same_curve(&k, &geo, Some(C0), &ivl, Some(c1), &ivl, 1e-12).unwrap());
        let dom = geo.0[0].natural_interval();
        assert!(proto::is_interior(&k, &geo, Some(C0), &dom, dom.mid(), 1e-6).unwrap());
        assert!(!proto::is_interior(&k, &geo, Some(C0), &dom, dom.min(), 1e-6).unwrap());
    }

    #[test]
    fn api_transforms() {
        let mut k = Kernel::default();
        let mut c = line([1.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        api_transform::api_translate(
            &mut k,
            Some(TransformObject::Curve(&mut c)),
            Vec3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        let ivl = c.natural_interval();
        let p = c.evaluate_point(&k, ivl.min()).unwrap();
        assert!((p - Point3::new(1.0, 1.0, 0.0)).length() < 1e-12);
        api_transform::api_scale_by_pt(
            &mut k,
            Some(TransformObject::Curve(&mut c)),
            Point3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 2.0, 2.0),
        )
        .unwrap();
        let ivl = c.natural_interval();
        let q = c.evaluate_point(&k, ivl.max()).unwrap();
        assert!((q - Point3::new(3.0, 1.0, 0.0)).length() < 1e-12);

        assert!(matches!(
            api_transform::api_rotate(
                &mut k,
                Some(TransformObject::Curve(&mut c)),
                Point3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                90.0
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            api_transform::api_scale(&mut k, None, Vec3::new(1.0, 1.0, 1.0)),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            api_transform::api_scale(
                &mut k,
                Some(TransformObject::Other),
                Vec3::new(0.0, 1.0, 1.0)
            ),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(api_transform::api_translate(
            &mut k,
            Some(TransformObject::Brep(&mut Brep::new())),
            Vec3::new(1.0, 0.0, 0.0)
        )
        .is_ok());
    }

    /// The B-rep walks on a wire edge from (0,0,0) to (1,0,0) of one B-rep.
    #[test]
    fn brep_walks() {
        use crate::error::EntityRef;
        use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
        use crate::topology::{
            Brep, Edge, Edgeuse, EdgeuseId, EdgeuseOwner, Region, Shell, Vertex, VertexId,
            Vertexuse, VertexuseId, VertexuseOwner,
        };

        let mut b = Brep::new();
        let r = b
            .add_region_record(Region {
                in_brep: true,
                ..Region::default()
            })
            .unwrap();
        let s = b
            .add_shell_record(Shell {
                faces: Vec::new(),
                region: Some(r),
                ..Shell::default()
            })
            .unwrap();
        let v: Vec<VertexId> = (0..3)
            .map(|i| {
                b.add_vertex_record(Vertex {
                    point: Vec3::new(f64::from(i), 0.0, 0.0),
                    tolerance: 1e-5,
                    vertexuses: Vec::new(),
                    poly_vertex: None,
                })
                .unwrap()
            })
            .collect();
        let mut edge = Edge::new([v[0], v[1]]);
        edge.edgeuses = vec![EdgeuseId::from_index(0), EdgeuseId::from_index(1)];
        let e = b.add_edge_record(edge).unwrap();
        let eus: Vec<EdgeuseId> = (0..2u32)
            .map(|i| {
                b.add_edgeuse_record(Edgeuse {
                    edge: Some(e),
                    owner: EdgeuseOwner::Shell(Some(s)),
                    vertexuse: Some(VertexuseId::from_index(i)),
                    orientation: if i == 0 {
                        OrientType::Same
                    } else {
                        OrientType::Opposite
                    },
                    ..Edgeuse::default()
                })
                .unwrap()
            })
            .collect();
        for (vi, eu) in v.iter().zip(&eus) {
            b.add_vertexuse_record(Vertexuse {
                vertex: Some(*vi),
                owner: VertexuseOwner::Edgeuse(Some(*eu)),
                ..Vertexuse::default()
            })
            .unwrap();
        }
        // the vertex records list their uses
        let (ve, v0, v1, v2) = (
            EntityRef::Edge(e),
            EntityRef::Vertex(v[0]),
            EntityRef::Vertex(v[1]),
            EntityRef::Vertex(v[2]),
        );
        assert_eq!(b.edge_start_vertex(ve).unwrap(), Some(v0));
        assert_eq!(b.edge_other_vertex(ve, Some(v0)).unwrap(), Some(v1));
        assert_eq!(b.vertex_point(v1).unwrap(), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(b.edge_use_faces(ve).unwrap(), vec![None, None]);
        assert!(b.edge_faces(ve).unwrap().is_empty());
        assert!(proto::are_connected(&b, Some(v0), Some(ve)).unwrap());
        assert!(proto::are_connected(&b, Some(ve), Some(v1)).unwrap());
        assert!(!proto::are_connected(&b, Some(v2), Some(ve)).unwrap());
        assert_eq!(proto::find_ed_on_degen_face(&b, ve, ve).unwrap(), 0);
        assert!(b.vertex_point(ve).is_err());
    }
}
