//! Control-flow tests for executive p3. Dependency responses are deliberately
//! scripted here; these tests do not establish C++ geometric parity.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::boolean::topology_intersector::p1::{IntersectionResults, IntersectionTable};
use crate::geom::solution_array::ObjectKind;
use crate::intersect::point_class::ClassObjectKind;
use std::cell::Cell;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Handle(u8);
impl ClassObject for Handle {
    fn class_kind(&self) -> ClassObjectKind {
        if (10..=13).contains(&self.0) {
            ClassObjectKind::Edge
        } else {
            ClassObjectKind::Other
        }
    }
}
impl SolutionObject for Handle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Other
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // independent scripted callback scenarios
struct Access {
    process_error: Option<SolidError>,
    mapped_edges: bool,
    corner_enables_global: bool,
    degenerate_edge: Option<Handle>,
    insert_live_state: bool,
    geoms: Vec<Handle>,
    append_geom: bool,
    geom_snapshots: Cell<usize>,
    geom_reads: Cell<usize>,
    created_attributes: usize,
    attributes: BTreeMap<Handle, Vec<Handle>>,
    removed_edges: Vec<Option<Handle>>,
    marked_pairs: Vec<(Handle, Handle)>,
    orient_error: Option<SolidError>,
    merge_error: Option<SolidError>,

    piecewise_error: Option<SolidError>,
    stitch_error: bool,
    editing: bool,
    fillet_editing: bool,
    events: Vec<&'static str>,
    stitch_tolerances: Vec<f64>,
    solver_tolerance: Option<f64>,
    brep_tolerances: Vec<f64>,
    stitch_lamina: u64,
    corner_metadata: Option<CornerView<Handle>>,
    corner_predicates: [bool; 2],
    corner_point: Point3,
    constant_results: Vec<bool>,
    corner_fail: Option<&'static str>,
    /// A hand-built target body: `edges(Handle(0))` lists its edges as
    /// `Handle(BODY_EDGE + i)` and the manifold check runs on its records.
    body: Option<Brep>,
}
const BODY_EDGE: u8 = 100;
fn absent<T>() -> Result<T> {
    Err(invalid("test handle has no geometry"))
}
impl ExecutiveAccess<Handle> for Access {
    fn solver_base(&self, _: Handle) -> Result<&crate::blend::fillet_solver::p1::FilletSolver> {
        Err(SolidError::invalid_input(
            "test::executive_access",
            "no retained solver",
        ))
    }
    fn solver_base_mut(
        &mut self,
        _: Handle,
    ) -> Result<&mut crate::blend::fillet_solver::p1::FilletSolver> {
        Err(SolidError::invalid_input(
            "test::executive_access",
            "no retained solver",
        ))
    }
    fn solver_has_surface(&self, _: Handle, _: usize) -> Result<bool> {
        Err(SolidError::invalid_input(
            "test::executive_access",
            "no retained solver",
        ))
    }
    fn edge_g1_knots(&mut self, _: &Kernel, _: Handle) -> Result<Vec<f64>> {
        Err(SolidError::invalid_input(
            "test::executive_access",
            "no retained solver",
        ))
    }
    fn edgeuse_vertex(&self, _: Handle) -> Result<Option<Handle>> {
        Err(SolidError::invalid_input(
            "test::executive_access",
            "no retained solver",
        ))
    }
    fn reinitialize_corner_edges(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        absent()
    }
    fn corner_tangent_solver(&self, _: Handle, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn make_corner_topology(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("corner_topology");
        if self.corner_enables_global {
            executive.options.global_merge = true;
        }
        Ok(())
    }
    fn corner_error_info(
        &self,
        _: Handle,
        _: &FilletExecutive<Handle>,
    ) -> Result<CornerView<Handle>> {
        self.corner_metadata
            .clone()
            .ok_or_else(|| invalid("no corner metadata"))
    }
    fn is_blending_corner(&self, _: Handle) -> Result<bool> {
        Ok(self.corner_predicates[0])
    }
    fn is_tangent_corner(&self, _: Handle) -> Result<bool> {
        assert!(self.corner_predicates[0]);
        Ok(self.corner_predicates[1])
    }
    fn corner_solvers(&self, _: Handle, _: &FilletExecutive<Handle>) -> Result<Vec<Handle>> {
        assert!(self.corner_predicates[0] && self.corner_predicates[1]);
        // Script the upstream repeated-first-edgeuse lookup, not distinct solvers.
        Ok(vec![Handle(3), Handle(3)])
    }
    fn corner_vertex(&self, _: Handle) -> Result<Option<Handle>> {
        Ok(Some(Handle(6)))
    }
    fn corner_approx_tolerance(&self, _: Handle) -> Result<f64> {
        Ok(0.125)
    }
    fn solver_geoms(&self, _: Handle) -> Result<Vec<Handle>> {
        self.geom_snapshots.set(self.geom_snapshots.get() + 1);
        Ok(self.geoms.clone())
    }
    fn solver_geom(&self, _: Handle, index: usize) -> Result<Option<Handle>> {
        self.geom_reads.set(self.geom_reads.get() + 1);
        Ok(self.geoms.get(index).copied())
    }
    fn solver_edge(&self, _: Handle) -> Result<Handle> {
        Ok(Handle(5))
    }
    fn solver_approx_tolerance(&self, _: Handle) -> Result<f64> {
        Ok(self.solver_tolerance.unwrap_or(0.125))
    }
    fn geom_face(&self, _: Handle) -> Result<Option<Handle>> {
        Ok(Some(Handle(2)))
    }
    fn topo_edge(&self, edge: Handle) -> Result<TopoEdgeView<Handle>> {
        assert_eq!(edge, Handle(20));
        Ok(TopoEdgeView {
            on_edge: true,
            has_curve: true,
            brep_edge: Some(Handle(11)),
        })
    }
    fn vertex_point(&self, _: Handle) -> Result<Point3> {
        Ok(self.corner_point)
    }
    fn edge_curve_ends(&self, _: &Kernel, _: Handle) -> Result<(Point3, Point3)> {
        Ok((Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0)))
    }
    fn set_calc_as_const_rad(&mut self, _: Handle, value: bool) -> Result<()> {
        self.constant_results.push(value);
        Ok(())
    }
    fn new_brep(&mut self, _: &Kernel) -> Result<Handle> {
        self.events.push("new");
        Ok(Handle(1))
    }
    fn delete_brep(&mut self, _: Handle) -> Result<()> {
        self.events.push("delete");
        Ok(())
    }
    fn tolerance(&self, _: &Kernel, _: Handle, _: bool) -> Result<f64> {
        Ok(0.125)
    }
    fn editing_enabled(&self, _: Handle) -> Result<bool> {
        Ok(self.editing)
    }
    fn set_editing_enabled(&mut self, brep: Handle, value: bool) -> Result<()> {
        if brep == Handle(0) {
            self.editing = value;
        } else {
            self.fillet_editing = value;
        }
        Ok(())
    }
    fn edges(&self, brep: Handle) -> Result<Vec<Handle>> {
        if let (Some(body), Handle(0)) = (&self.body, brep) {
            let count = u8::try_from(body.edge_list().len()).unwrap();
            return Ok((0..count).map(|i| Handle(BODY_EDGE + i)).collect());
        }
        Ok(if !self.mapped_edges {
            vec![]
        } else if brep == Handle(0) {
            vec![Handle(10), Handle(12)]
        } else {
            vec![Handle(11), Handle(13)]
        })
    }
    fn edge_is_manifold(&self, edge: Handle) -> Result<bool> {
        match (&self.body, self.body_edge(edge)) {
            (Some(body), Some(id)) => crate::topology::edge::is_manifold(body, id),
            // Scripted edges stand for no body.
            _ => Ok(true),
        }
    }
    fn edge_entity(&self, edge: Handle) -> Option<EntityRef> {
        self.body_edge(edge).map(EntityRef::Edge)
    }
    fn create_corners(
        &mut self,
        _: &mut Kernel,
        _: &mut FilletExecutive<Handle>,
        _: Option<&mut ErrorInfo<Handle>>,
    ) -> Result<()> {
        self.events.push("corners");
        Ok(())
    }
    fn vertices(&self, _: Handle) -> Result<Vec<Handle>> {
        Ok(vec![])
    }
    fn faces(&self, _: Handle) -> Result<Vec<Handle>> {
        Ok(vec![Handle(2)])
    }
    fn add_attribute(&mut self, edge: Handle, attribute: Option<Handle>) -> Result<()> {
        if let Some(attribute) = attribute {
            self.attributes.get_mut(&attribute).unwrap().push(edge);
        }
        Ok(())
    }
    fn new_keep_attribute(&mut self, _: &Kernel) -> Result<Handle> {
        let attribute = Handle(u8::try_from(30 + self.created_attributes).unwrap());
        self.created_attributes += 1;
        self.attributes.insert(attribute, vec![]);
        self.events.push("new_attr");
        Ok(attribute)
    }
    fn attribute_users(&self, attribute: Handle) -> Result<Vec<Handle>> {
        Ok(self.attributes.get(&attribute).unwrap().clone())
    }
    fn delete_attribute(&mut self, attribute: Handle) -> Result<()> {
        self.attributes.remove(&attribute).unwrap();
        self.events.push("delete_attr");
        Ok(())
    }
    fn with_marks<T>(
        &mut self,
        k: &mut Kernel,
        _: Handle,
        _: Handle,
        mates: &[(Handle, Handle)],
        body: impl FnOnce(&mut Self, &mut Kernel, &[ContextMark]) -> Result<T>,
    ) -> Result<T> {
        self.events.push("marks");
        self.marked_pairs = mates.to_vec();
        body(self, k, &[])
    }
    fn shrink_face(&mut self, _: &mut Kernel, _: Handle) -> Result<()> {
        absent()
    }
    fn notify_face(
        &mut self,
        _: &mut Kernel,
        face: Handle,
        operation: NotifyOperation,
        data1: Option<Handle>,
        data2: Option<Handle>,
        data3: Option<Handle>,
    ) -> Result<()> {
        assert_eq!(operation, NotifyOperation::PreEdit);
        assert_eq!(data1, Some(face));
        assert!(data2.is_none());
        assert!(data3.is_none());
        self.events.push("notify");
        Ok(())
    }
    fn boolean_post_process(
        &mut self,
        _: &mut Kernel,
        _: &mut FilletExecutive<Handle>,
        all: bool,
    ) -> Result<()> {
        assert!(all);
        self.events.push("post");
        Ok(())
    }
    fn process_smooth_edge_sequences(
        &mut self,
        k: &mut Kernel,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("smooth");
        // DoFilleting sets this flag before invoking CreateFilletBrep.
        let _ = k.context.doing_boolean();
        self.process_error.clone().map_or(Ok(()), Err)
    }
    fn set_brep_tolerance(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        tol: f64,
        larger: bool,
        cascade: bool,
    ) -> Result<()> {
        self.brep_tolerances.push(tol);
        assert!(larger && cascade);
        self.events.push("tolerance");
        Ok(())
    }
    fn stitch_faces(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: f64,
        faces: Option<&[Handle]>,
        spine: bool,
    ) -> Result<StitchAttempt> {
        assert!(faces.is_none());
        assert!(!spine);
        self.events.push("stitch_faces");
        Ok(StitchAttempt {
            status: None,
            stitched: 0,
            lamina: 0,
            max_vertex_gap: 0.0,
            max_edge_gap: 0.0,
        })
    }
    fn piecewise_merge(&mut self, k: &mut Kernel, _: &mut FilletExecutive<Handle>) -> Result<()> {
        assert!(k.context.doing_boolean());
        assert!(self.editing);
        self.events.push("piecewise");
        self.piecewise_error.clone().map_or(Ok(()), Err)
    }
    fn set_fillet_brep_edge_map(
        &mut self,
        _: &mut Kernel,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("edge_map");
        Ok(())
    }
    fn delete_faces(
        &mut self,
        _: &mut Kernel,
        _: &mut TopologyIntersector<Handle>,
        _: &[Handle],
        keep: Option<&[Handle]>,
    ) -> Result<()> {
        assert!(keep.is_none());
        self.events.push("delete_faces");
        Ok(())
    }
    fn merge_brep(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: Handle,
        copy: Option<&mut CopyBrepMap<Handle>>,
        reverse: Option<&mut CopyBrepMap<Handle>>,
        analytics: bool,
    ) -> Result<()> {
        assert!(copy.is_none() && reverse.is_none() && analytics);
        self.events.push("merge");
        self.merge_error.clone().map_or(Ok(()), Err)
    }
    fn do_stitching(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        tolerance: f64,
        vertices: Option<&[Handle]>,
        edges: Option<&[Handle]>,
        settings: StitchSettings,
        min_vertex: Option<&mut f64>,
        min_edge: Option<&mut f64>,
    ) -> Result<StitchAttempt> {
        assert!(
            vertices.is_none() && edges.is_none() && min_vertex.is_none() && min_edge.is_none()
        );
        assert!(settings.do_region_nesting && settings.ignore_problems && settings.validate_result);
        assert!(settings.split_edges_with_vertices && settings.making_manifold_solid);
        assert!(
            !settings.fast_edge_compare
                && !settings.squeeze_small_edges
                && !settings.remove_laminar_slivers
        );
        self.stitch_tolerances.push(tolerance);
        Ok(StitchAttempt {
            status: self.stitch_error.then(failure),
            stitched: 0,
            lamina: self.stitch_lamina,
            max_vertex_gap: 0.0,
            max_edge_gap: 0.0,
        })
    }
    fn remove_topo_edges(
        &mut self,
        _: &mut Kernel,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.removed_edges = executive.topo_edges.clone();
        self.events.push("remove_topo");
        Ok(())
    }
    fn orient_trimmed_surfaces(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        outward: bool,
        solid: &mut bool,
        unify: bool,
        faces: Option<&mut Vec<Handle>>,
    ) -> Result<()> {
        assert!(outward && !*solid && !unify && faces.is_none());
        self.events.push("orient");
        if self.created_attributes > 0 {
            assert!(
                !self.attributes.is_empty(),
                "orientation must see temporary attributes"
            );
        }
        self.orient_error.clone().map_or(Ok(()), Err) // ordinary errors are ignored
    }
    fn calc_corner_vertices(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("corner_vertices");
        if self.corner_fail == Some("vertices") {
            Err(failure())
        } else {
            Ok(())
        }
    }
    fn check_degeneracy(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<bool> {
        self.events.push("degeneracy");
        if let Some(edge) = self.degenerate_edge {
            executive
                .topo_edges
                .retain(|candidate| *candidate != Some(edge));
            return Ok(true);
        }
        Ok(false)
    }
    fn adjust_topology(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("adjust");
        Ok(())
    }
    fn adjust_tangent_corner(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("adjust_tangent");
        Ok(())
    }
    fn calc_corner_edges(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("corner_edges");
        Ok(())
    }
    fn calc_fillet_geom(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        executive.error.code = 6;
        Ok(())
    }
    fn test_rollover(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut Vec<Handle>,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        absent()
    }
    fn trim_rails(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        Ok(())
    }
    fn make_face_brep(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        assert!(executive.fillet.is_some());
        self.events.push("make_face");
        if self.append_geom {
            self.append_geom = false;
            self.geoms.push(Handle(9));
        }
        Ok(())
    }
    fn intersect_insert_relate(
        &mut self,
        _: &mut Kernel,
        _: &mut TopologyIntersector<Handle>,
    ) -> Result<()> {
        self.events.push("global");
        Ok(())
    }
    fn insert_geom_topology(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        if self.insert_live_state {
            executive
                .intersector
                .seed_mates_for_test(&[(Handle(10), Handle(11))]);
            let mut table = IntersectionTable::default();
            table.brep_surfaces.push(Some(Handle(8)));
            table.other_surfaces.push(Some(Handle(9)));
            table.results.push(IntersectionResults::default());
            executive.intersector.set_intersection_table(Some(table));
        }
        Ok(())
    }
    fn insert_corner_topology(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        if self.insert_live_state {
            assert_eq!(
                executive
                    .intersector
                    .intersection_table()
                    .unwrap()
                    .brep_surfaces,
                [Some(Handle(8))]
            );
            executive
                .intersector
                .seed_mates_for_test(&[(Handle(12), Handle(13))]);
        }
        Ok(())
    }
    fn trim_original_surfaces(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        kept: &mut Vec<Handle>,
        _: &mut Vec<Handle>,
        _: &[ContextMark],
        executive: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        if self.insert_live_state {
            assert_eq!(
                executive.intersector.other_mate(Some(Handle(12))),
                Some(Handle(13))
            );
        }
        kept.push(Handle(2));
        Ok(())
    }
    fn calc_corner_geom(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &mut FilletExecutive<Handle>,
    ) -> Result<()> {
        self.events.push("corner_geom");
        Ok(())
    }
    fn geom_has_surface(&self, _: Handle) -> Result<bool> {
        absent()
    }
    fn geom_rails(&self, _: Handle) -> Result<Vec<Option<Handle>>> {
        absent()
    }
    fn geom_side_edgeuses(&self, _: Handle) -> Result<Vec<Handle>> {
        absent()
    }
    fn edge_edgeuses(&self, _: Handle) -> Result<Vec<Handle>> {
        absent()
    }
    fn fillet_edge_note(
        &self,
        _: Handle,
    ) -> Result<(Option<&'static str>, FilletEdgeType, FilStatus)> {
        absent()
    }
    fn new_executive_objects(&mut self, _: &mut Kernel) -> Result<ExecutiveObjects<Handle>> {
        absent()
    }
    fn edgeuse_edge(&self, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn fillet_edge_original_face(&self, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn fillet_edge_original_edge(&self, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn find_fillet_brep_edge(
        &mut self,
        _: &mut Kernel,
        _: Handle,
        _: &[Handle],
        _: bool,
    ) -> Result<()> {
        absent()
    }
    fn corner_edges(&self, _: Handle) -> Result<Vec<Option<Handle>>> {
        absent()
    }
    fn blend_edgeuse(&mut self, _: &mut Kernel, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn edge_tolerance(&self, _: Handle) -> Result<f64> {
        absent()
    }
    fn edge_vertices(&self, _: Handle) -> Result<(Option<Handle>, Option<Handle>)> {
        absent()
    }
    fn variable_radius_solver(
        &self,
        _: Handle,
    ) -> Result<Option<&crate::blend::fillet_standard_solver::p1::VariableRadiusFs>> {
        absent()
    }
    fn variable_radius_solver_mut(
        &mut self,
        _: Handle,
    ) -> Result<Option<&mut crate::blend::fillet_standard_solver::p1::VariableRadiusFs>> {
        absent()
    }
    fn solver_edgeuse(&self, _: Handle, _: usize) -> Result<Option<Handle>> {
        absent()
    }
    fn solver_corner(&self, _: Handle, _: usize) -> Result<Option<Handle>> {
        absent()
    }
    fn solver_other_corner(&self, _: Handle, _: Handle) -> Result<Option<Handle>> {
        absent()
    }
    fn corner_setback_distance(&self, _: Handle) -> Result<f64> {
        absent()
    }
    fn new_surface_generator(
        &mut self,
        _: &mut Kernel,
        _: FilletSurfaceGenerator,
    ) -> Result<Handle> {
        absent()
    }
    fn new_constant_radius_solver(
        &mut self,
        _: &mut Kernel,
        _: f64,
        _: f64,
        _: f64,
        _: f64,
        _: Handle,
    ) -> Result<Handle> {
        absent()
    }
    fn set_surface_generator(&mut self, _: Handle, _: Handle) -> Result<()> {
        absent()
    }
    fn new_fillet_solver(
        &mut self,
        _: &mut Kernel,
        _: &crate::blend::fillet_executive::p2::SolverSpec<'_, Handle>,
    ) -> Result<Handle> {
        absent()
    }
}

fn state() -> FilletExecutive<Handle> {
    FilletExecutive {
        target: Some(Handle(0)),
        fillet: None,
        corners: vec![],
        solvers: vec![],
        topo_edges: vec![],
        edge_attribute: None,
        initialized: true,
        options: ExecutionOptions {
            piecewise_merge: false,
            topology_insertion: false,
            global_merge: false,
            trimming: false,
            classification: false,
        },
        error: ErrorInfo::default(),
        intersector: TopologyIntersector::new(None, None, 1e-5, 0.5),
        intersection_edges: vec![],
        intersection_vertices: vec![],
        law_sequence_edges: vec![],
        pseudo_brep: None,
        surface_generators: vec![],
        filleted_vertices: Vec::new(),
        vertex_corner_map: BTreeMap::new(),
        edgeuse_solver_map: BTreeMap::new(),
        self_intersection_handler: None,
        blend_curve_at_thirds: false,
        mark_corner_patches: false,
    }
}

#[test]
fn uninitialized_changes_only_generic_error_members() {
    let mut executive = state();
    executive.initialized = false;
    executive.error.status = 9;
    let mut access = Access::default();
    let mut k = Kernel::default();
    assert!(matches!(
        executive.do_fillet(&mut k, &mut access),
        Err(SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })
    ));
    assert_eq!(executive.error.code, 1);
    assert_eq!(executive.error.status, 9);
    assert_eq!(
        executive.error.comment,
        "ERROR: fillet parameters not set up."
    );
    assert!(access.events.is_empty());
    assert!(!executive.intersector.doing_fillets);
}

#[test]
fn null_create_status_is_remapped_by_do_filleting_and_boolean_restores() {
    let mut executive = state();
    executive.target = None;
    let mut access = Access::default();
    let mut k = Kernel::default();
    assert!(matches!(
        executive.create_fillet_brep(&mut k, &mut access, None),
        Err(SolidError::Upstream { code: 1006, .. })
    ));
    for prior in [false, true] {
        k.context.set_doing_boolean(prior);
        assert!(matches!(
            executive.do_filleting(&mut k, &mut access, None),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        assert_eq!(k.context.doing_boolean(), prior);
    }
    assert!(executive.intersector.doing_fillets);
}

#[test]
fn ordinary_smooth_error_is_ignored_and_tail_and_selective_assignment_run() {
    let mut executive = state();
    executive.error.code = 6;
    let mut output = ErrorInfo {
        code: 5,
        comment: "keep comment".into(),
        side_face: Some(Handle(7)),
        ..ErrorInfo::default()
    };
    let mut access = Access {
        process_error: Some(failure()),
        ..Access::default()
    };
    executive
        .create_fillet_brep(&mut Kernel::default(), &mut access, Some(&mut output))
        .unwrap();
    assert_eq!(
        access.events,
        ["smooth", "new", "tolerance", "stitch_faces"]
    );
    assert!(access.fillet_editing);
    assert_eq!(executive.fillet, Some(Handle(1)));
    assert_eq!(output.code, 6);
    assert_eq!(output.comment, "keep comment");
    assert_eq!(output.side_face, Some(Handle(7)));
}

#[test]
fn deferral_is_never_swallowed_or_remapped_and_restores_boolean() {
    let expected = SolidError::unported(
        "SmFilletExecutive::ProcessSmoothEdgeSequences",
        "blend.fillet_executive_p2",
    );
    let mut access = Access {
        process_error: Some(expected.clone()),
        ..Access::default()
    };
    let mut executive = state();
    let mut k = Kernel::default();
    assert_eq!(
        executive.do_filleting(&mut k, &mut access, None),
        Err(expected)
    );
    assert_eq!(access.events, ["smooth"]);
    assert!(!k.context.doing_boolean());
    assert!(executive.intersector.doing_fillets);
    assert!(executive.fillet.is_none());
}

#[test]
fn piecewise_return_restores_both_temporary_flags_on_success_and_error() {
    for error in [
        None,
        Some(failure()),
        Some(SolidError::unported(
            "SmFilletExecutive::DoPiecewiseMerge",
            "blend.fillet_executive_p1",
        )),
    ] {
        let mut executive = state();
        executive.options.piecewise_merge = true;
        let mut access = Access {
            piecewise_error: error.clone(),
            ..Access::default()
        };
        let mut k = Kernel::default();
        assert_eq!(
            executive.do_filleting(&mut k, &mut access, None),
            error.map_or(Ok(()), Err)
        );
        assert!(!access.editing);
        assert!(!k.context.doing_boolean());
        assert_eq!(access.events.last(), Some(&"piecewise"));
        assert!(!access.events.contains(&"edge_map"));
    }
}

#[test]
fn stitching_retries_exactly_five_times_doubles_tolerance_and_ignores_orientation_error() {
    let mut executive = state();
    executive.options.trimming = true;
    let mut access = Access {
        stitch_lamina: 1,
        orient_error: Some(failure()),
        ..Access::default()
    };
    let mut k = Kernel::default();
    executive.do_filleting(&mut k, &mut access, None).unwrap();
    assert_eq!(access.stitch_tolerances, [0.125, 0.25, 0.5, 1.0, 2.0]);
    assert_eq!(access.events.last(), Some(&"orient"));
    assert!(access.events.contains(&"post"));
    assert!(executive.fillet.is_none());
    assert!(executive.intersector.other_brep().is_none());
    assert!(!access.editing);
}

#[test]
fn final_stitch_failure_sets_bad_merge_and_stops_before_cleanup_postprocess() {
    let mut executive = state();
    executive.options.trimming = true;
    let mut access = Access {
        stitch_error: true,
        ..Access::default()
    };
    let mut k = Kernel::default();
    assert!(matches!(
        executive.do_filleting(&mut k, &mut access, None),
        Err(SolidError::Upstream { code: 1001, .. })
    ));
    assert_eq!(access.stitch_tolerances.len(), 5);
    assert_eq!(executive.error.code, 7);
    assert!(!access.events.contains(&"post"));
    assert!(!access.editing);
    assert!(!k.context.doing_boolean());
}

fn corner() -> CornerView<Handle> {
    CornerView {
        vertex: Some(Handle(6)),
        corner_type: FilletCornerType::NByTwo,
        name: "corner name".into(),
    }
}

#[test]
fn tangent_corner_constant_radius_uses_both_distances_and_twice_approx_tolerance() {
    for (point, expected) in [
        (0.0, true),
        (0.25, true),
        (0.2500000001, false),
        (5.0, false),
        (9.75, true),
        (10.0, true),
    ] {
        let mut executive = state();
        executive.corners.push(Handle(7));
        let mut access = Access {
            corner_metadata: Some(corner()),
            corner_predicates: [true, true],
            corner_point: Point3::new(point, 0.0, 0.0),
            ..Access::default()
        };
        executive
            .create_fillet_brep(&mut Kernel::default(), &mut access, None)
            .unwrap();
        assert_eq!(access.constant_results, [expected]);
        assert_eq!(
            access.events,
            [
                "smooth",
                "corner_topology",
                "corner_vertices",
                "degeneracy",
                "adjust",
                "adjust_tangent",
                "corner_vertices",
                "corner_edges",
                "new",
                "tolerance",
                "corner_geom",
                "stitch_faces"
            ]
        );
    }
}

#[test]
fn corner_failure_records_context_and_stops_without_allocating_shell() {
    let mut executive = state();
    executive.corners.push(Handle(7));
    let mut access = Access {
        corner_metadata: Some(corner()),
        corner_fail: Some("vertices"),
        ..Access::default()
    };
    assert!(matches!(
        executive.create_fillet_brep(&mut Kernel::default(), &mut access, None),
        Err(SolidError::Upstream { code: 1001, .. })
    ));
    assert_eq!(executive.error.code, 3);
    assert_eq!(executive.error.status, 2);
    assert_eq!(executive.error.corner_type, FilletCornerType::NByTwo);
    assert_eq!(executive.error.vertex, Some(Handle(6)));
    assert_eq!(executive.error.surfaces, [None]);
    assert_eq!(executive.error.entity_name, "corner name");
    assert_eq!(
        executive.error.comment,
        "Fillet Corner Error: Unable to Compute Geometry"
    );
    assert!(!access.events.contains(&"new"));
}

#[test]
fn nonblending_or_nontangent_corners_do_not_read_solver_metadata() {
    for predicates in [[false, true], [true, false]] {
        let mut executive = state();
        executive.corners.push(Handle(7));
        let mut access = Access {
            corner_predicates: predicates,
            ..Access::default()
        };
        executive
            .create_fillet_brep(&mut Kernel::default(), &mut access, None)
            .unwrap();
        assert_eq!(access.constant_results, [predicates[0]]);
    }
}

#[test]
fn corner_error_reporting_preserves_a_null_filleted_vertex() {
    let mut executive = state();
    executive.corners.push(Handle(7));
    let mut metadata = corner();
    metadata.vertex = None;
    let mut access = Access {
        corner_metadata: Some(metadata),
        corner_fail: Some("vertices"),
        ..Access::default()
    };
    assert!(matches!(
        executive.create_fillet_brep(&mut Kernel::default(), &mut access, None),
        Err(SolidError::Upstream { code: 1001, .. })
    ));
    assert_eq!(executive.error.code, 3);
    assert!(executive.error.vertex.is_none());
}

#[test]
fn disabled_mapping_experiments_do_not_add_candidates_or_intersections() {
    for trimming in [false, true] {
        let mut executive = state();
        executive.options.trimming = trimming;
        executive.intersection_edges = vec![Some(Handle(12))];
        executive.intersection_vertices = vec![Some(Handle(6))];
        executive
            .intersector
            .seed_mates_for_test(&[(Handle(10), Handle(11))]);
        let mut access = Access {
            mapped_edges: true,
            ..Access::default()
        };
        executive
            .do_filleting(&mut Kernel::default(), &mut access, None)
            .unwrap();
        assert_eq!(access.created_attributes, 0);
        assert!(access.removed_edges.is_empty());
        assert!(executive.topo_edges.is_empty());
        assert_eq!(executive.intersection_edges, [Some(Handle(12))]);
        assert_eq!(executive.intersection_vertices, [Some(Handle(6))]);
    }
}

#[test]
fn corner_topology_updates_the_global_merge_branch_on_the_live_executive() {
    let mut executive = state();
    executive.corners.push(Handle(7));
    let mut access = Access {
        corner_enables_global: true,
        ..Access::default()
    };
    executive
        .do_filleting(&mut Kernel::default(), &mut access, None)
        .unwrap();
    assert!(executive.options.global_merge);
    assert!(access.events.contains(&"global"));
}

#[test]
fn degeneracy_removes_deleted_candidates_before_the_attribute_scan() {
    let mut executive = state();
    executive.corners.push(Handle(7));
    executive.topo_edges.push(Some(Handle(20)));
    executive.options.trimming = true;
    let mut access = Access {
        mapped_edges: true,
        degenerate_edge: Some(Handle(20)),
        ..Access::default()
    };
    executive
        .do_filleting(&mut Kernel::default(), &mut access, None)
        .unwrap();
    assert_eq!(access.created_attributes, 0);
    assert!(access.removed_edges.is_empty());
}

#[test]
fn insertion_hooks_share_mates_and_existing_ssi_with_subsequent_phases() {
    let mut executive = state();
    executive.solvers.push(Handle(3));
    executive.corners.push(Handle(7));
    executive.options.topology_insertion = true;
    executive.options.trimming = true;
    let mut access = Access {
        mapped_edges: true,
        insert_live_state: true,
        geoms: vec![Handle(8)],
        ..Access::default()
    };
    executive
        .do_filleting(&mut Kernel::default(), &mut access, None)
        .unwrap();
    assert_eq!(
        access.marked_pairs,
        [(Handle(10), Handle(11)), (Handle(12), Handle(13))]
    );
    assert!(executive.intersector.intersection_table().is_some());
    assert_eq!(executive.error.code, 6);
}

#[test]
fn temporary_attributes_survive_orientation_and_are_cleaned_on_errors_or_deferrals() {
    for error in [
        None,
        Some(failure()),
        Some(SolidError::unported(
            "SmBrep::OrientTrimmedSurfaces",
            "topology.brep_p3",
        )),
    ] {
        let mut executive = state();
        executive.options.trimming = true;
        executive.topo_edges.push(Some(Handle(20)));
        let mut access = Access {
            mapped_edges: true,
            orient_error: error.clone(),
            ..Access::default()
        };
        let result = executive.do_filleting(&mut Kernel::default(), &mut access, None);
        assert_eq!(
            result.is_err(),
            error.as_ref().is_some_and(SolidError::is_unported)
        );
        assert_eq!(access.created_attributes, 1);
        assert!(access.attributes.is_empty());
        assert_eq!(access.events.last(), Some(&"delete_attr"));
        assert!(
            access.events.iter().position(|s| *s == "orient").unwrap()
                < access
                    .events
                    .iter()
                    .position(|s| *s == "delete_attr")
                    .unwrap()
        );
    }
}

#[test]
fn face_creation_reads_the_live_geometry_array_without_repeated_snapshots() {
    let mut executive = state();
    executive.solvers.push(Handle(3));
    let mut access = Access {
        geoms: vec![Handle(8)],
        append_geom: true,
        ..Access::default()
    };
    executive
        .create_fillet_brep(&mut Kernel::default(), &mut access, None)
        .unwrap();
    assert_eq!(
        access.events.iter().filter(|s| **s == "make_face").count(),
        2
    );
    assert_eq!(access.geom_reads.get(), 3);
    assert_eq!(access.geom_snapshots.get(), 0);
}

#[test]
fn temporary_attributes_are_also_cleaned_when_merge_fails_or_defers_before_orientation() {
    for error in [
        failure(),
        SolidError::unported("SmBrep::MergeBrep", "topology.brep_p3"),
    ] {
        let mut executive = state();
        executive.options.trimming = true;
        executive.topo_edges.push(Some(Handle(20)));
        let mut access = Access {
            mapped_edges: true,
            merge_error: Some(error.clone()),
            ..Access::default()
        };
        assert_eq!(
            executive.do_filleting(&mut Kernel::default(), &mut access, None),
            Err(error)
        );
        assert_eq!(access.created_attributes, 1);
        assert!(access.attributes.is_empty());
        assert!(!access.events.contains(&"orient"));
        assert_eq!(access.events.last(), Some(&"delete_attr"));
        assert!(!access.editing);
    }
}

#[test]
fn solver_tolerance_sentinel_is_converted_at_set_tolerance_call() {
    let mut executive = state();
    executive.solvers.push(Handle(3));
    let mut access = Access {
        solver_tolerance: Some(crate::geom::tol::USE_DEFAULT),
        ..Access::default()
    };
    let mut k = Kernel::default();
    executive
        .create_fillet_brep(&mut k, &mut access, None)
        .unwrap();
    assert_eq!(
        access.brep_tolerances,
        [0.125, zone_tol3d(&k, crate::geom::tol::USE_DEFAULT)]
    );
}

// ---- closed-manifold result check (not upstream, opt-in; WR-212, source issue #422) ----

use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::topology::{
    Brep, EdgeId, Edgeuse, EdgeuseOwner, Faceuse, LoopEdge, Loopuse, LoopuseStart, ShellId,
    VertexId,
};

impl Access {
    fn body_edge(&self, edge: Handle) -> Option<EdgeId> {
        let index = edge.0.checked_sub(BODY_EDGE)?;
        self.body
            .as_ref()?
            .edge_list()
            .get(usize::from(index))
            .copied()
    }
}

/// Adds triangle `corners` to `body` with the use topology SMLib builds: a mated
/// pair of face uses, one loop use each, and on each side one loop edge use per
/// edge, linked around the loop and appended to the edge's radial list.
fn add_triangle(body: &mut Brep, shell: ShellId, corners: [VertexId; 3]) {
    let mut loop_edges = Vec::new();
    for (start, end) in [
        (corners[0], corners[1]),
        (corners[1], corners[2]),
        (corners[2], corners[0]),
    ] {
        let existing = body.edge_list().iter().copied().find(|&e| {
            let [a, b] = body.edge(e).unwrap().vertices;
            (a, b) == (start, end) || (a, b) == (end, start)
        });
        let edge = existing.unwrap_or_else(|| body.add_edge(start, end).unwrap());
        let forward = body.edge(edge).unwrap().vertices[0] == start;
        loop_edges.push(LoopEdge { edge, forward });
    }
    let face = body.add_face(shell).unwrap();
    let loop_ = body.add_loop(face, loop_edges.clone()).unwrap();
    let up = body
        .add_faceuse_record(Faceuse {
            shell: Some(shell),
            face: Some(face),
            orientation: OrientType::Same,
            ..Faceuse::default()
        })
        .unwrap();
    let down = body
        .add_faceuse_record(Faceuse {
            shell: Some(shell),
            face: Some(face),
            mate: Some(up),
            orientation: OrientType::Opposite,
            ..Faceuse::default()
        })
        .unwrap();
    body.faceuse_mut(up).unwrap().mate = Some(down);
    body.face_mut(face).unwrap().faceuse = Some(up);
    let mut loopuses = Vec::new();
    for (faceuse, reversed) in [(up, false), (down, true)] {
        let loopuse = body
            .add_loopuse_record(Loopuse {
                faceuse: Some(faceuse),
                loop_: Some(loop_),
                orientation: if reversed {
                    OrientType::Opposite
                } else {
                    OrientType::Same
                },
                ..Loopuse::default()
            })
            .unwrap();
        body.faceuse_mut(faceuse).unwrap().loopuses.push(loopuse);
        let mut side = loop_edges.clone();
        if reversed {
            side.reverse();
        }
        let mut uses = Vec::new();
        for LoopEdge { edge, forward } in side {
            let edgeuse = body
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    owner: EdgeuseOwner::Loopuse(Some(loopuse)),
                    orientation: if forward == reversed {
                        OrientType::Opposite
                    } else {
                        OrientType::Same
                    },
                    ..Edgeuse::default()
                })
                .unwrap();
            body.edge_mut(edge).unwrap().edgeuses.push(edgeuse);
            uses.push(edgeuse);
        }
        for (i, &edgeuse) in uses.iter().enumerate() {
            let record = body.edgeuse_mut(edgeuse).unwrap();
            record.ccw = Some(uses[(i + 1) % uses.len()]);
            record.cw = Some(uses[(i + uses.len() - 1) % uses.len()]);
        }
        body.loopuse_mut(loopuse).unwrap().start = LoopuseStart::Edgeuse(Some(uses[0]));
        loopuses.push(loopuse);
    }
    body.loopuse_mut(loopuses[0]).unwrap().mate = Some(loopuses[1]);
    body.loopuse_mut(loopuses[1]).unwrap().mate = Some(loopuses[0]);
    body.loop_mut(loop_).unwrap().loopuse = Some(loopuses[0]);
}

/// A unit tetrahedron with the outward faces listed by `faces` (indices 0..4).
fn tetrahedron(faces: &[usize]) -> (Brep, [VertexId; 4]) {
    let mut body = Brep::new();
    let shell = body.add_shell().unwrap();
    let v = [
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(0.0, 0.0, 1.0),
    ]
    .map(|p| body.add_vertex(p).unwrap());
    let all = [
        [v[0], v[2], v[1]],
        [v[0], v[1], v[3]],
        [v[0], v[3], v[2]],
        [v[1], v[2], v[3]],
    ];
    for &face in faces {
        add_triangle(&mut body, shell, all[face]);
    }
    (body, v)
}

fn edge_between(body: &Brep, a: VertexId, b: VertexId) -> EdgeId {
    body.edge_list()
        .iter()
        .copied()
        .find(|&e| {
            let [s, t] = body.edge(e).unwrap().vertices;
            (s, t) == (a, b) || (s, t) == (b, a)
        })
        .unwrap()
}

fn fillet_body(body: Brep) -> (FilletExecutive<Handle>, Access, Result<()>) {
    let mut executive = state();
    let mut access = Access {
        body: Some(body),
        ..Access::default()
    };
    let mut k = Kernel::default();
    k.fixes.fillet_refuse_non_manifold_result = true;
    let result = executive.do_fillet(&mut k, &mut access);
    (executive, access, result)
}

#[test]
fn open_result_is_accepted_without_the_opt_in_check() {
    // Upstream (the default): DoFillet returns success for an open target.
    let mut executive = state();
    let mut access = Access {
        body: Some(tetrahedron(&[0, 1, 2]).0),
        ..Access::default()
    };
    let mut k = Kernel::default();
    executive.do_fillet(&mut k, &mut access).unwrap();
    assert_eq!(executive.error.code, 0);
    assert!(executive.error.edge.is_none());
}

#[test]
fn hand_built_tetrahedron_is_a_closed_manifold_solid() {
    let (body, _) = tetrahedron(&[0, 1, 2, 3]);
    assert_eq!(body.edge_list().len(), 6);
    for &e in body.edge_list() {
        assert_eq!(body.edge(e).unwrap().edgeuses.len(), 4);
        assert!(crate::topology::edge::is_manifold(&body, e).unwrap());
    }
}

#[test]
fn closed_manifold_result_is_accepted_unchanged() {
    let (executive, access, result) = fillet_body(tetrahedron(&[0, 1, 2, 3]).0);
    result.unwrap();
    assert_eq!(access.events.first(), Some(&"corners"));
    assert_eq!(executive.error.code, 0);
    assert!(executive.error.edge.is_none());
}

#[test]
fn open_result_is_refused_naming_its_first_laminar_edge() {
    // Face 3 is missing: its edges (1,2), (1,3) and (2,3) have one face each, and
    // (1,2) comes first in the edge list.
    let (body, v) = tetrahedron(&[0, 1, 2]);
    let laminar = edge_between(&body, v[1], v[2]);
    let index = body.edge_list().iter().position(|&e| e == laminar).unwrap();
    assert!(body.edge_list()[..index]
        .iter()
        .all(|&e| crate::topology::edge::is_manifold(&body, e).unwrap()));
    let (executive, _, result) = fillet_body(body);
    assert_eq!(
        result,
        Err(SolidError::Upstream {
            operation: OP,
            code: 1001,
            entity: Some(EntityRef::Edge(laminar)),
        })
    );
    assert_eq!(executive.error.code, 7); // SM_FILERR_BAD_MERGE
    assert_eq!(executive.error.status, 2); // SM_FIL_FAILURE
    assert_eq!(
        executive.error.edge,
        Some(Handle(BODY_EDGE + u8::try_from(index).unwrap()))
    );
    assert_eq!(
        executive.error.comment,
        "Fillet Merge Failure: Result Is Not A Closed Manifold Solid"
    );
}

#[test]
fn result_with_a_fin_on_an_edge_is_refused_naming_that_edge() {
    // A closed tetrahedron plus a fifth face on edge (0,1): six edge uses there.
    let (mut body, v) = tetrahedron(&[0, 1, 2, 3]);
    let fin = body.add_vertex(Point3::new(1.0, -1.0, 0.0)).unwrap();
    let shell = body.shells().ids().next().unwrap();
    add_triangle(&mut body, shell, [v[0], v[1], fin]);
    let shared = edge_between(&body, v[0], v[1]);
    assert_eq!(body.edge(shared).unwrap().edgeuses.len(), 6);
    let (executive, _, result) = fillet_body(body);
    assert!(matches!(
        result,
        Err(SolidError::Upstream { code: 1001, entity: Some(EntityRef::Edge(e)), .. })
            if e == shared
    ));
    assert_eq!(executive.error.code, 7);
}

#[test]
fn do_filleting_alone_does_not_check_the_result() {
    let mut executive = state();
    let mut access = Access {
        body: Some(tetrahedron(&[0, 1, 2]).0),
        ..Access::default()
    };
    let mut k = Kernel::default();
    executive.do_filleting(&mut k, &mut access, None).unwrap();
    assert_eq!(executive.error.code, 0);
}

#[test]
fn tag_merge_component_retains_allocated_markers_when_merge_fails() {
    let mut executive = state();
    executive.topo_edges.push(Some(Handle(20)));
    let mut access = Access {
        mapped_edges: true,
        merge_error: Some(failure()),
        ..Access::default()
    };
    let mut attributes = Vec::new();
    let result = executive.tag_and_merge_fillet_faces(
        &mut Kernel::default(),
        &mut access,
        Handle(0),
        Handle(1),
        &mut attributes,
    );
    assert!(result.is_err());
    assert_eq!(attributes, [Handle(30)]);
    assert_eq!(access.attributes.get(&Handle(30)), Some(&vec![Handle(11)]));
    assert_eq!(access.events, ["new_attr", "merge"]);
    assert_eq!(executive.topo_edges, [Some(Handle(20))]);
}
