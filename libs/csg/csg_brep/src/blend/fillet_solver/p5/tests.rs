//! Regression tests for caller continuations behind unavailable rollover calls.
use super::*;
use crate::blend::fillet_geom::p1::CornerLookup;
use crate::blend::fillet_geom::p1::{FilletEdge, FilletEdgeuse, FilletGeom};
use crate::blend::fillet_geom::p3::SplitCorners;
use crate::blend::fillet_solver::p1::FilletSolver;
use crate::curve::curve::p2::OwnerChange;
use crate::curve::line::Line;
use crate::geom::attribute::ObjectKey;
use crate::geom::object::ObjectRef;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::plane::Plane;
use crate::topology::{Edgeuse, EdgeuseOwner, Faceuse, Loopuse, ShellId, Vertexuse};

fn plane() -> Surface {
    Plane::from_normal(Point3::ZERO, Vec3::Z).into()
}
fn rail(k: &Kernel, fb: &mut FilletBrep, ends: &[f64], unknown: bool) -> EdgeId {
    let a = fb.brep_mut().add_vertex(Point3::ZERO).unwrap();
    let b = fb.brep_mut().add_vertex(Vec3::X).unwrap();
    for (v, corner) in [(a, 41), (b, 42)] {
        let mut rec = FilletVertex::new(k, Some(FilletCornerId(corner)));
        rec.set_fillet_vertex_type(FilletVertexType::RailXEdgeuse);
        fb.insert_fillet_vertex(v, rec);
    }
    let e = fb.brep_mut().add_edge(a, b).unwrap();
    fb.brep_mut().edge_mut(e).unwrap().curve =
        Some(Line::from_points(Point3::ZERO, Vec3::X, 3).into());
    let mut eus = Vec::new();
    for v in [a, b] {
        let vu = fb
            .brep_mut()
            .add_vertexuse_record(Vertexuse {
                vertex: Some(v),
                ..Vertexuse::default()
            })
            .unwrap();
        fb.brep_mut().vertex_mut(v).unwrap().vertexuses.push(vu);
        let eu = fb
            .brep_mut()
            .add_edgeuse_record(Edgeuse {
                edge: Some(e),
                vertexuse: Some(vu),
                orientation: if v == a {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                uv_trim_curve: Some(Line::from_points(Point3::ZERO, Vec3::X, 2).into()),
                ..Edgeuse::default()
            })
            .unwrap();
        fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
        eus.push(eu);
    }
    fb.brep_mut().edge_mut(e).unwrap().edgeuses = eus.clone();
    let mut rec = FilletEdge::with_corner(None);
    rec.set_original_face(Some(FaceId::from_index(0)));
    let mut cc = CurveClassification::new(k);
    cc.set_size(k, ends.len()).unwrap();
    let mut prev = 0.0;
    for (i, &end) in ends.iter().enumerate() {
        let ivl = cc.at_mut(i).unwrap();
        ivl.interval.set_min_max(prev, end).unwrap();
        prev = end;
        if !unknown {
            ivl.mid.set_class_object(
                PointClassType::Face,
                Some(EntityRef::Face(FaceId::from_index(0))),
            );
        }
    }
    rec.set_indexed_curve_class(Some(&cc)).unwrap();
    fb.insert_fillet_edge(e, rec);
    e
}
#[derive(Default)]
struct Owners {
    notifications: Vec<OwnerChange>,
}
impl RolloverOwnership for Owners {
    fn edge_owner(&self, edge: EdgeId) -> Result<(ObjectRef, Option<ObjectRef>)> {
        Ok((
            ObjectRef {
                type_code: crate::geom::assert_array::labels::EDGE_TYPE,
                key: ObjectKey(10000 + u64::from(edge.index())),
            },
            None,
        ))
    }
    fn owner_changed(&mut self, _: &mut Curve, change: OwnerChange) -> Result<()> {
        self.notifications.push(change);
        Ok(())
    }
}
struct Fixture {
    k: Kernel,
    solver: FilletSolver,
    fb: FilletBrep,
    target: Brep,
    geom: FilletGeomId,
    side: EdgeId,
    owners: Owners,
    corners: TestCorners,
}

/// The corner store of the fixture: every corner has type `.0` (`None`: never asked).
struct TestCorners(Option<FilletCornerType>);

impl SplitCorners for TestCorners {
    fn corner_of_vertex(&self, _: VertexId) -> Option<FilletCornerId> {
        None
    }
    fn lookup(&self) -> CornerLookup<'_> {
        CornerLookup::default()
    }
    fn corner_mut(
        &mut self,
        _: FilletCornerId,
    ) -> Result<&mut crate::blend::fillet_corner::p3::FilletCorner> {
        Err(SolidError::sm_err("no corner"))
    }
    fn corner_type(&self, _: FilletCornerId) -> Result<FilletCornerType> {
        Ok(self.0.expect("the corner type is not asked"))
    }
}

impl Fixture {
    fn new(ends: [&[f64]; 2], unknown: bool) -> Self {
        let k = Kernel::default();
        let mut solver = FilletSolver::new(1e-5, 1e-3, 0.017);
        let mut target = Brep::new();
        let mut faces = Vec::new();
        for _ in 0..2 {
            let mut f = crate::topology::Face::new(ShellId::from_index(0));
            f.surface = Some(plane());
            faces.push(target.add_face_record(f).unwrap());
        }
        let a = target.add_vertex(Point3::ZERO).unwrap();
        let b = target.add_vertex(Vec3::Y).unwrap();
        let side = target.add_edge(a, b).unwrap();
        for face in faces {
            let fu = target
                .add_faceuse_record(Faceuse {
                    face: Some(face),
                    ..Faceuse::default()
                })
                .unwrap();
            let lu = target
                .add_loopuse_record(Loopuse {
                    faceuse: Some(fu),
                    ..Loopuse::default()
                })
                .unwrap();
            let eu = target
                .add_edgeuse_record(Edgeuse {
                    edge: Some(side),
                    owner: EdgeuseOwner::Loopuse(Some(lu)),
                    ..Edgeuse::default()
                })
                .unwrap();
            target.edge_mut(side).unwrap().edgeuses.push(eu);
        }
        let mut fb = FilletBrep::new();
        let mut geom = FilletGeom::new(&k, None, FilletGeomType::Default).unwrap();
        for (i, e) in ends.iter().enumerate() {
            let r = rail(&k, &mut fb, e, unknown);
            geom.set_rail(i, Some(r));
            solver.surfaces[i] = Some(OffsetSurfaceSlot {
                key: 20 + i as u64,
                surface: OffsetSurface::new(
                    &k,
                    0.25 + i as f64 * 0.25,
                    plane(),
                    SurfaceId::Face(FaceId::from_index(0)),
                    crate::surface::offset_surface::SS_UNKNOWN,
                )
                .unwrap(),
            });
        }
        geom.set_fillet_surface(Some(plane()));
        let geom = fb.add_geom(geom);
        solver.edgeuses[0] = Some(EdgeuseId::from_index(0));
        Self {
            k,
            solver,
            fb,
            target,
            geom,
            side,
            owners: Owners::default(),
            corners: TestCorners(None),
        }
    }
    fn context(&mut self) -> Recovery<'_> {
        Recovery {
            solver: &mut self.solver,
            k: &self.k,
            target: &mut self.target,
            fillets: &mut self.fb,
            corners: &mut self.corners,
            ownership: &mut self.owners,
        }
    }
}

#[derive(Default)]
struct Script {
    calls: Vec<String>,
    side: Option<EdgeId>,
    tangent: bool,
    corner: bool,
    stalled: bool,
    fail: Option<&'static str>,
    find_count: usize,
}
impl Script {
    fn call(&mut self, name: &'static str) -> Result<()> {
        self.calls.push(name.into());
        if self.fail == Some(name) {
            Err(SolidError::sm_err("regression"))
        } else {
            Ok(())
        }
    }
}
impl RecoveryOps for Script {
    fn full(&mut self, c: &mut Recovery<'_>, g: FilletGeomId, which: usize) -> Result<()> {
        self.calls.push(format!("full{which}"));
        c.fillets.geom_mut(g)?.need_split = true;
        if which == 1 && self.fail == Some("full1") {
            return Err(SolidError::sm_err("regression"));
        }
        Ok(())
    }
    fn convex(
        &mut self,
        _: &mut Recovery<'_>,
        _: EdgeuseId,
        samples: u64,
        step_off: bool,
        tolerance: f64,
    ) -> Result<bool> {
        assert_eq!((samples, step_off, tolerance), (10, false, 0.0));
        self.call("convex")?;
        Ok(true)
    }
    fn find_side(&mut self, _: &mut Recovery<'_>, s: SideSearch<'_>) -> Result<SideHit> {
        self.calls.push(format!("find{}", *s.index));
        assert_eq!(s.tangency_tolerance, 0.017);
        assert_eq!(s.classification.size(), 2);
        self.find_count += 1;
        if self.stalled {
            *s.corner_edgeuse = Some(EdgeuseId::from_index(0));
            return Ok(SideHit::default());
        }
        if self.find_count == 1 {
            if self.corner {
                *s.corner_edgeuse = Some(EdgeuseId::from_index(0));
                *s.corner_vertex = Some(VertexId::from_index(1));
            }
            Ok(SideHit {
                edge: self.side,
                tangent: self.tangent,
                trim: false,
                opposite_normals: true,
            })
        } else {
            *s.index = 1;
            *s.corner_edgeuse = None;
            *s.corner_vertex = None;
            Ok(SideHit::default())
        }
    }
    fn topology_split(&mut self, c: &mut Recovery<'_>, g: FilletGeomId) -> Result<FilletGeomId> {
        self.call("split_geom")?;
        let mut geom = c.fillets.geom(g)?.clone();
        for which in 0..2 {
            let original = c.fillets.geom(g)?.rail(which).unwrap();
            let split = c.end(original)?;
            let new = rail(c.k, c.fillets, &[1.0], false);
            let start = c.start(new)?;
            let eu = eu::primary_edgeuse(c.fillets.brep(), new)?.unwrap();
            let vu = c.fillets.brep().edgeuse(eu)?.vertexuse.unwrap();
            c.fillets
                .brep_mut()
                .vertex_mut(start)?
                .vertexuses
                .retain(|v| *v != vu);
            c.fillets.brep_mut().vertex_mut(split)?.vertexuses.push(vu);
            c.fillets.brep_mut().vertexuse_mut(vu)?.vertex = Some(split);
            c.fillets.brep_mut().edge_mut(new)?.vertices[0] = split;
            geom.set_rail(which, Some(new));
        }
        let id = c.fillets.add_geom(geom);
        Ok(id)
    }
    fn split_rail(
        &mut self,
        c: &mut Recovery<'_>,
        _: FilletGeomId,
        _: EdgeId,
    ) -> Result<(VertexId, EdgeId)> {
        self.call("split_rail")?;
        let e = rail(c.k, c.fillets, &[1.0], false);
        Ok((c.start(e)?, e))
    }
    fn rail_x_edge(
        &mut self,
        _: &mut Recovery<'_>,
        _: VertexId,
        _: FilletGeomId,
        _: usize,
        _: EdgeId,
        _: EdgeId,
        guess: Option<Point3>,
    ) -> Result<()> {
        assert!(guess.is_some());
        self.call("rail_x_edge")
    }
    fn copy_offset(
        &mut self,
        _k: &Kernel,
        offset: &OffsetSurface,
    ) -> (Option<OffsetSurface>, Result<()>) {
        (Some(offset.clone()), self.call("copy_offset"))
    }
    fn cliff(
        &mut self,
        _: &mut Recovery<'_>,
        _: FilletGeomId,
        _: EdgeId,
        _: EdgeId,
        _: Option<FaceId>,
        _: bool,
        extension: f64,
    ) -> Result<Option<FilletGeomId>> {
        assert_eq!(extension, 0.25);
        self.call("cliff")?;
        Ok(Some(FilletGeomId(999)))
    }
    fn make_edge(
        &mut self,
        _: &Kernel,
        fb: &mut FilletBrep,
        start: VertexId,
        end: VertexId,
    ) -> Result<EdgeId> {
        self.call("make_edge")?;
        if self.corner {
            assert_eq!(fb.fillet_vertex(start)?.status(), FilStatus::Processed);
            assert_eq!(fb.brep().vertex(start)?.tolerance, 0.0);
        }
        let edge = fb.new_fillet_edge(start, end, None)?;
        for vertex in [start, end] {
            let vu = fb.brep_mut().add_vertexuse_record(Vertexuse {
                vertex: Some(vertex),
                ..Vertexuse::default()
            })?;
            fb.brep_mut().vertex_mut(vertex)?.vertexuses.push(vu);
            let eu = fb.brep_mut().add_edgeuse_record(Edgeuse {
                edge: Some(edge),
                vertexuse: Some(vu),
                orientation: if vertex == start {
                    OrientType::Same
                } else {
                    OrientType::Opposite
                },
                ..Edgeuse::default()
            })?;
            fb.brep_mut().edge_mut(edge)?.edgeuses.push(eu);
            fb.insert_fillet_edgeuse(eu, FilletEdgeuse::default());
        }
        Ok(edge)
    }

    fn corner_vertex(&mut self, c: &mut Recovery<'_>, v: VertexId, _: FilletGeomId) -> Result<()> {
        assert_eq!(c.fillets.fillet_vertex(v)?.status(), FilStatus::Unprocessed);
        self.call("corner")
    }
    fn recalc(
        &mut self,
        c: &mut Recovery<'_>,
        g: FilletGeomId,
        analytic: bool,
        direction: Option<Vec3>,
    ) -> Result<()> {
        assert!(!analytic);
        assert!(direction.is_some());
        assert!(c.fillets.geom(g)?.fillet_surface(None)?.is_none());
        self.call("recalc")?;
        c.fillets.geom_mut(g)?.set_fillet_surface(Some(plane()));
        Ok(())
    }
    fn support(
        &mut self,
        _: &mut Recovery<'_>,
        _: EdgeId,
        _: FilletGeomId,
        approx: f64,
        angle: f64,
    ) -> Result<()> {
        assert_eq!((approx, angle), (1e-5, 1e-3));
        self.call("support")
    }
    fn trim_rails(&mut self, _: &mut Recovery<'_>, _: FilletGeomId) -> Result<()> {
        self.call("trim")
    }
    fn tangent(&mut self, _: &mut Recovery<'_>, _: EdgeId) -> Result<bool> {
        self.call("tangent")?;
        Ok(true)
    }
}

#[test]
fn both_full_rollovers_execute_in_order_and_preserve_first_mutation_on_error() {
    let mut f = Fixture::new([&[1.0], &[1.0]], true);
    let id = f.geom;
    let mut script = Script {
        fail: Some("full1"),
        ..Script::default()
    };
    let mut adjusted = vec![FilletCornerId(9)];
    f.corners = TestCorners(Some(FilletCornerType::OneByOne));
    assert!(f.context().process(id, &mut adjusted, &mut script).is_err());
    assert_eq!(script.calls, ["full0", "full1"]);
    assert!(f.fb.geom(id).unwrap().need_split);
    assert_eq!(adjusted, [FilletCornerId(9)]);
}
#[test]
fn failed_first_full_rollover_stops_before_second() {
    struct Failing;
    impl RecoveryOps for Failing {
        fn full(&mut self, _: &mut Recovery<'_>, _: FilletGeomId, which: usize) -> Result<()> {
            assert_eq!(which, 0);
            Err(SolidError::sm_err("first"))
        }
    }
    let mut f = Fixture::new([&[1.0], &[1.0]], true);
    let id = f.geom;
    f.corners = TestCorners(Some(FilletCornerType::OneByOne));
    assert_eq!(
        f.context().process(id, &mut vec![], &mut Failing),
        Err(SolidError::sm_err("first"))
    );
}
#[test]
fn tangent_split_rebuilds_both_segments_and_restores_offset_identity_and_orientation() {
    let mut f = Fixture::new([&[0.4, 1.0], &[1.0]], true);
    let id = f.geom;
    let expected_new_geom = FilletGeomId(f.fb.geoms().len() as u32);
    let mut script = Script {
        side: Some(f.side),
        tangent: true,
        ..Script::default()
    };
    let mut adjusted = vec![FilletCornerId(41), FilletCornerId(41)];
    f.context().process(id, &mut adjusted, &mut script).unwrap();
    assert_eq!(
        script.calls,
        [
            "convex",
            "find0",
            "split_geom",
            "copy_offset",
            "rail_x_edge",
            "corner",
            "recalc",
            "trim",
            "find1",
            "tangent",
            "rail_x_edge",
            "recalc",
            "trim"
        ]
    );
    assert_eq!(f.solver.orientations, [1.0, 1.0]);
    assert_eq!(f.solver.surfaces[0].as_ref().unwrap().key, 20);
    assert_eq!(f.solver.fillet_geoms.last(), Some(&expected_new_geom));
    assert!(f.fb.geom(expected_new_geom).is_ok());
    assert_eq!(
        adjusted,
        [FilletCornerId(41), FilletCornerId(41), FilletCornerId(42)]
    );
    assert_eq!(
        f.fb.geom(id).unwrap().geom_type,
        FilletGeomType::TangentRollover
    );
}
#[test]
fn failure_before_trim_keeps_partial_surface_removal_corner_and_changed_offsets() {
    let mut f = Fixture::new([&[0.4, 1.0], &[1.0]], true);
    let id = f.geom;
    let mut script = Script {
        side: Some(f.side),
        tangent: true,
        fail: Some("recalc"),
        ..Script::default()
    };
    let mut adjusted = Vec::new();
    assert!(f.context().process(id, &mut adjusted, &mut script).is_err());
    assert_eq!(script.calls.last().unwrap(), "recalc");
    assert!(!script.calls.iter().any(|s| s == "trim"));
    assert_eq!(adjusted, [FilletCornerId(41)]);
    assert!(f
        .fb
        .geom(id)
        .unwrap()
        .fillet_surface(None)
        .unwrap()
        .is_none());
    assert_eq!(f.solver.orientations[0], -1.0);
    assert_ne!(f.solver.surfaces[0].as_ref().unwrap().key, 20);
}
#[test]
fn cliff_corner_support_is_accumulated_then_computed_before_final_trim() {
    let mut f = Fixture::new([&[0.4, 1.0], &[1.0]], true);
    let id = f.geom;
    let mut script = Script {
        side: Some(f.side),
        corner: true,
        ..Script::default()
    };
    f.context().process(id, &mut vec![], &mut script).unwrap();
    assert_eq!(
        script.calls,
        [
            "convex",
            "find0",
            "split_rail",
            "make_edge",
            "cliff",
            "find0",
            "recalc",
            "support",
            "trim"
        ]
    );
    assert_eq!(f.fb.geom(id).unwrap().other_rails1.len(), 1);
    assert_eq!(
        f.fb.geom(id).unwrap().geom_type,
        FilletGeomType::CliffRollover
    );
}
#[test]
fn a_broken_corner_callee_cannot_spin_the_segment_walk_forever() {
    let mut f = Fixture::new([&[0.4, 1.0], &[1.0]], true);
    let id = f.geom;
    let mut script = Script {
        stalled: true,
        ..Script::default()
    };
    let result = f.context().process(id, &mut vec![], &mut script);
    assert!(matches!(
        result,
        Err(SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })
    ));
    assert_eq!(script.find_count, 44);
}
#[test]
fn empty_class_copies_geometry_but_never_calls_the_side_search() {
    let mut f = Fixture::new([&[], &[1.0]], false);
    let id = f.geom;
    let mut script = Script::default();
    f.context().process(id, &mut vec![], &mut script).unwrap();
    assert_eq!(script.calls, ["convex"]);
}

#[test]
fn tiny_internal_intervals_are_collected_in_reverse_order_and_end_connects_keep_direction() {
    use super::super::{squeeze_rail, ClassificationOps};
    struct Connect {
        calls: Vec<(usize, bool)>,
    }
    impl ClassificationOps for Connect {
        fn connect(
            &mut self,
            _: &Kernel,
            _: &mut FilletBrep,
            _: EdgeId,
            index: usize,
            end: bool,
            allow_removal_of_ends: bool,
        ) -> Result<()> {
            assert!(allow_removal_of_ends);
            self.calls.push((index, end));
            Ok(())
        }
    }
    let k = Kernel::default();
    let mut fb = FilletBrep::new();
    let r = rail(
        &k,
        &mut fb,
        &[0.0001, 0.1, 0.1001, 0.8, 0.8001, 0.9999, 1.0],
        false,
    );
    let mut ops = Connect { calls: vec![] };
    let small = squeeze_rail(&k, &mut fb, r, 1e-5, &mut ops).unwrap();
    assert_eq!(small, [4, 2]);
    assert_eq!(ops.calls, [(6, true), (0, false)]);
}
#[test]
fn failed_end_connection_retains_completed_mutations_and_stops_the_walk() {
    use super::super::{squeeze_rail, ClassificationOps};
    struct Connect;
    impl ClassificationOps for Connect {
        fn connect(
            &mut self,
            _: &Kernel,
            fb: &mut FilletBrep,
            rail: EdgeId,
            index: usize,
            end: bool,
            allow_removal_of_ends: bool,
        ) -> Result<()> {
            assert!(allow_removal_of_ends);
            assert!(end);
            assert_eq!(index, 2);
            fb.fillet_edge_mut(rail)?
                .curve_class
                .as_mut()
                .unwrap()
                .remove_at(index)?;
            Err(SolidError::sm_err("connect"))
        }
    }
    let k = Kernel::default();
    let mut fb = FilletBrep::new();
    let r = rail(&k, &mut fb, &[0.1, 0.9999, 1.0], false);
    assert_eq!(
        squeeze_rail(&k, &mut fb, r, 1e-5, &mut Connect),
        Err(SolidError::sm_err("connect"))
    );
    assert_eq!(fb.fillet_edge(r).unwrap().curve_class().unwrap().size(), 2);
}

#[test]
fn ignored_copy_status_does_not_discard_its_completed_output() {
    let mut f = Fixture::new([&[0.4, 1.0], &[1.0]], true);
    let id = f.geom;
    let mut script = Script {
        side: Some(f.side),
        tangent: true,
        fail: Some("copy_offset"),
        ..Script::default()
    };
    f.context().process(id, &mut vec![], &mut script).unwrap();
    assert_eq!(
        f.fb.geom(FilletGeomId(1)).unwrap().off_surfs[0]
            .as_ref()
            .unwrap()
            .offset_distance(),
        0.25
    );
}

#[test]
fn refinement_receives_rail_index_geom_and_short_list_and_preserves_warning_only_mutation() {
    use super::super::{finish_classification, ClassificationOps};
    struct Refine {
        called: bool,
    }
    impl ClassificationOps for Refine {
        fn refine(
            &mut self,
            solver: &mut dyn crate::blend::fillet_solver::p1::FilletSolverOps,
            k: &Kernel,
            target: &mut Brep,
            fb: &mut FilletBrep,
            geom: FilletGeomId,
            which: usize,
            rail: EdgeId,
            small: &[usize],
        ) -> Result<()> {
            assert_eq!(which, 1);
            assert_eq!(small, [4, 2]);
            assert_eq!(solver.solver().approx_tol_3d, 1e-5);
            assert_eq!(target.faces().len(), 2);
            assert_eq!(k.tolerances.zone_tol_3d, 0.271);
            assert_eq!(fb.geom(geom)?.rail(which), Some(rail));
            self.called = true;
            fb.fillet_edge_mut(rail)?.set_status(FilStatus::Processed);
            Err(SolidError::sm_err("warning-only"))
        }
    }
    let mut f = Fixture::new([&[1.0], &[0.1, 0.2, 0.2001, 0.8, 0.8001, 1.0]], false);
    f.k.tolerances.zone_tol_3d = 0.271;
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(1).unwrap();
    let mut ops = Refine { called: false };
    finish_classification(
        &mut f.solver,
        &f.k,
        &mut f.target,
        &mut f.fb,
        id,
        1,
        rail,
        &mut ops,
    )
    .unwrap();
    assert!(ops.called);
    assert_eq!(
        f.fb.fillet_edge(rail).unwrap().status(),
        FilStatus::Processed
    );
}

#[test]
fn typed_split_adapter_keeps_vertex_backlinks_cached_ends_and_copied_rail_data() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let original_start = f.context().start(rail).unwrap();
    let old_end = f.context().end(rail).unwrap();
    f.fb.brep_mut().edge_mut(rail).unwrap().tolerance = 0.123;
    let mut ops = Script::default();
    let (split, new) = f.context().split_rail(id, rail, &mut ops).unwrap();
    assert_eq!(
        f.fb.brep().edge(rail).unwrap().vertices,
        [original_start, split]
    );
    assert_eq!(f.context().start(new).unwrap(), split);
    assert_eq!(f.context().end(new).unwrap(), old_end);
    assert_eq!(f.fb.brep().vertex(split).unwrap().vertexuses.len(), 2);
    assert_eq!(f.fb.brep().vertex(old_end).unwrap().vertexuses.len(), 1);
    assert_eq!(f.fb.brep().edge(new).unwrap().tolerance, 0.123);
    assert_eq!(f.fb.brep().vertex(split).unwrap().tolerance, 0.123);
    assert_eq!(f.fb.brep().vertex(old_end).unwrap().tolerance, 0.123);
    assert!(f.fb.brep().edge(new).unwrap().curve.is_some());
    let (owner, _) = f.owners.edge_owner(new).unwrap();
    assert_eq!(
        f.fb.brep()
            .edge(new)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .owner(),
        Some(owner)
    );
    assert_eq!(f.owners.notifications.len(), 1);
    assert_eq!(
        f.fb.fillet_edge(new).unwrap().edge_type,
        FilletEdgeType::Rail
    );
    assert_eq!(
        f.fb.fillet_edge(new).unwrap().orig_face,
        f.fb.fillet_edge(rail).unwrap().orig_face
    );
}
#[test]
fn support_intersection_failure_marks_the_edge_and_keeps_existing_curve() {
    struct FailedIntersection;
    impl RecoveryOps for FailedIntersection {
        fn surface_intersection(
            &mut self,
            _: &Kernel,
            _: &mut FilletBrep,
            request: SupportIntersection<'_>,
        ) -> Result<Option<Curve>> {
            assert!(!request.skip_two_points_intersection);
            assert!(request.end.is_none());
            assert!(request.start_direction.is_none());
            assert!(request.end_direction.is_none());
            assert!(request.reference_point.is_none());
            Err(SolidError::sm_err("intersection"))
        }
    }
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let start = f.context().start(rail).unwrap();
    f.fb.fillet_vertex_mut(start)
        .unwrap()
        .set_status(FilStatus::Processed);
    let eus = f.fb.brep().edge(rail).unwrap().edgeuses.clone();
    for eu in eus {
        f.fb.set_edgeuse_fillet_geom(eu, Some(id)).unwrap();
    }
    assert!(f
        .context()
        .support_edge(rail, id, 1e-5, 1e-3, &mut FailedIntersection)
        .is_err());
    assert_eq!(
        f.fb.fillet_edge(rail).unwrap().status(),
        FilStatus::SurfIntFailure
    );
    assert!(f.fb.brep().edge(rail).unwrap().curve.is_some());
}

#[test]
fn support_recovery_intersects_trims_installs_and_notifies_the_actual_curve() {
    struct Intersection;
    impl RecoveryOps for Intersection {
        fn surface_intersection(
            &mut self,
            _: &Kernel,
            _: &mut FilletBrep,
            request: SupportIntersection<'_>,
        ) -> Result<Option<Curve>> {
            assert!(!request.skip_two_points_intersection);
            assert!(request.end.is_none());
            assert!(request.start_direction.is_none());
            assert!(request.end_direction.is_none());
            assert!(request.reference_point.is_none());
            Ok(Some(Line::from_points(Vec3::Z, -Vec3::Z, 3).into()))
        }
    }
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let start = f.context().start(rail).unwrap();
    let end = f.context().end(rail).unwrap();
    f.fb.brep_mut().vertex_mut(start).unwrap().point = Vec3::Z;
    f.fb.fillet_vertex_mut(start)
        .unwrap()
        .set_status(FilStatus::Processed);
    let eus = f.fb.brep().edge(rail).unwrap().edgeuses.clone();
    for eu in eus {
        f.fb.set_edgeuse_fillet_geom(eu, Some(id)).unwrap();
    }
    f.context()
        .support_edge(rail, id, 1e-5, 1e-3, &mut Intersection)
        .unwrap();
    assert_eq!(f.fb.brep().vertex(end).unwrap().point, Vec3::ZERO);
    assert_eq!(
        f.fb.fillet_vertex(end).unwrap().status(),
        FilStatus::Processed
    );
    let curve = f.fb.brep().edge(rail).unwrap().curve.as_ref().unwrap();
    assert_eq!(
        curve
            .evaluate_point(&f.k, curve.natural_interval().min())
            .unwrap(),
        Vec3::Z
    );
    assert_eq!(
        curve
            .evaluate_point(&f.k, curve.natural_interval().max())
            .unwrap(),
        Vec3::ZERO
    );
    assert_eq!(curve.owner(), Some(f.owners.edge_owner(rail).unwrap().0));
    assert_eq!(f.owners.notifications.len(), 1);
}

#[test]
fn offset_copy_uses_the_real_port_and_preserves_base_identity_and_options() {
    let f = Fixture::new([&[1.0], &[1.0]], false);
    let source = &f.solver.surfaces[0].as_ref().unwrap().surface;
    let (copy, status) = Missing.copy_offset(&f.k, source);
    status.unwrap();
    let copy = copy.unwrap();
    assert_eq!(copy.base_id, source.base_id);
    assert_eq!(copy.offset_distance(), source.offset_distance());
    assert_eq!(
        copy.surface.allow_extension(),
        source.surface.allow_extension()
    );
    assert_eq!(copy.surface.singularities(), source.surface.singularities());
    assert_eq!(
        copy.surface
            .evaluate_point(&f.k, crate::math::Point2::new(0.2, 0.3))
            .unwrap(),
        source
            .surface
            .evaluate_point(&f.k, crate::math::Point2::new(0.2, 0.3))
            .unwrap()
    );
}

#[test]
fn public_split_helper_rejects_a_missing_infinite_region_after_partial_edits() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let old_end = f.context().end(rail).unwrap();
    // The real constructor supplies this region. Explicitly corrupt the fixture
    // to exercise the builder's null guard after SplitRail moved the end use.
    assert!(f.fb.brep().infinite_region().is_some());
    f.fb.brep_mut().set_infinite_region_record(None);
    let outcome = super::super::super::p1::split_rail(&f.k, &mut f.fb, &mut f.owners, id, rail);
    assert!(matches!(
        outcome,
        Err(SolidError::Upstream {
            code: crate::error::SM_ERR_NULL_POINTER,
            ..
        })
    ));
    assert_ne!(f.context().end(rail).unwrap(), old_end);
    assert!(f.fb.brep().vertex(old_end).unwrap().vertexuses.is_empty());
    assert!(f.owners.notifications.is_empty());
}

#[test]
fn public_split_helper_uses_the_ported_edge_builder_and_actual_geometry_id() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let old_end = f.context().end(rail).unwrap();
    // Exercise the actual constructor rather than injecting a substitute region.
    assert!(f.fb.brep().infinite_region().is_some());
    let (middle, new_rail) =
        super::super::super::p1::split_rail(&f.k, &mut f.fb, &mut f.owners, id, rail).unwrap();
    assert_eq!(f.context().end(rail).unwrap(), middle);
    assert_eq!(f.context().start(new_rail).unwrap(), middle);
    assert_eq!(f.context().end(new_rail).unwrap(), old_end);
    assert_eq!(f.fb.brep().vertex(middle).unwrap().vertexuses.len(), 2);
    assert_eq!(f.fb.brep().vertex(old_end).unwrap().vertexuses.len(), 1);
    let primary = eu::primary_edgeuse(f.fb.brep(), new_rail).unwrap().unwrap();
    let mate = eu::mate(f.fb.brep(), primary).unwrap().unwrap();
    assert_eq!(f.fb.fillet_edgeuse(mate).unwrap().fillet_geom(), Some(id));
    assert_eq!(
        f.fb.fillet_edge(new_rail).unwrap().edge_type,
        FilletEdgeType::Rail
    );
    let owner = f.owners.edge_owner(new_rail).unwrap().0;
    assert_eq!(
        f.fb.brep()
            .edge(new_rail)
            .unwrap()
            .curve
            .as_ref()
            .unwrap()
            .owner(),
        Some(owner)
    );
    assert_eq!(f.owners.notifications.len(), 1);
}

#[test]
fn end_squeezing_uses_the_real_connect_intervals_port() {
    use super::super::{squeeze_rail, MissingClassification};
    let mut f = Fixture::new([&[0.01, 0.99, 1.0], &[1.0]], false);
    let rail = f.fb.geom(f.geom).unwrap().rail(0).unwrap();
    let small = squeeze_rail(&f.k, &mut f.fb, rail, 0.001, &mut MissingClassification).unwrap();
    assert!(small.is_empty());
    let cc = f.fb.fillet_edge(rail).unwrap().curve_class().unwrap();
    assert_eq!(cc.size(), 1);
    assert_eq!(cc.sub_interval(0).unwrap().min(), 0.01);
    assert_eq!(cc.sub_interval(0).unwrap().max(), 0.99);
}

#[test]
fn split_preserves_tolerances_smaller_than_the_zone_default() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let end = f.context().end(rail).unwrap();
    f.fb.brep_mut().edge_mut(rail).unwrap().tolerance = 1e-12;
    f.fb.brep_mut().vertex_mut(end).unwrap().tolerance = 1e-12;
    let (middle, new_rail) = f
        .context()
        .split_rail(id, rail, &mut Script::default())
        .unwrap();
    assert_eq!(f.fb.brep().edge(new_rail).unwrap().tolerance, 1e-12);
    assert_eq!(f.fb.brep().vertex(middle).unwrap().tolerance, 1e-12);
    assert_eq!(f.fb.brep().vertex(end).unwrap().tolerance, 1e-12);
}

#[test]
fn public_support_helper_uses_the_real_surface_intersection() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let start = f.context().start(rail).unwrap();
    let end = f.context().end(rail).unwrap();
    f.fb.brep_mut().vertex_mut(start).unwrap().point = Vec3::Z;
    f.fb.fillet_vertex_mut(start)
        .unwrap()
        .set_status(FilStatus::Processed);
    let eus = f.fb.brep().edge(rail).unwrap().edgeuses.clone();
    for (eu, normal) in eus.into_iter().zip([Vec3::X, Vec3::Y]) {
        let mut geom = FilletGeom::new(&f.k, None, FilletGeomType::Default).unwrap();
        geom.set_fillet_surface(Some(Plane::from_normal(Point3::ZERO, normal).into()));
        let g = f.fb.add_geom(geom);
        f.fb.set_edgeuse_fillet_geom(eu, Some(g)).unwrap();
    }
    super::super::super::p1::compute_support_edge(
        &f.k,
        &mut f.fb,
        &mut f.owners,
        rail,
        id,
        1e-5,
        1e-3,
    )
    .unwrap();
    assert_eq!(f.fb.brep().vertex(end).unwrap().point, Vec3::ZERO);
    assert_eq!(
        f.fb.fillet_vertex(end).unwrap().status(),
        FilStatus::Processed
    );
    let curve = f.fb.brep().edge(rail).unwrap().curve.as_ref().unwrap();
    assert!(
        (curve
            .evaluate_point(&f.k, curve.natural_interval().min())
            .unwrap()
            - Vec3::Z)
            .length()
            < 1e-12
    );
    assert!(
        curve
            .evaluate_point(&f.k, curve.natural_interval().max())
            .unwrap()
            .length()
            < 1e-12
    );
    assert_eq!(curve.owner(), Some(f.owners.edge_owner(rail).unwrap().0));
    assert_eq!(f.owners.notifications.len(), 1);
}

#[test]
fn rollover_trim_uses_the_live_solver_at_its_nonzero_id() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let solver_id = crate::blend::fillet_geom::p1::FilletSolverId(37);
    f.fb.geom_mut(id).unwrap().solver = Some(solver_id);
    f.fb.geom_mut(id).unwrap().set_fillet_surface(Some(
        Plane::new(
            Point3::ZERO,
            Vec3::X,
            Vec3::Y,
            crate::math::Vec2::new(1.0, 1.0),
            &crate::geom::extent2d::Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        )
        .into(),
    ));
    Missing.trim_rails(&mut f.context(), id).unwrap();
    for rail in f.fb.geom(id).unwrap().rails.into_iter().flatten() {
        let primary = eu::primary_edgeuse(f.fb.brep(), rail).unwrap().unwrap();
        let uv =
            f.fb.brep()
                .edgeuse(primary)
                .unwrap()
                .uv_trim_curve
                .as_ref()
                .unwrap();
        assert_eq!(
            uv.evaluate_point(&f.k, 0.25).unwrap(),
            Point3::new(0.25, 0.0, 0.0)
        );
    }
    assert!(matches!(
        f.fb.trim_rail_curves_with_solver(
            &f.k,
            id,
            crate::blend::fillet_geom::p1::FilletSolverId(0),
            &f.solver,
            &f.target
        ),
        Err(SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. })
    ));
}

#[test]
fn rollover_corner_vertex_delegates_to_the_available_geom_port() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    f.fb.geom_mut(id).unwrap().solver = Some(crate::blend::fillet_geom::p1::FilletSolverId(37));
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let start = f.context().start(rail).unwrap();
    let r = Missing.corner_vertex(&mut f.context(), start, id);
    // the fixture's corner table is empty: the vertex's corner is not in it
    assert!(matches!(
        r,
        Err(SolidError::Upstream {
            code: crate::error::SM_ERR_NULL_POINTER,
            ..
        })
    ));
}

#[test]
fn rollover_topology_split_preserves_actual_solver_id_and_partial_edits() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    let solver_id = crate::blend::fillet_geom::p1::FilletSolverId(37);
    f.fb.geom_mut(id).unwrap().solver = Some(solver_id);
    f.solver.executive = Some(crate::blend::fillet_solver::p1::FilletExecutiveRef);
    for rail in f.fb.geom(id).unwrap().rails.into_iter().flatten() {
        f.fb.fillet_edge_mut(rail)
            .unwrap()
            .set_fillet_edge_type(FilletEdgeType::Rail);
        let uses = f.fb.brep().edge(rail).unwrap().edgeuses.clone();
        for eu in uses {
            f.fb.set_edgeuse_fillet_geom(eu, Some(id)).unwrap();
            let vu = f.fb.brep().edgeuse(eu).unwrap().vertexuse.unwrap();
            let vertex = f.fb.brep().vertexuse(vu).unwrap().vertex.unwrap();
            f.fb.brep_mut()
                .vertex_mut(vertex)
                .unwrap()
                .vertexuses
                .push(vu);
            f.fb.brep_mut().vertexuse_mut(vu).unwrap().owner =
                crate::topology::VertexuseOwner::Edgeuse(Some(eu));
            f.fb.insert_fillet_vertexuse(vu, crate::blend::fillet_geom::p1::FilletVertexuse::new());
        }
    }
    let rail = f.fb.geom(id).unwrap().rail(0).unwrap();
    let end_use = f.fb.brep().edge(rail).unwrap().edgeuses[1];
    let count = f.fb.geoms().len();
    let err = Missing.topology_split(&mut f.context(), id).unwrap_err();
    assert!(
        matches!(
            err,
            SolidError::Upstream {
                operation: "blend::fillet_geom::p5::FilletBrep::make_rail_edgeuse",
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ),
        "{err:?}"
    );
    let new_geom = f.fb.geoms().last().unwrap();
    assert_eq!(f.fb.geoms().len(), count + 1);
    assert_eq!(new_geom.fillet_solver(), Some(solver_id));
    assert_eq!(new_geom.fillet_geom_type(), FilletGeomType::Default);
    assert!(!f.fb.brep().edge(rail).unwrap().edgeuses.contains(&end_use));
    assert_eq!(f.fb.brep().edgeuse(end_use).unwrap().edge, None);
}

#[test]
fn rollover_recalc_uses_the_real_solver_and_reports_missing_edge_curve() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let id = f.geom;
    f.fb.geom_mut(id).unwrap().solver = Some(crate::blend::fillet_geom::p1::FilletSolverId(37));
    let err = Missing
        .recalc(&mut f.context(), id, false, Some(Vec3::Y))
        .unwrap_err();
    assert!(
        matches!(
            err,
            SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        err.operation(),
        "blend::fillet_geom::p3::FilletBrep::re_calc_fillet_geom"
    );
}

#[test]
fn landed_p3_refinement_is_a_noop_without_small_intervals_and_restores_errors() {
    use super::super::{ClassificationOps, MissingClassification};
    let mut f = Fixture::new([&[0.3, 0.31, 1.0], &[1.0]], false);
    let rail = f.fb.geom(f.geom).unwrap().rail(0).unwrap();
    MissingClassification
        .refine(
            &mut f.solver,
            &f.k,
            &mut f.target,
            &mut f.fb,
            f.geom,
            0,
            rail,
            &[],
        )
        .unwrap();
    let error = MissingClassification
        .refine(
            &mut f.solver,
            &f.k,
            &mut f.target,
            &mut f.fb,
            f.geom,
            0,
            rail,
            &[1],
        )
        .unwrap_err();
    assert_eq!(
        error,
        SolidError::sm_err("blend::fillet_solver::p3::refine_rail_curve_classification")
    );
    let cc = f.fb.fillet_edge(rail).unwrap().curve_class().unwrap();
    assert_eq!(cc.size(), 3);
    assert_eq!(cc.sub_interval(1).unwrap().min(), 0.3);
    assert_eq!(cc.sub_interval(1).unwrap().max(), 0.31);
}

#[test]
fn landed_p3_rail_intersection_reports_the_actual_missing_face_use() {
    let mut f = Fixture::new([&[1.0], &[1.0]], false);
    let rail = f.fb.geom(f.geom).unwrap().rail(0).unwrap();
    let vertex = edge::vertex(f.fb.brep(), rail).unwrap().unwrap();
    let a = f.target.add_vertex(Vec3::ZERO).unwrap();
    let b = f.target.add_vertex(Vec3::X).unwrap();
    let side = f.target.add_edge(a, b).unwrap();
    let geom = f.geom;
    let error = Missing
        .rail_x_edge(&mut f.context(), vertex, geom, 0, rail, side, None)
        .unwrap_err();
    assert_eq!(
        error,
        SolidError::null_pointer("blend::fillet_solver::p3::calc_rail_x_edge")
    );
    assert!(f.owners.notifications.is_empty());
}
