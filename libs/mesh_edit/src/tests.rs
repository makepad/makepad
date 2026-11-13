use crate::*;

#[test]
fn catmull_clark_cube_preserves_weights_uv_seams_and_composes_provenance() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let old_vertex = mesh.vertices()[0].id;
    let old_corner = mesh.corners()[0].id;
    let source = mesh.face_corners(mesh.faces()[0].id).unwrap();
    let edge = EdgeKey::new(source[0].vertex, source[1].vertex);
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: true,
            crease: 0.,
        },
        &mut ctx,
    )
    .unwrap();
    let weights = mesh
        .vertices()
        .iter()
        .enumerate()
        .map(|(i, v)| {
            (
                v.id,
                vec![JointWeight {
                    joint: (i % 2) as u32,
                    weight: 1.,
                }],
            )
        })
        .collect::<Vec<_>>();
    mesh.set_weights_bulk(&weights, &mut ctx).unwrap();
    let change = mesh.subdivide(1, &mut ctx).unwrap();
    assert_eq!(
        (
            mesh.vertices().len(),
            mesh.faces().len(),
            mesh.corners().len()
        ),
        (26, 24, 96)
    );
    for p in mesh.vertex(old_vertex).unwrap().position {
        assert!((p.abs() - 5. / 9.).abs() < 1e-12);
    }
    assert!(mesh.validate(&mut ctx).unwrap().is_closed_manifold);
    assert!(mesh
        .validate(&mut ctx)
        .unwrap()
        .issues
        .iter()
        .any(|i| i.kind == ValidationKind::NonPlanarFace));
    assert_eq!(
        mesh.edge_attributes()
            .values()
            .filter(|d| d.attributes.seam)
            .count(),
        2
    );
    let remap = change
        .remapped
        .iter()
        .find(|(source, _)| *source == ElementId::Corner(old_corner))
        .unwrap();
    let ElementId::Corner(new_corner) = remap.1[0] else {
        panic!("corner provenance")
    };
    assert_eq!(mesh.corner(new_corner).unwrap().uv, [0., 0.]);
    for v in mesh.vertices() {
        assert!((v.weights.iter().map(|w| w.weight).sum::<f64>() - 1.).abs() < 1e-12);
    }
    let mut two = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let source_corner = two.corners()[0].id;
    let result = two.subdivide(2, &mut ctx).unwrap();
    assert_eq!((two.vertices().len(), two.faces().len()), (98, 96));
    let targets = &result
        .remapped
        .iter()
        .find(|(id, _)| *id == ElementId::Corner(source_corner))
        .unwrap()
        .1;
    assert_eq!(targets.len(), 1);
    let ElementId::Corner(target) = targets[0] else {
        panic!("corner")
    };
    assert!(two.corner(target).is_some());
    assert_eq!(two.triangulate(&mut ctx).unwrap().triangles.len(), 192);
    let encoded = two.to_bytes(&mut ctx).unwrap();
    assert_eq!(
        Mesh::from_bytes(&encoded, &mut ctx)
            .unwrap()
            .to_bytes(&mut ctx)
            .unwrap(),
        encoded
    );
}

#[test]
fn smooth_brush_and_projection_are_bounded_and_preserve_boundary_attributes() {
    let mut positions = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            positions.push([x as f64, if x == 0 && z == 0 { 1. } else { 0. }, z as f64]);
        }
    }
    let polygons = [
        vec![0, 3, 4, 1],
        vec![1, 4, 5, 2],
        vec![3, 6, 7, 4],
        vec![4, 7, 8, 5],
    ]
    .map(Polygon::new);
    let mut ctx = Context::default();
    let mut mesh = Mesh::from_polygons(&positions, &polygons, &mut ctx).unwrap();
    let ids = mesh.vertices().iter().map(|v| v.id).collect::<Vec<_>>();
    let center = ids[4];
    mesh.set_vertex_weights(
        center,
        &[JointWeight {
            joint: 7,
            weight: 1.,
        }],
        &mut ctx,
    )
    .unwrap();
    let corners = mesh
        .corners()
        .iter()
        .map(|c| (c.id, c.uv))
        .collect::<Vec<_>>();
    mesh.smooth(&ids, 1, 0.5, true, &mut ctx).unwrap();
    assert_eq!(mesh.vertex(center).unwrap().position, [0., 0.5, 0.]);
    for (i, p) in positions.iter().enumerate() {
        if i != 4 {
            assert_eq!(&mesh.vertex(ids[i]).unwrap().position, p);
        }
    }
    assert_eq!(
        mesh.corners()
            .iter()
            .map(|c| (c.id, c.uv))
            .collect::<Vec<_>>(),
        corners
    );
    assert_eq!(mesh.vertex(center).unwrap().weights[0].joint, 7);
    mesh.brush(&ids, [0., 0.5, 0.], 1., [0., 0.25, 0.], 0.25, &mut ctx)
        .unwrap();
    assert_eq!(mesh.vertex(center).unwrap().position, [0., 0.75, 0.]);
    let before = mesh.to_bytes(&mut ctx).unwrap();
    assert!(mesh
        .brush(&ids, [0.; 3], 1., [0., 1., 0.], 0.1, &mut ctx)
        .is_err());
    assert_eq!(mesh.to_bytes(&mut ctx).unwrap(), before);
    let faces = mesh.faces().iter().map(|f| f.id).collect::<Vec<_>>();
    mesh.project_uv(&faces, 1, [0.5; 2], [0.5; 2], &mut ctx)
        .unwrap();
    for c in mesh.corners() {
        assert!(c.uv.iter().all(|v| (0.0..=1.0).contains(v)));
    }
    let before = mesh.to_bytes(&mut ctx).unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 40
    };
    assert!(mesh
        .smooth(
            &ids,
            64,
            0.5,
            true,
            &mut Context::new(Limits::default(), Some(&cancel))
        )
        .is_err());
    assert_eq!(mesh.to_bytes(&mut ctx).unwrap(), before);
}

#[test]
fn subdivision_preserves_fully_creased_cube_corners() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([1.; 3], &mut ctx).unwrap();
    let original=mesh.vertices().to_vec();
    let edges=mesh.adjacency(&mut ctx).unwrap().edges.keys().copied().collect::<Vec<_>>();
    for edge in edges {
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: false,
            crease: 1.,
        },
        &mut ctx,
    )
    .unwrap();
    }
    mesh.subdivide(2,&mut ctx).unwrap();
    for v in original {assert_eq!(mesh.vertex(v.id).unwrap().position,v.position);}
    assert!(mesh.validate(&mut ctx).unwrap().is_closed_manifold);
    assert!(mesh.corners().iter().all(|c|c.normal.unwrap().iter().filter(|x|x.abs()>1e-8).count()==1));
}

#[test]
fn cube_extrude_preserves_topology_uv_weights_and_identity() {
    let mut ctx = Context::default();
    let mut mesh = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let face = mesh.faces()[1].id;
    let vertex = mesh.face_corners(face).unwrap()[0].vertex;
    mesh.set_vertex_weights(
        vertex,
        &[
            JointWeight {
                joint: 3,
                weight: 2.,
            },
            JointWeight {
                joint: 7,
                weight: 1.,
            },
        ],
        &mut ctx,
    )
    .unwrap();
    let before = mesh.face_corners(face).unwrap().to_vec();
    let edge = EdgeKey::new(before[0].vertex, before[1].vertex);
    mesh.set_edge_attributes(
        edge,
        EdgeAttributes {
            seam: true,
            crease: 0.7,
        },
        &mut ctx,
    )
    .unwrap();
    let extrusion = mesh.extrude_face(face, [0., 0., 1.], &mut ctx).unwrap();
    assert_eq!(extrusion.cap, face);
    assert_eq!(extrusion.side_faces.len(), 4);
    let after = mesh.face_corners(face).unwrap();
    assert_eq!(
        before.iter().map(|c| (c.id, c.uv)).collect::<Vec<_>>(),
        after.iter().map(|c| (c.id, c.uv)).collect::<Vec<_>>()
    );
    assert_eq!(
        mesh.vertex(after[0].vertex).unwrap().weights,
        mesh.vertex(vertex).unwrap().weights
    );
    assert_eq!(
        mesh.edge_attributes()[&extrusion.rim_edges[0]]
            .attributes
            .crease,
        0.7
    );
    let report = mesh.validate(&mut ctx).unwrap();
    assert!(report.is_closed_manifold, "{report:?}");
    assert_eq!(mesh.triangulate(&mut ctx).unwrap().triangles.len(), 20);
    let bytes = mesh.to_bytes(&mut ctx).unwrap();
    assert_eq!(
        Mesh::from_bytes(&bytes, &mut ctx)
            .unwrap()
            .to_bytes(&mut ctx)
            .unwrap(),
        bytes
    );
}

#[test]
fn planar_inset_has_constant_distance_and_keeps_solid() {
    let mut ctx = Context::default();
    let mut m = Mesh::cube([2.; 3], &mut ctx).unwrap();
    let face = m.faces()[1].id;
    let result = m.inset_face(face, 0.25, &mut ctx).unwrap();
    assert_eq!(result.ring_faces.len(), 4);
    for c in m.face_corners(face).unwrap() {
        let p = m.vertex(c.vertex).unwrap().position;
        assert!((p[0].abs() - 0.75).abs() < 1e-12 && (p[1].abs() - 0.75).abs() < 1e-12);
        assert!(c.uv.iter().all(|v| *v >= 0.125 && *v <= 0.875));
    }
    assert!(m.validate(&mut ctx).unwrap().is_closed_manifold);
    let bytes = m.to_bytes(&mut ctx).unwrap();
    assert!(m.inset_face(face, 10., &mut ctx).is_err());
    assert_eq!(bytes, m.to_bytes(&mut ctx).unwrap());
}

#[test]
fn cancellation_and_bad_import_are_atomic() {
    let mut ctx = Context::default();
    let mut m = Mesh::plane([1., 1.], &mut ctx).unwrap();
    let before = m.to_bytes(&mut ctx).unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        let n = calls.get() + 1;
        calls.set(n);
        n > 8
    };
    let mut cancelled = Context::new(Limits::default(), Some(&cancel));
    assert_eq!(
        m.extrude_face(m.faces()[0].id, [0., 1., 0.], &mut cancelled)
            .unwrap_err(),
        MeshError::Cancelled
    );
    assert_eq!(m.to_bytes(&mut ctx).unwrap(), before);
    assert!(Mesh::from_polygons(
        &[[0., 0., 0.], [1., 0., 0.]],
        &[Polygon::new(vec![0, 1, 50])],
        &mut ctx
    )
    .is_err());
    let mut trailing = before.clone();
    trailing.push(0);
    assert!(Mesh::from_bytes(&trailing, &mut ctx).is_err());
    for n in 0..before.len() {
        assert!(Mesh::from_bytes(&before[..n], &mut Context::default()).is_err());
    }
}

#[test]
fn triangulation_handles_concavity_both_windings_and_collinear_corners() {
    let p = [
        [0., 0., 0.],
        [2., 0., 0.],
        [2., 1., 0.],
        [1., 1., 0.],
        [1., 2., 0.],
        [0., 2., 0.],
        [0., 1., 0.],
    ];
    for reversed in [false, true] {
        let mut ids = (0..p.len() as u32).collect::<Vec<_>>();
        if reversed {
            ids.reverse();
        }
        let mut ctx = Context::default();
        let m = Mesh::from_polygons(&p, &[Polygon::new(ids)], &mut ctx).unwrap();
        let t = m.triangulate(&mut ctx).unwrap();
        assert_eq!(t.triangles.len(), p.len() - 2);
        let area = t
            .triangles
            .iter()
            .map(|tri| {
                let [a, b, c] = tri.indices.map(|i| t.vertices[i as usize].position);
                let cross =
                    crate::geometry::cross(crate::geometry::sub(b, a), crate::geometry::sub(c, a));
                assert_eq!(cross[2] > 0., !reversed);
                cross[2].abs() * 0.5
            })
            .sum::<f64>();
        assert!((area - 3.).abs() < 1e-12);
        assert_eq!(t, m.triangulate(&mut ctx).unwrap());
    }
}

#[test]
fn bow_tie_vertex_and_three_face_edge_are_diagnosed() {
    let p = [
        [0., 0., 0.],
        [1., 0., 0.],
        [0., 1., 0.],
        [-1., 0., 0.],
        [0., -1., 0.],
        [0., 0., 1.],
    ];
    let mut ctx = Context::default();
    let m = Mesh::from_polygons(
        &p,
        &[Polygon::new(vec![0, 1, 2]), Polygon::new(vec![0, 3, 4])],
        &mut ctx,
    )
    .unwrap();
    assert_eq!(m.validate(&mut ctx).unwrap().non_manifold_vertices, 1);
    let m = Mesh::from_polygons(
        &p,
        &[
            Polygon::new(vec![0, 1, 2]),
            Polygon::new(vec![1, 0, 4]),
            Polygon::new(vec![0, 1, 5]),
        ],
        &mut ctx,
    )
    .unwrap();
    assert_eq!(
        m.adjacency(&mut ctx)
            .unwrap()
            .radial(EdgeKey::new(m.vertices()[0].id, m.vertices()[1].id))
            .len(),
        3
    );
    assert_eq!(m.validate(&mut ctx).unwrap().non_manifold_edges, 1);
}

#[test]
fn mirror_weld_and_delete_keep_stable_ids() {
    let mut ctx = Context::default();
    let mut m = Mesh::plane([2., 2.], &mut ctx).unwrap();
    let original = m.faces()[0].id;
    m.mirror(0, 0., &mut ctx).unwrap();
    assert_eq!(m.faces().len(), 2);
    let mirrored = m.faces()[1].id;
    let vertices = m.vertices().iter().map(|v| v.id).collect::<Vec<_>>();
    let changes = m.weld(&vertices, 0., &mut ctx).unwrap();
    assert_eq!(m.vertices().len(), 4);
    assert_eq!(
        changes
            .deleted
            .iter()
            .filter(|id| matches!(id, ElementId::Vertex(_)))
            .count(),
        4
    );
    m.delete_faces(&[original], &mut ctx).unwrap();
    assert_eq!(m.faces()[0].id, mirrored);
    assert!(m.validate(&mut ctx).unwrap().is_valid_surface);
    let before = m.to_bytes(&mut ctx).unwrap();
    let limits = Limits::default();
    assert!(m
        .extrude_face(mirrored, [0., 1., 0.], &mut Context::new(limits, None))
        .is_ok());
    assert_ne!(before, m.to_bytes(&mut ctx).unwrap());
}

#[test]
fn inward_closed_shells_turn_outward_and_cavities_open_sheets_stay() {
    // Two boxes: an outer 0..4 and a cavity 1..2 inside it, plus an open quad.
    let boxed = |o: f64, s: f64, inward: bool, positions: &mut Vec<[f64; 3]>, polygons: &mut Vec<Polygon>| {
        let base = positions.len() as u32;
        for z in [0., 1.] { for y in [0., 1.] { for x in [0., 1.] { positions.push([o + x * s, o + y * s, o + z * s]); } } }
        // Counter-clockwise seen from outside.
        for f in [[0u32, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5], [0, 2, 3, 1], [4, 5, 7, 6]] {
            let mut f = f.map(|i| base + i).to_vec(); if inward { f.reverse(); }
            polygons.push(Polygon::new(f));
        }
    };
    let volume = |m: &Mesh| { let t = m.triangulate(&mut Context::default()).unwrap();
        t.triangles.iter().map(|tri| { let [a, b, c] = tri.indices.map(|i| t.vertices[i as usize].position); crate::geometry::dot(a, crate::geometry::cross(b, c)) / 6. }).sum::<f64>() };
    let mut ctx = Context::default();
    // Outward shells (the primitives too) are left exactly as they are.
    for mut m in [Mesh::cube([1., 2., 3.], &mut ctx).unwrap(), { let (mut p, mut f) = (Vec::new(), Vec::new()); boxed(0., 1., false, &mut p, &mut f); Mesh::from_polygons(&p, &f, &mut ctx).unwrap() }] {
        let before = m.clone(); assert_eq!(m.orient_closed_outward(&mut ctx).unwrap(), 0); assert_eq!(m, before);
    }
    // An inward box turns outward, keeping every face identity.
    let (mut p, mut f) = (Vec::new(), Vec::new()); boxed(0., 1., true, &mut p, &mut f);
    let mut m = Mesh::from_polygons(&p, &f, &mut ctx).unwrap();
    assert!((volume(&m) + 1.).abs() < 1e-9);
    assert_eq!(m.orient_closed_outward(&mut ctx).unwrap(), 6);
    assert!((volume(&m) - 1.).abs() < 1e-9);
    assert!(m.validate(&mut ctx).unwrap().is_closed_manifold);
    // An outer shell written inward with an inward cavity: the shell turns, the cavity stays.
    let (mut p, mut f) = (Vec::new(), Vec::new());
    boxed(0., 4., true, &mut p, &mut f); boxed(1., 1., true, &mut p, &mut f);
    let base = p.len() as u32; p.extend([[10., 0., 0.], [11., 0., 0.], [11., 0., 1.], [10., 0., 1.]]);
    f.push(Polygon::new(vec![base + 3, base + 2, base + 1, base]));
    let mut m = Mesh::from_polygons(&p, &f, &mut ctx).unwrap();
    assert_eq!(m.orient_closed_outward(&mut ctx).unwrap(), 6);
    assert!((volume(&m) - (64. - 1.)).abs() < 1e-9);
    assert_eq!(m.orient_closed_outward(&mut ctx).unwrap(), 0);
}


#[test]
fn decimation_matches_the_old_path_with_reused_scratch() {
    for bump in [0., 0.125] {
        let positions: Vec<_> = (0..4).flat_map(|z| (0..4).map(move |x|
            [x as f64, if x == 2 && z == 2 { bump } else { 0. }, z as f64])).collect();
        let mut polygons = Vec::new();
        for z in 0..3 { for x in 0..3 {
            let corners = [z * 4 + x, (z + 1) * 4 + x, (z + 1) * 4 + x + 1, z * 4 + x + 1];
            polygons.push(Polygon {
                vertices: corners.to_vec(),
                uvs: corners.map(|i| [(i % 4) as f64 / 3., (i / 4) as f64 / 3.]).to_vec(),
                material: 7,
            });
        } }
        let mut mesh = Mesh::from_polygons(&positions, &polygons, &mut Context::default()).unwrap();
        if bump != 0. {
            let weights: Vec<_> = mesh.vertices().iter().enumerate().map(|(i, v)|
                (v.id, vec![JointWeight { joint: (i % 3) as u32, weight: 1. }])).collect();
            mesh.set_weights_bulk(&weights, &mut Context::default()).unwrap();
            mesh.pin_uv(&[mesh.corners()[0].id], true, &mut Context::default()).unwrap();
        }
        let mut old = mesh.clone();
        let mut ctx = Context::default();
        let mut old_ctx = Context::default();
        let expected = old.decimate_before_scratch(8, 2., &mut old_ctx).unwrap();
        let result = mesh.decimate(8, 2., &mut ctx).unwrap();
        assert!(result.collapses > 1, "fixture must reuse collapse scratch");
        assert_eq!(result, expected, "counts and provenance, bump {bump}");
        assert_eq!(mesh, old, "positions, IDs, corners, attributes and weights, bump {bump}");
        assert_eq!(ctx.work_used(), old_ctx.work_used(), "work budget, bump {bump}");
    }
}

// Frozen decimation and corner-copy paths from before scratch reuse. Shared
// triangulation, geometric validation and transaction code are unchanged.
mod legacy_decimate {
    use crate::{context::invalid,geometry::*,topology::{surface,blended_vertex},*};
    use std::collections::{BTreeMap,BTreeSet};

    impl Mesh {
        /// Triangulates the cage, then collapses eligible manifold edges toward the
        /// requested triangle-face count. max_error bounds every original vertex's
        /// cumulative displacement from its surviving representative. UV seams,
        /// material boundaries, pinned vertices and marked edge neighborhoods stay
        /// fixed. The achieved count is explicit when these constraints stop work.
        pub(super) fn decimate_before_scratch(&mut self,target_faces:usize,max_error:f64,ctx:&mut Context<'_>)->Result<DecimateResult>{
            if target_faces==0||!max_error.is_finite()||max_error<0.{return Err(invalid("decimation requires positive face target and nonnegative finite error"));}
            surface(self,ctx)?;let triangles=self.triangulate(ctx)?;ctx.counts(self.vertices.len(),triangles.triangles.len(),triangles.triangles.len()*3,triangles.triangles.len()*3)?;
            let ((achieved_faces,collapses),changes)=self.edit(ctx,triangles.vertices.len().saturating_mul(2048)+triangles.triangles.len().saturating_mul(1024),|m,ctx|{
                let source=m.clone();let mut first=BTreeSet::new();let mut deferred=Vec::new();let mut next=Mesh::new();next.next_id=source.next_id;next.vertices=source.vertices.clone();next.edge_data=source.edge_data.clone();
                let mut mappings=BTreeMap::<ElementId,Vec<ElementId>>::new();let mut reused=BTreeSet::new();
                for tri in &triangles.triangles{ctx.checkpoint(1)?;if first.insert(tri.source_face){emit_triangle(&mut next,&source,&triangles,tri,Some(tri.source_face),&mut mappings,&mut reused)?;}else{deferred.push(tri);}}
                for tri in deferred{emit_triangle(&mut next,&source,&triangles,tri,None,&mut mappings,&mut reused)?;}
                if reused.len()!=source.corners.len(){return Err(invalid("decimation refuses to discard collinear authored corners during triangulation"));}
                next.check_structure(ctx)?;*m=next;
                let mut protected=BTreeSet::new();let mut first_uv=BTreeMap::new();let mut first_material=BTreeMap::new();
                for f in &m.faces{for c in m.face_corners(f.id)?{ctx.checkpoint(1)?;if first_uv.get(&c.vertex).is_some_and(|uv|*uv!=c.uv)||first_material.get(&c.vertex).is_some_and(|mat|*mat!=f.material)||m.uv_pins.contains(&c.id){protected.insert(c.vertex);}
                    first_uv.entry(c.vertex).or_insert(c.uv);first_material.entry(c.vertex).or_insert(f.material);
                }}
                for edge in m.edge_data.keys(){protected.extend([edge.0,edge.1]);}
                let mut radii=m.vertices.iter().map(|v|(v.id,0f64)).collect::<BTreeMap<_,_>>();let mut merged=BTreeMap::<VertexId,VertexId>::new();let mut collapses=0;
                // Each pass collapses every independent candidate in error order:
                // a collapse dirties both one-rings, whose adjacency is stale until
                // the next pass rebuilds it. One collapse per adjacency rebuild made
                // this quadratic (a 4.6k-face mesh took 10 s).
                while m.faces.len()>target_faces{ctx.checkpoint(1)?;let adj=m.adjacency(ctx)?;let mut candidates=Vec::new();
                    for &edge in adj.edges.keys(){ctx.checkpoint(1)?;if protected.contains(&edge.0)||protected.contains(&edge.1){continue;}let distance=length(sub(m.vertex(edge.0).unwrap().position,m.vertex(edge.1).unwrap().position));let error=radii[&edge.0].max(radii[&edge.1])+distance*0.5;
                        if error<=max_error{candidates.push((error,edge));}}
                    candidates.sort_by(|a,b|a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));let mut progressed=false;
                    let mut dirty=BTreeSet::new();
                    for (error,edge) in candidates{ctx.checkpoint(1)?;
                        if dirty.contains(&edge.0)||dirty.contains(&edge.1){continue;}
                        // Never remove more incident triangles than the target asks.
                        if m.faces.len().saturating_sub(adj.radial(edge).len())<target_faces{continue;}
                        match collapse_triangle_neighborhood(m,&adj,edge,ctx){
                            Ok(_)=>{radii.insert(edge.0,error);radii.remove(&edge.1);merged.insert(edge.1,edge.0);collapses+=1;progressed=true;
                                for v in [edge.0,edge.1]{dirty.insert(v);for e in &adj.vertex_edges[&v]{dirty.insert(e.0);dirty.insert(e.1);}}
                                if m.faces.len()<=target_faces{break;}}
                            Err(error@MeshError::Cancelled)|Err(error@MeshError::Budget{..})=>return Err(error),
                            Err(_)=>continue,
                        }
                    }
                    if !progressed{break;}
                }
                // The outer edit owns rollback. Local link/orientation checks guard
                // candidates; a full manifold qualification still gates publication.
                surface(m,ctx)?;
                // Every original vertex maps to the survivor at the end of its merge chain.
                let resolve=|mut v:VertexId|{while let Some(&next)=merged.get(&v){v=next;}v};
                mappings.extend(source.vertices.iter().map(|v|(ElementId::Vertex(v.id),vec![ElementId::Vertex(resolve(v.id))])));
                Ok(((m.faces.len(),collapses),mappings.into_iter().collect()))
            })?;Ok(DecimateResult{changes,achieved_faces,collapses})
        }
    }
    fn emit_triangle(next:&mut Mesh,source:&Mesh,triangles:&TriangleMesh,tri:&Triangle,retain:Option<FaceId>,mappings:&mut BTreeMap<ElementId,Vec<ElementId>>,reused:&mut BTreeSet<CornerId>)->Result<()> {
        let face=match retain{Some(id)=>id,None=>FaceId(next.allocate_id()?)};let offset=u32::try_from(next.corners.len()).map_err(|_|invalid("decimation corner offset overflow"))?;
        next.faces.push(Face{id:face,first_corner:offset,corner_count:3,material:tri.material});mappings.entry(ElementId::Face(tri.source_face)).or_default().push(ElementId::Face(face));
        for index in tri.indices{let v=&triangles.vertices[index as usize];let id=if reused.insert(v.source_corner){v.source_corner}else{CornerId(next.allocate_id()?)};next.corners.push(Corner{id,vertex:v.source_vertex,uv:v.uv,normal:Some(v.normal)});
            mappings.entry(ElementId::Corner(v.source_corner)).or_default().push(ElementId::Corner(id));if source.uv_pins.contains(&v.source_corner){next.uv_pins.insert(id);}
        }Ok(())
    }

    /// Internal to the already-staged decimation edit. Refused candidates never
    /// mutate the mesh. Only the one-ring is copied/validated, rather than cloning
    /// and qualifying the entire mesh for each attempted collapse.
    fn collapse_triangle_neighborhood(mesh:&mut Mesh,adj:&Adjacency,edge:EdgeKey,ctx:&mut Context<'_>)->Result<()> {
        let uses=adj.radial(edge);
        if uses.is_empty()||uses.len()>2{return Err(invalid("collapse needs a manifold edge"));}
        let neighbors=|v:VertexId|adj.vertex_edges[&v].iter().map(|e|if e.0==v{e.1}else{e.0}).collect::<BTreeSet<_>>();
        ctx.checkpoint((adj.vertex_edges[&edge.0].len()+adj.vertex_edges[&edge.1].len())as u64)?;
        let a=neighbors(edge.0);let b=neighbors(edge.1);
        let mut opposite=BTreeSet::new();
        for use_ in uses{for c in mesh.face_corners(use_.face)?{ctx.checkpoint(1)?;if c.vertex!=edge.0&&c.vertex!=edge.1{opposite.insert(c.vertex);}}}
        if a.intersection(&b).copied().collect::<BTreeSet<_>>()!=opposite{return Err(invalid("collapse violates triangle link condition"));}
        let boundary=|v|adj.vertex_edges[&v].iter().any(|e|adj.radial(*e).len()==1);
        // An interior edge joining two boundary vertices would pinch the boundary.
        if uses.len()==2&&boundary(edge.0)&&boundary(edge.1){return Err(invalid("collapse would pinch the boundary"));}
        let (position,weights)=blended_vertex(mesh,edge.0,edge.1,0.5,ctx)?;
        let affected=adj.vertex_faces[&edge.0].iter().chain(&adj.vertex_faces[&edge.1]).copied().collect::<BTreeSet<_>>();
        let mut remove=BTreeSet::new();let mut replacements=BTreeMap::new();
        let mut patch_positions=Vec::new();let mut patch_vertices=BTreeMap::new();let mut polygons=Vec::new();let mut old_normals=Vec::new();
        for face in affected{
            ctx.checkpoint(1)?;let source=mesh.face_corners(face)?;
            if source.len()!=3{return Err(invalid("decimation collapse requires triangles"));}
            if source.iter().any(|c|c.vertex==edge.0)&&source.iter().any(|c|c.vertex==edge.1){remove.insert(face);continue;}
            old_normals.push(face_geometry(mesh,face,ctx)?.normal);
            let mut corners=source.to_vec();let mut indices=Vec::new();
            for c in &mut corners{
                ctx.checkpoint(1)?;if c.vertex==edge.1{c.vertex=edge.0;}c.normal=None;
                let next=patch_vertices.len()as u32;
                let index=*patch_vertices.entry(c.vertex).or_insert_with(||{patch_positions.push(if c.vertex==edge.0{position}else{mesh.vertex(c.vertex).unwrap().position});next});
                indices.push(index);
            }
            polygons.push(Polygon{vertices:indices,uvs:corners.iter().map(|c|c.uv).collect(),material:mesh.face(face).unwrap().material});
            replacements.insert(face,corners);
        }
        ctx.bytes(mesh.memory_bytes().saturating_mul(2).saturating_add(patch_positions.len().saturating_mul(2048)).saturating_add(polygons.len().saturating_mul(2048)))?;
        let patch=Mesh::from_polygons(&patch_positions,&polygons,ctx)?;
        for (face,normal) in patch.faces.iter().zip(old_normals){
            if dot(normal,face_geometry(&patch,face.id,ctx)?.normal)<=1e-8{return Err(invalid("collapse would reverse a face"));}
        }
        surface(&patch,ctx)?;
        // Every fallible geometric/topological check precedes mutation. Cancellation
        // during this commit propagates to the outer edit and rolls the whole job back.
        legacy_replace_corners(mesh,&replacements,ctx)?;mesh.legacy_repack_faces(&remove,ctx)?;
        mesh.vertices.retain(|v|v.id!=edge.1);
        let i=mesh.vertices.binary_search_by_key(&edge.0,|v|v.id).unwrap();mesh.vertices[i].position=position;mesh.vertices[i].weights=weights;
        Ok(())
    }

    fn legacy_replace_corners(mesh:&mut Mesh,replacements:&BTreeMap<FaceId,Vec<Corner>>,ctx:&mut Context<'_>)->Result<()> {
        let mut corners=Vec::new();
        for face in &mut mesh.faces {
            ctx.checkpoint(1)?;
            let source=replacements.get(&face.id).map(Vec::as_slice).unwrap_or_else(||
                &mesh.corners[face.first_corner as usize..(face.first_corner+face.corner_count) as usize]);
            let start=u32::try_from(corners.len()).map_err(|_|invalid("corner count overflow"))?;
            corners.extend_from_slice(source);face.first_corner=start;face.corner_count=source.len() as u32;
        }
        mesh.corners=corners;Ok(())
    }

    impl Mesh {
        fn legacy_repack_faces(&mut self, remove: &BTreeSet<FaceId>, ctx: &mut Context<'_>) -> Result<()> {
            let mut faces = Vec::with_capacity(self.faces.len() - remove.len());
            let mut corners = Vec::with_capacity(self.corners.len());
            for f in &self.faces {
                ctx.checkpoint(1)?;
                if !remove.contains(&f.id) {
                    let mut face = f.clone();
                    face.first_corner = corners.len() as u32;
                    corners.extend_from_slice(self.face_corners(f.id)?);
                    faces.push(face);
                }
            }
            self.faces = faces;
            self.corners = corners;
            self.prune_edge_attributes();
            Ok(())
        }
    }
}


mod normal_staging {
    use crate::{geometry::*, *};
    use makepad_csg_math::portable::PortableFloat;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    #[test]
    fn staged_normals_match_original_edit_and_fail_atomically() {
        let mut ctx=Context::default();
        let mut cube=Mesh::cube([2.;3],&mut ctx).unwrap();
        cube.set_vertex_weights(cube.vertices()[0].id,&[JointWeight{joint:3,weight:1.}],&mut ctx).unwrap();
        cube.pin_uv(&[cube.corners()[0].id],true,&mut ctx).unwrap();
        let edges=cube.adjacency(&mut ctx).unwrap().edges.keys().copied().collect::<Vec<_>>();
        cube.set_edge_attributes(edges[0],EdgeAttributes{seam:true,crease:0.},&mut ctx).unwrap();
        cube.set_edge_attributes(edges[1],EdgeAttributes{seam:false,crease:1.},&mut ctx).unwrap();
        // Uneven fan normals expose changes in BFS floating-point sum order.
        let positions=[[0.,0.,0.3],[1.,0.,0.],[1.,1.,0.2],[0.,1.,0.],[-1.,1.,-0.1],
            [-1.,0.,0.],[-1.,-1.,0.4],[0.,-1.,0.],[1.,-1.,-0.2]];
        let polygons=(1..=8).map(|i|Polygon::new(vec![0,i,i%8+1])).collect::<Vec<_>>();
        let fan=Mesh::from_polygons(&positions,&polygons,&mut ctx).unwrap();
        for source in [&cube,&fan] {
            for (smooth,angle) in [(false,0.),(true,0.7),(true,std::f64::consts::FRAC_PI_2),(true,std::f64::consts::PI)] {
                let mut actual=source.clone();let mut expected=source.clone();
                let changes=actual.recalculate_normals(smooth,angle,&mut Context::default()).unwrap();
                let (_,reference_changes)=expected.edit(&mut Context::default(),source.corners.len()*256,|mesh,ctx|{
                    derive_reference(mesh,smooth,angle,ctx)?;Ok(((),Vec::new()))
                }).unwrap();
                assert_eq!(changes,reference_changes,"normal edits retain every element ID");
                assert_eq!(actual,expected,"normal staging must preserve authored mesh data");
                for (a,b) in actual.corners().iter().zip(expected.corners()) {
                    assert_eq!(a.normal.unwrap().map(f64::to_bits),b.normal.unwrap().map(f64::to_bits),"corner normal bits differ");
                }
                assert_eq!(actual.to_bytes(&mut Context::default()).unwrap(),expected.to_bytes(&mut Context::default()).unwrap());
            }
        }
        let calls=std::cell::Cell::new(0usize);
        let count=||{calls.set(calls.get()+1);false};
        let mut probe=cube.clone();let mut measured=Context::new(Limits::default(),Some(&count));
        probe.recalculate_normals(true,std::f64::consts::PI,&mut measured).unwrap();
        for stop in [1,calls.get()/2,calls.get()] {
            let mut mesh=cube.clone();let seen=std::cell::Cell::new(0usize);
            let cancel=||{seen.set(seen.get()+1);seen.get()==stop};
            assert_eq!(mesh.recalculate_normals(true,std::f64::consts::PI,&mut Context::new(Limits::default(),Some(&cancel))),Err(MeshError::Cancelled));
            assert_eq!(mesh,cube,"cancellation at checkpoint {stop} changed the mesh");
        }
        for limits in [Limits{max_bytes:1}] {
            let mut mesh=cube.clone();
            assert!(matches!(mesh.recalculate_normals(true,std::f64::consts::PI,&mut Context::new(limits,None)),Err(MeshError::Budget{..})));
            assert_eq!(mesh,cube,"budget failure changed the mesh");
        }
    }

    // Original edge-scanning derivation, used inside the original Mesh::edit
    // path above so both normal bytes and retained provenance are compared.
    fn derive_reference(mesh:&mut Mesh,smooth:bool,angle:f64,ctx:&mut Context<'_>)->Result<()> {
        let adj=mesh.adjacency(ctx)?;let normals=mesh.faces.iter().map(|f|Ok((f.id,face_geometry(mesh,f.id,ctx)?.normal))).collect::<Result<BTreeMap<_,_>>>()?;
        let mut results=BTreeMap::new();let cosine=angle.pcos();
        for (&vertex,faces) in &adj.vertex_faces{let mut remaining=faces.iter().copied().collect::<BTreeSet<_>>();
            while let Some(&seed)=remaining.first(){let mut queue=VecDeque::from([seed]);remaining.remove(&seed);let mut fan=Vec::new();let mut n=[0.;3];
                while let Some(face)=queue.pop_front(){ctx.checkpoint(1)?;fan.push(face);n=add(n,normals[&face]);if !smooth{continue;}
                    for &edge in &adj.vertex_edges[&vertex]{ctx.checkpoint(1)?;let uses=adj.radial(edge);
                        if uses.len()!=2||mesh.edge_data.get(&edge).is_some_and(|d|d.attributes.crease>0.)||!uses.iter().any(|u|u.face==face){continue;}
                        let other=uses.iter().find(|u|u.face!=face).unwrap().face;
                        if dot(normals[&face],normals[&other])+1e-12>=cosine&&remaining.remove(&other){queue.push_back(other);}
                    }
                }
                let len=length(n);for face in fan{results.insert((vertex,face),if len>1e-12{mul(n,1./len)}else{normals[&face]});}
            }
        }
        for face in &mesh.faces{for c in &mut mesh.corners[face.first_corner as usize..(face.first_corner+face.corner_count)as usize]{ctx.checkpoint(1)?;c.normal=Some(results[&(c.vertex,face.id)]);}}
        Ok(())
    }
}
